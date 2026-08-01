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
//! - **Buffers cross as base64, not as JSON numbers.** A 60k-vertex drawable is
//!   about 2 MB of JSON digits versus 700 KB of base64 that decodes straight
//!   into a `Float32Array`. The channel is the same; the parse on the other side
//!   is not.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Serialize;

use crate::resources::drawable::{Drawable, DrawableGeometry, Lod};
use crate::resources::vertex::VertexSemantic;

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
    pub vertex_count: usize,
    pub triangle_count: usize,
    /// Base64 of tightly packed `f32` triples.
    pub positions: String,
    /// Base64 of `f32` triples, absent when the declaration has no normal.
    pub normals: Option<String>,
    /// Base64 of `f32` pairs, absent when the declaration has no texcoord0.
    pub uvs: Option<String>,
    /// Base64 of `u32` triples.
    pub indices: String,
}

impl MeshBundle {
    /// Flatten one LOD of a drawable. `None` when that LOD has no geometry that
    /// yields triangles.
    pub fn from_drawable(drawable: &Drawable, lod: Lod) -> Option<Self> {
        let models = &drawable.models[Lod::ALL.iter().position(|l| *l == lod)?];

        let mut parts = Vec::new();
        for (m, model) in models.iter().enumerate() {
            for (g, geometry) in model.geometries.iter().enumerate() {
                let shader = drawable
                    .shaders
                    .get(geometry.shader_id as usize)
                    .map(|s| s.name)
                    .unwrap_or(0);
                if let Some(part) = MeshPart::from_geometry(geometry, shader, m, g) {
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
}

impl MeshPart {
    fn from_geometry(
        geometry: &DrawableGeometry,
        shader: u32,
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
            vertex_count,
            triangle_count: indices.len() / 3,
            positions: encode_f32(&positions),
            normals: normals.as_deref().map(encode_f32),
            uvs: uvs.as_deref().map(encode_f32),
            indices: encode_u32(&indices),
        })
    }
}

fn encode_f32(values: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    STANDARD.encode(bytes)
}

fn encode_u32(values: &[u32]) -> String {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    STANDARD.encode(bytes)
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

    fn decode_f32(encoded: &str) -> Vec<f32> {
        STANDARD
            .decode(encoded)
            .unwrap()
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn packs_a_geometry_into_buffers() {
        let part = MeshPart::from_geometry(&triangle_geometry(), 0x1234, 0, 1).unwrap();

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

        let indices: Vec<u32> = STANDARD
            .decode(&part.indices)
            .unwrap()
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    /// A hand-edited file can name a vertex that is not there. Dropping the
    /// triangle beats handing the renderer an out-of-range index.
    #[test]
    fn drops_triangles_that_index_past_the_vertex_buffer() {
        let mut geometry = triangle_geometry();
        geometry.indices = vec![0, 1, 2, 0, 1, 99];
        let part = MeshPart::from_geometry(&geometry, 0, 0, 0).unwrap();
        assert_eq!(part.triangle_count, 1);
    }

    #[test]
    fn skips_a_geometry_with_no_usable_triangles() {
        let mut geometry = triangle_geometry();
        geometry.indices = vec![0, 1, 500];
        assert!(MeshPart::from_geometry(&geometry, 0, 0, 0).is_none());
    }

    /// A geometry whose declaration has no position is not renderable at all.
    #[test]
    fn skips_a_geometry_with_no_position() {
        let mut geometry = triangle_geometry();
        if let Some(declaration) = geometry.declaration.as_mut() {
            declaration.flags &= !1;
        }
        assert!(MeshPart::from_geometry(&geometry, 0, 0, 0).is_none());
    }

    /// The vertex count in the header is trusted only as far as the bytes go.
    #[test]
    fn clamps_the_vertex_count_to_the_buffer() {
        let mut geometry = triangle_geometry();
        geometry.vertex_count = 500;
        let part = MeshPart::from_geometry(&geometry, 0, 0, 0).unwrap();
        assert_eq!(part.vertex_count, 3);
    }
}
