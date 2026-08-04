//! How much work resolving a streamed model's textures actually costs.
//!
//! `resolve_world_textures` walks every `.ytd` in the model's folder, and each
//! one is an extract plus an inflate plus a full dictionary parse. This counts
//! how many of those a real streaming session pays for, and how many the
//! archetype's own `textureDictionary` would have answered outright.
//!
//! ```text
//! cargo run --release --example texture_chain
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use cw_core::filetypes::index_archetypes;
use cw_core::rpf::RpfManager;
use cw_core::utils::{gen_hash, GtaKeys};

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

/// How many models to resolve. A streaming session pulls this many in well
/// under a minute of flying.
const SAMPLE: usize = 400;

fn main() {
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let Ok(exe_data) = std::fs::read(folder.join("GTA5.exe")) else {
        eprintln!("no install at {}", folder.display());
        return;
    };
    let keys = GtaKeys::from_exe(&exe_data).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");
    let archetypes = index_archetypes(&manager, &keys);

    // Every `.ytd`'s stem hash to its path, the lookup the archetype's
    // `textureDictionary` hash needs.
    let mut ytd_by_hash: HashMap<u32, String> = HashMap::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if let Some(stem) = entry.name_lower.strip_suffix(".ytd") {
                ytd_by_hash
                    .entry(gen_hash(stem))
                    .or_insert_with(|| entry.path.clone());
            }
        }
    }

    // A spread of real model files, as the streamer would pull them.
    let models: Vec<&String> = manager
        .archives
        .iter()
        .flat_map(|a| a.entries.iter())
        .filter(|e| e.name_lower.ends_with(".ydr"))
        .map(|e| &e.path)
        .step_by(97)
        .take(SAMPLE)
        .collect();

    let ytds = manager.ytds();
    let mut folder_scanned = 0usize;
    let mut archetype_hits = 0usize;
    let mut sibling_hits = 0usize;
    let mut neither = 0usize;

    for path in &models {
        let (dir, name) = path.rsplit_once('\\').unwrap_or(("", path));
        let stem = name.strip_suffix(".ydr").unwrap_or(name);

        // What the folder scan costs today: every `.ytd` beside the model.
        folder_scanned += ytds.get(dir).map(|v| v.len()).unwrap_or(0);

        // What the archetype would have said instead.
        let archetype = archetypes.all.get(&gen_hash(stem));
        let named = archetype
            .filter(|a| a.texture_dictionary != 0)
            .and_then(|a| ytd_by_hash.get(&a.texture_dictionary));
        if named.is_some() {
            archetype_hits += 1;
        } else if ytds
            .get(dir)
            .is_some_and(|v| v.iter().any(|p| p.ends_with(&format!("\\{stem}.ytd"))))
        {
            sibling_hits += 1;
        } else {
            neither += 1;
        }
    }

    // What one folder scan actually costs in wall time, for the worst model in
    // the sample: extract plus inflate plus parse, per dictionary.
    let worst = models
        .iter()
        .max_by_key(|p| {
            let (dir, _) = p.rsplit_once('\\').unwrap_or(("", p));
            ytds.get(dir).map(|v| v.len()).unwrap_or(0)
        })
        .expect("a model");
    let (worst_dir, _) = worst.rsplit_once('\\').unwrap_or(("", worst));
    let worst_ytds = ytds.get(worst_dir).cloned().unwrap_or_default();

    let started = Instant::now();
    let mut bytes = 0usize;
    for ytd in &worst_ytds {
        let Some(r) = manager.find(ytd) else { continue };
        let Some((size, _)) = manager.entry(r).resource_segments() else {
            continue;
        };
        let Ok(data) = manager.extract(r, &keys) else {
            continue;
        };
        bytes += data.len();
        let _ = cw_core::resources::read_textures(cw_core::filetype::FileKind::Texture, &data, size);
    }
    let worst_time = started.elapsed();

    println!("{} models sampled\n", models.len());
    println!(
        "folder scan visits      : {folder_scanned} dictionaries ({:.1} per model)",
        folder_scanned as f64 / models.len() as f64
    );
    println!("archetype names the ytd : {archetype_hits} models");
    println!("only a sibling ytd      : {sibling_hits} models");
    println!("neither                 : {neither} models");
    println!(
        "\nworst folder in the sample: {} dictionaries, {:.2} MB, {worst_time:.2?} to extract and parse",
        worst_ytds.len(),
        bytes as f64 / (1024.0 * 1024.0)
    );
    println!("  {worst}");
}
