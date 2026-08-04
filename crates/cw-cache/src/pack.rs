//! Where the bytes live: append-only pack files, read through a mapping.
//!
//! **One file per asset was the obvious design and the wrong one.** A baked
//! install is on the order of 400,000 artifacts; that many small files on NTFS
//! means 400,000 MFT records, a directory enumeration that takes minutes, and
//! Windows Defender opening each one. A key-value store (sled, redb) solves it
//! but brings a large dependency and a second failure mode for data that is
//! regenerable by definition. One flat file with no index cannot evict.
//!
//! So: packs, each capped, rolled over when full, memory-mapped for reads. A
//! hit is a slice of a mapping plus one memcpy into the buffer the caller owns.
//!
//! **Eviction is whole-pack.** Packs are append-only, so evicting one record
//! leaves a hole nothing can reclaim without a compactor, and a compactor is a
//! background rewrite of gigabytes to recover space that costs nothing to leave
//! alone. Dropping the least recently used *pack* is coarse — it takes some
//! live records with it — but those re-bake, and the alternative is machinery
//! that can corrupt the store.
//!
//! Nothing here fsyncs. A torn write costs one re-bake, and the index replay
//! discards any record whose bytes did not survive; paying a flush per write to
//! protect derived data would be the wrong trade.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use memmap2::Mmap;

use crate::{Error, Result};

/// How large one pack grows before the next is started.
///
/// Big enough that a full bake is tens of packs rather than thousands, small
/// enough that evicting one is a proportionate amount of lost work.
pub const PACK_CAP: u64 = 512 * 1024 * 1024;

/// The pack that eviction never touches. See [`crate::Kind::is_pinned`].
pub const PINNED: u32 = u32::MAX;

pub fn pack_name(id: u32) -> String {
    if id == PINNED {
        "pack.pinned".to_string()
    } else {
        format!("pack.{id:04}")
    }
}

/// One pack open for reading, and its recency.
#[derive(Debug)]
pub struct Pack {
    pub id: u32,
    map: Mmap,
    /// Bumped on every hit. Eviction drops the pack with the lowest value, so
    /// this is an LRU clock and not a hit count.
    last_hit: AtomicU64,
}

impl Pack {
    /// Map a pack for reading. A zero-length pack is not mappable and is not an
    /// error — it simply holds nothing yet.
    pub fn open(dir: &Path, id: u32) -> Result<Option<Self>> {
        let path = dir.join(pack_name(id));
        let Ok(file) = File::open(&path) else {
            return Ok(None);
        };
        let length = file.metadata().map(|m| m.len()).unwrap_or(0);
        if length == 0 {
            return Ok(None);
        }
        // Safety: the cache directory is ours. Another process editing a pack
        // underneath us is outside what any mapping-based design can defend
        // against, and the consequence is a re-bake rather than wrong game data,
        // because the index records are validated against the pack's length.
        let map = unsafe { Mmap::map(&file) }.map_err(|e| Error::Io {
            what: "map a pack",
            source: e,
        })?;
        Ok(Some(Self {
            id,
            map,
            last_hit: AtomicU64::new(0),
        }))
    }

    /// The bytes at `offset`, or `None` when the record does not fit the pack —
    /// which is exactly what a torn append looks like on the next open.
    pub fn read(&self, offset: u64, length: u32, clock: u64) -> Option<&[u8]> {
        let start = offset as usize;
        let end = start.checked_add(length as usize)?;
        let bytes = self.map.get(start..end)?;
        self.last_hit.store(clock, Ordering::Relaxed);
        Some(bytes)
    }

    pub fn len(&self) -> u64 {
        self.map.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn last_hit(&self) -> u64 {
        self.last_hit.load(Ordering::Relaxed)
    }
}

/// The single writer: appends to the newest pack and rolls over when it fills.
///
/// One writer for the whole cache, behind one mutex. Writes are appends of a
/// few hundred kilobytes and the bake pass is the only thing doing them in
/// bulk; contention here is not the bottleneck, and a lock-free design would
/// have to solve concurrent rollover for no measurable gain.
#[derive(Debug)]
pub struct PackWriter {
    dir: PathBuf,
    id: u32,
    file: File,
    offset: u64,
}

impl PackWriter {
    /// Open the newest pack for appending, or start one.
    pub fn open(dir: &Path, id: u32) -> Result<Self> {
        let path = dir.join(pack_name(id));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| Error::Io {
                what: "open a pack for writing",
                source: e,
            })?;
        let offset = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            dir: dir.to_path_buf(),
            id,
            file,
            offset,
        })
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Append bytes, rolling over to a new pack first if this one is full.
    ///
    /// Returns where they landed. A record larger than the cap still goes in
    /// whole rather than being refused: the cap bounds how much one eviction
    /// throws away, and it is not a hard limit on a single artifact.
    pub fn append(&mut self, bytes: &[u8]) -> Result<(u32, u64)> {
        if self.id != PINNED && self.offset > 0 && self.offset + bytes.len() as u64 > PACK_CAP {
            self.roll_over()?;
        }
        self.file.write_all(bytes).map_err(|e| Error::Io {
            what: "append to a pack",
            source: e,
        })?;
        let at = self.offset;
        self.offset += bytes.len() as u64;
        Ok((self.id, at))
    }

    fn roll_over(&mut self) -> Result<()> {
        let next = self.id.wrapping_add(1);
        *self = Self::open(&self.dir, next)?;
        Ok(())
    }

    /// Switch to writing a specific pack — used to send pinned records to the
    /// pinned pack without disturbing the rolling one.
    pub fn reopen(&mut self, id: u32) -> Result<()> {
        if self.id != id {
            *self = Self::open(&self.dir.clone(), id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp directory that removes itself, so a failing test does not leave
    /// megabytes behind.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cw-cache-pack-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn appends_and_reads_back() {
        let scratch = Scratch::new("round-trip");
        let mut writer = PackWriter::open(&scratch.0, 0).expect("writer");
        let (pack_a, at_a) = writer.append(b"hello").expect("append");
        let (pack_b, at_b) = writer.append(b"world!!").expect("append");
        assert_eq!((pack_a, at_a), (0, 0));
        assert_eq!((pack_b, at_b), (0, 5));
        drop(writer);

        let pack = Pack::open(&scratch.0, 0).expect("open").expect("mapped");
        assert_eq!(pack.read(at_a, 5, 1), Some(&b"hello"[..]));
        assert_eq!(pack.read(at_b, 7, 2), Some(&b"world!!"[..]));
    }

    #[test]
    fn an_absent_or_empty_pack_maps_to_nothing() {
        let scratch = Scratch::new("empty");
        assert!(Pack::open(&scratch.0, 3).expect("open").is_none());
        PackWriter::open(&scratch.0, 0).expect("writer");
        assert!(Pack::open(&scratch.0, 0).expect("open").is_none());
    }

    /// A torn append leaves an index record pointing past the pack's end. The
    /// read has to refuse it rather than panic, because that is the normal
    /// consequence of the process being killed mid-bake.
    #[test]
    fn refuses_a_record_that_runs_past_the_pack() {
        let scratch = Scratch::new("torn");
        let mut writer = PackWriter::open(&scratch.0, 0).expect("writer");
        writer.append(b"short").expect("append");
        drop(writer);

        let pack = Pack::open(&scratch.0, 0).expect("open").expect("mapped");
        assert_eq!(pack.read(0, 500, 1), None);
        assert_eq!(pack.read(4, 5, 1), None);
        assert_eq!(pack.read(u64::MAX, 1, 1), None);
    }

    #[test]
    fn reopening_a_pack_appends_rather_than_truncating() {
        let scratch = Scratch::new("reopen");
        let mut writer = PackWriter::open(&scratch.0, 0).expect("writer");
        writer.append(b"first").expect("append");
        drop(writer);

        let mut writer = PackWriter::open(&scratch.0, 0).expect("writer");
        assert_eq!(writer.offset(), 5);
        let (_, at) = writer.append(b"second").expect("append");
        assert_eq!(at, 5);
        drop(writer);

        let pack = Pack::open(&scratch.0, 0).expect("open").expect("mapped");
        assert_eq!(pack.read(0, 11, 1), Some(&b"firstsecond"[..]));
    }

    #[test]
    fn rolls_over_when_a_pack_fills() {
        let scratch = Scratch::new("rollover");
        let mut writer = PackWriter::open(&scratch.0, 0).expect("writer");
        // Fake a nearly full pack rather than writing 512 MB.
        writer.append(&[0u8; 64]).expect("append");
        writer.offset = PACK_CAP - 8;
        let (id, at) = writer.append(&[1u8; 32]).expect("append");
        assert_eq!(id, 1, "a full pack rolls over");
        assert_eq!(at, 0, "and the next one starts at zero");
    }

    /// The pinned pack holds the install indexes and must never roll over, or
    /// the thing eviction is forbidden to touch would end up spread across
    /// packs that eviction is allowed to touch.
    #[test]
    fn the_pinned_pack_never_rolls_over() {
        let scratch = Scratch::new("pinned");
        let mut writer = PackWriter::open(&scratch.0, PINNED).expect("writer");
        writer.append(&[0u8; 16]).expect("append");
        writer.offset = PACK_CAP + 1;
        let (id, _) = writer.append(&[1u8; 32]).expect("append");
        assert_eq!(id, PINNED);
    }

    #[test]
    fn names_packs_stably() {
        assert_eq!(pack_name(0), "pack.0000");
        assert_eq!(pack_name(42), "pack.0042");
        assert_eq!(pack_name(PINNED), "pack.pinned");
    }

    #[test]
    fn a_hit_records_the_clock_for_eviction() {
        let scratch = Scratch::new("lru");
        let mut writer = PackWriter::open(&scratch.0, 0).expect("writer");
        writer.append(b"abcd").expect("append");
        drop(writer);

        let pack = Pack::open(&scratch.0, 0).expect("open").expect("mapped");
        assert_eq!(pack.last_hit(), 0);
        pack.read(0, 4, 77).expect("hit");
        assert_eq!(pack.last_hit(), 77);
        // A miss must not count as use, or an evicted pack could look live.
        pack.read(0, 999, 88);
        assert_eq!(pack.last_hit(), 77);
    }
}
