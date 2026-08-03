//! Looking at one file: open it, then read it as text, as hex pages, or as a
//! mesh.
//!
//! The rule from `docs/architecture.md` is that the WebView never parses archive
//! data. So every command here hands back something already interpreted — a
//! decoded string, a mesh bundle, or an explicitly requested byte range for the
//! hex view, which is the one case where bytes are the answer rather than the
//! input.
//!
//! One file is open at a time and its extracted payload is held in
//! [`ViewerState`]. Extraction means a seek, a decrypt and an inflate; a hex
//! view that pages through a 20 MB texture dictionary would otherwise redo all
//! three per scroll.

use std::path::PathBuf;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cw_core::audio::AwcFile;
use cw_core::filetype::FileKind;
use cw_core::filetypes::{read_ymap, read_ytyp, Archetype, Extension, MloArchetype, MloEntity};
use cw_core::image;
use cw_core::mesh::MeshBundle;
use cw_core::meta::{read_meta as read_metadata, AssetType, LodLevel, Meta, MetaFile, MetaNode, PriorityLevel};
use cw_core::resources::{
    read_bounds as read_collision_bounds, read_clips as read_clip_dictionary, read_drawables,
    read_named_drawables, read_named_fragments, read_textures, Drawable, Texture,
    TextureDictionary,
};
use cw_core::rpf::EntryKind;
use cw_core::text::{self, Encoding, TextFile};
use cw_core::utils::hash_string;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::explorer::GameState;

/// How many bytes one `read_hex` call will hand over. A page larger than this
/// is slower to serialize than the user can read.
const MAX_HEX_PAGE: usize = 64 * 1024;

/// The currently open file and its extracted payload.
pub struct OpenFile {
    pub info: EntryInfo,
    pub data: Vec<u8>,
    /// The AWC key, carried so the audio commands can decrypt without taking
    /// the game lock again.
    pub awc_key: [u32; 4],
}

#[derive(Default)]
pub struct ViewerState(pub Mutex<Option<OpenFile>>);

/// The system and graphics segment sizes of a resource, and its format version.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    pub system_size: usize,
    pub graphics_size: usize,
    /// 165 for `.ydr`/`.ydd`, 162 for `.yft`, 13 for `.ytd`.
    pub version: u32,
}

/// What the frontend needs to decide how to show a file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryInfo {
    pub path: String,
    pub name: String,
    /// `binary` or `resource`, as in a listing.
    pub kind: &'static str,
    pub file_kind: FileKind,
    /// Human label for `file_kind`, e.g. "drawable dictionary".
    pub file_kind_label: &'static str,
    /// Size of the entry as stored in the archive.
    pub size_on_disk: u64,
    /// Size of the payload once decrypted and decompressed.
    pub byte_length: usize,
    pub encrypted: bool,
    pub resource: Option<ResourceInfo>,
    /// The viewer the UI should open by default.
    pub viewer: &'static str,
    /// Every viewer that will work for this file. Hex is always in here.
    pub viewers: Vec<&'static str>,
    /// Why the preferred viewer is not the one the extension implies — a
    /// drawable that failed to parse, for instance. Shown, not swallowed.
    pub note: Option<String>,
}

/// One page of bytes for the hex view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HexPage {
    pub offset: usize,
    /// Base64 of the raw bytes in this range.
    pub bytes: String,
    pub length: usize,
    pub total: usize,
}

/// One drawable in an open `.ydr` or `.ydd`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDrawable {
    /// Position in the dictionary, and the handle the UI selects by.
    pub index: usize,
    /// The name from the dictionary's hash list, or `hash_XXXXXXXX`.
    pub name: String,
    pub hash: u32,
    /// Every LOD that has renderable geometry, highest first. A drawable whose
    /// lowest LOD is its only geometry still lists one bundle here. Absent
    /// entirely when the drawable holds nothing renderable, which is normal in
    /// a dictionary: it is still listed, so the count is never silently wrong.
    pub lods: Vec<MeshBundle>,
}

/// One resolved texture a model's parts reference, decoded to a PNG.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTexture {
    /// The Jenkins hash the parts reference — `MeshPart::diffuse` and friends.
    pub hash: u32,
    /// The name the texture carries in its dictionary.
    pub name: String,
    pub width: usize,
    pub height: usize,
    /// Base64 of a complete PNG. Decoded in Rust per the boundary rule: the
    /// browser would have no business reading BC-compressed blocks.
    pub png: String,
}

/// Everything drawable in one file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreview {
    pub path: String,
    pub drawables: Vec<ModelDrawable>,
    /// How many of `drawables` produced no geometry.
    pub empty_drawables: usize,
    /// Every texture the parts reference that could be resolved and decoded,
    /// keyed by [`ModelTexture::hash`].
    pub textures: Vec<ModelTexture>,
}

/// Open an entry: extract its payload, classify it, and keep it for the reads
/// that follow.
///
/// Runs like `list_dir` rather than `open_game_folder`: the work is bounded by
/// one entry, not by 179 archives.
#[tauri::command]
pub fn open_entry(
    path: String,
    game: State<'_, GameState>,
    viewer: State<'_, ViewerState>,
) -> Result<EntryInfo, String> {
    let guard = game.0.lock().map_err(|_| "game state is poisoned")?;
    let loaded = guard.as_ref().ok_or("no game folder is open")?;

    let entry_ref = loaded
        .manager
        .find(&path)
        .ok_or_else(|| format!("{path} is not in the index"))?;
    let entry = loaded.manager.entry(entry_ref);

    if entry.is_directory() {
        return Err(format!("{path} is a directory"));
    }

    let data = loaded
        .manager
        .extract(entry_ref, &loaded.keys)
        .map_err(|e| format!("could not read {path}: {e}"))?;

    let file_kind = FileKind::from_name(&entry.name_lower);
    let resource = entry
        .resource_segments()
        .map(|(system, graphics)| ResourceInfo {
            system_size: system,
            graphics_size: graphics,
            version: entry.resource_version().unwrap_or(0),
        });

    let awc_key = loaded.keys.awc_key;
    let (viewer_name, viewers, note) =
        choose_viewer(file_kind, &entry.name_lower, &data, resource.as_ref(), &awc_key);

    let info = EntryInfo {
        path: entry.path.clone(),
        name: entry.name.clone(),
        kind: match &entry.kind {
            EntryKind::Directory(_) => "directory",
            EntryKind::Binary(_) if file_kind == FileKind::Archive => "archive",
            EntryKind::Binary(_) => "binary",
            EntryKind::Resource(_) => "resource",
        },
        file_kind,
        file_kind_label: file_kind.label(),
        size_on_disk: entry.file_size_on_disk(),
        byte_length: data.len(),
        encrypted: entry.is_encrypted(),
        resource,
        viewer: viewer_name,
        viewers,
        note,
    };

    *viewer.0.lock().map_err(|_| "viewer state is poisoned")? = Some(OpenFile {
        info: info.clone(),
        data,
        awc_key,
    });

    Ok(info)
}

/// Pick the default viewer, and say what else would work.
///
/// A model that will not parse falls back rather than showing an empty canvas,
/// and the reason travels with it — silently degrading to hex would look like
/// the file was never a model.
fn choose_viewer(
    kind: FileKind,
    name_lower: &str,
    data: &[u8],
    resource: Option<&ResourceInfo>,
    awc_key: &[u32; 4],
) -> (&'static str, Vec<&'static str>, Option<String>) {
    let readable_as_text = text::is_text(data);
    let mut viewers = vec!["hex"];
    if readable_as_text {
        viewers.insert(0, "text");
    }

    if kind.has_audio() {
        return match AwcFile::read(data, awc_key) {
            Ok(awc) if awc.streams.iter().any(|s| s.is_playable()) => {
                viewers.insert(0, "audio");
                ("audio", viewers, None)
            }
            Ok(_) => (
                viewers[0],
                viewers.clone(),
                Some("this container holds no playable streams".into()),
            ),
            Err(e) => (viewers[0], viewers.clone(), Some(format!("{e}"))),
        };
    }

    if kind.has_clips() {
        match resource {
            Some(resource) => match read_clip_dictionary(kind, data, resource.system_size) {
                Ok(dictionary) if dictionary.has_content() => {
                    viewers.insert(0, "clip");
                    return ("clip", viewers, None);
                }
                Ok(_) => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("this dictionary holds no clips or animations".into()),
                    )
                }
                Err(e) => {
                    let note = format!("could not read the clips: {e}");
                    return (viewers[0], viewers.clone(), Some(note));
                }
            },
            None => {
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("a clip dictionary stored as a binary entry, not a resource".into()),
                )
            }
        }
    }

    if kind.has_metadata() {
        let system_size = resource.map(|r| r.system_size);
        // A `.ymap` or `.ytyp` that parses as an RSC resource gets the typed map
        // viewer: the placement hierarchy, or the archetype list with MLO
        // interiors. Anything else falls through to the generic XML tree.
        let is_placement = name_lower.ends_with(".ymap") || name_lower.ends_with(".ytyp");
        match read_metadata(kind, data, system_size) {
            Ok(meta) => {
                if is_placement && system_size.is_some() {
                    if let MetaFile::Rsc(meta) = &meta {
                        if build_map_preview(name_lower, meta).is_some() {
                            viewers.insert(0, "map");
                            viewers.insert(1, "meta");
                            return ("map", viewers, None);
                        }
                    }
                }
                if meta.has_content() {
                    viewers.insert(0, "meta");
                    return ("meta", viewers, None);
                }
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("this metadata file holds no structures".into()),
                );
            }
            Err(e) => {
                let note = format!("could not read the metadata: {e}");
                return (viewers[0], viewers.clone(), Some(note));
            }
        }
    }

    if kind.has_textures() {
        match resource {
            Some(resource) => match read_textures(kind, data, resource.system_size) {
                Ok(dictionary) if dictionary.textures.iter().any(Texture::is_decodable) => {
                    viewers.insert(0, "texture");
                    return ("texture", viewers, None);
                }
                Ok(dictionary) if dictionary.textures.is_empty() => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("this dictionary holds no textures".into()),
                    )
                }
                Ok(_) => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("none of the textures here are in a format with a decoder".into()),
                    )
                }
                Err(e) => {
                    let note = format!("could not read the textures: {e}");
                    return (viewers[0], viewers.clone(), Some(note));
                }
            },
            None => {
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("a texture dictionary stored as a binary entry, not a resource".into()),
                )
            }
        }
    }

    if kind.has_bounds() {
        match resource {
            Some(resource) => match read_collision_bounds(kind, data, resource.system_size) {
                Ok(bounds) if bounds.triangle_mesh().is_some() => {
                    viewers.insert(0, "bounds");
                    return ("bounds", viewers, None);
                }
                Ok(_) => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("this collision file holds no triangles to draw".into()),
                    )
                }
                Err(e) => {
                    let note = format!("could not read the collision bounds: {e}");
                    return (viewers[0], viewers.clone(), Some(note));
                }
            },
            None => {
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("a bounds file stored as a binary entry, not a resource".into()),
                )
            }
        }
    }

    if kind.has_model() {
        let system_size = resource.map(|r| r.system_size);
        // A `.yft` has no top-level drawable: its geometry lives in the embedded
        // `FragDrawable`s, read via `Frag.cs`.
        if kind == FileKind::Fragment {
            return match system_size {
                Some(system_size) => {
                    match cw_core::resources::read_fragments(kind, data, system_size) {
                        Ok(fragment)
                            if fragment
                                .all_drawables()
                                .iter()
                                .any(|d| d.drawable.has_geometry()) =>
                        {
                            viewers.insert(0, "model");
                            ("model", viewers, None)
                        }
                        Ok(_) => (
                            viewers[0],
                            viewers.clone(),
                            Some("this fragment holds no renderable geometry".into()),
                        ),
                        Err(e) => (
                            viewers[0],
                            viewers.clone(),
                            Some(format!("could not read the fragment: {e}")),
                        ),
                    }
                }
                None => (
                    viewers[0],
                    viewers.clone(),
                    Some("a fragment stored as a binary entry, not a resource".into()),
                ),
            };
        }

        match resource {
            Some(resource) => match read_drawables(kind, data, resource.system_size) {
                Ok(drawables) if drawables.iter().any(Drawable::has_geometry) => {
                    viewers.insert(0, "model");
                    return ("model", viewers, None);
                }
                Ok(_) => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("this file holds no renderable geometry".into()),
                    )
                }
                Err(e) => {
                    let note = format!("could not read the geometry: {e}");
                    return (viewers[0], viewers.clone(), Some(note));
                }
            },
            None => {
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("a drawable stored as a binary entry, not a resource".into()),
                )
            }
        }
    }

    if kind.has_gxt2() {
        return match cw_core::gxt2::Gxt2File::read(data) {
            Ok(_gxt2) => {
                viewers.insert(0, "gxt2");
                ("gxt2", viewers, None)
            }
            Err(e) => {
                let note = format!("could not read the text table: {e}");
                (viewers[0], viewers.clone(), Some(note))
            }
        };
    }

    let note = match kind {
        FileKind::Particle if !readable_as_text => {
            Some("particle dictionaries embed drawables, but Particle.cs is not ported yet".into())
        }
        FileKind::AudioMetadata if !readable_as_text => {
            Some("RelFile.cs, which names and sequences the waves in the .awc files, is not ported yet".into())
        }
        FileKind::Clip | FileKind::Nav | FileKind::Bounds if !readable_as_text => {
            // `.yed`, `.yfd`, `.yld` and `.ypdb` land here: they are animation
            // files but not clip dictionaries, and each needs its own reader.
            Some(format!("no {} parser yet, so this opens as bytes", kind.label()))
        }
        _ => None,
    };

    (viewers[0], viewers, note)
}

/// Decode the open file as text.
#[tauri::command]
pub fn read_text(viewer: State<'_, ViewerState>) -> Result<TextFile, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    Ok(text::decode(&open.data))
}

/// One entry of a `.gxt2` text table.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gxt2Entry {
    pub hash: u32,
    pub text: String,
}

/// The text table an open `.gxt2` holds.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gxt2Preview {
    pub path: String,
    pub entries: Vec<Gxt2Entry>,
}

/// Read the `.gxt2` text table the open file holds.
#[tauri::command]
pub fn read_gxt2(viewer: State<'_, ViewerState>) -> Result<Gxt2Preview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let file = cw_core::gxt2::Gxt2File::read(&open.data)
        .map_err(|e| format!("could not read the text table: {e}"))?;

    Ok(Gxt2Preview {
        path: open.info.path.clone(),
        entries: file
            .entries
            .into_iter()
            .map(|e| Gxt2Entry { hash: e.hash, text: e.text })
            .collect(),
    })
}

/// One page of the open file's bytes.
#[tauri::command]
pub fn read_hex(
    offset: usize,
    length: usize,
    viewer: State<'_, ViewerState>,
) -> Result<HexPage, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let total = open.data.len();
    let start = offset.min(total);
    let end = start.saturating_add(length.min(MAX_HEX_PAGE)).min(total);
    let slice = &open.data[start..end];

    Ok(HexPage {
        offset: start,
        bytes: STANDARD.encode(slice),
        length: slice.len(),
        total,
    })
}

/// Build the mesh bundles for the open file.
#[tauri::command]
pub fn read_model(
    viewer: State<'_, ViewerState>,
    game: State<'_, GameState>,
) -> Result<ModelPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let resource = open
        .info
        .resource
        .as_ref()
        .ok_or("this file is not a resource, so it has no geometry")?;

    let named = match open.info.file_kind {
        FileKind::Fragment => read_named_fragments(open.info.file_kind, &open.data, resource.system_size)
            .map_err(|e| format!("could not read the geometry: {e}"))?,
        _ => read_named_drawables(open.info.file_kind, &open.data, resource.system_size)
            .map_err(|e| format!("could not read the geometry: {e}"))?,
    };

    // The texture hashes every part wants, diffuse and otherwise, deduplicated.
    let mut wanted = std::collections::BTreeSet::new();
    for entry in &named {
        for bundle in MeshBundle::all(&entry.drawable) {
            for part in &bundle.parts {
                for slot in [part.diffuse, part.normal, part.specular] {
                    if let Some(hash) = slot {
                        wanted.insert(hash);
                    }
                }
            }
        }
    }

    let textures = resolve_model_textures(
        &open.info.path,
        &wanted,
        named.iter().filter_map(|e| e.drawable.texture_dictionary.as_ref()),
        &game,
    );

    let mut empty_drawables = 0;
    let drawables: Vec<ModelDrawable> = named
        .into_iter()
        .map(|entry| {
            let lods = MeshBundle::all(&entry.drawable);
            if lods.is_empty() {
                empty_drawables += 1;
            }
            ModelDrawable {
                index: entry.index,
                name: entry.name,
                hash: entry.hash,
                lods,
            }
        })
        .collect();

    if drawables.iter().all(|d| d.lods.is_empty()) {
        return Err("this file holds no renderable geometry".into());
    }

    Ok(ModelPreview {
        path: open.info.path.clone(),
        drawables,
        empty_drawables,
        textures,
    })
}

/// Resolve the texture hashes a model's shaders reference.
///
/// Resolution order, mirroring the reference's `GetTextureSamplerList` chain:
/// the drawables' embedded dictionaries first, then a `.ytd` sharing the
/// model's basename, then every `.ytd` in the same folder. Most stock drawables
/// carry no embedded dictionary — the pixels live in sibling `.ytd` files the
/// archetype loads — so the folder scan is what usually lands.
///
/// A hash that resolves to a texture that cannot be decoded is skipped, and a
/// part referencing it renders untextured rather than aborting the model.
fn resolve_model_textures<'a>(
    path: &str,
    wanted: &std::collections::BTreeSet<u32>,
    embedded: impl Iterator<Item = &'a cw_core::resources::TextureDictionary>,
    game: &GameState,
) -> Vec<ModelTexture> {
    if wanted.is_empty() {
        return Vec::new();
    }

    let (folder, basename) = match path.rsplit_once('\\') {
        Some((folder, name)) => {
            let stem = name
                .strip_suffix(".ydr")
                .or_else(|| name.strip_suffix(".ydd"))
                .or_else(|| name.strip_suffix(".yft"))
                .unwrap_or(name);
            (folder, stem)
        }
        None => ("", path),
    };

    let mut dictionaries: Vec<(String, cw_core::resources::TextureDictionary)> = Vec::new();

    // The drawables' own dictionaries first: they are already in memory, and
    // they are what the shader *points* at.
    for dictionary in embedded {
        dictionaries.push(("embedded".into(), dictionary.clone()));
    }

    // Then the sibling `.ytd` files. The whole game archive tree is walked; a
    // `.ytd` belongs to the model's folder when its path is the model's folder
    // plus the file name. The same-basename sibling is visited first, since the
    // reference tries it before falling back to the archetype's dictionary list.
    let game_guard = game.0.lock().ok();
    let loaded = game_guard.as_ref().and_then(|g| g.as_ref());

    if let Some(loaded) = loaded {
        let sibling = format!("{folder}\\{basename}.ytd");
        let mut paths: Vec<String> = Vec::new();
        for archive in &loaded.manager.archives {
            for entry in &archive.entries {
                if !entry.name_lower.ends_with(".ytd") {
                    continue;
                }
                // Preferred sibling first, then same-folder files. A `.ytd`
                // anywhere else in the tree is unrelated to this model.
                if entry.path.as_str() != sibling.as_str()
                    && !entry.path.starts_with(&format!("{folder}\\"))
                {
                    continue;
                }
                paths.push(entry.path.clone());
            }
        }
        paths.sort_by_key(|p| *p != sibling);

        for path in paths {
            let Some(reference) = loaded.manager.find(&path) else { continue };
            let entry = loaded.manager.entry(reference);
            let Some((system_size, _)) = entry.resource_segments() else { continue };
            if let Ok(data) = loaded.manager.extract(reference, &loaded.keys) {
                if let Ok(dictionary) =
                    cw_core::resources::read_textures(FileKind::Texture, &data, system_size)
                {
                    dictionaries.push((path, dictionary));
                }
            }
        }
    }

    let mut by_hash: std::collections::HashMap<u32, ModelTexture> =
        std::collections::HashMap::new();
    for (_, dictionary) in &dictionaries {
        for hash in wanted {
            if by_hash.contains_key(hash) {
                continue;
            }
            let Some(texture) = dictionary.lookup(*hash) else { continue };
            let Ok(decoded) = image::decode_mip(texture, 0) else { continue };
            let Ok(png) = image::to_png(&decoded) else { continue };
            by_hash.insert(
                *hash,
                ModelTexture {
                    hash: *hash,
                    name: texture
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("{:08x}", texture.name_hash)),
                    width: decoded.width,
                    height: decoded.height,
                    png: STANDARD.encode(png),
                },
            );
        }
        // Once every requested hash has a texture, stop scanning folders.
        if by_hash.len() == wanted.len() {
            break;
        }
    }

    by_hash.values().cloned().collect()
}

/// One collision geometry in an open `.ybn`, as uploadable buffers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundsPart {
    /// The tree path from the bounds root, e.g. `child0`.
    pub name: String,
    pub vertex_count: usize,
    pub triangle_count: usize,
    /// Base64 of tightly packed `f32` triples, in world space.
    pub positions: String,
    /// Base64 of `u32` triples.
    pub indices: String,
}

/// The collision geometry of one file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundsPreview {
    pub path: String,
    /// The root bounds type, e.g. `Composite`.
    pub bounds_type: &'static str,
    /// Number of children in the composite tree, when the root is one.
    pub children: usize,
    pub vertex_count: usize,
    pub triangle_count: usize,
    pub parts: Vec<BoundsPart>,
}

/// Flatten the collision geometry of the open `.ybn` for the 3D viewer.
///
/// Mirrors `read_model`: the triangles are decoded and transformed in Rust, and
/// only the buffers cross.
#[tauri::command]
pub fn read_bounds(viewer: State<'_, ViewerState>) -> Result<BoundsPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let resource = open
        .info
        .resource
        .as_ref()
        .ok_or("this file is not a resource, so it holds no collision geometry")?;

    let bounds = read_collision_bounds(open.info.file_kind, &open.data, resource.system_size)
        .map_err(|e| format!("could not read the collision bounds: {e}"))?;

    let children = match &bounds.data {
        cw_core::resources::BoundsData::Composite(c) => c.children.len(),
        _ => 0,
    };

    let mesh = bounds
        .triangle_mesh()
        .ok_or("this file holds no triangles to draw")?;

    let parts = mesh
        .parts
        .into_iter()
        .map(|part| BoundsPart {
            name: part.name,
            vertex_count: part.vertex_count,
            triangle_count: part.triangle_count,
            positions: encode_f32(&part.positions),
            indices: encode_u32(&part.indices),
        })
        .collect();

    Ok(BoundsPreview {
        path: open.info.path.clone(),
        bounds_type: bounds.bounds_type.as_str(),
        children,
        vertex_count: mesh.vertex_count,
        triangle_count: mesh.triangle_count,
        parts,
    })
}

fn encode_f32(values: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    STANDARD.encode(bytes)
}

fn encode_u32(values: &[u32]) -> String {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    STANDARD.encode(bytes)
}

/// One texture in an open `.ytd`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureEntry {
    /// Position in the dictionary, and the handle for `read_texture_image`.
    pub index: usize,
    /// The name the file carries. Textures are named, unlike AWC streams.
    pub name: String,
    pub name_hash: u32,
    pub width: u16,
    pub height: u16,
    pub levels: u8,
    /// The `D3DFORMAT` name, e.g. `D3DFMT_DXT5`.
    pub format: String,
    pub usage: &'static str,
    /// Size of the pixel payload, all mips together.
    pub byte_length: usize,
    /// False when the format has no decoder or the payload is short. The entry
    /// is still listed — a dictionary showing eight of nine textures with no
    /// explanation for the ninth is worse than showing it greyed out.
    pub decodable: bool,
}

/// What an open `.ytd` contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TexturePreview {
    pub path: String,
    pub textures: Vec<TextureEntry>,
}

/// One decoded mip, as a PNG.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureImage {
    pub index: usize,
    pub level: usize,
    pub width: usize,
    pub height: usize,
    /// Base64 of a complete PNG, ready for an `<img>` element.
    pub png: String,
}

/// Parse the open file as a texture dictionary. Kept private so all three
/// texture commands agree on what "the open file" means.
fn open_ytd(open: &OpenFile) -> Result<TextureDictionary, String> {
    let resource = open
        .info
        .resource
        .as_ref()
        .ok_or("this file is not a resource, so it holds no textures")?;
    read_textures(open.info.file_kind, &open.data, resource.system_size)
        .map_err(|e| format!("could not read the textures: {e}"))
}

/// List the textures in the open `.ytd`.
///
/// The pixels are not decoded here: a dictionary runs to dozens of textures and
/// tens of megabytes, and the listing is what the UI needs first — the same
/// split as `read_audio`.
#[tauri::command]
pub fn read_texture(viewer: State<'_, ViewerState>) -> Result<TexturePreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let dictionary = open_ytd(open)?;

    let textures = dictionary
        .textures
        .iter()
        .enumerate()
        .map(|(index, texture)| TextureEntry {
            index,
            name: texture
                .name
                .clone()
                .unwrap_or_else(|| format!("{:08x}", texture.name_hash)),
            name_hash: texture.name_hash,
            width: texture.width,
            height: texture.height,
            levels: texture.levels,
            format: texture
                .format
                .map(|f| f.label().to_string())
                .unwrap_or_else(|| format!("unknown ({:#x})", texture.format_raw)),
            usage: texture.usage.label(),
            byte_length: texture.data.len(),
            decodable: texture.is_decodable(),
        })
        .collect();

    Ok(TexturePreview {
        path: open.info.path.clone(),
        textures,
    })
}

/// Decode one texture's mip level to a PNG.
///
/// Decoded in Rust rather than in the WebView, per the boundary rule: no
/// browser reads BC-compressed blocks, and shipping the raw payload across
/// would be handing the frontend archive bytes to parse. PNG rather than raw
/// RGBA because a 2048² texture is 16 MB of pixels and 22 MB once base64'd.
#[tauri::command]
pub fn read_texture_image(
    index: usize,
    level: usize,
    viewer: State<'_, ViewerState>,
) -> Result<TextureImage, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let dictionary = open_ytd(open)?;

    let texture = dictionary
        .textures
        .get(index)
        .ok_or_else(|| format!("texture {index} is not in this dictionary"))?;

    let decoded = image::decode_mip(texture, level).map_err(|e| format!("{e}"))?;
    let png = image::to_png(&decoded).map_err(|e| format!("{e}"))?;

    Ok(TextureImage {
        index,
        level,
        width: decoded.width,
        height: decoded.height,
        png: STANDARD.encode(png),
    })
}

/// The file format `save_texture` writes.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextureExport {
    /// The stored pixels with a DDS header in front — no decode, no loss, and
    /// the whole mip chain.
    Dds,
    /// One decoded mip as RGBA. Lossless as an encoding, but a projection of
    /// the source format onto four 8-bit channels.
    Png,
}

/// Write one texture to disk as a `.dds` or a `.png`.
#[tauri::command]
pub fn save_texture(
    index: usize,
    level: usize,
    format: TextureExport,
    destination: String,
    viewer: State<'_, ViewerState>,
) -> Result<usize, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let dictionary = open_ytd(open)?;

    let texture = dictionary
        .textures
        .get(index)
        .ok_or_else(|| format!("texture {index} is not in this dictionary"))?;

    let bytes = match format {
        TextureExport::Dds => image::to_dds(texture).map_err(|e| format!("{e}"))?,
        TextureExport::Png => {
            let decoded = image::decode_mip(texture, level).map_err(|e| format!("{e}"))?;
            image::to_png(&decoded).map_err(|e| format!("{e}"))?
        }
    };

    write_file(&destination, &bytes)?;
    Ok(bytes.len())
}

/// One clip in an open `.ycd`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipEntry {
    /// Position in the listing.
    pub index: usize,
    /// The clip's short name, or `hash_XXXXXXXX` when it carries no path.
    pub name: String,
    /// The full authoring path the clip was built from, when it has one.
    pub path: Option<String>,
    /// The hash the game looks this clip up by.
    pub hash: u32,
    /// `animation` or `animation list`.
    pub kind: &'static str,
    pub duration: f32,
    pub rate: f32,
    pub start_time: f32,
    pub end_time: f32,
    /// Hashes of the animations this clip plays, resolvable in `animations`.
    pub animations: Vec<u32>,
    /// Tags and properties are counted, not read: `ClipTag` and
    /// `ClipProperty` are not ported. See `docs/formats.md`.
    pub tag_count: usize,
    pub property_count: usize,
}

/// One animation in an open `.ycd`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationEntry {
    pub index: usize,
    pub hash: u32,
    /// `hash_XXXXXXXX` unless the name happens to be in the reference's table.
    pub name: String,
    pub frames: u16,
    pub duration: f32,
    /// Frames divided by duration, as the file's own numbers give it.
    pub frame_rate: f32,
    pub sequences: usize,
    /// One per animated bone track.
    pub bone_tracks: usize,
    /// How many clips the file says reference this animation.
    pub usage_count: u32,
}

/// What an open `.ycd` contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipPreview {
    pub path: String,
    pub clips: Vec<ClipEntry>,
    pub animations: Vec<AnimationEntry>,
}

/// List the clips and animations in the open `.ycd`.
///
/// The animation *data* is not decoded, here or anywhere: the channel codecs
/// in `Clip.cs` are not ported, and playing an animation would also need a
/// skeleton. This is the listing, which is what the Explorer needs first.
#[tauri::command]
pub fn read_clips(viewer: State<'_, ViewerState>) -> Result<ClipPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let resource = open
        .info
        .resource
        .as_ref()
        .ok_or("this file is not a resource, so it holds no clips")?;
    let dictionary = read_clip_dictionary(open.info.file_kind, &open.data, resource.system_size)
        .map_err(|e| format!("could not read the clips: {e}"))?;

    let clips = dictionary
        .clips
        .iter()
        .enumerate()
        .map(|(index, clip)| ClipEntry {
            index,
            name: clip.display_name(),
            path: clip.name.clone(),
            hash: clip.hash,
            kind: clip.clip_type.label(),
            duration: clip.duration,
            rate: clip.rate,
            start_time: clip.start_time,
            end_time: clip.end_time,
            animations: clip.animations.clone(),
            tag_count: clip.tag_count,
            property_count: clip.property_count,
        })
        .collect();

    let animations = dictionary
        .animations
        .iter()
        .enumerate()
        .map(|(index, animation)| AnimationEntry {
            index,
            hash: animation.hash,
            name: hash_string(animation.hash),
            frames: animation.frames,
            duration: animation.duration,
            frame_rate: animation.frame_rate(),
            sequences: animation.sequence_frames.len(),
            bone_tracks: animation.bone_ids.len(),
            usage_count: animation.usage_count,
        })
        .collect();

    Ok(ClipPreview {
        path: open.info.path.clone(),
        clips,
        animations,
    })
}

/// What an open `.ymt` contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaPreview {
    pub path: String,
    /// `RSC`, `PSO` or `RBF`: which of the three encodings this file uses.
    pub format: String,
    /// The name of the root structure, which is what the file *is*.
    pub root_name: String,
    /// Total nodes, so the view can say how much it is showing.
    pub node_count: usize,
    pub tree: MetaNode,
}

/// Walk the open metadata file into a tree of named values.
#[tauri::command]
pub fn read_meta(viewer: State<'_, ViewerState>) -> Result<MetaPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    // A metadata file is RSC only when the archive says it is a resource; the
    // other two encodings are plain binary entries.
    let system_size = open.info.resource.as_ref().map(|r| r.system_size);
    let meta = read_metadata(open.info.file_kind, &open.data, system_size)
        .map_err(|e| format!("could not read the metadata: {e}"))?;

    let tree = meta.to_tree();
    Ok(MetaPreview {
        path: open.info.path.clone(),
        format: meta.format().to_string(),
        root_name: meta.root_name(),
        node_count: tree.count(),
        tree,
    })
}

/// A typed view of an open `.ymap` or `.ytyp`: the placement dictionary, or
/// the archetype dictionary. This is the model above the generic meta tree —
/// what the world view will need to place things, and what a placement or
/// archetype file actually *means*.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MapPreview {
    Ymap(YmapPreview),
    Ytyp(YtypPreview),
}

/// What a `.ymap` holds: the placed entities as a parent/child tree, and the
/// supporting structures that accompany them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YmapPreview {
    /// The map's name hash, resolved against the name table.
    pub name: String,
    pub name_hash: u32,
    /// The parent map's name hash, resolved — empty for a top-level map.
    pub parent: String,
    pub parent_hash: u32,
    /// `flag 1` is the SCRIPTED flag (destruction maps).
    pub flags: u32,
    pub content_flags: u32,
    pub streaming_extents_min: [f32; 3],
    pub streaming_extents_max: [f32; 3],
    pub entities_extents_min: [f32; 3],
    pub entities_extents_max: [f32; 3],
    /// The entities with no parent in this file, each with its subtree.
    pub roots: Vec<MapEntity>,
    /// Total entities, roots and children together.
    pub entity_count: usize,
    /// How many of `entity_count` are interior instances.
    pub mlo_entities: usize,
    pub car_generators: usize,
    pub time_cycle_modifiers: usize,
    pub box_occluders: usize,
    pub occlude_models: usize,
    pub lod_lights: usize,
    pub distant_lod_lights: usize,
    pub grass_batches: usize,
    pub grass_instances: usize,
    pub strings: usize,
}

/// One placed entity. The `children` repeat this shape, so the whole placement
/// tree crosses in one payload and the viewer renders it collapsed by default.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapEntity {
    /// Position in the file's combined entity list, and the identity for the
    /// tree's keys.
    pub index: usize,
    /// The archetype's name hash, resolved — what this entity *is*.
    pub name: String,
    pub name_hash: u32,
    /// True for a `CMloInstanceDef`, an interior instance.
    pub is_mlo: bool,
    /// The interior entity sets an MLO instance activates, resolved hashes.
    pub default_entity_sets: Vec<String>,
    pub guid: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale_xy: f32,
    pub scale_z: f32,
    /// Index of the parent into the combined entity list, `-1` for a root.
    pub parent_index: i32,
    pub lod_dist: f32,
    pub child_lod_dist: f32,
    /// `HD`, `LOD`, `SLOD1`... as the viewer should label it.
    pub lod_level: String,
    pub priority_level: String,
    pub flags: u32,
    /// The rig an entity carries — a door, a ladder, an audio emitter...
    pub extensions: Vec<MapExtension>,
    pub children: Vec<MapEntity>,
}

/// An extension block on an entity or archetype: the common fields every one of
/// the fifteen extension structures carries, plus which structure it was.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapExtension {
    /// Resolved structure name, e.g. `CExtensionDefDoor`.
    pub structure: String,
    pub structure_hash: u32,
    /// The extension's name hash, resolved.
    pub name: String,
    pub name_hash: u32,
    pub offset_position: [f32; 3],
}

/// What a `.ytyp` holds: its archetypes, discriminated by which block each came
/// from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtypPreview {
    /// The dictionary's own name hash, resolved.
    pub name: String,
    pub name_hash: u32,
    pub archetypes: Vec<YtypArchetype>,
    pub extensions: Vec<MapExtension>,
    pub composite_entity_types: usize,
    pub strings: usize,
}

/// One archetype. `base` holds the shared first half of all three kinds; `mlo`
/// is the interior tail only `CMloArchetypeDef` carries.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtypArchetype {
    pub index: usize,
    /// `base`, `time` or `mlo`.
    pub kind: &'static str,
    /// The archetype's own name hash, resolved.
    pub name: String,
    pub name_hash: u32,
    /// The `assetName` hash — what the engine loads — resolved.
    pub asset_name: String,
    pub asset_hash: u32,
    pub asset_type: String,
    pub drawable_dictionary: String,
    pub texture_dictionary: String,
    pub clip_dictionary: String,
    pub physics_dictionary: String,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub bs_centre: [f32; 3],
    pub bs_radius: f32,
    pub lod_dist: f32,
    pub hd_texture_dist: f32,
    pub flags: u32,
    pub extensions: Vec<MapExtension>,
    /// Only a `time` archetype has one.
    pub time_flags: Option<u32>,
    /// Only an `mlo` archetype has one.
    pub mlo: Option<YtypMlo>,
}

/// The interior tail of an `mlo` archetype: rooms, portals, entity sets, and
/// the entities that make it up.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtypMlo {
    pub flags: u32,
    pub rooms: Vec<MloRoomPreview>,
    pub portals: Vec<MloPortalPreview>,
    pub entity_sets: Vec<MloEntitySetPreview>,
    pub entities: Vec<MapEntity>,
    pub time_cycle_modifiers: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloRoomPreview {
    pub index: usize,
    pub name: String,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub blend: f32,
    pub timecycle_name: String,
    pub flags: u32,
    pub portal_count: u32,
    pub floor_id: i32,
    pub attached_objects: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloPortalPreview {
    pub index: usize,
    pub room_from: u32,
    pub room_to: u32,
    pub flags: u32,
    pub mirror_priority: u32,
    pub opacity: u32,
    pub audio_occlusion: u32,
    pub corners: usize,
    pub attached_objects: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloEntitySetPreview {
    pub index: usize,
    /// The set's name hash, resolved — how a script turns an interior room's
    /// contents on and off.
    pub name: String,
    pub name_hash: u32,
    pub locations: usize,
    pub entities: usize,
}

/// Read the open `.ymap` or `.ytyp` as the typed model, not the generic tree.
#[tauri::command]
pub fn read_map(viewer: State<'_, ViewerState>) -> Result<MapPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let name_lower = open.info.name.to_ascii_lowercase();
    if !name_lower.ends_with(".ymap") && !name_lower.ends_with(".ytyp") {
        return Err(format!("{} is not a placement or archetype dictionary", open.info.name));
    }

    let system_size = open.info.resource.as_ref().map(|r| r.system_size);
    let meta = read_metadata(open.info.file_kind, &open.data, system_size)
        .map_err(|e| format!("could not read the metadata: {e}"))?;

    let preview = match &meta {
        MetaFile::Rsc(meta) => build_map_preview(&name_lower, meta)
            .ok_or("this file holds no typed placement or archetype records")?,
        _ => return Err("not an RSC resource, so there is no typed map to show".into()),
    };
    Ok(preview)
}

/// Build the typed preview for a `.ymap` or `.ytyp`, chosen by the file name.
/// `None` when the reader fails or the file carries nothing to show — which is
/// what lets `choose_viewer` fall back to the generic tree.
fn build_map_preview(name_lower: &str, meta: &Meta) -> Option<MapPreview> {
    if name_lower.ends_with(".ymap") {
        build_ymap_preview(meta)
    } else if name_lower.ends_with(".ytyp") {
        build_ytyp_preview(meta)
    } else {
        None
    }
}

fn build_ymap_preview(meta: &Meta) -> Option<MapPreview> {
    let ymap = read_ymap(meta).ok()?;
    if ymap.entities.is_empty()
        && ymap.car_generators.is_empty()
        && ymap.time_cycle_modifiers.is_empty()
        && ymap.box_occluders.is_empty()
        && ymap.occlude_models.is_empty()
        && ymap.lod_lights.direction.is_empty()
        && ymap.distant_lod_lights.position.is_empty()
        && ymap.grass_instance_batches.is_empty()
    {
        return None;
    }

    let entities = &ymap.entities;
    let roots = ymap
        .root_entities
        .iter()
        .map(|&index| build_map_entity(index, &entities[index], entities))
        .collect();
    let grass_instances: usize = ymap
        .grass_instance_batches
        .iter()
        .map(|batch| batch.instances.len())
        .sum();

    Some(MapPreview::Ymap(YmapPreview {
        name: hash_string(ymap.name),
        name_hash: ymap.name,
        parent: hash_string(ymap.parent),
        parent_hash: ymap.parent,
        flags: ymap.flags,
        content_flags: ymap.content_flags,
        streaming_extents_min: ymap.streaming_extents_min,
        streaming_extents_max: ymap.streaming_extents_max,
        entities_extents_min: ymap.entities_extents_min,
        entities_extents_max: ymap.entities_extents_max,
        roots,
        entity_count: ymap.entities.len(),
        mlo_entities: ymap.mlo_entities.len(),
        car_generators: ymap.car_generators.len(),
        time_cycle_modifiers: ymap.time_cycle_modifiers.len(),
        box_occluders: ymap.box_occluders.len(),
        occlude_models: ymap.occlude_models.len(),
        lod_lights: ymap.lod_lights.direction.len(),
        distant_lod_lights: ymap.distant_lod_lights.position.len(),
        grass_batches: ymap.grass_instance_batches.len(),
        grass_instances,
        strings: ymap.strings.len(),
    }))
}

/// One entity, and its subtree. `entities` is the flat list the hierarchy
/// indices point into; the borrow is immutable for the whole build, so indexing
/// is safe.
fn build_map_entity(index: usize, entity: &cw_core::filetypes::Entity, entities: &[cw_core::filetypes::Entity]) -> MapEntity {
    MapEntity {
        index,
        name: hash_string(entity.def.archetype_name),
        name_hash: entity.def.archetype_name,
        is_mlo: entity.is_mlo,
        default_entity_sets: entity
            .default_entity_sets
            .iter()
            .map(|hash| hash_string(*hash))
            .collect(),
        guid: entity.def.guid,
        position: entity.def.position,
        rotation: entity.def.rotation,
        scale_xy: entity.def.scale_xy,
        scale_z: entity.def.scale_z,
        parent_index: entity.def.parent_index,
        lod_dist: entity.def.lod_dist,
        child_lod_dist: entity.def.child_lod_dist,
        lod_level: lod_level_label(entity.def.lod_level),
        priority_level: priority_level_label(entity.def.priority_level),
        flags: entity.def.flags,
        extensions: entity.extensions.iter().map(build_extension).collect(),
        children: entity
            .children
            .iter()
            .map(|&child| build_map_entity(child, &entities[child], entities))
            .collect(),
    }
}

fn build_ytyp_preview(meta: &Meta) -> Option<MapPreview> {
    let ytyp = read_ytyp(meta).ok()?;
    if ytyp.archetypes.is_empty() {
        return None;
    }

    Some(MapPreview::Ytyp(YtypPreview {
        name: hash_string(ytyp.name_hash),
        name_hash: ytyp.name_hash,
        archetypes: ytyp
            .archetypes
            .iter()
            .enumerate()
            .map(|(index, archetype)| build_ytyp_archetype(index, archetype))
            .collect(),
        extensions: ytyp.extensions.iter().map(build_extension).collect(),
        composite_entity_types: ytyp.composite_entity_types.len(),
        strings: ytyp.strings.len(),
    }))
}

fn build_ytyp_archetype(index: usize, archetype: &Archetype) -> YtypArchetype {
    let (kind, base, time_flags, mlo): (&'static str, &cw_core::filetypes::BaseArchetype, Option<u32>, Option<&MloArchetype>) =
        match archetype {
            Archetype::Base(base) => ("base", base, None, None),
            Archetype::Time(time) => ("time", &time.base, Some(time.time_flags), None),
            Archetype::Mlo(mlo) => ("mlo", &mlo.base, None, Some(mlo)),
        };

    YtypArchetype {
        index,
        kind,
        name: hash_string(base.name),
        name_hash: base.name,
        asset_name: hash_string(base.asset_name),
        asset_hash: base.asset_name,
        asset_type: asset_type_label(base.asset_type),
        drawable_dictionary: hash_string(base.drawable_dictionary),
        texture_dictionary: hash_string(base.texture_dictionary),
        clip_dictionary: hash_string(base.clip_dictionary),
        physics_dictionary: hash_string(base.physics_dictionary),
        bb_min: base.bb_min,
        bb_max: base.bb_max,
        bs_centre: base.bs_centre,
        bs_radius: base.bs_radius,
        lod_dist: base.lod_dist,
        hd_texture_dist: base.hd_texture_dist,
        flags: base.flags,
        extensions: base.extensions.iter().map(build_extension).collect(),
        time_flags,
        mlo: mlo.map(build_mlo),
    }
}

fn build_mlo(mlo: &MloArchetype) -> YtypMlo {
    YtypMlo {
        flags: mlo.mlo_flags,
        rooms: mlo
            .rooms
            .iter()
            .enumerate()
            .map(|(index, room)| MloRoomPreview {
                index,
                name: room.name.clone().unwrap_or_default(),
                bb_min: room.bb_min,
                bb_max: room.bb_max,
                blend: room.blend,
                timecycle_name: hash_string(room.timecycle_name),
                flags: room.flags,
                portal_count: room.portal_count,
                floor_id: room.floor_id,
                attached_objects: room.attached_objects.len(),
            })
            .collect(),
        portals: mlo
            .portals
            .iter()
            .enumerate()
            .map(|(index, portal)| MloPortalPreview {
                index,
                room_from: portal.room_from,
                room_to: portal.room_to,
                flags: portal.flags,
                mirror_priority: portal.mirror_priority,
                opacity: portal.opacity,
                audio_occlusion: portal.audio_occlusion,
                corners: portal.corners.len(),
                attached_objects: portal.attached_objects.len(),
            })
            .collect(),
        entity_sets: mlo
            .entity_sets
            .iter()
            .enumerate()
            .map(|(index, set)| MloEntitySetPreview {
                index,
                name: hash_string(set.name),
                name_hash: set.name,
                locations: set.locations.len(),
                entities: set.entities.len(),
            })
            .collect(),
        entities: mlo
            .entities
            .iter()
            .enumerate()
            .map(|(index, entity)| build_mlo_entity(index, entity))
            .collect(),
        time_cycle_modifiers: mlo.time_cycle_modifiers.len(),
    }
}

/// An entity inside an MLO has no placement children of its own — it is part of
/// the interior's fixed inventory — so the subtree is always empty.
fn build_mlo_entity(index: usize, entity: &MloEntity) -> MapEntity {
    MapEntity {
        index,
        name: hash_string(entity.entity.archetype_name),
        name_hash: entity.entity.archetype_name,
        is_mlo: false,
        default_entity_sets: Vec::new(),
        guid: entity.entity.guid,
        position: entity.entity.position,
        rotation: entity.entity.rotation,
        scale_xy: entity.entity.scale_xy,
        scale_z: entity.entity.scale_z,
        parent_index: entity.entity.parent_index,
        lod_dist: entity.entity.lod_dist,
        child_lod_dist: entity.entity.child_lod_dist,
        lod_level: lod_level_label(entity.entity.lod_level),
        priority_level: priority_level_label(entity.entity.priority_level),
        flags: entity.entity.flags,
        extensions: entity.extensions.iter().map(build_extension).collect(),
        children: Vec::new(),
    }
}

fn build_extension(extension: &Extension) -> MapExtension {
    MapExtension {
        structure: hash_string(extension.structure),
        structure_hash: extension.structure,
        name: hash_string(extension.name),
        name_hash: extension.name,
        offset_position: extension.offset_position,
    }
}

fn lod_level_label(level: LodLevel) -> String {
    match level {
        LodLevel::Hd => "HD".into(),
        LodLevel::Lod => "LOD".into(),
        LodLevel::Slod1 => "SLOD1".into(),
        LodLevel::Slod2 => "SLOD2".into(),
        LodLevel::Slod3 => "SLOD3".into(),
        LodLevel::OrphanHd => "ORPHAN_HD".into(),
        LodLevel::Slod4 => "SLOD4".into(),
        LodLevel::Unknown(value) => format!("{value}"),
    }
}

fn priority_level_label(level: PriorityLevel) -> String {
    match level {
        PriorityLevel::Required => "Required".into(),
        PriorityLevel::OptionalHigh => "OptionalHigh".into(),
        PriorityLevel::Medium => "Medium".into(),
        PriorityLevel::Low => "Low".into(),
        PriorityLevel::Unknown(value) => format!("{value}"),
    }
}

fn asset_type_label(asset: AssetType) -> String {
    match asset {
        AssetType::Uninitialised => "uninitialised".into(),
        AssetType::AssetFragment => "fragment".into(),
        AssetType::AssetDrawable => "drawable".into(),
        AssetType::AssetDrawableDictionary => "drawable dictionary".into(),
        AssetType::AssetTxd => "texture dictionary".into(),
        AssetType::AssetBounds => "bounds".into(),
        AssetType::AssetNavMesh => "navmesh".into(),
        AssetType::AssetTimeCycle => "timecycle".into(),
        AssetType::AssetClipDictionary => "clip dictionary".into(),
        AssetType::AssetCollision => "collision".into(),
        AssetType::AssetCluster => "cluster".into(),
        AssetType::AssetLodLight => "lod light".into(),
        AssetType::AssetCompositeEntityType => "composite entity type".into(),
        AssetType::AssetPropBatch => "prop batch".into(),
        AssetType::AssetVertexShader => "vertex shader".into(),
        AssetType::AssetPixelShader => "pixel shader".into(),
        AssetType::AssetTexture => "texture".into(),
        AssetType::AssetDrawableAtlas => "drawable atlas".into(),
        AssetType::Unknown(value) => format!("unknown ({value})"),
    }
}

/// One stream in an open `.awc`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    /// Position in the container, and the handle for `read_audio_track`.
    pub index: usize,
    /// The 29-bit name hash. Resolving it to a name needs the name tables from
    /// `MetaNames.cs`, which are not ported.
    pub id: u32,
    pub codec: &'static str,
    pub sample_rate: u16,
    pub samples: u32,
    pub seconds: f32,
    pub playable: bool,
    /// True when the stream is MIDI rather than audio.
    pub midi: bool,
    /// The chunk kinds this stream carries, for the detail line.
    pub chunks: Vec<&'static str>,
}

/// What an open `.awc` contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPreview {
    pub path: String,
    pub version: u16,
    pub multi_channel: bool,
    pub whole_file_encrypted: bool,
    pub tracks: Vec<AudioTrack>,
}

/// One decoded stream, as a `.wav`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioClip {
    pub index: usize,
    pub sample_rate: u16,
    pub samples: usize,
    /// Base64 of a complete RIFF/WAVE file, ready for an `<audio>` element.
    pub wav: String,
}

/// Parse the open file as an AWC. Kept private so both audio commands agree on
/// what "the open file" means.
fn open_awc(open: &OpenFile) -> Result<AwcFile, String> {
    if !open.info.file_kind.has_audio() {
        return Err(format!("{} is not an audio container", open.info.name));
    }
    AwcFile::read(&open.data, &open.awc_key).map_err(|e| format!("{e}"))
}

/// List the streams in the open `.awc`.
///
/// The waves themselves are not decoded here: a container runs to hundreds of
/// streams and tens of megabytes, and the listing is what the UI needs first.
#[tauri::command]
pub fn read_audio(viewer: State<'_, ViewerState>) -> Result<AudioPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let awc = open_awc(open)?;

    let tracks = awc
        .streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| !stream.is_carrier)
        .map(|(index, stream)| AudioTrack {
            index,
            id: stream.id,
            codec: stream.format.map(|f| f.codec.label()).unwrap_or("none"),
            sample_rate: stream.format.map(|f| f.sample_rate).unwrap_or(0),
            samples: stream.format.map(|f| f.samples).unwrap_or(0),
            seconds: stream.format.map(|f| f.seconds()).unwrap_or(0.0),
            playable: stream.is_playable(),
            midi: stream.midi.is_some(),
            chunks: stream.chunks.iter().map(|c| c.kind.label()).collect(),
        })
        .collect();

    Ok(AudioPreview {
        path: open.info.path.clone(),
        version: awc.version,
        multi_channel: awc.multi_channel,
        whole_file_encrypted: awc.whole_file_encrypted,
        tracks,
    })
}

/// Decode one stream to a `.wav`.
///
/// ADPCM is decoded in Rust rather than in the WebView, per the boundary rule:
/// browsers do not play Rockstar's ADPCM framing, and shipping the raw blocks
/// across would be handing the frontend archive bytes to parse.
#[tauri::command]
pub fn read_audio_track(index: usize, viewer: State<'_, ViewerState>) -> Result<AudioClip, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let awc = open_awc(open)?;

    let stream = awc
        .streams
        .get(index)
        .ok_or_else(|| format!("stream {index} is not in this container"))?;
    if !stream.is_playable() {
        return Err(format!("stream {index} holds no playable audio"));
    }

    let samples = stream.samples();
    Ok(AudioClip {
        index,
        sample_rate: stream.format.map(|f| f.sample_rate).unwrap_or(0),
        samples: samples.len(),
        wav: STANDARD.encode(stream.to_wav()),
    })
}

/// Write the open file's payload to `destination`, as extracted — decrypted and
/// decompressed, which is what every other tool expects a `.ydr` on disk to be.
#[tauri::command]
pub fn extract_to(destination: String, viewer: State<'_, ViewerState>) -> Result<usize, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    write_file(&destination, &open.data)?;
    Ok(open.data.len())
}

/// Write one decoded stream to disk as a `.wav`.
#[tauri::command]
pub fn save_audio_track(
    index: usize,
    destination: String,
    viewer: State<'_, ViewerState>,
) -> Result<usize, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let awc = open_awc(open)?;

    let stream = awc
        .streams
        .get(index)
        .ok_or_else(|| format!("stream {index} is not in this container"))?;
    let wav = stream.to_wav();
    write_file(&destination, &wav)?;
    Ok(wav.len())
}

/// The encoding an edited file should be written back in. Mirrors
/// [`cw_core::text::Encoding`] on the wire.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

impl From<WireEncoding> for Encoding {
    fn from(value: WireEncoding) -> Self {
        match value {
            WireEncoding::Utf8 => Encoding::Utf8,
            WireEncoding::Utf8Bom => Encoding::Utf8Bom,
            WireEncoding::Utf16Le => Encoding::Utf16Le,
            WireEncoding::Utf16Be => Encoding::Utf16Be,
        }
    }
}

/// Write edited text to disk in the encoding it was read as.
///
/// To disk, not back into the archive: RPF *writing* is not ported — see
/// `docs/formats.md`. The UI says so rather than offering a save that would
/// quietly do something else.
#[tauri::command]
pub fn save_text(
    destination: String,
    contents: String,
    encoding: WireEncoding,
) -> Result<usize, String> {
    let bytes = text::encode(&contents, encoding.into());
    write_file(&destination, &bytes)?;
    Ok(bytes.len())
}

fn write_file(destination: &str, bytes: &[u8]) -> Result<(), String> {
    let path = PathBuf::from(destination);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(system_size: usize) -> ResourceInfo {
        ResourceInfo {
            system_size,
            graphics_size: 0,
            version: 165,
        }
    }

    /// The AWC key only matters for `.awc`, which none of these cases are.
    fn choose_viewer_for_test(
        kind: FileKind,
        data: &[u8],
        resource: Option<&ResourceInfo>,
    ) -> (&'static str, Vec<&'static str>, Option<String>) {
        choose_viewer(kind, "", data, resource, &[0; 4])
    }

    /// The name only matters for the metadata dispatch — a `.ymap` or `.ytyp`
    /// gets the typed map viewer. Same AWC key caveat as the helper above.
    fn choose_viewer_for_name(
        kind: FileKind,
        name: &str,
        data: &[u8],
        resource: Option<&ResourceInfo>,
    ) -> (&'static str, Vec<&'static str>, Option<String>) {
        choose_viewer(kind, name, data, resource, &[0; 4])
    }

    #[test]
    fn offers_text_for_a_meta_file_and_hex_as_a_fallback() {
        let xml = b"<?xml version=\"1.0\"?>\n<CVehicleModelInfo />\n";
        let (viewer, viewers, note) = choose_viewer_for_test(FileKind::Text, xml, None);
        assert_eq!(viewer, "text");
        assert_eq!(viewers, vec!["text", "hex"]);
        assert!(note.is_none());
    }

    /// Hex is always available, whatever else is.
    #[test]
    fn always_offers_hex() {
        for (kind, data) in [
            (FileKind::Text, b"plain".as_slice()),
            (FileKind::Data, &[0u8, 1, 2, 3]),
            (FileKind::Unknown, &[0xFF; 64]),
        ] {
            let (_, viewers, _) = choose_viewer_for_test(kind, data, None);
            assert!(viewers.contains(&"hex"), "{kind:?} does not offer hex");
        }
    }

    /// A `.ydr` whose bytes are not a drawable must say why rather than opening
    /// an empty 3D view.
    #[test]
    fn explains_a_model_that_will_not_parse() {
        let junk = vec![0xFFu8; 64];
        let (viewer, _, note) =
            choose_viewer_for_test(FileKind::Model, &junk, Some(&resource(4096)));
        assert_eq!(viewer, "hex");
        let note = note.expect("a reason");
        assert!(note.contains("geometry"), "unhelpful note: {note}");
    }

    /// A `.ydr` that parses without error but carries nothing to draw is the
    /// common failure, not a parse error: the reader zero-fills past the end of
    /// a short payload, so junk reads as a drawable with no models.
    #[test]
    fn does_not_open_the_3d_view_for_a_drawable_with_no_geometry() {
        let empty = vec![0u8; 512];
        let (viewer, viewers, note) =
            choose_viewer_for_test(FileKind::Model, &empty, Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"model"));
        assert!(note.unwrap().contains("renderable"));
    }

    /// A `.yft` whose bytes hold no renderable drawable must say so rather than
    /// opening an empty 3D view. Zeros parse into a fragment with nothing in it,
    /// the same way they parse into a drawable with no models.
    #[test]
    fn does_not_open_the_3d_view_for_a_fragment_with_no_geometry() {
        let empty = vec![0u8; 512];
        let (viewer, viewers, note) =
            choose_viewer_for_test(FileKind::Fragment, &empty, Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"model"));
        assert!(note.unwrap().contains("renderable"));
    }

    /// A `.gxt2` that parses opens the strings viewer.
    #[test]
    fn opens_the_strings_view_for_a_gxt2_table() {
        let mut data = Vec::new();
        data.extend_from_slice(&1196971058u32.to_le_bytes()); // "GXT2"
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&0xABCD1234u32.to_le_bytes());
        data.extend_from_slice(&24u32.to_le_bytes()); // offset of the text
        data.extend_from_slice(&1196971058u32.to_le_bytes());
        data.extend_from_slice(&30u32.to_le_bytes()); // end of the blob
        data.extend_from_slice(b"hola\0");
        let (viewer, viewers, note) = choose_viewer_for_test(FileKind::Gxt2, &data, None);
        assert_eq!(viewer, "gxt2");
        assert!(viewers.contains(&"gxt2"));
        assert!(note.is_none());
    }

    /// A `.gxt2` whose first magic is missing is not a text table, and must say
    /// so rather than opening an empty strings view.
    #[test]
    fn does_not_open_the_strings_view_for_bytes_without_the_magic() {
        let junk = vec![0u8; 64];
        let (viewer, viewers, note) = choose_viewer_for_test(FileKind::Gxt2, &junk, None);
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"gxt2"));
        assert!(note.unwrap().contains("text table"));
    }

    /// A `.ytd` whose bytes hold no decodable texture must say so rather than
    /// opening an empty gallery. Zeros parse into an empty dictionary, the same
    /// way they parse into a drawable with no models.
    #[test]
    fn does_not_open_the_texture_view_for_an_empty_dictionary() {
        let empty = vec![0u8; 512];
        let (viewer, viewers, note) =
            choose_viewer_for_test(FileKind::Texture, &empty, Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"texture"));
        assert!(note.unwrap().contains("no textures"));
    }

    /// A `.ycd` whose bytes hold neither a clip nor an animation must say so
    /// rather than opening an empty listing. Zeros parse into an empty
    /// dictionary, the same way they parse into a drawable with no models.
    #[test]
    fn does_not_open_the_clip_view_for_an_empty_dictionary() {
        let empty = vec![0u8; 512];
        let (viewer, viewers, note) =
            choose_viewer_for_test(FileKind::ClipDictionary, &empty, Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"clip"));
        assert!(note.unwrap().contains("no clips"));
    }

    /// A `.ymt` that is neither a resource nor one of the two binary encodings
    /// has to say so: it is a metadata file the reader does not recognise, not
    /// an empty one.
    #[test]
    fn does_not_open_the_metadata_view_for_bytes_with_no_container() {
        let junk: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        let (viewer, viewers, note) = choose_viewer_for_test(FileKind::Meta, &junk, None);
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"meta"));
        assert!(note.unwrap().contains("could not read the metadata"));
    }

    /// An RBF payload is a plain binary entry, so the metadata view has to open
    /// without any resource information.
    #[test]
    fn opens_the_metadata_view_for_a_binary_entry() {
        // `RBF0`, then a descriptor for an element named `root` with one
        // uint child, then the two close markers.
        let mut rbf = b"RBF0".to_vec();
        rbf.extend_from_slice(&[0, 0, 4, 0]);
        rbf.extend_from_slice(b"root");
        rbf.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        rbf.extend_from_slice(&[1, 0x10, 5, 0]);
        rbf.extend_from_slice(b"count");
        rbf.extend_from_slice(&7u32.to_le_bytes());
        rbf.extend_from_slice(&[0xFF, 0xFF]);

        let (viewer, viewers, note) = choose_viewer_for_test(FileKind::Meta, &rbf, None);
        assert_eq!(viewer, "meta", "{note:?}");
        assert!(viewers.contains(&"meta"));
    }

    /// The synthetic payload builders below mirror the ones in
    /// `cw-core`'s own `ytyp`/`ymap` tests: a minimal RSC meta whose blocks and
    /// pointers carry one typed record.
    fn put_u16(d: &mut [u8], at: usize, v: u16) {
        d[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u32(d: &mut [u8], at: usize, v: u32) {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u64(d: &mut [u8], at: usize, v: u64) {
        d[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn put_i32(d: &mut [u8], at: usize, v: i32) {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn addr(o: u64) -> u64 {
        cw_core::resources::reader::SYSTEM_BASE + o
    }

    /// A `.ytyp` with one base archetype: the minimal meta a typed reader needs.
    fn synthetic_ytyp() -> Vec<u8> {
        let mut d = vec![0u8; 0x900];
        put_u32(&mut d, 0x1C, 1); // root block index
        put_u64(&mut d, 0x20, addr(0x300)); // structures
        put_u64(&mut d, 0x30, addr(0x500)); // blocks
        put_u16(&mut d, 0x48, 2); // structure count
        put_u16(&mut d, 0x4C, 3); // block count

        put_u32(&mut d, 0x300, cw_core::filetypes::names::C_MAP_TYPES);
        put_u32(&mut d, 0x320, cw_core::filetypes::names::C_BASE_ARCHETYPE_DEF);

        // Block 1: CMapTypes, payload at 0x700.
        put_u32(&mut d, 0x500, cw_core::filetypes::names::C_MAP_TYPES);
        put_u32(&mut d, 0x504, 80);
        put_u64(&mut d, 0x508, addr(0x700));
        // Block 2: POINTER block holding one archetype pointer, at 0x740.
        put_u32(&mut d, 0x510, cw_core::meta::rsc::POINTER_BLOCK);
        put_u32(&mut d, 0x514, 8);
        put_u64(&mut d, 0x518, addr(0x740));
        // Block 3: CBaseArchetypeDef, payload at 0x760.
        put_u32(&mut d, 0x520, cw_core::filetypes::names::C_BASE_ARCHETYPE_DEF);
        put_u32(&mut d, 0x524, 144);
        put_u64(&mut d, 0x528, addr(0x760));

        // CMapTypes payload at 0x700: archetypes array at offset 24 -> block 2.
        put_u64(&mut d, 0x700 + 24, 2);
        put_u16(&mut d, 0x700 + 24 + 8, 1);
        put_u16(&mut d, 0x700 + 24 + 10, 1);
        put_u32(&mut d, 0x700 + 40, 0xABBA); // name hash

        // POINTER block payload: one pointer to block 3, offset 0.
        put_u64(&mut d, 0x740, 3);

        // Base archetype payload at 0x760.
        put_u32(&mut d, 0x760 + 8, 60); // lodDist
        put_u32(&mut d, 0x760 + 88, 0x1111); // name
        put_u32(&mut d, 0x760 + 112, 0x2222); // assetName
        put_u32(&mut d, 0x760 + 108, 2); // assetType drawable
        put_u32(&mut d, 0x760 + 100, 0x3333); // drawableDictionary

        d
    }

    /// A `.ymap` with one parent entity: three blocks, one entity record.
    fn synthetic_ymap() -> Vec<u8> {
        let mut d = vec![0u8; 0x900];
        put_u32(&mut d, 0x1C, 1); // root block
        put_u64(&mut d, 0x20, addr(0x300)); // structures
        put_u64(&mut d, 0x30, addr(0x500)); // blocks
        put_u16(&mut d, 0x48, 2); // structure count
        put_u16(&mut d, 0x4C, 3); // block count

        put_u32(&mut d, 0x300, cw_core::filetypes::names::C_MAP_DATA);
        put_u32(&mut d, 0x320, cw_core::filetypes::names::C_ENTITY_DEF);

        // Block 1 CMapData @0x700, block 2 POINTER @0x740, block 3 CEntityDef @0x760.
        put_u32(&mut d, 0x500, cw_core::filetypes::names::C_MAP_DATA);
        put_u32(&mut d, 0x504, 512);
        put_u64(&mut d, 0x508, addr(0x700));
        put_u32(&mut d, 0x510, cw_core::meta::rsc::POINTER_BLOCK);
        put_u32(&mut d, 0x514, 8);
        put_u64(&mut d, 0x518, addr(0x740));
        put_u32(&mut d, 0x520, cw_core::filetypes::names::C_ENTITY_DEF);
        put_u32(&mut d, 0x524, 256);
        put_u64(&mut d, 0x528, addr(0x760));

        // CMapData: name, entities array at 96 -> block 2, count 1.
        put_u32(&mut d, 0x700 + 8, 0xCAFE);
        put_u64(&mut d, 0x700 + 96, 2);
        put_u16(&mut d, 0x700 + 96 + 8, 1);
        put_u16(&mut d, 0x700 + 96 + 10, 1);
        put_u32(&mut d, 0x700 + 96 + 12, 0);

        // POINTER block: one pointer to block 3 at offset 0.
        put_u64(&mut d, 0x740, 3);

        // Entity 0: a root with an archetype hash.
        put_u32(&mut d, 0x760 + 8, 0xAAAA); // archetypeName
        put_i32(&mut d, 0x760 + 72, -1); // parentIndex root
        put_i32(&mut d, 0x760 + 84, 0); // lodLevel HD

        d
    }

    /// A `.ytyp` that parses into typed archetypes opens the map viewer, not
    /// the generic XML tree.
    #[test]
    fn opens_the_map_view_for_a_ytyp_that_parses() {
        let (viewer, viewers, note) = choose_viewer_for_name(
            FileKind::Meta,
            "test.ytyp",
            &synthetic_ytyp(),
            Some(&resource(0x900)),
        );
        assert_eq!(viewer, "map", "{note:?}");
        assert!(viewers.contains(&"map"));
        assert!(viewers.contains(&"meta"), "the tree stays offered as a tab");
    }

    /// The same, for a `.ymap`.
    #[test]
    fn opens_the_map_view_for_a_ymap_that_parses() {
        let (viewer, viewers, note) = choose_viewer_for_name(
            FileKind::Meta,
            "test.ymap",
            &synthetic_ymap(),
            Some(&resource(0x900)),
        );
        assert_eq!(viewer, "map", "{note:?}");
        assert!(viewers.contains(&"map"));
    }

    /// A `.ymap` that parses as an RSC resource but holds no typed records must
    /// fall back to the generic tree rather than opening an empty map.
    #[test]
    fn does_not_open_the_map_view_for_a_ymap_with_no_entities() {
        let (viewer, viewers, note) = choose_viewer_for_name(
            FileKind::Meta,
            "test.ymap",
            &vec![0u8; 0x900],
            Some(&resource(0x900)),
        );
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"map"));
        assert!(note.is_some());
    }

    /// Only the two placement formats get the map viewer: a `.ymt` of the same
    /// bytes stays on the generic tree.
    #[test]
    fn only_the_placement_formats_get_the_map_view() {
        let (viewer, _, _) = choose_viewer_for_name(
            FileKind::Meta,
            "test.ymt",
            &synthetic_ytyp(),
            Some(&resource(0x900)),
        );
        assert_ne!(viewer, "map");
    }

    /// A `.ytyp` stored as a binary entry has no resource split point, so the
    /// typed readers cannot run and the file stays on the generic tree.
    #[test]
    fn does_not_open_the_map_view_for_a_binary_ytyp() {
        let (viewer, viewers, _) =
            choose_viewer_for_name(FileKind::Meta, "test.ytyp", &synthetic_ytyp(), None);
        assert_ne!(viewer, "map");
        assert!(!viewers.contains(&"map"));
    }

    /// `.yed` and its siblings are animation files with no ported reader, so
    /// they must fall back with a reason rather than trying the `.ycd` path.
    #[test]
    fn does_not_treat_the_other_animation_formats_as_clip_dictionaries() {
        assert!(!FileKind::from_name("x.yed").has_clips());
        let (viewer, _, note) =
            choose_viewer_for_test(FileKind::Clip, &vec![0u8; 512], Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(note.is_some());
    }

    /// A `.ytd` stored as a binary entry has no page flags, so the payload
    /// cannot be split into its two segments.
    #[test]
    fn refuses_a_texture_dictionary_with_no_resource_header() {
        let (viewer, _, note) = choose_viewer_for_test(FileKind::Texture, &[0u8; 256], None);
        assert_eq!(viewer, "hex");
        assert!(note.unwrap().contains("binary entry"));
    }

    /// The formats that are not ported yet each get a note, so "opens as hex"
    /// never looks like a bug.
    #[test]
    fn names_the_formats_that_are_not_ported() {
        for kind in [
            FileKind::Fragment,
            FileKind::Particle,
            FileKind::Meta,
            FileKind::Bounds,
        ] {
            let binary = vec![0u8; 512];
            let (viewer, _, note) = choose_viewer_for_test(kind, &binary, Some(&resource(512)));
            assert_eq!(viewer, "hex", "{kind:?}");
            assert!(note.is_some(), "{kind:?} has no explanation");
        }
    }

    /// A `.ydr` stored as a binary entry has no page flags, so there is no way
    /// to split the payload. That is a real case in mod archives.
    #[test]
    fn refuses_a_drawable_with_no_resource_header() {
        let (viewer, _, note) = choose_viewer_for_test(FileKind::Model, &[0u8; 256], None);
        assert_eq!(viewer, "hex");
        assert!(note.unwrap().contains("binary entry"));
    }
}
