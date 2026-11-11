//! Flattening a parsed [`Drawable`] into buffers a renderer can upload.
//!
//! Not a port of anything: the reference builds DirectX buffers directly in
//! `CodeWalker/Rendering`. This is the equivalent step for a WebView frontend,
//! and it exists in `cw-core` rather than in `src-tauri` so the conversion stays
//! testable without an app around it.
//!
//! Two decisions shape it:
//!
//! - **The vertex declaration is resolved here, once.** A geometry's bytes mean
//!   nothing without it, and the rule from `docs/architecture.md` is that the
//!   frontend never parses archive data. So positions, normals and texcoords
//!   come out as plain `f32` arrays and the frontend only uploads them.
//! - **Buffers are kept as bytes, and how they cross is the caller's choice.**
//!   A [`Buffer`] serializes as base64, which is what the Explorer's commands
//!   send and is still far better than JSON number arrays: a 60k-vertex
//!   drawable is about 2 MB of digits versus 700 KB of base64. The world view's
//!   streaming path takes [`Buffer::as_bytes`] instead and puts it in a wire
//!   envelope section, where it costs nothing at all. Same buffers, two
//!   transports, one flattening.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Serialize, Serializer};

use crate::resources::drawable::{
    Drawable, DrawableGeometry, Lod, BUMP_SAMPLER, DIFFUSE_SAMPLER, LOOKUP_SAMPLER, SPEC_SAMPLER,
    TERRAIN_LAYER_SAMPLERS,
};
use crate::resources::vertex::VertexSemantic;

/// A vertex or index buffer: tightly packed, little-endian, ready to upload.
///
/// Serializes as base64 so the JSON commands are unchanged, and hands out its
/// bytes for the envelope so that path never encodes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer(Vec<u8>);

impl Buffer {
    pub fn from_f32(values: &[f32]) -> Self {
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Self(bytes)
    }

    pub fn from_u32(values: &[u32]) -> Self {
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for Buffer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

/// One drawable, ready to render.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshBundle {
    /// The drawable's own name, when it carries one.
    pub name: Option<String>,
    /// Which LOD the parts came from.
    pub lod: &'static str,
    /// Every LOD this drawable has models for, highest first.
    pub available_lods: Vec<&'static str>,
    pub bounding_box_min: [f32; 3],
    pub bounding_box_max: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub vertex_count: usize,
    pub triangle_count: usize,
    pub parts: Vec<MeshPart>,
}

/// One draw call: a position buffer plus whatever else the declaration carried.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshPart {
    /// `model{i}.geometry{j}`, so a part in the viewer maps back to the file.
    pub name: String,
    /// Jenkins hash of the shader name. Resolving it to text needs the name
    /// tables from `MetaNames.cs`, which are not ported yet.
    pub shader: u32,
    /// Jenkins hash of the shader's `.sps` file name (`trees_normal_spec.sps`).
    /// `shader` is the base name and several files share it (`normal` is also
    /// `normal_cutout`, `default` is also `cutout`), so what a part is drawn
    /// with, an alpha test or no draw at all, is decided on this one.
    pub shader_file: u32,
    /// The shader's render bucket: 0 solid, 1 alpha, 2 decal, 3 cutout in the
    /// stock files (see `docs/formats.md`).
    pub render_bucket: u8,
    /// Jenkins hash of the texture that fills this geometry's diffuse slot, if
    /// its shader binds one. The viewer resolves it against the drawable's
    /// embedded dictionary or the model's sibling `.ytd`.
    pub diffuse: Option<u32>,
    /// Hash of the texture in the normal (bump) slot, when bound.
    pub normal: Option<u32>,
    /// Hash of the texture in the specular slot, when bound.
    pub specular: Option<u32>,
    /// The four ground layers of a `terrain_cb_w_4lyr` shader, by hash, in
    /// `TextureSampler_layer0` to `3` order. `None` for every other shader; when
    /// set, [`MeshPart::colours`] and [`MeshPart::uvs1`] carry what blends them.
    pub layers: Option<[Option<u32>; 4]>,
    /// The blend mask of the `_cm` terrain variants (`lookupSampler`), sampled
    /// with the second texcoord.
    pub mask: Option<u32>,
    pub vertex_count: usize,
    pub triangle_count: usize,
    /// Tightly packed `f32` triples.
    pub positions: Buffer,
    /// `f32` triples, absent when the declaration has no normal.
    pub normals: Option<Buffer>,
    /// `f32` pairs, absent when the declaration has no texcoord0.
    pub uvs: Option<Buffer>,
    /// `f32` pairs of texcoord1, only for a terrain blend that has one.
    pub uvs1: Option<Buffer>,
    /// Terrain blend weights, only for a [`MeshPart::layers`] part: eight bytes
    /// a vertex, `COLOUR0` as RGBA then `COLOUR1` as RGBA, each byte the
    /// reference's `Colour` over 255. A colour the declaration lacks is
    /// `[255; 4]`, which the reference also reads as no blend.
    pub colours: Option<Buffer>,
    /// `u32` triples.
    pub indices: Buffer,
}

/// The texture hashes a shader binds for one geometry.
#[derive(Debug, Clone, Copy, Default)]
struct PartTextures {
    diffuse: Option<u32>,
    normal: Option<u32>,
    specular: Option<u32>,
    layers: Option<[Option<u32>; 4]>,
    mask: Option<u32>,
}

impl MeshBundle {
    /// Flatten one LOD of a drawable. `None` when that LOD has no geometry that
    /// yields triangles.
    pub fn from_drawable(drawable: &Drawable, lod: Lod) -> Option<Self> {
        let models = &drawable.models[Lod::ALL.iter().position(|l| *l == lod)?];

        let mut parts = Vec::new();
        for (m, model) in models.iter().enumerate() {
            for (g, geometry) in model.geometries.iter().enumerate() {
                let shader_fx = drawable.shaders.get(geometry.shader_id as usize);
                let shader = shader_fx.map(|s| s.name).unwrap_or(0);
                let textures = shader_fx
                    .map(|s| {
                        let layers = TERRAIN_LAYER_SAMPLERS.map(|param| s.texture_for(param));
                        PartTextures {
                            diffuse: s.texture_for(DIFFUSE_SAMPLER),
                            normal: s.texture_for(BUMP_SAMPLER),
                            specular: s.texture_for(SPEC_SAMPLER),
                            layers: layers.iter().any(Option::is_some).then_some(layers),
                            mask: s.texture_for(LOOKUP_SAMPLER),
                        }
                    })
                    .unwrap_or_default();
                if let Some(mut part) = MeshPart::from_geometry(geometry, shader, textures, m, g) {
                    if let Some(fx) = shader_fx {
                        part.shader_file = fx.file_name;
                        part.render_bucket = fx.render_bucket;
                    }
                    parts.push(part);
                }
            }
        }
        if parts.is_empty() {
            return None;
        }

        let available_lods = Lod::ALL
            .iter()
            .zip(&drawable.models)
            .filter(|(_, models)| !models.is_empty())
            .map(|(lod, _)| lod.as_str())
            .collect();

        Some(Self {
            name: drawable.name.clone(),
            lod: lod.as_str(),
            available_lods,
            bounding_box_min: drawable.bounding_box_min,
            bounding_box_max: drawable.bounding_box_max,
            bounding_sphere_radius: drawable.bounding_sphere_radius,
            vertex_count: parts.iter().map(|p| p.vertex_count).sum(),
            triangle_count: parts.iter().map(|p| p.triangle_count).sum(),
            parts,
        })
    }

    /// The highest LOD that yields anything.
    pub fn best(drawable: &Drawable) -> Option<Self> {
        Lod::ALL
            .iter()
            .find_map(|lod| Self::from_drawable(drawable, *lod))
    }

    /// Every LOD that yields geometry, highest first. For the LOD picker: each
    /// bundle is its own `from_drawable` call, so a part a lower LOD drops is
    /// simply absent rather than left as a stale mesh.
    pub fn all(drawable: &Drawable) -> Vec<Self> {
        Lod::ALL
            .iter()
            .filter_map(|lod| Self::from_drawable(drawable, *lod))
            .collect()
    }
}

impl MeshPart {
    /// Every texture hash this part draws with: the three slots, the terrain
    /// layers and the blend mask.
    pub fn texture_hashes(&self) -> impl Iterator<Item = u32> + '_ {
        [self.diffuse, self.normal, self.specular, self.mask]
            .into_iter()
            .chain(self.layers.into_iter().flatten())
            .flatten()
    }

    fn from_geometry(
        geometry: &DrawableGeometry,
        shader: u32,
        textures: PartTextures,
        model_index: usize,
        geometry_index: usize,
    ) -> Option<Self> {
        if !geometry.is_renderable() {
            return None;
        }
        let declaration = geometry.declaration?;

        let count = geometry
            .vertex_count
            .min(geometry.vertex_bytes.len() / geometry.vertex_stride.max(1));
        if count == 0 {
            return None;
        }

        let has_normal = declaration.has(VertexSemantic::NORMAL);
        let has_uv = declaration.has(VertexSemantic::TEXCOORD0);

        let mut positions = Vec::with_capacity(count * 3);
        let mut normals = has_normal.then(|| Vec::with_capacity(count * 3));
        let mut uvs = has_uv.then(|| Vec::with_capacity(count * 2));
        // A terrain blend also needs the vertex colours and the second texcoord.
        let blend = textures.layers.is_some();
        let mut colours = blend.then(|| Vec::with_capacity(count * 8));
        let mut uvs1 = (blend && declaration.has(VertexSemantic::TEXCOORD1))
            .then(|| Vec::with_capacity(count * 2));

        for i in 0..count {
            let Some(vertex) = geometry.vertex(i) else {
                break;
            };
            let position = declaration
                .decode(vertex, VertexSemantic::POSITION)
                .unwrap_or([0.0; 4]);
            positions.extend_from_slice(&position[..3]);

            if let Some(normals) = normals.as_mut() {
                let n = declaration
                    .decode(vertex, VertexSemantic::NORMAL)
                    .unwrap_or([0.0, 0.0, 1.0, 0.0]);
                normals.extend_from_slice(&n[..3]);
            }
            if let Some(uvs) = uvs.as_mut() {
                let uv = declaration
                    .decode(vertex, VertexSemantic::TEXCOORD0)
                    .unwrap_or([0.0; 4]);
                uvs.extend_from_slice(&uv[..2]);
            }
            if let Some(colours) = colours.as_mut() {
                for semantic in [VertexSemantic::COLOUR0, VertexSemantic::COLOUR1] {
                    let c = declaration
                        .decode(vertex, semantic)
                        .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                    colours.extend(c.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
                }
            }
            if let Some(uvs1) = uvs1.as_mut() {
                let uv = declaration
                    .decode(vertex, VertexSemantic::TEXCOORD1)
                    .unwrap_or([0.0; 4]);
                uvs1.extend_from_slice(&uv[..2]);
            }
        }

        let vertex_count = positions.len() / 3;

        // An index past the end of the vertex buffer would draw garbage or
        // crash the renderer, so the whole triangle is dropped. Stock files
        // never hit this; files that have been edited by hand do.
        let mut indices: Vec<u32> = Vec::with_capacity(geometry.indices.len());
        for triangle in geometry.indices.chunks_exact(3) {
            if triangle.iter().any(|i| *i as usize >= vertex_count) {
                continue;
            }
            indices.extend(triangle.iter().map(|i| *i as u32));
        }
        if indices.is_empty() {
            return None;
        }

        Some(Self {
            name: format!("model{model_index}.geometry{geometry_index}"),
            shader,
            shader_file: 0,
            render_bucket: 0,
            diffuse: textures.diffuse,
            normal: textures.normal,
            specular: textures.specular,
            layers: textures.layers,
            mask: textures.mask,
            vertex_count,
            triangle_count: indices.len() / 3,
            positions: Buffer::from_f32(&positions),
            normals: normals.as_deref().map(Buffer::from_f32),
            uvs: uvs.as_deref().map(Buffer::from_f32),
            uvs1: uvs1.as_deref().map(Buffer::from_f32),
            colours: colours.map(Buffer),
            indices: Buffer::from_u32(&indices),
        })
    }

    /// The index buffer as the narrowest integer that still addresses every
    /// vertex, and how wide that is in bits.
    ///
    /// `geometry.indices` is already `Vec<u16>` in the parsed drawable and gets
    /// widened to `u32` on the way out, which doubles roughly a quarter of a
    /// part's payload for nothing. A part with 65,536 vertices or more genuinely
    /// needs the width, so the caller records what it got rather than assuming.
    pub fn narrow_indices(&self) -> (Vec<u8>, u8) {
        if self.vertex_count > u16::MAX as usize {
            return (self.indices.as_bytes().to_vec(), 32);
        }
        let mut out = Vec::with_capacity(self.indices.len() / 2);
        for index in self.indices.as_bytes().chunks_exact(4) {
            // Safe to truncate: every index is below `vertex_count`, which this
            // branch has already bounded by `u16::MAX`.
            out.extend_from_slice(&[index[0], index[1]]);
        }
        (out, 16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::vertex::VertexDeclaration;

    fn triangle_geometry() -> DrawableGeometry {
        let declaration = VertexDeclaration {
            flags: 0b0100_1001, // position, normal, texcoord0
            stride: 32,
            unknown_6h: 0,
            count: 3,
            types: 0x7755_5555_5599_6996,
        };

        let mut bytes = Vec::new();
        for (position, uv) in [
            ([0.0f32, 0.0, 0.0], [0.0f32, 0.0]),
            ([1.0, 0.0, 0.0], [1.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 1.0]),
        ] {
            for c in position {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
            for c in [0.0f32, 0.0, 1.0] {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
            for c in uv {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
        }

        DrawableGeometry {
            shader_id: 0,
            bounds: None,
            vertex_count: 3,
            vertex_stride: 32,
            declaration: Some(declaration),
            vertex_bytes: bytes,
            indices: vec![0, 1, 2],
        }
    }

    fn decode_f32(buffer: &Buffer) -> Vec<f32> {
        buffer
            .as_bytes()
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn packs_a_geometry_into_buffers() {
        let part =
            MeshPart::from_geometry(&triangle_geometry(), 0x1234, PartTextures::default(), 0, 1)
                .unwrap();

        assert_eq!(part.name, "model0.geometry1");
        assert_eq!(part.shader, 0x1234);
        assert_eq!(part.vertex_count, 3);
        assert_eq!(part.triangle_count, 1);

        assert_eq!(
            decode_f32(&part.positions),
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        );
        assert_eq!(
            decode_f32(part.normals.as_ref().unwrap()),
            vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(
            decode_f32(part.uvs.as_ref().unwrap()),
            vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
        );

        let indices: Vec<u32> = part
            .indices
            .as_bytes()
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    /// A `PNCCTT` vertex (position, normal, colour0, colour1, texcoord0,
    /// texcoord1): the layout of the `terrain_cb_w_4lyr_2tex` family.
    fn terrain_geometry() -> DrawableGeometry {
        let declaration = VertexDeclaration {
            flags: 0b1111_1001, // position, normal, colour0, colour1, texcoord0, texcoord1
            stride: 48,
            unknown_6h: 0,
            count: 6,
            types: 0x5599_6006,
        };
        let mut bytes = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in p {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
            for c in [0.0f32, 0.0, 1.0] {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
            bytes.extend_from_slice(&[255, 128, 0, 51]); // colour0
            bytes.extend_from_slice(&[0, 255, 64, 255]); // colour1
            for c in [p[0] * 4.0, p[1] * 4.0, 0.25, 0.75] {
                bytes.extend_from_slice(&c.to_le_bytes()); // texcoord0, texcoord1
            }
        }
        DrawableGeometry {
            shader_id: 0,
            bounds: None,
            vertex_count: 3,
            vertex_stride: 48,
            declaration: Some(declaration),
            vertex_bytes: bytes,
            indices: vec![0, 1, 2],
        }
    }

    /// A part with ground layers carries the blend inputs: both vertex colours
    /// as bytes and texcoord1. A part without them does not pay for them.
    #[test]
    fn a_terrain_blend_carries_colours_and_the_second_texcoord() {
        let layers = Some([Some(1), Some(2), None, Some(4)]);
        let textures = PartTextures {
            layers,
            mask: Some(9),
            ..PartTextures::default()
        };
        let part = MeshPart::from_geometry(&terrain_geometry(), 0, textures, 0, 0).unwrap();

        assert_eq!(part.layers, layers);
        assert_eq!(part.texture_hashes().collect::<Vec<_>>(), vec![9, 1, 2, 4]);
        let colours = part.colours.as_ref().unwrap().as_bytes();
        assert_eq!(colours.len(), 3 * 8);
        assert_eq!(&colours[..8], &[255, 128, 0, 51, 0, 255, 64, 255]);
        assert_eq!(
            decode_f32(part.uvs1.as_ref().unwrap()),
            vec![0.25, 0.75, 0.25, 0.75, 0.25, 0.75]
        );

        let plain =
            MeshPart::from_geometry(&terrain_geometry(), 0, PartTextures::default(), 0, 0).unwrap();
        assert!(plain.colours.is_none() && plain.uvs1.is_none() && plain.layers.is_none());
    }

    /// A terrain blend whose vertices have no colour1 reads it as opaque white,
    /// which is what leaves the blend to the mask texture.
    #[test]
    fn a_missing_colour_reads_as_white() {
        let mut geometry = terrain_geometry();
        geometry.declaration.as_mut().unwrap().flags &= !0b10_0000; // no colour1
                                                                    // Re-pack without colour1 so the bytes match the declaration.
        let mut bytes = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in p.iter().chain([0.0f32, 0.0, 1.0].iter()) {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
            bytes.extend_from_slice(&[10, 20, 30, 40]);
            for c in [0.0f32, 0.0, 0.0, 0.0] {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
        }
        geometry.vertex_bytes = bytes;
        geometry.vertex_stride = 44;
        let textures = PartTextures {
            layers: Some([Some(1), None, None, None]),
            ..PartTextures::default()
        };
        let part = MeshPart::from_geometry(&geometry, 0, textures, 0, 0).unwrap();
        let colours = part.colours.as_ref().unwrap().as_bytes();
        assert_eq!(&colours[..8], &[10, 20, 30, 40, 255, 255, 255, 255]);
    }

    /// A hand-edited file can name a vertex that is not there. Dropping the
    /// triangle beats handing the renderer an out-of-range index.
    #[test]
    fn drops_triangles_that_index_past_the_vertex_buffer() {
        let mut geometry = triangle_geometry();
        geometry.indices = vec![0, 1, 2, 0, 1, 99];
        let part = MeshPart::from_geometry(&geometry, 0, PartTextures::default(), 0, 0).unwrap();
        assert_eq!(part.triangle_count, 1);
    }

    #[test]
    fn skips_a_geometry_with_no_usable_triangles() {
        let mut geometry = triangle_geometry();
        geometry.indices = vec![0, 1, 500];
        assert!(MeshPart::from_geometry(&geometry, 0, PartTextures::default(), 0, 0).is_none());
    }

    /// A geometry whose declaration has no position is not renderable at all.
    #[test]
    fn skips_a_geometry_with_no_position() {
        let mut geometry = triangle_geometry();
        if let Some(declaration) = geometry.declaration.as_mut() {
            declaration.flags &= !1;
        }
        assert!(MeshPart::from_geometry(&geometry, 0, PartTextures::default(), 0, 0).is_none());
    }

    /// The vertex count in the header is trusted only as far as the bytes go.
    #[test]
    fn clamps_the_vertex_count_to_the_buffer() {
        let mut geometry = triangle_geometry();
        geometry.vertex_count = 500;
        let part = MeshPart::from_geometry(&geometry, 0, PartTextures::default(), 0, 0).unwrap();
        assert_eq!(part.vertex_count, 3);
    }
}
