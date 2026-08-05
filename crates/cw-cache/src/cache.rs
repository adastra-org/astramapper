//! The facade: one type tying keys, packs and the index together.
//!
//! # The rule everything here obeys
//!
//! **A cache failure may never fail the operation that asked for it.** Every
//! artifact in here is derived data the caller can rebuild, so a corrupt table,
//! a full disk or a torn pack must degrade to "compile it again", never to an
//! error the user sees. That is why [`Cache::get`] returns `Option` rather than
//! `Result`, why [`Cache::get_or_insert_with`] swallows write failures into a
//! counter, and why a poisoned lock is taken anyway rather than propagated: a
//! panic in one thread must not disable caching for the rest of the session.
//!
//! # What `open` decides
//!
//! A mismatched [`FORMAT_VERSION`] or install stamp **wipes the folder**. There
//! is no migration path and there should not be one: a migration is a bug
//! surface with a silent failure mode (serving one asset's bytes for another's)
//! guarding data that costs one bake to rebuild.
//!
//! # Reads, writes and the gap between them
//!
//! Packs are mapped for reading and appended to through a separate handle, so a
//! record written after a mapping was taken falls outside it. [`Cache::get`]
//! notices, remaps that pack once and retries. This is also exactly what a torn
//! append looks like, and it resolves the same way: the retry fails, the lookup
//! is a miss, and the artifact is rebuilt.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::{Deserialize, Serialize};

use crate::index::{self, Index, Record};
use crate::key::{CacheKey, Kind, FORMAT_VERSION};
use crate::pack::{self, Pack, PackWriter, PINNED};
use crate::{Error, Result};

/// How much journal accumulates before the table is rewritten.
///
/// At 36 bytes a record this is about 116,000 writes, and a rewrite of the whole
/// table at that point costs a few megabytes of sequential IO. Compacting more
/// often would pay that repeatedly during a bake for no benefit; less often
/// makes the next open slower.
const JOURNAL_LIMIT: u64 = 4 * 1024 * 1024;

/// The smallest budget worth honouring.
///
/// Below this the cache thrashes: a single region of the city is hundreds of
/// megabytes of compiled models, so a smaller budget would evict the packs it
/// just wrote and turn every pass into a re-bake.
pub const MIN_BUDGET: u64 = 1024 * 1024 * 1024;

/// The default budget, matching the figure in the phase 4 plan.
pub const DEFAULT_BUDGET: u64 = 8 * 1024 * 1024 * 1024;

/// What the folder claims to be, checked on every open.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
    install_stamp: u64,
    /// Recorded so the folder is legible on its own. Not enforced: changing the
    /// budget must not throw the cache away.
    budget_bytes: u64,
}

/// A snapshot of what the cache holds. Cheap enough to poll for a UI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CacheStats {
    pub bytes: u64,
    pub budget_bytes: u64,
    pub records: usize,
    pub packs: usize,
    pub hits: u64,
    pub misses: u64,
    /// Writes that failed and were dropped. Non-zero means the cache is degraded
    /// but the app is still correct.
    pub write_errors: u64,
}

/// Everything guarded by one lock: the record table and the open mappings.
#[derive(Debug)]
struct State {
    index: Index,
    packs: HashMap<u32, Arc<Pack>>,
    /// Total pack bytes on disk, tracked incrementally so the budget check does
    /// not stat the folder on every write.
    bytes: u64,
}

/// A pack-file cache of compiled assets, keyed by source.
#[derive(Debug)]
pub struct Cache {
    dir: PathBuf,
    install_stamp: u64,
    budget: u64,
    state: RwLock<State>,
    /// The rolling pack, and the pinned pack, as two handles rather than one
    /// that switches. Switching would leave the writer pointed at the wrong pack
    /// if the write between the two switches failed.
    rolling: Mutex<PackWriter>,
    pinned: Mutex<PackWriter>,
    /// Monotonic, and the only source of recency for eviction.
    clock: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    write_errors: AtomicU64,
}

impl Cache {
    /// Open, or start, a cache in `dir`.
    ///
    /// `install_stamp` identifies the game folder these artifacts were compiled
    /// from; a different one wipes the cache. `budget_bytes` is clamped up to
    /// [`MIN_BUDGET`].
    pub fn open(dir: &Path, install_stamp: u64, budget_bytes: u64) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io {
            what: "create the cache directory",
            source: e,
        })?;
        let budget = budget_bytes.max(MIN_BUDGET);

        let usable = read_manifest(dir).is_some_and(|manifest| {
            manifest.format_version == FORMAT_VERSION && manifest.install_stamp == install_stamp
        });
        if !usable {
            wipe(dir)?;
        }
        write_manifest(
            dir,
            &Manifest {
                format_version: FORMAT_VERSION,
                install_stamp,
                budget_bytes: budget,
            },
        )?;

        // Every pack is mapped up front rather than on first read. There are at
        // most a few dozen at any budget, and a pack that is not mapped cannot
        // be an eviction candidate — which during a long bake would mean the
        // budget is only ever enforced against the one pack being written.
        let ids = pack_ids(dir);
        let mut packs = HashMap::new();
        let mut bytes = 0;
        for id in &ids {
            bytes += file_len(&dir.join(pack::pack_name(*id)));
            if let Some(pack) = Pack::open(dir, *id)? {
                packs.insert(*id, Arc::new(pack));
            }
        }

        let mut table = Index::load(dir)?;
        // The journal entry can land while the pack append does not. Drop
        // anything the packs cannot actually back, and fold the result into the
        // table so the journal stops replaying records that are already gone.
        if table.drop_unbacked(|id| packs.get(&id).map(|pack| pack.len())) > 0 {
            index::compact(dir, &mut table)?;
        }

        // Append to the newest pack rather than starting one, so a session that
        // wrote a kilobyte does not leave a pack behind. Ids only ever climb, so
        // an evicted pack's number is never handed out again while its records
        // could still be in the journal.
        let newest = ids
            .iter()
            .copied()
            .filter(|id| *id != PINNED)
            .max()
            .unwrap_or(0);

        Ok(Self {
            dir: dir.to_path_buf(),
            install_stamp,
            budget,
            state: RwLock::new(State {
                index: table,
                packs,
                bytes,
            }),
            rolling: Mutex::new(PackWriter::open(dir, newest)?),
            pinned: Mutex::new(PackWriter::open(dir, PINNED)?),
            clock: AtomicU64::new(1),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
        })
    }

    /// The bytes stored under `key`, or `None` for anything at all going wrong.
    ///
    /// One memcpy out of the mapping: the caller owns the result, which is what
    /// every caller needs anyway to hand it across the IPC boundary.
    pub fn get(&self, key: CacheKey) -> Option<Vec<u8>> {
        let found = self.lookup(key);
        let counter = if found.is_some() {
            &self.hits
        } else {
            &self.misses
        };
        counter.fetch_add(1, Ordering::Relaxed);
        found
    }

    fn lookup(&self, key: CacheKey) -> Option<Vec<u8>> {
        let clock = self.tick();
        let (record, mapped) = {
            let state = self.read_state();
            let record = *state.index.records.get(&key)?;
            (record, state.packs.get(&record.pack).cloned())
        };

        if let Some(pack) = &mapped {
            if let Some(bytes) = pack.read(record.offset, record.length, clock) {
                return Some(bytes.to_vec());
            }
        }

        // Not mapped, or mapped before this record was appended. One remap, one
        // retry; a second failure means the append was torn and the artifact is
        // gone, which is a miss and not an error.
        let refreshed = self.remap(record.pack, clock)?;
        let bytes = refreshed.read(record.offset, record.length, clock)?;
        Some(bytes.to_vec())
    }

    /// Re-open a pack whose file has outgrown its mapping.
    fn remap(&self, id: u32, clock: u64) -> Option<Arc<Pack>> {
        let on_disk = file_len(&self.dir.join(pack::pack_name(id)));
        let mut state = self.write_state();
        if let Some(existing) = state.packs.get(&id) {
            // Another thread may have remapped it while this one waited.
            if existing.len() >= on_disk {
                return Some(existing.clone());
            }
        }
        let pack = Arc::new(Pack::open(&self.dir, id).ok()??);
        pack.touch(clock);
        state.packs.insert(id, pack.clone());
        Some(pack)
    }

    /// Store `bytes` under `key`, evicting if that puts the cache over budget.
    pub fn put(&self, key: CacheKey, kind: Kind, bytes: &[u8]) -> Result<()> {
        // A record's length is a `u32` on disk. Nothing this project compiles
        // comes close, and refusing is the right answer rather than truncating
        // the length into a record that would read back as garbage.
        if bytes.len() > u32::MAX as usize {
            return Ok(());
        }
        let clock = self.tick();

        let (pack, offset, finished) = {
            let mut writer = if kind.is_pinned() {
                lock(&self.pinned)
            } else {
                lock(&self.rolling)
            };
            let before = writer.id();
            let (pack, offset) = writer.append(bytes)?;
            (pack, offset, (pack != before).then_some(before))
        };

        let record = Record {
            key,
            pack,
            offset,
            length: bytes.len() as u32,
            kind: kind as u8,
        };
        let appended = index::append(&self.dir, &record)?;

        let mut state = self.write_state();
        state.index.records.insert(key, record);
        state.index.journal_len += appended;
        state.bytes += bytes.len() as u64;

        // A pack that just filled up is never read again through the writer, so
        // map it now and count the mapping as use. Left unmapped it could not be
        // evicted; touched with the current clock it is the last thing evicted,
        // which is right, because it holds the newest work.
        if let Some(id) = finished {
            if let Ok(Some(full)) = Pack::open(&self.dir, id) {
                full.touch(clock);
                state.packs.insert(id, Arc::new(full));
            }
        }

        if state.index.journal_len > JOURNAL_LIMIT {
            index::compact(&self.dir, &mut state.index)?;
        }
        self.evict(&mut state, pack);
        Ok(())
    }

    /// The cached bytes, or the result of compiling them and storing that.
    ///
    /// `key` is an `Option` because [`CacheKey::of`] refuses inputs it cannot
    /// trust. Taking it this way means the uncacheable case still runs the
    /// compiler through this one call, instead of leaving each caller to write
    /// its own bypass branch and forget the fallback in one of them.
    ///
    /// A failed write is counted and dropped: the compiled bytes are correct
    /// whether or not they reached the disk.
    pub fn get_or_insert_with<F, E>(
        &self,
        key: Option<CacheKey>,
        kind: Kind,
        compile: F,
    ) -> std::result::Result<Vec<u8>, E>
    where
        F: FnOnce() -> std::result::Result<Vec<u8>, E>,
    {
        if let Some(key) = key {
            if let Some(bytes) = self.get(key) {
                return Ok(bytes);
            }
        }
        let bytes = compile()?;
        if let Some(key) = key {
            if self.put(key, kind, &bytes).is_err() {
                self.write_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(bytes)
    }

    /// Fold the journal into the table. Call on shutdown; otherwise it happens
    /// on its own once the journal passes [`JOURNAL_LIMIT`].
    pub fn compact(&self) -> Result<()> {
        let mut state = self.write_state();
        index::compact(&self.dir, &mut state.index)
    }

    pub fn stats(&self) -> CacheStats {
        let state = self.read_state();
        CacheStats {
            bytes: state.bytes,
            budget_bytes: self.budget,
            records: state.index.records.len(),
            packs: state.packs.len(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
        }
    }

    /// Throw everything away and start again, without reopening the cache.
    pub fn clear(&self) -> Result<()> {
        let mut rolling = lock(&self.rolling);
        let mut pinned = lock(&self.pinned);
        let mut state = self.write_state();

        // Unmap before deleting. Windows refuses to remove a mapped file.
        state.packs.clear();
        state.index = Index::default();
        state.bytes = 0;
        wipe(&self.dir)?;

        *rolling = PackWriter::open(&self.dir, 0)?;
        *pinned = PackWriter::open(&self.dir, PINNED)?;
        write_manifest(
            &self.dir,
            &Manifest {
                format_version: FORMAT_VERSION,
                install_stamp: self.install_stamp,
                budget_bytes: self.budget,
            },
        )
    }

    /// Drop whole packs, least recently used first, until inside the budget.
    ///
    /// `protect` is the pack being written to. Without it the newest pack, which
    /// nothing has read yet, would be the first candidate every time.
    fn evict(&self, state: &mut State, protect: u32) {
        while state.bytes > self.budget {
            let victim = state
                .packs
                .values()
                .filter(|pack| pack.id != PINNED && pack.id != protect)
                // Ties broken by id so eviction is deterministic rather than
                // following hash order.
                .min_by_key(|pack| (pack.last_hit(), pack.id))
                .map(|pack| pack.id);
            let Some(victim) = victim else { break };

            let freed = state.packs.remove(&victim).map_or(0, |pack| pack.len());
            state.index.drop_pack(victim);
            state.bytes = state.bytes.saturating_sub(freed);
            // A concurrent reader may still hold this mapping, and Windows will
            // not delete a mapped file. Leaving the file is safe: its records
            // are gone from the index, so the next open sees a pack nothing
            // references and evicts it first.
            let _ = std::fs::remove_file(self.dir.join(pack::pack_name(victim)));
        }
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// A poisoned lock means some other thread panicked, not that this data is
    /// wrong. Disabling the cache for the session over it would be worse.
    fn read_state(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write_state(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    let bytes = std::fs::read(dir.join("manifest.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest).expect("a manifest always serializes");
    std::fs::write(dir.join("manifest.json"), bytes).map_err(|e| Error::Io {
        what: "write the cache manifest",
        source: e,
    })
}

/// Remove everything the cache owns, and nothing else.
///
/// Named files rather than `remove_dir_all`, because the directory is given to
/// us and this must not become a way to delete whatever the caller pointed at.
fn wipe(dir: &Path) -> Result<()> {
    for name in ["manifest.json", "index.bin", "index.bin.new", "index.log"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    for id in pack_ids(dir) {
        let _ = std::fs::remove_file(dir.join(pack::pack_name(id)));
    }
    Ok(())
}

fn pack_ids(dir: &Path) -> Vec<u32> {
    let mut ids = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return ids;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix("pack.")) else {
            continue;
        };
        if rest == "pinned" {
            ids.push(PINNED);
        } else if let Ok(id) = rest.parse::<u32>() {
            if id != PINNED {
                ids.push(id);
            }
        }
    }
    ids.sort_unstable();
    ids
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::KeyInputs;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cw-cache-facade-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn open(&self, stamp: u64) -> Cache {
            Cache::open(&self.0, stamp, MIN_BUDGET).expect("open")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn key(n: u32) -> CacheKey {
        CacheKey::of(KeyInputs {
            kind: Kind::Model,
            archive_stamp: 0xabc,
            entry_header: (n, 64),
            entry_path: "x64a.rpf/props/prop.ydr",
            params: 0,
        })
        .expect("key")
    }

    #[test]
    fn stores_and_reads_back_within_one_session() {
        let scratch = Scratch::new("round-trip");
        let cache = scratch.open(1);
        cache.put(key(1), Kind::Model, b"compiled model").expect("put");
        assert_eq!(cache.get(key(1)).as_deref(), Some(&b"compiled model"[..]));
    }

    /// The whole point: bytes written by one run are served by the next. This
    /// is what makes the key's stability across builds load-bearing.
    #[test]
    fn survives_a_reopen() {
        let scratch = Scratch::new("reopen");
        {
            let cache = scratch.open(7);
            cache.put(key(1), Kind::Model, b"first").expect("put");
            cache.put(key(2), Kind::Texture, b"second").expect("put");
        }
        let cache = scratch.open(7);
        assert_eq!(cache.get(key(1)).as_deref(), Some(&b"first"[..]));
        assert_eq!(cache.get(key(2)).as_deref(), Some(&b"second"[..]));
    }

    #[test]
    fn an_unknown_key_is_a_miss_and_not_an_error() {
        let scratch = Scratch::new("miss");
        let cache = scratch.open(1);
        assert_eq!(cache.get(key(99)), None);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().hits, 0);
    }

    #[test]
    fn a_different_install_wipes_the_cache() {
        let scratch = Scratch::new("install");
        {
            let cache = scratch.open(7);
            cache.put(key(1), Kind::Model, b"from install seven").expect("put");
        }
        let cache = scratch.open(8);
        assert_eq!(cache.get(key(1)), None, "another install must not be served");
        assert_eq!(cache.stats().records, 0);
        assert_eq!(cache.stats().bytes, 0);
    }

    /// A format bump has to behave like a wipe, not like a migration and not
    /// like a read of bytes written under different rules.
    #[test]
    fn a_format_mismatch_wipes_the_cache() {
        let scratch = Scratch::new("format");
        {
            let cache = scratch.open(7);
            cache.put(key(1), Kind::Model, b"old format").expect("put");
        }
        let manifest = scratch.0.join("manifest.json");
        let text = std::fs::read_to_string(&manifest).expect("read");
        std::fs::write(
            &manifest,
            text.replace(
                &format!("\"format_version\": {FORMAT_VERSION}"),
                "\"format_version\": 99",
            ),
        )
        .expect("write");

        let cache = scratch.open(7);
        assert_eq!(cache.get(key(1)), None);
        assert_eq!(cache.stats().records, 0);
    }

    #[test]
    fn get_or_insert_with_compiles_once() {
        let scratch = Scratch::new("insert");
        let cache = scratch.open(1);
        let mut compiles = 0;
        for _ in 0..3 {
            let bytes = cache
                .get_or_insert_with(Some(key(1)), Kind::Model, || {
                    compiles += 1;
                    Ok::<_, ()>(b"compiled".to_vec())
                })
                .expect("insert");
            assert_eq!(bytes, b"compiled");
        }
        assert_eq!(compiles, 1, "later calls must come from the cache");
    }

    /// An input the key refuses still has to produce the artifact, every time.
    #[test]
    fn get_or_insert_with_still_compiles_without_a_key() {
        let scratch = Scratch::new("keyless");
        let cache = scratch.open(1);
        let mut compiles = 0;
        for _ in 0..3 {
            cache
                .get_or_insert_with(None, Kind::Model, || {
                    compiles += 1;
                    Ok::<_, ()>(b"compiled".to_vec())
                })
                .expect("insert");
        }
        assert_eq!(compiles, 3);
        assert_eq!(cache.stats().records, 0, "nothing was cached");
    }

    #[test]
    fn a_compiler_error_reaches_the_caller() {
        let scratch = Scratch::new("compile-error");
        let cache = scratch.open(1);
        let result: std::result::Result<Vec<u8>, &str> =
            cache.get_or_insert_with(Some(key(1)), Kind::Model, || Err("no such entry"));
        assert_eq!(result, Err("no such entry"));
        assert_eq!(cache.stats().records, 0);
    }

    /// Records written after a pack was mapped fall outside the mapping. The
    /// read has to notice and remap rather than report a miss for bytes that
    /// are demonstrably on disk.
    #[test]
    fn reads_records_written_after_the_mapping_was_taken() {
        let scratch = Scratch::new("remap");
        {
            let cache = scratch.open(1);
            cache.put(key(1), Kind::Model, b"before").expect("put");
        }
        // Opening maps the pack at its current length; this write extends it.
        let cache = scratch.open(1);
        cache.put(key(2), Kind::Model, b"after the mapping").expect("put");
        assert_eq!(cache.get(key(2)).as_deref(), Some(&b"after the mapping"[..]));
        assert_eq!(cache.get(key(1)).as_deref(), Some(&b"before"[..]));
    }

    /// The bake being killed is the normal case. The journal entry survives and
    /// the bytes do not, and that must read as a miss.
    #[test]
    fn a_record_whose_bytes_did_not_survive_is_dropped_on_open() {
        let scratch = Scratch::new("torn");
        {
            let cache = scratch.open(1);
            cache.put(key(1), Kind::Model, b"intact").expect("put");
            cache.put(key(2), Kind::Model, b"lost to a kill").expect("put");
        }
        // Roll the pack back to before the second write, leaving its journal
        // entry in place.
        let pack = scratch.0.join(pack::pack_name(0));
        let bytes = std::fs::read(&pack).expect("read");
        std::fs::write(&pack, &bytes[..b"intact".len()]).expect("truncate");

        let cache = scratch.open(1);
        assert_eq!(cache.get(key(1)).as_deref(), Some(&b"intact"[..]));
        assert_eq!(cache.get(key(2)), None);
        assert_eq!(cache.stats().records, 1);
    }

    #[test]
    fn clearing_leaves_an_empty_but_usable_cache() {
        let scratch = Scratch::new("clear");
        let cache = scratch.open(1);
        cache.put(key(1), Kind::Model, b"gone soon").expect("put");
        cache.clear().expect("clear");

        assert_eq!(cache.get(key(1)), None);
        assert_eq!(cache.stats().records, 0);
        assert_eq!(cache.stats().bytes, 0);

        cache.put(key(2), Kind::Model, b"after clearing").expect("put");
        assert_eq!(cache.get(key(2)).as_deref(), Some(&b"after clearing"[..]));
    }

    #[test]
    fn compaction_keeps_every_record_readable() {
        let scratch = Scratch::new("compact");
        let cache = scratch.open(1);
        for n in 0..50 {
            cache.put(key(n), Kind::Model, &[n as u8; 32]).expect("put");
        }
        cache.compact().expect("compact");
        assert!(!scratch.0.join("index.log").exists());
        drop(cache);

        let cache = scratch.open(1);
        for n in 0..50 {
            assert_eq!(cache.get(key(n)).as_deref(), Some(&[n as u8; 32][..]), "{n}");
        }
    }

    /// Eviction is whole-pack and least recently used, and it must never take
    /// the pack being written to.
    #[test]
    fn eviction_drops_the_coldest_pack_and_spares_the_live_one() {
        let scratch = Scratch::new("evict");
        let cache = Cache::open(&scratch.0, 1, MIN_BUDGET).expect("open");
        // Fake three finished packs rather than writing 1.5 GB.
        {
            let mut state = cache.write_state();
            for id in 0..3u32 {
                std::fs::write(scratch.0.join(pack::pack_name(id)), [id as u8; 16])
                    .expect("write a pack");
                let pack = Pack::open(&scratch.0, id).expect("open").expect("mapped");
                pack.touch(10 - id as u64); // pack 2 is the coldest
                state.packs.insert(id, Arc::new(pack));
                state.index.records.insert(
                    key(id),
                    Record {
                        key: key(id),
                        pack: id,
                        offset: 0,
                        length: 16,
                        kind: Kind::Model as u8,
                    },
                );
            }
            state.bytes = MIN_BUDGET + 16;
            cache.evict(&mut state, 0);
        }

        let state = cache.read_state();
        assert!(!state.packs.contains_key(&2), "the coldest pack goes");
        assert!(state.packs.contains_key(&0), "the live pack stays");
        assert!(state.packs.contains_key(&1));
        assert!(
            !state.index.records.contains_key(&key(2)),
            "its records go with it"
        );
        assert!(!scratch.0.join(pack::pack_name(2)).exists());
    }

    /// The indexes are what make a warm start fast. Evicting them to fit one
    /// more prop would be exactly backwards.
    #[test]
    fn eviction_never_takes_the_pinned_pack() {
        let scratch = Scratch::new("pinned");
        let cache = Cache::open(&scratch.0, 1, MIN_BUDGET).expect("open");
        cache.put(key(1), Kind::Index, b"the archive index").expect("put");
        {
            let mut state = cache.write_state();
            state.bytes = MIN_BUDGET * 4;
            cache.evict(&mut state, 0);
        }
        assert_eq!(cache.get(key(1)).as_deref(), Some(&b"the archive index"[..]));
    }

    #[test]
    fn pinned_records_go_to_the_pinned_pack() {
        let scratch = Scratch::new("pinned-pack");
        let cache = scratch.open(1);
        cache.put(key(1), Kind::Index, b"pinned").expect("put");
        cache.put(key(2), Kind::Model, b"rolling").expect("put");

        let state = cache.read_state();
        assert_eq!(state.index.records[&key(1)].pack, PINNED);
        assert_eq!(state.index.records[&key(2)].pack, 0);
    }

    #[test]
    fn the_budget_is_clamped_to_something_workable() {
        let scratch = Scratch::new("budget");
        let cache = Cache::open(&scratch.0, 1, 4096).expect("open");
        assert_eq!(cache.stats().budget_bytes, MIN_BUDGET);
    }

    /// Reopening with a different budget is a setting change, not a reason to
    /// throw away gigabytes of correct artifacts.
    #[test]
    fn changing_the_budget_keeps_the_contents() {
        let scratch = Scratch::new("rebudget");
        {
            let cache = scratch.open(1);
            cache.put(key(1), Kind::Model, b"still here").expect("put");
        }
        let cache = Cache::open(&scratch.0, 1, DEFAULT_BUDGET).expect("open");
        assert_eq!(cache.get(key(1)).as_deref(), Some(&b"still here"[..]));
    }

    #[test]
    fn stats_count_hits_and_misses() {
        let scratch = Scratch::new("stats");
        let cache = scratch.open(1);
        cache.put(key(1), Kind::Model, b"1234567890").expect("put");
        cache.get(key(1));
        cache.get(key(1));
        cache.get(key(2));

        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses), (2, 1));
        assert_eq!(stats.records, 1);
        assert_eq!(stats.bytes, 10);
        assert_eq!(stats.write_errors, 0);
    }

    /// Cheap proof that the locking holds up, and that no two writers hand out
    /// the same offset.
    #[test]
    fn concurrent_writers_do_not_overlap() {
        let scratch = Scratch::new("threads");
        let cache = Arc::new(scratch.open(1));
        std::thread::scope(|scope| {
            for thread in 0..4u32 {
                let cache = Arc::clone(&cache);
                scope.spawn(move || {
                    for n in 0..25u32 {
                        let id = thread * 100 + n;
                        cache
                            .put(key(id), Kind::Model, &id.to_le_bytes())
                            .expect("put");
                    }
                });
            }
        });

        assert_eq!(cache.stats().records, 100);
        for thread in 0..4u32 {
            for n in 0..25u32 {
                let id = thread * 100 + n;
                assert_eq!(
                    cache.get(key(id)).as_deref(),
                    Some(&id.to_le_bytes()[..]),
                    "{id}"
                );
            }
        }
    }
}
