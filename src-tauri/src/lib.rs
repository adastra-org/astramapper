//! AstraMapper Tauri backend.
//!
//! This crate is the IPC surface only: it owns app state and window management
//! and delegates every parse to `cw-core`. Heavy work stays on the Rust side —
//! the WebView receives computed results (tree listings, decoded textures,
//! selection state), never raw archive bytes to parse itself.

// Public so `tests/explorer_gate.rs` can drive the listing logic against a real
// scan without a Tauri runtime.
pub mod bake;
pub mod cache;
pub mod cells;
pub mod clusters;
pub mod debug_bridge;
pub mod explorer;
pub mod ground;
pub mod perf;
pub mod placements;
pub mod textures;
pub mod viewer;
pub mod world;
pub mod world_index;

use serde::Serialize;

/// Build information for the about panel, and a smoke test that `cw-core` is
/// actually linked in and reachable across the IPC boundary.
#[derive(Serialize)]
struct AppInfo {
    name: &'static str,
    version: &'static str,
    /// Jenkins hash of "astramapper", computed by the ported `cw-core` routine.
    core_check: u32,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: "AstraMapper",
        version: env!("CARGO_PKG_VERSION"),
        core_check: cw_core::utils::gen_hash("astramapper"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(explorer::GameState::default())
        .manage(viewer::ViewerState::default())
        .manage(world::GpuState::default())
        .manage(world_index::WorldIndexState::default())
        // An `Arc` because `open_game_folder` cancels the running bake, and the
        // pass itself holds the same flag from a blocking thread.
        .manage(std::sync::Arc::new(bake::BakeState::default()))
        .plugin(tauri_plugin_opener::init())
        // Picking the GTA V install folder.
        .plugin(tauri_plugin_dialog::init())
        // Extracting files out of archives to disk.
        .plugin(tauri_plugin_fs::init())
        // Persisting settings (game folder, DLC level, window layout).
        .plugin(tauri_plugin_store::Builder::new().build())
        // The debug bridge listens only in debug builds, or when asked for with
        // `ASTRAMAPPER_DEBUG_BRIDGE=1`; see `debug_bridge`.
        .setup(|_app| {
            debug_bridge::start();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            debug_bridge::debug_bridge_info,
            debug_bridge::debug_bridge_sync,
            explorer::open_game_folder,
            explorer::list_dir,
            explorer::game_is_open,
            explorer::release_archives,
            cache::cache_stats,
            cache::cache_clear,
            perf::perf_report,
            perf::perf_reset,
            bake::bake_start,
            bake::bake_cancel,
            bake::bake_running,
            world::world_status,
            world::system_memory,
            world::hardware_info,
            world::world_dlc_ymaps,
            world::world_maps_in_group,
            world::world_map,
            world::world_map_file,
            world::world_asset,
            world::world_model,
            world_index::world_index_build,
            world_index::world_index_cells,
            world_index::world_index_cluster_measure,
            world::set_gpu_capabilities,
            world::project_save,
            world::project_load,
            viewer::open_entry,
            viewer::read_text,
            viewer::read_gxt2,
            viewer::read_hex,
            viewer::read_model,
            viewer::read_bounds,
            viewer::read_clips,
            viewer::read_meta,
            viewer::read_map,
            viewer::read_texture,
            viewer::read_texture_image,
            viewer::save_texture,
            viewer::read_audio,
            viewer::read_audio_track,
            viewer::save_audio_track,
            viewer::extract_to,
            viewer::save_text,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
