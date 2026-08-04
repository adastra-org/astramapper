//! Times the mapped extraction path against reopening the archive per read.
//!
//! Run with a real install:
//!
//! ```text
//! cargo run --release --example extract_bench
//! ASTRAMAPPER_GTA_PATH="..." cargo run --release --example extract_bench
//! ```
//!
//! Not a test. The gates prove the two paths agree byte for byte; this only
//! says what the difference costs.

use std::path::PathBuf;
use std::time::Instant;

use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

/// How many entries to pull. Enough to swamp per-run noise, small enough to
/// finish while you watch.
const SAMPLE: usize = 4_000;

fn main() {
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let exe = folder.join("GTA5.exe");
    let Ok(exe_data) = std::fs::read(&exe) else {
        eprintln!("no install at {}", folder.display());
        return;
    };
    let keys = GtaKeys::from_exe(&exe_data).expect("keys");

    let started = Instant::now();
    let manager = RpfManager::scan(&folder, &keys).expect("scan");
    println!(
        "scanned {} archives in {:.1?}",
        manager.archives.len(),
        started.elapsed()
    );

    // A spread of real streaming reads: the model and texture files the world
    // view actually pulls. Taken round-robin across archives rather than in
    // table order, because streaming a neighbourhood touches many archives and
    // the front of one archive's table is not representative of that.
    let mut per_archive: Vec<Vec<cw_core::rpf::EntryRef>> = Vec::new();
    for (a, archive) in manager.archives.iter().enumerate() {
        let mut here = Vec::new();
        for (e, entry) in archive.entries.iter().enumerate() {
            let name = &entry.name_lower;
            let wanted = match std::env::var("BENCH_KIND").as_deref() {
                // The index scans: thousands of small files, where the cost of
                // opening the archive is a real fraction of the read.
                Ok("small") => name.ends_with(".ytyp") || name.ends_with(".ymap"),
                _ => name.ends_with(".ydr") || name.ends_with(".ytd"),
            };
            if wanted {
                here.push(cw_core::rpf::EntryRef {
                    archive: a,
                    entry: e,
                });
            }
        }
        if !here.is_empty() {
            per_archive.push(here);
        }
    }
    let mut refs = Vec::new();
    let mut round = 0;
    while refs.len() < SAMPLE {
        let mut took = false;
        for entries in &per_archive {
            if let Some(r) = entries.get(round) {
                refs.push(*r);
                took = true;
                if refs.len() == SAMPLE {
                    break;
                }
            }
        }
        if !took {
            break;
        }
        round += 1;
    }
    let distinct: std::collections::HashSet<_> = refs
        .iter()
        .map(|r| &manager.archives[r.archive].physical_path)
        .collect();
    println!(
        "sampling {} entries across {} archives ({} files on disk)\n",
        refs.len(),
        per_archive.len().min(refs.len()),
        distinct.len()
    );

    // Reopen per read: what `RpfFile::extract` does, and what the manager did
    // before the mapping layer.
    let started = Instant::now();
    let mut reopen_bytes = 0usize;
    for r in &refs {
        let archive = &manager.archives[r.archive];
        if let Ok(data) = archive.extract(&archive.entries[r.entry], &keys) {
            reopen_bytes += data.len();
        }
    }
    let reopen = started.elapsed();

    // Through the mapping. First pass pays for the mappings themselves.
    let started = Instant::now();
    let mut mapped_bytes = 0usize;
    for r in &refs {
        if let Ok(data) = manager.extract(*r, &keys) {
            mapped_bytes += data.len();
        }
    }
    let mapped_cold = started.elapsed();

    let started = Instant::now();
    for r in &refs {
        let _ = manager.extract(*r, &keys);
    }
    let mapped_warm = started.elapsed();

    assert_eq!(reopen_bytes, mapped_bytes, "the two paths must agree");

    println!("reopen per read : {reopen:.2?}");
    println!("mapped (cold)   : {mapped_cold:.2?}");
    println!("mapped (warm)   : {mapped_warm:.2?}");
    println!(
        "\n{:.2} MB across {} reads, {} archives mapped",
        mapped_bytes as f64 / (1024.0 * 1024.0),
        refs.len(),
        manager.mapped_archives()
    );
    println!(
        "warm speedup    : {:.2}x",
        reopen.as_secs_f64() / mapped_warm.as_secs_f64().max(f64::EPSILON)
    );
}
