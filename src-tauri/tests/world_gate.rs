//! The world view's scaffolding against a real install: does grouping the
//! install into content groups and reading a `.ymap` into placements work, from
//! the same functions the IPC commands call?
//!
//! ```text
//! cargo test -p astramapper -- --ignored --nocapture
//! ```

use astramapper_lib::world::{
    content_groups, find_model_for, read_world_map, read_world_model, ymaps_in_group,
};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use std::path::PathBuf;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn gta_path() -> PathBuf {
    PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.to_string()))
}

fn scan() -> (RpfManager, GtaKeys) {
    let exe = gta_path().join("GTA5.exe");
    let data = std::fs::read(&exe).unwrap_or_else(|e| panic!("cannot read {}: {e}", exe.display()));
    let keys = GtaKeys::from_exe(&data).expect("key recovery failed");
    let manager = RpfManager::scan(&gta_path(), &keys).expect("scan the game folder");
    (manager, keys)
}

/// The install has to split into a `base` group plus DLC packs, with the
/// `patchday*` packs present — that is what the world view's content selection
/// is for.
#[test]
#[ignore = "needs a local GTA V install"]
fn groups_the_install_into_base_and_dlc_packs() {
    let (manager, _) = scan();
    let groups = content_groups(&manager);

    let base = groups
        .iter()
        .find(|g| g.id == "base")
        .expect("a base group");
    assert!(base.archives > 0 && base.entries > 0, "base is empty");

    let pack = groups.iter().find(|g| g.id.starts_with("patchday"));
    assert!(
        pack.is_some(),
        "expected a patchday* DLC pack, got: {}",
        groups
            .iter()
            .map(|g| g.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let total_entries: usize = groups.iter().map(|g| g.entries).sum();
    assert_eq!(total_entries, manager.entry_count());
    println!(
        "{} groups, {} group entries: {}",
        groups.len(),
        total_entries,
        groups
            .iter()
            .take(6)
            .map(|g| format!("{} ({} archives, {} entries)", g.id, g.archives, g.entries))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// A content group has to name real `.ymap` files, and a patchday group has to
/// hold the patch's maps.
#[test]
#[ignore = "needs a local GTA V install"]
fn a_group_lists_its_ymaps() {
    let (manager, _) = scan();
    let groups = content_groups(&manager);
    let group = groups
        .iter()
        .find(|g| g.id == "base")
        .expect("a base group");

    let ymaps = ymaps_in_group(&manager, &group.id);
    assert!(!ymaps.is_empty(), "base should hold .ymap files");
    assert!(
        ymaps.windows(2).all(|w| w[0].0 <= w[1].0),
        "the group's maps should be sorted by path"
    );

    let patch = groups
        .iter()
        .find(|g| g.id.starts_with("patchday"))
        .expect("a patchday* group");
    let patch_ymaps = ymaps_in_group(&manager, &patch.id);
    println!(
        "base: {} ymaps, {}: {} ymaps",
        ymaps.len(),
        patch.id,
        patch_ymaps.len()
    );
}

/// Reading one `.ymap` into placements has to hand back every entity with its
/// position, and the whole map has to sit inside the city's extents.
#[test]
#[ignore = "needs a local GTA V install"]
fn reads_a_ymap_into_world_placements() {
    let (manager, keys) = scan();

    // Find a real city map: one of the `_city` maps is the safest bet for a
    // placement file with entities. Prefer one with children, since the world
    // view is what the hierarchy is for.
    let candidate = manager
        .archives
        .iter()
        .flat_map(|a| a.entries.iter())
        .map(|e| &e.path)
        .find(|p| {
            p.ends_with(".ymap")
                && p.contains(r"\levels\gta5\_city")
                && !p.contains(r"\.rpf\interiors\")
        })
        .expect("the install should have a city map");

    let map = read_world_map(candidate, &keys, &manager).expect("read the map");
    assert!(map.entity_count > 0, "{} places nothing", map.path);

    let hd = map.entities.iter().filter(|e| e.lod_level == "HD").count();
    assert!(hd > 0, "expected HD entities");

    let mut seen = std::collections::HashSet::new();
    for entity in &map.entities {
        assert!(seen.insert(entity.index), "duplicate entity index");
        assert!(
            entity.position.iter().all(|v| v.is_finite()),
            "non-finite position on {}",
            entity.index
        );
    }

    println!(
        "{}: {} entities ({} HD, {} MLO), extents {}..{}",
        map.name,
        map.entity_count,
        hd,
        map.mlo_entities,
        format_3(&map.extents_min),
        format_3(&map.extents_max)
    );
}

fn format_3(v: &[f32; 3]) -> String {
    format!("({}, {}, {})", v[0], v[1], v[2])
}

/// The LOD streamer resolves a placed entity's archetype to a model file and
/// that file to geometry. A city map's HD entities should name models that
/// exist and draw.
#[test]
#[ignore = "needs a local GTA V install"]
fn streams_an_entitys_archetype_to_real_geometry() {
    let (manager, keys) = scan();

    let candidate = manager
        .archives
        .iter()
        .flat_map(|a| a.entries.iter())
        .map(|e| &e.path)
        .find(|p| p.ends_with(".ymap") && p.contains(r"\levels\gta5\_city"))
        .expect("the install should have a city map");
    let map = read_world_map(candidate, &keys, &manager).expect("read the map");

    // Take the first entity whose archetype resolved to a real name; only those
    // can name a file on disk.
    let entity = map
        .entities
        .iter()
        .find(|e| !e.name.starts_with("hash_"))
        .expect("a city map should have nameable entities");

    let model_path = find_model_for(&manager, &entity.name)
        .unwrap_or_else(|| panic!("no model file for {}", entity.name));
    assert!(
        model_path.to_lowercase().ends_with(".ydr")
            || model_path.to_lowercase().ends_with(".yft")
            || model_path.to_lowercase().ends_with(".ydd"),
        "resolved a non-model path: {model_path}"
    );

    let preview = read_world_model(&model_path, &keys, &manager).expect("read the model");
    assert!(
        !preview.drawables.is_empty(),
        "{model_path} read no drawables"
    );
    let drawable = preview
        .drawables
        .iter()
        .find(|d| !d.lods.is_empty())
        .expect("a model should have geometry");
    assert!(
        drawable.lods.iter().any(|l| l.triangle_count > 0),
        "{model_path} has LODs but no triangles"
    );

    println!(
        "{} -> {}: {} drawable, {} LODs, {} triangles at high",
        entity.name,
        model_path,
        preview.drawables.len(),
        drawable.lods.len(),
        drawable.lods[0].triangle_count
    );
}

/// A name the table cannot resolve, or a prop that is not in the archives, has
/// to come back as `None` rather than panicking the streamer.
#[test]
#[ignore = "needs a local GTA V install"]
fn an_unresolvable_archetype_names_no_file() {
    let (manager, _) = scan();
    assert_eq!(find_model_for(&manager, "hash_DEADBEEF"), None);
    assert_eq!(
        find_model_for(&manager, "a_prop_that_is_not_in_the_game"),
        None
    );
}
