//! Scanning a game folder into one addressable index of every archive entry.
//!
//! Port of `CodeWalker.Core/GameFiles/RpfManager.cs`, narrowed to what reading
//! needs. The reference keeps six overlapping archive lists (base/mod/dlc and
//! no-mod variants of each) to resolve override precedence; that belongs with
//! DLC selection in phase 3, so this keeps one flat list and one index.
//!
//! Scanning is parallel across top-level archives. The reference is serial;
//! nothing in a scan is shared, and there are ~179 archives to get through.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rayon::prelude::*;
use serde::Serialize;

use super::entry::{EntryKind, RpfEntry};
use super::file::RpfFile;
use super::mapped::ArchiveMaps;
use crate::utils::GtaKeys;
use crate::Result;

/// Where one entry lives: which archive in [`RpfManager::archives`], and which
/// entry within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EntryRef {
    pub archive: usize,
    pub entry: usize,
}

/// An archive that could not be read. The reference logs these and moves on,
/// because one corrupt or renamed archive must not lose the other 178.
#[derive(Debug, Clone, Serialize)]
pub struct ScanError {
    pub path: PathBuf,
    pub message: String,
}

/// Every archive under a game folder, flattened and indexed by path.
#[derive(Debug, Default)]
pub struct RpfManager {
    pub folder: PathBuf,
    /// All archives, nested ones included, each with an empty `children` —
    /// the nesting lives in [`Self::archive_parent`].
    pub archives: Vec<RpfFile>,
    /// Index of each archive's containing archive, or `None` for a top-level
    /// one. Parallel to `archives`.
    pub archive_parent: Vec<Option<usize>>,
    pub errors: Vec<ScanError>,

    /// Lowercase entry path to its location. Directories are included.
    entry_index: HashMap<String, EntryRef>,
    /// Lowercase archive path to its index in `archives`. An archive's path is
    /// the same string as the `.rpf` entry that contains it, which is what lets
    /// a directory listing step from one into the other.
    archive_index: HashMap<String, usize>,
    /// Every `.ytd`'s path grouped by its folder, built once. Resolving a
    /// model's textures scans the same folder's dictionaries; without this the
    /// whole archive tree would be walked per model, which is costly when many
    /// stream at once.
    ytds_by_folder: OnceLock<HashMap<String, Vec<String>>>,
    /// The best model file for each lowercase basename stem, built once —
    /// `update` copies beat base, `.ydr` beats `.yft` beats `.ydd`. Resolving
    /// an archetype to its asset is then a hash lookup, not a tree walk.
    ///
    /// A `OnceLock` rather than a `Mutex<Option<_>>`: readers get a plain
    /// reference with no guard to hold, so a caller cannot accidentally keep
    /// the lock across a long read and serialize every other streaming request
    /// behind itself.
    asset_index: OnceLock<HashMap<String, (u8, u8, String)>>,
    /// Every `.ytd` keyed by the Jenkins hash of its stem, built once. This is
    /// the form an archetype's `textureDictionary` names one in.
    ytd_by_hash: OnceLock<HashMap<u32, String>>,
    /// The `.rpf` files currently mapped into memory. See
    /// [`crate::rpf::mapped`] for why extraction reads through a mapping.
    maps: ArchiveMaps,
    /// What each archive's bytes on disk looked like at scan time, parallel to
    /// `archives`. See [`Self::archive_stamps`].
    stamps: OnceLock<Vec<u64>>,
}

/// A cheap identity for one `.rpf` file on disk: its length and its modified
/// time, mixed together.
///
/// Not a content hash. Hashing 120 GB to find out whether anything changed
/// costs more than every read it could save, and length-plus-mtime is what
/// every build system on the planet settles for. A patch that rewrites an
/// archive changes both.
///
/// Zero when the file cannot be stat'd, which reads as "unknown" — a caller
/// keying on this must treat zero as never matching, because an unreadable
/// file is not evidence that nothing changed.
pub fn stamp_file(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // A 64-bit mix so a one-second mtime change moves every bit, rather than
    // only the low ones. Splitmix64's finalizer.
    let mut x = meta.len() ^ modified.rotate_left(32);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    let mixed = x ^ (x >> 31);
    // Zero means "unknown", so never produce it by accident.
    if mixed == 0 {
        1
    } else {
        mixed
    }
}

impl RpfManager {
    /// Scan `folder` recursively for `.rpf` archives and index everything in
    /// them.
    pub fn scan(folder: &Path, keys: &GtaKeys) -> Result<Self> {
        let mut paths = Vec::new();
        collect_rpf_paths(folder, &mut paths)?;
        paths.sort();

        let scanned: Vec<(PathBuf, Result<RpfFile>)> = paths
            .par_iter()
            .map(|path| {
                let rel = path
                    .strip_prefix(folder)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_lowercase();
                (path.clone(), RpfFile::open(path, &rel, keys))
            })
            .collect();

        let mut manager = Self {
            folder: folder.to_path_buf(),
            ..Default::default()
        };

        for (path, result) in scanned {
            match result {
                Ok(rpf) => manager.add(rpf),
                Err(e) => manager.errors.push(ScanError {
                    path,
                    message: e.to_string(),
                }),
            }
        }

        manager.build_index();
        Ok(manager)
    }

    /// Flatten one archive tree into the manager.
    fn add(&mut self, rpf: RpfFile) {
        flatten_into(rpf, None, &mut self.archives, &mut self.archive_parent);
    }

    fn build_index(&mut self) {
        let mut entries = HashMap::new();
        let mut archives = HashMap::new();
        for (a, archive) in self.archives.iter().enumerate() {
            archives.insert(archive.path.clone(), a);
            let nested = self.archive_parent[a].is_some();
            for (e, entry) in archive.entries.iter().enumerate() {
                // A nested archive's root directory has the same path as the
                // `.rpf` entry holding it, because that is the same thing seen
                // from two sides. Index only the parent's entry: it is the one
                // that can be extracted as a file, and listing it already steps
                // through into the archive.
                if nested && e == 0 {
                    continue;
                }
                // Later archives win a collision, matching the reference's
                // dictionary assignment. Real collisions here are a mod
                // overriding a base file; deciding that properly is DLC
                // precedence work, which belongs with phase 3.
                entries.insert(
                    entry.path.clone(),
                    EntryRef {
                        archive: a,
                        entry: e,
                    },
                );
            }
        }
        self.entry_index = entries;
        self.archive_index = archives;
    }

    /// Total entries across every archive.
    pub fn entry_count(&self) -> usize {
        self.archives.iter().map(|a| a.entries.len()).sum()
    }

    /// Archives that are not nested inside another.
    pub fn top_level(&self) -> impl Iterator<Item = &RpfFile> {
        self.archives
            .iter()
            .zip(&self.archive_parent)
            .filter(|(_, p)| p.is_none())
            .map(|(a, _)| a)
    }

    /// Look one entry up by its full lowercase path, e.g.
    /// `x64a.rpf\levels\gta5\_cityw\downtown_01\dt1_01.rpf`.
    pub fn find(&self, path: &str) -> Option<EntryRef> {
        self.entry_index.get(&normalize_path(path)).copied()
    }

    pub fn entry(&self, r: EntryRef) -> &RpfEntry {
        &self.archives[r.archive].entries[r.entry]
    }

    pub fn archive(&self, r: EntryRef) -> &RpfFile {
        &self.archives[r.archive]
    }

    /// Immediate children of a directory entry.
    ///
    /// The archive an entry belongs to and the archive it *contains* are
    /// different things: a `.rpf` entry is a leaf in its own archive and the
    /// root of another. Listing one transparently steps into the nested
    /// archive, which is what makes the whole game folder look like one tree.
    pub fn list_dir(&self, r: EntryRef) -> Vec<EntryRef> {
        let entry = self.entry(r);

        if matches!(entry.kind, EntryKind::Binary(_)) && entry.name_lower.ends_with(".rpf") {
            if let Some(nested) = self.nested_archive(r) {
                return self.archives[nested]
                    .children_of(0)
                    .map(|e| EntryRef {
                        archive: nested,
                        entry: e,
                    })
                    .collect();
            }
            return Vec::new();
        }

        self.archives[r.archive]
            .children_of(r.entry)
            .map(|e| EntryRef {
                archive: r.archive,
                entry: e,
            })
            .collect()
    }

    /// The archive an `.rpf` entry opens into, if it was scanned successfully.
    /// `None` for a nested archive whose own scan failed — a listing of it is
    /// empty rather than an error.
    pub fn nested_archive(&self, r: EntryRef) -> Option<usize> {
        self.archive_index.get(&self.entry(r).path).copied()
    }

    /// Children of an archive's root directory.
    pub fn list_root(&self, archive: usize) -> Vec<EntryRef> {
        self.archives[archive]
            .children_of(0)
            .map(|entry| EntryRef { archive, entry })
            .collect()
    }

    /// Read an entry's payload.
    ///
    /// Reads through a mapping of the `.rpf` on disk when there is one, so a
    /// streamed model costs a seek and a memcpy inside an already-resident
    /// file rather than an `open` plus a syscall read. A physical file that
    /// cannot be mapped falls back to [`RpfFile::extract`], which opens it;
    /// the decrypt and inflate path below is the same either way.
    pub fn extract(&self, r: EntryRef, keys: &GtaKeys) -> Result<Vec<u8>> {
        let archive = &self.archives[r.archive];
        let entry = &archive.entries[r.entry];
        match self.maps.get(&archive.physical_path) {
            Some(map) => archive.extract_from(&mut std::io::Cursor::new(&map[..]), entry, keys),
            None => archive.extract(entry, keys),
        }
    }

    /// Drop every archive mapping, releasing the file locks they hold.
    ///
    /// A mapped archive cannot be replaced on Windows, so a user who wants to
    /// patch the game while the tool is open needs a way to let go. Mappings
    /// are recreated on the next extraction.
    pub fn release_archives(&self) {
        self.maps.release();
    }

    /// How many archives are mapped right now.
    pub fn mapped_archives(&self) -> usize {
        self.maps.live()
    }

    /// Every entry's basename keyed by its Jenkins hash.
    ///
    /// Port of CodeWalker's `BuildBaseJenkIndex`: a `.ymap` archetype hash is
    /// the Jenkins hash of the archetype's name, and that name is the model's
    /// basename — so hashing every archive entry's stem builds the table that
    /// turns an entity's archetype hash back into `prop_bin_04a`. Both the
    /// original case and the lowercase form are registered, since a hash made
    /// from the original case and one from the lowercase differ.
    pub fn index_names(&self) -> HashMap<u32, String> {
        let mut names: HashMap<u32, String> = HashMap::new();
        let register = |names: &mut HashMap<u32, String>, basename: &str| {
            let stem = match basename.rfind('.') {
                Some(dot) => &basename[..dot],
                None => basename,
            };
            if stem.is_empty() {
                return;
            }
            names
                .entry(crate::utils::gen_hash(stem))
                .or_insert_with(|| stem.to_string());
        };
        for archive in &self.archives {
            for entry in &archive.entries {
                register(&mut names, &entry.name);
                register(&mut names, &entry.name_lower);
            }
        }
        names
    }

    /// Every `.ytd`'s path grouped by its folder, built once and kept.
    pub fn ytds(&self) -> &HashMap<String, Vec<String>> {
        self.ytds_by_folder.get_or_init(|| {
            let mut by_folder: HashMap<String, Vec<String>> = HashMap::new();
            for archive in &self.archives {
                for entry in &archive.entries {
                    if !entry.name_lower.ends_with(".ytd") {
                        continue;
                    }
                    let folder = match entry.path.rsplit_once('\\') {
                        Some((folder, _)) => folder.to_string(),
                        None => String::new(),
                    };
                    by_folder
                        .entry(folder)
                        .or_default()
                        .push(entry.path.clone());
                }
            }
            by_folder
        })
    }

    /// What each archive's bytes on disk looked like, parallel to `archives`.
    ///
    /// A **nested archive inherits its top-level ancestor's stamp**, because a
    /// nested `.rpf` has no separate existence on disk: patching
    /// `update\update.rpf` rewrites every archive inside it at once, and there
    /// is nothing else to stat. That inheritance is the whole value of stamping
    /// per archive rather than per install — a patch to one `.rpf` invalidates
    /// only what came out of it, not the other 178.
    ///
    /// Built once and kept. Stat'ing 179 files is cheap; doing it per cache
    /// lookup would not be.
    pub fn archive_stamps(&self) -> &[u64] {
        self.stamps.get_or_init(|| {
            let mut stamps: Vec<u64> = Vec::with_capacity(self.archives.len());
            for (index, parent) in self.archive_parent.iter().enumerate() {
                let stamp = match parent {
                    Some(parent) => stamps[*parent],
                    None => stamp_file(&self.archives[index].physical_path),
                };
                stamps.push(stamp);
            }
            stamps
        })
    }

    /// The stamp of the archive an entry came out of.
    pub fn stamp_of(&self, reference: EntryRef) -> u64 {
        self.archive_stamps()
            .get(reference.archive)
            .copied()
            .unwrap_or(0)
    }

    /// Every `.ytd` keyed by the Jenkins hash of its stem, built once.
    ///
    /// An archetype names its texture dictionary by hash, not by path, so this
    /// is what turns `BaseArchetype::texture_dictionary` into a file. An
    /// `update\` copy wins, matching [`Self::assets`]; otherwise the first
    /// seen keeps the slot, because a later duplicate is a different pack's
    /// copy of the same dictionary rather than a better one.
    pub fn ytd_by_hash(&self) -> &HashMap<u32, String> {
        self.ytd_by_hash.get_or_init(|| {
            let mut index: HashMap<u32, String> = HashMap::new();
            for archive in &self.archives {
                for entry in &archive.entries {
                    let Some(stem) = entry.name_lower.strip_suffix(".ytd") else {
                        continue;
                    };
                    if stem.is_empty() {
                        continue;
                    }
                    let hash = crate::utils::gen_hash(stem);
                    let from_update = entry.path.starts_with("update\\");
                    match index.get(&hash) {
                        Some(existing) if !from_update || existing.starts_with("update\\") => {}
                        _ => {
                            index.insert(hash, entry.path.clone());
                        }
                    }
                }
            }
            index
        })
    }

    /// The best model file per lowercase basename stem, built once.
    ///
    /// The value's first two tuple elements mirror `find_model_for`'s
    /// precedence: `update\` copies beat the base game, and `.ydr` beats
    /// `.yft` beats `.ydd`. The path breaks a tie.
    pub fn assets(&self) -> &HashMap<String, (u8, u8, String)> {
        self.asset_index.get_or_init(|| {
            let ext_priority = |lower: &str| -> u8 {
                if lower.ends_with(".ydr") {
                    0
                } else if lower.ends_with(".yft") {
                    1
                } else {
                    2
                }
            };
            let mut index: HashMap<String, (u8, u8, String)> = HashMap::new();
            for archive in &self.archives {
                for entry in &archive.entries {
                    let lower = &entry.name_lower;
                    if !(lower.ends_with(".ydr")
                        || lower.ends_with(".yft")
                        || lower.ends_with(".ydd"))
                    {
                        continue;
                    }
                    let stem = &lower[..lower.len().saturating_sub(4)];
                    if stem.is_empty() {
                        continue;
                    }
                    // `name_lower` is the basename; the `update\` precedence
                    // lives in the full path, so the two are read apart.
                    let candidate = (
                        u8::from(!entry.path.starts_with("update\\")),
                        ext_priority(lower),
                        entry.path.clone(),
                    );
                    let keep = index
                        .get(stem)
                        .is_some_and(|existing| existing < &candidate);
                    if !keep {
                        index.insert(stem.to_string(), candidate);
                    }
                }
            }
            index
        })
    }
}

/// Accept both separators and fold case, so a path from the frontend matches
/// the backslash-lowercase form the index is built in.
fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// Move an archive and everything nested inside it into a flat list, recording
/// each one's parent.
fn flatten_into(
    mut rpf: RpfFile,
    parent: Option<usize>,
    out: &mut Vec<RpfFile>,
    parents: &mut Vec<Option<usize>>,
) {
    let children = std::mem::take(&mut rpf.children);
    let index = out.len();
    out.push(rpf);
    parents.push(parent);
    for child in children {
        flatten_into(child, Some(index), out, parents);
    }
}

/// Recursive equivalent of `Directory.GetFiles(folder, "*.rpf", AllDirectories)`.
/// An unreadable subdirectory is skipped rather than failing the whole scan.
fn collect_rpf_paths(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect_rpf_paths(&path, out)?,
            Ok(t) if t.is_file() => {
                let is_rpf = path
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("rpf"))
                    .unwrap_or(false);
                if is_rpf {
                    out.push(path);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unreadable file has to stamp as zero, because a caller keying a cache
    /// on this must be able to tell "unchanged" from "no idea".
    #[test]
    fn an_unreadable_file_stamps_as_unknown() {
        assert_eq!(stamp_file(Path::new("nowhere-at-all.rpf")), 0);
    }

    #[test]
    fn a_files_stamp_is_stable_and_moves_with_its_contents() {
        let dir = std::env::temp_dir().join(format!("astramapper-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.rpf");

        std::fs::write(&path, b"one").unwrap();
        let first = stamp_file(&path);
        assert_ne!(first, 0);
        assert_eq!(first, stamp_file(&path), "the same file stamps the same");

        // A different length is enough on its own; mtime resolution varies by
        // filesystem and this must not depend on it.
        std::fs::write(&path, b"a longer payload").unwrap();
        assert_ne!(stamp_file(&path), first, "rewriting it moves the stamp");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The point of stamping per archive rather than per install: patching one
    /// `.rpf` must invalidate what came out of it and nothing else. A nested
    /// archive has no separate existence on disk, so it inherits.
    #[test]
    fn a_nested_archive_inherits_its_top_level_stamp() {
        let archive = |physical: &str| RpfFile {
            name: "a.rpf".into(),
            name_lower: "a.rpf".into(),
            path: "a.rpf".into(),
            physical_path: PathBuf::from(physical),
            file_size: 0,
            start_pos: 0,
            version: 7,
            entry_count: 0,
            names_length: 0,
            encryption: crate::rpf::RpfEncryption::None,
            entries: Vec::new(),
            children: Vec::new(),
        };

        let mut manager = RpfManager::default();
        // The top-level archive's path is real, so it stamps non-zero; the
        // nested ones have no path of their own, which is the point.
        let real =
            std::env::temp_dir().join(format!("astramapper-nest-{}.rpf", std::process::id()));
        std::fs::write(&real, b"payload").unwrap();
        manager.archives = vec![archive(&real.to_string_lossy()), archive(""), archive("")];
        // Archive 1 is nested in 0; archive 2 is nested in 1.
        manager.archive_parent = vec![None, Some(0), Some(1)];

        let stamps = manager.archive_stamps();
        assert_eq!(stamps.len(), 3);
        assert_ne!(stamps[0], 0, "a real file stamps");
        assert_eq!(stamps[1], stamps[0], "a nested archive inherits");
        assert_eq!(stamps[2], stamps[0], "and so does one nested two deep");
        assert_eq!(
            manager.stamp_of(EntryRef {
                archive: 2,
                entry: 0
            }),
            stamps[0]
        );

        let _ = std::fs::remove_file(&real);
    }

    #[test]
    fn normalizes_separators_and_case() {
        assert_eq!(
            normalize_path("X64A.rpf/Levels/GTA5"),
            r"x64a.rpf\levels\gta5"
        );
        assert_eq!(normalize_path(r"x64a.rpf\levels"), r"x64a.rpf\levels");
    }

    #[test]
    fn finds_no_rpfs_in_an_empty_directory() {
        let dir = std::env::temp_dir().join("astramapper-rpf-scan-empty");
        std::fs::create_dir_all(&dir).unwrap();
        let mut out = Vec::new();
        collect_rpf_paths(&dir, &mut out).unwrap();
        assert!(out.is_empty());
        let _ = std::fs::remove_dir(&dir);
    }

    /// A folder that does not exist is not an error — the reference's scan
    /// tolerates missing subdirectories, and the picker can hand us anything.
    #[test]
    fn tolerates_a_missing_directory() {
        let mut out = Vec::new();
        collect_rpf_paths(Path::new("astramapper-does-not-exist"), &mut out).unwrap();
        assert!(out.is_empty());
    }

    /// The name index keys an entry's stem by its Jenkins hash, in both the
    /// original case and the lowercase form.
    #[test]
    fn indexes_entry_basenames_by_hash() {
        use crate::rpf::entry::{BinaryEntry, EntryKind, RpfEncryption, RpfEntry};
        use crate::rpf::file::RpfFile;

        let entry = |name: &str| RpfEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            name_offset: 0,
            path: name.to_lowercase(),
            parent: None,
            h1: 0,
            h2: 0,
            kind: EntryKind::Binary(BinaryEntry {
                file_offset: 0,
                file_size: 0,
                uncompressed_size: 0,
                encrypted: false,
            }),
        };
        let manager = RpfManager {
            folder: PathBuf::new(),
            archives: vec![RpfFile {
                name: "dlc.rpf".into(),
                name_lower: "dlc.rpf".into(),
                path: "dlc.rpf".into(),
                physical_path: PathBuf::new(),
                file_size: 0,
                start_pos: 0,
                version: 7,
                entry_count: 2,
                names_length: 0,
                encryption: RpfEncryption::None,
                entries: vec![entry("prop_bin_04a.ydr"), entry("td_prop_ground_01.yft")],
                children: Vec::new(),
            }],
            archive_parent: vec![None],
            errors: Vec::new(),
            entry_index: HashMap::new(),
            archive_index: HashMap::new(),
            ytds_by_folder: OnceLock::new(),
            asset_index: OnceLock::new(),
            ytd_by_hash: OnceLock::new(),
            maps: ArchiveMaps::default(),
            stamps: OnceLock::new(),
        };

        let names = manager.index_names();
        assert_eq!(
            names.get(&crate::utils::gen_hash("prop_bin_04a")),
            Some(&"prop_bin_04a".to_string())
        );
        assert_eq!(
            names.get(&crate::utils::gen_hash("td_prop_ground_01")),
            Some(&"td_prop_ground_01".to_string())
        );
        assert_eq!(names.get(&crate::utils::gen_hash("prop_bin_04a.ydr")), None);
    }

    /// The `.ytd` folder index groups dictionaries by their containing folder.
    #[test]
    fn groups_ytds_by_folder() {
        use crate::rpf::entry::{BinaryEntry, EntryKind, RpfEncryption, RpfEntry};
        use crate::rpf::file::RpfFile;

        let entry = |name: &str, path: &str| RpfEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            name_offset: 0,
            path: path.to_string(),
            parent: None,
            h1: 0,
            h2: 0,
            kind: EntryKind::Binary(BinaryEntry {
                file_offset: 0,
                file_size: 0,
                uncompressed_size: 0,
                encrypted: false,
            }),
        };
        let manager = RpfManager {
            folder: PathBuf::new(),
            archives: vec![RpfFile {
                name: "dlc.rpf".into(),
                name_lower: "dlc.rpf".into(),
                path: "dlc.rpf".into(),
                physical_path: PathBuf::new(),
                file_size: 0,
                start_pos: 0,
                version: 7,
                entry_count: 3,
                names_length: 0,
                encryption: RpfEncryption::None,
                entries: vec![
                    entry("a.ytd", r"x64\levels\a.ytd"),
                    entry("b.ytd", r"x64\levels\b.ytd"),
                    entry("c.ytd", r"update\x64\c.ytd"),
                ],
                children: Vec::new(),
            }],
            archive_parent: vec![None],
            errors: Vec::new(),
            entry_index: HashMap::new(),
            archive_index: HashMap::new(),
            ytds_by_folder: OnceLock::new(),
            asset_index: OnceLock::new(),
            ytd_by_hash: OnceLock::new(),
            maps: ArchiveMaps::default(),
            stamps: OnceLock::new(),
        };

        let by_folder = manager.ytds();
        assert_eq!(
            by_folder.get(r"x64\levels").map(|v| v.len()),
            Some(2),
            "the folder should hold both dictionaries"
        );
        assert_eq!(by_folder.get(r"update\x64").map(|v| v.len()), Some(1));
    }

    /// The asset index picks the best model for a stem: `update` beats base,
    /// and a `.ydr` beats a `.yft` of the same name.
    #[test]
    fn asset_index_prefers_update_and_ydr() {
        use crate::rpf::entry::{BinaryEntry, EntryKind, RpfEncryption, RpfEntry};
        use crate::rpf::file::RpfFile;

        let entry = |path: &str| {
            let name = path.rsplit('\\').next().unwrap().to_string();
            RpfEntry {
                name: name.clone(),
                name_lower: name.to_lowercase(),
                name_offset: 0,
                path: path.to_string(),
                parent: None,
                h1: 0,
                h2: 0,
                kind: EntryKind::Binary(BinaryEntry {
                    file_offset: 0,
                    file_size: 0,
                    uncompressed_size: 0,
                    encrypted: false,
                }),
            }
        };
        let manager = RpfManager {
            folder: PathBuf::new(),
            archives: vec![RpfFile {
                name: "dlc.rpf".into(),
                name_lower: "dlc.rpf".into(),
                path: "dlc.rpf".into(),
                physical_path: PathBuf::new(),
                file_size: 0,
                start_pos: 0,
                version: 7,
                entry_count: 4,
                names_length: 0,
                encryption: RpfEncryption::None,
                entries: vec![
                    entry(r"x64\props\fence.ydr"),
                    entry(r"x64\props\fence.yft"),
                    entry(r"update\x64\props\fence.ydr"),
                    entry(r"x64\props\car.ydd"),
                ],
                children: Vec::new(),
            }],
            archive_parent: vec![None],
            errors: Vec::new(),
            entry_index: HashMap::new(),
            archive_index: HashMap::new(),
            ytds_by_folder: OnceLock::new(),
            asset_index: OnceLock::new(),
            ytd_by_hash: OnceLock::new(),
            maps: ArchiveMaps::default(),
            stamps: OnceLock::new(),
        };

        let index = manager.assets();
        let (update_flag, ext, path) = index.get("fence").expect("fence indexed");
        assert_eq!(*update_flag, 0, "the update copy wins");
        assert_eq!(*ext, 0, "and the .ydr over the .yft");
        assert_eq!(path, r"update\x64\props\fence.ydr");
        assert!(index.contains_key("car"));
    }
}
