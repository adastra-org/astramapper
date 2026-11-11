//! Fragments: the physics-wrapped drawables that `.yft` files hold.
//!
//! Port of `FragType`, `FragDrawable`, `FragBoneTransforms`, `FragGlassWindow`,
//! `FragVehicleGlassWindows`, `FragPhysicsLODGroup`, `FragPhysicsLOD`,
//! `FragPhysArticulatedBodyType`, `FragPhysJointType` and the rest of
//! `CodeWalker.Core/GameFiles/Resources/Frag.cs`.
//!
//! A `.yft` is the shell around the game's most common model shape: a main
//! `FragDrawable`, a dictionary array of alternative drawables, and a physics
//! tree whose children each carry up to two more. Every drawable here is a
//! [`Drawable`] with a 168-byte fragment tail, so the geometry path reuses the
//! drawable reader and the fragment port adds what the physics needs.
//!
//! The viewer only needs the drawables; the physics blocks are read through so
//! the pointer chain stays aligned, and their fields are kept for the gate.

use serde::Serialize;

use crate::Result;

use super::bounds::{Bounds, Matrix4F};
use super::drawable::{Drawable, ResourceFileBase};
use super::reader::ResourceReader;
use super::vertex::VertexDeclaration;

/// Guard on any count read out of a file before it is used to size an
/// allocation.
const MAX_ELEMENTS: usize = 16_000_000;

/// The root of a `.yft`: a fragment type and the blocks it names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragType {
    pub base: ResourceFileBase,
    pub bounding_sphere_center: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub name: Option<String>,
    /// The main drawable.
    pub drawable: Option<FragDrawable>,
    /// Alternative drawables, named by `drawable_array_names`.
    pub drawable_array: Vec<FragDrawable>,
    pub drawable_array_names: Vec<Option<String>>,
    pub bone_transforms: Option<FragBoneTransforms>,
    pub glass_windows: Vec<FragGlassWindow>,
    pub physics_lod_group: Option<FragPhysicsLODGroup>,
    pub drawable_cloth: Option<FragDrawable>,
    pub vehicle_glass_windows: Option<FragVehicleGlassWindows>,
}

impl FragType {
    /// Read a `FragType` at the cursor, following every pointer it names.
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let base = ResourceFileBase::read(r)?;
        let _unknown_10h = r.read_u64()?;
        let _unknown_18h = r.read_u64()?;
        let bounding_sphere_center = r.read_vec3()?;
        let bounding_sphere_radius = r.read_f32()?;
        let drawable_pointer = r.read_u64()?;
        let drawable_array_pointer = r.read_u64()?;
        let drawable_array_names_pointer = r.read_u64()?;
        let drawable_array_count = r.read_u32()?;
        let _drawable_array_flag = r.read_u32()?;
        let _unknown_50h = r.read_u64()?;
        let name_pointer = r.read_u64()?;
        let _cloths = read_pointer_list_header(r)?;
        let _unknown_70h = r.read_u64()?;
        let _unknown_78h = r.read_u64()?;
        let _unknown_80h = r.read_u64()?;
        let _unknown_88h = r.read_u64()?;
        let _unknown_90h = r.read_u64()?;
        let _unknown_98h = r.read_u64()?;
        let _unknown_a0h = r.read_u64()?;
        let bone_transforms_pointer = r.read_u64()?;
        let _unknown_b0h = r.read_u32()?;
        let _unknown_b4h = r.read_u32()?;
        let _unknown_b8h = r.read_u32()?;
        let _unknown_bch = r.read_u32()?;
        let _unknown_c0h = r.read_u32()?;
        let _unknown_c4h = r.read_u32()?;
        let _unknown_c8h = r.read_u32()?;
        let _unknown_cch = r.read_f32()?;
        let _gravity_factor = r.read_f32()?;
        let _buoyancy_factor = r.read_f32()?;
        let _unknown_d8h = r.read_u8()?;
        let glass_windows_count = r.read_u8()?;
        let _unknown_dah = r.read_u16()?;
        let _unknown_dch = r.read_u32()?;
        let glass_windows_pointer = r.read_u64()?;
        let _unknown_e8h = r.read_u64()?;
        let physics_lod_group_pointer = r.read_u64()?;
        let drawable_cloth_pointer = r.read_u64()?;
        let _unknown_100h = r.read_u64()?;
        let _unknown_108h = r.read_u64()?;
        let _light_attributes = read_simple_list_header(r)?;
        let vehicle_glass_windows_pointer = r.read_u64()?;
        let _unknown_128h = r.read_u64()?;

        let name = r.read_string_at(name_pointer)?;
        let drawable = r.at(drawable_pointer, FragDrawable::read)?;
        let drawable_array = read_drawable_array(r, drawable_array_pointer, drawable_array_count)?;
        let drawable_array_names =
            read_drawable_array_names(r, drawable_array_names_pointer, drawable_array_count)?;
        let bone_transforms = r.at(bone_transforms_pointer, FragBoneTransforms::read)?;
        let glass_windows = read_glass_windows(r, glass_windows_pointer, glass_windows_count)?;
        let physics_lod_group = r.at(physics_lod_group_pointer, FragPhysicsLODGroup::read)?;
        let drawable_cloth = r.at(drawable_cloth_pointer, FragDrawable::read)?;
        let vehicle_glass_windows =
            r.at(vehicle_glass_windows_pointer, FragVehicleGlassWindows::read)?;

        Ok(Self {
            base,
            bounding_sphere_center,
            bounding_sphere_radius,
            name,
            drawable,
            drawable_array,
            drawable_array_names,
            bone_transforms,
            glass_windows,
            physics_lod_group,
            drawable_cloth,
            vehicle_glass_windows,
        })
    }

    /// Every renderable drawable reachable from this fragment, in the order the
    /// reference walks them: the main drawable, the cloth, the array entries,
    /// then the physics children of each LOD.
    pub fn all_drawables(&self) -> Vec<&FragDrawable> {
        let mut out = Vec::new();
        if let Some(d) = &self.drawable {
            out.push(d);
        }
        if let Some(d) = &self.drawable_cloth {
            out.push(d);
        }
        out.extend(self.drawable_array.iter());
        if let Some(group) = &self.physics_lod_group {
            for lod in group.all_lods() {
                out.extend(
                    lod.children
                        .iter()
                        .flat_map(|c| c.drawable1.iter().chain(c.drawable2.iter())),
                );
            }
        }
        out
    }
}

/// A drawable inside a fragment: the 168-byte drawable base plus a 168-byte
/// fragment tail (a matrix, a bounds pointer and optional matrices).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragDrawable {
    /// The drawable base and its geometry.
    pub drawable: Drawable,
    /// The fragment's own transform, columns `[x, y, z, flags]`.
    pub frag_matrix: Matrix4F,
    pub bound_pointer: u64,
    pub name: Option<String>,
    /// Parallel arrays: a name index per matrix.
    pub frag_matrices_inds: Vec<u64>,
    /// The bound block this drawable's geometry collides with, when present.
    pub bound: Option<Bounds>,
}

impl FragDrawable {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let drawable = Drawable::read(r, false)?;

        let _unknown_0a8h = r.read_u64()?;
        let frag_matrix = read_matrix4f(r)?;
        let bound_pointer = r.read_u64()?;
        let frag_matrices_inds_pointer = r.read_u64()?;
        let frag_matrices_inds_count = r.read_u16()? as usize;
        let frag_matrices_capacity = r.read_u16()? as usize;
        let _unknown_104h = r.read_u32()?;
        let frag_matrices_pointer = r.read_u64()?;
        let _frag_matrices_count = r.read_u16()?;
        let _unknown_112h = r.read_u16()?;
        let _unknown_114h = r.read_u32()?;
        let _unknown_118h = r.read_u64()?;
        let _unknown_120h = r.read_u64()?;
        let _unknown_128h = r.read_u64()?;
        let name_pointer = r.read_u64()?;
        let _unknown_138h = r.read_u64()?;
        let _unknown_140h = r.read_u64()?;
        let _unknown_148h = r.read_u64()?;

        let bound = r.at(bound_pointer, Bounds::read)?;
        let frag_matrices_inds = r
            .read_u64s_at(frag_matrices_inds_pointer, frag_matrices_inds_count)?
            .unwrap_or_default();
        let _frag_matrices = read_matrix4fs_at(r, frag_matrices_pointer, frag_matrices_capacity)?;
        let name = r.read_string_at(name_pointer)?;

        Ok(Self {
            drawable,
            frag_matrix,
            bound_pointer,
            name,
            frag_matrices_inds,
            bound,
        })
    }
}

/// The `ResourcePointerList64<FragDrawable>`-shaped header at `pointer`, read
/// at `count` entries.
fn read_drawable_array(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: u32,
) -> Result<Vec<FragDrawable>> {
    if pointer == 0 {
        return Ok(Vec::new());
    }
    let pointers = r
        .read_u64s_at(pointer, count.min(MAX_ELEMENTS as u32) as usize)?
        .unwrap_or_default();
    let mut out = Vec::with_capacity(pointers.len());
    for p in pointers {
        if let Some(d) = r.at(p, FragDrawable::read)? {
            out.push(d);
        }
    }
    Ok(out)
}

fn read_drawable_array_names(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: u32,
) -> Result<Vec<Option<String>>> {
    if pointer == 0 {
        return Ok(Vec::new());
    }
    let pointers = r
        .read_u64s_at(pointer, count.min(MAX_ELEMENTS as u32) as usize)?
        .unwrap_or_default();
    let mut out = Vec::with_capacity(pointers.len());
    for p in pointers {
        out.push(r.read_string_at(p)?);
    }
    Ok(out)
}

fn read_glass_windows(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: u8,
) -> Result<Vec<FragGlassWindow>> {
    if pointer == 0 {
        return Ok(Vec::new());
    }
    let pointers = r.read_u64s_at(pointer, count as usize)?.unwrap_or_default();
    let mut out = Vec::with_capacity(pointers.len());
    for p in pointers {
        if let Some(w) = r.at(p, FragGlassWindow::read)? {
            out.push(w);
        }
    }
    Ok(out)
}

/// The header of a `ResourceSimpleList64<T>`: `{pointer, count, capacity, pad}`.
fn read_simple_list_header(r: &mut ResourceReader<'_>) -> Result<u64> {
    let pointer = r.read_u64()?;
    let _count = r.read_u16()?;
    let _capacity = r.read_u16()?;
    let _pad = r.read_u32()?;
    Ok(pointer)
}

/// The header of a `ResourcePointerList64<T>`: `{pointer, count, capacity, pad}`.
fn read_pointer_list_header(r: &mut ResourceReader<'_>) -> Result<u64> {
    let pointer = r.read_u64()?;
    let _count = r.read_u16()?;
    let _capacity = r.read_u16()?;
    let _pad = r.read_u32()?;
    Ok(pointer)
}

/// Bone transforms: a 32-byte header then `ItemCount1` 3x4 matrices inline.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragBoneTransforms {
    pub item_count: u8,
    /// One `Matrix3_s` (three `Vector4` rows) per bone.
    pub items: Vec<[[f32; 4]; 3]>,
}

impl FragBoneTransforms {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _unknown_00h = r.read_u64()?;
        let _unknown_08h = r.read_u64()?;
        let item_count = r.read_u8()?;
        let _item_count2 = r.read_u8()?;
        let _unknown_12h = r.read_u16()?;
        let _unknown_14h = r.read_u32()?;
        let _unknown_18h = r.read_u64()?;

        let mut items = Vec::with_capacity(item_count as usize);
        for _ in 0..item_count {
            items.push(read_matrix3(r)?);
        }
        Ok(Self { item_count, items })
    }
}

/// A 3x4 matrix: three `Vector4` rows.
fn read_matrix3(r: &mut ResourceReader<'_>) -> Result<[[f32; 4]; 3]> {
    Ok([
        [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?],
        [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?],
        [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?],
    ])
}

/// A shatterable glass window embedded in a fragment.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragGlassWindow {
    pub projection_rows: [[f32; 4]; 3],
    pub thickness: f32,
    pub flags: u16,
    pub tangent: [f32; 3],
}

impl FragGlassWindow {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let projection_row1 = [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?];
        let _unk_uint1 = r.read_u32()?;
        let projection_row2 = [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?];
        let _unk_uint2 = r.read_u32()?;
        let projection_row3 = [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?];
        let _unk_uint3 = r.read_u32()?;
        let _unk_float13 = r.read_f32()?;
        let _unk_float14 = r.read_f32()?;
        let _unk_float15 = r.read_f32()?;
        let _unk_float16 = r.read_f32()?;
        let _vertex_declaration = VertexDeclaration::read(r)?;
        let thickness = r.read_f32()?;
        let _unk_ushort1 = r.read_u16()?;
        let flags = r.read_u16()?;
        let _unk_float18 = r.read_f32()?;
        let _unk_float19 = r.read_f32()?;
        let tangent = r.read_vec3()?;
        let _unk_uint4 = r.read_u32()?;

        Ok(Self {
            projection_rows: [projection_row1, projection_row2, projection_row3],
            thickness,
            flags,
            tangent,
        })
    }
}

/// The whole-vehicle glass block: an offset table then one window per entry,
/// each padded to a 16-byte boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragVehicleGlassWindows {
    pub item_count: u16,
    pub windows: Vec<FragWindow>,
}

impl FragVehicleGlassWindows {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _unknown_0h = r.read_u32()?;
        let _unknown_4h = r.read_u16()?;
        let item_count = r.read_u16()? as usize;
        let _total_length = r.read_u32()?;
        let offset_count = item_count + (item_count & 1);
        let _window_offsets = read_uint_pairs_at_current(r, offset_count)?;
        let _unk_uint0 = r.read_u32()?;

        let mut windows = Vec::with_capacity(item_count);
        let mut consumed = 16 + offset_count * 8;
        for _ in 0..item_count {
            windows.push(FragWindow::read(r)?);
            consumed += windows.last().map(|w| w.read_length).unwrap_or(0);
            let pad = (16 - (consumed % 16)) % 16;
            if pad > 0 {
                let _ = r.read_bytes(pad)?;
                consumed += pad;
            }
        }

        Ok(Self {
            item_count: item_count as u16,
            windows,
        })
    }
}

fn read_uint_pairs_at_current(r: &mut ResourceReader<'_>, count: usize) -> Result<Vec<(u32, u32)>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push((r.read_u32()?, r.read_u32()?));
    }
    Ok(out)
}

/// One window in the vehicle glass block.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragWindow {
    pub projection: Matrix4F,
    pub item_id: u16,
    pub shatter_map_width: u16,
    pub item_data_count: u16,
    /// Length in bytes this window consumed, used by the caller's padding.
    pub read_length: usize,
}

impl FragWindow {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let start = r.position();
        let projection = read_matrix4f(r)?;
        let _unk_uint1 = r.read_u32()?;
        let item_id = r.read_u16()?;
        let _unk_ushort1 = r.read_u16()?;
        let shatter_map_width = r.read_u16()?;
        let item_data_count = r.read_u16()? as usize;
        let item_data_byte_length = r.read_u16()?;
        let _unk_ushort3 = r.read_u16()?;
        let _unk_uint2 = r.read_u32()?;
        let _unk_uint3 = r.read_u32()?;
        let _unk_float17 = r.read_f32()?;
        let _unk_float18 = r.read_f32()?;
        let _unk_ushort4 = r.read_u16()?;
        let _unk_ushort5 = r.read_u16()?;
        let _cracks_texture_tiling = r.read_f32()?;
        let _unk_uint4 = r.read_u32()?;
        let _unk_uint5 = r.read_u32()?;

        if item_data_byte_length != 0 {
            let _shatter_map_row_offsets = r.read_u16s_at(r.position(), item_data_count)?;
            for _ in 0..item_data_count {
                read_shatter_map_row(r)?;
            }
        }

        Ok(Self {
            projection,
            item_id,
            shatter_map_width,
            item_data_count: item_data_count as u16,
            read_length: (r.position() - start) as usize,
        })
    }
}

fn read_shatter_map_row(r: &mut ResourceReader<'_>) -> Result<()> {
    let start1 = r.read_u8()?;
    let end1 = r.read_u8()?;
    let n = end1 as i32 - start1 as i32 + 1;
    if n > 0 {
        let _data1 = r.read_bytes(n as usize)?;
        let start2 = r.read_u8()?;
        if start2 != 255 {
            let end2 = r.read_u8()?;
            let n2 = end2 as i32 - start2 as i32 + 1;
            if n2 > 0 {
                let _data2 = r.read_bytes(n2 as usize)?;
            }
        }
    }
    Ok(())
}

/// The three physics LODs a fragment can name.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragPhysicsLODGroup {
    pub lod1: Option<FragPhysicsLOD>,
    pub lod2: Option<FragPhysicsLOD>,
    pub lod3: Option<FragPhysicsLOD>,
}

impl FragPhysicsLODGroup {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _vft = r.read_u32()?;
        let _unknown_04h = r.read_u32()?;
        let _unknown_08h = r.read_u64()?;
        let lod1_pointer = r.read_u64()?;
        let lod2_pointer = r.read_u64()?;
        let lod3_pointer = r.read_u64()?;
        let _unknown_28h = r.read_u64()?;

        let lod1 = r.at(lod1_pointer, FragPhysicsLOD::read)?;
        let lod2 = r.at(lod2_pointer, FragPhysicsLOD::read)?;
        let lod3 = r.at(lod3_pointer, FragPhysicsLOD::read)?;

        Ok(Self { lod1, lod2, lod3 })
    }

    pub fn all_lods(&self) -> Vec<&FragPhysicsLOD> {
        [&self.lod1, &self.lod2, &self.lod3]
            .into_iter()
            .flatten()
            .collect()
    }
}

/// One physics LOD: a damping block, and the tree of groups and children.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragPhysicsLOD {
    pub position_offset: [f32; 3],
    pub groups: Vec<FragPhysTypeGroup>,
    pub children: Vec<FragPhysTypeChild>,
    pub archetype1: Option<FragPhysArchetype>,
    pub archetype2: Option<FragPhysArchetype>,
    pub bound: Option<Bounds>,
}

impl FragPhysicsLOD {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _vft = r.read_u32()?;
        let _unknown_04h = r.read_u32()?;
        let _unknown_08h = r.read_u64()?;
        let _unknown_10h = r.read_u32()?;
        let _unknown_14h = r.read_f32()?;
        let _unknown_18h = r.read_f32()?;
        let _unknown_1ch = r.read_f32()?;
        let _articulated_body_type_pointer = r.read_u64()?;
        let _children_unk_floats_pointer = r.read_u64()?;
        let position_offset = r.read_vec3()?;
        let _unknown_3ch = r.read_u32()?;
        let _unknown_40h = r.read_vec3()?;
        let _unknown_4ch = r.read_u32()?;
        let _unknown_50h = r.read_vec3()?;
        let _unknown_5ch = r.read_u32()?;
        let _damping_linear_c = r.read_vec3()?;
        let _unknown_6ch = r.read_u32()?;
        let _damping_linear_v = r.read_vec3()?;
        let _unknown_7ch = r.read_u32()?;
        let _damping_linear_v2 = r.read_vec3()?;
        let _unknown_8ch = r.read_u32()?;
        let _damping_angular_c = r.read_vec3()?;
        let _unknown_9ch = r.read_u32()?;
        let _damping_angular_v = r.read_vec3()?;
        let _unknown_ach = r.read_u32()?;
        let _damping_angular_v2 = r.read_vec3()?;
        let _unknown_bch = r.read_u32()?;
        let group_names_pointer = r.read_u64()?;
        let groups_pointer = r.read_u64()?;
        let children_pointer = r.read_u64()?;
        let archetype1_pointer = r.read_u64()?;
        let archetype2_pointer = r.read_u64()?;
        let bound_pointer = r.read_u64()?;
        let _children_inertia_tensors_pointer = r.read_u64()?;
        let _children_unk_vecs_pointer = r.read_u64()?;
        let _frag_transforms_pointer = r.read_u64()?;
        let _unknown_data1_pointer = r.read_u64()?;
        let _unknown_data2_pointer = r.read_u64()?;
        let _unknown_data1_count = r.read_u8()?;
        let _unknown_data2_count = r.read_u8()?;
        let groups_count = r.read_u8()? as usize;
        let _root_groups_count = r.read_u8()?;
        let _unknown_11ch = r.read_u8()?;
        let children_count = r.read_u8()? as usize;
        let _children_count2 = r.read_u8()?;
        let _unknown_11fh = r.read_u8()?;
        let _unknown_120h = r.read_u64()?;
        let _unknown_128h = r.read_u64()?;

        let groups = read_type_groups(r, groups_pointer, groups_count)?;
        // Group names may live in a sibling block; vanilla files embed them in
        // the groups themselves, so reading the block is skipped.
        let _group_names_pointer = group_names_pointer;
        let children = read_type_children(r, children_pointer, children_count)?;
        let archetype1 = r.at(archetype1_pointer, FragPhysArchetype::read)?;
        let archetype2 = r.at(archetype2_pointer, FragPhysArchetype::read)?;
        let bound = r.at(bound_pointer, Bounds::read)?;

        Ok(Self {
            position_offset,
            groups,
            children,
            archetype1,
            archetype2,
            bound,
        })
    }
}

fn read_type_groups(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: usize,
) -> Result<Vec<FragPhysTypeGroup>> {
    if pointer == 0 {
        return Ok(Vec::new());
    }
    let pointers = r
        .read_u64s_at(pointer, count.min(MAX_ELEMENTS))?
        .unwrap_or_default();
    let mut out = Vec::with_capacity(pointers.len());
    for p in pointers {
        if let Some(g) = r.at(p, FragPhysTypeGroup::read)? {
            out.push(g);
        }
    }
    Ok(out)
}

fn read_type_children(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: usize,
) -> Result<Vec<FragPhysTypeChild>> {
    if pointer == 0 {
        return Ok(Vec::new());
    }
    let pointers = r
        .read_u64s_at(pointer, count.min(MAX_ELEMENTS))?
        .unwrap_or_default();
    let mut out = Vec::with_capacity(pointers.len());
    for p in pointers {
        if let Some(c) = r.at(p, FragPhysTypeChild::read)? {
            out.push(c);
        }
    }
    Ok(out)
}

/// One physics child: a pair of drawables (damaged/intact) and an event set.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragPhysTypeChild {
    pub pristine_mass: f32,
    pub damaged_mass: f32,
    pub group_index: u16,
    pub bone_tag: u16,
    pub drawable1: Option<FragDrawable>,
    pub drawable2: Option<FragDrawable>,
}

impl FragPhysTypeChild {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _vft = r.read_u32()?;
        let _unknown_04h = r.read_u32()?;
        let pristine_mass = r.read_f32()?;
        let damaged_mass = r.read_f32()?;
        let group_index = r.read_u16()?;
        let bone_tag = r.read_u16()?;
        let _unknown_14h = r.read_u32()?;
        let _unknown_18h = r.read_u64()?;
        let _unknown_20h = r.read_u64()?;
        let _unknown_28h = r.read_u64()?;
        let _unknown_30h = r.read_u64()?;
        let _unknown_38h = r.read_u64()?;
        let _unknown_40h = r.read_u64()?;
        let _unknown_48h = r.read_u64()?;
        let _unknown_50h = r.read_u64()?;
        let _unknown_58h = r.read_u64()?;
        let _unknown_60h = r.read_u64()?;
        let _unknown_68h = r.read_u64()?;
        let _unknown_70h = r.read_u64()?;
        let _unknown_78h = r.read_u64()?;
        let _unknown_80h = r.read_u64()?;
        let _unknown_88h = r.read_u64()?;
        let _unknown_90h = r.read_u64()?;
        let _unknown_98h = r.read_u64()?;
        let drawable1_pointer = r.read_u64()?;
        let drawable2_pointer = r.read_u64()?;
        let _evt_set_pointer = r.read_u64()?;
        let _unknown_b8h = r.read_u64()?;
        let _unknown_c0h = r.read_u64()?;
        let _unknown_c8h = r.read_u64()?;
        let _unknown_d0h = r.read_u64()?;
        let _unknown_d8h = r.read_u64()?;
        let _unknown_e0h = r.read_u64()?;
        let _unknown_e8h = r.read_u64()?;
        let _unknown_f0h = r.read_u64()?;
        let _unknown_f8h = r.read_u64()?;

        let drawable1 = r.at(drawable1_pointer, FragDrawable::read)?;
        let drawable2 = r.at(drawable2_pointer, FragDrawable::read)?;

        Ok(Self {
            pristine_mass,
            damaged_mass,
            group_index,
            bone_tag,
            drawable1,
            drawable2,
        })
    }
}

/// A physics group: the damage and joint limits applied to a set of children.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragPhysTypeGroup {
    pub strength: f32,
    pub child_group_index: u8,
    pub parent_index: u8,
    pub child_index: u8,
    pub child_count: u8,
    pub glass_window_index: u8,
    pub glass_flags: u8,
    pub name: Option<String>,
}

impl FragPhysTypeGroup {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _unknown_00h = r.read_f32()?;
        let _unknown_04h = r.read_f32()?;
        let _unknown_08h = r.read_f32()?;
        let _unknown_0ch = r.read_f32()?;
        let strength = r.read_f32()?;
        let _force_transmission_scale_up = r.read_f32()?;
        let _force_transmission_scale_down = r.read_f32()?;
        let _joint_stiffness = r.read_f32()?;
        let _min_soft_angle1 = r.read_f32()?;
        let _max_soft_angle1 = r.read_f32()?;
        let _max_soft_angle2 = r.read_f32()?;
        let _max_soft_angle3 = r.read_f32()?;
        let _rotation_speed = r.read_f32()?;
        let _rotation_strength = r.read_f32()?;
        let _restoring_strength = r.read_f32()?;
        let _restoring_max_torque = r.read_f32()?;
        let _latch_strength = r.read_f32()?;
        let _mass = r.read_f32()?;
        let _unknown_48h = r.read_f32()?;
        let child_group_index = r.read_u8()?;
        let parent_index = r.read_u8()?;
        let child_index = r.read_u8()?;
        let child_count = r.read_u8()?;
        let _child_group_count = r.read_u8()?;
        let _unk_byte51 = r.read_u8()?;
        let glass_window_index = r.read_u8()?;
        let glass_flags = r.read_u8()?;
        let _min_damage_force = r.read_f32()?;
        let _damage_health = r.read_f32()?;
        let _unk_float5c = r.read_f32()?;
        let _unk_float60 = r.read_f32()?;
        let _unk_float64 = r.read_f32()?;
        let _unk_float68 = r.read_f32()?;
        let _unk_float6c = r.read_f32()?;
        let _unk_float70 = r.read_f32()?;
        let _unk_float74 = r.read_f32()?;
        let _unk_float78 = r.read_f32()?;
        let _unknown_7ch = r.read_f32()?;
        let name = read_name_struct(r)?;
        let _unk_floata8 = r.read_f32()?;
        let _unknown_ach = r.read_f32()?;

        Ok(Self {
            strength,
            child_group_index,
            parent_index,
            child_index,
            child_count,
            glass_window_index,
            glass_flags,
            name,
        })
    }
}

/// A 40-byte packed name: ten little-endian uints carrying four ASCII chars
/// each. Trailing NULs are trimmed.
fn read_name_struct(r: &mut ResourceReader<'_>) -> Result<Option<String>> {
    let mut bytes = Vec::with_capacity(40);
    for _ in 0..10 {
        bytes.extend_from_slice(&r.read_u32()?.to_le_bytes());
    }
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    let s = String::from_utf8_lossy(&bytes).into_owned();
    Ok((!s.is_empty()).then_some(s))
}

/// One physics archetype: the bound a LOD's children pivot around.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragPhysArchetype {
    pub name: Option<String>,
    pub mass: f32,
    pub bound: Option<Bounds>,
}

impl FragPhysArchetype {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _vft = r.read_u32()?;
        let _unknown_04h = r.read_u32()?;
        let _unknown_08h = r.read_u64()?;
        let _unknown_10h = r.read_u64()?;
        let name_pointer = r.read_u64()?;
        let bound_pointer = r.read_u64()?;
        let _unknown_28h = r.read_u32()?;
        let _unknown_2ch = r.read_u32()?;
        let _unknown_30h = r.read_u32()?;
        let _unknown_34h = r.read_u32()?;
        let _unknown_38h = r.read_u64()?;
        let mass = r.read_f32()?;
        let _mass_inv = r.read_f32()?;
        let _unknown_48h = r.read_f32()?;
        let _unknown_4ch = r.read_f32()?;
        let _unknown_50h = r.read_f32()?;
        let _unknown_54h = r.read_f32()?;
        let _unknown_58h = r.read_u64()?;
        let _inertia_tensor = r.read_vec3()?;
        let _unknown_6ch = r.read_u32()?;
        let _inertia_tensor_inv = r.read_vec3()?;
        let _unknown_7ch = r.read_u32()?;
        let _unknown_80h = r.read_vec3()?;
        let _unknown_8ch = r.read_u32()?;
        let _unknown_90h = r.read_vec3()?;
        let _unknown_9ch = r.read_u32()?;
        let _unknown_a0h = r.read_vec3()?;
        let _unknown_ach = r.read_u32()?;
        let _unknown_b0h = r.read_vec3()?;
        let _unknown_bch = r.read_u32()?;
        let _unknown_c0h = r.read_vec3()?;
        let _unknown_cch = r.read_u32()?;
        let _unknown_d0h = r.read_vec3()?;
        let _unknown_dch = r.read_u32()?;

        let name = r.read_string_at(name_pointer)?;
        let bound = r.at(bound_pointer, Bounds::read)?;

        Ok(Self { name, mass, bound })
    }
}

/// Read a `Matrix4F` (four `[x, y, z, flags]` columns) at the cursor.
pub(crate) fn read_matrix4f(r: &mut ResourceReader<'_>) -> Result<Matrix4F> {
    Ok(Matrix4F {
        column1: [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?],
        column2: [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?],
        column3: [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?],
        column4: [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?],
    })
}

fn read_matrix4fs_at(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: usize,
) -> Result<Vec<Matrix4F>> {
    if pointer == 0 {
        return Ok(Vec::new());
    }
    let bytes = r.read_bytes_at(pointer, count * 64)?.unwrap_or_default();
    let mut out = Vec::with_capacity(bytes.len() / 64);
    for c in bytes.chunks_exact(64) {
        let f = |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
        out.push(Matrix4F {
            column1: [f(0), f(4), f(8), f(12)],
            column2: [f(16), f(20), f(24), f(28)],
            column3: [f(32), f(36), f(40), f(44)],
            column4: [f(48), f(52), f(56), f(60)],
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::reader::SYSTEM_BASE;

    #[test]
    fn name_struct_reads_four_chars_per_uint_and_trims() {
        let mut data = vec![0u8; 44];
        for (i, b) in b"abcd".iter().enumerate() {
            data[i] = *b;
        }
        let mut r = ResourceReader::new(&data, 44);
        let name = read_name_struct(&mut r).unwrap();
        assert_eq!(name.as_deref(), Some("abcd"));
    }

    #[test]
    fn empty_name_struct_reads_as_none() {
        let data = vec![0u8; 40];
        let mut r = ResourceReader::new(&data, 40);
        let name = read_name_struct(&mut r).unwrap();
        assert_eq!(name, None);
    }

    /// A minimal `.yft` shell: a fragment naming one string and nothing else.
    /// Reading it must not require the physics blocks to exist.
    #[test]
    fn reads_a_fragment_shell() {
        let mut data = vec![0u8; 0x420];
        let put_u64 = |d: &mut Vec<u8>, at: usize, v: u64| {
            d[at..at + 8].copy_from_slice(&v.to_le_bytes());
        };
        let put_u32 = |d: &mut Vec<u8>, at: usize, v: u32| {
            d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let addr = |offset: u64| SYSTEM_BASE + offset;

        put_u32(&mut data, 0x00, 0x40_5E_36_00); // vft
        put_u32(&mut data, 0x04, 1);
        // BoundingSphereCenter at 0x20 (12 bytes), radius at 0x2C.
        data[0x20..0x2C].copy_from_slice(
            &[1.0f32, 2.0, 3.0]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        data[0x2C..0x30].copy_from_slice(&4.0f32.to_le_bytes());
        // NamePointer at 0x58.
        put_u64(&mut data, 0x58, addr(0x400));
        // Cloths pointer list header at 0x60: null pointer, zeroed counts.
        // PhysicsLODGroupPointer at 0xF0: null.
        // LightAttributes simple list at 0x110: null pointer.
        data[0x400..0x400 + 6].copy_from_slice(b"bench\0");

        let mut r = ResourceReader::new(&data, 0x420);
        let fragment = FragType::read(&mut r).expect("read a fragment shell");
        assert_eq!(fragment.name.as_deref(), Some("bench"));
        assert_eq!(fragment.bounding_sphere_radius, 4.0);
        assert_eq!(fragment.bounding_sphere_center, [1.0, 2.0, 3.0]);
        assert!(fragment.drawable.is_none());
        assert!(fragment.drawable_array.is_empty());
        assert!(fragment.physics_lod_group.is_none());
        assert!(fragment.all_drawables().is_empty());
    }
}
