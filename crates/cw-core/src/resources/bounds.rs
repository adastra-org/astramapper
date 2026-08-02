//! Collision bounds: the read half of `Bounds.cs` behind the `.ybn` shell of
//! `YbnFile.cs`.
//!
//! A `.ybn` is one `Bounds` at the start of the payload. The reference reads it
//! through `IResourceXXSystemBlock.GetType`, which dispatches on the type byte
//! at offset `0x10` before the shared fields are read — so the whole hierarchy
//! (`BoundSphere`, `BoundBox`, `BoundGeometry`, `BoundBVH`, `BoundComposite`,
//! …) is really one 112-byte base struct plus a per-type tail. That is how this
//! port models it: a single [`Bounds`] whose [`BoundsData`] carries the tail.
//!
//! Scope: everything a viewer needs to draw the shape — the composite tree, the
//! geometry's vertices and polygons, materials, the octant and BVH acceleration
//! structures. Deliberately not ported here: the XML round trip, the vertex
//! *shrink* pass (which is editor, not file), and every `SphereIntersect` /
//! `RayIntersect` collision routine in `CodeWalker/World`. A `.ybn` has no
//! writing either, like every other resource in this crate, so `formats.md`
//! marks it **partial**.
//!
//! Four things about the layout that are not guessable from the struct listing:
//!
//! 1. **The type byte rides inside a shared base.** `BoundsType` lives at the
//!    *first* byte of the base struct, and `GetType` peeks it by jumping 16
//!    bytes in — past the `ResourceFileBase` header — and back. A port that
//!    dispatches on the extension instead would mislabel a `Composite` as a
//!    `Box`; the file carries its own discriminator.
//! 2. **A polygon record keeps its type in the low bits of its first byte.**
//!    `ReadPolygons` masks byte 0 with `0xF8` before parsing, so the area field
//!    is read with three bits cleared, and the type is `b0 & 7`. Triangle
//!    vertex indices carry an `0x8000` "is a flag" bit that is *not* part of
//!    the index; the reference masks it off wherever it addresses a vertex.
//! 3. **Materials are padded to four on the way in and truncated back.** The
//!    reference reads `max(4, MaterialsCount)` entries and then, when the count
//!    is below four, copies out only the real ones. The array handed to callers
//!    is always the short one.
//! 4. **The octant item pointers are a sibling block, not a tail.** The 32
//!    bytes of counts are read at `OctantsPointer`; the eight `u64` item
//!    pointers live at `OctantItemsPointer`, which the caller passes as a
//!    parameter. Both pointers come from the geometry header.

use serde::Serialize;

use crate::Result;

use super::drawable::ResourceFileBase;
use super::reader::ResourceReader;

/// Guard on any count read out of a file before it is used to size an
/// allocation. Nothing in a stock bounds file comes close; a corrupt or hostile
/// file can name any 32-bit number.
const MAX_ELEMENTS: usize = 16_000_000;

/// The `BoundsType` byte every bounds block leads with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BoundsType {
    None,
    Sphere,
    Capsule,
    Box,
    Geometry,
    GeometryBvh,
    Composite,
    Disc,
    Cylinder,
    Cloth,
}

impl BoundsType {
    /// The byte values in files. `None` is 255 — the reference uses it only as
    /// a placeholder in XML, never in a file, but it is kept because `Create`
    /// returns null for it.
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Sphere,
            1 => Self::Capsule,
            3 => Self::Box,
            4 => Self::Geometry,
            8 => Self::GeometryBvh,
            10 => Self::Composite,
            12 => Self::Disc,
            13 => Self::Cylinder,
            15 => Self::Cloth,
            _ => Self::None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Sphere => "Sphere",
            Self::Capsule => "Capsule",
            Self::Box => "Box",
            Self::Geometry => "Geometry",
            Self::GeometryBvh => "GeometryBVH",
            Self::Composite => "Composite",
            Self::Disc => "Disc",
            Self::Cylinder => "Cylinder",
            Self::Cloth => "Cloth",
        }
    }
}

/// A bounds block. The shared 112-byte base, then a per-type tail.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub base: ResourceFileBase,
    pub bounds_type: BoundsType,
    pub sphere_radius: f32,
    pub box_max: [f32; 3],
    pub margin: f32,
    pub box_min: [f32; 3],
    pub unknown_3ch: u32,
    pub box_center: [f32; 3],
    pub material_index: u8,
    pub procedural_id: u8,
    /// Five bits of room id, then three of ped density. See [`Bounds::room_id`]
    /// and [`Bounds::ped_density`].
    pub room_id_and_ped_density: u8,
    pub unk_flags: u8,
    pub sphere_center: [f32; 3],
    pub poly_flags: u8,
    pub material_color_index: u8,
    /// Inertia, per the XML name the reference writes.
    pub unknown_60h: [f32; 3],
    pub volume: f32,
    pub data: BoundsData,
}

impl Bounds {
    /// The room id is the low five bits of the packed byte.
    pub fn room_id(&self) -> u8 {
        self.room_id_and_ped_density & 0x1F
    }

    /// The ped density is the high three bits of the packed byte.
    pub fn ped_density(&self) -> u8 {
        self.room_id_and_ped_density >> 5
    }

    /// Whether this is a geometry (or BVH geometry) with vertices to draw.
    pub fn has_geometry(&self) -> bool {
        match &self.data {
            BoundsData::Geometry(g) => !g.vertices.is_empty() || !g.polygons.is_empty(),
            BoundsData::Bvh(g) => !g.geometry.vertices.is_empty() || !g.geometry.polygons.is_empty(),
            _ => false,
        }
    }

    /// Read one `Bounds` at the cursor.
    ///
    /// Dispatch mirrors the reference exactly: peek the type byte sixteen bytes
    /// in, then read the shared base and the matching tail from the same
    /// position.
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let start = r.position();
        r.set_position(start + 16);
        let bounds_type = BoundsType::from_u8(r.read_u8()?);
        r.set_position(start);

        let base = ResourceFileBase::read(r)?;
        // The type byte was already peeked for the dispatch; skip the rest of
        // the leading fields — `Type`, `Unknown_11h`, `Unknown_12h` — before
        // the shared values start at 0x14.
        let _ = r.read_u8()?;
        let _unknown_11h = r.read_u8()?;
        let _unknown_12h = r.read_u16()?;
        let sphere_radius = r.read_f32()?;
        let _unknown_18h = r.read_u32()?;
        let _unknown_1ch = r.read_u32()?;
        let box_max = r.read_vec3()?;
        let margin = r.read_f32()?;
        let box_min = r.read_vec3()?;
        let unknown_3ch = r.read_u32()?;
        let box_center = r.read_vec3()?;
        let material_index = r.read_u8()?;
        let procedural_id = r.read_u8()?;
        let room_id_and_ped_density = r.read_u8()?;
        let unk_flags = r.read_u8()?;
        let sphere_center = r.read_vec3()?;
        let poly_flags = r.read_u8()?;
        let material_color_index = r.read_u8()?;
        let _unknown_5eh = r.read_u16()?;
        let unknown_60h = r.read_vec3()?;
        let volume = r.read_f32()?;

        let data = match bounds_type {
            BoundsType::Capsule | BoundsType::Disc | BoundsType::Cylinder => {
                let _unknown_70h = r.read_u32()?;
                let _unknown_74h = r.read_u32()?;
                let _unknown_78h = r.read_u32()?;
                let _unknown_7ch = r.read_u32()?;
                BoundsData::Primitive
            }
            BoundsType::Geometry => BoundsData::Geometry(BoundGeometry::read(r)?),
            BoundsType::GeometryBvh => BoundsData::Bvh(BoundBvh::read(r)?),
            BoundsType::Composite => BoundsData::Composite(BoundComposite::read(r)?),
            _ => BoundsData::Primitive,
        };

        Ok(Self {
            base,
            bounds_type,
            sphere_radius,
            box_max,
            margin,
            box_min,
            unknown_3ch,
            box_center,
            material_index,
            procedural_id,
            room_id_and_ped_density,
            unk_flags,
            sphere_center,
            poly_flags,
            material_color_index,
            unknown_60h,
            volume,
            data,
        })
    }
}

/// The per-type tail of a [`Bounds`]. The primitive shapes carry no extra bytes
/// beyond the shared base.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BoundsData {
    Primitive,
    Geometry(BoundGeometry),
    Bvh(BoundBvh),
    Composite(BoundComposite),
}

/// The geometry tail (`BoundGeometry`): a 192-byte header of pointers and
/// counts, then the arrays it names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundGeometry {
    /// Vertices shrunk along their normals by the margin. Present in `.yft`.
    pub vertices_shrunk: Vec<[f32; 3]>,
    pub polygons: Vec<BoundPolygon>,
    pub vertices: Vec<[f32; 3]>,
    /// The geometry's origin; a vertex's world position is `vertex + center`.
    /// See `GetVertexPos` in the reference.
    pub center_geom: [f32; 3],
    /// The per-axis scale stored quantised vertices are multiplied by. Kept
    /// because the gate checks it and the vertex decode depends on it.
    pub quantum: [f32; 3],
    pub vertex_colours: Vec<BoundMaterialColour>,
    pub octants: Option<BoundGeomOctants>,
    pub materials: Vec<BoundMaterial>,
    pub material_colours: Vec<BoundMaterialColour>,
    pub polygon_material_indices: Vec<u8>,
    pub vertices_count: usize,
    pub polygons_count: usize,
}

impl BoundGeometry {
    /// The material index a polygon's triangle records. Zero is the default
    /// when the parallel arrays are absent, as in the reference's
    /// `GetMaterialIndex`.
    pub fn material_index_of(&self, polygon: usize) -> u8 {
        self.polygon_material_indices
            .get(polygon)
            .copied()
            .unwrap_or(0)
    }

    /// Read a `BoundGeometry` at the cursor, which must be positioned at the
    /// start of its tail (offset `0x70` of the owning `Bounds`).
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _unknown_70h = r.read_u32()?;
        let _unknown_74h = r.read_u32()?;
        let vertices_shrunk_pointer = r.read_u64()?;
        let _unknown_80h = r.read_u16()?;
        let _unknown_82h = r.read_u16()?;
        let vertices_shrunk_count = r.read_u32()? as usize;
        let polygons_pointer = r.read_u64()?;
        let quantum = r.read_vec3()?;
        let _unknown_9ch = r.read_f32()?;
        let center_geom = r.read_vec3()?;        let _unknown_ach = r.read_f32()?;
        let vertices_pointer = r.read_u64()?;
        let vertex_colours_pointer = r.read_u64()?;
        let octants_pointer = r.read_u64()?;
        let octant_items_pointer = r.read_u64()?;
        let vertices_count = r.read_u32()? as usize;
        let polygons_count = r.read_u32()? as usize;
        let _unknown_d8h = r.read_u32()?;
        let _unknown_dch = r.read_u32()?;
        let _unknown_e0h = r.read_u32()?;
        let _unknown_e4h = r.read_u32()?;
        let _unknown_e8h = r.read_u32()?;
        let _unknown_ech = r.read_u32()?;
        let materials_pointer = r.read_u64()?;
        let material_colours_pointer = r.read_u64()?;
        let _unknown_100h = r.read_u32()?;
        let _unknown_104h = r.read_u32()?;
        let _unknown_108h = r.read_u32()?;
        let _unknown_10ch = r.read_u32()?;
        let _unknown_110h = r.read_u32()?;
        let _unknown_114h = r.read_u32()?;
        let polygon_material_indices_pointer = r.read_u64()?;
        let materials_count = r.read_u8()? as usize;
        let material_colours_count = r.read_u8()? as usize;
        let _unknown_122h = r.read_u16()?;
        let _unknown_124h = r.read_u32()?;
        let _unknown_128h = r.read_u32()?;
        let _unknown_12ch = r.read_u32()?;

        let vertices_shrunk_count = vertices_shrunk_count.min(MAX_ELEMENTS);
        let vertices_count = vertices_count.min(MAX_ELEMENTS);
        let polygons_count = polygons_count.min(MAX_ELEMENTS);

        let vertices_shrunk = r
            .read_bytes_at(vertices_shrunk_pointer, vertices_shrunk_count * 6)?
            .map(|bytes| decode_quantized_vertices(&bytes, quantum))
            .unwrap_or_default();

        let polygons = read_polygons(r, polygons_pointer, polygons_count)?;

        let vertices = r
            .read_bytes_at(vertices_pointer, vertices_count * 6)?
            .map(|bytes| decode_quantized_vertices(&bytes, quantum))
            .unwrap_or_default();

        let vertex_colours = r
            .read_bytes_at(vertex_colours_pointer, vertices_count * 4)
            .map(|bytes| bytes.unwrap_or_default().chunks_exact(4).map(colour).collect())
            .unwrap_or_default();

        let octants = r
            .at(octants_pointer, |r| BoundGeomOctants::read(r, octant_items_pointer))?;

        // Materials are padded to four on the way in so the reader never sees
        // a short block, then truncated back to the declared count. The
        // reference reads `max(4, count)` and copies out the real ones.
        let padded_material_count = materials_count.clamp(4, MAX_ELEMENTS);
        let materials: Vec<BoundMaterial> = r
            .read_bytes_at(materials_pointer, padded_material_count * 8)?
            .map(|bytes| {
                bytes
                    .chunks_exact(8)
                    .map(|c| BoundMaterial {
                        data1: u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                        data2: u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let materials: Vec<BoundMaterial> = materials.into_iter().take(materials_count).collect();

        let material_colours = r
            .read_bytes_at(material_colours_pointer, material_colours_count * 4)
            .map(|bytes| bytes.unwrap_or_default().chunks_exact(4).map(colour).collect())
            .unwrap_or_default();

        let polygon_material_indices = r
            .read_bytes_at(polygon_material_indices_pointer, polygons_count)?
            .unwrap_or_default();

        Ok(Self {
            vertices_shrunk,
            polygons,
            vertices,
            center_geom,
            quantum,
            vertex_colours,
            octants,
            materials,
            material_colours,
            polygon_material_indices,
            vertices_count,
            polygons_count,
        })
    }
}

/// The `BoundBVH` tail: the geometry plus a BVH block.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundBvh {
    pub geometry: BoundGeometry,
    pub bvh: Option<Bvh>,
}

impl BoundBvh {
    /// Read a `BoundBVH` at the cursor, which must be positioned at the start
    /// of its tail (offset `0x70` of the owning `Bounds`).
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let geometry = BoundGeometry::read(r)?;
        let bvh_pointer = r.read_u64()?;
        let _unknown_138h = r.read_u32()?;
        let _unknown_13ch = r.read_u32()?;
        let _unknown_140h = r.read_u16()?;
        let _unknown_142h = r.read_u16()?;
        let _unknown_144h = r.read_u32()?;
        let _unknown_148h = r.read_u32()?;
        let _unknown_14ch = r.read_u32()?;

        // The reference skips the block for small pointers: "this can happen in
        // some ydr's for some reason".
        let bvh = if bvh_pointer > 65_535 {
            r.at(bvh_pointer, Bvh::read)?
        } else {
            None
        };

        Ok(Self { geometry, bvh })
    }
}

/// The `BoundComposite` tail: up to 65535 children, each with two transform
/// matrices, a bounding box, two flag words, and an optional BVH.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundComposite {
    pub children: Vec<Bounds>,
    pub children_transformation1: Vec<Matrix4F>,
    pub children_transformation2: Vec<Matrix4F>,
    pub children_bounding_boxes: Vec<[f32; 8]>,
    pub children_flags1: Vec<[u32; 2]>,
    pub children_flags2: Vec<[u32; 2]>,
    pub bvh: Option<Bvh>,
}

impl BoundComposite {
    /// Read a `BoundComposite` at the cursor, which must be positioned at the
    /// start of its tail (offset `0x70` of the owning `Bounds`).
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let children_pointer = r.read_u64()?;
        let children_transformation1_pointer = r.read_u64()?;
        let children_transformation2_pointer = r.read_u64()?;
        let children_bounding_boxes_pointer = r.read_u64()?;
        let children_flags1_pointer = r.read_u64()?;
        let children_flags2_pointer = r.read_u64()?;
        let children_count1 = r.read_u16()? as usize;
        let children_count2 = r.read_u16()? as usize;
        let _unknown_a4h = r.read_u32()?;
        let bvh_pointer = r.read_u64()?;

        let count = children_count1.min(children_count2).min(MAX_ELEMENTS);

        let children: Vec<Option<Bounds>> = r
            .read_u64s_at(children_pointer, count)?
            .unwrap_or_default()
            .into_iter()
            .map(|pointer| r.at(pointer, Bounds::read))
            .collect::<Result<Vec<_>>>()?;
        let children: Vec<Bounds> = children.into_iter().flatten().collect();

        let children_transformation1 = read_matrix_array(r, children_transformation1_pointer, count)?;
        let children_transformation2 = read_matrix_array(r, children_transformation2_pointer, count)?;
        let children_bounding_boxes = read_aabb_array(r, children_bounding_boxes_pointer, count)?;
        let children_flags1 = read_flag_array(r, children_flags1_pointer, count)?;
        let children_flags2 = read_flag_array(r, children_flags2_pointer, count)?;
        let bvh = r.at(bvh_pointer, Bvh::read)?;

        Ok(Self {
            children,
            children_transformation1,
            children_transformation2,
            children_bounding_boxes,
            children_flags1,
            children_flags2,
            bvh,
        })
    }
}

/// The `BVH` block: node and tree lists plus the quantisation vectors that
/// turn their `i16` bounds back into space.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bvh {
    pub nodes: Vec<BvhNode>,
    pub bounding_box_min: [f32; 4],
    pub bounding_box_max: [f32; 4],
    pub bounding_box_center: [f32; 4],
    pub quantum_inverse: [f32; 4],
    pub quantum: [f32; 4],
    pub trees: Vec<BvhTree>,
}

impl Bvh {
    /// Read a `BVH` block at the cursor.
    pub     fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        // `Nodes` is a `ResourceSimpleList64b_s` — the "b" variant, with 32-bit
        // count and capacity and no trailing pad, and it is read by *capacity*.
        let nodes_pointer = r.read_u64()?;
        let _nodes_count = r.read_u32()?;
        let nodes_capacity = r.read_u32()? as usize;
        let _unknown_10h = r.read_u32()?;
        let _unknown_14h = r.read_u32()?;
        let _unknown_18h = r.read_u32()?;
        let _unknown_1ch = r.read_u32()?;
        let bounding_box_min = read_vec4(r)?;
        let bounding_box_max = read_vec4(r)?;
        let bounding_box_center = read_vec4(r)?;
        let quantum_inverse = read_vec4(r)?;
        let quantum = read_vec4(r)?;
        // `Trees` is a `ResourceSimpleList64_s` — 16-bit count and capacity
        // plus a pad word, read by *count*.
        let trees_pointer = r.read_u64()?;
        let trees_count = r.read_u16()? as usize;
        let _trees_capacity = r.read_u16()?;
        let _trees_pad = r.read_u32()?;

        let nodes = r
            .read_bytes_at(nodes_pointer, nodes_capacity.min(MAX_ELEMENTS) * 16)?
            .map(|bytes| bytes.chunks_exact(16).map(BvhNode::from_bytes).collect())
            .unwrap_or_default();

        let trees = r
            .read_bytes_at(trees_pointer, trees_count.min(MAX_ELEMENTS) * 16)?
            .map(|bytes| bytes.chunks_exact(16).map(BvhTree::from_bytes).collect())
            .unwrap_or_default();

        Ok(Self {
            nodes,
            bounding_box_min,
            bounding_box_max,
            bounding_box_center,
            quantum_inverse,
            quantum,
            trees,
        })
    }
}

/// A `BVHNode_s`: six quantised `i16` bounds plus item id and count.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BvhNode {
    pub min: [i16; 3],
    pub max: [i16; 3],
    pub item_id: i16,
    pub item_count: i16,
}

impl BvhNode {
    fn from_bytes(c: &[u8]) -> Self {
        let s = |i: usize| i16::from_le_bytes([c[i * 2], c[i * 2 + 1]]);
        Self {
            min: [s(0), s(1), s(2)],
            max: [s(3), s(4), s(5)],
            item_id: s(6),
            item_count: s(7),
        }
    }
}

/// A `BVHTreeInfo_s`: six quantised `i16` bounds plus a node range.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BvhTree {
    pub min: [i16; 3],
    pub max: [i16; 3],
    pub node_index1: i16,
    pub node_index2: i16,
}

impl BvhTree {
    fn from_bytes(c: &[u8]) -> Self {
        let s = |i: usize| i16::from_le_bytes([c[i * 2], c[i * 2 + 1]]);
        Self {
            min: [s(0), s(1), s(2)],
            max: [s(3), s(4), s(5)],
            node_index1: s(6),
            node_index2: s(7),
        }
    }
}

/// The octant occupancy lists of a `BoundGeometry`. Eight counts read at the
/// octant pointer; eight item pointers read at the separately-named item
/// pointer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundGeomOctants {
    pub counts: [u32; 8],
    pub items: Vec<Vec<u32>>,
}

impl BoundGeomOctants {
    fn read(r: &mut ResourceReader<'_>, items_pointer: u64) -> Result<Self> {
        let mut counts = [0u32; 8];
        for count in &mut counts {
            *count = r.read_u32()?;
        }

        let pointers = r.read_u64s_at(items_pointer, 8)?.unwrap_or_default();
        let mut items = Vec::with_capacity(8);
        for (i, pointer) in pointers.iter().enumerate() {
            let count = (counts.get(i).copied().unwrap_or(0) as usize).min(MAX_ELEMENTS);
            items.push(r.read_u32s_at(*pointer, count)?.unwrap_or_default());
        }

        Ok(Self { counts, items })
    }
}

/// A polygon of a collision geometry. The 16-byte record's low bits of its
/// first byte carry the type; the byte is masked before the first field is read.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum BoundPolygon {
    Triangle(BoundPolygonTriangle),
    Sphere(BoundPolygonSphere),
    Capsule(BoundPolygonCapsule),
    Box(BoundPolygonBox),
    Cylinder(BoundPolygonCylinder),
}

impl BoundPolygon {
    /// The vertex indices the polygon addresses, in order. Triangle indices are
    /// masked to 15 bits; the `0x8000` flag bit is not an index.
    pub fn vertex_indices(&self) -> Vec<u16> {
        match self {
            Self::Triangle(t) => t.vertex_indices.map(|v| v & 0x7FFF).to_vec(),
            Self::Sphere(s) => vec![s.sphere_index],
            Self::Capsule(c) => vec![c.capsule_index1, c.capsule_index2],
            Self::Box(b) => b.box_indices.map(|v| v as u16).to_vec(),
            Self::Cylinder(c) => vec![c.cylinder_index1, c.cylinder_index2],
        }
    }
}

/// `BoundPolygonTriangle`: an area, three vertex indices (with flag bits), and
/// three edge indices. Edge `0xFFFF` means "no neighbour".
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundPolygonTriangle {
    pub area: f32,
    pub vertex_indices: [u16; 3],
    pub edge_indices: [u16; 3],
}

/// `BoundPolygonSphere`: an index into the vertex list and a radius.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundPolygonSphere {
    pub sphere_index: u16,
    pub sphere_radius: f32,
}

/// `BoundPolygonCapsule`: two vertex indices and a radius.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundPolygonCapsule {
    pub capsule_index1: u16,
    pub capsule_radius: f32,
    pub capsule_index2: u16,
}

/// `BoundPolygonBox`: four vertex indices.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundPolygonBox {
    pub box_indices: [i16; 4],
}

/// `BoundPolygonCylinder`: two vertex indices and a radius.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundPolygonCylinder {
    pub cylinder_index1: u16,
    pub cylinder_radius: f32,
    pub cylinder_index2: u16,
}

/// `BoundMaterial_s`: two 32-bit words the material properties are packed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundMaterial {
    pub data1: u32,
    pub data2: u32,
}

impl BoundMaterial {
    /// The low byte of `data1`.
    pub fn material_type(&self) -> u8 {
        (self.data1 & 0xFF) as u8
    }

    pub fn flags(&self) -> u16 {
        (((self.data1 >> 24) & 0xFF) as u16) | ((self.data2 & 0xFF) as u16) << 8
    }

    pub fn material_color_index(&self) -> u8 {
        ((self.data2 >> 8) & 0xFF) as u8
    }
}

/// One colour as stored: four bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundMaterialColour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

fn colour(c: &[u8]) -> BoundMaterialColour {
    BoundMaterialColour {
        r: c[0],
        g: c[1],
        b: c[2],
        a: c[3],
    }
}

/// Decode `BoundVertex_s` records (three `i16`s each) and scale by the
/// quantum, which is `bounding box size / 32767`. Verbatim:
/// `Vertices[i] = bv.Vector * Quantum`.
fn decode_quantized_vertices(bytes: &[u8], quantum: [f32; 3]) -> Vec<[f32; 3]> {
    bytes
        .chunks_exact(6)
        .map(|c| {
            let s = |i: usize| i16::from_le_bytes([c[i * 2], c[i * 2 + 1]]);
            [
                s(0) as f32 * quantum[0],
                s(1) as f32 * quantum[1],
                s(2) as f32 * quantum[2],
            ]
        })
        .collect()
}

fn read_polygons(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: usize,
) -> Result<Vec<BoundPolygon>> {
    let Some(mut data) = r.read_bytes_at(pointer, count * 16)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let offset = i * 16;
        let first = data[offset];
        data[offset] = first & 0xF8;
        let kind = first & 7;
        let poly = match kind {
            0 => {
                let s = |i: usize| u16::from_le_bytes([data[offset + i * 2], data[offset + i * 2 + 1]]);
                let f = |i: usize| {
                    f32::from_le_bytes([
                        data[offset + i * 4],
                        data[offset + i * 4 + 1],
                        data[offset + i * 4 + 2],
                        data[offset + i * 4 + 3],
                    ])
                };
                BoundPolygon::Triangle(BoundPolygonTriangle {
                    area: f(0),
                    vertex_indices: [s(2), s(3), s(4)],
                    edge_indices: [s(5), s(6), s(7)],
                })
            }
            1 => {
                let s = |i: usize| u16::from_le_bytes([data[offset + i * 2], data[offset + i * 2 + 1]]);
                let f = |i: usize| {
                    f32::from_le_bytes([
                        data[offset + i * 4],
                        data[offset + i * 4 + 1],
                        data[offset + i * 4 + 2],
                        data[offset + i * 4 + 3],
                    ])
                };
                BoundPolygon::Sphere(BoundPolygonSphere {
                    sphere_index: s(1),
                    sphere_radius: f(1),
                })
            }
            2 => {
                let s = |i: usize| u16::from_le_bytes([data[offset + i * 2], data[offset + i * 2 + 1]]);
                let f = |i: usize| {
                    f32::from_le_bytes([
                        data[offset + i * 4],
                        data[offset + i * 4 + 1],
                        data[offset + i * 4 + 2],
                        data[offset + i * 4 + 3],
                    ])
                };
                BoundPolygon::Capsule(BoundPolygonCapsule {
                    capsule_index1: s(1),
                    capsule_radius: f(1),
                    capsule_index2: s(4),
                })
            }
            3 => {
                let s = |i: usize| {
                    i16::from_le_bytes([data[offset + i * 2], data[offset + i * 2 + 1]])
                };
                BoundPolygon::Box(BoundPolygonBox {
                    box_indices: [s(2), s(3), s(4), s(5)],
                })
            }
            4 => {
                let s = |i: usize| u16::from_le_bytes([data[offset + i * 2], data[offset + i * 2 + 1]]);
                let f = |i: usize| {
                    f32::from_le_bytes([
                        data[offset + i * 4],
                        data[offset + i * 4 + 1],
                        data[offset + i * 4 + 2],
                        data[offset + i * 4 + 3],
                    ])
                };
                BoundPolygon::Cylinder(BoundPolygonCylinder {
                    cylinder_index1: s(1),
                    cylinder_radius: f(1),
                    cylinder_index2: s(4),
                })
            }
            _ => continue,
        };
        out.push(poly);
    }
    Ok(out)
}

/// A `Matrix4F_s`: four columns of a `Vector3` plus a flag word each.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Matrix4F {
    /// `[x, y, z, flags]` per column.
    pub column1: [f32; 4],
    pub column2: [f32; 4],
    pub column3: [f32; 4],
    pub column4: [f32; 4],
}

impl Matrix4F {
    /// The four translation components of the matrix's columns.
    pub fn translation(&self) -> [f32; 3] {
        [self.column4[0], self.column4[1], self.column4[2]]
    }
}

/// An affine transform: a column-major 3x3 rotation/scale plus a translation.
/// Kept private; only triangle flattening uses it.
struct Affine {
    m: [[f32; 3]; 3],
    t: [f32; 3],
}

impl Affine {
    fn identity() -> Self {
        Self {
            m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            t: [0.0; 3],
        }
    }

    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.t;
        for (c, s) in self.m.iter().zip(p) {
            for (out, m) in out.iter_mut().zip(c) {
                *out += m * s;
            }
        }
        out
    }

    /// `then` applied first, then `self`.
    fn compose(&self, then: &Self) -> Self {
        let mut m = [[0.0f32; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, col) in row.iter_mut().enumerate() {
                *col = (0..3).map(|k| self.m[i][k] * then.m[k][j]).sum();
            }
        }
        let t = self.apply(then.t);
        Self { m, t }
    }
}

/// A triangle slice of the collision geometry, one part per geometry block.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollisionMesh {
    /// One part per geometry that yielded triangles, named by its tree path
    /// e.g. `child0` or `child2.1`.
    pub parts: Vec<CollisionPart>,
    pub vertex_count: usize,
    pub triangle_count: usize,
}

/// One geometry's triangles, ready to upload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollisionPart {
    /// The tree path from the bounds root.
    pub name: String,
    pub vertex_count: usize,
    pub triangle_count: usize,
    /// Tightly packed `f32` triples, in world space.
    pub positions: Vec<f32>,
    /// `u32` triples.
    pub indices: Vec<u32>,
}

impl Bounds {
    /// Flatten every triangle polygon in the tree into buffers a renderer can
    /// upload.
    ///
    /// Vertices are stored quantised and relative to their geometry's centre;
    /// each is placed in world space by adding `CenterGeom` and pushing it
    /// through the composite transform chain, the same way `GetVertexPos` does
    /// for one level. `None` when the tree holds no triangles — some `.ybn`
    /// are pure spheres and capsules.
    pub fn triangle_mesh(&self) -> Option<CollisionMesh> {
        let mut parts = Vec::new();
        self.walk_triangles(&Affine::identity(), "Bounds", &mut parts);
        if parts.is_empty() {
            return None;
        }
        let vertex_count = parts.iter().map(|p| p.vertex_count).sum();
        let triangle_count = parts.iter().map(|p| p.triangle_count).sum();
        Some(CollisionMesh {
            parts,
            vertex_count,
            triangle_count,
        })
    }

    fn walk_triangles(&self, transform: &Affine, path: &str, out: &mut Vec<CollisionPart>) {
        match &self.data {
            BoundsData::Geometry(g) => push_triangle_part(g, transform, path, out),
            BoundsData::Bvh(g) => push_triangle_part(&g.geometry, transform, path, out),
            BoundsData::Composite(c) => {
                for (i, child) in c.children.iter().enumerate() {
                    let matrix = matrix_for(c, i);
                    let child_transform = transform.compose(&matrix.affine());
                    child.walk_triangles(
                        &child_transform,
                        &format!("{path}.{i}"),
                        out,
                    );
                }
            }
            BoundsData::Primitive => {}
        }
    }
}

impl Matrix4F {
    /// The `Affine` this matrix represents: the first three columns are the
    /// basis, the fourth the translation; the flag word of each column is
    /// skipped.
    fn affine(&self) -> Affine {
        Affine {
            m: [
                [self.column1[0], self.column1[1], self.column1[2]],
                [self.column2[0], self.column2[1], self.column2[2]],
                [self.column3[0], self.column3[1], self.column3[2]],
            ],
            t: [self.column4[0], self.column4[1], self.column4[2]],
        }
    }
}

/// The transform of a composite child: the first matrix array when present,
/// else the second, else identity — the reference's
/// `ChildrenTransformation1 ?? ChildrenTransformation2`.
fn matrix_for(composite: &BoundComposite, index: usize) -> Matrix4F {
    let from = |list: &[Matrix4F]| list.get(index).copied();
    from(&composite.children_transformation1)
        .or_else(|| from(&composite.children_transformation2))
        .unwrap_or(Matrix4F {
            column1: [1.0, 0.0, 0.0, 0.0],
            column2: [0.0, 1.0, 0.0, 0.0],
            column3: [0.0, 0.0, 1.0, 0.0],
            column4: [0.0, 0.0, 0.0, 0.0],
        })
}

fn push_triangle_part(
    geometry: &BoundGeometry,
    transform: &Affine,
    path: &str,
    out: &mut Vec<CollisionPart>,
) {
    let mut positions: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for polygon in &geometry.polygons {
        let BoundPolygon::Triangle(t) = polygon else {
            continue;
        };
        let indices3 = t.vertex_indices.map(|v| (v & 0x7FFF) as usize);
        if indices3.iter().any(|i| *i >= geometry.vertices.len()) {
            continue;
        }
        let base = positions.len() / 3;
        for vertex in indices3 {
            let local = geometry.vertices[vertex];
            let world = transform.apply([
                local[0] + geometry.center_geom[0],
                local[1] + geometry.center_geom[1],
                local[2] + geometry.center_geom[2],
            ]);
            positions.extend_from_slice(&world);
        }
        indices.extend_from_slice(&[
            base as u32,
            (base + 1) as u32,
            (base + 2) as u32,
        ]);
    }

    if indices.is_empty() {
        return;
    }
    out.push(CollisionPart {
        name: path.to_string(),
        vertex_count: positions.len() / 3,
        triangle_count: indices.len() / 3,
        positions,
        indices,
    });
}

/// A `Matrix4F_s` array: each entry is four columns of `(Vector3, u32)`.
fn read_matrix_array(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: usize,
) -> Result<Vec<Matrix4F>> {
    let count = count.min(MAX_ELEMENTS);
    let Some(bytes) = r.read_bytes_at(pointer, count * 64)? else {
        return Ok(Vec::new());
    };
    Ok(bytes
        .chunks_exact(64)
        .map(|c| {
            let f = |i: usize| f32::from_le_bytes([c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]]);
            Matrix4F {
                column1: [f(0), f(1), f(2), f(3)],
                column2: [f(4), f(5), f(6), f(7)],
                column3: [f(8), f(9), f(10), f(11)],
                column4: [f(12), f(13), f(14), f(15)],
            }
        })
        .collect())
}

/// An `AABB_s` array: each entry is two `Vector4`s.
fn read_aabb_array(r: &mut ResourceReader<'_>, pointer: u64, count: usize) -> Result<Vec<[f32; 8]>> {
    let count = count.min(MAX_ELEMENTS);
    let Some(bytes) = r.read_bytes_at(pointer, count * 32)? else {
        return Ok(Vec::new());
    };
    Ok(bytes
        .chunks_exact(32)
        .map(|c| {
            let f = |i: usize| f32::from_le_bytes([c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]]);
            std::array::from_fn(f)
        })
        .collect())
}

/// A `BoundCompositeChildrenFlags` array: each entry is two `u32` flag words.
fn read_flag_array(r: &mut ResourceReader<'_>, pointer: u64, count: usize) -> Result<Vec<[u32; 2]>> {
    let count = count.min(MAX_ELEMENTS);
    let Some(bytes) = r.read_bytes_at(pointer, count * 8)? else {
        return Ok(Vec::new());
    };
    Ok(bytes
        .chunks_exact(8)
        .map(|c| [
            u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
        ])
        .collect())
}

fn read_vec4(r: &mut ResourceReader<'_>) -> Result<[f32; 4]> {
    Ok([r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nine byte values a bounds block can carry, per `BoundsType`.
    #[test]
    fn maps_the_type_bytes() {
        assert_eq!(BoundsType::from_u8(0), BoundsType::Sphere);
        assert_eq!(BoundsType::from_u8(1), BoundsType::Capsule);
        assert_eq!(BoundsType::from_u8(3), BoundsType::Box);
        assert_eq!(BoundsType::from_u8(4), BoundsType::Geometry);
        assert_eq!(BoundsType::from_u8(8), BoundsType::GeometryBvh);
        assert_eq!(BoundsType::from_u8(10), BoundsType::Composite);
        assert_eq!(BoundsType::from_u8(12), BoundsType::Disc);
        assert_eq!(BoundsType::from_u8(13), BoundsType::Cylinder);
        assert_eq!(BoundsType::from_u8(15), BoundsType::Cloth);
        assert_eq!(BoundsType::from_u8(255), BoundsType::None);
        assert_eq!(BoundsType::from_u8(9), BoundsType::None);
    }

    /// The room id is the low five bits, the ped density the high three.
    #[test]
    fn unpacks_room_and_ped_density() {
        let mut b = Bounds {
            base: ResourceFileBase {
                file_vft: 0,
                file_unknown: 0,
                file_pages_info_pointer: 0,
            },
            bounds_type: BoundsType::Box,
            sphere_radius: 0.0,
            box_max: [0.0; 3],
            margin: 0.0,
            box_min: [0.0; 3],
            unknown_3ch: 0,
            box_center: [0.0; 3],
            material_index: 0,
            procedural_id: 0,
            room_id_and_ped_density: 0,
            unk_flags: 0,
            sphere_center: [0.0; 3],
            poly_flags: 0,
            material_color_index: 0,
            unknown_60h: [0.0; 3],
            volume: 0.0,
            data: BoundsData::Primitive,
        };
        b.room_id_and_ped_density = 0b0110_0101;
        assert_eq!(b.room_id(), 5);
        assert_eq!(b.ped_density(), 3);
    }

    /// The five polygon records, decoded from their exact 16-byte layouts.
    ///
    /// The type rides in the low three bits of the first byte, which is masked
    /// off before the first field is read; the radius of the curved polygons is
    /// at byte 4, not byte 8.
    #[test]
    fn decodes_the_polygon_records() {
        let mut data = vec![0u8; 5 * 16];
        let put_u16 = |d: &mut Vec<u8>, at: usize, v: u16| {
            d[at..at + 2].copy_from_slice(&v.to_le_bytes());
        };
        let put_u32 = |d: &mut Vec<u8>, at: usize, v: u32| {
            d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };

        // Triangle: type 0, area, three vertex indices, three edge indices.
        let o = 0;
        put_u32(&mut data, o, 1.5f32.to_bits());
        put_u16(&mut data, o + 4, 0x8001);
        put_u16(&mut data, o + 6, 2);
        put_u16(&mut data, o + 8, 3);
        put_u16(&mut data, o + 10, 4);
        put_u16(&mut data, o + 12, 0xFFFF);
        put_u16(&mut data, o + 14, 5);

        // Sphere: type 1, index, radius.
        let o = 16;
        data[o] = 1;
        put_u16(&mut data, o + 2, 7);
        put_u32(&mut data, o + 4, 2.25f32.to_bits());

        // Capsule: type 2, index1, radius, index2.
        let o = 32;
        data[o] = 2;
        put_u16(&mut data, o + 2, 9);
        put_u32(&mut data, o + 4, 3.5f32.to_bits());
        put_u16(&mut data, o + 8, 11);

        // Box: type 3, four vertex indices.
        let o = 48;
        data[o] = 3;
        put_u16(&mut data, o + 4, 12i16 as u16);
        put_u16(&mut data, o + 6, 13);
        put_u16(&mut data, o + 8, 14);
        put_u16(&mut data, o + 10, 15);

        // Cylinder: type 4, index1, radius, index2.
        let o = 64;
        data[o] = 4;
        put_u16(&mut data, o + 2, 16);
        put_u32(&mut data, o + 4, 4.75f32.to_bits());
        put_u16(&mut data, o + 8, 18);

        let mut r = ResourceReader::new(&data, data.len());
        let polys = read_polygons(&mut r, super::super::reader::SYSTEM_BASE, 5).unwrap();

        assert_eq!(polys.len(), 5);
        match &polys[0] {
            BoundPolygon::Triangle(t) => {
                assert_eq!(t.area, 1.5);
                // The 0x8000 flag bit survives in the raw index.
                assert_eq!(t.vertex_indices, [0x8001, 2, 3]);
                assert_eq!(t.edge_indices, [4, 0xFFFF, 5]);
            }
            other => panic!("expected a triangle, got {other:?}"),
        }
        match &polys[1] {
            BoundPolygon::Sphere(s) => {
                assert_eq!(s.sphere_index, 7);
                assert_eq!(s.sphere_radius, 2.25);
            }
            other => panic!("expected a sphere, got {other:?}"),
        }
        match &polys[2] {
            BoundPolygon::Capsule(c) => {
                assert_eq!(c.capsule_index1, 9);
                assert_eq!(c.capsule_radius, 3.5);
                assert_eq!(c.capsule_index2, 11);
            }
            other => panic!("expected a capsule, got {other:?}"),
        }
        match &polys[3] {
            BoundPolygon::Box(b) => {
                assert_eq!(b.box_indices, [12, 13, 14, 15]);
            }
            other => panic!("expected a box, got {other:?}"),
        }
        match &polys[4] {
            BoundPolygon::Cylinder(c) => {
                assert_eq!(c.cylinder_index1, 16);
                assert_eq!(c.cylinder_radius, 4.75);
                assert_eq!(c.cylinder_index2, 18);
            }
            other => panic!("expected a cylinder, got {other:?}"),
        }
    }

    /// The flatten places each triangle in world space: vertex + center geom,
    /// pushed through the composite transform chain.
    #[test]
    fn triangle_mesh_places_vertices_in_world_space() {
        let geometry = BoundGeometry {
            vertices_shrunk: vec![],
            polygons: vec![BoundPolygon::Triangle(BoundPolygonTriangle {
                area: 0.0,
                vertex_indices: [0, 1, 2],
                edge_indices: [0xFFFF; 3],
            })],
            vertices: vec![
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
            center_geom: [100.0, 0.0, 0.0],
            quantum: [1.0, 1.0, 1.0],
            vertex_colours: vec![],
            octants: None,
            materials: vec![],
            material_colours: vec![],
            polygon_material_indices: vec![],
            vertices_count: 3,
            polygons_count: 1,
        };

        // A child matrix translating by (0, 0, 5) in the Z axis.
        let child_transform = Matrix4F {
            column1: [1.0, 0.0, 0.0, 0.0],
            column2: [0.0, 1.0, 0.0, 0.0],
            column3: [0.0, 0.0, 1.0, 0.0],
            column4: [0.0, 0.0, 5.0, 0.0],
        };

        let bounds = Bounds {
            base: ResourceFileBase {
                file_vft: 0,
                file_unknown: 0,
                file_pages_info_pointer: 0,
            },
            bounds_type: BoundsType::Composite,
            sphere_radius: 0.0,
            box_max: [0.0; 3],
            margin: 0.0,
            box_min: [0.0; 3],
            unknown_3ch: 0,
            box_center: [0.0; 3],
            material_index: 0,
            procedural_id: 0,
            room_id_and_ped_density: 0,
            unk_flags: 0,
            sphere_center: [0.0; 3],
            poly_flags: 0,
            material_color_index: 0,
            unknown_60h: [0.0; 3],
            volume: 0.0,
            data: BoundsData::Composite(BoundComposite {
                children: vec![Bounds {
                    base: ResourceFileBase {
                        file_vft: 0,
                        file_unknown: 0,
                        file_pages_info_pointer: 0,
                    },
                    bounds_type: BoundsType::Geometry,
                    sphere_radius: 0.0,
                    box_max: [0.0; 3],
                    margin: 0.0,
                    box_min: [0.0; 3],
                    unknown_3ch: 0,
                    box_center: [0.0; 3],
                    material_index: 0,
                    procedural_id: 0,
                    room_id_and_ped_density: 0,
                    unk_flags: 0,
                    sphere_center: [0.0; 3],
                    poly_flags: 0,
                    material_color_index: 0,
                    unknown_60h: [0.0; 3],
                    volume: 0.0,
                    data: BoundsData::Geometry(geometry),
                }],
                children_transformation1: vec![child_transform],
                children_transformation2: vec![],
                children_bounding_boxes: vec![],
                children_flags1: vec![],
                children_flags2: vec![],
                bvh: None,
            }),
        };

        let mesh = bounds.triangle_mesh().expect("a mesh");
        assert_eq!(mesh.parts.len(), 1);
        let part = &mesh.parts[0];
        assert_eq!(part.vertex_count, 3);
        assert_eq!(part.triangle_count, 1);
        assert_eq!(part.name, "Bounds.0");
        // vertex 0 = (1,0,0) + center(100,0,0) + translation(0,0,5).
        assert_eq!(part.positions[..3], [101.0, 0.0, 5.0]);
        assert_eq!(part.positions[3..6], [100.0, 1.0, 5.0]);
        assert_eq!(part.positions[6..9], [100.0, 0.0, 5.0]);
        assert_eq!(part.indices, [0, 1, 2]);
    }
}
