//! Where the compiled-asset cache is opened, keyed, and reported on.
//!
//! `cw-cache` depends on neither `tauri` nor `cw-core`: it stores opaque bytes
//! under an opaque key. This module is the seam that gives those two things
//! meaning — it resolves the cache directory from the app, stamps the install,
//! and turns an archive entry into a [`CacheKey`].
//!
//! **The cache is owned by [`crate::explorer::Loaded`]**, not held in its own
//! Tauri state. Its contents are scoped to one install, so tying its lifetime to
//! the install removes the state where an open cache and an open game folder
//! disagree about which folder that is.

use std::path::{Path, PathBuf};

use cw_cache::{Cache, CacheKey, KeyInputs, Kind, DEFAULT_BUDGET};
use cw_core::rpf::{stamp_file, RpfManager};
use serde::Serialize;
use tauri::{Manager, State};

use crate::explorer::GameState;
use crate::textures::GpuCaps;

/// Where the packs live: under the app's own cache directory, and never beside
/// the game folder or inside the repository.
///
/// `None` when the platform will not name one, which costs the cache and
/// nothing else.
pub fn assets_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|dir| dir.join("assets"))
}

/// What identifies the install these artifacts were compiled from.
///
/// The executable, because a game update rewrites it and a game update is
/// exactly the moment every compiled artifact should be considered suspect. Per
/// *archive* invalidation is finer and is already handled by the key; this is
/// the coarse gate on the folder as a whole.
pub fn install_stamp(folder: &Path) -> u64 {
    stamp_file(&folder.join("GTA5.exe"))
}

/// Open the cache for `folder`, or give up quietly.
///
/// Returns `None` rather than an error: a cache that cannot be opened must
/// leave the app working, only slower.
pub fn open(dir: Option<PathBuf>, folder: &Path) -> Option<Cache> {
    let dir = dir?;
    Cache::open(&dir, install_stamp(folder), DEFAULT_BUDGET).ok()
}

/// The key for one compiled model.
///
/// `caps` is in the key because the same texture compiles to different bytes on
/// a machine without BPTC: two GPUs must not read each other's artifacts.
pub fn model_key(manager: &RpfManager, path: &str, caps: GpuCaps) -> Option<CacheKey> {
    entry_key(manager, path, Kind::Model, caps.bits() as u64)
}

/// The key for one flattened `.ymap`.
///
/// `typ_generation` is in the key because **a flattened map is not decided by
/// the `.ymap` alone.** Its MLO instances expand through archetypes that live in
/// `.ytyp` files in other archives, and an expansion *adds entities to the
/// list* — so a stale one does not merely draw something wrong, it changes what
/// exists in the world. The entry key covers the map's own archive; this covers
/// the other input. See [`crate::explorer::Loaded::typ_generation`].
pub fn map_key(manager: &RpfManager, path: &str, typ_generation: u64) -> Option<CacheKey> {
    entry_key(manager, path, Kind::Map, typ_generation)
}

/// The key for any artifact compiled out of one archive entry.
fn entry_key(manager: &RpfManager, path: &str, kind: Kind, params: u64) -> Option<CacheKey> {
    let reference = manager.find(path)?;
    let entry = manager.entry(reference);
    CacheKey::of(KeyInputs {
        kind,
        archive_stamp: manager.stamp_of(reference),
        // `h1`/`h2` carry the entry's offset and size inside its archive.
        entry_header: (entry.h1, entry.h2),
        // The manager's own index key, so it is unique across the whole install
        // and not just within one archive.
        entry_path: &entry.path,
        params,
    })
}

/// [`cw_cache::CacheStats`] in the shape the WebView reads.
///
/// A mirror rather than a serde attribute on the original, because camelCase is
/// this app's IPC convention and `cw-cache` is not supposed to know an app
/// exists.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireCacheStats {
    pub bytes: u64,
    pub budget_bytes: u64,
    pub records: usize,
    pub packs: usize,
    pub hits: u64,
    pub misses: u64,
    pub write_errors: u64,
}

impl From<cw_cache::CacheStats> for WireCacheStats {
    fn from(stats: cw_cache::CacheStats) -> Self {
        Self {
            bytes: stats.bytes,
            budget_bytes: stats.budget_bytes,
            records: stats.records,
            packs: stats.packs,
            hits: stats.hits,
            misses: stats.misses,
            write_errors: stats.write_errors,
        }
    }
}

/// What the cache holds right now, for the settings panel.
///
/// `None` when no folder is open or the cache could not be opened, which the UI
/// shows as "off" rather than as a failure.
#[tauri::command]
pub fn cache_stats(game: State<'_, GameState>) -> Result<Option<WireCacheStats>, String> {
    Ok(game
        .try_loaded()
        .and_then(|loaded| loaded.cache.as_ref().map(|cache| cache.stats().into())))
}

/// Throw the cache away. The visible escape hatch for a cache gone wrong.
#[tauri::command]
pub fn cache_clear(game: State<'_, GameState>) -> Result<(), String> {
    let loaded = game.loaded()?;
    match &loaded.cache {
        Some(cache) => cache.clear().map_err(|e| format!("could not clear the cache: {e}")),
        None => Ok(()),
    }
}
