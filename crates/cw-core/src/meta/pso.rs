//! PSO: the metadata container that carries its own schema.
//!
//! Port of `PsoFile` and the section types from
//! `CodeWalker.Core/GameFiles/MetaTypes/Pso.cs`. Read only.
//!
//! A PSO file is a run of `{ident, length, payload}` sections. Three matter:
//!
//! - `PSIN` — the data. One flat byte blob; every value in the file is at an
//!   offset into it.
//! - `PMAP` — the block table. Each block names a structure and marks a range
//!   of the data blob, and the root block is the entry point.
//! - `PSCH` — the schema: for each structure, the offset, type and name hash of
//!   every field, and for each enum, its values. This is why a PSO can be
//!   walked without `PsoTypes.cs`, the reference's 1.5 MB table of hand-written
//!   definitions: the file describes itself.
//!
//! Two things that are not guessable:
//!
//! 1. **It is big-endian**, on PC, in a little-endian game. `PsoFile.Load`
//!    opens a `BigEndian` reader and every field below is read through it.
//! 2. **A section's payload includes its own 8-byte header.** `Read` rewinds by
//!    8 before taking `Length` bytes, so every offset in the file is relative
//!    to the start of the header, not to the start of the payload.

use serde::Serialize;

use crate::utils::DataReader;
use crate::{CwError, Result};

/// `PSIN`, big-endian. `PsoFile.IsPSO` tests this and nothing else.
const PSIN: u32 = 0x5053_494E;
const PMAP: u32 = 0x504D_4150;
const PSCH: u32 = 0x5053_4348;
const STRF: u32 = 0x5354_5246;
const STRS: u32 = 0x5354_5253;
const STRE: u32 = 0x5354_5245;

/// Guard on a count read out of the file before it sizes an allocation.
const MAX_ELEMENTS: usize = 1_000_000;

/// What a field holds. `PsoDataType` in the reference; the gaps in the values
/// are the reference's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PsoDataType {
    Bool,
    SByte,
    UByte,
    SShort,
    UShort,
    SInt,
    UInt,
    Float,
    Float2,
    Float3,
    Float4,
    String,
    Structure,
    Array,
    Enum,
    Flags,
    Map,
    Float3a,
    Float4a,
    HFloat,
    Long,
    /// A value the reference does not name. Kept rather than refused, so an
    /// unexpected field shows up as unreadable instead of failing the file.
    Unknown(u8),
}

impl PsoDataType {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0x00 => Self::Bool,
            0x01 => Self::SByte,
            0x02 => Self::UByte,
            0x03 => Self::SShort,
            0x04 => Self::UShort,
            0x05 => Self::SInt,
            0x06 => Self::UInt,
            0x07 => Self::Float,
            0x08 => Self::Float2,
            0x09 => Self::Float3,
            0x0A => Self::Float4,
            0x0B => Self::String,
            0x0C => Self::Structure,
            0x0D => Self::Array,
            0x0E => Self::Enum,
            0x0F => Self::Flags,
            0x10 => Self::Map,
            0x14 => Self::Float3a,
            0x15 => Self::Float4a,
            0x1E => Self::HFloat,
            0x20 => Self::Long,
            other => Self::Unknown(other),
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Unknown(raw) => format!("unknown({raw:#04x})"),
            other => format!("{other:?}"),
        }
    }
}

/// One block of the data section: a structure, and where its bytes are.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PsoBlock {
    pub name_hash: u32,
    pub offset: i32,
    pub length: i32,
}

/// One field of a structure.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PsoStructureEntry {
    pub name_hash: u32,
    pub data_type: PsoDataType,
    /// The subtype. Which meaning it carries depends on `data_type` — for an
    /// array it says where the elements live, for an enum how wide the value
    /// is, for a structure whether it is inline or a pointer.
    pub unknown_5h: u8,
    pub data_offset: u16,
    /// For an array, the element entry index and count, packed. For a
    /// structure, the name hash of the structure. For an enum, the enum's.
    pub reference_key: u32,
}

/// A structure definition.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PsoStructure {
    pub name_hash: u32,
    pub length: i32,
    pub entries: Vec<PsoStructureEntry>,
}

impl PsoStructure {
    pub fn entry(&self, index: usize) -> Option<&PsoStructureEntry> {
        self.entries.get(index)
    }

    /// The first field with this name, which is how a map's `Key` and `Item`
    /// are found.
    pub fn find(&self, name_hash: u32) -> Option<&PsoStructureEntry> {
        self.entries.iter().find(|e| e.name_hash == name_hash)
    }
}

/// An enum definition.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PsoEnum {
    pub name_hash: u32,
    /// Name hash and value, in declaration order.
    pub entries: Vec<(u32, i32)>,
}

impl PsoEnum {
    pub fn find(&self, value: i32) -> Option<u32> {
        self.entries
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(name, _)| *name)
    }
}

/// A parsed PSO container.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PsoFile {
    /// The whole `PSIN` section, its 8-byte header included: every offset in
    /// the file is relative to this.
    #[serde(skip)]
    pub data: Vec<u8>,
    /// Block id of the root structure. Blocks are numbered from 1.
    pub root_id: i32,
    pub blocks: Vec<PsoBlock>,
    pub structures: Vec<PsoStructure>,
    pub enums: Vec<PsoEnum>,
    /// `STRF` and `STRS`: file paths and names, kept because the data section
    /// references strings by index into them.
    pub file_strings: Vec<String>,
    pub strings: Vec<String>,
    /// True when the file carries a `STRE` section. Nobody has worked out its
    /// encryption, the reference included, so the bytes are not kept.
    pub has_encrypted_strings: bool,
}

impl PsoFile {
    /// `PsoFile.GetBlock`: blocks are numbered from 1.
    pub fn block(&self, id: i32) -> Option<&PsoBlock> {
        if id < 1 {
            return None;
        }
        self.blocks.get((id - 1) as usize)
    }

    pub fn structure(&self, name_hash: u32) -> Option<&PsoStructure> {
        self.structures.iter().find(|s| s.name_hash == name_hash)
    }

    pub fn enumeration(&self, name_hash: u32) -> Option<&PsoEnum> {
        self.enums.iter().find(|e| e.name_hash == name_hash)
    }
}

/// Whether a payload starts with the PSO magic.
pub fn is_pso(data: &[u8]) -> bool {
    data.len() >= 4 && u32::from_be_bytes([data[0], data[1], data[2], data[3]]) == PSIN
}

/// Parse a PSO payload.
pub fn read(data: &[u8]) -> Result<PsoFile> {
    if !is_pso(data) {
        return Err(CwError::Invalid {
            what: "pso",
            detail: "the file does not start with a PSIN section".into(),
        });
    }

    let mut file = PsoFile::default();
    let mut r = DataReader::new_big_endian(data);

    while r.remaining() > 8 {
        let start = r.position();
        let ident = r.read_u32()?;
        let length = r.read_i32()?;
        if length < 8 {
            return Err(CwError::Invalid {
                what: "pso",
                detail: format!("a section of {length} bytes at {start}"),
            });
        }
        // The reference rewinds and reads `Length` bytes, so a section's own
        // header is part of its payload and offsets inside it count from there.
        r.set_position(start);
        let section = r.read_bytes(length as usize)?;

        match ident {
            PSIN => file.data = section.to_vec(),
            PMAP => read_data_map(&mut file, section)?,
            PSCH => read_schema(&mut file, section)?,
            STRF => file.file_strings = read_strings(section)?,
            STRS => file.strings = read_strings(section)?,
            STRE => file.has_encrypted_strings = true,
            // PSIG and CHKS are a signature and a checksum; the reference reads
            // them into blobs it never looks at.
            _ => {}
        }
    }

    Ok(file)
}

fn read_data_map(file: &mut PsoFile, section: &[u8]) -> Result<()> {
    let mut r = DataReader::new_big_endian(section);
    let _ident = r.read_i32()?;
    let _length = r.read_i32()?;
    file.root_id = r.read_i32()?;
    let mut count = r.read_i16()?;
    let _unknown_eh = r.read_i16()?;

    // "any other way to know which version?" — the reference's own comment. A
    // non-positive count means the real one is four fields further on.
    if count <= 0 {
        count = r.read_i16()?;
        let _unknown_0 = r.read_i16()?;
        let _unknown_1 = r.read_i16()?;
        let _unknown_2 = r.read_i16()?;
    }

    let count = (count.max(0) as usize).min(MAX_ELEMENTS);
    file.blocks = Vec::with_capacity(count);
    for _ in 0..count {
        let name_hash = r.read_u32()?;
        let offset = r.read_i32()?;
        let _unknown_8h = r.read_i32()?;
        let length = r.read_i32()?;
        file.blocks.push(PsoBlock {
            name_hash,
            offset,
            length,
        });
    }
    Ok(())
}

fn read_schema(file: &mut PsoFile, section: &[u8]) -> Result<()> {
    let mut r = DataReader::new_big_endian(section);
    let _ident = r.read_i32()?;
    let _length = r.read_i32()?;
    let count = (r.read_u32()? as usize).min(MAX_ELEMENTS);

    let mut index = Vec::with_capacity(count);
    for _ in 0..count {
        let name_hash = r.read_u32()?;
        let offset = r.read_i32()?;
        index.push((name_hash, offset));
    }

    for (name_hash, offset) in index {
        if offset < 0 || offset as usize >= section.len() {
            continue;
        }
        r.set_position(offset as usize);
        // The kind is the top byte of the word the definition starts with.
        let kind = section[offset as usize];
        match kind {
            0 => {
                let x = r.read_u32()?;
                let entries_count = ((x & 0xFFFF) as usize).min(MAX_ELEMENTS);
                let length = r.read_i32()?;
                let _unknown_ch = r.read_u32()?;

                let mut entries = Vec::with_capacity(entries_count);
                for _ in 0..entries_count {
                    entries.push(PsoStructureEntry {
                        name_hash: r.read_u32()?,
                        data_type: PsoDataType::from_raw(r.read_u8()?),
                        unknown_5h: r.read_u8()?,
                        data_offset: r.read_u16()?,
                        reference_key: r.read_u32()?,
                    });
                }
                file.structures.push(PsoStructure {
                    name_hash,
                    length,
                    entries,
                });
            }
            1 => {
                let x = r.read_u32()?;
                let entries_count = ((x & 0x00FF_FFFF) as usize).min(MAX_ELEMENTS);
                let mut entries = Vec::with_capacity(entries_count);
                for _ in 0..entries_count {
                    let name = r.read_u32()?;
                    let value = r.read_i32()?;
                    entries.push((name, value));
                }
                file.enums.push(PsoEnum { name_hash, entries });
            }
            other => {
                return Err(CwError::Invalid {
                    what: "pso",
                    detail: format!("schema entry of unknown kind {other}"),
                })
            }
        }
    }
    Ok(())
}

/// A string section is NUL-terminated strings from the end of its header to
/// the end of the section.
fn read_strings(section: &[u8]) -> Result<Vec<String>> {
    let mut r = DataReader::new_big_endian(section);
    let _ident = r.read_i32()?;
    let _length = r.read_i32()?;

    let mut strings = Vec::new();
    while r.remaining() > 0 {
        strings.push(r.read_string()?);
    }
    Ok(strings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(ident: u32, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&ident.to_be_bytes());
        out.extend_from_slice(&((body.len() + 8) as i32).to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    fn synthetic() -> Vec<u8> {
        let mut data = section(PSIN, &[1, 2, 3, 4]);

        // One block: structure 0xAABBCCDD at offset 8, 4 bytes long.
        let mut map = Vec::new();
        map.extend_from_slice(&1i32.to_be_bytes()); // root id
        map.extend_from_slice(&1i16.to_be_bytes()); // entries
        map.extend_from_slice(&0x7070i16.to_be_bytes());
        map.extend_from_slice(&0xAABB_CCDDu32.to_be_bytes());
        map.extend_from_slice(&8i32.to_be_bytes());
        map.extend_from_slice(&0i32.to_be_bytes());
        map.extend_from_slice(&4i32.to_be_bytes());
        data.extend_from_slice(&section(PMAP, &map));

        // A schema of one structure with one float field, and one enum.
        let mut schema = Vec::new();
        schema.extend_from_slice(&2u32.to_be_bytes()); // count
        schema.extend_from_slice(&0xAABB_CCDDu32.to_be_bytes());
        // Offsets count from the section header: 12 bytes of it, then the
        // 16-byte index, so the first definition lands at 28 and the 12-byte
        // structure entry after it puts the enum at 52.
        schema.extend_from_slice(&28i32.to_be_bytes());
        schema.extend_from_slice(&0x1111_2222u32.to_be_bytes());
        schema.extend_from_slice(&52i32.to_be_bytes());
        schema.extend_from_slice(&0x0000_0001u32.to_be_bytes()); // kind 0, 1 entry
        schema.extend_from_slice(&4i32.to_be_bytes()); // structure length
        schema.extend_from_slice(&0u32.to_be_bytes());
        schema.extend_from_slice(&0x3333_4444u32.to_be_bytes()); // field name
        schema.push(0x07); // Float
        schema.push(0);
        schema.extend_from_slice(&0u16.to_be_bytes()); // offset
        schema.extend_from_slice(&0u32.to_be_bytes());
        schema.extend_from_slice(&0x0100_0001u32.to_be_bytes()); // kind 1, 1 entry
        schema.extend_from_slice(&0x5555_6666u32.to_be_bytes());
        schema.extend_from_slice(&3i32.to_be_bytes());
        data.extend_from_slice(&section(PSCH, &schema));

        data.extend_from_slice(&section(STRS, b"first\0second\0"));
        data
    }

    #[test]
    fn reads_the_sections_a_walker_needs() {
        let file = read(&synthetic()).unwrap();

        assert_eq!(file.root_id, 1);
        assert_eq!(
            file.data.len(),
            12,
            "the PSIN header is part of its payload"
        );

        let block = file.block(1).expect("root block");
        assert_eq!(block.name_hash, 0xAABB_CCDD);
        assert_eq!(block.offset, 8);
        assert!(file.block(0).is_none(), "blocks are numbered from 1");

        let structure = file.structure(0xAABB_CCDD).expect("structure");
        assert_eq!(structure.length, 4);
        assert_eq!(structure.entries.len(), 1);
        assert_eq!(structure.entries[0].data_type, PsoDataType::Float);
        assert_eq!(structure.entries[0].name_hash, 0x3333_4444);

        let enumeration = file.enumeration(0x1111_2222).expect("enum");
        assert_eq!(enumeration.find(3), Some(0x5555_6666));
        assert_eq!(enumeration.find(4), None);

        assert_eq!(file.strings, vec!["first", "second"]);
        assert!(!file.has_encrypted_strings);
    }

    #[test]
    fn recognises_and_rejects_by_magic() {
        assert!(is_pso(&synthetic()));
        assert!(!is_pso(b"RBF0"));
        assert!(read(b"RBF0\0\0\0\0").is_err());
    }
}
