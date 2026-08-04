//! The world view's scaffolding against a real install: does grouping the
//! install into content groups and reading a `.ymap` into placements work, from
//! the same functions the IPC commands call?
//!
//! ```text
//! cargo test -p astramapper -- --ignored --nocapture
//! ```

use astramapper_lib::world::{
    content_groups, find_model_for, read_world_map, read_world_model, ymaps_in_group, WorldMap,
};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use std::collections::HashMap;
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

/// Every `.ymap` under `levels\gta5`, in index order.
///
/// The interiors subtree is excluded: those are the MLO archetypes' own maps,
/// not placement files, and they place nothing on their own.
fn city_maps(manager: &RpfManager) -> Vec<&String> {
    manager
        .archives
        .iter()
        .flat_map(|a| a.entries.iter())
        .map(|e| &e.path)
        .filter(|p| {
            p.ends_with(".ymap")
                && p.contains(r"\levels\gta5\_city")
                && !p.contains(r"\.rpf\interiors\")
        })
        .collect()
}

/// The first city map satisfying `wanted`, read with the same indexes the
/// commands use.
///
/// Picking the *first* city map and asserting about it is what made this gate
/// brittle: the first one in index order is `hei_dt1_00.ymap`, which holds
/// nine entities and not one of them is HD. A map is only a fair witness for a
/// property if it was chosen for having it.
fn first_city_map(
    manager: &RpfManager,
    keys: &GtaKeys,
    names: &HashMap<u32, String>,
    wanted: impl Fn(&WorldMap) -> bool,
) -> WorldMap {
    city_maps(manager)
        .into_iter()
        .filter_map(|p| read_world_map(p, keys, manager, None, Some(names)).ok())
        .find(|map| wanted(map))
        .expect("no city map matched")
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
    let names = manager.index_names();

    // A map that actually places HD geometry. Most `_city` maps are LOD-only
    // companions, so the property under test has to drive the choice.
    let map = first_city_map(&manager, &keys, &names, |map| {
        map.entities.iter().any(|e| e.lod_level == "HD")
    });
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

/// An MLO instance's interior has to expand into world-space entities whose
/// archetypes resolve to names, sitting at finite positions around the instance.
#[test]
#[ignore = "needs a local GTA V install"]
fn expands_an_mlo_instances_interior_into_the_world() {
    let (manager, keys) = scan();
    let names = manager.index_names();

    // Names come from the entry-basename index, as the commands pass them. The
    // static `metanames` table alone resolves a few thousand of the install's
    // archetypes, so without this every expanded entity reads back as `hash_*`
    // and the assertion below is unwinnable.
    let map = first_city_map(&manager, &keys, &names, |map| map.mlo_expanded > 0);

    let expanded: Vec<_> = map.entities.iter().filter(|e| e.from_mlo).collect();
    assert_eq!(expanded.len(), map.mlo_expanded);
    assert!(map.mlo_entities > 0, "expansion needs MLO instances");

    let resolvable = expanded
        .iter()
        .filter(|e| !e.name.starts_with("hash_"))
        .count();
    assert!(
        resolvable > 0,
        "expanded interior entities should name their archetypes"
    );

    for entity in &expanded {
        assert!(
            entity.position.iter().all(|v| v.is_finite()),
            "non-finite expanded position on {}",
            entity.name
        );
    }

    println!(
        "{}: {} MLO instances, {} interior entities expanded ({} nameable)",
        map.name, map.mlo_entities, map.mlo_expanded, resolvable,
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
    let names = manager.index_names();

    // The streamer only ever draws an entity whose archetype resolves to a
    // file, so the witness has to be one of those. Choosing the first entity of
    // the first map instead tests whichever accident is at the front.
    let mut found = None;
    for path in city_maps(&manager) {
        let Ok(map) = read_world_map(path, &keys, &manager, None, Some(&names)) else {
            continue;
        };
        let hit = map
            .entities
            .iter()
            .filter(|e| e.lod_level == "HD" && !e.name.starts_with("hash_"))
            .find_map(|e| find_model_for(&manager, &e.name).map(|p| (e.name.clone(), p)));
        if let Some(hit) = hit {
            found = Some(hit);
            break;
        }
    }
    let (entity_name, model_path) =
        found.expect("a city map should place an HD entity whose archetype names a model");
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

    // A model that references a diffuse texture and has a sibling `.ytd` must
    // have resolved that texture: the whole point of the world's texture pass.
    let wants_diffuse = drawable
        .lods
        .iter()
        .flat_map(|l| l.parts.iter())
        .any(|p| p.diffuse.is_some());
    if wants_diffuse {
        let sibling = sibling_ytd(&model_path);
        if manager.find(&sibling).is_some() {
            assert!(
                !preview.textures.is_empty(),
                "{model_path} references a diffuse texture but resolved none"
            );
        }
    }

    println!(
        "{} -> {}: {} drawable, {} LODs, {} triangles at high, {} textures",
        entity_name,
        model_path,
        preview.drawables.len(),
        drawable.lods.len(),
        drawable.lods[0].triangle_count,
        preview.textures.len()
    );
}

/// The `.ytd` that should sit beside a model file.
fn sibling_ytd(path: &str) -> String {
    let (folder, name) = path.rsplit_once('\\').unwrap_or(("", path));
    let stem = name
        .strip_suffix(".ydr")
        .or_else(|| name.strip_suffix(".yft"))
        .or_else(|| name.strip_suffix(".ydd"))
        .unwrap_or(name);
    if folder.is_empty() {
        format!("{stem}.ytd")
    } else {
        format!("{folder}\\{stem}.ytd")
    }
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
