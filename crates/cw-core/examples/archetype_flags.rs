//! Evaluates `classify_archetype` over a real install against the archetype-name
//! tokens the frontend's `streamPenalty` falls back to, and prints samples of
//! what the data classifies that the name does not. The evidence behind the
//! thresholds in `filetypes/class.rs`.
//!
//! ```text
//! cargo run --release -p cw-core --example archetype_flags
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use cw_core::filetypes::{classify_archetype, index_archetypes, ArchetypeClass};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

const TERRAIN: &[&str] = &["terrain", "ter", "terr", "grnd"];
const DECAL: &[&str] = &["decal", "decals", "dcl"];
const VEG: &[&str] = &[
    "tree",
    "trees",
    "bush",
    "bushes",
    "palm",
    "plant",
    "plants",
    "veg",
    "vegetation",
    "grass",
    "cactus",
    "fern",
    "shrub",
    "hedge",
    "weed",
    "weeds",
    "flower",
    "flowers",
    "reed",
    "reeds",
    "vine",
    "ivy",
    "foliage",
    "yucca",
    "agave",
    "sapling",
];

/// The frontend's `classifyName`, for comparison.
fn class_by_name(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    let mut found = "none";
    for token in lower.split(|c: char| !c.is_ascii_lowercase()) {
        if TERRAIN.contains(&token) {
            return "terrain";
        }
        if DECAL.contains(&token) {
            found = "decal";
        } else if VEG.contains(&token) && found == "none" {
            found = "veg";
        }
    }
    found
}

fn main() {
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let Ok(exe_data) = std::fs::read(folder.join("GTA5.exe")) else {
        eprintln!("no install at {}", folder.display());
        return;
    };
    let keys = GtaKeys::from_exe(&exe_data).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");
    let names = manager.index_names();
    let indexes = index_archetypes(&manager, &keys);

    let started = Instant::now();
    let classes: Vec<(u32, ArchetypeClass)> = indexes
        .all
        .iter()
        .map(|(hash, info)| (*hash, classify_archetype(info)))
        .collect();
    println!(
        "{} archetypes classified in {:.2?}",
        classes.len(),
        started.elapsed()
    );

    // (data class, name class) -> count, over archetypes whose name resolves.
    let mut table: BTreeMap<(String, &str), usize> = BTreeMap::new();
    let mut samples: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (hash, class) in &classes {
        let Some(name) = names.get(hash) else {
            continue;
        };
        let by_name = class_by_name(name);
        *table.entry((format!("{class:?}"), by_name)).or_default() += 1;
        if by_name == "none" && *class != ArchetypeClass::None {
            let list = samples.entry(format!("{class:?}")).or_default();
            if list.len() < 16 {
                list.push(name);
            }
        }
    }
    println!("data class x name class (named archetypes only):");
    for ((data, name), count) in &table {
        println!("  {data:>10} / name {name:<8} {count}");
    }
    for (class, list) in &samples {
        println!("{class} by data, none by name: {}", list.join(", "));
    }
}
