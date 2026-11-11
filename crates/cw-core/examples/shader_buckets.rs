//! Counts drawable shaders by (render bucket, `.sps` file) over every Nth model
//! file of an install, with the names of the files this crate knows about. The
//! evidence for which shaders the World view alpha tests or skips.
//!
//! ```text
//! cargo run --release -p cw-core --example shader_buckets -- 8
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::resources::{read_named_drawables, read_named_fragments};
use cw_core::rpf::RpfManager;
use cw_core::utils::{jenk::gen_hash, GtaKeys};

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn main() {
    let every: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(8);
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let exe = std::fs::read(folder.join("GTA5.exe")).expect("GTA5.exe");
    let keys = GtaKeys::from_exe(&exe).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");
    let known: Vec<String> = include_str!("shader_names.txt")
        .lines()
        .map(str::to_owned)
        .collect();
    let names: std::collections::HashMap<u32, &str> = known
        .iter()
        .map(|n| (gen_hash(&format!("{n}.sps")), n.as_str()))
        .collect();

    let mut counts: BTreeMap<(u8, u32), usize> = BTreeMap::new();
    let mut seen = 0usize;
    let mut files = 0usize;
    for archive in &manager.archives {
        for entry in &archive.entries {
            let kind = FileKind::from_name(&entry.name_lower);
            if !matches!(kind, FileKind::Model | FileKind::Fragment) {
                continue;
            }
            seen += 1;
            if !seen.is_multiple_of(every) {
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
            files += 1;
            for nd in &named {
                for s in &nd.drawable.shaders {
                    *counts.entry((s.render_bucket, s.file_name)).or_default() += 1;
                }
            }
        }
    }
    println!("{files} model files sampled (every {every}th)");
    let mut by_bucket: BTreeMap<u8, Vec<(usize, u32)>> = BTreeMap::new();
    for ((bucket, file), n) in counts {
        by_bucket.entry(bucket).or_default().push((n, file));
    }
    for (bucket, mut list) in by_bucket {
        list.sort_by(|a, b| b.cmp(a));
        let total: usize = list.iter().map(|x| x.0).sum();
        println!("bucket {bucket}: {total} shaders, {} files", list.len());
        for (n, file) in list.iter().take(45) {
            let name = names
                .get(file)
                .map(|s| s.to_string())
                .unwrap_or(format!("{file:#010x}"));
            println!("   {n:>7}  {name}");
        }
    }
}
