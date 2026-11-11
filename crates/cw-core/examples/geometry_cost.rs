//! What shipping a model's geometry costs as base64 against raw sections.
//!
//! The texture half of the wire envelope is measured and landed. This asks
//! whether the geometry half earns its own change, and whether narrowing
//! indices to `u16` is worth recording on the section.
//!
//! ```text
//! cargo run --release --example geometry_cost
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cw_core::filetype::FileKind;
use cw_core::mesh::MeshBundle;
use cw_core::resources::{read_named_drawables, read_named_fragments};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

/// How many model files to walk. Enough to be representative, not so many the
/// run is dominated by archive extraction.
const SAMPLE: usize = 400;

fn main() {
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let Ok(exe) = std::fs::read(folder.join("GTA5.exe")) else {
        eprintln!("no install at {}", folder.display());
        return;
    };
    let keys = GtaKeys::from_exe(&exe).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");

    let mut base64_time = Duration::ZERO;
    let mut base64_bytes = 0usize;
    let mut raw_bytes = 0usize;
    let mut index_u32 = 0usize;
    let mut index_u16 = 0usize;
    let mut parts = 0usize;
    let mut done = 0usize;
    let mut narrowable = 0usize;

    'outer: for archive in &manager.archives {
        for entry in &archive.entries {
            let kind = FileKind::from_name(&entry.name_lower);
            if !matches!(kind, FileKind::Model | FileKind::Fragment) {
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

            for drawable in &named {
                for bundle in MeshBundle::all(&drawable.drawable) {
                    for part in &bundle.parts {
                        // What ships today: every buffer base64'd into a JSON
                        // string, which the frontend then decodes into a fresh
                        // typed array.
                        let started = Instant::now();
                        let mut encoded = 0usize;
                        for buffer in [
                            Some(part.positions.as_bytes()),
                            part.normals.as_ref().map(|b| b.as_bytes()),
                            part.uvs.as_ref().map(|b| b.as_bytes()),
                            Some(part.indices.as_bytes()),
                        ]
                        .into_iter()
                        .flatten()
                        {
                            encoded += STANDARD.encode(buffer).len();
                            raw_bytes += buffer.len();
                        }
                        base64_time += started.elapsed();
                        base64_bytes += encoded;

                        let indices = part.indices.as_bytes().len();
                        index_u32 += indices;
                        if part.vertex_count <= u16::MAX as usize {
                            index_u16 += indices / 2;
                            narrowable += 1;
                        } else {
                            index_u16 += indices;
                        }
                        parts += 1;
                    }
                }
            }

            done += 1;
            if done == SAMPLE {
                break 'outer;
            }
        }
    }

    let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
    println!("{done} model files, {parts} parts\n");
    println!(
        "base64  {base64_time:.2?}  ->  {:.1} MB on the wire",
        mb(base64_bytes)
    );
    println!(
        "raw     (a memcpy)  ->  {:.1} MB on the wire",
        mb(raw_bytes)
    );
    println!(
        "\nwire {:.2}x smaller, base64 costs {:.1} ms per model file",
        base64_bytes as f64 / raw_bytes.max(1) as f64,
        base64_time.as_secs_f64() * 1000.0 / done.max(1) as f64
    );
    println!(
        "\nindices  u32 {:.1} MB  ->  narrowed {:.1} MB ({}/{} parts fit u16, saving {:.1} MB)",
        mb(index_u32),
        mb(index_u16),
        narrowable,
        parts,
        mb(index_u32 - index_u16)
    );
}
