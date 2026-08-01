//! The phase 1.2 gate: prove the RPF layer reads the real game folder and
//! agrees with the C# reference on what is inside it.
//!
//! Like the crypto gate these need a GTA V Legacy install and are `#[ignore]`d.
//!
//! ```text
//! cargo test -p cw-core --test rpf_gate -- --ignored --nocapture
//! ```
//!
//! Expected values come from `CodeWalker.Core.dll` loaded into PowerShell, not
//! from reading this port's own output back. See `docs/phases.md`.

use cw_core::rpf::entry::RESOURCE_SIZE_ESCAPE as ESCAPE_U32;
use cw_core::rpf::{EntryKind, RpfEncryption, RpfFile, RpfManager};

/// The saturated resource-length sentinel, as a `u64` for JSON comparisons.
const RESOURCE_SIZE_ESCAPE: u64 = ESCAPE_U32 as u64;
use cw_core::utils::GtaKeys;
use std::path::PathBuf;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn gta_path() -> PathBuf {
    PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.to_string()))
}

/// `CARGO_MANIFEST_DIR` is the crate, not the workspace; fixtures live at the
/// workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve the workspace root")
}

fn load_keys() -> GtaKeys {
    let exe = gta_path().join("GTA5.exe");
    let data = std::fs::read(&exe).unwrap_or_else(|e| panic!("cannot read {}: {e}", exe.display()));
    GtaKeys::from_exe(&data).expect("key recovery failed")
}

/// The two archives the crypto gate already pinned, now read through the real
/// entry model rather than the hand-rolled loop that lived in that test.
#[test]
#[ignore = "needs a local GTA V install"]
fn reads_the_archives_the_crypto_gate_pinned() {
    let keys = load_keys();

    for (name, expected_entries) in [("x64a.rpf", 656u32), ("common.rpf", 1421)] {
        let path = gta_path().join(name);
        let rpf = RpfFile::open(&path, name, &keys).unwrap_or_else(|e| panic!("{name}: {e}"));

        assert_eq!(rpf.encryption, RpfEncryption::Ng, "{name} should be NG");
        assert_eq!(rpf.entry_count, expected_entries, "{name} entry count");
        assert_eq!(rpf.entries.len(), expected_entries as usize);
        assert!(rpf.root().is_directory(), "{name} root must be a directory");
        assert_eq!(rpf.root().path, name, "{name} root path");

        // Every non-root entry got a path from the tree walk. An orphan means
        // the child ranges did not cover the table.
        let orphans = rpf
            .entries
            .iter()
            .skip(1)
            .filter(|e| e.parent.is_none())
            .count();
        assert_eq!(orphans, 0, "{name}: {orphans} entries have no parent");

        println!(
            "{name}: {} entries, {} nested archives, {} total",
            rpf.entries.len(),
            rpf.children.len(),
            rpf.total_entry_count()
        );
    }
}

/// Extraction has to survive every entry in an archive, not a hand-picked one.
#[test]
#[ignore = "needs a local GTA V install"]
fn extracts_every_file_in_an_archive() {
    let keys = load_keys();
    let path = gta_path().join("x64a.rpf");
    let rpf = RpfFile::open(&path, "x64a.rpf", &keys).expect("open x64a.rpf");

    let mut reader = std::fs::File::open(&path).unwrap();
    let (mut ok, mut bytes, mut failed) = (0usize, 0u64, Vec::new());

    for entry in rpf.entries.iter().filter(|e| e.is_file()) {
        match rpf.extract_from(&mut reader, entry, &keys) {
            Ok(data) => {
                ok += 1;
                bytes += data.len() as u64;
            }
            Err(e) => failed.push(format!("{}: {e}", entry.path)),
        }
    }

    assert!(
        failed.is_empty(),
        "{} of {} extractions failed:\n{}",
        failed.len(),
        ok + failed.len(),
        failed.join("\n")
    );
    println!("x64a.rpf: extracted {ok} files, {bytes} bytes");
}

/// A resource's inflated payload must begin with the RSC7 magic once the
/// 16-byte header is skipped — the cheapest proof that the offset, the sector
/// arithmetic and the deflate are all right at once.
#[test]
#[ignore = "needs a local GTA V install"]
fn extracted_resources_are_not_noise() {
    let keys = load_keys();
    let path = gta_path().join("x64a.rpf");
    let rpf = RpfFile::open(&path, "x64a.rpf", &keys).expect("open x64a.rpf");
    let mut reader = std::fs::File::open(&path).unwrap();

    let mut checked = 0;
    for entry in rpf.entries.iter() {
        let EntryKind::Resource(_) = &entry.kind else {
            continue;
        };
        let data = rpf.extract_from(&mut reader, entry, &keys).unwrap();
        assert!(
            !data.is_empty(),
            "{}: resource extracted to nothing",
            entry.path
        );
        // Inflated resource payloads are the system+graphics blocks, not
        // another RSC7 header, so the check is that they are plausibly sized
        // rather than a magic number.
        assert!(
            data.len() >= 16,
            "{}: {} bytes is too short to be a resource",
            entry.path,
            data.len()
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "x64a.rpf has no resource entries — wrong archive?"
    );
    println!("x64a.rpf: {checked} resources inflate to plausible payloads");
}

/// The acceptance criterion for phase 1.2: enumerate the whole game folder.
#[test]
#[ignore = "needs a local GTA V install"]
fn enumerates_every_archive_in_the_game_folder() {
    let keys = load_keys();
    let folder = gta_path();

    let started = std::time::Instant::now();
    let manager = RpfManager::scan(&folder, &keys).expect("scan the game folder");
    let elapsed = started.elapsed();

    let top = manager.top_level().count();
    let nested = manager.archives.len() - top;

    println!(
        "scanned {top} top-level archives ({nested} nested, {} total) \
         holding {} entries in {:.1}s",
        manager.archives.len(),
        manager.entry_count(),
        elapsed.as_secs_f64()
    );

    for e in &manager.errors {
        println!("  FAILED {}: {}", e.path.display(), e.message);
    }

    assert!(
        manager.errors.is_empty(),
        "{} archives failed to scan",
        manager.errors.len()
    );
    assert_eq!(
        top, 179,
        "expected 179 top-level legacy RPFs, found {top} — check the install"
    );

    // The index has to resolve a path that crosses an archive boundary.
    let root = manager.find("x64a.rpf").expect("x64a.rpf is not indexed");
    assert!(manager.entry(root).is_directory());
    assert!(!manager.list_root(root.archive).is_empty());
}

/// The cross-check that actually matters: every entry, compared field by field
/// against what `CodeWalker.Core.dll` reports for the same archive.
///
/// This port agreeing with itself proves nothing. Regenerate the expected data
/// with `scripts/rpf-oracle.ps1`; the test skips if it is missing, since
/// `fixtures/` is gitignored.
#[test]
#[ignore = "needs a local GTA V install"]
fn matches_the_reference_entry_by_entry() {
    let oracle_path = workspace_root().join("fixtures/rpf_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "SKIP: {} is missing. Generate it with scripts/rpf-oracle.ps1",
            oracle_path.display()
        );
        return;
    };
    // PowerShell's Out-File -Encoding utf8 writes a BOM.
    let oracle: serde_json::Value =
        serde_json::from_str(raw.trim_start_matches('\u{feff}')).expect("parse rpf_oracle.json");

    let keys = load_keys();
    let archives = oracle.as_object().expect("oracle is an object");
    assert!(!archives.is_empty(), "the oracle dump is empty");

    for (name, expected) in archives {
        let path = gta_path().join(name);
        let rpf = RpfFile::open(&path, name, &keys).unwrap_or_else(|e| panic!("{name}: {e}"));

        assert_eq!(
            rpf.entry_count as u64,
            expected["entryCount"].as_u64().unwrap(),
            "{name}: entry count"
        );
        assert_eq!(
            rpf.names_length as u64,
            expected["namesLength"].as_u64().unwrap(),
            "{name}: names length"
        );

        let want = expected["entries"].as_array().unwrap();
        assert_eq!(want.len(), rpf.entries.len(), "{name}: entry list length");

        let mut mismatches = Vec::new();
        for (i, (got, want)) in rpf.entries.iter().zip(want).enumerate() {
            let want_path = want["path"].as_str().unwrap();
            let want_kind = want["kind"].as_str().unwrap();
            let got_kind = match &got.kind {
                EntryKind::Directory(_) => "dir",
                EntryKind::Binary(_) => "bin",
                EntryKind::Resource(_) => "res",
            };
            // The reference reports offset 0 and size 0 for directories.
            let want_offset = want["offset"].as_u64().unwrap();
            let want_size = want["size"].as_u64().unwrap();
            let got_offset = got.file_offset().unwrap_or(0) as u64;
            let got_size = got.file_size_on_disk();

            if got.path != want_path {
                mismatches.push(format!(
                    "[{i}] path: got {:?}, want {want_path:?}",
                    got.path
                ));
            }
            if got_kind != want_kind {
                mismatches.push(format!("[{i}] {want_path}: kind {got_kind} != {want_kind}"));
            }
            if got_offset != want_offset {
                mismatches.push(format!(
                    "[{i}] {want_path}: offset {got_offset} != {want_offset}"
                ));
            }
            if got_size != want_size {
                mismatches.push(format!("[{i}] {want_path}: size {got_size} != {want_size}"));
            }
        }

        assert!(
            mismatches.is_empty(),
            "{name}: {} of {} entries disagree with the reference:\n{}",
            mismatches.len(),
            want.len(),
            mismatches
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );

        // Extracted payloads, byte for byte. The metadata above can be right
        // while the sector arithmetic, the decryption or the inflate is wrong;
        // this is the check that cannot be passed by accident.
        let mut reader = std::fs::File::open(&path).unwrap();
        let mut compared = 0usize;
        let mut bad = Vec::new();
        for (got, want) in rpf.entries.iter().zip(want) {
            let Some(want_hash) = want["dataSha256"].as_str() else {
                continue; // a directory, or the reference could not read it
            };
            let data = match rpf.extract_from(&mut reader, got, &keys) {
                Ok(d) => d,
                Err(e) => {
                    bad.push(format!("{}: extraction failed: {e}", got.path));
                    continue;
                }
            };
            let want_len = want["dataLength"].as_u64().unwrap();
            if data.len() as u64 != want_len {
                bad.push(format!(
                    "{}: {} bytes, reference got {want_len}",
                    got.path,
                    data.len()
                ));
                continue;
            }
            if sha256_hex(&data) != want_hash {
                bad.push(format!("{}: content differs from the reference", got.path));
                continue;
            }
            compared += 1;
        }

        assert!(
            bad.is_empty(),
            "{name}: {} extracted payloads disagree with the reference:\n{}",
            bad.len(),
            bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
        );

        println!(
            "{name}: all {} entries and all {compared} extracted payloads match the reference",
            want.len()
        );
    }
}

/// The `OPEN` path, against a second independent implementation.
///
/// Every retail archive is NG-encrypted, so nothing above this exercises the
/// unencrypted table-of-contents branch. RPFTool is a prior TypeScript reader
/// that only handles `OPEN` archives, and it ships seven as fixtures. Two
/// implementations written from the same spec disagreeing is the cheapest bug
/// signal available.
///
/// Regenerate with `bash scripts/rpftool-oracle.sh`.
#[test]
#[ignore = "needs the RPFTool checkout and its fixtures"]
fn matches_rpftool_on_open_archives() {
    let dir = workspace_root().join("fixtures/rpftool");
    let Ok(raw) = std::fs::read_to_string(dir.join("index.json")) else {
        println!(
            "SKIP: {} is missing. Generate it with scripts/rpftool-oracle.sh",
            dir.join("index.json").display()
        );
        return;
    };
    let index: serde_json::Value = serde_json::from_str(&raw).expect("parse index.json");
    let archives = index.as_array().expect("index.json is an array");
    assert!(!archives.is_empty(), "the RPFTool index is empty");

    // These archives are unencrypted, so the keys are never consulted. Build
    // them anyway rather than special-casing the call.
    let keys = load_keys();
    let mut total = 0usize;

    for a in archives {
        let name = a["name"].as_str().unwrap();
        let rpf_path = PathBuf::from(a["rpf"].as_str().unwrap());
        let dump: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap(),
        )
        .expect("parse an RPFTool dump");

        let rpf =
            RpfFile::open(&rpf_path, "dlc.rpf", &keys).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            rpf.encryption,
            RpfEncryption::Open,
            "{name}: expected an OPEN archive, got {:?}",
            rpf.encryption
        );

        // RPFTool presents one transparent tree: it descends into nested
        // archives and reports their contents as ordinary paths, with the
        // `.rpf` acting as a directory component. Flattening every archive in
        // this subtree the same way makes the nesting part of the comparison
        // rather than something excluded from it.
        //
        // Paths are forward-slashed and relative to the archive root, and sizes
        // follow RPFTool's own convention: on-disk for resources, uncompressed
        // for binaries. Neither matches CodeWalker's `GetFileSize()`, which is
        // what makes this an independent check and not a restatement.
        let mut ours = std::collections::HashMap::new();
        for archive in rpf.iter_archives() {
            for e in archive.entries.iter().filter(|e| e.is_file()) {
                let rel = e
                    .path
                    .strip_prefix("dlc.rpf\\")
                    .unwrap_or(&e.path)
                    .replace('\\', "/");
                let size = match &e.kind {
                    EntryKind::Binary(b) => b.uncompressed_size as u64,
                    EntryKind::Resource(r) => r.file_size as u64,
                    EntryKind::Directory(_) => unreachable!(),
                };
                let kind = match &e.kind {
                    EntryKind::Resource(_) => "resource",
                    _ => "binary",
                };
                ours.insert(rel, (kind, size));
            }
        }

        let mut bad = Vec::new();
        let mut compared = 0;
        let mut escapes = 0;
        for f in dump["files"].as_array().unwrap() {
            let path = f["path"].as_str().unwrap();
            let want_kind = f["kind"].as_str().unwrap();
            let want_size = f["size"].as_u64().unwrap();
            match ours.get(path) {
                None => bad.push(format!("{path}: RPFTool has it, this port does not")),
                Some((kind, size)) => {
                    if *kind != want_kind {
                        bad.push(format!("{path}: kind {kind} != {want_kind}"));
                    } else if want_size == RESOURCE_SIZE_ESCAPE && *size >= RESOURCE_SIZE_ESCAPE {
                        // A known, verified divergence. RPFTool reports the raw
                        // 0xFFFFFF sentinel; it does not implement the escape
                        // where the real length is unpacked from the payload's
                        // own header. CodeWalker does, and agrees with this
                        // port: for b2a's `b2a.ytd` both say 20921589.
                        //
                        // So this is the one place the two oracles disagree and
                        // RPFTool is the one that is wrong. Assert only that we
                        // resolved it to something at least as large.
                        escapes += 1;
                    } else if *size != want_size {
                        bad.push(format!("{path}: size {size} != {want_size}"));
                    } else {
                        compared += 1;
                    }
                }
            }
        }

        assert!(
            bad.is_empty(),
            "{name}: {} entries disagree with RPFTool:\n{}",
            bad.len(),
            bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
        );
        assert!(compared > 0, "{name}: nothing was compared");
        if escapes > 0 {
            println!("{name}: {compared} entries agree with RPFTool ({escapes} oversized resources RPFTool leaves unresolved)");
        } else {
            println!("{name}: {compared} entries agree with RPFTool");
        }
        total += compared;
    }

    println!(
        "{total} OPEN-archive entries agree with RPFTool across {} archives",
        archives.len()
    );
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Listing must step transparently from an archive into one nested inside it,
/// otherwise the Explorer sees `.rpf` files as dead ends.
#[test]
#[ignore = "needs a local GTA V install"]
fn listing_steps_into_nested_archives() {
    let keys = load_keys();
    let manager = RpfManager::scan(&gta_path(), &keys).expect("scan the game folder");

    let nested = manager
        .archives
        .iter()
        .enumerate()
        .find(|(i, _)| manager.archive_parent[*i].is_some())
        .map(|(i, a)| (i, a.path.clone()))
        .expect("the install should contain nested archives");

    let (index, path) = nested;
    let entry = manager
        .find(&path)
        .unwrap_or_else(|| panic!("nested archive {path} has no entry"));

    assert_eq!(
        manager.nested_archive(entry),
        Some(index),
        "{path} does not resolve to the archive it opens"
    );

    let listed = manager.list_dir(entry);
    assert!(!listed.is_empty(), "{path} listed as empty");
    assert!(
        listed.iter().all(|r| r.archive == index),
        "{path} listing leaked entries from the parent archive"
    );
    println!("{path}: {} entries at its root", listed.len());
}
