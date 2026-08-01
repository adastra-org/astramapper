//! The phase 1 gate: prove the ported crypto opens a real retail archive.
//!
//! Every stock GTA V archive is NG-encrypted, and the keys are not distributed —
//! they are recovered from the local `GTA5.exe`. Until this passes, no parser in
//! the crate has any real input to read, so nothing downstream should be built
//! on top of it.
//!
//! These tests need a GTA V Legacy install and are `#[ignore]`d so CI stays
//! green without one. Run them with:
//!
//! ```text
//! cargo test -p cw-core -- --ignored --nocapture
//! ```
//!
//! Point them at a different install with `ASTRAMAPPER_GTA_PATH`.

use cw_core::utils::{gen_hash_bytes, DataReader, GtaKeys};
use std::path::PathBuf;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

/// `RpfEncryption` from `RpfFile.cs`.
const ENC_NONE: u32 = 0;
const ENC_OPEN: u32 = 0x4E45_504F; // "OPEN"
const ENC_AES: u32 = 0x0FFF_FFF9;
const ENC_NG: u32 = 0x0FEF_FFFF;

/// `RPF7`, the only version GTA V ships.
const RPF7: u32 = 0x5250_4637;

fn gta_path() -> PathBuf {
    PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.to_string()))
}

fn load_keys() -> GtaKeys {
    let exe = gta_path().join("GTA5.exe");
    let data = std::fs::read(&exe).unwrap_or_else(|e| panic!("cannot read {}: {e}", exe.display()));
    GtaKeys::from_exe(&data).expect("key recovery failed")
}

#[test]
#[ignore = "needs a local GTA V install"]
fn recovers_the_keys_from_the_local_exe() {
    let keys = load_keys();

    // Fingerprints of the recovered material, so a regression in the SHA-1
    // scan, the unmasking or the inflate is caught precisely rather than as a
    // vague downstream failure. These are Jenkins digests, not the keys: they
    // pin the bytes without committing them.
    assert_eq!(
        gen_hash_bytes(&keys.aes_key),
        0x9599_6749,
        "AES key does not match the expected fingerprint"
    );
    assert_eq!(gen_hash_bytes(&keys.lut), 0x382A_E9D0, "hash LUT changed");

    let flat: Vec<u8> = keys
        .ng_keys
        .iter()
        .flat_map(|k| k.iter().copied())
        .collect();
    assert_eq!(gen_hash_bytes(&flat), 0xD941_EE2C, "NG key set changed");

    assert_eq!(
        keys.awc_key,
        [4165194522, 2330575623, 1737828644, 4060688116]
    );

    // The LUT is *not* a permutation — it has 229 distinct values, with 27
    // slots differing from the identity mapping. That matters: `gta5_hash` is
    // Jenkins composed with this table, so for the 229 unremapped byte values
    // the two hashes coincide exactly. A port that dropped the table would
    // still produce correct hashes for most inputs and wrong ones for the rest.
    let remapped = keys
        .lut
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b != i as u8)
        .count();
    assert_eq!(remapped, 27, "LUT remapping changed");

    println!(
        "recovered {} NG keys; LUT remaps {remapped} of 256 byte values",
        keys.ng_keys.len()
    );
}

/// Decrypt the entry and name tables of a real archive and read filenames out
/// of them. Nothing but correct keys and a correct NG implementation produces
/// readable ASCII names here.
#[test]
#[ignore = "needs a local GTA V install"]
fn opens_a_real_encrypted_archive() {
    let keys = load_keys();

    for archive in ["x64a.rpf", "common.rpf"] {
        let path = gta_path().join(archive);
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let file_size = data.len() as u32;

        let mut r = DataReader::new(&data);
        let version = r.read_u32().unwrap();
        let entry_count = r.read_u32().unwrap();
        let names_length = r.read_u32().unwrap();
        let encryption = r.read_u32().unwrap();

        assert_eq!(version, RPF7, "{archive}: not an RPF7 archive");

        let entries_raw = r.read_bytes(entry_count as usize * 16).unwrap();
        let names_raw = r.read_bytes(names_length as usize).unwrap();

        let (entries, names) = match encryption {
            ENC_NONE | ENC_OPEN => (entries_raw.to_vec(), names_raw.to_vec()),
            ENC_AES => (keys.decrypt_aes(entries_raw), keys.decrypt_aes(names_raw)),
            // Anything unrecognised is assumed NG, as the reference does.
            _ => (
                keys.decrypt_ng(entries_raw, archive, file_size),
                keys.decrypt_ng(names_raw, archive, file_size),
            ),
        };

        assert_eq!(
            encryption, ENC_NG,
            "{archive}: expected a retail NG-encrypted archive"
        );

        // The first entry is always the root directory: y=0 (empty name at
        // offset 0) and x=0x7fffff00.
        let mut er = DataReader::new(&entries);
        let _root_name_offset = er.read_u32().unwrap();
        let root_marker = er.read_u32().unwrap();
        assert_eq!(
            root_marker, 0x7fff_ff00,
            "{archive}: root entry is not a directory — decryption produced noise"
        );

        // Every entry's name offset must land inside the name table, and the
        // names must be printable. Random bytes would fail both.
        let mut printable_names = 0;
        for i in 0..entry_count as usize {
            let mut e = DataReader::new(&entries);
            e.set_position(i * 16);
            let name_offset = (e.read_u32().unwrap() & 0xFFFF) as usize;
            assert!(
                name_offset < names.len(),
                "{archive}: entry {i} name offset {name_offset} is outside the {} byte name table",
                names.len()
            );

            let mut nr = DataReader::new(&names);
            nr.set_position(name_offset);
            let name = nr.read_string().unwrap();
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                printable_names += 1;
            }
        }

        let ratio = printable_names as f64 / entry_count as f64;
        assert!(
            ratio > 0.95,
            "{archive}: only {printable_names}/{entry_count} names are printable ({ratio:.2}) — \
             decryption is wrong"
        );

        println!(
            "{archive}: {entry_count} entries, {printable_names} printable names, \
             {} byte name table",
            names.len()
        );
    }
}

/// The key index depends on both the archive name and its size on disk. Feeding
/// the wrong one selects a different key and yields noise — worth pinning,
/// because it fails silently rather than erroring.
#[test]
#[ignore = "needs a local GTA V install"]
fn the_wrong_name_or_size_selects_a_different_key() {
    let keys = load_keys();
    let path = gta_path().join("x64a.rpf");
    let data = std::fs::read(&path).expect("read x64a.rpf");
    let size = data.len() as u32;

    let right = keys.ng_key_for("x64a.rpf", size);
    let wrong_name = keys.ng_key_for("x64b.rpf", size);
    let wrong_size = keys.ng_key_for("x64a.rpf", size + 1);

    assert_ne!(
        right, wrong_name,
        "a different name must select another key"
    );
    assert_ne!(
        right, wrong_size,
        "a different size must select another key"
    );
}
