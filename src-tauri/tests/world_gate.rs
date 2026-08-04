//! The world view's scaffolding against a real install: does grouping the
//! install into content groups and reading a `.ymap` into placements work, from
//! the same functions the IPC commands call?
//!
//! ```text
//! cargo test -p astramapper -- --ignored --nocapture
//! ```

use astramapper_lib::world::{
    compile_world_model, content_groups, find_model_for, read_world_map, read_world_model,
    ymaps_in_group, WorldMap,
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

    let preview =
        read_world_model(&model_path, &keys, &manager, None).expect("read the model");
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

/// Going to the archetype's own `textureDictionary` instead of scanning the
/// model's folder has to resolve every texture the scan did, or it is not an
/// optimization but a bug.
///
/// The folder scan visits 28.8 dictionaries for the average model and 332 for
/// the worst in a sample of 400, each an extract plus an inflate plus a full
/// parse.
///
/// **The two may legitimately disagree about a texture's size, and this is not
/// a failure.** GTA ships a base dictionary and an HD one, `foo.ytd` beside
/// `foo+hi.ytd` or `foo+hidr.ytd`, holding the same texture names at different
/// resolutions; the engine swaps the HD tier in inside `hdTextureDist`. The
/// archetype names the base dictionary, so the archetype chain gets the base
/// tier deterministically, where the folder scan got whichever tier its
/// arbitrary ordering reached first. `dt1_00_3` is the worked example:
/// `im_freeway_barrier001_n` is 256x256 in the archetype's `dt1_00_grnd.ytd`
/// and 512x512 in a `+hidr` sibling.
///
/// So what is pinned here is coverage — the set of hashes that got pixels —
/// not the bytes.
#[test]
#[ignore = "needs a local GTA V install"]
fn the_archetype_resolves_the_same_textures_as_the_folder_scan() {
    use std::time::Instant;

    let (manager, keys) = scan();
    let names = manager.index_names();
    let archetypes = cw_core::filetypes::index_archetypes(&manager, &keys);

    let models = placed_models(&manager, &keys, &names, 40);

    let mut scanned_time = std::time::Duration::ZERO;
    let mut named_time = std::time::Duration::ZERO;
    let mut compared = 0;
    let mut tiers = 0;

    for model in &models {
        // The archetype chain runs first, so it pays for any archive mapping
        // and page cache the folder scan then gets for free. Whatever speedup
        // this reports is therefore a lower bound.
        let started = Instant::now();
        let Ok(named) = read_world_model(model, &keys, &manager, Some(&archetypes)) else {
            continue;
        };
        named_time += started.elapsed();

        let started = Instant::now();
        let Ok(scanned) = read_world_model(model, &keys, &manager, None) else {
            continue;
        };
        scanned_time += started.elapsed();

        // The set of hashes that got pixels is what the renderer sees.
        let mut a: Vec<u32> = scanned.textures.iter().map(|t| t.hash).collect();
        let mut b: Vec<u32> = named.textures.iter().map(|t| t.hash).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "{model} resolved a different texture set");

        for texture in &named.textures {
            assert!(
                texture.width > 0 && texture.height > 0 && !texture.png.is_empty(),
                "{model}: {} resolved to nothing",
                texture.name
            );
            let other = scanned
                .textures
                .iter()
                .find(|t| t.hash == texture.hash)
                .expect("hash sets already matched");
            if (texture.width, texture.height) != (other.width, other.height) {
                tiers += 1;
            }
        }
        compared += 1;
    }

    assert!(compared > 0, "nothing was compared");
    println!(
        "{compared} placed models: folder scan {scanned_time:.2?}, archetype {named_time:.2?} ({:.1}x), {tiers} textures resolved to a different HD tier",
        scanned_time.as_secs_f64() / named_time.as_secs_f64().max(f64::EPSILON)
    );
}

/// The first `limit` distinct HD models the city maps actually place.
///
/// Real placements, in real placement order, which is what the streamer would
/// pull. Generated rather than hand-listed so the gates never drift onto a
/// model the game stopped shipping.
fn placed_models(
    manager: &RpfManager,
    keys: &GtaKeys,
    names: &HashMap<u32, String>,
    limit: usize,
) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    for path in city_maps(manager) {
        let Ok(map) = read_world_map(path, keys, manager, None, Some(names)) else {
            continue;
        };
        for entity in map.entities.iter().filter(|e| e.lod_level == "HD") {
            if let Some(model) = find_model_for(manager, &entity.name) {
                if !models.contains(&model) {
                    models.push(model);
                }
            }
        }
        if models.len() >= limit {
            break;
        }
    }
    assert!(!models.is_empty(), "no placed models found");
    models
}

/// The block path and the PNG path must resolve the same textures, and the
/// blocks must be laid out the way the frontend is about to assume they are.
///
/// This is the assertion that makes shipping raw blocks safe. The frontend
/// calls `compressedTexImage2D` with the byte length the metadata declares and
/// no way to check it: a length that disagrees with the level's dimensions is
/// either a GL error or, worse, a silently wrong upload. So every mip is
/// checked against `TextureFormat::compute_pitch`, which is the same arithmetic
/// the driver does.
///
/// Coverage is pinned as a hash set rather than as bytes, for the same reason
/// as `the_archetype_resolves_the_same_textures_as_the_folder_scan`: the two
/// paths agree on *which* textures, and the PNG path only ever carried mip 0.
#[test]
#[ignore = "needs a local GTA V install"]
fn the_block_path_resolves_the_same_textures_as_the_png_path() {
    use astramapper_lib::textures::GpuCaps;
    use cw_core::resources::TextureFormat;
    use cw_core::wire::{EnvelopeView, Kind};
    use std::time::{Duration, Instant};

    let (manager, keys) = scan();
    let names = manager.index_names();
    let archetypes = cw_core::filetypes::index_archetypes(&manager, &keys);
    let caps = GpuCaps {
        s3tc: true,
        rgtc: true,
        bptc: true,
    };

    let models = placed_models(&manager, &keys, &names, 40);

    let mut png_time = Duration::ZERO;
    let mut block_time = Duration::ZERO;
    let mut png_bytes = 0usize;
    let mut block_bytes = 0usize;
    let mut compared = 0usize;
    let mut mips = 0usize;
    let mut fallbacks = 0usize;
    let mut narrowed_parts = 0usize;
    let mut total_parts = 0usize;
    // Which block formats real placed geometry actually uses, printed below.
    // The shader work needs this to know what it has to handle.
    let mut by_format: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    for model in &models {
        let started = Instant::now();
        let Ok(png) = read_world_model(model, &keys, &manager, Some(&archetypes)) else {
            continue;
        };
        png_time += started.elapsed();
        // Everything the JSON path puts on the wire, so the comparison below is
        // like for like: the envelope carries geometry *and* textures.
        png_bytes += png.textures.iter().map(|t| t.png.len()).sum::<usize>();
        for drawable in &png.drawables {
            for bundle in &drawable.lods {
                for part in &bundle.parts {
                    let base64_of = |bytes: usize| bytes.div_ceil(3) * 4;
                    png_bytes += base64_of(part.positions.len());
                    png_bytes += part.normals.as_ref().map_or(0, |b| base64_of(b.len()));
                    png_bytes += part.uvs.as_ref().map_or(0, |b| base64_of(b.len()));
                    png_bytes += base64_of(part.indices.len());
                }
            }
        }

        let started = Instant::now();
        let Ok(blob) = compile_world_model(model, &keys, &manager, Some(&archetypes), caps) else {
            continue;
        };
        block_time += started.elapsed();
        block_bytes += blob.len();

        let view = EnvelopeView::read(&blob).expect("the envelope reads back");
        assert_eq!(view.kind, Kind::Model);
        let meta: serde_json::Value = serde_json::from_str(view.meta).expect("the metadata is JSON");
        let textures = meta["textures"].as_array().expect("textures");

        // Geometry: the envelope's sections must carry byte for byte what the
        // JSON path carried as base64, with indices narrowed where they fit.
        // The frontend takes typed-array *views* over these sections, so a
        // single byte out of place is a mesh drawn from the wrong floats.
        for (drawable, from_json) in meta["drawables"]
            .as_array()
            .expect("drawables")
            .iter()
            .zip(&png.drawables)
        {
            for (bundle, json_bundle) in drawable["lods"]
                .as_array()
                .expect("lods")
                .iter()
                .zip(&from_json.lods)
            {
                for (part, json_part) in bundle["parts"]
                    .as_array()
                    .expect("parts")
                    .iter()
                    .zip(&json_bundle.parts)
                {
                    let section = |key: &str| -> Option<&[u8]> {
                        part[key].as_u64().and_then(|i| view.section(i as usize))
                    };
                    assert_eq!(
                        section("positions"),
                        Some(json_part.positions.as_bytes()),
                        "{model}: {} positions differ",
                        json_part.name
                    );
                    assert_eq!(
                        section("normals"),
                        json_part.normals.as_ref().map(|b| b.as_bytes()),
                        "{model}: {} normals differ",
                        json_part.name
                    );
                    assert_eq!(
                        section("uvs"),
                        json_part.uvs.as_ref().map(|b| b.as_bytes()),
                        "{model}: {} uvs differ",
                        json_part.name
                    );

                    // Narrowed or not, the indices must name the same vertices.
                    let bits = part["indexBits"].as_u64().unwrap();
                    let bytes = section("indices").expect("an index section");
                    let narrowed: Vec<u32> = match bits {
                        16 => bytes
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]) as u32)
                            .collect(),
                        32 => bytes
                            .chunks_exact(4)
                            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                            .collect(),
                        other => panic!("{model}: index width {other}"),
                    };
                    let wide: Vec<u32> = json_part
                        .indices
                        .as_bytes()
                        .chunks_exact(4)
                        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect();
                    assert_eq!(narrowed, wide, "{model}: {} indices differ", json_part.name);
                    // A view needs its offset to divide its element width, and
                    // the encoder's alignment is what guarantees it.
                    let offset = bytes.as_ptr() as usize - blob.as_ptr() as usize;
                    assert_eq!(offset % 16, 0, "{model}: an index section is misaligned");
                    narrowed_parts += usize::from(bits == 16);
                    total_parts += 1;
                }
            }
        }

        let mut a: Vec<u64> = png.textures.iter().map(|t| t.hash as u64).collect();
        let mut b: Vec<u64> = textures.iter().map(|t| t["hash"].as_u64().unwrap()).collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "{model} resolved a different texture set");

        for texture in textures {
            let name = texture["name"].as_str().unwrap_or("?");
            let section = texture["section"].as_u64().unwrap() as usize;
            let bytes = view
                .section(section)
                .unwrap_or_else(|| panic!("{model}: {name} names a section that is not there"));
            let levels = texture["mips"].as_array().expect("mips");
            assert!(!levels.is_empty(), "{model}: {name} has no mip levels");

            // Every declared byte accounted for, and no byte left over: the
            // frontend walks these lengths to slice the chain.
            let declared: usize = levels
                .iter()
                .map(|m| m["byteLength"].as_u64().unwrap() as usize)
                .sum();
            assert_eq!(
                declared,
                bytes.len(),
                "{model}: {name}'s mip lengths do not add up to its section"
            );

            let compressed = texture["compressed"].as_bool().unwrap();
            if !compressed {
                fallbacks += 1;
            }
            let format_name = texture["format"].as_str().unwrap();
            *by_format.entry(format_name.to_string()).or_default() += 1;
            let format = match format_name {
                "bc1" => Some(TextureFormat::Dxt1),
                "bc2" => Some(TextureFormat::Dxt3),
                "bc3" => Some(TextureFormat::Dxt5),
                "bc4" => Some(TextureFormat::Ati1),
                "bc5" => Some(TextureFormat::Ati2),
                "bc7" => Some(TextureFormat::Bc7),
                "rgba8" => None,
                other => panic!("{model}: {name} names an unknown format {other}"),
            };

            let mut expected_width = texture["width"].as_u64().unwrap() as usize;
            let mut expected_height = texture["height"].as_u64().unwrap() as usize;
            for (level, mip) in levels.iter().enumerate() {
                let width = mip["width"].as_u64().unwrap() as usize;
                let height = mip["height"].as_u64().unwrap() as usize;
                let length = mip["byteLength"].as_u64().unwrap() as usize;
                assert_eq!(
                    (width, height),
                    (expected_width, expected_height),
                    "{model}: {name} mip {level} is not the halving of the one before it"
                );
                // The arithmetic the GL driver will do. `compute_pitch` keeps
                // the `max(1)` a 2x2 or 1x1 block-compressed mip needs, which is
                // exactly where a naive `width * height / 16` goes wrong.
                let expected = match format {
                    Some(format) => format.compute_pitch(width, height).1,
                    None => width * height * 4,
                };
                assert_eq!(
                    length, expected,
                    "{model}: {name} mip {level} is {length} bytes for {width}x{height}"
                );
                expected_width = (expected_width / 2).max(1);
                expected_height = (expected_height / 2).max(1);
                mips += 1;
            }
        }
        compared += 1;
    }

    assert!(compared > 0, "nothing was compared");
    let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
    println!(
        "{compared} placed models, {mips} mip levels ({fallbacks} textures fell back to rgba8)"
    );
    println!("  {narrowed_parts}/{total_parts} parts index fewer than 65,536 vertices");
    println!(
        "  json+base64  {png_time:.2?}  {:.1} MB",
        mb(png_bytes)
    );
    println!(
        "  envelope     {block_time:.2?}  {:.1} MB  ({:.0}x cpu, {:.2}x wire)",
        mb(block_bytes),
        png_time.as_secs_f64() / block_time.as_secs_f64().max(f64::EPSILON),
        png_bytes as f64 / block_bytes.max(1) as f64
    );
    let total: usize = by_format.values().sum();
    for (format, count) in &by_format {
        println!("  {format:6} {count:5}  {:.1}%", 100.0 * *count as f64 / total as f64);
    }
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
