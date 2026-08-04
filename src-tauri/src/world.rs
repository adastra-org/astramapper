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

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cw_core::filetype::FileKind;
use cw_core::filetypes::{expand_mlo_entities, index_mlo_archetypes, read_ymap, MloArchetype};
use cw_core::image;
use cw_core::mesh::MeshBundle;
use cw_core::meta::{read_meta as read_metadata, LodLevel, MetaFile};
use cw_core::resources::{read_named_drawables, read_named_fragments, TextureDictionary};
use cw_core::rpf::{size_from_flags, RpfManager};
use cw_core::utils::{hash_string, GtaKeys};
use serde::Serialize;
use tauri::State;

use crate::explorer::GameState;
use crate::viewer::{ModelDrawable, ModelPreview, ModelTexture, ViewerState};

/// How many `.ymap` files one group listing hands over. A city group can name
/// thousands; the UI shows this many and the total.
const MAX_YMAPS_PER_GROUP: usize = 200;

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
#[tauri::command]
pub fn world_asset(name: String, game: State<'_, GameState>) -> Result<Option<String>, String> {
    let loaded = game.loaded()?;
    Ok(find_model_for(&loaded.manager, &name))
}

/// Read a model file's geometry, every LOD, for the world view to draw.
///
/// Like `world_map`, this reads past the viewer's one-open-file state: several
/// entities stream their models at once, so the Explorer's single open file
/// must not be disturbed.
///
/// The texture resolution chain is deliberately narrower than the Explorer's:
/// embedded dictionaries first, then the sibling `.ytd` (same folder, same
/// basename) found by one index lookup. The Explorer's third step — every
/// `.ytd` in the model's folder — is a full-tree scan that costs too much per
/// streamed model; most stock models' pixels live in the sibling anyway.
pub fn read_world_model(
    path: &str,
    keys: &GtaKeys,
    manager: &RpfManager,
) -> Result<ModelPreview, String> {
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

    // The embedded texture dictionaries, kept aside before the drawables are
    // flattened into bundles.
    let embedded: Vec<TextureDictionary> = named
        .iter()
        .filter_map(|entry| entry.drawable.texture_dictionary.clone())
        .collect();

    let mut wanted = std::collections::BTreeSet::new();
    let mut empty_drawables = 0;
    let drawables = named
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

    let textures = resolve_world_textures(path, &wanted, &embedded, keys, manager);

    Ok(ModelPreview {
        path: path.to_string(),
        drawables,
        empty_drawables,
        textures,
    })
}

/// Resolve a model's texture hashes into PNGs for the WebView.
///
/// The drawables' embedded dictionaries come first, then a `.ytd` sharing the
/// model's basename, then every `.ytd` in the same folder — the chain the
/// Explorer uses. Most stock drawables carry no embedded dictionary and the
/// pixels live in same-folder `.ytd` files the archetype loads, so the folder
/// scan is what usually lands; without it streamed models render flat grey.
fn resolve_world_textures(
    path: &str,
    wanted: &std::collections::BTreeSet<u32>,
    embedded: &[TextureDictionary],
    keys: &GtaKeys,
    manager: &RpfManager,
) -> Vec<ModelTexture> {
    if wanted.is_empty() {
        return Vec::new();
    }

    let mut by_hash: std::collections::HashMap<u32, ModelTexture> =
        std::collections::HashMap::new();
    for dictionary in embedded {
        resolve_from_dictionary(dictionary, wanted, &mut by_hash, None);
        if by_hash.len() == wanted.len() {
            break;
        }
    }

    if by_hash.len() < wanted.len() {
        // The model's folder and stem, for the sibling and folder scans.
        let (folder, basename) = match path.rsplit_once('\\') {
            Some((folder, name)) => {
                let stem = name
                    .strip_suffix(".ydr")
                    .or_else(|| name.strip_suffix(".yft"))
                    .or_else(|| name.strip_suffix(".ydd"))
                    .unwrap_or(name);
                (folder, stem)
            }
            None => ("", path),
        };

        // Every `.ytd` in the model's folder, the same-basename sibling first —
        // the reference tries it before the archetype's dictionary list. The
        // folder index is scanned once per install, not per model.
        let sibling = format!("{folder}\\{basename}.ytd");
        let mut paths: Vec<&str> = manager
            .ytds()
            .get(folder)
            .map(|paths| paths.iter().map(String::as_str).collect())
            .unwrap_or_default();
        paths.sort_by_key(|p| *p != sibling.as_str());

        for ytd_path in paths {
            let Some(reference) = manager.find(ytd_path) else {
                continue;
            };
            let entry = manager.entry(reference);
            let Some((system_size, _)) = entry.resource_segments() else {
                continue;
            };
            if let Ok(data) = manager.extract(reference, keys) {
                if let Ok(dictionary) =
                    cw_core::resources::read_textures(FileKind::Texture, &data, system_size)
                {
                    resolve_from_dictionary(
                        &dictionary,
                        wanted,
                        &mut by_hash,
                        Some(ytd_path),
                    );
                }
            }
            if by_hash.len() == wanted.len() {
                break;
            }
        }
    }

    by_hash.into_values().collect()
}

/// Decode every wanted hash a dictionary holds into the response.
fn resolve_from_dictionary(
    dictionary: &TextureDictionary,
    wanted: &std::collections::BTreeSet<u32>,
    by_hash: &mut std::collections::HashMap<u32, ModelTexture>,
    source: Option<&str>,
) {
    for hash in wanted {
        if by_hash.contains_key(hash) {
            continue;
        }
        let Some(texture) = dictionary.lookup(*hash) else {
            continue;
        };
        let Ok(decoded) = image::decode_mip(texture, 0) else {
            continue;
        };
        let Ok(png) = image::to_png(&decoded) else {
            continue;
        };
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
                source: source.map(str::to_string),
            },
        );
    }
}

#[tauri::command]
pub fn world_model(path: String, game: State<'_, GameState>) -> Result<ModelPreview, String> {
    let loaded = game.loaded()?;
    read_world_model(&path, &loaded.keys, &loaded.manager)
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
pub fn world_dlc_ymaps(
    group: String,
    game: State<'_, GameState>,
) -> Result<YmapListingPage, String> {
    let loaded = game.loaded()?;

    let all = ymaps_in_group(&loaded.manager, &group);
    let total = all.len();
    let ymaps = all
        .into_iter()
        .take(MAX_YMAPS_PER_GROUP)
        .map(|(path, name)| YmapListing { path, name })
        .collect();

    Ok(YmapListingPage { total, ymaps })
}

/// Every `.ymap` in one content group, uncapped. The sidebar page caps at
/// `MAX_YMAPS_PER_GROUP`; loading a whole group (the base game, say) wants the
/// full list.
#[tauri::command]
pub fn world_maps_in_group(
    group: String,
    game: State<'_, GameState>,
) -> Result<Vec<YmapListing>, String> {
    let loaded = game.loaded()?;
    Ok(ymaps_in_group(&loaded.manager, &group)
        .into_iter()
        .map(|(path, name)| YmapListing { path, name })
        .collect())
}

/// Read one `.ymap` by archive path into flattened world placements.
///
/// Reads past the viewer's one-open-file state on purpose — see the module
/// note — so placing several maps does not fight the Explorer over what is
/// open.
#[tauri::command]
pub fn world_map(path: String, game: State<'_, GameState>) -> Result<WorldMap, String> {
    let loaded = game.loaded()?;

    // Both indexes are built once and kept: scanning every `.ytyp` is the
    // expensive part, and only a map that places interiors needs it.
    read_world_map(
        &path,
        &loaded.keys,
        &loaded.manager,
        Some(loaded.mlo_archetypes()),
        Some(loaded.names()),
    )
}

/// Place a `.ymap` the user picked from disk, whether or not the install's
/// index knows it — a custom/mod map. Reads the payload off the filesystem and
/// shares the flattening with `read_world_map`.
#[tauri::command]
pub fn world_map_file(path: String, game: State<'_, GameState>) -> Result<WorldMap, String> {
    let loaded = game.loaded()?;

    let raw = std::fs::read(&path).map_err(|e| format!("could not read {path}: {e}"))?;
    let (payload, system_size) = loose_ymap_payload(&raw);

    flatten_ymap(
        &path,
        &payload,
        system_size,
        &loaded.keys,
        &loaded.manager,
        Some(loaded.mlo_archetypes()),
        Some(loaded.names()),
    )
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
