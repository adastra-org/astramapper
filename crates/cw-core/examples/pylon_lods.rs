//! Lists the archetypes whose name matches a substring (default: power-line
//! towers) with their `.ytyp` LOD distance, box and flags. Evidence for the
//! LOD band choice of the World view (#9 pylons).
//!
//! ```text
//! cargo run --release -p cw-core --example pylon_lods -- pylon elecpole
//! ```

use std::path::PathBuf;

use cw_core::filetypes::index_archetypes;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn main() {
    let mut needles: Vec<String> = std::env::args().skip(1).map(|a| a.to_lowercase()).collect();
    if needles.is_empty() {
        needles = vec!["pylon".into(), "elecpole".into(), "power_".into()];
    }
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
    let mut rows: Vec<_> = indexes
        .all
        .iter()
        .filter_map(|(hash, info)| {
            let name = names.get(hash)?;
            let lower = name.to_lowercase();
            needles
                .iter()
                .any(|n| lower.contains(n.as_str()))
                .then(|| (name.to_string(), info))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    println!("name | lodDist | bbMin | bbMax | flags");
    for (name, info) in rows {
        println!(
            "{name} | {} | {:?} | {:?} | {:#x}",
            info.lod_dist, info.bb_min, info.bb_max, info.flags
        );
    }
}
