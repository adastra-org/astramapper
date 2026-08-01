//! The RPF7 table-of-contents entry model.
//!
//! Port of the `RpfEntry` / `RpfDirectoryEntry` / `RpfBinaryFileEntry` /
//! `RpfResourceFileEntry` hierarchy at the bottom of
//! `CodeWalker.Core/GameFiles/RpfFile.cs`.
//!
//! The C# version is a class hierarchy with virtual `Read`/`Write`. Here it is
//! one struct with a kind enum: every entry is exactly 16 bytes and the three
//! layouts are disjoint, so a closed enum models it without the downcasting the
//! reference does everywhere (`e as RpfBinaryFileEntry`).
//!
//! The layouts are *not* the same field in three widths. A directory reads its
//! name offset as a full `u32`, a resource as a `u16`, and a binary entry takes
//! the low 16 bits of a packed `u64`. They agree only because name tables are
//! smaller than 64 KB; do not unify them.

use crate::utils::DataReader;
use crate::{CwError, Result};
use serde::Serialize;

/// A directory entry's discriminator, in the second `u32` of the 16 bytes.
pub const DIRECTORY_MARKER: u32 = 0x7FFF_FF00;

/// The reference truncates names past this length, with the comment "long names
/// can freeze the RPFExplorer".
pub const MAX_NAME_LEN: usize = 256;

/// How an archive's entry and name tables are encrypted.
///
/// `from_u32` maps every unrecognised value to `Ng`, which is what the
/// reference's `default:` case does. An archive with a garbage encryption field
/// is therefore attempted, not rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RpfEncryption {
    None,
    Open,
    Aes,
    Ng,
}

impl RpfEncryption {
    pub const NONE: u32 = 0;
    pub const OPEN: u32 = 0x4E45_504F; // "OPEN"
    pub const AES: u32 = 0x0FFF_FFF9;
    pub const NG: u32 = 0x0FEF_FFFF;

    pub fn from_u32(value: u32) -> Self {
        match value {
            Self::NONE => Self::None,
            Self::OPEN => Self::Open,
            Self::AES => Self::Aes,
            // Unknown values are assumed NG, exactly as `RpfFile.ReadHeader`
            // does. The reference's comment is "never seems to get here".
            _ => Self::Ng,
        }
    }

    /// Whether the tables carry no encryption at all.
    pub fn is_plain(self) -> bool {
        matches!(self, Self::None | Self::Open)
    }
}

/// One 16-byte table-of-contents entry.
#[derive(Debug, Clone, Serialize)]
pub struct RpfEntry {
    /// Name as stored, truncated to [`MAX_NAME_LEN`].
    pub name: String,
    /// Lowercased `name`, as the reference keeps alongside it. Paths and
    /// extension tests are built from this, never from `name`.
    pub name_lower: String,
    /// Byte offset of `name` in the archive's name table.
    pub name_offset: u32,
    /// Full lowercase path within the archive structure, built during the tree
    /// walk in [`crate::rpf::file::RpfFile`].
    pub path: String,
    /// Index of the containing directory in the archive's flat entry list.
    /// `None` for the root.
    pub parent: Option<usize>,
    /// The two raw header `u32`s the reference keeps as `H1`/`H2` for tooling.
    pub h1: u32,
    pub h2: u32,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Serialize)]
pub enum EntryKind {
    Directory(DirectoryEntry),
    Binary(BinaryEntry),
    Resource(ResourceEntry),
}

/// Children of a directory occupy a contiguous run of the flat entry list, so
/// the reference's separate `Directories`/`Files` vectors are derivable and are
/// not stored again here.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DirectoryEntry {
    pub entries_index: u32,
    pub entries_count: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BinaryEntry {
    pub file_offset: u32,
    /// Size on disk. Zero means the payload is stored uncompressed.
    pub file_size: u32,
    pub uncompressed_size: u32,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ResourceEntry {
    pub file_offset: u32,
    pub file_size: u32,
    pub system_flags: u32,
    pub graphics_flags: u32,
    /// The reference sets this from `name.EndsWith(".ysc")` and comments "any
    /// other way to know..?". Carrying the doubt across: there is no flag for
    /// it in the entry, so a differently named encrypted script would be missed
    /// here exactly as it is there.
    pub encrypted: bool,
}

impl RpfEntry {
    /// Whether this entry is a directory.
    pub fn is_directory(&self) -> bool {
        matches!(self.kind, EntryKind::Directory(_))
    }

    /// Whether this entry is a file of either flavour.
    pub fn is_file(&self) -> bool {
        !self.is_directory()
    }

    /// Name with the last extension removed, as `GetShortNameLower` does.
    pub fn short_name_lower(&self) -> &str {
        match self.name_lower.rfind('.') {
            Some(i) if i > 0 => &self.name_lower[..i],
            _ => &self.name_lower,
        }
    }

    /// Byte length of the entry's payload on disk.
    ///
    /// This is the reference's `GetFileSize()`: for both flavours a zero
    /// `file_size` means "not compressed", and the real length comes from
    /// elsewhere — the uncompressed size for binaries, the resource page flags
    /// for resources.
    pub fn file_size_on_disk(&self) -> u64 {
        match &self.kind {
            EntryKind::Directory(_) => 0,
            EntryKind::Binary(b) => {
                if b.file_size == 0 {
                    b.uncompressed_size as u64
                } else {
                    b.file_size as u64
                }
            }
            EntryKind::Resource(r) => {
                if r.file_size == 0 {
                    size_from_flags(r.system_flags) as u64
                        + size_from_flags(r.graphics_flags) as u64
                } else {
                    r.file_size as u64
                }
            }
        }
    }

    /// Offset of the payload, in 512-byte sectors from the archive start.
    pub fn file_offset(&self) -> Option<u32> {
        match &self.kind {
            EntryKind::Directory(_) => None,
            EntryKind::Binary(b) => Some(b.file_offset),
            EntryKind::Resource(r) => Some(r.file_offset),
        }
    }

    /// A resource entry's system and graphics segment sizes, decoded from its
    /// page flags. `None` for directories and binary files.
    ///
    /// This is the split point an extracted payload has to be cut at before its
    /// internal pointers mean anything — see
    /// [`crate::resources::ResourceReader`].
    pub fn resource_segments(&self) -> Option<(usize, usize)> {
        match &self.kind {
            EntryKind::Resource(r) => Some((
                size_from_flags(r.system_flags) as usize,
                size_from_flags(r.graphics_flags) as usize,
            )),
            _ => None,
        }
    }

    /// The resource format version, from the top nibble of each flag word.
    /// YDR and YDD are 165, YFT 162, YTD 13.
    pub fn resource_version(&self) -> Option<u32> {
        match &self.kind {
            EntryKind::Resource(r) => Some(crate::resources::version_from_flags(
                r.system_flags,
                r.graphics_flags,
            )),
            _ => None,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        match &self.kind {
            EntryKind::Directory(_) => false,
            EntryKind::Binary(b) => b.encrypted,
            EntryKind::Resource(r) => r.encrypted,
        }
    }
}

/// Decode the 16 bytes at the reader's position into an entry kind.
///
/// The caller has already peeked `h1`/`h2` to pick the variant; this consumes
/// the full 16 bytes either way, matching the reference's rewind-then-`Read`.
pub fn read_kind(r: &mut DataReader<'_>, h2: u32) -> Result<(EntryKind, u32)> {
    if h2 == DIRECTORY_MARKER {
        let name_offset = r.read_u32()?;
        let ident = r.read_u32()?;
        if ident != DIRECTORY_MARKER {
            return Err(CwError::Invalid {
                what: "RPF7 directory entry",
                detail: format!("identifier {ident:#010x} is not {DIRECTORY_MARKER:#010x}"),
            });
        }
        let entries_index = r.read_u32()?;
        let entries_count = r.read_u32()?;
        Ok((
            EntryKind::Directory(DirectoryEntry {
                entries_index,
                entries_count,
            }),
            name_offset,
        ))
    } else if h2 & 0x8000_0000 == 0 {
        // Binary entry: name offset, size and offset are bit-packed into one
        // u64, then two more u32s.
        let buf = r.read_u64()?;
        let name_offset = (buf & 0xFFFF) as u32;
        let file_size = ((buf >> 16) & 0xFF_FFFF) as u32;
        let file_offset = ((buf >> 40) & 0xFF_FFFF) as u32;
        let uncompressed_size = r.read_u32()?;
        let encryption_type = r.read_u32()?;
        let encrypted = match encryption_type {
            0 => false,
            1 => true,
            other => {
                return Err(CwError::Invalid {
                    what: "RPF7 file entry",
                    detail: format!("unknown encryption type {other}"),
                })
            }
        };
        Ok((
            EntryKind::Binary(BinaryEntry {
                file_offset,
                file_size,
                uncompressed_size,
                encrypted,
            }),
            name_offset,
        ))
    } else {
        let name_offset = r.read_u16()? as u32;
        let b = r.read_bytes(3)?;
        let file_size = b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16;
        let b = r.read_bytes(3)?;
        let file_offset = (b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16) & 0x7F_FFFF;
        let system_flags = r.read_u32()?;
        let graphics_flags = r.read_u32()?;
        Ok((
            EntryKind::Resource(ResourceEntry {
                file_offset,
                file_size,
                system_flags,
                graphics_flags,
                // Set by the caller once the name is known.
                encrypted: false,
            }),
            name_offset,
        ))
    }
}

/// A resource whose size field reads `0xFFFFFF` is really `>= 0xFFFFFF`; the
/// true length is spread across four bytes of its 16-byte payload header.
pub const RESOURCE_SIZE_ESCAPE: u32 = 0xFF_FFFF;

/// Unpack the real length from the first 16 bytes of an oversized resource's
/// payload. The byte order is genuinely this scrambled in the reference.
pub fn huge_resource_size(header: &[u8; 16]) -> u32 {
    (header[7] as u32)
        | (header[14] as u32) << 8
        | (header[5] as u32) << 16
        | (header[2] as u32) << 24
}

/// Decode a resource page-flags word into a byte count.
///
/// Port of `RpfResourceFileEntry.GetSizeFromFlags`. The low 4 bits scale a
/// 0x200 base; the rest are nine page counts at nine sizes. Computed in `u64`
/// and truncated, because the reference's mixed `int`/`uint` arithmetic widens
/// to `long` and then casts back to `int`.
pub fn size_from_flags(flags: u32) -> u32 {
    let s0 = (flags >> 27) & 0x1;
    let s1 = ((flags >> 26) & 0x1) << 1;
    let s2 = ((flags >> 25) & 0x1) << 2;
    let s3 = ((flags >> 24) & 0x1) << 3;
    let s4 = ((flags >> 17) & 0x7F) << 4;
    let s5 = ((flags >> 11) & 0x3F) << 5;
    let s6 = ((flags >> 7) & 0xF) << 6;
    let s7 = ((flags >> 5) & 0x3) << 7;
    let s8 = ((flags >> 4) & 0x1) << 8;
    let pages = (s0 + s1 + s2 + s3 + s4 + s5 + s6 + s7 + s8) as u64;
    let base_size = 0x200u64 << (flags & 0xF);
    (base_size * pages) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value here came out of the reference, not out of the formula:
    ///
    /// ```powershell
    /// Add-Type -Path "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll"
    /// [CodeWalker.GameFiles.RpfResourceFileEntry]::GetSizeFromFlags([uint32]0x08000004)
    /// ```
    #[test]
    fn decodes_resource_page_flags() {
        assert_eq!(size_from_flags(0x0000_0000), 0);
        assert_eq!(size_from_flags(0x0800_0004), 8192);
        assert_eq!(size_from_flags(0x0400_0000), 1024);
        assert_eq!(size_from_flags(0x0002_0000), 8192);
        assert_eq!(size_from_flags(0x6000_0009), 0);
    }

    /// `GetSizeFromFlags` returns `int` and its arithmetic widens to `long`
    /// before being cast back, so saturated flag words overflow into negative
    /// numbers. The reference really does return -1627389952 for `0x0FEFFFFF`.
    /// This port computes in `u64` and truncates, which reproduces the same 32
    /// bits — a check that the whole bit layout is right, since any misplaced
    /// shift would land somewhere else entirely.
    #[test]
    fn reproduces_the_reference_overflow_bit_for_bit() {
        assert_eq!(size_from_flags(0xFFFF_FFFF), 520_093_696);
        assert_eq!(size_from_flags(0x0FFF_FFFF), 520_093_696);
        assert_eq!(size_from_flags(0x0000_0FFF), 1_610_612_736);
        assert_eq!(size_from_flags(0x0FEF_FFFF), (-1_627_389_952i32) as u32);
    }

    #[test]
    fn maps_unknown_encryption_to_ng() {
        assert_eq!(RpfEncryption::from_u32(0), RpfEncryption::None);
        assert_eq!(RpfEncryption::from_u32(0x4E45_504F), RpfEncryption::Open);
        assert_eq!(RpfEncryption::from_u32(0x0FFF_FFF9), RpfEncryption::Aes);
        assert_eq!(RpfEncryption::from_u32(0x0FEF_FFFF), RpfEncryption::Ng);
        assert_eq!(RpfEncryption::from_u32(0xDEAD_BEEF), RpfEncryption::Ng);
    }

    #[test]
    fn reads_a_directory_entry() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&7u32.to_le_bytes());
        bytes.extend_from_slice(&DIRECTORY_MARKER.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());

        let mut r = DataReader::new(&bytes);
        let (kind, name_offset) = read_kind(&mut r, DIRECTORY_MARKER).unwrap();
        assert_eq!(name_offset, 7);
        assert_eq!(r.position(), 16);
        match kind {
            EntryKind::Directory(d) => {
                assert_eq!(d.entries_index, 1);
                assert_eq!(d.entries_count, 3);
            }
            other => panic!("expected a directory, got {other:?}"),
        }
    }

    #[test]
    fn reads_a_binary_entry() {
        // name offset 0x0042, size on disk 0x001234, offset 0x005678.
        let packed: u64 = 0x0042 | (0x0000_1234u64 << 16) | (0x0000_5678u64 << 40);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&packed.to_le_bytes());
        bytes.extend_from_slice(&0x9999u32.to_le_bytes()); // uncompressed
        bytes.extend_from_slice(&1u32.to_le_bytes()); // encrypted

        let h2 = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        assert_eq!(
            h2 & 0x8000_0000,
            0,
            "test vector must look like a binary entry"
        );

        let mut r = DataReader::new(&bytes);
        let (kind, name_offset) = read_kind(&mut r, h2).unwrap();
        assert_eq!(name_offset, 0x42);
        assert_eq!(r.position(), 16);
        match kind {
            EntryKind::Binary(b) => {
                assert_eq!(b.file_size, 0x1234);
                assert_eq!(b.file_offset, 0x5678);
                assert_eq!(b.uncompressed_size, 0x9999);
                assert!(b.encrypted);
            }
            other => panic!("expected a binary file, got {other:?}"),
        }
    }

    #[test]
    fn rejects_an_unknown_binary_encryption_type() {
        let mut bytes = vec![0u8; 12];
        bytes.extend_from_slice(&2u32.to_le_bytes());
        assert!(read_kind(&mut DataReader::new(&bytes), 0).is_err());
    }

    #[test]
    fn reads_a_resource_entry() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x0042u16.to_le_bytes());
        bytes.extend_from_slice(&[0x34, 0x12, 0x00]); // size 0x001234
                                                      // The top bit of the offset field is the resource discriminator and is
                                                      // masked off by the reader.
        bytes.extend_from_slice(&[0x78, 0x56, 0x80]);
        bytes.extend_from_slice(&0xAAAAu32.to_le_bytes());
        bytes.extend_from_slice(&0xBBBBu32.to_le_bytes());

        let h2 = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        assert_ne!(h2 & 0x8000_0000, 0, "test vector must look like a resource");

        let mut r = DataReader::new(&bytes);
        let (kind, name_offset) = read_kind(&mut r, h2).unwrap();
        assert_eq!(name_offset, 0x42);
        assert_eq!(r.position(), 16);
        match kind {
            EntryKind::Resource(res) => {
                assert_eq!(res.file_size, 0x1234);
                assert_eq!(
                    res.file_offset, 0x5678,
                    "the 0x80 marker bit must be masked off"
                );
                assert_eq!(res.system_flags, 0xAAAA);
                assert_eq!(res.graphics_flags, 0xBBBB);
            }
            other => panic!("expected a resource, got {other:?}"),
        }
    }

    #[test]
    fn unpacks_an_oversized_resource_length() {
        let mut header = [0u8; 16];
        header[7] = 0x11;
        header[14] = 0x22;
        header[5] = 0x33;
        header[2] = 0x44;
        assert_eq!(huge_resource_size(&header), 0x4433_2211);
    }

    #[test]
    fn strips_the_last_extension_only() {
        let entry = |n: &str| RpfEntry {
            name: n.to_string(),
            name_lower: n.to_lowercase(),
            name_offset: 0,
            path: String::new(),
            parent: None,
            h1: 0,
            h2: 0,
            kind: EntryKind::Directory(DirectoryEntry {
                entries_index: 0,
                entries_count: 0,
            }),
        };
        assert_eq!(
            entry("prop_bench_01a.ydr").short_name_lower(),
            "prop_bench_01a"
        );
        assert_eq!(entry("v_res_d_bskt.ytd").short_name_lower(), "v_res_d_bskt");
        assert_eq!(entry("noextension").short_name_lower(), "noextension");
        // A leading dot is not an extension separator.
        assert_eq!(entry(".hidden").short_name_lower(), ".hidden");
    }
}
