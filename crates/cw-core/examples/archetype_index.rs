//! Times building the archetype index over a real install, and says what it
//! found. This is the pass that used to stall the first map placing an
//! interior.
//!
//! ```text
//! cargo run --release --example archetype_index
//! ```

use std::path::PathBuf;
use std::time::Instant;

use cw_core::filetypes::index_archetypes;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn main() {
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let Ok(exe_data) = std::fs::read(folder.join("GTA5.exe")) else {
        eprintln!("no install at {}", folder.display());
        return;
    };
    let keys = GtaKeys::from_exe(&exe_data).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");

    let ytyps = manager
        .archives
        .iter()
        .flat_map(|a| a.entries.iter())
        .filter(|e| e.name_lower.ends_with(".ytyp"))
        .count();

    let started = Instant::now();
    let indexes = index_archetypes(&manager, &keys);
    let elapsed = started.elapsed();

    let with_physics = indexes
        .all
        .values()
        .filter(|a| a.physics_dictionary != 0)
        .count();
    let with_textures = indexes
        .all
        .values()
        .filter(|a| a.texture_dictionary != 0)
        .count();

    println!("{ytyps} .ytyp files indexed in {elapsed:.2?}");
    println!(
        "{} archetypes ({} interiors), {} name a physics dictionary, {} name a texture dictionary",
        indexes.all.len(),
        indexes.mlo.len(),
        with_physics,
        with_textures
    );
}
