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
use cw_core::filetypes::read_ymap;
use cw_core::image;
use cw_core::mesh::MeshBundle;
use cw_core::meta::{read_meta as read_metadata, LodLevel, MetaFile};
use cw_core::resources::{read_named_drawables, read_named_fragments, TextureDictionary};
use cw_core::rpf::RpfManager;
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
/// `assetName`, which is a file in the archives. This finds it: a `.ydr`,
/// `.yft` or `.ydd` whose basename matches the resolved name, preferring a
/// `update\` copy over the base game the way the reference's content precedence
/// does, and a `.ydr` over a `.yft` over a `.ydd`. Names the name table cannot
/// resolve (`hash_XXXXXXXX`) have no file to find. Pure of Tauri state so the
/// gate can drive it against a real scan; see `tests/world_gate.rs`.
pub fn find_model_for(manager: &RpfManager, name: &str) -> Option<String> {
    let needle = name.trim().to_lowercase();
    if needle.is_empty() || needle.starts_with("hash_") {
        return None;
    }
    let ext_priority = |lower: &str| -> u8 {
        if lower.ends_with(".ydr") {
            0
        } else if lower.ends_with(".yft") {
            1
        } else {
            2
        }
    };
    // A lower `candidate` tuple wins: `update` beats base, a `.ydr` beats a
    // `.yft`, then the path breaks the tie.
    let mut best: Option<(u8, u8, &str)> = None;
    for archive in &manager.archives {
        for entry in &archive.entries {
            let lower = &entry.name_lower;
            if !(lower.ends_with(".ydr") || lower.ends_with(".yft") || lower.ends_with(".ydd")) {
                continue;
            }
            let basename = lower.rsplit('\\').next().unwrap_or(lower);
            let stem = &basename[..basename.len().saturating_sub(4)];
            if stem != needle {
                continue;
            }
            let candidate = (
                u8::from(!lower.starts_with("update\\")),
                ext_priority(lower),
                entry.path.as_str(),
            );
            if best.is_none_or(|current| candidate < current) {
                best = Some(candidate);
            }
        }
    }
    best.map(|(_, _, path)| path.to_string())
}

/// Resolve an archetype name to a model file, for the world view to draw.
#[tauri::command]
pub fn world_asset(name: String, game: State<'_, GameState>) -> Result<Option<String>, String> {
    let guard = game.0.lock().map_err(|_| "game state is poisoned")?;
    let loaded = guard.as_ref().ok_or("no game folder is open")?;
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

/// Resolve a model's texture hashes against its embedded dictionaries and its
/// sibling `.ytd`, decoding each hit to a PNG for the WebView.
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
        resolve_from_dictionary(dictionary, wanted, &mut by_hash);
        if by_hash.len() == wanted.len() {
            break;
        }
    }

    if by_hash.len() < wanted.len() {
        if let Some(sibling) = sibling_texture_path(path) {
            if let Some(reference) = manager.find(&sibling) {
                let entry = manager.entry(reference);
                if let Some((system_size, _)) = entry.resource_segments() {
                    if let Ok(data) = manager.extract(reference, keys) {
                        if let Ok(dictionary) =
                            cw_core::resources::read_textures(FileKind::Texture, &data, system_size)
                        {
                            resolve_from_dictionary(&dictionary, wanted, &mut by_hash);
                        }
                    }
                }
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
            },
        );
    }
}

/// The `.ytd` that shares a model's folder and basename, if the path has a
/// basename worth matching.
fn sibling_texture_path(path: &str) -> Option<String> {
    let (folder, basename) = match path.rsplit_once('\\') {
        Some((folder, name)) => (folder, name),
        None => ("", path),
    };
    let stem = basename
        .strip_suffix(".ydr")
        .or_else(|| basename.strip_suffix(".yft"))
        .or_else(|| basename.strip_suffix(".ydd"))?;
    Some(if folder.is_empty() {
        format!("{stem}.ytd")
    } else {
        format!("{folder}\\{stem}.ytd")
    })
}

#[tauri::command]
pub fn world_model(path: String, game: State<'_, GameState>) -> Result<ModelPreview, String> {
    let guard = game.0.lock().map_err(|_| "game state is poisoned")?;
    let loaded = guard.as_ref().ok_or("no game folder is open")?;
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
    let game_guard = game.0.lock().map_err(|_| "game state is poisoned")?;
    let open_map_path = viewer
        .0
        .lock()
        .map_err(|_| "viewer state is poisoned")?
        .as_ref()
        .map(|open| &open.info)
        .filter(|info| info.name.to_ascii_lowercase().ends_with(".ymap"))
        .map(|info| info.path.clone());

    match game_guard.as_ref() {
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
    let guard = game.0.lock().map_err(|_| "game state is poisoned")?;
    let loaded = guard.as_ref().ok_or("no game folder is open")?;

    let all = ymaps_in_group(&loaded.manager, &group);
    let total = all.len();
    let ymaps = all
        .into_iter()
        .take(MAX_YMAPS_PER_GROUP)
        .map(|(path, name)| YmapListing { path, name })
        .collect();

    Ok(YmapListingPage { total, ymaps })
}

/// Read one `.ymap` by archive path into flattened world placements.
///
/// Reads past the viewer's one-open-file state on purpose — see the module
/// note — so placing several maps does not fight the Explorer over what is
/// open.
#[tauri::command]
pub fn world_map(path: String, game: State<'_, GameState>) -> Result<WorldMap, String> {
    let guard = game.0.lock().map_err(|_| "game state is poisoned")?;
    let loaded = guard.as_ref().ok_or("no game folder is open")?;
    read_world_map(&path, &loaded.keys, &loaded.manager)
}

/// The placement read itself, free of Tauri state so the gate can drive it
/// against a real scan. See `tests/world_gate.rs`.
pub fn read_world_map(
    path: &str,
    keys: &GtaKeys,
    manager: &RpfManager,
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

    let meta = read_metadata(FileKind::Meta, &data, Some(system_size))
        .map_err(|e| format!("could not read the metadata: {e}"))?;
    let ymap = match &meta {
        MetaFile::Rsc(meta) => read_ymap(meta).map_err(|e| format!("{e}"))?,
        _ => return Err("this .ymap is not an RSC resource".into()),
    };

    let entities = ymap
        .entities
        .iter()
        .enumerate()
        .map(|(index, entity)| WorldEntity {
            index,
            name: hash_string(entity.def.archetype_name),
            name_hash: entity.def.archetype_name,
            is_mlo: entity.is_mlo,
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

    Ok(WorldMap {
        path: path.to_string(),
        name: hash_string(ymap.name),
        name_hash: ymap.name,
        parent: hash_string(ymap.parent),
        entity_count: ymap.entities.len(),
        mlo_entities: ymap.mlo_entities.len(),
        extents_min: ymap.entities_extents_min,
        extents_max: ymap.entities_extents_max,
        entities,
    })
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
}
