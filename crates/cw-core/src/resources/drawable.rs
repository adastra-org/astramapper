//! Drawables: the geometry half of `Drawable.cs`.
//!
//! Port of `DrawableBase` / `Drawable` / `DrawableDictionary` / `ShaderGroup` /
//! `ShaderFX` / `DrawableModelsBlock` / `DrawableModel` / `DrawableGeometry` /
//! `VertexBuffer` / `IndexBuffer` from
//! `CodeWalker.Core/GameFiles/Resources/Drawable.cs`.
//!
//! Scope: what a viewer needs to draw the thing — the LOD model lists, their
//! geometries, and each geometry's vertex and index buffers. Deliberately not
//! ported here: shader *parameters*, the embedded texture dictionary, the
//! skeleton, joints, light attributes and collision bounds. Each is a block of
//! its own and each is read past rather than into, so the pointers are kept and
//! the sub-blocks stay `TODO(port)`.
//!
//! Two things about the layout that are not guessable from the struct listing:
//!
//! 1. **`DrawableModelsBlock` is not a block.** It has no header of its own. The
//!    reference reads it at whichever pointer is non-zero and then ignores that
//!    position entirely, re-seeking to each of the four LOD pointers held by the
//!    *owner*. So the four lists are addressed independently and only look
//!    nested.
//! 2. **A pointer list is `{pointer, count, capacity, pad}` and the pointers are
//!    read at `capacity`, not `count`.** The reference does this for all four
//!    lists; the two agree in every stock file.

use serde::Serialize;

use crate::Result;

use super::reader::ResourceReader;
use super::texture::{Texture, TextureDictionary};
use super::vertex::VertexDeclaration;

/// Jenkins hashes of the shader parameters that name the important textures, so
/// a material can be matched to the texture that fills it. From the
/// `ShaderParamNames` enum in `ShaderParams.cs`.
pub const DIFFUSE_SAMPLER: u32 = 4059966321;
pub const BUMP_SAMPLER: u32 = 1186448975;
pub const SPEC_SAMPLER: u32 = 1619499462;
pub const DIFFUSE_HF_SAMPLER: u32 = 2946270081;
pub const TINT_PALETTE_SAMPLER: u32 = 4131954791;
pub const DETAIL_SAMPLER: u32 = 3393362404;
/// The four ground layers of the `terrain_cb_w_4lyr` family, in the order the
/// reference's `TerrainShader` binds them (`TextureSampler_layer0` to `3`), and
/// the mask the `_cm` variants blend by (`lookupSampler`).
pub const TERRAIN_LAYER_SAMPLERS: [u32; 4] = [3576369631, 606121937, 831736502, 2025281789];
pub const LOOKUP_SAMPLER: u32 = 2295086480;

/// Guard on any count read out of a file before it is used to size an
/// allocation. Nothing in a stock drawable comes close; a corrupt or hostile
/// file can name any 32-bit number.
const MAX_ELEMENTS: usize = 16_000_000;

/// The 16-byte `ResourceFileBase` every resource starts with.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ResourceFileBase {
    pub file_vft: u32,
    pub file_unknown: u32,
    pub file_pages_info_pointer: u64,
}

impl ResourceFileBase {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        Ok(Self {
            file_vft: r.read_u32()?,
            file_unknown: r.read_u32()?,
            file_pages_info_pointer: r.read_u64()?,
        })
    }
}

/// One level of detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Lod {
    High,
    Medium,
    Low,
    VeryLow,
}

impl Lod {
    pub const ALL: [Lod; 4] = [Lod::High, Lod::Medium, Lod::Low, Lod::VeryLow];

    pub fn as_str(self) -> &'static str {
        match self {
            Lod::High => "high",
            Lod::Medium => "medium",
            Lod::Low => "low",
            Lod::VeryLow => "veryLow",
        }
    }
}

/// A drawable: a bounding volume, a shader group and up to four LOD model lists.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drawable {
    pub base: ResourceFileBase,
    pub name: Option<String>,
    pub bounding_center: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub bounding_box_min: [f32; 3],
    pub bounding_box_max: [f32; 3],
    pub lod_distances: [f32; 4],
    /// Models per LOD, in [`Lod::ALL`] order.
    pub models: [Vec<DrawableModel>; 4],
    pub shaders: Vec<ShaderFx>,
    /// The textures the shaders reference, embedded in the drawable. Absent
    /// when the shader group carries none — most models name their textures
    /// and the dictionary lives in a separate `.ytd`.
    pub texture_dictionary: Option<TextureDictionary>,
    /// Kept, not followed. See the module note.
    pub skeleton_pointer: u64,
    pub joints_pointer: u64,
    pub bound_pointer: u64,
}

impl Drawable {
    /// Read a `Drawable` at the cursor.
    ///
    /// `with_name` distinguishes the two shapes the reference has: `Drawable`
    /// (208 bytes, carries a name, light attributes and bounds) and the plain
    /// `DrawableBase` (168) that fragments embed. Everything up to offset 0xA8
    /// is identical.
    pub fn read(r: &mut ResourceReader<'_>, with_name: bool) -> Result<Self> {
        let base = ResourceFileBase::read(r)?;

        let shader_group_pointer = r.read_u64()?;
        let skeleton_pointer = r.read_u64()?;
        let bounding_center = r.read_vec3()?;
        let bounding_sphere_radius = r.read_f32()?;
        let bounding_box_min = r.read_vec3()?;
        let _unknown_3ch = r.read_u32()?;
        let bounding_box_max = r.read_vec3()?;
        let _unknown_4ch = r.read_u32()?;

        let lod_pointers = [
            r.read_u64()?, // high
            r.read_u64()?, // medium
            r.read_u64()?, // low
            r.read_u64()?, // very low
        ];
        let lod_distances = [r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?];
        let _render_mask_flags = [r.read_u32()?, r.read_u32()?, r.read_u32()?, r.read_u32()?];
        let joints_pointer = r.read_u64()?;
        let _unknown_98h = r.read_u16()?;
        let _models_blocks_size = r.read_u16()?;
        let _unknown_9ch = r.read_u32()?;
        let _models_pointer = r.read_u64()?;

        let (name_pointer, bound_pointer) = if with_name {
            let name_pointer = r.read_u64()?;
            // ResourceSimpleList64<LightAttributes>, 16 bytes, read past.
            let _light_attributes_pointer = r.read_u64()?;
            let _light_attributes_count = r.read_u32()?;
            let _light_attributes_pad = r.read_u32()?;
            let _unk_pointer = r.read_u64()?;
            (name_pointer, r.read_u64()?)
        } else {
            (0, 0)
        };

        let name = r.read_string_at(name_pointer)?;
        let (shaders, texture_dictionary) = read_shader_group(r, shader_group_pointer)?;

        let mut models = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for (slot, pointer) in models.iter_mut().zip(lod_pointers) {
            *slot = read_model_list(r, pointer)?;
        }

        Ok(Self {
            base,
            name,
            bounding_center,
            bounding_sphere_radius,
            bounding_box_min,
            bounding_box_max,
            lod_distances,
            models,
            shaders,
            texture_dictionary,
            skeleton_pointer,
            joints_pointer,
            bound_pointer,
        })
    }

    /// Whether any LOD holds geometry that could be drawn.
    ///
    /// Cheap on purpose: it answers "is the 3D view worth opening" without
    /// building any buffers. A junk payload parses without error — the reader
    /// zero-fills past the end and reads a null pointer as an absent block — so
    /// "it parsed" is not the question worth asking.
    pub fn has_geometry(&self) -> bool {
        self.models
            .iter()
            .flatten()
            .any(|model| model.geometries.iter().any(DrawableGeometry::is_renderable))
    }

    /// The highest-detail LOD that has any models, with its level.
    pub fn best_lod(&self) -> Option<(Lod, &[DrawableModel])> {
        Lod::ALL
            .iter()
            .zip(&self.models)
            .find(|(_, models)| !models.is_empty())
            .map(|(lod, models)| (*lod, models.as_slice()))
    }

    /// The texture that fills a shader's named slot, resolved through the
    /// embedded dictionary.
    ///
    /// A shader's parameters name the texture for each slot by the texture's
    /// own hash; the pixels live in the drawable's embedded dictionary, keyed
    /// by the same hash. `None` when the slot is unbound, the shader is absent,
    /// or the dictionary does not hold it.
    pub fn texture_for_shader(&self, shader: usize, param: u32) -> Option<&Texture> {
        let shader = self.shaders.get(shader)?;
        let hash = shader.texture_for(param)?;
        self.texture_dictionary.as_ref()?.lookup(hash)
    }

    /// The diffuse texture for a geometry's shader, if the model embeds one.
    pub fn diffuse_texture(&self, shader: usize) -> Option<&Texture> {
        self.texture_for_shader(shader, DIFFUSE_SAMPLER)
    }
}

/// A `.ydd`: several drawables under Jenkins-hashed names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DrawableDictionary {
    pub base: ResourceFileBase,
    /// Jenkins hashes of the drawable names, parallel to `drawables`.
    pub hashes: Vec<u32>,
    pub drawables: Vec<Drawable>,
}

impl DrawableDictionary {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let base = ResourceFileBase::read(r)?;

        let _unknown_10h = r.read_u64()?;
        let _unknown_18h = r.read_u64()?;
        let hashes_pointer = r.read_u64()?;
        let hashes_count = r.read_u16()? as usize;
        let _hashes_count2 = r.read_u16()?;
        let _unknown_2ch = r.read_u32()?;
        let drawables_pointer = r.read_u64()?;
        let drawables_count = r.read_u16()? as usize;
        let _drawables_count2 = r.read_u16()?;
        let _unknown_3ch = r.read_u32()?;

        let hashes = r
            .read_u32s_at(hashes_pointer, hashes_count)?
            .unwrap_or_default();

        let pointers = r
            .read_u64s_at(drawables_pointer, drawables_count)?
            .unwrap_or_default();
        let mut drawables = Vec::with_capacity(pointers.len());
        for pointer in pointers {
            if let Some(drawable) = r.at(pointer, |r| Drawable::read(r, true))? {
                drawables.push(drawable);
            }
        }

        Ok(Self {
            base,
            hashes,
            drawables,
        })
    }
}

/// A material: a shader and the parameters that fill its slots.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderFx {
    /// Jenkins hash of the shader name, e.g. `spec`, `normal_decal`.
    pub name: u32,
    /// Jenkins hash of the `.sps` file name.
    pub file_name: u32,
    pub render_bucket: u8,
    pub parameter_count: u8,
    pub texture_parameter_count: u8,
    /// The parameter list: one entry per named slot, in file order. A texture
    /// parameter names the texture that fills it, by the texture's own hash.
    pub parameters: Vec<ShaderParameter>,
}

impl ShaderFx {
    /// The texture hash bound to the parameter named `param` — the name hashes
    /// like [`DIFFUSE_SAMPLER`] — if this shader binds one.
    pub fn texture_for(&self, param: u32) -> Option<u32> {
        self.parameters
            .iter()
            .find(|p| p.name == param)
            .and_then(|p| p.texture)
    }
}

/// One shader parameter: a named slot, and the texture that fills it when it is
/// a texture slot. Non-texture parameters (vector4s, buffers) are identified by
/// their name but their value is not read.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderParameter {
    /// Jenkins hash of the parameter name, e.g. `DiffuseSampler`.
    pub name: u32,
    /// `0` for a texture, `1+` for a vector4 array. Only texture parameters are
    /// resolved to a [`ShaderParameter::texture`].
    pub data_type: u8,
    /// Jenkins hash of the texture's name, present only for texture parameters.
    pub texture: Option<u32>,
}

/// The `ShaderGroup` block: an embedded texture dictionary and a list of shaders.
///
/// Port of `ShaderGroup` from `Drawable.cs`. The reference also walks the
/// shader parameters, swapping each `TextureBase` reference for the texture the
/// embedded dictionary actually holds; this port keeps the reference and lets
/// [`Drawable::texture_dictionary`] resolve it at the call site, so a drawable
/// whose dictionary fails to parse is not a total loss.
fn read_shader_group(
    r: &mut ResourceReader<'_>,
    pointer: u64,
) -> Result<(Vec<ShaderFx>, Option<TextureDictionary>)> {
    let Some(group) = r.at(pointer, |r| {
        let _vft = r.read_u32()?;
        let _unknown_4h = r.read_u32()?;
        let texture_dictionary_pointer = r.read_u64()?;
        let shaders_pointer = r.read_u64()?;
        let shaders_count = r.read_u16()? as usize;
        let _shaders_count2 = r.read_u16()?;
        let _unknown_1ch = r.read_u32()?;
        let _unknown_20h = r.read_u64()?;
        let _unknown_28h = r.read_u64()?;
        let _shader_group_blocks_size = r.read_u32()?;
        let _unknown_34h = r.read_u32()?;
        let _unknown_38h = r.read_u64()?;
        Ok((texture_dictionary_pointer, shaders_pointer, shaders_count))
    })?
    else {
        return Ok((Vec::new(), None));
    };

    let (texture_dictionary_pointer, shaders_pointer, shaders_count) = group;

    // The embedded dictionary is read best-effort: it is what the shaders'
    // texture parameters point into, and a broken one should cost the material
    // resolution, not the whole drawable. `TextureDictionary::read` refuses a
    // header whose name and texture lists disagree, which is how a truncated
    // embedded `.ytd` announces itself.
    let texture_dictionary = r.at(texture_dictionary_pointer, TextureDictionary::read)?;

    let pointers = r
        .read_u64s_at(shaders_pointer, shaders_count)?
        .unwrap_or_default();

    let mut shaders = Vec::with_capacity(pointers.len());
    for pointer in pointers {
        if let Some(shader) = r.at(pointer, read_shader_fx)? {
            shaders.push(shader);
        }
    }
    Ok((shaders, texture_dictionary))
}

fn read_shader_fx(r: &mut ResourceReader<'_>) -> Result<ShaderFx> {
    let parameters_pointer = r.read_u64()?;
    let name = r.read_u32()?;
    let _unknown_ch = r.read_u32()?;
    let parameter_count = r.read_u8()?;
    let render_bucket = r.read_u8()?;
    let _unknown_12h = r.read_u16()?;
    let _parameter_size = r.read_u16()?;
    let _parameter_data_size = r.read_u16()?;
    let file_name = r.read_u32()?;
    let _unknown_1ch = r.read_u32()?;
    let _render_bucket_mask = r.read_u32()?;
    let _unknown_24h = r.read_u16()?;
    let _unknown_26h = r.read_u8()?;
    let texture_parameter_count = r.read_u8()?;

    let parameters = read_shader_parameters(r, parameters_pointer, parameter_count)?;

    Ok(ShaderFx {
        name,
        file_name,
        render_bucket,
        parameter_count,
        texture_parameter_count,
        parameters,
    })
}

/// The `ShaderParametersBlock` of one shader.
///
/// Port of `ShaderParametersBlock.Read` from `Drawable.cs`. The block is a
/// sequence of 16-byte parameter records, then the vector4 data they point at,
/// then one u32 hash per parameter. Only the texture parameters are resolved —
/// each names a `TextureBase` whose `NameHash` is what the embedded dictionary
/// is keyed by. The vector4 data is skipped, matching the reference's
/// `reader.Position += offset`.
fn read_shader_parameters(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: u8,
) -> Result<Vec<ShaderParameter>> {
    let Some(params) = r.at(pointer, |r| {
        let mut records = Vec::with_capacity(count as usize);
        let mut offset = 0usize;
        for _ in 0..count {
            let data_type = r.read_u8()?;
            let _unknown_1h = r.read_u8()?;
            let _unknown_2h = r.read_u16()?;
            let _unknown_4h = r.read_u32()?;
            let data_pointer = r.read_u64()?;
            offset += 16 * data_type as usize;
            records.push((data_type, data_pointer));
        }
        // The vector4 data the pointers reference is embedded here, skipped the
        // way the reference skips it.
        let _ = r.read_bytes(offset)?;
        let hashes = r
            .read_u32s_at(r.position(), count as usize)?
            .unwrap_or_default();

        let mut parameters = Vec::with_capacity(records.len());
        for (index, (data_type, data_pointer)) in records.into_iter().enumerate() {
            let name = hashes.get(index).copied().unwrap_or(0);
            let texture = if data_type == 0 {
                read_texture_base_hash(r, data_pointer)?
            } else {
                None
            };
            parameters.push(ShaderParameter {
                name,
                data_type,
                texture,
            });
        }
        Ok(parameters)
    })?
    else {
        return Ok(Vec::new());
    };
    Ok(params)
}

/// The `NameHash` of the `TextureBase` at `pointer`, if it has a name.
///
/// Port of the legacy branch of `TextureBase.Read`: the 80-byte base header,
/// then the name string, hashed lowercased. Only the name is of interest here —
/// the pixels live in the embedded dictionary.
fn read_texture_base_hash(r: &mut ResourceReader<'_>, pointer: u64) -> Result<Option<u32>> {
    if pointer == 0 {
        return Ok(None);
    }
    r.at(pointer, |r| {
        for _ in 0..10 {
            let _ = r.read_u32()?;
        }
        let name_pointer = r.read_u64()?;
        let _unknown_30h = r.read_u16()?;
        let _unknown_32h = r.read_u16()?;
        let _unknown_34h = r.read_u32()?;
        let _unknown_38h = r.read_u32()?;
        let _unknown_3ch = r.read_u32()?;
        let _usage_data = r.read_u32()?;
        let _unknown_44h = r.read_u32()?;
        let _extra_flags = r.read_u32()?;
        let _unknown_4ch = r.read_u32()?;
        let Some(name) = r.read_string_at(name_pointer)? else {
            return Ok(None);
        };
        Ok(Some(crate::utils::jenk::gen_hash(&name.to_lowercase())))
    })
    .map(|opt| opt.flatten())
}

/// One LOD list: `{pointer, count, capacity, pad}` followed by `capacity`
/// pointers to models.
fn read_model_list(r: &mut ResourceReader<'_>, pointer: u64) -> Result<Vec<DrawableModel>> {
    let Some((array_pointer, capacity)) = r.at(pointer, |r| {
        let array_pointer = r.read_u64()?;
        let _count = r.read_u16()?;
        let capacity = r.read_u16()? as usize;
        Ok((array_pointer, capacity))
    })?
    else {
        return Ok(Vec::new());
    };

    let pointers = r.read_u64s_at(array_pointer, capacity)?.unwrap_or_default();
    let mut models = Vec::with_capacity(pointers.len());
    for pointer in pointers {
        if let Some(model) = r.at(pointer, DrawableModel::read)? {
            models.push(model);
        }
    }
    Ok(models)
}

/// A model: geometries sharing a skeleton binding, each with its own material.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DrawableModel {
    pub geometries: Vec<DrawableGeometry>,
    /// Index into the skeleton's bone list, from the high byte of the binding
    /// word. Zero when the drawable has no skeleton.
    pub bone_index: u8,
    pub has_skin: bool,
    pub render_mask: u8,
}

impl DrawableModel {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _vft = r.read_u32()?;
        let _unknown_4h = r.read_u32()?;
        let geometries_pointer = r.read_u64()?;
        let geometries_count = r.read_u16()? as usize;
        let _geometries_count2 = r.read_u16()?;
        let _unknown_14h = r.read_u32()?;
        let bounds_pointer = r.read_u64()?;
        let shader_mapping_pointer = r.read_u64()?;
        let skeleton_binding = r.read_u32()?;
        let render_mask_flags = r.read_u16()?;
        let _geometries_count3 = r.read_u16()?;

        let shader_mapping = r
            .read_u16s_at(shader_mapping_pointer, geometries_count)?
            .unwrap_or_default();

        // The bounds array is one AABB per geometry, preceded by a combined one
        // when there is more than a single geometry. Hence the +1 and the index
        // shift below.
        let bounds_count = if geometries_count > 1 {
            geometries_count + 1
        } else {
            geometries_count
        };
        let bounds = read_aabbs(r, bounds_pointer, bounds_count)?;

        let pointers = r
            .read_u64s_at(geometries_pointer, geometries_count)?
            .unwrap_or_default();

        let mut geometries = Vec::with_capacity(pointers.len());
        for (i, pointer) in pointers.into_iter().enumerate() {
            let Some(mut geometry) = r.at(pointer, DrawableGeometry::read)? else {
                continue;
            };
            geometry.shader_id = shader_mapping.get(i).copied().unwrap_or(0);
            geometry.bounds = if bounds.len() > 1 {
                bounds
                    .get(i + 1)
                    .copied()
                    .or_else(|| bounds.first().copied())
            } else {
                bounds.first().copied()
            };
            geometries.push(geometry);
        }

        Ok(Self {
            geometries,
            bone_index: ((skeleton_binding >> 24) & 0xFF) as u8,
            has_skin: (skeleton_binding >> 8) & 0xFF != 0,
            render_mask: (render_mask_flags & 0xFF) as u8,
        })
    }
}

/// Min and max as stored: two `Vector4`s, the `w` of each unused.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

fn read_aabbs(r: &mut ResourceReader<'_>, pointer: u64, count: usize) -> Result<Vec<Aabb>> {
    let Some(bytes) = r.read_bytes_at(pointer, count * 32)? else {
        return Ok(Vec::new());
    };
    Ok(bytes
        .chunks_exact(32)
        .map(|c| {
            let f = |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
            Aabb {
                min: [f(0), f(4), f(8)],
                max: [f(16), f(20), f(24)],
            }
        })
        .collect())
}

/// One draw call: a vertex buffer, an index buffer and a material index.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DrawableGeometry {
    /// Index into [`Drawable::shaders`].
    pub shader_id: u16,
    pub bounds: Option<Aabb>,
    pub vertex_count: usize,
    pub vertex_stride: usize,
    /// The declaration the vertex bytes are laid out by.
    pub declaration: Option<VertexDeclaration>,
    #[serde(skip)]
    pub vertex_bytes: Vec<u8>,
    #[serde(skip)]
    pub indices: Vec<u16>,
}

impl DrawableGeometry {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _vft = r.read_u32()?;
        let _unknown_4h = r.read_u32()?;
        let _unknown_8h = r.read_u64()?;
        let _unknown_10h = r.read_u64()?;
        let vertex_buffer_pointer = r.read_u64()?;
        let _unknown_20h = r.read_u64()?;
        let _unknown_28h = r.read_u64()?;
        let _unknown_30h = r.read_u64()?;
        let index_buffer_pointer = r.read_u64()?;
        let _unknown_40h = r.read_u64()?;
        let _unknown_48h = r.read_u64()?;
        let _unknown_50h = r.read_u64()?;
        let _indices_count = r.read_u32()?;
        let _triangles_count = r.read_u32()?;
        let declared_vertex_count = r.read_u16()? as usize;
        let _unknown_62h = r.read_u16()?;
        let _unknown_64h = r.read_u32()?;
        let _bone_ids_pointer = r.read_u64()?;
        let vertex_stride = r.read_u16()? as usize;
        let _bone_ids_count = r.read_u16()?;
        let _unknown_74h = r.read_u32()?;
        let _vertex_data_pointer = r.read_u64()?;

        let (declaration, vertex_count, vertex_bytes) =
            read_vertex_buffer(r, vertex_buffer_pointer)?;
        let indices = read_index_buffer(r, index_buffer_pointer)?;

        // The reference falls back to the buffer's own count when the geometry
        // declares zero, which some files do.
        let vertex_count = if declared_vertex_count == 0 {
            vertex_count
        } else {
            declared_vertex_count
        };

        Ok(Self {
            shader_id: 0,
            bounds: None,
            vertex_count,
            vertex_stride,
            declaration,
            vertex_bytes,
            indices,
        })
    }

    /// Whether this geometry could produce triangles at all: a declaration that
    /// includes a position, vertex bytes to read it from, and at least one
    /// triangle's worth of indices.
    pub fn is_renderable(&self) -> bool {
        self.declaration
            .is_some_and(|d| d.has(super::vertex::VertexSemantic::POSITION))
            && self.vertex_stride > 0
            && !self.vertex_bytes.is_empty()
            && self.indices.len() >= 3
    }

    /// One vertex's bytes, or `None` past the end of the buffer.
    pub fn vertex(&self, index: usize) -> Option<&[u8]> {
        if self.vertex_stride == 0 {
            return None;
        }
        let start = index.checked_mul(self.vertex_stride)?;
        self.vertex_bytes.get(start..start + self.vertex_stride)
    }
}

/// Returns the declaration, the vertex count and the raw vertex bytes.
///
/// The reference reads both `Data1` and `Data2` and then takes
/// `Data1 ?? Data2`; in every stock file they are the same pointer. Reading only
/// the one that gets used avoids a second copy of what is usually the largest
/// allocation in the file.
fn read_vertex_buffer(
    r: &mut ResourceReader<'_>,
    pointer: u64,
) -> Result<(Option<VertexDeclaration>, usize, Vec<u8>)> {
    let Some((stride, data_pointer1, data_pointer2, vertex_count, info_pointer)) =
        r.at(pointer, |r| {
            let _vft = r.read_u32()?;
            let _unknown_4h = r.read_u32()?;
            let stride = r.read_u16()? as usize;
            let _flags = r.read_u16()?;
            let _unknown_ch = r.read_u32()?;
            let data_pointer1 = r.read_u64()?;
            let vertex_count = r.read_u32()? as usize;
            let _unknown_1ch = r.read_u32()?;
            let data_pointer2 = r.read_u64()?;
            let _unknown_28h = r.read_u64()?;
            let info_pointer = r.read_u64()?;
            Ok((
                stride,
                data_pointer1,
                data_pointer2,
                vertex_count,
                info_pointer,
            ))
        })?
    else {
        return Ok((None, 0, Vec::new()));
    };

    let declaration = r.at(info_pointer, VertexDeclaration::read)?;

    let data_pointer = if data_pointer1 != 0 {
        data_pointer1
    } else {
        data_pointer2
    };
    let length = vertex_count.saturating_mul(stride);
    let bytes = if length == 0 || length > MAX_ELEMENTS {
        Vec::new()
    } else {
        r.read_bytes_at(data_pointer, length)?.unwrap_or_default()
    };

    Ok((declaration, vertex_count, bytes))
}

/// Indices are always 16-bit in Legacy resources; the reference's only 32-bit
/// path is a Gen9 `TODO`.
fn read_index_buffer(r: &mut ResourceReader<'_>, pointer: u64) -> Result<Vec<u16>> {
    let Some((count, indices_pointer)) = r.at(pointer, |r| {
        let _vft = r.read_u32()?;
        let _unknown_4h = r.read_u32()?;
        let count = r.read_u32()? as usize;
        let _unknown_ch = r.read_u32()?;
        let indices_pointer = r.read_u64()?;
        Ok((count, indices_pointer))
    })?
    else {
        return Ok(Vec::new());
    };

    if count == 0 || count > MAX_ELEMENTS {
        return Ok(Vec::new());
    }
    Ok(r.read_u16s_at(indices_pointer, count)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::reader::SYSTEM_BASE;

    /// Build a system segment where every block sits at a known address, then
    /// read it back. This pins the field offsets: any drifted read lands on a
    /// neighbouring field and the assertions fail.
    fn synthetic_drawable() -> Vec<u8> {
        // Layout, all in the system segment:
        //   0x000  Drawable (208 bytes)
        //   0x100  LOD-high pointer list header (16)
        //   0x110  model pointer array (8)
        //   0x120  DrawableModel (48)
        //   0x160  geometry pointer array (8)
        //   0x170  shader mapping (2)
        //   0x180  DrawableGeometry (152)
        //   0x220  VertexBuffer (128)
        //   0x2A0  IndexBuffer (96)
        //   0x300  VertexDeclaration (16)
        //   0x310  vertex bytes (3 * 12)
        //   0x340  indices (3 * 2)
        //   0x350  name
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
        let put_f32 = |d: &mut Vec<u8>, at: usize, v: f32| {
            d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let addr = |offset: u64| SYSTEM_BASE + offset;

        // Drawable
        put_f32(&mut d, 0x20, 1.0); // bounding centre x
        put_f32(&mut d, 0x2C, 7.5); // bounding sphere radius
        put_f32(&mut d, 0x30, -2.0); // bbox min x
        put_f32(&mut d, 0x40, 2.0); // bbox max x
        put_u64(&mut d, 0x50, addr(0x100)); // LOD high
        put_f32(&mut d, 0x70, 100.0); // LOD distance high
        put_u64(&mut d, 0xA8, addr(0x350)); // name

        // LOD-high pointer list: one model.
        put_u64(&mut d, 0x100, addr(0x110));
        put_u16(&mut d, 0x108, 1); // count
        put_u16(&mut d, 0x10A, 1); // capacity
        put_u64(&mut d, 0x110, addr(0x120));

        // DrawableModel with one geometry.
        put_u64(&mut d, 0x128, addr(0x160)); // geometries pointer
        put_u16(&mut d, 0x130, 1); // geometries count
        put_u64(&mut d, 0x138, 0); // bounds pointer (absent)
        put_u64(&mut d, 0x140, addr(0x170)); // shader mapping
        put_u32(&mut d, 0x148, 0x0300_0100); // skeleton binding: bone 3, skinned
        put_u64(&mut d, 0x160, addr(0x180));
        put_u16(&mut d, 0x170, 5); // shader id

        // DrawableGeometry
        put_u64(&mut d, 0x180 + 0x18, addr(0x220)); // vertex buffer
        put_u64(&mut d, 0x180 + 0x38, addr(0x2A0)); // index buffer
        put_u32(&mut d, 0x180 + 0x58, 3); // indices count
        put_u16(&mut d, 0x180 + 0x60, 3); // vertices count
        put_u16(&mut d, 0x180 + 0x70, 12); // vertex stride

        // VertexBuffer
        put_u16(&mut d, 0x220 + 0x08, 12); // stride
        put_u64(&mut d, 0x220 + 0x10, addr(0x310)); // data 1
        put_u32(&mut d, 0x220 + 0x18, 3); // vertex count
        put_u64(&mut d, 0x220 + 0x30, addr(0x300)); // declaration

        // IndexBuffer
        put_u32(&mut d, 0x2A0 + 0x08, 3); // indices count
        put_u64(&mut d, 0x2A0 + 0x10, addr(0x340));

        // VertexDeclaration: position only, Float3.
        put_u32(&mut d, 0x300, 0b1); // flags
        put_u16(&mut d, 0x304, 12); // stride
        d[0x307] = 1; // count
        put_u64(&mut d, 0x308, 0x7755_5555_5599_6996);

        // Three vertices.
        for (i, v) in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .into_iter()
            .enumerate()
        {
            for (k, c) in v.into_iter().enumerate() {
                put_f32(&mut d, 0x310 + i * 12 + k * 4, c);
            }
        }
        for (i, v) in [0u16, 1, 2].into_iter().enumerate() {
            put_u16(&mut d, 0x340 + i * 2, v);
        }

        d[0x350..0x350 + 8].copy_from_slice(b"tri_test");

        d
    }

    #[test]
    fn reads_a_drawable_end_to_end() {
        let data = synthetic_drawable();
        let mut r = super::ResourceReader::new(&data, data.len());
        let drawable = Drawable::read(&mut r, true).unwrap();

        assert_eq!(drawable.name.as_deref(), Some("tri_test"));
        assert_eq!(drawable.bounding_sphere_radius, 7.5);
        assert_eq!(drawable.bounding_center, [1.0, 0.0, 0.0]);
        assert_eq!(drawable.bounding_box_min, [-2.0, 0.0, 0.0]);
        assert_eq!(drawable.bounding_box_max, [2.0, 0.0, 0.0]);
        assert_eq!(drawable.lod_distances[0], 100.0);

        let (lod, models) = drawable.best_lod().unwrap();
        assert_eq!(lod, Lod::High);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].bone_index, 3);
        assert!(models[0].has_skin);

        let geometry = &models[0].geometries[0];
        assert_eq!(geometry.shader_id, 5);
        assert_eq!(geometry.vertex_count, 3);
        assert_eq!(geometry.vertex_stride, 12);
        assert_eq!(geometry.indices, vec![0, 1, 2]);

        let declaration = geometry.declaration.unwrap();
        let vertex = geometry.vertex(1).unwrap();
        let position = declaration.decode(vertex, 0).unwrap();
        assert_eq!([position[0], position[1], position[2]], [1.0, 0.0, 0.0]);
    }

    /// A drawable with no LOD pointers at all is empty, not an error — plenty
    /// of dictionary entries are placeholders.
    #[test]
    fn reads_a_drawable_with_no_models() {
        let data = vec![0u8; 0x100];
        let mut r = super::ResourceReader::new(&data, data.len());
        let drawable = Drawable::read(&mut r, true).unwrap();
        assert!(drawable.best_lod().is_none());
        assert!(drawable.shaders.is_empty());
        assert!(drawable.name.is_none());
    }
}
