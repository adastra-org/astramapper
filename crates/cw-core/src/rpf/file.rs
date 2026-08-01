//! Reading one RPF7 archive: header, table of contents, directory tree, nested
//! archives, and payload extraction.
//!
//! Port of `RpfFile.ReadHeader` / `ScanStructure` / `ExtractFile*` from
//! `CodeWalker.Core/GameFiles/RpfFile.cs`.
//!
//! Offsets in an RPF are in 512-byte sectors from the *archive* start, not the
//! file start. For a nested archive those differ, which is why `start_pos` is
//! threaded through everything instead of seeking from zero.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::entry::{
    huge_resource_size, read_kind, EntryKind, RpfEncryption, RpfEntry, DIRECTORY_MARKER,
    MAX_NAME_LEN, RESOURCE_SIZE_ESCAPE,
};
use crate::utils::{DataReader, GtaKeys};
use crate::{CwError, Result};

/// `RPF7`, the only version GTA V ships.
pub const RPF7: u32 = 0x5250_4637;

/// Payload offsets are counted in sectors of this size.
pub const SECTOR: u64 = 512;

/// A resource payload begins with a 16-byte RSC7 header, skipped on extraction.
const RSC7_HEADER_LEN: u32 = 0x10;

/// One archive: its header, its flat entry list, and any archives nested inside
/// it.
#[derive(Debug, Clone, Serialize)]
pub struct RpfFile {
    /// File name as stored, original case. NG key selection hashes *this*, not
    /// the lowercased form — `GTA5Hash.CalculateHash` does no case folding.
    pub name: String,
    pub name_lower: String,
    /// Lowercase path within the game folder, or within the parent archive.
    pub path: String,
    /// The `.rpf` on disk that ultimately holds these bytes. Nested archives
    /// inherit their top-level parent's path, as `GetPhysicalFilePath` does.
    pub physical_path: PathBuf,
    /// Size of this archive. For a nested one, the size of its entry in the
    /// parent — it also feeds NG key selection.
    pub file_size: u64,
    /// Byte offset of this archive within `physical_path`.
    pub start_pos: u64,

    pub version: u32,
    pub entry_count: u32,
    pub names_length: u32,
    pub encryption: RpfEncryption,

    /// Every entry, in table order. Directory children are contiguous runs of
    /// this list; index 0 is always the root directory.
    pub entries: Vec<RpfEntry>,
    /// Archives nested inside this one, already scanned.
    pub children: Vec<RpfFile>,
}

impl RpfFile {
    /// Open and fully scan an archive from disk, including nested archives.
    pub fn open(physical_path: &Path, rel_path: &str, keys: &GtaKeys) -> Result<Self> {
        let meta = std::fs::metadata(physical_path)?;
        let name = physical_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut reader = std::fs::File::open(physical_path)?;
        Self::scan(
            &mut reader,
            physical_path.to_path_buf(),
            name,
            rel_path.to_lowercase(),
            meta.len(),
            0,
            keys,
        )
    }

    /// Scan an archive at `start_pos` in an already-open reader.
    #[allow(clippy::too_many_arguments)]
    fn scan<R: Read + Seek>(
        reader: &mut R,
        physical_path: PathBuf,
        name: String,
        path: String,
        file_size: u64,
        start_pos: u64,
        keys: &GtaKeys,
    ) -> Result<Self> {
        let mut rpf = Self::read_header(
            reader,
            physical_path,
            name,
            path,
            file_size,
            start_pos,
            keys,
        )?;
        rpf.scan_children(reader, keys)?;
        Ok(rpf)
    }

    #[allow(clippy::too_many_arguments)]
    fn read_header<R: Read + Seek>(
        reader: &mut R,
        physical_path: PathBuf,
        name: String,
        path: String,
        file_size: u64,
        start_pos: u64,
        keys: &GtaKeys,
    ) -> Result<Self> {
        reader.seek(SeekFrom::Start(start_pos))?;

        let mut head = [0u8; 16];
        reader.read_exact(&mut head)?;
        let mut hr = DataReader::new(&head);
        let version = hr.read_u32()?;
        let entry_count = hr.read_u32()?;
        let names_length = hr.read_u32()?;
        let encryption_raw = hr.read_u32()?;

        if version != RPF7 {
            return Err(CwError::Invalid {
                what: "RPF archive",
                detail: format!("version {version:#010x} is not RPF7 ({RPF7:#010x}) — not GTA V"),
            });
        }

        let encryption = RpfEncryption::from_u32(encryption_raw);

        let table_len = entry_count as usize * 16;
        let mut entries_data = vec![0u8; table_len];
        reader.read_exact(&mut entries_data)?;
        let mut names_data = vec![0u8; names_length as usize];
        reader.read_exact(&mut names_data)?;

        // The archive's own name and size on disk select the NG key. Both are
        // wrong-able silently: a wrong one decrypts to noise, not an error.
        match encryption {
            RpfEncryption::None | RpfEncryption::Open => {}
            RpfEncryption::Aes => {
                entries_data = keys.decrypt_aes(&entries_data);
                names_data = keys.decrypt_aes(&names_data);
            }
            RpfEncryption::Ng => {
                let size = file_size as u32;
                entries_data = keys.decrypt_ng(&entries_data, &name, size);
                names_data = keys.decrypt_ng(&names_data, &name, size);
            }
        }

        let mut rpf = Self {
            name_lower: name.to_lowercase(),
            name,
            path,
            physical_path,
            file_size,
            start_pos,
            version,
            entry_count,
            names_length,
            encryption,
            entries: Vec::with_capacity(entry_count as usize),
            children: Vec::new(),
        };

        rpf.read_entries(reader, &entries_data, &names_data)?;
        rpf.build_tree()?;
        Ok(rpf)
    }

    fn read_entries<R: Read + Seek>(
        &mut self,
        reader: &mut R,
        entries_data: &[u8],
        names_data: &[u8],
    ) -> Result<()> {
        let mut er = DataReader::new(entries_data);

        for i in 0..self.entry_count as usize {
            // Peek the two header words to pick the layout, then rewind and
            // decode the full 16 bytes — the reference's `Position -= 8`.
            er.set_position(i * 16);
            let h1 = er.read_u32()?;
            let h2 = er.read_u32()?;
            er.set_position(i * 16);

            let (mut kind, name_offset) = read_kind(&mut er, h2)?;

            // A resource whose length field is saturated stores the real length
            // inside its own payload, so this needs a seek away from the table
            // and back. The reference saves and restores the position for the
            // same reason.
            if let EntryKind::Resource(res) = &mut kind {
                if res.file_size == RESOURCE_SIZE_ESCAPE {
                    let at = self.start_pos + res.file_offset as u64 * SECTOR;
                    let mut header = [0u8; 16];
                    reader.seek(SeekFrom::Start(at))?;
                    reader.read_exact(&mut header)?;
                    res.file_size = huge_resource_size(&header);
                }
            }

            let mut nr = DataReader::new(names_data);
            nr.set_position(name_offset as usize);
            let mut name = nr.read_string()?;
            if name.chars().count() > MAX_NAME_LEN {
                // Verbatim from the reference: "long names can freeze the
                // RPFExplorer". Counting chars, not bytes, because the C#
                // truncates a UTF-16 string by length.
                name = name.chars().take(MAX_NAME_LEN).collect();
            }
            let name_lower = name.to_lowercase();

            if let EntryKind::Resource(res) = &mut kind {
                res.encrypted = name_lower.ends_with(".ysc");
            }

            self.entries.push(RpfEntry {
                name,
                name_lower,
                name_offset,
                path: String::new(),
                parent: None,
                h1,
                h2,
                kind,
            });
        }

        Ok(())
    }

    /// Walk the directory tree, assigning parents and full paths.
    ///
    /// Children of a directory are the contiguous run
    /// `entries_index .. entries_index + entries_count`. The reference walks
    /// this with an explicit stack and no revisit guard, so a corrupt archive
    /// whose directory points at an ancestor loops forever. This keeps the
    /// stack walk but refuses to enter a directory twice.
    fn build_tree(&mut self) -> Result<()> {
        if self.entries.is_empty() {
            return Err(CwError::Invalid {
                what: "RPF archive",
                detail: "the entry table is empty; there is no root directory".into(),
            });
        }
        if !self.entries[0].is_directory() {
            return Err(CwError::Invalid {
                what: "RPF archive",
                detail: format!(
                    "the first entry is not a directory (marker {:#010x}, expected {DIRECTORY_MARKER:#010x}) — \
                     the table decrypted to noise",
                    self.entries[0].h2
                ),
            });
        }

        self.entries[0].path = self.path.clone();

        let total = self.entries.len();
        let mut visited = vec![false; total];
        let mut stack = vec![0usize];
        visited[0] = true;

        while let Some(idx) = stack.pop() {
            let (start, count) = match &self.entries[idx].kind {
                EntryKind::Directory(d) => (d.entries_index as usize, d.entries_count as usize),
                _ => continue,
            };
            let end = start.checked_add(count).ok_or_else(|| CwError::Invalid {
                what: "RPF directory entry",
                detail: format!("child range {start}+{count} overflows"),
            })?;
            if end > total {
                return Err(CwError::Invalid {
                    what: "RPF directory entry",
                    detail: format!(
                        "'{}' claims children {start}..{end} but the table holds {total} entries",
                        self.entries[idx].path
                    ),
                });
            }

            let parent_path = self.entries[idx].path.clone();
            // The index is needed for `visited` and for the stack, and the loop
            // writes back into `self.entries`, so an iterator would need a
            // second borrow of the thing it is iterating.
            #[allow(clippy::needless_range_loop)]
            for i in start..end {
                self.entries[i].parent = Some(idx);
                self.entries[i].path = format!("{}\\{}", parent_path, self.entries[i].name_lower);
                if self.entries[i].is_directory() && !visited[i] {
                    visited[i] = true;
                    stack.push(i);
                }
            }
        }

        Ok(())
    }

    /// Recurse into every nested `.rpf` stored as a binary entry.
    fn scan_children<R: Read + Seek>(&mut self, reader: &mut R, keys: &GtaKeys) -> Result<()> {
        let nested: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                matches!(e.kind, EntryKind::Binary(_))
                    && e.name_lower.ends_with(".rpf")
                    && is_valid_path(&e.path)
            })
            .map(|(i, _)| i)
            .collect();

        for i in nested {
            let entry = &self.entries[i];
            let offset = match entry.file_offset() {
                Some(o) => o,
                None => continue,
            };
            let child = Self::scan(
                reader,
                self.physical_path.clone(),
                entry.name.clone(),
                entry.path.clone(),
                entry.file_size_on_disk(),
                self.start_pos + offset as u64 * SECTOR,
                keys,
            );
            match child {
                Ok(c) => self.children.push(c),
                // The reference logs a per-entry error and keeps scanning; one
                // unreadable nested archive must not lose the whole parent.
                Err(_) => continue,
            }
        }

        Ok(())
    }

    /// The archive's root directory entry.
    pub fn root(&self) -> &RpfEntry {
        &self.entries[0]
    }

    /// Indices of a directory entry's immediate children.
    pub fn children_of(&self, index: usize) -> std::ops::Range<usize> {
        match &self.entries[index].kind {
            EntryKind::Directory(d) => {
                let start = d.entries_index as usize;
                let end = (start + d.entries_count as usize).min(self.entries.len());
                start..end.max(start)
            }
            _ => 0..0,
        }
    }

    /// Total entries across this archive and everything nested inside it.
    pub fn total_entry_count(&self) -> usize {
        self.entries.len()
            + self
                .children
                .iter()
                .map(|c| c.total_entry_count())
                .sum::<usize>()
    }

    /// Every archive in this subtree, including this one.
    pub fn iter_archives(&self) -> Box<dyn Iterator<Item = &RpfFile> + '_> {
        Box::new(std::iter::once(self).chain(self.children.iter().flat_map(|c| c.iter_archives())))
    }

    /// Read one entry's payload, decrypted and decompressed.
    pub fn extract(&self, entry: &RpfEntry, keys: &GtaKeys) -> Result<Vec<u8>> {
        let mut reader = std::fs::File::open(&self.physical_path)?;
        self.extract_from(&mut reader, entry, keys)
    }

    /// Read one entry's payload using an already-open reader over
    /// `physical_path`.
    pub fn extract_from<R: Read + Seek>(
        &self,
        reader: &mut R,
        entry: &RpfEntry,
        keys: &GtaKeys,
    ) -> Result<Vec<u8>> {
        match &entry.kind {
            EntryKind::Directory(_) => Err(CwError::Invalid {
                what: "RPF entry",
                detail: format!("'{}' is a directory, not a file", entry.path),
            }),
            EntryKind::Binary(b) => {
                let len = entry.file_size_on_disk();
                if len == 0 {
                    return Ok(Vec::new());
                }
                let at = self.start_pos + b.file_offset as u64 * SECTOR;
                reader.seek(SeekFrom::Start(at))?;
                let mut raw = vec![0u8; len as usize];
                reader.read_exact(&mut raw)?;

                if b.encrypted {
                    raw = self.decrypt_payload(raw, &entry.name, b.uncompressed_size, keys);
                }

                // A nonzero size-on-disk is what marks the payload compressed;
                // there is no separate flag.
                if b.file_size > 0 {
                    if let Some(out) = inflate(&raw) {
                        return Ok(out);
                    }
                }
                Ok(raw)
            }
            EntryKind::Resource(r) => {
                if r.file_size == 0 {
                    return Ok(Vec::new());
                }
                if r.file_size <= RSC7_HEADER_LEN {
                    return Err(CwError::Invalid {
                        what: "RPF resource entry",
                        detail: format!(
                            "'{}' is {} bytes, too small for its 16-byte RSC7 header",
                            entry.path, r.file_size
                        ),
                    });
                }
                let len = r.file_size - RSC7_HEADER_LEN;
                let at = self.start_pos + r.file_offset as u64 * SECTOR + RSC7_HEADER_LEN as u64;
                reader.seek(SeekFrom::Start(at))?;
                let mut raw = vec![0u8; len as usize];
                reader.read_exact(&mut raw)?;

                if r.encrypted {
                    raw = self.decrypt_payload(raw, &entry.name, r.file_size, keys);
                }

                // Resources are always deflate; the reference falls back to the
                // raw bytes when inflate fails rather than failing the read.
                //
                // TODO(port): the reference also subtracts the 16-byte header
                // from `entry.FileSize` on that fallback path, which changes
                // what a second extraction of the same entry reads. Not carried
                // across — it looks like a bug, not a format rule.
                Ok(inflate(&raw).unwrap_or(raw))
            }
        }
    }

    /// Decrypt an entry payload.
    ///
    /// The reference picks AES only when the *archive* is AES-encrypted and
    /// otherwise assumes NG, with the comment "fix for openIV modded files" —
    /// so a NONE/OPEN archive holding an encrypted entry takes the NG path.
    fn decrypt_payload(&self, data: Vec<u8>, name: &str, length: u32, keys: &GtaKeys) -> Vec<u8> {
        if self.encryption == RpfEncryption::Aes {
            keys.decrypt_aes(&data)
        } else {
            keys.decrypt_ng(&data, name, length)
        }
    }
}

/// Raw deflate, returning `None` on failure the way `DecompressBytes` returns
/// null. Payloads are padded to sector boundaries, so there is always trailing
/// garbage after the deflate stream ends; the decoder stops at the final block
/// and ignores it.
fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    use flate2::read::DeflateDecoder;
    let mut out = Vec::new();
    match DeflateDecoder::new(data).read_to_end(&mut out) {
        Ok(_) => Some(out),
        Err(_) => None,
    }
}

/// Port of `RpfFile.IsValidPath`, the guard on recursing into a nested archive.
/// Verbatim limits, including the reference's reasoning: a very long or very
/// deep path is taken as an attempt to crash the tool.
fn is_valid_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 500 {
        return false;
    }
    let mut depth = 0;
    for c in path.chars() {
        if c == ':' || c == ';' {
            return false;
        }
        if c == '/' || c == '\\' {
            depth += 1;
        }
    }
    depth <= 20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_the_reference_rejects() {
        assert!(is_valid_path(r"x64a.rpf\levels\gta5\test.rpf"));
        assert!(!is_valid_path(""));
        assert!(!is_valid_path("c:\\windows\\evil.rpf"));
        assert!(!is_valid_path("a;b.rpf"));
        assert!(!is_valid_path(&"a\\".repeat(21)));
        assert!(is_valid_path(&"a\\".repeat(20)));
        assert!(!is_valid_path(&"a".repeat(501)));
    }

    #[test]
    fn round_trips_raw_deflate() {
        use flate2::write::DeflateEncoder;
        use flate2::Compression;
        use std::io::Write;

        let payload = b"levels/gta5/_citye/downtown_01/dt1_01.ymap".repeat(8);
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&payload).unwrap();
        let compressed = enc.finish().unwrap();

        assert_eq!(inflate(&compressed).unwrap(), payload);
    }

    /// Sector padding means extraction always hands the decoder more bytes than
    /// the deflate stream needs. That must not be treated as corruption.
    #[test]
    fn ignores_trailing_sector_padding() {
        use flate2::write::DeflateEncoder;
        use flate2::Compression;
        use std::io::Write;

        let payload = b"padded to a sector boundary".to_vec();
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&payload).unwrap();
        let mut compressed = enc.finish().unwrap();
        compressed.resize(SECTOR as usize, 0xCD);

        assert_eq!(inflate(&compressed).unwrap(), payload);
    }

    #[test]
    fn reports_failed_inflation_rather_than_panicking() {
        assert!(inflate(&[0xFF; 32]).is_none());
    }
}
