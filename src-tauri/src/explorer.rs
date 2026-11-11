//! Explorer Mode's IPC surface: open a game folder, then list it.
//!
//! The rule from `docs/phases.md`: heavy work happens in Rust threads, and raw
//! archive bytes never cross the IPC boundary for the frontend to parse. So
//! these commands hand back listings — names, kinds and sizes — and nothing
//! else. Extraction and per-type viewers come later in phase 2.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use cw_core::filetypes::{index_archetypes, ArchetypeIndexes, MloArchetype};
use cw_core::rpf::{EntryKind, RpfManager};
use cw_core::utils::GtaKeys;
use serde::Serialize;
use tauri::State;

/// A scanned game folder. Held for the life of the app once opened.
pub struct Loaded {
    /// The folder that was scanned, so other modes can say what is mounted.
    pub folder: String,
    /// Held rather than recovered on demand: the keys come from a SHA-1 scan
    /// over the whole of `GTA5.exe`, which is far too slow to repeat per
    /// extraction.
    pub keys: GtaKeys,
    pub manager: RpfManager,
    /// The compiled-asset cache for this install, or `None` when it could not be
    /// opened. Owned here because its contents are scoped to this folder: when
    /// the install goes, so does the handle to its cache. See
    /// [`crate::cache`].
    pub cache: Option<cw_cache::Cache>,
    /// The archetype indexes, built once when the first map is placed. Reading
    /// every `.ytyp` is expensive, so they are kept, not rebuilt per map, and
    /// both halves come out of the same pass.
    archetypes: OnceLock<ArchetypeIndexes>,
    /// Every entry basename keyed by its hash, built once — the table that
    /// turns a `.ymap` archetype hash back into a model name. See
    /// [`RpfManager::index_names`].
    names: OnceLock<HashMap<u32, String>>,
    /// One value standing for the state of every archive that holds a `.ytyp`.
    /// See [`Self::typ_generation`].
    typ_generation: OnceLock<u64>,
}

impl Loaded {
    /// The archetype indexes, built on first use.
    ///
    /// A `OnceLock` rather than a `Mutex<Option<_>>`: the caller gets a plain
    /// reference and has no guard to hold, so a long map read cannot serialize
    /// every other one behind it. Concurrent first calls block until the one
    /// that won finishes, so the `.ytyp` scan still happens exactly once.
    pub fn archetypes(&self) -> &ArchetypeIndexes {
        self.archetypes.get_or_init(|| {
            // Timed because it is lazy: it does not show up in the open, it
            // shows up as the first placed map being slower than the rest, and
            // that is a confusing thing to look at without a name on it.
            crate::perf::span("index.archetypes", || {
                index_archetypes(&self.manager, &self.keys)
            })
        })
    }

    /// The interiors half of [`Self::archetypes`].
    pub fn mlo_archetypes(&self) -> &HashMap<u32, MloArchetype> {
        &self.archetypes().mlo
    }

    /// The entry-basename index, built on first use. Same reasoning as
    /// [`Self::archetypes`].
    pub fn names(&self) -> &HashMap<u32, String> {
        self.names
            .get_or_init(|| crate::perf::span("index.names", || self.manager.index_names()))
    }

    /// One value standing for the state of every archive that holds a `.ytyp`.
    ///
    /// The cache keys an artifact by the archive its *source entry* came from,
    /// which is right for a model and not enough for a map: flattening a
    /// `.ymap` expands its MLO instances through archetypes read out of `.ytyp`
    /// files in other archives entirely, and an expansion adds entities to the
    /// placement list. Patch the archive holding the interiors and a map keyed
    /// on its own archive alone would keep serving the old contents — a
    /// difference in *what is in the world*, not in how it looks.
    ///
    /// So the stamps of every archive that holds one are mixed into a single
    /// value that rides in the map key. Patch any of them and every cached map
    /// misses, which is the conservative direction: the cost of being wrong
    /// here is a silently wrong world, and the cost of being over-eager is one
    /// re-flatten at 0.44ms.
    ///
    /// Built once. The stamps themselves are already memoized by the manager,
    /// so this is a scan of the entry names and nothing more.
    ///
    /// Known limit, shared with the model key: an archive holding a `.ytd` a
    /// model draws is not in *that* key either. The install stamp on
    /// `GTA5.exe` is the coarse gate over both, so an official update wipes
    /// everything; a mod tool that rewrites one archive and nothing else is the
    /// gap. Closing it for models is follow-up work.
    pub fn typ_generation(&self) -> u64 {
        *self.typ_generation.get_or_init(|| {
            let stamps = self.manager.archive_stamps();
            let mut mixed: u64 = 0xcbf2_9ce4_8422_2325;
            for (index, archive) in self.manager.archives.iter().enumerate() {
                if !archive
                    .entries
                    .iter()
                    .any(|e| e.name_lower.ends_with(".ytyp"))
                {
                    continue;
                }
                let stamp = stamps.get(index).copied().unwrap_or(0);
                // FNV-style mix, order-dependent on purpose: the archive order
                // is the manager's scan order and is stable for one install.
                mixed = (mixed ^ stamp).wrapping_mul(0x0000_0100_0000_01b3);
            }
            mixed
        })
    }
}

/// A `RwLock` rather than a `Mutex`: the world streams many maps and models at
/// once, and every read of the scanned install must not queue behind the
/// others. Only opening a new folder takes the write side.
///
/// The install is behind an `Arc` so a command can clone it, drop the guard,
/// and carry it into `spawn_blocking`. A `RwLockReadGuard` is neither `Send`
/// nor `'static` and cannot cross an `.await`; an `Arc<Loaded>` is both.
#[derive(Default)]
pub struct GameState(pub RwLock<Option<Arc<Loaded>>>);

impl GameState {
    /// The open install, or an error naming why there is none.
    ///
    /// Clones the `Arc` and releases the read lock before returning, which is
    /// what keeps the "no guard crosses an await" rule mechanical rather than
    /// a thing each command has to remember.
    pub fn loaded(&self) -> Result<Arc<Loaded>, String> {
        let guard = self.0.read().map_err(|_| "game state is poisoned")?;
        guard.clone().ok_or_else(|| "no game folder is open".into())
    }

    /// The open install, or `None` when no folder is open.
    pub fn try_loaded(&self) -> Option<Arc<Loaded>> {
        self.0.read().ok().and_then(|g| g.clone())
    }
}

/// What the frontend learns about a freshly opened folder.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSummary {
    pub folder: String,
    /// Archives that are not nested inside another.
    pub top_level_archives: usize,
    /// Every archive, nested ones included.
    pub total_archives: usize,
    pub total_entries: usize,
    /// Archives that failed to scan. One bad archive does not fail the folder.
    pub failed: Vec<String>,
    pub elapsed_ms: u64,
}

/// One row in a directory listing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub name: String,
    /// Full path, and the handle for listing or extracting this entry.
    pub path: String,
    /// `directory`, `archive`, `binary` or `resource`.
    pub kind: &'static str,
    /// Size on disk in bytes. Zero for directories.
    pub size: u64,
    /// Whether listing this entry yields anything — true for directories and
    /// for `.rpf` files, which open into an archive of their own.
    pub expandable: bool,
}

/// Recover the keys from `GTA5.exe` and scan every archive under `folder`.
///
/// Both steps are slow enough to freeze the WebView — the key scan hashes the
/// whole executable — so this runs on the blocking pool rather than the main
/// thread.
#[tauri::command]
pub async fn open_game_folder(
    app: tauri::AppHandle,
    folder: String,
    state: State<'_, GameState>,
    bake: State<'_, Arc<crate::bake::BakeState>>,
) -> Result<GameSummary, String> {
    // Stop the bake before scanning, not after: it is about to be working
    // against an install nobody can see any more. It is not joined, and does not
    // need to be — it holds an `Arc<Loaded>`, so the install and the cache it
    // writes to stay alive until it winds down on its own.
    bake.cancel();
    let path = PathBuf::from(&folder);
    // Resolved here rather than inside the scan, so `scan` stays free of Tauri
    // and the gate can drive it.
    let assets = crate::cache::assets_dir(&app);
    let (loaded, summary) = tauri::async_runtime::spawn_blocking(move || scan(&path, assets))
        .await
        .map_err(|e| format!("the scan task failed: {e}"))??;

    *state.0.write().map_err(|_| "game state is poisoned")? = Some(Arc::new(loaded));
    Ok(summary)
}

fn scan(folder: &Path, assets: Option<PathBuf>) -> Result<(Loaded, GameSummary), String> {
    let started = std::time::Instant::now();

    // Broken into phases rather than timed as one number: the whole open is
    // around half a second and which half-second it is decides whether there is
    // anything to do about it. See `docs/phases.md` 4.10.
    let exe = folder.join("GTA5.exe");
    let timer = crate::perf::Timer::start("open.read_exe");
    let exe_data = std::fs::read(&exe).map_err(|e| {
        format!(
            "cannot read {}: {e}. Pick the folder holding GTA5.exe.",
            exe.display()
        )
    })?;
    timer.stop_with_bytes(exe_data.len() as u64);

    let keys = crate::perf::span("open.recover_keys", || GtaKeys::from_exe(&exe_data))
        .map_err(|e| format!("could not recover the archive keys from GTA5.exe: {e}"))?;

    let manager = crate::perf::span("open.scan_archives", || RpfManager::scan(folder, &keys))
        .map_err(|e| format!("scan failed: {e}"))?;

    let top = manager.top_level().count();
    let summary = GameSummary {
        folder: folder.display().to_string(),
        top_level_archives: top,
        total_archives: manager.archives.len(),
        total_entries: manager.entry_count(),
        failed: manager
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.path.display(), e.message))
            .collect(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    };

    Ok((
        Loaded {
            folder: folder.display().to_string(),
            keys,
            manager,
            cache: crate::cache::open(assets, folder),
            archetypes: OnceLock::new(),
            names: OnceLock::new(),
            typ_generation: OnceLock::new(),
        },
        summary,
    ))
}

/// List a directory. An empty or absent `path` lists the top-level archives.
#[tauri::command]
pub fn list_dir(path: Option<String>, state: State<'_, GameState>) -> Result<Vec<Listing>, String> {
    let loaded = state.loaded()?;
    listing_for(&loaded.manager, &path.unwrap_or_default())
}

/// The listing itself, free of Tauri state so it can be tested against a real
/// scan. See `tests/explorer_gate.rs`.
pub fn listing_for(manager: &RpfManager, path: &str) -> Result<Vec<Listing>, String> {
    if path.is_empty() {
        let mut rows: Vec<Listing> = manager
            .top_level()
            .map(|a| Listing {
                name: a.name.clone(),
                path: a.path.clone(),
                kind: "archive",
                size: a.file_size,
                expandable: true,
            })
            .collect();
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        return Ok(rows);
    }

    let target = manager
        .find(path)
        .ok_or_else(|| format!("{path} is not in the index"))?;

    let mut rows: Vec<Listing> = manager
        .list_dir(target)
        .into_iter()
        .map(|r| {
            let entry = manager.entry(r);
            let is_archive = entry.name_lower.ends_with(".rpf");
            let kind = match &entry.kind {
                EntryKind::Directory(_) => "directory",
                EntryKind::Binary(_) if is_archive => "archive",
                EntryKind::Binary(_) => "binary",
                EntryKind::Resource(_) => "resource",
            };
            Listing {
                name: entry.name.clone(),
                path: entry.path.clone(),
                kind,
                size: entry.file_size_on_disk(),
                expandable: entry.is_directory() || is_archive,
            }
        })
        .collect();

    // Directories first, then by name, which is what a file browser should do
    // and what the entry table order does not give.
    rows.sort_by(|a, b| {
        let group = |k: &str| if k == "directory" { 0 } else { 1 };
        group(a.kind)
            .cmp(&group(b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(rows)
}

/// Whether a folder is already open, so the UI can restore its state.
#[tauri::command]
pub fn game_is_open(state: State<'_, GameState>) -> Result<bool, String> {
    Ok(state
        .0
        .read()
        .map_err(|_| "game state is poisoned")?
        .is_some())
}

/// Let go of every mapped archive.
///
/// Extraction reads through a mapping of the `.rpf` on disk, and Windows holds
/// a file lock for a mapping's lifetime. A user who wants to patch the game,
/// or run OpenIV, while AstraMapper is open needs a way to release them. The
/// index survives; the next extraction maps what it needs again.
#[tauri::command]
pub fn release_archives(state: State<'_, GameState>) -> Result<usize, String> {
    let Some(loaded) = state.try_loaded() else {
        return Ok(0);
    };
    let released = loaded.manager.mapped_archives();
    loaded.manager.release_archives();
    Ok(released)
}
