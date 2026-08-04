//! Keeping the `.rpf` files on disk mapped, so extraction is a memcpy.
//!
//! Not a port of anything. The reference opens a `FileStream` per extraction
//! (`RpfFile.ExtractFile`), which is fine for a tool that opens one file at a
//! time and wrong for a world streamer that pulls thousands of entries a
//! minute out of the same handful of archives.
//!
//! Three decisions shape it:
//!
//! - **Keyed by physical path, not by archive index.** The 5,199 nested
//!   archives in a stock install resolve to 179 files on disk. Keying by
//!   archive would map the same file thousands of times.
//! - **Capped, and LRU-evicted.** Not for address space — 64-bit has room to
//!   spare, and a mapping reserves address space rather than committing memory
//!   — but because Windows holds a file lock for a mapping's lifetime. A user
//!   who wants to patch the game while the tool is open must be able to, hence
//!   the cap and [`ArchiveMaps::release`].
//! - **Failure is not an error.** A network drive, a permissions problem or a
//!   file that changed size all just mean the caller falls back to opening the
//!   file. Nothing about mapping is load-bearing for correctness.
//!
//! Mapping a file another process could truncate is undefined behaviour under
//! a strict reading of `Mmap::map`. The game folder is not being written while
//! the tool runs, and the fallback path exists for when that assumption breaks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use memmap2::Mmap;

/// How many archives stay mapped at once. A stock install has 179 top-level
/// archives and a session touches far fewer; 64 covers the working set of any
/// one area of the map with room over.
pub const MAX_LIVE_MAPPINGS: usize = 64;

#[derive(Debug)]
struct Mapping {
    map: Arc<Mmap>,
    /// Monotonic tick of the last hit, for LRU eviction.
    used: u64,
}

/// The live mappings, keyed by the `.rpf`'s path on disk.
#[derive(Debug, Default)]
pub struct ArchiveMaps {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    live: HashMap<PathBuf, Mapping>,
    /// Paths that failed to map. Remembered so a folder on a network drive
    /// does not retry the `open` plus `map` on every single extraction.
    refused: HashMap<PathBuf, ()>,
    clock: u64,
}

impl ArchiveMaps {
    /// The mapping for `path`, creating it if there is room and it is mappable.
    ///
    /// `None` means the caller should open the file itself. The `Arc` is
    /// cloned out under the lock and the lock released, so a slow read never
    /// blocks another archive's lookup.
    pub fn get(&self, path: &Path) -> Option<Arc<Mmap>> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        inner.clock += 1;
        let now = inner.clock;

        if let Some(existing) = inner.live.get_mut(path) {
            existing.used = now;
            return Some(Arc::clone(&existing.map));
        }
        if inner.refused.contains_key(path) {
            return None;
        }

        // Mapping an empty file fails on every platform, and an archive of
        // zero bytes has nothing to extract anyway.
        let map = match std::fs::File::open(path).and_then(|file| {
            if file.metadata()?.len() == 0 {
                return Err(std::io::Error::other("empty archive"));
            }
            // SAFETY: see the module note. The game folder is not written
            // while the tool runs, and `extract` falls back to a plain read
            // whenever this returns `None`.
            unsafe { Mmap::map(&file) }
        }) {
            Ok(map) => Arc::new(map),
            Err(_) => {
                inner.refused.insert(path.to_path_buf(), ());
                return None;
            }
        };

        if inner.live.len() >= MAX_LIVE_MAPPINGS {
            inner.evict_one();
        }
        inner.live.insert(
            path.to_path_buf(),
            Mapping {
                map: Arc::clone(&map),
                used: now,
            },
        );
        Some(map)
    }

    /// Drop every mapping, releasing the file locks they hold.
    ///
    /// A mapping already handed out stays alive until its `Arc` is dropped, so
    /// this frees the archives as the in-flight reads finish rather than
    /// immediately. Clears the refusal list too, so a folder that became
    /// readable is retried.
    pub fn release(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.live.clear();
        inner.refused.clear();
    }

    /// How many archives are mapped right now.
    pub fn live(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .len()
    }
}

impl Inner {
    /// Drop the least recently used mapping.
    fn evict_one(&mut self) {
        let Some(oldest) = self
            .live
            .iter()
            .min_by_key(|(_, m)| m.used)
            .map(|(path, _)| path.clone())
        else {
            return;
        };
        self.live.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A file the maps can actually map, in a directory that cleans itself up.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("astramapper-maps-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self { dir }
        }

        fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.dir.join(name);
            let mut file = std::fs::File::create(&path).expect("create");
            file.write_all(contents).expect("write");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn maps_a_file_and_hands_back_its_bytes() {
        let scratch = Scratch::new("bytes");
        let path = scratch.file("a.rpf", b"hello archive");
        let maps = ArchiveMaps::default();

        let map = maps.get(&path).expect("mapped");
        assert_eq!(&map[..], b"hello archive");
    }

    /// The whole point of keying by path: the same archive maps once however
    /// many nested archives resolve to it.
    #[test]
    fn maps_the_same_path_once() {
        let scratch = Scratch::new("once");
        let path = scratch.file("a.rpf", b"payload");
        let maps = ArchiveMaps::default();

        let first = maps.get(&path).expect("mapped");
        let second = maps.get(&path).expect("mapped");

        assert!(Arc::ptr_eq(&first, &second), "the same mapping is reused");
        assert_eq!(maps.live(), 1);
    }

    #[test]
    fn a_missing_file_is_refused_rather_than_failing() {
        let scratch = Scratch::new("missing");
        let path = scratch.dir.join("not-here.rpf");
        let maps = ArchiveMaps::default();

        assert!(maps.get(&path).is_none());
        // And again, from the refusal list rather than a second failed open.
        assert!(maps.get(&path).is_none());
        assert_eq!(maps.live(), 0);
    }

    /// An empty archive cannot be mapped on any platform, and refusing it is
    /// the same path as any other unmappable file.
    #[test]
    fn an_empty_file_is_refused() {
        let scratch = Scratch::new("empty");
        let path = scratch.file("empty.rpf", b"");
        let maps = ArchiveMaps::default();

        assert!(maps.get(&path).is_none());
    }

    #[test]
    fn evicts_the_least_recently_used_past_the_cap() {
        let scratch = Scratch::new("evict");
        let maps = ArchiveMaps::default();

        let paths: Vec<PathBuf> = (0..MAX_LIVE_MAPPINGS)
            .map(|i| scratch.file(&format!("{i}.rpf"), format!("archive {i}").as_bytes()))
            .collect();
        for path in &paths {
            assert!(maps.get(path).is_some());
        }
        assert_eq!(maps.live(), MAX_LIVE_MAPPINGS);

        // Touch the first so the second becomes the least recently used, then
        // map one more and check the right one was dropped.
        maps.get(&paths[0]).expect("still mapped");
        let extra = scratch.file("extra.rpf", b"extra");
        maps.get(&extra).expect("mapped");

        assert_eq!(maps.live(), MAX_LIVE_MAPPINGS);
        let inner = maps.inner.lock().expect("lock");
        assert!(inner.live.contains_key(&paths[0]), "the touched one stays");
        assert!(!inner.live.contains_key(&paths[1]), "the coldest one goes");
    }

    #[test]
    fn release_drops_every_mapping() {
        let scratch = Scratch::new("release");
        let path = scratch.file("a.rpf", b"payload");
        let maps = ArchiveMaps::default();

        maps.get(&path).expect("mapped");
        assert_eq!(maps.live(), 1);
        maps.release();
        assert_eq!(maps.live(), 0);
    }
}
