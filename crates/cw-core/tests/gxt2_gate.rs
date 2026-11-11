//! The GXT2 gate: prove the text-table reader parses real `.gxt2` files and
//! that parse → serialize reproduces them byte for byte.
//!
//! ```text
//! cargo test -p cw-core --test gxt2_gate -- --ignored --nocapture
//! ```
//!
//! GXT2 is the first format in the port that round-trips: the reference has a
//! full `Save`, and this port has a full `write`, so `docs/formats.md` marks it
//! **done** rather than partial. The byte-equality check is the whole gate —
//! a reader that swapped two fields would not reproduce the input.

use std::path::PathBuf;

use cw_core::gxt2::Gxt2File;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn gta_path() -> PathBuf {
    PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.to_string()))
}

fn load_keys() -> GtaKeys {
    let exe = gta_path().join("GTA5.exe");
    let data = std::fs::read(&exe).unwrap_or_else(|e| panic!("cannot read {}: {e}", exe.display()));
    GtaKeys::from_exe(&data).expect("key recovery failed")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve the workspace root")
}

/// Every sampled `.gxt2` agrees with the reference entry by entry.
#[test]
#[ignore = "needs a local GTA V install and fixtures/gxt2_oracle.json"]
fn agrees_with_the_reference_gxt2_reader() {
    let oracle_path = workspace_root().join("fixtures/gxt2_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "skipping: {} is missing. Run scripts/gxt2-oracle.ps1 first.",
            oracle_path.display()
        );
        return;
    };
    let oracle: serde_json::Value =
        serde_json::from_str(raw.trim_start_matches('\u{FEFF}')).expect("parse the gxt2 oracle");

    let keys = load_keys();
    let manager = RpfManager::scan(&gta_path(), &keys).expect("scan the game folder");

    let files = oracle["files"].as_array().expect("oracle files array");
    let mut entries = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");
        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let data = manager
            .extract(entry_ref, &keys)
            .unwrap_or_else(|e| panic!("{path}: extract failed: {e}"));

        let got = Gxt2File::read(&data).unwrap_or_else(|e| panic!("{path}: parse failed: {e}"));

        let want_entries = file["entries"].as_array().expect("entries");
        assert_eq!(got.entries.len(), want_entries.len(), "{path}: entry count");
        for (i, (g, w)) in got.entries.iter().zip(want_entries).enumerate() {
            assert_eq!(
                g.hash,
                w["hash"].as_u64().expect("hash") as u32,
                "{path}[{i}]: hash"
            );
            assert_eq!(
                g.text,
                w["text"].as_str().expect("text"),
                "{path}[{i}]: text"
            );
        }
        entries += got.entries.len();
    }

    assert!(files.len() > 0, "the oracle should list some .gxt2 files");
    println!(
        "{} .gxt2 files agree with the reference, {entries} entries",
        files.len()
    );
}

/// Every `.gxt2` in the install parses, and parse → serialize reproduces the
/// extracted payload byte for byte.
#[test]
#[ignore = "needs a local GTA V install"]
fn every_gxt2_round_trips() {
    let keys = load_keys();
    let manager = RpfManager::scan(&gta_path(), &keys).expect("scan the game folder");

    let mut total = 0usize;
    let mut parsed = 0usize;
    let mut roundtripped = 0usize;
    let mut entries = 0usize;

    for archive in &manager.archives {
        for entry in &archive.entries {
            if !entry.name_lower.ends_with(".gxt2") {
                continue;
            }
            total += 1;
            let entry_ref = manager.find(&entry.path).expect("entry in index");
            let data = manager
                .extract(entry_ref, &keys)
                .unwrap_or_else(|e| panic!("{}: extract failed: {e}", entry.path));

            match Gxt2File::read(&data) {
                Ok(file) => {
                    parsed += 1;
                    entries += file.entries.len();
                    assert_eq!(
                        file.write(),
                        data,
                        "{}: parse → serialize did not reproduce the payload",
                        entry.path
                    );
                    roundtripped += 1;
                }
                Err(e) => {
                    panic!("{}: parse failed: {e}", entry.path);
                }
            }
        }
    }

    assert!(
        total > 1000,
        "the install should contain thousands of .gxt2 files"
    );
    assert_eq!(total, parsed, "every .gxt2 should parse");
    assert_eq!(total, roundtripped, "every .gxt2 should round-trip");
    println!("{total} .gxt2 files parse and round-trip byte for byte, {entries} entries");
}
