//! The disk cache against a real install: does a warm read give back exactly
//! what the compiler produced, and is it worth having?
//!
//! ```text
//! cargo test -p astramapper --test cache_gate -- --ignored --nocapture
//! ```
//!
//! **Byte identity is the assertion that matters.** A cache that serves
//! *nearly* the right bytes is worse than no cache: the failure is silent,
//! shows up as wrong geometry or a scrambled texture, and looks like a parser
//! bug. Everything else here is a measurement.
//!
//! Invalidation is not gated here, deliberately. Proving that a changed archive
//! moves its entries' keys means writing to the game folder, and the property
//! is already pinned without doing that: `stamp_file` moving with a file's
//! contents is a `cw-core` unit test, the stamp changing the key is a
//! `cw-cache` one, and a nested archive inheriting its ancestor's stamp is a
//! third.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use astramapper_lib::cache::{install_stamp, model_key};
use astramapper_lib::textures::GpuCaps;
use astramapper_lib::world::{compile_world_model, find_model_for, read_world_map};
use cw_cache::{Cache, Kind, MIN_BUDGET};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use cw_core::wire::EnvelopeView;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

/// What a GPU that takes every block format reports, so the gate exercises the
/// same encoder the app does rather than the RGBA fallback.
const CAPS: GpuCaps = GpuCaps {
    s3tc: true,
    rgtc: true,
    bptc: true,
};

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

/// A scratch cache directory that removes itself. Never the app's real one: a
/// gate must not evict the user's baked artifacts to make room for its own.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("astramapper-cache-gate-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn open(&self) -> Cache {
        Cache::open(&self.0, install_stamp(&gta_path()), MIN_BUDGET).expect("open the cache")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Distinct model files placed by city `.ymap`s, which is the population the
/// world view actually streams.
fn placed_models(manager: &RpfManager, keys: &GtaKeys, limit: usize) -> Vec<String> {
    let names: HashMap<u32, String> = manager.index_names();
    let maps: Vec<&String> = manager
        .archives
        .iter()
        .flat_map(|a| a.entries.iter())
        .map(|e| &e.path)
        .filter(|p| {
            p.ends_with(".ymap")
                && p.contains(r"\levels\gta5\_city")
                && !p.contains(r"\.rpf\interiors\")
        })
        .collect();

    let mut models: Vec<String> = Vec::new();
    for path in maps {
        let Ok(map) = read_world_map(path, keys, manager, None, Some(&names)) else {
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

/// The gate. Compile cold through the cache, read back warm, and require the
/// bytes to be identical to what the compiler produced — across a reopen, so
/// the mapped read and the journal replay are both in the path.
///
/// The cold figure it prints is *not* comparable to the 2.90s in the phase 4
/// notes: this passes no archetype index, so texture resolution falls back to
/// scanning each model's folder. The ratio is the point.
#[test]
#[ignore = "needs a local GTA V install"]
fn a_warm_read_returns_exactly_what_was_compiled() {
    let (manager, keys) = scan();
    let models = placed_models(&manager, &keys, 40);
    let scratch = Scratch::new("identity");

    let mut compiled: Vec<(String, Vec<u8>)> = Vec::new();
    let cold_started = Instant::now();
    {
        let cache = scratch.open();
        for path in &models {
            let key = model_key(&manager, path, CAPS);
            assert!(key.is_some(), "{path} has no cache key");
            let bytes = cache
                .get_or_insert_with(key, Kind::Model, || {
                    compile_world_model(path, &keys, &manager, None, CAPS)
                })
                .unwrap_or_else(|e| panic!("compiling {path}: {e}"));
            compiled.push((path.clone(), bytes));
        }
        let stats = cache.stats();
        assert_eq!(stats.hits, 0, "nothing can hit on an empty cache");
        assert_eq!(stats.misses, models.len() as u64);
        assert_eq!(stats.write_errors, 0);
    }
    let cold = cold_started.elapsed();

    // A fresh handle: the table is read off disk and the packs are remapped,
    // which is the path a second launch takes.
    let warm_started = Instant::now();
    let cache = scratch.open();
    let mut warm_bytes = 0u64;
    for (path, expected) in &compiled {
        let key = model_key(&manager, path, CAPS).expect("key");
        let got = cache
            .get(key)
            .unwrap_or_else(|| panic!("{path} should be cached"));
        assert_eq!(&got, expected, "{path} came back different");
        // Whatever else is true of these bytes, they have to still be an
        // envelope the frontend can read.
        EnvelopeView::read(&got).unwrap_or_else(|e| panic!("{path} is not an envelope: {e}"));
        warm_bytes += got.len() as u64;
    }
    let warm = warm_started.elapsed();

    let stats = cache.stats();
    assert_eq!(stats.hits, models.len() as u64);
    assert_eq!(stats.misses, 0, "a warm read must not miss");
    assert_eq!(stats.records, models.len());

    println!(
        "{} models: cold {:.2}s, warm {:.0}ms ({:.1}x), {:.1} MB stored across {} pack(s)",
        models.len(),
        cold.as_secs_f64(),
        warm.as_secs_f64() * 1000.0,
        cold.as_secs_f64() / warm.as_secs_f64().max(f64::EPSILON),
        warm_bytes as f64 / 1_048_576.0,
        stats.packs,
    );
}

/// A second pass over the same models must not compile anything, which is the
/// whole claim. Separately gated from byte identity because a cache can be
/// byte-correct and still miss on every lookup.
#[test]
#[ignore = "needs a local GTA V install"]
fn the_second_pass_compiles_nothing() {
    let (manager, keys) = scan();
    let models = placed_models(&manager, &keys, 20);
    let scratch = Scratch::new("hit-rate");
    let cache = scratch.open();

    let mut compiles = 0;
    for _ in 0..2 {
        for path in &models {
            let key = model_key(&manager, path, CAPS);
            cache
                .get_or_insert_with(key, Kind::Model, || {
                    compiles += 1;
                    compile_world_model(path, &keys, &manager, None, CAPS)
                })
                .unwrap_or_else(|e| panic!("compiling {path}: {e}"));
        }
    }

    assert_eq!(compiles, models.len(), "the second pass must be all hits");
    assert_eq!(cache.stats().hits, models.len() as u64);
}

/// The same model compiled for a different GPU is a different artifact. Two
/// machines sharing a profile directory, or one machine whose driver stops
/// reporting BPTC, must not read each other's blocks.
#[test]
#[ignore = "needs a local GTA V install"]
fn gpu_capabilities_are_part_of_the_key() {
    let (manager, keys) = scan();
    let path = &placed_models(&manager, &keys, 1)[0];

    let full = model_key(&manager, path, CAPS).expect("key");
    let none = model_key(&manager, path, GpuCaps::default()).expect("key");
    assert_ne!(full, none, "a different GPU must not share artifacts");

    let scratch = Scratch::new("caps");
    let cache = scratch.open();
    let blocks = cache
        .get_or_insert_with(Some(full), Kind::Model, || {
            compile_world_model(path, &keys, &manager, None, CAPS)
        })
        .expect("compile");
    let rgba = cache
        .get_or_insert_with(Some(none), Kind::Model, || {
            compile_world_model(path, &keys, &manager, None, GpuCaps::default())
        })
        .expect("compile");

    assert_ne!(blocks, rgba, "the two encodings must differ");
    assert_eq!(cache.get(full).as_deref(), Some(blocks.as_slice()));
    assert_eq!(cache.get(none).as_deref(), Some(rgba.as_slice()));
}
