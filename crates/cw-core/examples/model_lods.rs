//! Prints, for model files whose name contains an argument, the drawable's own
//! LOD distances, what each LOD bundle holds (parts, triangles) and the shader
//! of each geometry with its render bucket. Evidence for the LOD band choice and
//! the alpha test of the World view.
//!
//! ```text
//! cargo run --release -p cw-core --example model_lods -- prop_pylon_01 prop_tree_
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::mesh::MeshBundle;
use cw_core::resources::{read_named_drawables, read_named_fragments};
use cw_core::rpf::RpfManager;
use cw_core::utils::{jenk::gen_hash, GtaKeys};

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

/// Shader names worth resolving by hash.
const SHADERS: &[&str] = &[
    "default",
    "spec",
    "normal",
    "normal_spec",
    "normal_spec_alpha",
    "normal_alpha",
    "alpha",
    "cutout",
    "cutout_fence",
    "cutout_hard",
    "normal_cutout",
    "normal_spec_cutout",
    "trees",
    "trees_lod",
    "trees_lod2",
    "trees_normal",
    "trees_normal_spec",
    "trees_normal_diffspec",
    "trees_normal_spec_tnt",
    "trees_normal_diffspec_tnt",
    "trees_tnt",
    "trees_shadow_proxy",
    "normal_spec_decal",
    "decal",
    "grass",
    "grass_batch",
    "normal_spec_wind",
    "cable",
    "emissive",
    "normal_spec_reflect",
    "spec_reflect",
    "terrain_cb_w_4lyr",
    "normal_um",
    "normal_spec_um",
];

fn main() {
    let needles: Vec<String> = std::env::args().skip(1).map(|a| a.to_lowercase()).collect();
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let exe = std::fs::read(folder.join("GTA5.exe")).expect("GTA5.exe");
    let keys = GtaKeys::from_exe(&exe).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");
    let shader_names: HashMap<u32, &str> = SHADERS.iter().map(|n| (gen_hash(n), *n)).collect();

    for archive in &manager.archives {
        for entry in &archive.entries {
            let kind = FileKind::from_name(&entry.name_lower);
            if !matches!(kind, FileKind::Model | FileKind::Fragment) {
                continue;
            }
            if !needles
                .iter()
                .any(|n| entry.name_lower.contains(n.as_str()))
            {
                continue;
            }
            let Some(r) = manager.find(&entry.path) else {
                continue;
            };
            let Some((size, _)) = entry.resource_segments() else {
                continue;
            };
            let Ok(data) = manager.extract(r, &keys) else {
                continue;
            };
            let named = match kind {
                FileKind::Fragment => read_named_fragments(kind, &data, size),
                _ => read_named_drawables(kind, &data, size),
            };
            let Ok(named) = named else { continue };
            for nd in &named {
                let d = &nd.drawable;
                println!(
                    "{} [{}] lodDistances={:?} radius={:.1}",
                    entry.name_lower, nd.name, d.lod_distances, d.bounding_sphere_radius
                );
                for bundle in MeshBundle::all(d) {
                    let shaders: Vec<String> = bundle
                        .parts
                        .iter()
                        .map(|p| {
                            let bucket = d
                                .shaders
                                .iter()
                                .find(|s| s.name == p.shader)
                                .map(|s| s.render_bucket)
                                .unwrap_or(255);
                            let file = d
                                .shaders
                                .iter()
                                .find(|s| s.name == p.shader)
                                .map(|s| s.file_name)
                                .unwrap_or(0);
                            let name = shader_names
                                .get(&p.shader)
                                .map(|s| s.to_string())
                                .unwrap_or(format!("{:#010x}", p.shader));
                            format!("{name}/b{bucket}/f{file}")
                        })
                        .collect();
                    println!(
                        "   {:?}: {} parts, {} tris, shaders {}",
                        bundle.lod,
                        bundle.parts.len(),
                        bundle.triangle_count,
                        shaders.join(" ")
                    );
                }
            }
        }
    }
}
