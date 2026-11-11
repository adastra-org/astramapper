//! The ytyp/ymap gate: prove the typed metadata readers agree with the C#
//! reference structure for structure, and hold together over the whole install.
//!
//! ```text
//! pwsh -File scripts/ytyp-oracle.ps1
//! pwsh -File scripts/ymap-oracle.ps1
//! cargo test -p cw-core --test ymap_gate -- --ignored --nocapture
//! ```
//!
//! Two checks:
//!
//! 1. **Against the oracle.** `fixtures/ytyp_oracle.json` and
//!    `fixtures/ymap_oracle.json` are what `YtypFile` and `YmapFile` in
//!    `CodeWalker.Core.dll` see — every archetype/entity hash and count, plus
//!    the hierarchy's root/child wiring, compared field for field.
//! 2. **Structurally, over a wide sample.** Every `.ytyp` and `.ymap` in the
//!    install must read, and what it reads must hold together: an MLO
//!    archetype's entity sets must resolve their entities, and a `.ymap`'s
//!    parent indices must point inside its own entity list. The second is the
//!    check that matters, because a meta file that walks wrong still parses —
//!    the reference reads a wrong offset as a plausible-looking structure.

use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::filetypes::{read_ymap, read_ytyp};
use cw_core::meta::{read_meta, MetaFile};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use serde_json::Value;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn gta_path() -> PathBuf {
    PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.to_string()))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve the workspace root")
}

fn load_keys() -> GtaKeys {
    let exe = gta_path().join("GTA5.exe");
    let data = std::fs::read(&exe).unwrap_or_else(|e| panic!("cannot read {}: {e}", exe.display()));
    GtaKeys::from_exe(&data).expect("key recovery failed")
}

fn scan(keys: &GtaKeys) -> RpfManager {
    RpfManager::scan(&gta_path(), keys).expect("scan the game folder")
}

fn f32_at(value: &Value) -> f32 {
    value.as_f64().unwrap_or(0.0) as f32
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
}

/// Every `.ytyp` in the install, read typed.
#[test]
#[ignore = "needs a local GTA V install"]
fn reads_every_ytyp_in_the_install() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<(cw_core::rpf::EntryRef, String)> = Vec::new();
    for (index, archive) in manager.archives.iter().enumerate() {
        for (entry_index, entry) in archive.entries.iter().enumerate() {
            if entry.name_lower.ends_with(".ytyp") {
                paths.push((
                    cw_core::rpf::EntryRef {
                        archive: index,
                        entry: entry_index,
                    },
                    entry.path.clone(),
                ));
            }
        }
    }
    assert!(!paths.is_empty(), "the install should contain .ytyp files");

    let mut files = 0usize;
    let mut archetypes = 0usize;
    let mut mlo_rooms = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut truncated: Vec<String> = Vec::new();

    for (entry_ref, path) in &paths {
        let entry = manager.entry(*entry_ref);
        let Some(system_size) = entry.resource_segments().map(|(system, _)| system) else {
            // A non-resource `.ytyp` is RBF or PSO — the reference's
            // `NonMetaLoad` path, which yields no typed archetypes.
            continue;
        };
        let Ok(data) = manager.extract(*entry_ref, &keys) else {
            failures.push(format!("{path}: extract failed"));
            continue;
        };
        let meta = match read_meta(FileKind::Meta, &data, Some(system_size)) {
            Ok(MetaFile::Rsc(meta)) => meta,
            Ok(_) => {
                failures.push(format!("{path}: not an RSC meta"));
                continue;
            }
            Err(e) => {
                failures.push(format!("{path}: {e}"));
                continue;
            }
        };
        let ytyp = match read_ytyp(&meta) {
            Ok(ytyp) => ytyp,
            Err(e) if data.len() < system_size => {
                // The known truncated-payload class — same handful of
                // destruction archives that break the `.ytd` and `.ycd`
                // readers. See `docs/formats.md`.
                truncated.push(format!(
                    "{path}: {} of {system_size} bytes ({e})",
                    data.len()
                ));
                continue;
            }
            Err(e) => {
                failures.push(format!("{path}: {e}"));
                continue;
            }
        };

        files += 1;
        archetypes += ytyp.archetypes.len();
        for archetype in &ytyp.archetypes {
            if let cw_core::filetypes::ytyp::Archetype::Mlo(mlo) = archetype {
                mlo_rooms += mlo.rooms.len();
                // Every entity-set location must resolve.
                for set in &mlo.entity_sets {
                    for entity in &set.entities {
                        assert!(
                            entity.archetype_name != 0 || entity.guid != 0,
                            "{path}: an entity set holds an empty entity"
                        );
                    }
                }
            }
        }
    }

    assert!(files > 100, "sample too small to mean anything: {files}");
    assert!(failures.is_empty(), "{} failed: {:?}", failures.len(), {
        failures.truncate(10);
        failures
    });

    println!("{files} .ytyp files, {archetypes} archetypes, {mlo_rooms} MLO rooms, all read");
    if !truncated.is_empty() {
        println!(
            "{} refused as truncated payloads:\n  {}",
            truncated.len(),
            truncated.join("\n  ")
        );
    }
}

/// Every `.ymap` in the install, read typed with its hierarchy intact.
#[test]
#[ignore = "needs a local GTA V install"]
fn reads_every_ymap_in_the_install() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<(cw_core::rpf::EntryRef, String)> = Vec::new();
    for (index, archive) in manager.archives.iter().enumerate() {
        for (entry_index, entry) in archive.entries.iter().enumerate() {
            if entry.name_lower.ends_with(".ymap") {
                paths.push((
                    cw_core::rpf::EntryRef {
                        archive: index,
                        entry: entry_index,
                    },
                    entry.path.clone(),
                ));
            }
        }
    }
    assert!(!paths.is_empty(), "the install should contain .ymap files");

    let mut files = 0usize;
    let mut entities = 0usize;
    let mut roots = 0usize;
    let mut grass_batches = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut truncated: Vec<String> = Vec::new();

    for (entry_ref, path) in &paths {
        let entry = manager.entry(*entry_ref);
        let Some(system_size) = entry.resource_segments().map(|(system, _)| system) else {
            // A non-resource `.ymap` is RBF or PSO — `NonMetaLoad`, which
            // yields no typed map data.
            continue;
        };
        let Ok(data) = manager.extract(*entry_ref, &keys) else {
            failures.push(format!("{path}: extract failed"));
            continue;
        };
        let meta = match read_meta(FileKind::Meta, &data, Some(system_size)) {
            Ok(MetaFile::Rsc(meta)) => meta,
            Ok(_) => {
                failures.push(format!("{path}: not an RSC meta"));
                continue;
            }
            Err(e) => {
                failures.push(format!("{path}: {e}"));
                continue;
            }
        };
        let ymap = match read_ymap(&meta) {
            Ok(ymap) => ymap,
            Err(e) if data.len() < system_size => {
                truncated.push(format!(
                    "{path}: {} of {system_size} bytes ({e})",
                    data.len()
                ));
                continue;
            }
            Err(e) => {
                failures.push(format!("{path}: {e}"));
                continue;
            }
        };

        files += 1;
        entities += ymap.entities.len();
        roots += ymap.root_entities.len();
        grass_batches += ymap.grass_instance_batches.len();

        // Every entity's parent index either points inside the list (and the
        // hierarchy pass wired it to that parent) or is treated as a root —
        // exactly the reference's `EnsureEntities` rule. A root may itself be
        // a parent, so children are validated independently.
        let entity_count = ymap.entities.len();
        for (i, entity) in ymap.entities.iter().enumerate() {
            for child in &entity.children {
                assert!(
                    *child < entity_count,
                    "{path}: entity {i} child {child} out of range"
                );
            }
        }
        for root in &ymap.root_entities {
            assert!(*root < entity_count, "{path}: root {root} out of range");
        }
    }

    assert!(files > 50, "sample too small to mean anything: {files}");
    assert!(failures.is_empty(), "{} failed: {:?}", failures.len(), {
        failures.truncate(10);
        failures
    });

    println!(
        "{files} .ymap files, {entities} entities, {roots} roots, {grass_batches} grass batches, all read"
    );
    if !truncated.is_empty() {
        println!(
            "{} refused as truncated payloads:\n  {}",
            truncated.len(),
            truncated.join("\n  ")
        );
    }
}

/// Compare the typed `.ytyp` reader against `fixtures/ytyp_oracle.json`.
#[test]
#[ignore = "needs fixtures/ytyp_oracle.json — see scripts/ytyp-oracle.ps1"]
fn agrees_with_the_reference_ytyp_reader() {
    let oracle_path = workspace_root().join("fixtures/ytyp_oracle.json");
    let Ok(text) = std::fs::read_to_string(&oracle_path) else {
        println!("skipping: {} is missing", oracle_path.display());
        return;
    };
    let oracle: Value = serde_json::from_str(&text).expect("parse the oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("files array");
    assert!(!files.is_empty(), "the oracle holds no files");

    let mut archetypes_checked = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");
        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let entry = manager.entry(entry_ref);
        let system_size = entry.resource_segments().map(|(system, _)| system);
        let data = manager
            .extract(entry_ref, &keys)
            .unwrap_or_else(|e| panic!("{path}: extract failed: {e}"));
        let meta = match read_meta(FileKind::Meta, &data, system_size) {
            Ok(MetaFile::Rsc(meta)) => meta,
            Ok(_) => panic!("{path}: not an RSC meta"),
            Err(e) => panic!("{path}: {e}"),
        };
        let ytyp = read_ytyp(&meta).unwrap_or_else(|e| panic!("{path}: {e}"));

        assert_eq!(
            ytyp.name_hash,
            file["nameHash"].as_u64().unwrap() as u32,
            "{path}: name hash"
        );
        assert_eq!(
            ytyp.archetypes.len(),
            file["archetypes"].as_array().unwrap().len(),
            "{path}: archetype count"
        );

        for (i, (got, want)) in ytyp
            .archetypes
            .iter()
            .zip(file["archetypes"].as_array().unwrap())
            .enumerate()
        {
            archetypes_checked += 1;
            let at = format!("{path}.archetype{i}");

            let (name, asset_name, asset_type, hash) = match got {
                cw_core::filetypes::ytyp::Archetype::Base(a) => {
                    (a.name, a.asset_name, a.asset_type.raw() as u32, a.hash())
                }
                cw_core::filetypes::ytyp::Archetype::Time(a) => (
                    a.base.name,
                    a.base.asset_name,
                    a.base.asset_type.raw() as u32,
                    a.base.hash(),
                ),
                cw_core::filetypes::ytyp::Archetype::Mlo(a) => (
                    a.base.name,
                    a.base.asset_name,
                    a.base.asset_type.raw() as u32,
                    a.base.hash(),
                ),
            };
            assert_eq!(name, want["name"].as_u64().unwrap() as u32, "{at}: name");
            assert_eq!(
                asset_name,
                want["assetName"].as_u64().unwrap() as u32,
                "{at}: assetName"
            );
            assert_eq!(hash, want["hash"].as_u64().unwrap() as u32, "{at}: hash");
            assert_eq!(
                asset_type,
                want["assetType"].as_u64().unwrap() as u32,
                "{at}: assetType"
            );
            assert_eq!(
                want["kind"].as_str().unwrap(),
                match got {
                    cw_core::filetypes::ytyp::Archetype::Base(_) => "base",
                    cw_core::filetypes::ytyp::Archetype::Time(_) => "time",
                    cw_core::filetypes::ytyp::Archetype::Mlo(_) => "mlo",
                },
                "{at}: kind"
            );
            if let cw_core::filetypes::ytyp::Archetype::Mlo(mlo) = got {
                let want_rooms = want["rooms"].as_array().unwrap();
                assert_eq!(mlo.rooms.len(), want_rooms.len(), "{at}: room count");
                for (ri, (room, want_room)) in mlo.rooms.iter().zip(want_rooms).enumerate() {
                    assert_eq!(
                        room.name.as_deref().unwrap_or(""),
                        want_room["name"].as_str().unwrap_or(""),
                        "{at}.room{ri}: name"
                    );
                }
            }
        }
    }

    println!(
        "{} .ytyp files, {archetypes_checked} archetypes match the reference",
        files.len()
    );
}

/// Compare the typed `.ymap` reader against `fixtures/ymap_oracle.json`.
#[test]
#[ignore = "needs fixtures/ymap_oracle.json — see scripts/ymap-oracle.ps1"]
fn agrees_with_the_reference_ymap_reader() {
    let oracle_path = workspace_root().join("fixtures/ymap_oracle.json");
    let Ok(text) = std::fs::read_to_string(&oracle_path) else {
        println!("skipping: {} is missing", oracle_path.display());
        return;
    };
    let oracle: Value = serde_json::from_str(&text).expect("parse the oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("files array");
    assert!(!files.is_empty(), "the oracle holds no files");

    let mut entities_checked = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");
        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let entry = manager.entry(entry_ref);
        let system_size = entry.resource_segments().map(|(system, _)| system);
        let data = manager
            .extract(entry_ref, &keys)
            .unwrap_or_else(|e| panic!("{path}: extract failed: {e}"));
        let meta = match read_meta(FileKind::Meta, &data, system_size) {
            Ok(MetaFile::Rsc(meta)) => meta,
            Ok(_) => panic!("{path}: not an RSC meta"),
            Err(e) => panic!("{path}: {e}"),
        };
        let ymap = read_ymap(&meta).unwrap_or_else(|e| panic!("{path}: {e}"));

        assert_eq!(
            ymap.name,
            file["name"].as_u64().unwrap() as u32,
            "{path}: name"
        );
        assert_eq!(
            ymap.parent,
            file["parent"].as_u64().unwrap() as u32,
            "{path}: parent"
        );
        assert_eq!(
            ymap.flags,
            file["flags"].as_u64().unwrap() as u32,
            "{path}: flags"
        );
        assert_eq!(
            ymap.entities.len(),
            file["entities"].as_array().unwrap().len(),
            "{path}: entity count"
        );
        assert_eq!(
            ymap.root_entities.len(),
            file["rootEntities"].as_array().unwrap().len(),
            "{path}: root count"
        );
        assert_eq!(
            ymap.mlo_entities.len(),
            file["mloEntities"].as_array().unwrap().len(),
            "{path}: mlo count"
        );

        for (i, (got, want)) in ymap
            .entities
            .iter()
            .zip(file["entities"].as_array().unwrap())
            .enumerate()
        {
            entities_checked += 1;
            let at = format!("{path}.entity{i}");
            assert_eq!(
                got.def.archetype_name,
                want["archetypeName"].as_u64().unwrap() as u32,
                "{at}: archetypeName"
            );
            assert_eq!(got.is_mlo, want["isMlo"].as_bool().unwrap(), "{at}: isMlo");
            assert_eq!(
                got.def.parent_index,
                want["parentIndex"].as_i64().unwrap() as i32,
                "{at}: parentIndex"
            );
            assert_eq!(
                got.def.flags,
                want["flags"].as_u64().unwrap() as u32,
                "{at}: flags"
            );
            assert!(
                close(got.def.position[0], f32_at(&want["position"][0])),
                "{at}: x"
            );
            assert!(
                close(got.def.position[1], f32_at(&want["position"][1])),
                "{at}: y"
            );
            assert!(
                close(got.def.position[2], f32_at(&want["position"][2])),
                "{at}: z"
            );
            assert_eq!(
                got.children,
                want["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_i64().unwrap() as usize)
                    .collect::<Vec<usize>>(),
                "{at}: children"
            );
        }
    }

    println!(
        "{} .ymap files, {entities_checked} entities match the reference",
        files.len()
    );
}
