//! Explorer Mode's IPC surface: open a game folder, then list it.
//!
//! The rule from `docs/phases.md`: heavy work happens in Rust threads, and raw
//! archive bytes never cross the IPC boundary for the frontend to parse. So
//! these commands hand back listings — names, kinds and sizes — and nothing
//! else. Extraction and per-type viewers come later in phase 2.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
}

#[derive(Default)]
pub struct GameState(pub Mutex<Option<Loaded>>);

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
    folder: String,
    state: State<'_, GameState>,
) -> Result<GameSummary, String> {
    let path = PathBuf::from(&folder);
    let (loaded, summary) = tauri::async_runtime::spawn_blocking(move || scan(&path))
        .await
        .map_err(|e| format!("the scan task failed: {e}"))??;

    *state.0.lock().map_err(|_| "game state is poisoned")? = Some(loaded);
    Ok(summary)
}

fn scan(folder: &Path) -> Result<(Loaded, GameSummary), String> {
    let started = std::time::Instant::now();

    let exe = folder.join("GTA5.exe");
    let exe_data = std::fs::read(&exe).map_err(|e| {
        format!(
            "cannot read {}: {e}. Pick the folder holding GTA5.exe.",
            exe.display()
        )
    })?;
    let keys = GtaKeys::from_exe(&exe_data)
        .map_err(|e| format!("could not recover the archive keys from GTA5.exe: {e}"))?;

    let manager = RpfManager::scan(folder, &keys).map_err(|e| format!("scan failed: {e}"))?;

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
        },
        summary,
    ))
}

/// List a directory. An empty or absent `path` lists the top-level archives.
#[tauri::command]
pub fn list_dir(path: Option<String>, state: State<'_, GameState>) -> Result<Vec<Listing>, String> {
    let guard = state.0.lock().map_err(|_| "game state is poisoned")?;
    let loaded = guard.as_ref().ok_or("no game folder is open")?;
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
        .lock()
        .map_err(|_| "game state is poisoned")?
        .is_some())
}
