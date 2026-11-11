//! Over every `.ymap` of an install: for entities whose archetype name contains
//! an argument, the distribution of `(lodLevel, entity lodDist, archetype
//! lodDist)` and the parent they hang from. Evidence for the LOD band choice of
//! the World view.
//!
//! ```text
//! cargo run --release -p cw-core --example entity_lods -- prop_pylon_0 prop_tree_
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::filetypes::{index_archetypes, read_ymap};
use cw_core::meta::{read_meta, MetaFile};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

/// `(lod level, entity lodDist, archetype lodDist, has a parent)`.
type Row = (String, i64, i64, bool);

fn main() {
    let needles: Vec<String> = std::env::args().skip(1).map(|a| a.to_lowercase()).collect();
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let exe = std::fs::read(folder.join("GTA5.exe")).expect("GTA5.exe");
    let keys = GtaKeys::from_exe(&exe).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");
    let names = manager.index_names();
    let indexes = index_archetypes(&manager, &keys);

    // archetype name -> (lod level, entity lodDist, archetype lodDist, has parent) -> count
    let mut table: BTreeMap<String, BTreeMap<Row, usize>> = BTreeMap::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if !entry.name_lower.ends_with(".ymap") {
                continue;
            }
            let Some(r) = manager.find(&entry.path) else {
                continue;
            };
            let Some((size, _)) = entry.resource_segments() else {
                continue;
            };
            let Ok(data) = manager.extract(r, &keys) else {
                continue;
            };
            let Ok(MetaFile::Rsc(meta)) = read_meta(FileKind::Meta, &data, Some(size)) else {
                continue;
            };
            let Ok(ymap) = read_ymap(&meta) else { continue };
            for (i, e) in ymap.entities.iter().enumerate() {
                let Some(name) = names.get(&e.def.archetype_name) else {
                    continue;
                };
                let lower = name.to_lowercase();
                if !needles.iter().any(|n| lower.contains(n.as_str())) {
                    continue;
                }
                if std::env::var("ENTITY_POSITIONS").is_ok() {
                    let parent = usize::try_from(e.def.parent_index)
                        .ok()
                        .and_then(|p| ymap.entities.get(p))
                        .and_then(|p| names.get(&p.def.archetype_name))
                        .map(|s| s.as_str())
                        .unwrap_or("-");
                    println!(
                        "@ {} #{i} {:?} lodDist {} childLodDist {} children {} pos {:?} parent {} ({})",
                        name, e.def.lod_level, e.def.lod_dist, e.def.child_lod_dist, e.children.len(), e.def.position, e.def.parent_index, parent
                    );
                }
                let arch = indexes
                    .all
                    .get(&e.def.archetype_name)
                    .map(|a| a.lod_dist as i64)
                    .unwrap_or(-9);
                *table
                    .entry(name.to_string())
                    .or_default()
                    .entry((
                        format!("{:?}", e.def.lod_level),
                        e.def.lod_dist as i64,
                        arch,
                        e.def.parent_index >= 0,
                    ))
                    .or_default() += 1;
            }
        }
    }
    println!("name | lodLevel | entity lodDist | archetype lodDist | has parent | count");
    for (name, rows) in table {
        for ((level, dist, arch, parent), n) in rows {
            println!("{name} | {level} | {dist} | {arch} | {parent} | {n}");
        }
    }
}
