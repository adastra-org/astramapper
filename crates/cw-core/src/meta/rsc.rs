//! RSC meta: the metadata encoding that rides inside an RSC7 resource.
//!
//! Port of `Meta`, `MetaStructureInfo`, `MetaEnumInfo` and `MetaDataBlock` from
//! `CodeWalker.Core/GameFiles/MetaTypes/Meta.cs`. Read only.
//!
//! This is what a `.ymap`, a `.ytyp` and a resource-entry `.ymt` are made of.
//! Like PSO it carries its own schema, so it can be walked without the
//! reference's generated type tables; unlike PSO it is little-endian, and its
//! data is split into numbered blocks rather than one blob.
//!
//! The pointer format inside those blocks is the thing to know: a "pointer" is
//! not an address but a **block id in the low 12 bits and an offset into that
//! block in the next 20**. So an array of structures is `{packed pointer,
//! count, count, pad}`, and walking it means resolving a block each time — and
//! stepping into the *next* block when a long array runs off the end of one.

use serde::Serialize;

use crate::resources::reader::ResourceReader;
use crate::Result;

/// Guard on a count read out of the file before it sizes an allocation.
const MAX_ELEMENTS: usize = 1_000_000;

/// What a field holds. `MetaStructureEntryDataType` in the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MetaDataType {
    Boolean,
    SignedByte,
    UnsignedByte,
    SignedShort,
    UnsignedShort,
    SignedInt,
    UnsignedInt,
    Float,
    FloatXyz,
    FloatXyzw,
    ByteEnum,
    IntEnum,
    ShortFlags,
    IntFlags1,
    IntFlags2,
    Hash,
    Array,
    ArrayOfChars,
    ArrayOfBytes,
    DataBlockPointer,
    CharPointer,
    StructurePointer,
    Structure,
    /// Not one of the reference's values. Kept rather than refused: an
    /// unexpected field shows as unreadable instead of failing the file.
    Unknown(u8),
}

impl MetaDataType {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            0x01 => Self::Boolean,
            0x10 => Self::SignedByte,
            0x11 => Self::UnsignedByte,
            0x12 => Self::SignedShort,
            0x13 => Self::UnsignedShort,
            0x14 => Self::SignedInt,
            0x15 => Self::UnsignedInt,
            0x21 => Self::Float,
            0x33 => Self::FloatXyz,
            0x34 => Self::FloatXyzw,
            0x60 => Self::ByteEnum,
            0x62 => Self::IntEnum,
            0x64 => Self::ShortFlags,
            0x63 => Self::IntFlags1,
            0x65 => Self::IntFlags2,
            0x4A => Self::Hash,
            0x52 => Self::Array,
            0x40 => Self::ArrayOfChars,
            0x50 => Self::ArrayOfBytes,
            0x59 => Self::DataBlockPointer,
            0x44 => Self::CharPointer,
            0x07 => Self::StructurePointer,
            0x05 => Self::Structure,
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

/// The name hash `MetaTypeName.ARRAYINFO` uses. An entry under this name is
/// not a field: it describes the element type of the array field before it.
pub const ARRAY_INFO: u32 = 0x100;
/// `MetaTypeName.STRING`, the structure name a block of string bytes carries.
pub const STRING_BLOCK: u32 = 0x10;
/// `MetaTypeName.POINTER`, the structure name a block of `MetaPOINTER` carries.
pub const POINTER_BLOCK: u32 = 0x07;

/// One field of a structure. `MetaStructureEntryInfo_s`, 16 bytes.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaEntry {
    pub name_hash: u32,
    pub data_offset: i32,
    pub data_type: MetaDataType,
    pub unknown_9h: u8,
    pub reference_type_index: i16,
    /// The structure or enum this field refers to, or a length for the fixed
    /// arrays. Which it means depends on `data_type`.
    pub reference_key: u32,
}

/// A structure definition. `MetaStructureInfo`, 32 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaStructure {
    pub name_hash: u32,
    pub key: u32,
    /// The stride of one element of this structure in a data block.
    pub size: i32,
    pub entries: Vec<MetaEntry>,
}

/// An enum definition. `MetaEnumInfo`, 24 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaEnum {
    pub name_hash: u32,
    pub key: u32,
    /// Name hash and value, in declaration order.
    pub entries: Vec<(u32, i32)>,
}

/// One numbered block of data. `MetaDataBlock`, 16 bytes plus its payload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaBlock {
    /// The structure every element in this block is an instance of, or one of
    /// the `MetaTypeName` pseudo-types for a block of strings or pointers.
    pub structure_name_hash: u32,
    #[serde(skip)]
    pub data: Vec<u8>,
}

impl MetaBlock {
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// A parsed RSC meta.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// Index of the block the tree starts at. Blocks are numbered from 1.
    pub root_block_index: i32,
    pub structures: Vec<MetaStructure>,
    pub enums: Vec<MetaEnum>,
    pub blocks: Vec<MetaBlock>,
    /// The file's own name, when it carries one.
    pub name: Option<String>,
    /// True when the file has a `MetaEncryptedStringsBlock`. Nobody has worked
    /// out that encryption, the reference included.
    pub has_encrypted_strings: bool,
}

impl Meta {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        // ResourceFileBase.
        let _file_vft = r.read_u32()?;
        let _file_unknown = r.read_u32()?;
        let _file_pages_info_pointer = r.read_u64()?;

        let _unknown_10h = r.read_u32()?;
        let _unknown_14h = r.read_u16()?;
        let has_encrypted_strings = r.read_u8()?;
        let _unknown_17h = r.read_u8()?;
        let _unknown_18h = r.read_u32()?;
        let root_block_index = r.read_u32()? as i32;
        let structures_pointer = r.read_u64()?;
        let enums_pointer = r.read_u64()?;
        let blocks_pointer = r.read_u64()?;
        let name_pointer = r.read_u64()?;
        let _encrypted_strings_pointer = r.read_u64()?;
        let structures_count = r.read_u16()? as usize;
        let enums_count = r.read_u16()? as usize;
        let blocks_count = r.read_u16()? as usize;

        let mut meta = Self {
            root_block_index,
            name: r.read_string_at(name_pointer)?,
            has_encrypted_strings: has_encrypted_strings != 0,
            ..Default::default()
        };

        // Each of the three is a `ResourceSimpleArray`: elements end to end at
        // the pointer, no per-element indirection.
        for i in 0..structures_count.min(MAX_ELEMENTS) {
            let at = structures_pointer + (i * 32) as u64;
            if let Some(structure) = r.at(at, read_structure)? {
                meta.structures.push(structure);
            }
        }
        for i in 0..enums_count.min(MAX_ELEMENTS) {
            let at = enums_pointer + (i * 24) as u64;
            if let Some(enumeration) = r.at(at, read_enum)? {
                meta.enums.push(enumeration);
            }
        }
        for i in 0..blocks_count.min(MAX_ELEMENTS) {
            let at = blocks_pointer + (i * 16) as u64;
            if let Some(block) = r.at(at, read_block)? {
                meta.blocks.push(block);
            }
        }

        Ok(meta)
    }

    /// `Meta.GetBlock`: blocks are numbered from 1.
    pub fn block(&self, id: i32) -> Option<&MetaBlock> {
        if id < 1 {
            return None;
        }
        self.blocks.get((id - 1) as usize)
    }

    pub fn structure(&self, name_hash: u32) -> Option<&MetaStructure> {
        self.structures.iter().find(|s| s.name_hash == name_hash)
    }

    pub fn enumeration(&self, name_hash: u32) -> Option<&MetaEnum> {
        self.enums.iter().find(|e| e.name_hash == name_hash)
    }

    /// Whether this looks like a meta at all. Zeros parse into an empty one,
    /// the same way they parse into a drawable with no models.
    pub fn has_content(&self) -> bool {
        !self.structures.is_empty() && !self.blocks.is_empty()
    }

    /// The name hash of the root structure, which is what the file *is*:
    /// `CMapTypes`, `CScenarioPointRegion`, and so on.
    pub fn root_structure(&self) -> Option<u32> {
        self.block(self.root_block_index)
            .map(|block| block.structure_name_hash)
    }
}

fn read_structure(r: &mut ResourceReader<'_>) -> Result<MetaStructure> {
    let name_hash = r.read_u32()?;
    let key = r.read_u32()?;
    let _unknown_8h = r.read_u32()?;
    let _unknown_ch = r.read_u32()?;
    let entries_pointer = r.read_u64()?;
    let size = r.read_u32()? as i32;
    let _unknown_1ch = r.read_u16()?;
    let entries_count = r.read_u16()? as usize;

    let mut entries = Vec::with_capacity(entries_count.min(1024));
    let bytes = r
        .read_bytes_at(entries_pointer, entries_count.min(MAX_ELEMENTS) * 16)?
        .unwrap_or_default();
    for e in bytes.chunks_exact(16) {
        entries.push(MetaEntry {
            name_hash: u32::from_le_bytes([e[0], e[1], e[2], e[3]]),
            data_offset: i32::from_le_bytes([e[4], e[5], e[6], e[7]]),
            data_type: MetaDataType::from_raw(e[8]),
            unknown_9h: e[9],
            reference_type_index: i16::from_le_bytes([e[10], e[11]]),
            reference_key: u32::from_le_bytes([e[12], e[13], e[14], e[15]]),
        });
    }

    Ok(MetaStructure {
        name_hash,
        key,
        size,
        entries,
    })
}

fn read_enum(r: &mut ResourceReader<'_>) -> Result<MetaEnum> {
    let name_hash = r.read_u32()?;
    let key = r.read_u32()?;
    let entries_pointer = r.read_u64()?;
    let entries_count = r.read_u32()? as usize;

    let bytes = r
        .read_bytes_at(entries_pointer, entries_count.min(MAX_ELEMENTS) * 8)?
        .unwrap_or_default();
    let entries = bytes
        .chunks_exact(8)
        .map(|e| {
            (
                u32::from_le_bytes([e[0], e[1], e[2], e[3]]),
                i32::from_le_bytes([e[4], e[5], e[6], e[7]]),
            )
        })
        .collect();

    Ok(MetaEnum {
        name_hash,
        key,
        entries,
    })
}

fn read_block(r: &mut ResourceReader<'_>) -> Result<MetaBlock> {
    let structure_name_hash = r.read_u32()?;
    let data_length = r.read_u32()? as usize;
    let data_pointer = r.read_u64()?;

    let data = if data_length > MAX_ELEMENTS * 64 {
        Vec::new()
    } else {
        r.read_bytes_at(data_pointer, data_length)?
            .unwrap_or_default()
    };

    Ok(MetaBlock {
        structure_name_hash,
        data,
    })
}

/// A packed pointer: block id in the low 12 bits, offset in the next 20.
///
/// `Array_Structure`, `CharPointer`, `DataBlockPointer` and `MetaPOINTER` all
/// carry one of these, which is why the unpacking lives here once rather than
/// in each of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaPointer {
    pub block_id: i32,
    pub offset: usize,
}

impl MetaPointer {
    pub fn unpack(pointer: u64) -> Self {
        Self {
            block_id: (pointer & 0xFFF) as i32,
            offset: ((pointer >> 12) & 0xFFFFF) as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::reader::SYSTEM_BASE;

    #[test]
    fn unpacks_a_block_pointer() {
        // Block 3, offset 0x40: the reference builds this as
        // `(offset << 12) | blockId`.
        let packed = (0x40u64 << 12) | 3;
        let pointer = MetaPointer::unpack(packed);
        assert_eq!(pointer.block_id, 3);
        assert_eq!(pointer.offset, 0x40);
    }

    /// One structure, one enum and one block, at known addresses. Any drifted
    /// field read lands on a neighbour and the assertions fail.
    fn synthetic() -> Vec<u8> {
        let mut d = vec![0u8; 0x400];
        let put_u16 = |d: &mut Vec<u8>, at: usize, v: u16| {
            d[at..at + 2].copy_from_slice(&v.to_le_bytes());
        };
        let put_u32 = |d: &mut Vec<u8>, at: usize, v: u32| {
            d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put_u64 = |d: &mut Vec<u8>, at: usize, v: u64| {
            d[at..at + 8].copy_from_slice(&v.to_le_bytes());
        };
        let addr = |o: u64| SYSTEM_BASE + o;

        put_u32(&mut d, 0x1C, 1); // root block index
        put_u64(&mut d, 0x20, addr(0x100)); // structures
        put_u64(&mut d, 0x28, addr(0x200)); // enums
        put_u64(&mut d, 0x30, addr(0x280)); // blocks
        put_u64(&mut d, 0x38, addr(0x380)); // name
        put_u16(&mut d, 0x48, 1); // structure count
        put_u16(&mut d, 0x4A, 1); // enum count
        put_u16(&mut d, 0x4C, 1); // block count

        // MetaStructureInfo at 0x100, entries at 0x140.
        put_u32(&mut d, 0x100, 0xAABB_CCDD);
        put_u32(&mut d, 0x104, 0x1234);
        put_u64(&mut d, 0x110, addr(0x140));
        put_u32(&mut d, 0x118, 16); // structure size
        put_u16(&mut d, 0x11E, 1); // entry count
        put_u32(&mut d, 0x140, 0x1111_2222); // entry name
        put_u32(&mut d, 0x144, 4); // data offset
        d[0x148] = 0x21; // Float
        put_u32(&mut d, 0x14C, 0);

        // MetaEnumInfo at 0x200, entries at 0x240.
        put_u32(&mut d, 0x200, 0x3333_4444);
        put_u64(&mut d, 0x208, addr(0x240));
        put_u32(&mut d, 0x210, 1);
        put_u32(&mut d, 0x240, 0x5555_6666);
        put_u32(&mut d, 0x244, 9);

        // MetaDataBlock at 0x280, payload at 0x300.
        put_u32(&mut d, 0x280, 0xAABB_CCDD);
        put_u32(&mut d, 0x284, 8);
        put_u64(&mut d, 0x288, addr(0x300));
        put_u32(&mut d, 0x304, 0x4048_0000); // 3.125f at offset 4

        d[0x380..0x384].copy_from_slice(b"test");
        d
    }

    #[test]
    fn reads_the_schema_and_the_blocks() {
        let data = synthetic();
        let mut r = ResourceReader::new(&data, data.len());
        let meta = Meta::read(&mut r).unwrap();

        assert_eq!(meta.name.as_deref(), Some("test"));
        assert_eq!(meta.root_block_index, 1);
        assert_eq!(meta.root_structure(), Some(0xAABB_CCDD));
        assert!(meta.has_content());

        let structure = meta.structure(0xAABB_CCDD).expect("structure");
        assert_eq!(structure.size, 16);
        assert_eq!(structure.entries.len(), 1);
        assert_eq!(structure.entries[0].data_type, MetaDataType::Float);
        assert_eq!(structure.entries[0].data_offset, 4);

        let enumeration = meta.enumeration(0x3333_4444).expect("enum");
        assert_eq!(enumeration.entries, vec![(0x5555_6666, 9)]);

        let block = meta.block(1).expect("root block");
        assert_eq!(block.len(), 8);
        assert!(meta.block(0).is_none(), "blocks are numbered from 1");
    }

    #[test]
    fn reads_zeros_as_an_empty_meta() {
        let data = vec![0u8; 0x100];
        let mut r = ResourceReader::new(&data, data.len());
        let meta = Meta::read(&mut r).unwrap();
        assert!(!meta.has_content());
    }
}
