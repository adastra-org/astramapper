//! Typed access over a parsed RSC meta.
//!
//! Port of the `MetaTypes.Get*` helpers in
//! `CodeWalker.Core/GameFiles/MetaTypes/MetaTypes.cs`, which are how `YtypFile`
//! and `YmapFile` turn the schema-carrying `Meta` into the concrete structures
//! the game's engine — and the world view — actually use. The generic
//! [`crate::meta::tree`] walk stays the general fallback; this is the typed
//! layer above it, and it is deliberately limited to the structures the two
//! file-type readers name.
//!
//! Everything here is read-only. The `MetaBuilder` side — the machinery that
//! turns typed structures back into a `Meta` and then into resource bytes — is
//! not ported, so a `.ytyp` and a `.ymap` read but do not write.

use serde::Serialize;

use super::rsc::{Meta, MetaPointer, STRING_BLOCK};
use crate::Result;

/// `MetaTypeName.UINT`, the structure name a block of uints carries.
const UINT_BLOCK: u32 = 0x15;
/// `MetaTypeName.FLOAT`, the structure name a block of floats carries.
const FLOAT_BLOCK: u32 = 0x21;
/// `MetaTypeName.HASH`, the structure name a block of hashes carries.
const HASH_BLOCK: u32 = 0x4A;
/// `MetaTypeName.VECTOR4`, the structure name a block of Vector4s carries.
const VECTOR4_BLOCK: u32 = 0x33;

/// The 16-byte array descriptor nearly every structure carries: a packed
/// pointer, two counts and a pad.
///
/// `Array_StructurePointer`, `Array_Structure`, `Array_uint`, `Array_float`,
/// `Array_byte`, `Array_Vector3` and `CharPointer` are all this one shape in
/// the reference — the field that differs is what the pointed-at data means,
/// which the caller supplies. `PointerDataId` is the 1-based block id, so the
/// zero-based block index is `id - 1`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Array {
    pub pointer: u64,
    /// The declared element count. `Count1` in the reference.
    pub count: u16,
    /// A second count, capacity-like, equal to `count` in stock files.
    pub capacity: u16,
    pub pad: u32,
}

impl Array {
    /// The packed pointer's block id, 1-based.
    pub fn pointer_data_id(&self) -> u32 {
        (self.pointer & 0xFFF) as u32
    }

    /// The packed pointer's zero-based block index.
    pub fn pointer_data_index(&self) -> i64 {
        (self.pointer & 0xFFF) as i64 - 1
    }

    /// The packed pointer's offset into the block.
    pub fn pointer_data_offset(&self) -> usize {
        ((self.pointer >> 12) & 0xFFFFF) as usize
    }
}

impl Array {
    pub fn read(r: &mut crate::resources::reader::ResourceReader<'_>) -> Result<Self> {
        Ok(Self {
            pointer: r.read_u64()?,
            count: r.read_u16()?,
            capacity: r.read_u16()?,
            pad: r.read_u32()?,
        })
    }
}

/// An empty `Array`, for a field that points at nothing.
impl Default for Array {
    fn default() -> Self {
        Self {
            pointer: 0,
            count: 0,
            capacity: 0,
            pad: 0,
        }
    }
}

/// The 8-byte data block pointer `OccludeModel` and friends use.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataBlockPointer {
    pub pointer: u64,
}

impl DataBlockPointer {
    pub fn pointer_data_id(&self) -> u32 {
        (self.pointer & 0xFFF) as u32
    }

    pub fn pointer_data_index(&self) -> i64 {
        (self.pointer & 0xFFF) as i64 - 1
    }

    pub fn pointer_data_offset(&self) -> usize {
        ((self.pointer >> 12) & 0xFFFFF) as usize
    }
}

impl Default for DataBlockPointer {
    fn default() -> Self {
        Self { pointer: 0 }
    }
}

/// Read a fixed-size structure out of the bytes of a meta block.
///
/// The structures are plain little-endian packed records whose field offsets
/// are fixed and documented in `MetaTypes.cs`; `decode` reads them from a
/// `&[u8]` view starting at `offset`. This mirrors `Marshal.PtrToStructure`.
pub trait Decode: Sized {
    /// The record's stride, in bytes.
    const SIZE: usize;

    fn decode(data: &[u8], offset: usize) -> Self;
}

/// All the concrete records this layer can decode, sized by [`Decode::SIZE`].
///
/// The bounds rule is the reference's `Marshal.PtrToStructure` behaviour, not a
/// strict one: the reference checks only that the record *starts* inside the
/// block and reads the tail regardless of where it lands. Real files rely on
/// this — an archetype record can straddle a block's last byte. A record that
/// starts past the end is absent, a record that starts inside but runs past it
/// decodes with the missing tail zero-filled, matching this crate's short-read
/// rule elsewhere.
fn decode<T: Decode>(data: &[u8], offset: usize) -> Option<T> {
    if offset >= data.len() {
        return None;
    }
    if offset + T::SIZE <= data.len() {
        return Some(T::decode(data, offset));
    }
    let mut buf = vec![0u8; T::SIZE];
    buf[..data.len() - offset].copy_from_slice(&data[offset..]);
    Some(T::decode(&buf, 0))
}

/// Decode a record out of a block's bytes, bounds-checked.
pub fn decode_at<T: Decode>(data: &[u8], offset: usize) -> Option<T> {
    decode::<T>(data, offset)
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn i16_at(data: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn i32_at(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn f32_at(data: &[u8], offset: usize) -> f32 {
    f32::from_bits(u32_at(data, offset))
}

fn vec3_at(data: &[u8], offset: usize) -> [f32; 3] {
    [f32_at(data, offset), f32_at(data, offset + 4), f32_at(data, offset + 8)]
}

fn vec4_at(data: &[u8], offset: usize) -> [f32; 4] {
    [
        f32_at(data, offset),
        f32_at(data, offset + 4),
        f32_at(data, offset + 8),
        f32_at(data, offset + 12),
    ]
}

fn array_at(data: &[u8], offset: usize) -> Array {
    Array {
        pointer: u64::from_le_bytes(
            data[offset..offset + 8].try_into().expect("array pointer is 8 bytes"),
        ),
        count: u16_at(data, offset + 8),
        capacity: u16_at(data, offset + 10),
        pad: u32_at(data, offset + 12),
    }
}

fn data_block_pointer_at(data: &[u8], offset: usize) -> DataBlockPointer {
    DataBlockPointer {
        pointer: u64::from_le_bytes(
            data[offset..offset + 8].try_into().expect("data block pointer is 8 bytes"),
        ),
    }
}

/// Read a `MetaPOINTER[]` out of the `POINTER` block an [`Array`] names.
///
/// `GetPointerArray` in the reference. Count zero, or a pointer block that is
/// not `POINTER`, yields `None`.
pub fn get_pointer_array(meta: &Meta, array: &Array) -> Option<Vec<MetaPointer>> {
    let count = array.count as usize;
    if count == 0 {
        return None;
    }
    let block = meta.block(array.pointer_data_id() as i32)?;
    if block.structure_name_hash != super::rsc::POINTER_BLOCK {
        return None;
    }
    let mut ptrs = Vec::with_capacity(count);
    for i in 0..count {
        let offset = array.pointer_data_offset() + i * 8;
        if offset + 8 > block.data.len() {
            break;
        }
        let raw = u64::from_le_bytes(
            block.data[offset..offset + 8].try_into().expect("pointer is 8 bytes"),
        );
        ptrs.push(MetaPointer::unpack(raw));
    }
    Some(ptrs)
}

/// The first block whose structure is `name`, decoded from byte zero.
///
/// `GetTypedData<T>` in the reference, which throws when no such block exists;
/// here that is `None` and the caller decides what a missing root means.
pub fn get_typed_data<T: Decode>(meta: &Meta, name: u32) -> Option<T> {
    meta.blocks
        .iter()
        .find(|block| block.structure_name_hash == name)
        .and_then(|block| decode::<T>(&block.data, 0))
}

/// Read each pointer's target, keeping only those whose block structure is
/// `name`. Null, a short block, a type mismatch or an empty result is `None`.
///
/// `GetTypedPointerArray<T>` in the reference — the "bad hack just for ymap
/// entities" that filters a shared pointer array by the target's structure
/// type, because `CEntityDef` and `CMloInstanceDef` sit in the same array.
pub fn get_typed_pointer_array<T: Decode>(meta: &Meta, name: u32, ptrs: &[MetaPointer]) -> Option<Vec<T>> {
    let mut items = Vec::new();
    for ptr in ptrs {
        let Some(block) = meta.block(ptr.block_id) else {
            continue;
        };
        if block.structure_name_hash != name {
            continue;
        }
        if let Some(item) = decode::<T>(&block.data, ptr.offset) {
            items.push(item);
        }
    }
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

/// Read `count` records of `T` out of consecutive blocks of structure `name`,
/// starting at the packed pointer's block and offset.
///
/// `ConvertDataArray<T>` in the reference — the workhorse behind every array
/// in a `.ytyp` or `.ymap`. A large array is split across blocks, each read to
/// its end before stepping to the next. A type mismatch or a block running out
/// of data mid-array yields `None`.
pub fn convert_data_array<T: Decode>(
    meta: &Meta,
    name: u32,
    pointer: u64,
    count: u32,
) -> Option<Vec<T>> {
    if count == 0 {
        return None;
    }
    let mut items = Vec::with_capacity(count as usize);

    let mut ptr_index = (pointer & 0xFFF) as usize;
    let ptr_offset = ((pointer >> 12) & 0xFFFFF) as usize;
    let mut block = meta.block(ptr_index as i32);
    if block.is_none() || block.unwrap().structure_name_hash != name {
        return None;
    }

    let item_size = T::SIZE;
    let mut item_offset = ptr_offset / item_size;
    let mut left = count as usize;

    while left > 0 {
        let Some(b) = block else { break };
        if b.structure_name_hash != name {
            break;
        }
        let block_count = b.data.len() / item_size;
        let mut item_count = block_count.saturating_sub(item_offset);
        if item_count > left {
            item_count = left;
        }
        for i in 0..item_count {
            let offset = (item_offset + i) * item_size;
            let Some(item) = decode::<T>(&b.data, offset) else {
                return None;
            };
            items.push(item);
        }
        item_offset = 0;
        left -= item_count;
        if left == 0 {
            return Some(items);
        }
        ptr_index += 1;
        block = meta.block(ptr_index as i32);
    }

    None
}

/// Read a structure-pointer array, filtering each target by its structure.
/// `ConvertDataArray<T>` for the `Array_StructurePointer` shape.
pub fn convert_data_array_ptr<T: Decode>(
    meta: &Meta,
    name: u32,
    array: &Array,
) -> Option<Vec<T>> {
    if array.count == 0 {
        return None;
    }
    let ptrs = get_pointer_array(meta, array)?;
    if ptrs.len() < array.count as usize {
        return None;
    }
    let mut items = Vec::with_capacity(array.count as usize);
    for ptr in ptrs.iter().take(array.count as usize) {
        let Some(block) = meta.block(ptr.block_id) else {
            continue;
        };
        if block.structure_name_hash != name {
            return None;
        }
        if let Some(item) = decode::<T>(&block.data, ptr.offset) {
            items.push(item);
        }
    }
    Some(items)
}

/// `MetaHash[]`, which is a `uint[]` under a `HASH` block type.
pub fn get_hash_array(meta: &Meta, array: &Array) -> Option<Vec<u32>> {
    convert_data_array::<Uint>(meta, HASH_BLOCK, array.pointer, array.count as u32)
        .map(|items| items.into_iter().map(|u: Uint| u.0).collect())
}

/// `uint[]`.
pub fn get_uint_array(meta: &Meta, array: &Array) -> Option<Vec<u32>> {
    convert_data_array::<Uint>(meta, UINT_BLOCK, array.pointer, array.count as u32)
        .map(|items| items.into_iter().map(|u: Uint| u.0).collect())
}

/// `float[]`.
pub fn get_float_array(meta: &Meta, array: &Array) -> Option<Vec<f32>> {
    convert_data_array::<Float>(meta, FLOAT_BLOCK, array.pointer, array.count as u32)
        .map(|items| items.into_iter().map(|f: Float| f.0).collect())
}

/// `Vector4[]`, the shape portal corners are stored in — a `FloatXYZ` field
/// that the reference reads with 16-byte stride. `GetPaddedVector3Array`.
pub fn get_vec4_array(meta: &Meta, array: &Array) -> Option<Vec<[f32; 4]>> {
    convert_data_array::<Vec4>(meta, VECTOR4_BLOCK, array.pointer, array.count as u32)
        .map(|items| items.into_iter().map(|v: Vec4| v.0).collect())
}

/// `byte[]`, read out of the block the array names without a type check.
pub fn get_byte_array(meta: &Meta, array: &Array) -> Option<Vec<u8>> {
    let ptr_index = array.pointer_data_index();
    if ptr_index < 0 {
        return None;
    }
    let block = meta.block((ptr_index + 1) as i32)?;
    let count = array.count as usize;
    let offset = array.pointer_data_offset();
    if offset + count > block.data.len() {
        return None;
    }
    Some(block.data[offset..offset + count].to_vec())
}

/// A `CharPointer`: a string of `count` bytes in a `STRING` block.
pub fn get_string(meta: &Meta, array: &Array) -> Option<String> {
    let block_index = array.pointer_data_index();
    if block_index < 0 {
        return None;
    }
    let block = meta.block((block_index + 1) as i32)?;
    if block.structure_name_hash != STRING_BLOCK {
        return None;
    }
    let offset = array.pointer_data_offset();
    let length = array.count as usize;
    if offset + length > block.data.len() {
        return None;
    }
    Some(
        block.data[offset..offset + length]
            .iter()
            .map(|&b| if b < 0x80 { b as char } else { '?' })
            .collect(),
    )
}

/// Every string in the file's consecutive `STRING` blocks.
///
/// `GetStrings` in the reference, which walks the string blocks in order and
/// splits each on NUL bytes. Strings decode as ASCII, byte for byte — the same
/// lossy mapping `get_string` uses.
pub fn get_strings(meta: &Meta) -> Option<Vec<String>> {
    let start = meta
        .blocks
        .iter()
        .position(|block| block.structure_name_hash == STRING_BLOCK)?;

    let mut strings = Vec::new();
    for block in &meta.blocks[start..] {
        if block.structure_name_hash != STRING_BLOCK {
            break;
        }
        let data = &block.data;
        let mut start_index;
        let mut end_index = 0usize;
        for b in 0..data.len() {
            if data[b] == 0 {
                start_index = end_index;
                end_index = b;
                if end_index > start_index {
                    let s = data[start_index..end_index]
                        .iter()
                        .map(|&c| if c < 0x80 { c as char } else { '?' })
                        .collect::<String>();
                    strings.push(s);
                    end_index += 1;
                }
            }
        }
        if end_index != data.len().saturating_sub(1) {
            start_index = end_index;
            end_index = data.len().saturating_sub(1);
            if end_index > start_index {
                let s = data[start_index..end_index]
                    .iter()
                    .map(|&c| if c < 0x80 { c as char } else { '?' })
                    .collect::<String>();
                strings.push(s);
            }
        }
    }

    if strings.is_empty() {
        None
    } else {
        Some(strings)
    }
}

// The primitive records used by the typed arrays.

struct Uint(u32);
impl Decode for Uint {
    const SIZE: usize = 4;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self(u32_at(data, offset))
    }
}

struct Float(f32);
impl Decode for Float {
    const SIZE: usize = 4;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self(f32_at(data, offset))
    }
}

struct Vec4([f32; 4]);
impl Decode for Vec4 {
    const SIZE: usize = 16;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self(vec4_at(data, offset))
    }
}

// The typed structures the file-type readers decode. Field offsets come
// verbatim from the `//offset` comments in `MetaTypes.cs`.

/// `rage__eLodType`, an int enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LodLevel {
    Hd,
    Lod,
    Slod1,
    Slod2,
    Slod3,
    OrphanHd,
    Slod4,
    /// A value the enum does not name.
    Unknown(i32),
}

impl From<i32> for LodLevel {
    fn from(v: i32) -> Self {
        match v {
            0 => Self::Hd,
            1 => Self::Lod,
            2 => Self::Slod1,
            3 => Self::Slod2,
            4 => Self::Slod3,
            5 => Self::OrphanHd,
            6 => Self::Slod4,
            other => Self::Unknown(other),
        }
    }
}

impl LodLevel {
    /// The raw enum value, for comparing levels.
    pub fn raw(self) -> i32 {
        match self {
            Self::Hd => 0,
            Self::Lod => 1,
            Self::Slod1 => 2,
            Self::Slod2 => 3,
            Self::Slod3 => 4,
            Self::OrphanHd => 5,
            Self::Slod4 => 6,
            Self::Unknown(v) => v,
        }
    }
}

/// `rage__ePriorityLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PriorityLevel {
    Required,
    OptionalHigh,
    Medium,
    Low,
    Unknown(i32),
}

impl From<i32> for PriorityLevel {
    fn from(v: i32) -> Self {
        match v {
            0 => Self::Required,
            1 => Self::OptionalHigh,
            2 => Self::Medium,
            3 => Self::Low,
            other => Self::Unknown(other),
        }
    }
}

/// `rage__fwArchetypeDef__eAssetType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AssetType {
    Uninitialised,
    AssetFragment,
    AssetDrawable,
    AssetDrawableDictionary,
    AssetTxd,
    AssetBounds,
    AssetNavMesh,
    AssetTimeCycle,
    AssetClipDictionary,
    AssetCollision,
    AssetCluster,
    AssetLodLight,
    AssetCompositeEntityType,
    AssetPropBatch,
    AssetVertexShader,
    AssetPixelShader,
    AssetTexture,
    AssetDrawableAtlas,
    Unknown(i32),
}

impl From<i32> for AssetType {
    fn from(v: i32) -> Self {
        match v {
            0 => Self::Uninitialised,
            1 => Self::AssetFragment,
            2 => Self::AssetDrawable,
            3 => Self::AssetDrawableDictionary,
            4 => Self::AssetTxd,
            5 => Self::AssetBounds,
            6 => Self::AssetNavMesh,
            7 => Self::AssetTimeCycle,
            8 => Self::AssetClipDictionary,
            9 => Self::AssetCollision,
            10 => Self::AssetCluster,
            11 => Self::AssetLodLight,
            12 => Self::AssetCompositeEntityType,
            13 => Self::AssetPropBatch,
            14 => Self::AssetVertexShader,
            15 => Self::AssetPixelShader,
            16 => Self::AssetTexture,
            17 => Self::AssetDrawableAtlas,
            other => Self::Unknown(other),
        }
    }
}

impl AssetType {
    /// The raw enum value.
    pub fn raw(self) -> i32 {
        match self {
            Self::Uninitialised => 0,
            Self::AssetFragment => 1,
            Self::AssetDrawable => 2,
            Self::AssetDrawableDictionary => 3,
            Self::AssetTxd => 4,
            Self::AssetBounds => 5,
            Self::AssetNavMesh => 6,
            Self::AssetTimeCycle => 7,
            Self::AssetClipDictionary => 8,
            Self::AssetCollision => 9,
            Self::AssetCluster => 10,
            Self::AssetLodLight => 11,
            Self::AssetCompositeEntityType => 12,
            Self::AssetPropBatch => 13,
            Self::AssetVertexShader => 14,
            Self::AssetPixelShader => 15,
            Self::AssetTexture => 16,
            Self::AssetDrawableAtlas => 17,
            Self::Unknown(v) => v,
        }
    }
}

/// `CBaseArchetypeDef`, 144 bytes. The shared first half of every archetype.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseArchetypeDef {
    pub lod_dist: f32,
    pub flags: u32,
    pub special_attribute: u32,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub bs_centre: [f32; 3],
    pub bs_radius: f32,
    pub hd_texture_dist: f32,
    pub name: u32,
    pub texture_dictionary: u32,
    pub clip_dictionary: u32,
    pub drawable_dictionary: u32,
    pub physics_dictionary: u32,
    pub asset_type: AssetType,
    pub asset_name: u32,
    pub extensions: Array,
}

impl Decode for BaseArchetypeDef {
    const SIZE: usize = 144;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            lod_dist: f32_at(data, offset + 8),
            flags: u32_at(data, offset + 12),
            special_attribute: u32_at(data, offset + 16),
            bb_min: vec3_at(data, offset + 32),
            bb_max: vec3_at(data, offset + 48),
            bs_centre: vec3_at(data, offset + 64),
            bs_radius: f32_at(data, offset + 80),
            hd_texture_dist: f32_at(data, offset + 84),
            name: u32_at(data, offset + 88),
            texture_dictionary: u32_at(data, offset + 92),
            clip_dictionary: u32_at(data, offset + 96),
            drawable_dictionary: u32_at(data, offset + 100),
            physics_dictionary: u32_at(data, offset + 104),
            asset_type: i32_at(data, offset + 108).into(),
            asset_name: u32_at(data, offset + 112),
            extensions: array_at(data, offset + 120),
        }
    }
}

/// `CTimeArchetypeDef`, 160 bytes: a `CBaseArchetypeDef` plus a 16-byte tail.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeArchetypeDef {
    pub base: BaseArchetypeDef,
    pub time_flags: u32,
}

impl Decode for TimeArchetypeDef {
    const SIZE: usize = 160;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            base: BaseArchetypeDef::decode(data, offset),
            time_flags: u32_at(data, offset + 144),
        }
    }
}

/// `CMloArchetypeDefData`, the 96-byte tail of an MLO archetype.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloArchetypeDefData {
    pub mlo_flags: u32,
    pub entities: Array,
    pub rooms: Array,
    pub portals: Array,
    pub entity_sets: Array,
    pub time_cycle_modifiers: Array,
}

impl MloArchetypeDefData {
    fn read(data: &[u8], offset: usize) -> Self {
        Self {
            mlo_flags: u32_at(data, offset + 144),
            entities: array_at(data, offset + 152),
            rooms: array_at(data, offset + 168),
            portals: array_at(data, offset + 184),
            entity_sets: array_at(data, offset + 200),
            time_cycle_modifiers: array_at(data, offset + 216),
        }
    }
}

/// `CMloArchetypeDef`, 240 bytes: base plus the MLO tail.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloArchetypeDef {
    pub base: BaseArchetypeDef,
    pub mlo: MloArchetypeDefData,
}

impl Decode for MloArchetypeDef {
    const SIZE: usize = 240;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            base: BaseArchetypeDef::decode(data, offset),
            mlo: MloArchetypeDefData::read(data, offset),
        }
    }
}

/// `CEntityDef`, 128 bytes. The record behind every placed entity.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityDef {
    pub archetype_name: u32,
    pub flags: u32,
    pub guid: u32,
    pub position: [f32; 3],
    /// The stored quaternion, inverted relative to the world orientation —
    /// the reader's job to un-invert, per the reference.
    pub rotation: [f32; 4],
    pub scale_xy: f32,
    pub scale_z: f32,
    pub parent_index: i32,
    pub lod_dist: f32,
    pub child_lod_dist: f32,
    pub lod_level: LodLevel,
    pub num_children: u32,
    pub priority_level: PriorityLevel,
    pub extensions: Array,
    pub ambient_occlusion_multiplier: i32,
    pub artificial_ambient_occlusion: i32,
    pub tint_value: u32,
}

impl Decode for EntityDef {
    const SIZE: usize = 128;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            archetype_name: u32_at(data, offset + 8),
            flags: u32_at(data, offset + 12),
            guid: u32_at(data, offset + 16),
            position: vec3_at(data, offset + 32),
            rotation: vec4_at(data, offset + 48),
            scale_xy: f32_at(data, offset + 64),
            scale_z: f32_at(data, offset + 68),
            parent_index: i32_at(data, offset + 72),
            lod_dist: f32_at(data, offset + 76),
            child_lod_dist: f32_at(data, offset + 80),
            lod_level: i32_at(data, offset + 84).into(),
            num_children: u32_at(data, offset + 88),
            priority_level: i32_at(data, offset + 92).into(),
            extensions: array_at(data, offset + 96),
            ambient_occlusion_multiplier: i32_at(data, offset + 112),
            artificial_ambient_occlusion: i32_at(data, offset + 116),
            tint_value: u32_at(data, offset + 120),
        }
    }
}

/// `CMloInstanceDef`, 160 bytes: a `CEntityDef` plus a 32-byte tail.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloInstanceDef {
    pub entity: EntityDef,
    pub group_id: u32,
    pub floor_id: u32,
    pub default_entity_sets: Array,
    pub num_exit_portals: u32,
    pub mlo_inst_flags: u32,
}

impl Decode for MloInstanceDef {
    const SIZE: usize = 160;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            entity: EntityDef::decode(data, offset),
            group_id: u32_at(data, offset + 128),
            floor_id: u32_at(data, offset + 132),
            default_entity_sets: array_at(data, offset + 136),
            num_exit_portals: u32_at(data, offset + 152),
            mlo_inst_flags: u32_at(data, offset + 156),
        }
    }
}

/// `CMloRoomDef`, 112 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloRoomDef {
    pub name: Array,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub blend: f32,
    pub timecycle_name: u32,
    pub secondary_timecycle_name: u32,
    pub flags: u32,
    pub portal_count: u32,
    pub floor_id: i32,
    pub exterior_visibilty_depth: i32,
    pub attached_objects: Array,
}

impl Decode for MloRoomDef {
    const SIZE: usize = 112;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            name: array_at(data, offset + 8),
            bb_min: vec3_at(data, offset + 32),
            bb_max: vec3_at(data, offset + 48),
            blend: f32_at(data, offset + 64),
            timecycle_name: u32_at(data, offset + 68),
            secondary_timecycle_name: u32_at(data, offset + 72),
            flags: u32_at(data, offset + 76),
            portal_count: u32_at(data, offset + 80),
            floor_id: i32_at(data, offset + 84),
            exterior_visibilty_depth: i32_at(data, offset + 88),
            attached_objects: array_at(data, offset + 96),
        }
    }
}

/// `CMloPortalDef`, 64 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloPortalDef {
    pub room_from: u32,
    pub room_to: u32,
    pub flags: u32,
    pub mirror_priority: u32,
    pub opacity: u32,
    pub audio_occlusion: u32,
    pub corners: Array,
    pub attached_objects: Array,
}

impl Decode for MloPortalDef {
    const SIZE: usize = 64;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            room_from: u32_at(data, offset + 8),
            room_to: u32_at(data, offset + 12),
            flags: u32_at(data, offset + 16),
            mirror_priority: u32_at(data, offset + 20),
            opacity: u32_at(data, offset + 24),
            audio_occlusion: u32_at(data, offset + 28),
            corners: array_at(data, offset + 32),
            attached_objects: array_at(data, offset + 48),
        }
    }
}

/// `CMloEntitySet`, 48 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloEntitySet {
    pub name: u32,
    pub locations: Array,
    pub entities: Array,
}

impl Decode for MloEntitySet {
    const SIZE: usize = 48;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            name: u32_at(data, offset + 8),
            locations: array_at(data, offset + 16),
            entities: array_at(data, offset + 32),
        }
    }
}

/// `CMloTimeCycleModifier`, 48 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloTimeCycleModifier {
    pub name: u32,
    pub sphere: [f32; 4],
    pub percentage: f32,
    pub range: f32,
    pub start_hour: u32,
    pub end_hour: u32,
}

impl Decode for MloTimeCycleModifier {
    const SIZE: usize = 48;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            name: u32_at(data, offset + 8),
            sphere: vec4_at(data, offset + 16),
            percentage: f32_at(data, offset + 32),
            range: f32_at(data, offset + 36),
            start_hour: u32_at(data, offset + 40),
            end_hour: u32_at(data, offset + 44),
        }
    }
}

/// `CMapTypes`, 80 bytes: the root of a `.ytyp`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapTypes {
    pub extensions: Array,
    pub archetypes: Array,
    pub name: u32,
    pub dependencies: Array,
    pub composite_entity_types: Array,
}

impl Decode for MapTypes {
    const SIZE: usize = 80;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            extensions: array_at(data, offset + 8),
            archetypes: array_at(data, offset + 24),
            name: u32_at(data, offset + 40),
            dependencies: array_at(data, offset + 48),
            composite_entity_types: array_at(data, offset + 64),
        }
    }
}

/// `CCompositeEntityType`, 304 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompositeEntityType {
    pub name: String,
    pub lod_dist: f32,
    pub flags: u32,
    pub special_attribute: u32,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub bs_centre: [f32; 3],
    pub bs_radius: f32,
    pub start_model: String,
    pub end_model: String,
    pub start_imap_file: u32,
    pub end_imap_file: u32,
    pub pt_fx_asset_name: u32,
    pub animations: Array,
}

impl Decode for CompositeEntityType {
    const SIZE: usize = 304;
    fn decode(data: &[u8], offset: usize) -> Self {
        let chars64 = |data: &[u8], offset: usize| -> String {
            let bytes = &data[offset..offset + 64];
            let len = bytes.iter().position(|&b| b == 0).unwrap_or(64);
            bytes[..len]
                .iter()
                .map(|&b| if b < 0x80 { b as char } else { '?' })
                .collect()
        };
        Self {
            name: chars64(data, offset),
            lod_dist: f32_at(data, offset + 64),
            flags: u32_at(data, offset + 68),
            special_attribute: u32_at(data, offset + 72),
            bb_min: vec3_at(data, offset + 80),
            bb_max: vec3_at(data, offset + 96),
            bs_centre: vec3_at(data, offset + 112),
            bs_radius: f32_at(data, offset + 128),
            start_model: chars64(data, offset + 136),
            end_model: chars64(data, offset + 200),
            start_imap_file: u32_at(data, offset + 264),
            end_imap_file: u32_at(data, offset + 268),
            pt_fx_asset_name: u32_at(data, offset + 272),
            animations: array_at(data, offset + 280),
        }
    }
}

/// `CMapData`, 512 bytes: the root of a `.ymap`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapData {
    pub name: u32,
    pub parent: u32,
    pub flags: u32,
    pub content_flags: u32,
    pub streaming_extents_min: [f32; 3],
    pub streaming_extents_max: [f32; 3],
    pub entities_extents_min: [f32; 3],
    pub entities_extents_max: [f32; 3],
    pub entities: Array,
    pub container_lods: Array,
    pub box_occluders: Array,
    pub occlude_models: Array,
    pub physics_dictionaries: Array,
    pub instanced_data: InstancedMapData,
    pub time_cycle_modifiers: Array,
    pub car_generators: Array,
    pub lod_lights_soa: LodLight,
    pub distant_lod_lights_soa: DistantLodLight,
    pub block: BlockDesc,
}

impl Decode for MapData {
    const SIZE: usize = 512;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            name: u32_at(data, offset + 8),
            parent: u32_at(data, offset + 12),
            flags: u32_at(data, offset + 16),
            content_flags: u32_at(data, offset + 20),
            streaming_extents_min: vec3_at(data, offset + 32),
            streaming_extents_max: vec3_at(data, offset + 48),
            entities_extents_min: vec3_at(data, offset + 64),
            entities_extents_max: vec3_at(data, offset + 80),
            entities: array_at(data, offset + 96),
            container_lods: array_at(data, offset + 112),
            box_occluders: array_at(data, offset + 128),
            occlude_models: array_at(data, offset + 144),
            physics_dictionaries: array_at(data, offset + 160),
            instanced_data: InstancedMapData::decode(data, offset + 176),
            time_cycle_modifiers: array_at(data, offset + 224),
            car_generators: array_at(data, offset + 240),
            lod_lights_soa: LodLight::decode(data, offset + 256),
            distant_lod_lights_soa: DistantLodLight::decode(data, offset + 392),
            block: BlockDesc::decode(data, offset + 440),
        }
    }
}

/// `rage__fwInstancedMapData`, 48 bytes: the prop and grass instance lists.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstancedMapData {
    pub imap_link: u32,
    pub prop_instance_list: Array,
    pub grass_instance_list: Array,
}

impl Decode for InstancedMapData {
    const SIZE: usize = 48;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            imap_link: u32_at(data, offset + 8),
            prop_instance_list: array_at(data, offset + 16),
            grass_instance_list: array_at(data, offset + 32),
        }
    }
}

/// `rage__spdAABB`, 32 bytes: a `Vector4` min and max.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpdAabb {
    pub min: [f32; 4],
    pub max: [f32; 4],
}

impl Decode for SpdAabb {
    const SIZE: usize = 32;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            min: vec4_at(data, offset),
            max: vec4_at(data, offset + 16),
        }
    }
}

/// `rage__fwGrassInstanceListDef`, 96 bytes.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrassInstanceListDef {
    pub batch_aabb: SpdAabb,
    pub scale_range: [f32; 3],
    pub archetype_name: u32,
    pub lod_dist: u32,
    pub lod_fade_start_dist: f32,
    pub lod_inst_fade_range: f32,
    pub orient_to_terrain: f32,
    pub instance_list: Array,
}

impl Decode for GrassInstanceListDef {
    const SIZE: usize = 96;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            batch_aabb: SpdAabb::decode(data, offset),
            scale_range: vec3_at(data, offset + 32),
            archetype_name: u32_at(data, offset + 48),
            lod_dist: u32_at(data, offset + 52),
            lod_fade_start_dist: f32_at(data, offset + 56),
            lod_inst_fade_range: f32_at(data, offset + 60),
            orient_to_terrain: f32_at(data, offset + 64),
            instance_list: array_at(data, offset + 72),
        }
    }
}

/// `rage__fwGrassInstanceListDef__InstanceData`, 16 bytes.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrassInstanceData {
    pub position: [u16; 3],
    pub normal_x: u8,
    pub normal_y: u8,
    pub color: [u8; 3],
    pub scale: u8,
    pub ao: u8,
    pub pad: [u8; 3],
}

impl Decode for GrassInstanceData {
    const SIZE: usize = 16;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            position: [
                u16_at(data, offset),
                u16_at(data, offset + 2),
                u16_at(data, offset + 4),
            ],
            normal_x: data[offset + 6],
            normal_y: data[offset + 7],
            color: [data[offset + 8], data[offset + 9], data[offset + 10]],
            scale: data[offset + 11],
            ao: data[offset + 12],
            pad: [data[offset + 13], data[offset + 14], data[offset + 15]],
        }
    }
}

/// `CTimeCycleModifier`, 64 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeCycleModifier {
    pub name: u32,
    pub min_extents: [f32; 3],
    pub max_extents: [f32; 3],
    pub percentage: f32,
    pub range: f32,
    pub start_hour: u32,
    pub end_hour: u32,
}

impl Decode for TimeCycleModifier {
    const SIZE: usize = 64;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            name: u32_at(data, offset + 8),
            min_extents: vec3_at(data, offset + 16),
            max_extents: vec3_at(data, offset + 32),
            percentage: f32_at(data, offset + 48),
            range: f32_at(data, offset + 52),
            start_hour: u32_at(data, offset + 56),
            end_hour: u32_at(data, offset + 60),
        }
    }
}

/// `CCarGen`, 80 bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CarGen {
    pub position: [f32; 3],
    pub orient_x: f32,
    pub orient_y: f32,
    pub perpendicular_length: f32,
    pub car_model: u32,
    pub flags: u32,
    pub body_color_remap1: i32,
    pub body_color_remap2: i32,
    pub body_color_remap3: i32,
    pub body_color_remap4: i32,
    pub pop_group: u32,
    pub livery: i8,
}

impl Decode for CarGen {
    const SIZE: usize = 80;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            position: vec3_at(data, offset + 16),
            orient_x: f32_at(data, offset + 32),
            orient_y: f32_at(data, offset + 36),
            perpendicular_length: f32_at(data, offset + 40),
            car_model: u32_at(data, offset + 44),
            flags: u32_at(data, offset + 48),
            body_color_remap1: i32_at(data, offset + 52),
            body_color_remap2: i32_at(data, offset + 56),
            body_color_remap3: i32_at(data, offset + 60),
            body_color_remap4: i32_at(data, offset + 64),
            pop_group: u32_at(data, offset + 68),
            livery: data[offset + 72] as i8,
        }
    }
}

/// `BoxOccluder`, 16 bytes: eight signed shorts.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxOccluder {
    pub i_center_x: i16,
    pub i_center_y: i16,
    pub i_center_z: i16,
    pub i_cos_z: i16,
    pub i_length: i16,
    pub i_width: i16,
    pub i_height: i16,
    pub i_sin_z: i16,
}

impl Decode for BoxOccluder {
    const SIZE: usize = 16;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            i_center_x: i16_at(data, offset),
            i_center_y: i16_at(data, offset + 2),
            i_center_z: i16_at(data, offset + 4),
            i_cos_z: i16_at(data, offset + 6),
            i_length: i16_at(data, offset + 8),
            i_width: i16_at(data, offset + 10),
            i_height: i16_at(data, offset + 12),
            i_sin_z: i16_at(data, offset + 14),
        }
    }
}

/// `OccludeModel`, 64 bytes.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OccludeModel {
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub data_size: u32,
    pub verts: DataBlockPointer,
    pub num_verts_in_bytes: u16,
    pub num_tris: u16,
    pub flags: u32,
}

impl Decode for OccludeModel {
    const SIZE: usize = 64;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            bmin: vec3_at(data, offset),
            bmax: vec3_at(data, offset + 16),
            data_size: u32_at(data, offset + 32),
            verts: data_block_pointer_at(data, offset + 40),
            num_verts_in_bytes: u16_at(data, offset + 48),
            num_tris: u16_at(data, offset + 50),
            flags: u32_at(data, offset + 52),
        }
    }
}

/// `CLODLight`, 136 bytes: the LOD lights as a structure-of-arrays.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LodLight {
    pub direction: Array,
    pub falloff: Array,
    pub falloff_exponent: Array,
    pub time_and_state_flags: Array,
    pub hash: Array,
    pub cone_inner_angle: Array,
    pub cone_outer_angle_or_cap_ext: Array,
    pub corona_intensity: Array,
}

impl Decode for LodLight {
    const SIZE: usize = 136;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            direction: array_at(data, offset + 8),
            falloff: array_at(data, offset + 24),
            falloff_exponent: array_at(data, offset + 40),
            time_and_state_flags: array_at(data, offset + 56),
            hash: array_at(data, offset + 72),
            cone_inner_angle: array_at(data, offset + 88),
            cone_outer_angle_or_cap_ext: array_at(data, offset + 104),
            corona_intensity: array_at(data, offset + 120),
        }
    }
}

/// `CDistantLODLight`, 48 bytes.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistantLodLight {
    pub position: Array,
    pub rgbi: Array,
    pub num_street_lights: u16,
    pub category: u16,
}

impl Decode for DistantLodLight {
    const SIZE: usize = 48;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            position: array_at(data, offset + 8),
            rgbi: array_at(data, offset + 24),
            num_street_lights: u16_at(data, offset + 40),
            category: u16_at(data, offset + 42),
        }
    }
}

/// `CBlockDesc`, 72 bytes: the ymap's metadata header.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockDesc {
    pub version: u32,
    pub flags: u32,
    pub name: Array,
    pub exported_by: Array,
    pub owner: Array,
    pub time: Array,
}

impl Decode for BlockDesc {
    const SIZE: usize = 72;
    fn decode(data: &[u8], offset: usize) -> Self {
        Self {
            version: u32_at(data, offset),
            flags: u32_at(data, offset + 4),
            name: array_at(data, offset + 8),
            exported_by: array_at(data, offset + 24),
            owner: array_at(data, offset + 40),
            time: array_at(data, offset + 56),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packed-pointer encoding: block id in the low 12 bits, offset in the
    /// next 20. A field that reads the halves backwards lands on a neighbour.
    #[test]
    fn unpacks_array_pointers() {
        let array = Array {
            pointer: (0x40u64 << 12) | 3,
            count: 2,
            capacity: 2,
            pad: 0,
        };
        assert_eq!(array.pointer_data_id(), 3);
        assert_eq!(array.pointer_data_index(), 2);
        assert_eq!(array.pointer_data_offset(), 0x40);
    }

    #[test]
    fn decodes_base_archetype_at_documented_offsets() {
        let mut data = vec![0u8; 144];
        data[8..12].copy_from_slice(&60f32.to_le_bytes());
        data[88..92].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        data[108..112].copy_from_slice(&2i32.to_le_bytes());
        let arch = BaseArchetypeDef::decode(&data, 0);
        assert_eq!(arch.lod_dist, 60.0);
        assert_eq!(arch.name, 0xDEADBEEF);
        assert_eq!(arch.asset_type, AssetType::AssetDrawable);
    }
}
