//! What shipping a texture to the WebView costs, the current way against
//! handing over the blocks the GPU already wants.
//!
//! Today: decode BC to full-resolution RGBA, deflate that into a PNG, base64
//! the PNG. The proposed path copies the mip chain out of the resource and
//! uploads it with `compressedTexImage2D`.
//!
//! ```text
//! cargo run --release --example texture_cost
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cw_core::filetype::FileKind;
use cw_core::image;
use cw_core::resources::read_textures;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

const SAMPLE: usize = 600;

fn main() {
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let Ok(exe_data) = std::fs::read(folder.join("GTA5.exe")) else {
        eprintln!("no install at {}", folder.display());
        return;
    };
    let keys = GtaKeys::from_exe(&exe_data).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");

    let mut png_time = Duration::ZERO;
    let mut blocks_time = Duration::ZERO;
    let mut png_bytes = 0usize;
    let mut block_bytes = 0usize;
    let mut rgba_bytes = 0usize;
    let mut done = 0usize;
    let mut compressed = 0usize;

    'outer: for archive in &manager.archives {
        for entry in &archive.entries {
            if !entry.name_lower.ends_with(".ytd") {
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
            let Ok(dict) = read_textures(FileKind::Texture, &data, size) else {
                continue;
            };

            for texture in &dict.textures {
                // What ships today.
                let started = Instant::now();
                let Ok(decoded) = image::decode_mip(texture, 0) else {
                    continue;
                };
                let Ok(png) = image::to_png(&decoded) else {
                    continue;
                };
                let encoded = STANDARD.encode(&png);
                png_time += started.elapsed();
                png_bytes += encoded.len();
                rgba_bytes += decoded.pixels.len();

                // What could ship: every mip, as stored.
                let started = Instant::now();
                let mut chain: Vec<u8> = Vec::new();
                for level in 0..texture.levels as usize {
                    if let Some(bytes) = texture.mip_bytes(level) {
                        chain.extend_from_slice(bytes);
                    }
                }
                let raw = chain.len();
                blocks_time += started.elapsed();
                block_bytes += raw;

                if texture.format.is_some_and(|f| f.is_compressed()) {
                    compressed += 1;
                }
                done += 1;
                if done == SAMPLE {
                    break 'outer;
                }
            }
        }
    }

    let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
    println!("{done} textures ({compressed} block-compressed)\n");
    println!(
        "decode + png + base64 : {png_time:.2?}  ->  {:.1} MB on the wire",
        mb(png_bytes)
    );
    println!(
        "raw mip chain         : {blocks_time:.2?}  ->  {:.1} MB on the wire",
        mb(block_bytes)
    );
    println!(
        "\nintermediate RGBA the current path allocates and throws away: {:.1} MB",
        mb(rgba_bytes)
    );
    println!(
        "cpu   {:.0}x cheaper, wire {:.2}x smaller",
        png_time.as_secs_f64() / blocks_time.as_secs_f64().max(f64::EPSILON),
        png_bytes as f64 / block_bytes.max(1) as f64
    );
}
