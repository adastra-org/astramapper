//! World Mode's IPC surface: what content exists, and how to place it.
//!
//! Phase 3 renders the world in three.js in the WebView (see
//! `docs/architecture.md`), so this side has two jobs. It groups every archive
//! by the content it belongs to — `base` for the root `x64a..w.rpf` set, one
//! group per DLC pack, which is the `patchday*` selection the phase asks for —
//! and it reads a `.ymap` into flattened world-space placements.
//!
//! The boundary rule from `docs/architecture.md` holds: the frontend receives
//! already-decoded placements (positions, quaternions, scales, LOD levels),
//! never raw archive bytes.
//!
//! Unlike `viewer.rs`, these commands deliberately read past the one-open-file
//! state: placing several maps means parsing several `.ymap` files, and the
//! Explorer's `ViewerState` holds exactly one. `world_map` parses from the
//! `GameState` index on demand instead, so the two modes never fight over the
//! open file.
//!
//! Every command that touches an archive is `async` and does its work through
//! [`blocking`], because a synchronous `#[tauri::command]` runs on the event
//! loop and the streamer issues these dozens at a time. See [`blocking`] for
//! the one rule that keeps that safe.

use cw_core::filetype::FileKind;
use cw_core::filetypes::{
    expand_mlo_entities, index_mlo_archetypes, read_ymap, ArchetypeIndexes, MloArchetype,
};
use cw_core::mesh::MeshBundle;
use cw_core::meta::{read_meta as read_metadata, LodLevel, MetaFile};
use cw_core::resources::{read_named_drawables, read_named_fragments, TextureDictionary};
use cw_core::rpf::{size_from_flags, RpfManager};
use cw_core::utils::{hash_string, GtaKeys};
use cw_core::wire::{Envelope, Kind};
use serde::Serialize;
use tauri::State;

use crate::explorer::GameState;
use crate::viewer::{ModelDrawable, ModelPreview, ViewerState};

/// How many `.ymap` files one group listing hands over. A city group can name
/// thousands; the UI shows this many and the total.
const MAX_YMAPS_PER_GROUP: usize = 200;

/// Run a command's real work off the event loop.
///
/// A `#[tauri::command]` that is not `async` runs on the main thread, so every
/// archive read, inflate and texture decode in this module used to serialize
/// there and block the WebView. Streaming issues these dozens at a time, so
/// they have to actually overlap.
///
/// **The invariant that makes this safe: no lock guard may cross the `.await`.**
/// Take what you need out of [`GameState`] with [`GameState::loaded`] first,
/// which hands back an owned `Arc<Loaded>` and drops the read guard before
/// returning, then move that `Arc` into the closure. A `RwLockReadGuard` is
/// neither `Send` nor `'static` and will not compile here, which is the point.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("the task failed: {e}"))?
}

/// One content group: the archives that make up a slice of the install.
///
/// The id is the stable key the UI toggles and asks `world_dlc_ymaps` for. The
/// label is the same string — a group's identity is its name, not a
/// presentation of it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentGroup {
    pub id: String,
    pub label: String,
    /// Archives in the group, nested ones counted.
    pub archives: usize,
    /// Entries across those archives.
    pub entries: usize,
}

/// What the world view needs before the user places anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStatus {
    pub game_open: bool,
    pub folder: Option<String>,
    pub dlc_groups: Vec<ContentGroup>,
    /// The `.ymap` open in the Explorer, when there is one — so the world view
    /// can offer to place the map the user was just looking at.
    pub open_map_path: Option<String>,
}

/// One `.ymap` a content group holds.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YmapListing {
    /// Full archive path, the handle for `world_map`.
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YmapListingPage {
    pub total: usize,
    /// The first `MAX_YMAPS_PER_GROUP`, sorted by path.
    pub ymaps: Vec<YmapListing>,
}

/// One placed entity in the world, flattened from the `.ymap`'s entity list.
///
/// `children` is a count, not a subtree: every entity is placed at its own
/// stored position, and the hierarchy is for LOD reasoning, not transforms.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldEntity {
    /// Position in the file's combined entity list.
    pub index: usize,
    /// The archetype's name hash, resolved.
    pub name: String,
    pub name_hash: u32,
    /// True for a `CMloInstanceDef`, an interior instance.
    pub is_mlo: bool,
    /// True for an interior entity expanded out of an MLO archetype, into the
    /// world at its instance's transform.
    pub from_mlo: bool,
    pub position: [f32; 3],
    /// The stored quaternion, as the file carries it — the world view hands it
    /// to three.js unchanged. Whether the stored form needs conjugating to
    /// match the reference's world orientation is open; the gate should pin it
    /// before real geometry is drawn.
    pub rotation: [f32; 4],
    pub scale_xy: f32,
    pub scale_z: f32,
    /// Index of the parent into the combined list, `-1` for a root.
    pub parent_index: i32,
    /// `HD`, `LOD`, `SLOD1`... as the viewer should label it.
    pub lod_level: String,
    /// The entity's own LOD distance — the boundary the streamer uses to switch
    /// HD geometry off and the LOD on.
    pub lod_dist: f32,
    pub children: usize,
}

/// A `.ymap` read for the world: the map's own fields plus every entity.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldMap {
    pub path: String,
    pub name: String,
    pub name_hash: u32,
    pub parent: String,
    pub entity_count: usize,
    pub mlo_entities: usize,
    /// The declared entity extents. The viewer frames from the actual
    /// positions, which survive when a map declares degenerate extents.
    pub extents_min: [f32; 3],
    pub extents_max: [f32; 3],
    pub entities: Vec<WorldEntity>,
    /// How many interior entities were expanded out of the map's MLO instances.
    pub mlo_expanded: usize,
}

/// The content group an archive belongs to, from its lowercase path.
///
/// The root archives group as `base`; a DLC pack — `update\x64\dlcpacks\*` or
/// `x64\dlcpacks\*` — groups by pack name, which is what `patchday*` selection
/// means; anything else groups by its directory. `base` sorts before the packs
/// because `BTreeMap` keys the content groups in the UI's desired order.
fn group_key(path: &str) -> String {
    let parts: Vec<&str> = path.split('\\').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["update", "x64", "dlcpacks", pack, ..] => pack.to_string(),
        ["x64", "dlcpacks", pack, ..] => pack.to_string(),
        [_name] => "base".to_string(),
        [dir, ..] => dir.to_string(),
        [] => "base".to_string(),
    }
}

/// Every archive's group, indexed like `manager.archives`: a nested archive
/// inherits its top-level ancestor's group.
fn archive_groups(manager: &RpfManager) -> Vec<String> {
    let mut groups: Vec<String> = Vec::with_capacity(manager.archives.len());
    for (index, parent) in manager.archive_parent.iter().enumerate() {
        let id = match parent {
            Some(parent) => groups[*parent].clone(),
            None => group_key(&manager.archives[index].path),
        };
        groups.push(id);
    }
    groups
}

/// The content groups of an install, sorted with `base` first. Pure of Tauri
/// state so the gate can drive it against a real scan; see `tests/world_gate.rs`.
pub fn content_groups(manager: &RpfManager) -> Vec<ContentGroup> {
    let labels = archive_groups(manager);
    let mut by_id: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();
    for (archive, label) in manager.archives.iter().zip(&labels) {
        let entry = by_id.entry(label.clone()).or_default();
        entry.0 += 1;
        entry.1 += archive.entries.len();
    }
    by_id
        .into_iter()
        .map(|(id, (archives, entries))| ContentGroup {
            id: id.clone(),
            label: id,
            archives,
            entries,
        })
        .collect()
}

/// The `.ymap` files a content group holds, sorted by path. Pure for the gate.
pub fn ymaps_in_group(manager: &RpfManager, group: &str) -> Vec<(String, String)> {
    let labels = archive_groups(manager);
    let mut found: Vec<(String, String)> = Vec::new();
    for (archive, label) in manager.archives.iter().zip(&labels) {
        if label != group {
            continue;
        }
        for entry in &archive.entries {
            if entry.name_lower.ends_with(".ymap") {
                found.push((entry.path.clone(), entry.name.clone()));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The best model file for an archetype name, or `None`.
///
/// A placed entity names its archetype; the engine loads that archetype's
/// `assetName`, which is a file in the archives. The `RpfManager` keeps a
/// per-stem asset index (see [`RpfManager::assets`]) that prefers an `update\`
/// copy over the base game and a `.ydr` over a `.yft` over a `.ydd`, so this
/// is a lookup rather than a tree walk. Names the name table cannot resolve
/// (`hash_XXXXXXXX`) have no file to find. Pure of Tauri state so the gate can
/// drive it against a real scan; see `tests/world_gate.rs`.
pub fn find_model_for(manager: &RpfManager, name: &str) -> Option<String> {
    let needle = name.trim().to_lowercase();
    if needle.is_empty() || needle.starts_with("hash_") {
        return None;
    }
    manager
        .assets()
        .get(&needle)
        .map(|(_, _, path)| path.clone())
}

/// Resolve an archetype name to a model file, for the world view to draw.
///
/// A hash lookup once the asset index exists, but the *first* call builds it
/// out of every entry in the install, so this cannot run on the event loop
/// either.
#[tauri::command]
pub async fn world_asset(
    name: String,
    game: State<'_, GameState>,
) -> Result<Option<String>, String> {
    let loaded = game.loaded()?;
    blocking(move || Ok(find_model_for(&loaded.manager, &name))).await
}

/// One model file read into drawables, with the texture hashes its parts want.
///
/// The half of a model read that does not depend on what the pixels become —
/// shared by the world view's block path and the gate's PNG path so the two
/// cannot drift on which drawables or which hashes they saw.
struct ReadModel {
    drawables: Vec<ModelDrawable>,
    empty_drawables: usize,
    wanted: std::collections::BTreeSet<u32>,
    /// Keeps the parsed drawables alive, because `embedded` borrows from them.
    _named: Vec<cw_core::resources::NamedDrawable>,
}

/// Extract, parse and flatten a model file. See [`ReadModel`].
///
/// Like `world_map`, this reads past the viewer's one-open-file state: several
/// entities stream their models at once, so the Explorer's single open file
/// must not be disturbed.
fn read_model_file(
    path: &str,
    keys: &GtaKeys,
    manager: &RpfManager,
) -> Result<ReadModel, String> {
    let entry_ref = manager
        .find(path)
        .ok_or_else(|| format!("{path} is not in the index"))?;
    let entry = manager.entry(entry_ref);
    let kind = FileKind::from_name(&entry.name_lower);
    if !matches!(
        kind,
        FileKind::Model | FileKind::ModelDictionary | FileKind::Fragment
    ) {
        return Err(format!("{} is not a model", entry.name));
    }
    let (system_size, _) = entry
        .resource_segments()
        .ok_or("this model is not stored as a resource")?;
    let data = manager
        .extract(entry_ref, keys)
        .map_err(|e| format!("could not read {path}: {e}"))?;

    let named = match kind {
        FileKind::Fragment => {
            read_named_fragments(kind, &data, system_size).map_err(|e| format!("{e}"))?
        }
        _ => read_named_drawables(kind, &data, system_size).map_err(|e| format!("{e}"))?,
    };

    let mut wanted = std::collections::BTreeSet::new();
    let mut empty_drawables = 0;
    let drawables: Vec<ModelDrawable> = named
        .iter()
        .map(|entry| {
            let lods = MeshBundle::all(&entry.drawable);
            if lods.is_empty() {
                empty_drawables += 1;
            }
            for bundle in &lods {
                for part in &bundle.parts {
                    for hash in [part.diffuse, part.normal, part.specular]
                        .into_iter()
                        .flatten()
                    {
                        wanted.insert(hash);
                    }
                }
            }
            ModelDrawable {
                index: entry.index,
                name: entry.name.clone(),
                hash: entry.hash,
                lods,
            }
        })
        .collect();

    Ok(ReadModel {
        drawables,
        empty_drawables,
        wanted,
        _named: named,
    })
}

impl ReadModel {
    /// The embedded texture dictionaries, borrowed rather than cloned: a
    /// drawable's dictionary holds every mip of every texture it carries.
    fn embedded(&self) -> Vec<&TextureDictionary> {
        self._named
            .iter()
            .filter_map(|entry| entry.drawable.texture_dictionary.as_ref())
            .collect()
    }
}

/// Read a model with its textures as base64 PNGs.
///
/// The shape the world view used before the binary envelope, kept because the
/// gate compares the two paths against each other; see
/// `the_block_path_resolves_the_same_textures_as_the_png_path` in
/// `tests/world_gate.rs`. Nothing in the app calls it.
///
/// `archetypes` is what makes the texture chain cheap; see
/// [`crate::viewer::resolve_textures`]. Passing `None` still works and still
/// resolves, it just falls back to scanning the model's folder.
pub fn read_world_model(
    path: &str,
    keys: &GtaKeys,
    manager: &RpfManager,
    archetypes: Option<&ArchetypeIndexes>,
) -> Result<ModelPreview, String> {
    let read = read_model_file(path, keys, manager)?;
    let textures = crate::viewer::resolve_textures(
        path,
        &read.wanted,
        &read.embedded(),
        keys,
        manager,
        archetypes,
    );

    Ok(ModelPreview {
        path: path.to_string(),
        drawables: read.drawables,
        empty_drawables: read.empty_drawables,
        textures,
    })
}

/// A compiled model's metadata: everything but the bytes, which ride in the
/// envelope's sections.
///
/// The shapes below deliberately mirror `cw_core::mesh` rather than reusing it.
/// A `MeshPart` carries its buffers; a `WirePart` carries the section indices
/// they were put in, and the two transports genuinely describe different
/// things. Reusing one struct for both would mean a serde mode flag, which is
/// how a format ends up with fields that are sometimes there.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldModelMeta {
    pub path: String,
    pub drawables: Vec<WireDrawable>,
    pub empty_drawables: usize,
    pub textures: Vec<crate::textures::WireTexture>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireDrawable {
    pub index: usize,
    pub name: String,
    pub hash: u32,
    pub lods: Vec<WireBundle>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireBundle {
    pub name: Option<String>,
    pub lod: &'static str,
    pub available_lods: Vec<&'static str>,
    pub bounding_box_min: [f32; 3],
    pub bounding_box_max: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub vertex_count: usize,
    pub triangle_count: usize,
    pub parts: Vec<WirePart>,
}

/// One draw call, with its buffers named by section index.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePart {
    pub name: String,
    pub shader: u32,
    pub diffuse: Option<u32>,
    pub normal: Option<u32>,
    pub specular: Option<u32>,
    pub vertex_count: usize,
    pub triangle_count: usize,
    /// Section holding `f32` triples.
    pub positions: usize,
    /// Section holding `f32` triples, absent when the declaration had no normal.
    pub normals: Option<usize>,
    /// Section holding `f32` pairs, absent when there was no texcoord0.
    pub uvs: Option<usize>,
    pub indices: usize,
    /// 16 or 32. Every part in a 3,382-part sample of stock geometry indexed
    /// fewer than 65,536 vertices, which halves the index payload, but a part
    /// that needs the width still gets it — so the frontend reads this rather
    /// than assuming either one.
    pub index_bits: u8,
}

/// Push one drawable's buffers into the envelope, naming the sections.
fn wire_drawables(drawables: &[ModelDrawable], envelope: &mut Envelope) -> Vec<WireDrawable> {
    drawables
        .iter()
        .map(|drawable| WireDrawable {
            index: drawable.index,
            name: drawable.name.clone(),
            hash: drawable.hash,
            lods: drawable
                .lods
                .iter()
                .map(|bundle| WireBundle {
                    name: bundle.name.clone(),
                    lod: bundle.lod,
                    available_lods: bundle.available_lods.clone(),
                    bounding_box_min: bundle.bounding_box_min,
                    bounding_box_max: bundle.bounding_box_max,
                    bounding_sphere_radius: bundle.bounding_sphere_radius,
                    vertex_count: bundle.vertex_count,
                    triangle_count: bundle.triangle_count,
                    parts: bundle
                        .parts
                        .iter()
                        .map(|part| {
                            let (indices, index_bits) = part.narrow_indices();
                            WirePart {
                                name: part.name.clone(),
                                shader: part.shader,
                                diffuse: part.diffuse,
                                normal: part.normal,
                                specular: part.specular,
                                vertex_count: part.vertex_count,
                                triangle_count: part.triangle_count,
                                positions: envelope.push(part.positions.as_bytes()),
                                normals: part
                                    .normals
                                    .as_ref()
                                    .map(|buffer| envelope.push(buffer.as_bytes())),
                                uvs: part
                                    .uvs
                                    .as_ref()
                                    .map(|buffer| envelope.push(buffer.as_bytes())),
                                indices: envelope.push(indices),
                                index_bits,
                            }
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

/// Read a model and pack it into a wire envelope, textures as GPU blocks.
///
/// Pure of Tauri state so the gate can drive it against a real scan; see
/// `tests/world_gate.rs`.
pub fn compile_world_model(
    path: &str,
    keys: &GtaKeys,
    manager: &RpfManager,
    archetypes: Option<&ArchetypeIndexes>,
    caps: crate::textures::GpuCaps,
) -> Result<Vec<u8>, String> {
    let read = read_model_file(path, keys, manager)?;

    // The section index is assigned as each texture resolves, which is why the
    // encoder takes it: resolution order decides section order, and the chain
    // may skip a dictionary that answers nothing.
    let next_section = std::sync::atomic::AtomicUsize::new(0);
    let encoded = crate::viewer::resolve_textures_with(
        path,
        &read.wanted,
        &read.embedded(),
        keys,
        manager,
        archetypes,
        |hash, texture, source| {
            let section = next_section.load(std::sync::atomic::Ordering::Relaxed);
            let out = crate::textures::encode(hash, texture, source, caps, section)?;
            next_section.store(section + 1, std::sync::atomic::Ordering::Relaxed);
            Some(out)
        },
    );

    // `resolve_textures_with` collects out of a `HashMap`, so the order it hands
    // back is not the order sections were assigned. Sort by the index each
    // texture recorded and the envelope's push order matches its metadata again.
    let mut encoded = encoded;
    encoded.sort_by_key(|texture| texture.meta.section);

    // Textures are pushed first because their section indexes were assigned
    // during resolution, before the envelope existed. Geometry follows and
    // takes whatever comes next.
    let mut envelope = Envelope::new();
    let mut textures = Vec::with_capacity(encoded.len());
    for texture in encoded {
        let section = envelope.push(texture.bytes);
        debug_assert_eq!(section, texture.meta.section);
        textures.push(texture.meta);
    }
    let drawables = wire_drawables(&read.drawables, &mut envelope);

    let meta = WorldModelMeta {
        path: path.to_string(),
        drawables,
        empty_drawables: read.empty_drawables,
        textures,
    };
    let json = serde_json::to_string(&meta).map_err(|e| format!("could not encode {path}: {e}"))?;
    Ok(envelope.finish(Kind::Model, &json))
}

/// The world view's hot path: one compiled model, from the cache when it can be.
///
/// The cached and the interactive path are the same line, which is what makes
/// "compile on demand" and "bake in the background" the same code. A model the
/// cache has never seen is compiled here and stored on the way out; one it has
/// is a mapped read and a memcpy.
#[tauri::command]
pub async fn world_model(
    path: String,
    game: State<'_, GameState>,
    gpu: State<'_, GpuState>,
) -> Result<tauri::ipc::Response, String> {
    let loaded = game.loaded()?;
    let caps = gpu.get();
    blocking(move || {
        let compile = || {
            compile_world_model(
                &path,
                &loaded.keys,
                &loaded.manager,
                Some(loaded.archetypes()),
                caps,
            )
        };
        match &loaded.cache {
            Some(cache) => {
                let key = crate::cache::model_key(&loaded.manager, &path, caps);
                cache.get_or_insert_with(key, cw_cache::Kind::Model, compile)
            }
            None => compile(),
        }
    })
    .await
    .map(tauri::ipc::Response::new)
}

/// What the WebView's GL context accepts, as it reported at world-mode start.
///
/// Read on every model compile and written once, so `RwLock` rather than a
/// channel: a reader never blocks another reader, and the one write happens
/// before any streaming does.
#[derive(Debug, Default)]
pub struct GpuState(pub std::sync::RwLock<crate::textures::GpuCaps>);

impl GpuState {
    /// The reported capabilities, or none at all if the lock is poisoned —
    /// which costs fidelity (RGBA instead of blocks) and never correctness.
    pub fn get(&self) -> crate::textures::GpuCaps {
        self.0
            .read()
            .map(|caps| *caps)
            .unwrap_or_default()
    }
}

/// Tell the backend which compressed texture formats this GL context takes.
///
/// Must be called before the first model streams. Until it is, every texture
/// falls back to RGBA: correct, four times the VRAM, and the frontend's job to
/// avoid by making this its first call in world mode.
#[tauri::command]
pub fn set_gpu_capabilities(
    caps: crate::textures::GpuCaps,
    gpu: State<'_, GpuState>,
) -> Result<(), String> {
    *gpu.0.write().map_err(|_| "gpu state is poisoned")? = caps;
    Ok(())
}

/// Save a world project payload to disk, as the frontend serialized it.
#[tauri::command]
pub fn project_save(path: String, contents: String) -> Result<usize, String> {
    std::fs::write(&path, contents.as_bytes())
        .map_err(|e| format!("could not write {path}: {e}"))?;
    Ok(contents.len())
}

/// Read a world project payload back from disk.
#[tauri::command]
pub fn project_load(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("could not read {path}: {e}"))
}

/// The world view's first question: is there a game, what content does it have,
/// and is a map open in the Explorer.
#[tauri::command]
pub fn world_status(
    game: State<'_, GameState>,
    viewer: State<'_, ViewerState>,
) -> Result<WorldStatus, String> {
    let opened = game.try_loaded();
    let open_map_path = viewer
        .0
        .lock()
        .map_err(|_| "viewer state is poisoned")?
        .as_ref()
        .map(|open| &open.info)
        .filter(|info| info.name.to_ascii_lowercase().ends_with(".ymap"))
        .map(|info| info.path.clone());

    match opened.as_ref() {
        Some(loaded) => Ok(WorldStatus {
            game_open: true,
            folder: Some(loaded.folder.clone()),
            dlc_groups: content_groups(&loaded.manager),
            open_map_path,
        }),
        None => Ok(WorldStatus {
            game_open: false,
            folder: None,
            dlc_groups: Vec::new(),
            open_map_path,
        }),
    }
}

/// The `.ymap` files in one content group, capped for the sidebar.
#[tauri::command]
pub async fn world_dlc_ymaps(
    group: String,
    game: State<'_, GameState>,
) -> Result<YmapListingPage, String> {
    let loaded = game.loaded()?;
    blocking(move || {
        let all = ymaps_in_group(&loaded.manager, &group);
        let total = all.len();
        let ymaps = all
            .into_iter()
            .take(MAX_YMAPS_PER_GROUP)
            .map(|(path, name)| YmapListing { path, name })
            .collect();

        Ok(YmapListingPage { total, ymaps })
    })
    .await
}

/// Every `.ymap` in one content group, uncapped. The sidebar page caps at
/// `MAX_YMAPS_PER_GROUP`; loading a whole group (the base game, say) wants the
/// full list.
#[tauri::command]
pub async fn world_maps_in_group(
    group: String,
    game: State<'_, GameState>,
) -> Result<Vec<YmapListing>, String> {
    let loaded = game.loaded()?;
    blocking(move || {
        Ok(ymaps_in_group(&loaded.manager, &group)
            .into_iter()
            .map(|(path, name)| YmapListing { path, name })
            .collect())
    })
    .await
}

/// Read one `.ymap` by archive path into flattened world placements.
///
/// Reads past the viewer's one-open-file state on purpose — see the module
/// note — so placing several maps does not fight the Explorer over what is
/// open.
///
/// Comes back as a wire envelope rather than JSON. Over the base game that is
/// 763,716 placements, and as JSON they weigh 201.5 MB — more to serialize than
/// the maps cost to read. See `crate::placements`.
#[tauri::command]
pub async fn world_map(
    path: String,
    game: State<'_, GameState>,
) -> Result<tauri::ipc::Response, String> {
    let loaded = game.loaded()?;
    blocking(move || {
        // Both indexes are built once and kept: scanning every `.ytyp` is the
        // expensive part, and only a map that places interiors needs it.
        let map = read_world_map(
            &path,
            &loaded.keys,
            &loaded.manager,
            Some(loaded.mlo_archetypes()),
            Some(loaded.names()),
        )?;
        Ok(tauri::ipc::Response::new(crate::placements::encode(&map)?))
    })
    .await
}

/// Place a `.ymap` the user picked from disk, whether or not the install's
/// index knows it — a custom/mod map. Reads the payload off the filesystem and
/// shares the flattening with `read_world_map`, and the envelope with
/// `world_map`.
#[tauri::command]
pub async fn world_map_file(
    path: String,
    game: State<'_, GameState>,
) -> Result<tauri::ipc::Response, String> {
    let loaded = game.loaded()?;
    blocking(move || {
        let raw = std::fs::read(&path).map_err(|e| format!("could not read {path}: {e}"))?;
        let (payload, system_size) = loose_ymap_payload(&raw);

        let map = flatten_ymap(
            &path,
            &payload,
            system_size,
            &loaded.keys,
            &loaded.manager,
            Some(loaded.mlo_archetypes()),
            Some(loaded.names()),
        )?;
        Ok(tauri::ipc::Response::new(crate::placements::encode(&map)?))
    })
    .await
}

/// A standalone `.ymap` on disk, as a mod author distributes one.
///
/// A resource payload may carry its 16-byte RSC7 header (the form OpenIV and
/// the reference export); when it does, the system segment size comes from the
/// header's system flags and the header is trimmed. Without the header, the
/// whole payload is one system segment — the reference's fallback for a bare
/// resource file.
fn loose_ymap_payload(data: &[u8]) -> (Vec<u8>, usize) {
    if data.len() >= 16 && &data[..4] == b"RSC7" {
        let system_flags = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        (data[16..].to_vec(), size_from_flags(system_flags) as usize)
    } else {
        (data.to_vec(), data.len())
    }
}

/// The placement read itself, free of Tauri state so the gate can drive it
/// against a real scan. See `tests/world_gate.rs`.
///
/// `mlo_index` resolves each MLO instance to its archetype's interior. When
/// `None`, one is built from the manager on the spot — convenient for the
/// gate; the app command passes its cached index. `names` resolves an
/// archetype hash back to its model name; `None` falls back to the meta table.
pub fn read_world_map(
    path: &str,
    keys: &GtaKeys,
    manager: &RpfManager,
    mlo_index: Option<&std::collections::HashMap<u32, MloArchetype>>,
    names: Option<&std::collections::HashMap<u32, String>>,
) -> Result<WorldMap, String> {
    let entry_ref = manager
        .find(path)
        .ok_or_else(|| format!("{path} is not in the index"))?;
    let entry = manager.entry(entry_ref);

    if !entry.name_lower.ends_with(".ymap") {
        return Err(format!("{} is not a .ymap", entry.name));
    }
    let (system_size, _) = entry
        .resource_segments()
        .ok_or("this .ymap is not stored as a resource")?;
    let data = manager
        .extract(entry_ref, keys)
        .map_err(|e| format!("could not read {path}: {e}"))?;

    flatten_ymap(path, &data, system_size, keys, manager, mlo_index, names)
}

/// Flatten a `.ymap` payload into world placements — the part `read_world_map`
/// and the standalone-file read (`world_map_file`) share.
///
/// `data` is the payload the manager hands out (the 16-byte RSC7 header already
/// stripped) and `system_size` its system segment length; a standalone file is
/// prepared the same way by `loose_ymap_payload`.
pub fn flatten_ymap(
    path: &str,
    data: &[u8],
    system_size: usize,
    keys: &GtaKeys,
    manager: &RpfManager,
    mlo_index: Option<&std::collections::HashMap<u32, MloArchetype>>,
    names: Option<&std::collections::HashMap<u32, String>>,
) -> Result<WorldMap, String> {
    let meta = read_metadata(FileKind::Meta, data, Some(system_size))
        .map_err(|e| format!("could not read the metadata: {e}"))?;
    let ymap = match &meta {
        MetaFile::Rsc(meta) => read_ymap(meta).map_err(|e| format!("{e}"))?,
        _ => return Err("this .ymap is not an RSC resource".into()),
    };

    let mut entities: Vec<WorldEntity> = ymap
        .entities
        .iter()
        .enumerate()
        .map(|(index, entity)| WorldEntity {
            index,
            name: name_for(entity.def.archetype_name, names),
            name_hash: entity.def.archetype_name,
            is_mlo: entity.is_mlo,
            from_mlo: false,
            position: entity.def.position,
            rotation: entity.def.rotation,
            scale_xy: entity.def.scale_xy,
            scale_z: entity.def.scale_z,
            parent_index: entity.def.parent_index,
            lod_level: lod_level_label(entity.def.lod_level),
            lod_dist: entity.def.lod_dist,
            children: entity.children.len(),
        })
        .collect();

    // Expand every MLO instance's interior into world space. The instance's
    // `archetypeName` is the index key; an instance whose archetype does not
    // resolve (a destroyed/modded reference) just contributes nothing.
    let mut mlo_expanded = 0;
    let owned_index: std::collections::HashMap<u32, MloArchetype>;
    let index: &std::collections::HashMap<u32, MloArchetype> = match mlo_index {
        Some(index) => index,
        None if !ymap.mlo_entities.is_empty() => {
            owned_index = index_mlo_archetypes(manager, keys);
            &owned_index
        }
        None => {
            owned_index = std::collections::HashMap::new();
            &owned_index
        }
    };

    for mlo_index_in_map in &ymap.mlo_entities {
        let instance = &ymap.entities[*mlo_index_in_map];
        let Some(interior) = index.get(&instance.def.archetype_name) else {
            continue;
        };
        let expanded = expand_mlo_entities(
            instance.def.position,
            instance.def.rotation,
            &interior.entities,
        );
        for entity in &expanded {
            entities.push(WorldEntity {
                index: entities.len(),
                name: name_for(entity.archetype_name, names),
                name_hash: entity.archetype_name,
                is_mlo: false,
                from_mlo: true,
                position: entity.position,
                rotation: entity.rotation,
                scale_xy: entity.scale_xy,
                scale_z: entity.scale_z,
                parent_index: -1,
                lod_level: lod_level_label(entity.lod_level),
                lod_dist: entity.lod_dist,
                children: 0,
            });
        }
        mlo_expanded += expanded.len();
    }

    // The streamer keys off these extents, and many files declare degenerate
    // (zero) ones — the box the placements actually live in is what should
    // decide whether the map streams.
    let mut extents_min = [f32::INFINITY; 3];
    let mut extents_max = [f32::NEG_INFINITY; 3];
    for entity in &entities {
        for axis in 0..3 {
            extents_min[axis] = extents_min[axis].min(entity.position[axis]);
            extents_max[axis] = extents_max[axis].max(entity.position[axis]);
        }
    }
    // A map with no placements streams nothing, so the degenerate box is fine.
    if entities.is_empty() {
        extents_min = ymap.entities_extents_min;
        extents_max = ymap.entities_extents_max;
    }

    Ok(WorldMap {
        path: path.to_string(),
        name: name_for(ymap.name, names),
        name_hash: ymap.name,
        parent: hash_string(ymap.parent),
        entity_count: ymap.entities.len(),
        mlo_entities: ymap.mlo_entities.len(),
        extents_min,
        extents_max,
        entities,
        mlo_expanded,
    })
}

/// Resolve a hash to its asset name. The entry-basename index is what the
/// archives actually name (a prop's archetype hash is the hash of its model's
/// basename); the static meta table is the fallback when the archives do not
/// know it.
fn name_for(hash: u32, names: Option<&std::collections::HashMap<u32, String>>) -> String {
    names
        .and_then(|index| index.get(&hash))
        .cloned()
        .unwrap_or_else(|| hash_string(hash))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_the_root_archives_as_base() {
        assert_eq!(group_key("x64a.rpf"), "base");
        assert_eq!(group_key("common.rpf"), "base");
    }

    #[test]
    fn groups_dlc_packs_by_pack_name() {
        assert_eq!(
            group_key(r"update\x64\dlcpacks\patchday2ng\dlc.rpf"),
            "patchday2ng"
        );
        assert_eq!(
            group_key(r"x64\dlcpacks\mpchristmas2\dlc.rpf"),
            "mpchristmas2"
        );
    }

    #[test]
    fn groups_anything_else_by_directory() {
        assert_eq!(group_key(r"update\update.rpf"), "update");
        assert_eq!(group_key(r"update\x64\dlc.rpf"), "update");
    }

    #[test]
    fn strips_an_rsc7_header_for_its_system_size() {
        let mut raw = vec![0u8; 16 + 8192];
        raw[..4].copy_from_slice(b"RSC7");
        // 0x0800_0004 decodes to 8192 bytes (see cw-core's size_from_flags).
        raw[8..12].copy_from_slice(&0x0800_0004u32.to_le_bytes());
        let (payload, system) = loose_ymap_payload(&raw);
        assert_eq!(system, 8192);
        assert_eq!(payload.len(), 8192);
    }

    #[test]
    fn treats_a_headerless_payload_as_one_system_segment() {
        let raw = vec![7u8; 512];
        let (payload, system) = loose_ymap_payload(&raw);
        assert_eq!(payload.len(), 512);
        assert_eq!(system, 512);
    }
}
