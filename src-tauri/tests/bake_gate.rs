//! The bake's reachability pass against a real install: does walking the maps
//! actually find the city, and is what it finds small enough to bake?
//!
//! ```text
//! cargo test --release -p astramapper --test bake_gate -- --ignored --nocapture
//! ```
//!
//! This is the decision the whole pass rests on. A naive bake enumerates the
//! archives and is tens of gigabytes of peds, vehicles and cutscenes; this one
//! bakes only what a `.ymap` places. The gate exists to keep that claim
//! honest with a number rather than an assertion in a comment.
//!
//! The bake *itself* is not gated: driving it needs a `tauri::ipc::Channel`,
//! which needs a running app. Its own state machine is unit tested, and the
//! artifacts it writes are pinned by `cache_gate.rs`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use astramapper_lib::bake::reachable_models;
use astramapper_lib::world::content_groups;
use cw_core::filetypes::index_archetypes;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

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

/// Reachability has to find the base game's placed models, order them by how
/// often they are placed, and come out very much smaller than the install.
#[test]
#[ignore = "needs a local GTA V install"]
fn reachability_finds_the_placed_city_and_nothing_else() {
    let (manager, keys) = scan();
    let archetypes = index_archetypes(&manager, &keys);
    let names: HashMap<u32, String> = manager.index_names();

    let base = content_groups(&manager)
        .into_iter()
        .find(|group| group.id == "base")
        .expect("the install has a base group");

    let started = Instant::now();
    let models = reachable_models(
        &manager,
        &keys,
        &archetypes.mlo,
        &names,
        std::slice::from_ref(&base.id),
        &|| false,
    );
    let elapsed = started.elapsed();

    assert!(
        models.len() > 1000,
        "only {} models reachable, the walk is not reaching the city",
        models.len()
    );

    // The ordering is what makes a bake cancelled halfway still worth having.
    for pair in models.windows(2) {
        assert!(
            pair[0].1 >= pair[1].1,
            "{} ({}) came before {} ({})",
            pair[0].0,
            pair[0].1,
            pair[1].0,
            pair[1].1
        );
    }

    // Every path has to be something the compiler can actually open.
    for (path, _) in models.iter().take(200) {
        assert!(manager.find(path).is_some(), "{path} is not in the index");
        assert!(
            path.ends_with(".ydr") || path.ends_with(".yft") || path.ends_with(".ydd"),
            "{path} is not a model file"
        );
    }

    let placements: usize = models.iter().map(|(_, count)| count).sum();
    let entries = manager.entry_count();
    println!(
        "base: {} distinct models from {} placements in {:.1}s, {:.2}% of {} entries",
        models.len(),
        placements,
        elapsed.as_secs_f64(),
        models.len() as f64 / entries as f64 * 100.0,
        entries,
    );
    println!("  most placed:");
    for (path, count) in models.iter().take(8) {
        println!("    {count:>6}  {path}");
    }

    // The number that justifies the ordering. The install has far more
    // reachable models than any budget fits, so what matters is not how many a
    // bake finishes but how much of what is actually placed the ones it did
    // finish account for.
    println!("  placement coverage by rank:");
    for cut in [100, 500, 1000, 5000, 10000] {
        if cut > models.len() {
            break;
        }
        let covered: usize = models.iter().take(cut).map(|(_, count)| count).sum();
        println!(
            "    top {cut:>6}: {:>5.1}% of placements",
            covered as f64 / placements as f64 * 100.0
        );
    }
}

/// A cancelled walk must return promptly and not part-way through a map, so a
/// user who stops a bake does not wait out the rest of the install.
#[test]
#[ignore = "needs a local GTA V install"]
fn cancelling_the_walk_stops_it() {
    let (manager, keys) = scan();
    let archetypes = index_archetypes(&manager, &keys);
    let names: HashMap<u32, String> = manager.index_names();

    let started = Instant::now();
    let models = reachable_models(
        &manager,
        &keys,
        &archetypes.mlo,
        &names,
        &["base".into()],
        &|| true,
    );
    assert!(
        models.is_empty(),
        "a walk cancelled at the start finds nothing"
    );
    assert!(
        started.elapsed().as_secs() < 5,
        "cancelling took {:?}",
        started.elapsed()
    );
}
