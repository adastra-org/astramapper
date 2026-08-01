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

use rayon::prelude::*;
use serde::Serialize;

use super::entry::{EntryKind, RpfEntry};
use super::file::RpfFile;
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
    pub fn extract(&self, r: EntryRef, keys: &GtaKeys) -> Result<Vec<u8>> {
        let archive = &self.archives[r.archive];
        archive.extract(&archive.entries[r.entry], keys)
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
}
