//! Which key is where: a compacted table plus an append-only journal.
//!
//! Two files rather than one, for one reason. The table (`index.bin`) is
//! rewritten wholesale and atomically, which is cheap at 400,000 records but
//! not something to do per `put`. The journal (`index.log`) is appended to per
//! `put`, which is a few dozen bytes. On open, the table is read and the
//! journal replayed over it; compaction merges the two and truncates the
//! journal.
//!
//! **A torn journal entry is expected, not exceptional.** The process can be
//! killed mid-bake, and nothing here fsyncs. So replay stops at the first
//! record that does not parse rather than treating it as corruption, and the
//! caller separately drops any record whose bytes do not fit the pack it names.
//! A record lost that way costs one re-bake.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use crate::key::CacheKey;
use crate::{Error, Result};

/// `{key: 16, pack: 4, offset: 8, length: 4, kind: 1, pad: 3}`.
pub const RECORD_BYTES: usize = 36;

/// Where one artifact lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub key: CacheKey,
    pub pack: u32,
    pub offset: u64,
    pub length: u32,
    pub kind: u8,
}

impl Record {
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.key.to_bytes());
        out.extend_from_slice(&self.pack.to_le_bytes());
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.length.to_le_bytes());
        out.push(self.kind);
        out.extend_from_slice(&[0u8; 3]);
    }

    pub fn read_from(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < RECORD_BYTES {
            return None;
        }
        let mut key = [0u8; 16];
        key.copy_from_slice(&bytes[..16]);
        Some(Self {
            key: CacheKey::from_bytes(&key),
            pack: u32::from_le_bytes(bytes[16..20].try_into().ok()?),
            offset: u64::from_le_bytes(bytes[20..28].try_into().ok()?),
            length: u32::from_le_bytes(bytes[28..32].try_into().ok()?),
            kind: bytes[32],
        })
    }
}

/// Every record the cache knows about, by key.
#[derive(Debug, Default)]
pub struct Index {
    pub records: HashMap<CacheKey, Record>,
    /// Journal bytes since the last compaction, so the caller knows when it is
    /// worth rewriting the table.
    pub journal_len: u64,
}

impl Index {
    /// Read the table and replay the journal over it.
    ///
    /// A later journal entry wins, because that is what a rewrite of the same
    /// key means: the artifact was re-baked and the old bytes are dead.
    pub fn load(dir: &Path) -> Result<Self> {
        let mut records = read_records(&dir.join("index.bin"))?;
        let journal = read_all(&dir.join("index.log"))?;
        let journal_len = journal.len() as u64;

        let mut at = 0;
        while at + RECORD_BYTES <= journal.len() {
            let Some(record) = Record::read_from(&journal[at..]) else {
                break;
            };
            records.insert(record.key, record);
            at += RECORD_BYTES;
        }

        Ok(Self {
            records,
            journal_len,
        })
    }

    /// Drop every record that does not fit the pack it names.
    ///
    /// The other half of surviving a torn write: the journal entry may have
    /// landed while the pack append did not. Returns how many were dropped.
    pub fn drop_unbacked(&mut self, pack_len: impl Fn(u32) -> Option<u64>) -> usize {
        let before = self.records.len();
        self.records.retain(|_, record| {
            pack_len(record.pack)
                .is_some_and(|len| record.offset + record.length as u64 <= len)
        });
        before - self.records.len()
    }

    /// Drop every record in one pack, for whole-pack eviction.
    pub fn drop_pack(&mut self, pack: u32) -> usize {
        let before = self.records.len();
        self.records.retain(|_, record| record.pack != pack);
        before - self.records.len()
    }
}

/// Append one record to the journal.
pub fn append(dir: &Path, record: &Record) -> Result<u64> {
    let mut bytes = Vec::with_capacity(RECORD_BYTES);
    record.write_to(&mut bytes);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("index.log"))
        .map_err(|e| Error::Io {
            what: "open the index journal",
            source: e,
        })?;
    file.write_all(&bytes).map_err(|e| Error::Io {
        what: "append to the index journal",
        source: e,
    })?;
    Ok(bytes.len() as u64)
}

/// Rewrite the table from the live records and truncate the journal.
///
/// **Atomic by rename.** Writing `index.bin` in place would leave the cache
/// unreadable if the process died halfway, and unreadable is worse than stale:
/// the whole point of the table is that it can be trusted on open.
///
/// Takes `&mut` only to zero [`Index::journal_len`]: leaving that to the caller
/// makes forgetting it a silent bug that compacts on every write from then on.
pub fn compact(dir: &Path, index: &mut Index) -> Result<()> {
    let mut bytes = Vec::with_capacity(index.records.len() * RECORD_BYTES);
    // Sorted, so the file is deterministic and diffable, and so a future
    // version can binary-search it without loading the map.
    let mut records: Vec<&Record> = index.records.values().collect();
    records.sort_by_key(|record| record.key);
    for record in records {
        record.write_to(&mut bytes);
    }

    let temporary = dir.join("index.bin.new");
    std::fs::write(&temporary, &bytes).map_err(|e| Error::Io {
        what: "write the index table",
        source: e,
    })?;
    std::fs::rename(&temporary, dir.join("index.bin")).map_err(|e| Error::Io {
        what: "replace the index table",
        source: e,
    })?;
    // Only now is the journal redundant.
    let _ = std::fs::remove_file(dir.join("index.log"));
    index.journal_len = 0;
    Ok(())
}

fn read_records(path: &Path) -> Result<HashMap<CacheKey, Record>> {
    let bytes = read_all(path)?;
    let mut records = HashMap::with_capacity(bytes.len() / RECORD_BYTES);
    let mut at = 0;
    while at + RECORD_BYTES <= bytes.len() {
        if let Some(record) = Record::read_from(&bytes[at..]) {
            records.insert(record.key, record);
        }
        at += RECORD_BYTES;
    }
    Ok(records)
}

fn read_all(path: &Path) -> Result<Vec<u8>> {
    let Ok(mut file) = File::open(path) else {
        return Ok(Vec::new());
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| Error::Io {
        what: "read the index",
        source: e,
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cw-cache-index-{name}-{}-{:?}",
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

    fn record(key: u64, pack: u32, offset: u64, length: u32) -> Record {
        Record {
            key: CacheKey(key, key.wrapping_mul(3)),
            pack,
            offset,
            length,
            kind: 2,
        }
    }

    #[test]
    fn a_record_round_trips_through_bytes() {
        let original = record(7, 3, 4096, 128);
        let mut bytes = Vec::new();
        original.write_to(&mut bytes);
        assert_eq!(bytes.len(), RECORD_BYTES);
        assert_eq!(Record::read_from(&bytes), Some(original));
    }

    #[test]
    fn an_empty_directory_loads_an_empty_index() {
        let scratch = Scratch::new("empty");
        let index = Index::load(&scratch.0).expect("load");
        assert!(index.records.is_empty());
    }

    #[test]
    fn the_journal_survives_a_restart() {
        let scratch = Scratch::new("journal");
        append(&scratch.0, &record(1, 0, 0, 10)).expect("append");
        append(&scratch.0, &record(2, 0, 10, 20)).expect("append");

        let index = Index::load(&scratch.0).expect("load");
        assert_eq!(index.records.len(), 2);
        assert_eq!(index.journal_len, 2 * RECORD_BYTES as u64);
    }

    /// The bake process being killed is the normal case, not an exotic one.
    /// Replay has to keep everything before the tear and stop cleanly.
    #[test]
    fn replay_stops_at_a_truncated_journal_entry() {
        let scratch = Scratch::new("torn");
        append(&scratch.0, &record(1, 0, 0, 10)).expect("append");
        append(&scratch.0, &record(2, 0, 10, 20)).expect("append");

        // Chop the last entry in half, as a killed process would.
        let path = scratch.0.join("index.log");
        let bytes = std::fs::read(&path).expect("read");
        std::fs::write(&path, &bytes[..bytes.len() - RECORD_BYTES / 2]).expect("truncate");

        let index = Index::load(&scratch.0).expect("load");
        assert_eq!(index.records.len(), 1, "the intact record survives");
        assert!(index.records.contains_key(&CacheKey(1, 3)));
    }

    #[test]
    fn a_later_write_of_the_same_key_wins() {
        let scratch = Scratch::new("rewrite");
        append(&scratch.0, &record(1, 0, 0, 10)).expect("append");
        append(&scratch.0, &record(1, 0, 512, 99)).expect("append");

        let index = Index::load(&scratch.0).expect("load");
        assert_eq!(index.records.len(), 1);
        let found = index.records[&CacheKey(1, 3)];
        assert_eq!((found.offset, found.length), (512, 99));
    }

    #[test]
    fn compaction_folds_the_journal_into_the_table() {
        let scratch = Scratch::new("compact");
        append(&scratch.0, &record(1, 0, 0, 10)).expect("append");
        append(&scratch.0, &record(2, 0, 10, 20)).expect("append");
        let mut index = Index::load(&scratch.0).expect("load");
        compact(&scratch.0, &mut index).expect("compact");

        assert!(!scratch.0.join("index.log").exists(), "journal is gone");
        assert_eq!(index.journal_len, 0, "and so is the reason to compact again");
        let reloaded = Index::load(&scratch.0).expect("load");
        assert_eq!(reloaded.records.len(), 2);
        assert_eq!(reloaded.journal_len, 0);
    }

    #[test]
    fn compaction_then_more_writes_keeps_both() {
        let scratch = Scratch::new("compact-then-write");
        append(&scratch.0, &record(1, 0, 0, 10)).expect("append");
        let mut index = Index::load(&scratch.0).expect("load");
        compact(&scratch.0, &mut index).expect("compact");
        append(&scratch.0, &record(2, 0, 10, 20)).expect("append");

        let reloaded = Index::load(&scratch.0).expect("load");
        assert_eq!(reloaded.records.len(), 2);
    }

    /// The other half of surviving a tear: the journal entry landed, the pack
    /// append did not.
    #[test]
    fn drops_records_whose_bytes_are_not_in_the_pack() {
        let mut index = Index::default();
        for r in [record(1, 0, 0, 10), record(2, 0, 90, 20), record(3, 5, 0, 4)] {
            index.records.insert(r.key, r);
        }
        // Pack 0 is 100 bytes long; pack 5 does not exist.
        let dropped = index.drop_unbacked(|pack| (pack == 0).then_some(100));
        assert_eq!(dropped, 2, "the overrun and the missing pack both go");
        assert_eq!(index.records.len(), 1);
        assert!(index.records.contains_key(&CacheKey(1, 3)));
    }

    #[test]
    fn dropping_a_pack_drops_only_its_records() {
        let mut index = Index::default();
        for r in [record(1, 0, 0, 10), record(2, 1, 0, 20), record(3, 0, 10, 4)] {
            index.records.insert(r.key, r);
        }
        assert_eq!(index.drop_pack(0), 2);
        assert_eq!(index.records.len(), 1);
        assert!(index.records.contains_key(&CacheKey(2, 6)));
    }

    /// A garbage table must not take the journal down with it, because the
    /// journal is the newer half.
    #[test]
    fn a_short_tail_on_the_table_is_ignored() {
        let scratch = Scratch::new("short-table");
        let mut bytes = Vec::new();
        record(1, 0, 0, 10).write_to(&mut bytes);
        bytes.extend_from_slice(&[0xab; 7]);
        std::fs::write(scratch.0.join("index.bin"), &bytes).expect("write");

        let index = Index::load(&scratch.0).expect("load");
        assert_eq!(index.records.len(), 1);
    }
}
