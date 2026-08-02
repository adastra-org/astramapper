//! The texture gate: prove the texture layer reads real `.ytd` files, agrees
//! with the C# reference field for field, and writes back the same `.dds`.
//!
//! ```text
//! pwsh -File scripts/texture-oracle.ps1
//! cargo test -p cw-core --test texture_gate -- --ignored --nocapture
//! ```
//!
//! Three checks, in descending order of how exact they can be:
//!
//! 1. **Metadata and payload, exactly.** Every field of every texture, plus a
//!    SHA-256 of `TextureData.FullData`. The payload hash is the one that
//!    matters: it proves the storage-length arithmetic, which is neither the
//!    mip layout nor anything a header states outright.
//! 2. **DDS bytes, exactly.** A SHA-256 of what `DDSIO.GetDDSFile` writes. One
//!    value covering the header, the pixel-format table and the per-mip pitch.
//! 3. **Decoded pixels, within a bound.** This one *cannot* be exact, and the
//!    reason is worth stating rather than papering over: `DecompressDxt1Block`
//!    expands the two endpoint colours to 8 bits and then interpolates with
//!    truncating division, while this port interpolates in the packed 5/6-bit
//!    domain and rounds, which is what the D3D spec and every GPU do. Only
//!    block indices 2 and 3 can differ, and only by rounding. The test asserts
//!    the bound and prints the worst deviation it saw, so a real decode error —
//!    a swapped channel, a wrong block order, a shifted mip — cannot hide under
//!    it: those move a channel by tens or hundreds, not by two.
//!
//! Plus a wide self-consistency pass over the whole install, which is the only
//! coverage BC7 gets: the reference's `GetPixels` has an empty
//! `case DXGI_FORMAT_BC7_UNORM` and returns null, so there is no oracle for it.

use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::image;
use cw_core::resources::{read_textures, Texture, TextureFormat};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use serde_json::Value;
use sha2::{Digest, Sha256};

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

/// How far a decoded channel may sit from CodeWalker's. See the module note:
/// the endpoint colours agree exactly and only the two interpolated ones can
/// move, so this is a rounding bound, not a fudge factor. A 5-bit channel steps
/// by 8.2 in 8-bit space, so a third of a step is the most rounding can move
/// one. The observed worst over the sample is **1**; the headroom is for the
/// formats the sample happens not to reach.
const MAX_CHANNEL_DEVIATION: i32 = 4;

fn gta_path() -> PathBuf {
    PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.to_string()))
}

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

fn scan(keys: &GtaKeys) -> RpfManager {
    RpfManager::scan(&gta_path(), keys).expect("scan the game folder")
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Read one entry and parse its texture dictionary.
fn textures_at(manager: &RpfManager, keys: &GtaKeys, path: &str) -> Vec<Texture> {
    let entry_ref = manager
        .find(path)
        .unwrap_or_else(|| panic!("{path} is not in the index"));
    let entry = manager.entry(entry_ref);
    let (system_size, _) = entry
        .resource_segments()
        .unwrap_or_else(|| panic!("{path} is not a resource entry"));
    let data = manager
        .extract(entry_ref, keys)
        .unwrap_or_else(|e| panic!("{path}: extract failed: {e}"));

    read_textures(FileKind::from_name(&entry.name_lower), &data, system_size)
        .unwrap_or_else(|e| panic!("{path}: parse failed: {e}"))
        .textures
}

fn u64_at(value: &Value, key: &str) -> u64 {
    value[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key} is not a number: {value}"))
}

#[test]
#[ignore = "needs a local GTA V install and fixtures/texture_oracle.json"]
fn agrees_with_the_reference_texture_reader() {
    let oracle_path = workspace_root().join("fixtures/texture_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "skipping: {} is missing. Run scripts/texture-oracle.ps1 first.",
            oracle_path.display()
        );
        return;
    };
    // PowerShell's Out-File -Encoding utf8 writes a BOM.
    let oracle: Value =
        serde_json::from_str(raw.trim_start_matches('\u{FEFF}')).expect("parse the texture oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("oracle files array");
    assert!(!files.is_empty(), "the oracle is empty");

    let mut textures_checked = 0usize;
    let mut dds_checked = 0usize;
    let mut pixels_checked = 0usize;
    let mut worst_deviation = 0i32;
    let mut bc7_without_oracle = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");

        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let entry = manager.entry(entry_ref);
        let (system_size, graphics_size) = entry.resource_segments().expect("a resource entry");

        assert_eq!(
            system_size,
            u64_at(file, "systemSize") as usize,
            "{path}: system segment size"
        );
        assert_eq!(
            graphics_size,
            u64_at(file, "graphicsSize") as usize,
            "{path}: graphics segment size"
        );
        // A `.ytd` is version 13, where a drawable is 165 — a different top
        // nibble in each flag word, so this catches a swapped pair.
        assert_eq!(
            entry.resource_version(),
            Some(u64_at(file, "version") as u32),
            "{path}: resource version"
        );

        let ours = textures_at(&manager, &keys, path);
        let theirs = file["textures"].as_array().expect("textures");
        assert_eq!(ours.len(), theirs.len(), "{path}: texture count");

        for (i, (texture, expected)) in ours.iter().zip(theirs).enumerate() {
            let at = format!("{path}[{i}]");

            assert_eq!(
                texture.name.as_deref(),
                expected["name"].as_str(),
                "{at}: name"
            );
            assert_eq!(
                texture.name_hash as u64,
                u64_at(expected, "nameHash"),
                "{at}: name hash"
            );
            assert_eq!(
                texture.width as u64,
                u64_at(expected, "width"),
                "{at}: width"
            );
            assert_eq!(
                texture.height as u64,
                u64_at(expected, "height"),
                "{at}: height"
            );
            assert_eq!(
                texture.depth as u64,
                u64_at(expected, "depth"),
                "{at}: depth"
            );
            assert_eq!(
                texture.stride as u64,
                u64_at(expected, "stride"),
                "{at}: stride"
            );
            assert_eq!(
                texture.format_raw as u64,
                u64_at(expected, "format"),
                "{at}: format"
            );
            assert_eq!(
                texture.levels as u64,
                u64_at(expected, "levels"),
                "{at}: mip level count"
            );
            assert_eq!(
                texture.usage.0 as u64,
                u64_at(expected, "usage"),
                "{at}: usage"
            );
            assert_eq!(
                texture.usage_flags as u64,
                u64_at(expected, "usageFlags"),
                "{at}: usage flags"
            );
            assert_eq!(
                texture.extra_flags as u64,
                u64_at(expected, "extraFlags"),
                "{at}: extra flags"
            );

            // The payload length is the reference's storage arithmetic, which
            // is not the mip layout. Getting it right to the byte is the point.
            assert_eq!(
                texture.data.len() as u64,
                u64_at(expected, "dataLength"),
                "{at}: payload length"
            );
            let expected_hash = expected["dataSha256"].as_str().unwrap_or("");
            if !expected_hash.is_empty() {
                assert_eq!(
                    hex_digest(&texture.data),
                    expected_hash,
                    "{at}: payload bytes"
                );
            }

            let expected_dds = expected["ddsSha256"].as_str().unwrap_or("");
            if !expected_dds.is_empty() {
                let dds = image::to_dds(texture)
                    .unwrap_or_else(|e| panic!("{at}: could not write a DDS: {e}"));
                assert_eq!(hex_digest(&dds), expected_dds, "{at}: DDS bytes");
                dds_checked += 1;
            }

            if expected["pixels"].is_object() {
                let deviation = compare_pixels(&at, texture, &expected["pixels"]);
                worst_deviation = worst_deviation.max(deviation);
                pixels_checked += 1;
            } else if texture.format == Some(TextureFormat::Bc7) {
                // Expected: `GetPixels` has an empty case for BC7 and returns
                // null. The wide pass below is what covers it.
                bc7_without_oracle += 1;
            }

            textures_checked += 1;
        }

        // The parallel hash array is a separate pointer from the texture array,
        // so it is checked separately rather than assumed to line up.
        let ours_dictionary_hashes: Vec<u64> = file["hashes"]
            .as_array()
            .expect("hashes")
            .iter()
            .map(|h| h.as_u64().expect("a hash"))
            .collect();
        let entry_ref = manager.find(path).expect("in the index");
        let data = manager.extract(entry_ref, &keys).expect("extract");
        let dictionary = read_textures(FileKind::Texture, &data, system_size).expect("parse");
        assert_eq!(
            dictionary
                .hashes
                .iter()
                .map(|h| *h as u64)
                .collect::<Vec<_>>(),
            ours_dictionary_hashes,
            "{path}: name hash table"
        );
    }

    // An empty oracle field reads to the loop above exactly like "the reference
    // cannot do this one", so it skips rather than fails. These two assertions
    // are what stop a broken oracle from passing as a green gate.
    assert!(
        dds_checked > 0,
        "the oracle recorded no DDS hashes to compare against"
    );
    assert!(
        pixels_checked > 0,
        "the oracle recorded no pixels to compare against"
    );
    println!(
        "{} files, {textures_checked} textures agree with CodeWalker field for field\n\
         {dds_checked} DDS files byte-identical, {pixels_checked} decodes within \
         {worst_deviation}/255 per channel ({bc7_without_oracle} BC7 textures the reference cannot decode)",
        files.len()
    );
}

/// Compare a sample of decoded pixels against the reference's, and return the
/// worst per-channel deviation seen.
///
/// The oracle stores BGRA, which is what `GetPixels` returns — it swaps red and
/// blue on the way out for the bitmap it feeds. Alpha is compared exactly for
/// every format except `X8R8G8B8`, where the reference passes the stored byte
/// through and this port forces it opaque, the byte being an `X` and not an `A`.
fn compare_pixels(at: &str, texture: &Texture, expected: &Value) -> i32 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let stride = expected["stride"].as_u64().expect("pixel stride") as usize;
    let bgra = STANDARD
        .decode(expected["bgra"].as_str().expect("pixel bytes"))
        .expect("base64 pixels");

    let decoded = image::decode_mip(texture, 0)
        .unwrap_or_else(|e| panic!("{at}: could not decode mip 0: {e}"));

    let compare_alpha = texture.format != Some(TextureFormat::X8R8G8B8);
    let mut worst = 0i32;

    for (sample, chunk) in bgra.chunks_exact(4).enumerate() {
        let index = sample * stride;
        let (x, y) = (index % decoded.width, index / decoded.width);
        let Some(ours) = decoded.pixel(x, y) else {
            break;
        };
        // Theirs is BGRA, ours is RGBA.
        let theirs = [chunk[2], chunk[1], chunk[0], chunk[3]];

        let channels = if compare_alpha { 4 } else { 3 };
        for channel in 0..channels {
            let deviation = (ours[channel] as i32 - theirs[channel] as i32).abs();
            worst = worst.max(deviation);
            assert!(
                deviation <= MAX_CHANNEL_DEVIATION,
                "{at}: pixel ({x},{y}) channel {channel} is {} against the reference's {} \
                 — {deviation} is past the {MAX_CHANNEL_DEVIATION}/255 rounding bound, \
                 so this is a decode error and not interpolation",
                ours[channel],
                theirs[channel]
            );
        }
    }

    worst
}

/// Every decodable texture in a wide sample of the install must decode to a
/// full surface, and its DDS must round-trip back to the same pixels.
///
/// This is the only coverage BC7 has — the reference cannot decode it. The DDS
/// round trip is what makes the check mean something for a format with no
/// oracle: `to_dds` copies the stored bytes and `decode_mip` interprets them, so
/// agreeing pins the mip offsets against the pitch table independently.
#[test]
#[ignore = "needs a local GTA V install"]
fn every_sampled_texture_decodes_to_a_full_surface() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<String> = Vec::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if FileKind::from_name(&entry.name_lower).has_textures() {
                paths.push(entry.path.clone());
            }
        }
    }
    assert!(!paths.is_empty(), "the install should contain .ytd files");

    // A spread across the whole install rather than the first N, which would
    // all come from one archive.
    let step = (paths.len() / 200).max(1);
    let sample: Vec<&String> = paths.iter().step_by(step).take(200).collect();

    let mut decoded = 0usize;
    let mut pixels = 0usize;
    let mut undecodable = 0usize;
    let mut by_format: std::collections::BTreeMap<&str, usize> = Default::default();

    for path in &sample {
        for texture in textures_at(&manager, &keys, path) {
            if !texture.is_decodable() {
                undecodable += 1;
                continue;
            }
            let format = texture.format.expect("decodable implies a format");

            let image = image::decode_mip(&texture, 0)
                .unwrap_or_else(|e| panic!("{path}/{:?}: {e}", texture.name));
            assert_eq!(
                image.pixels.len(),
                image.width * image.height * 4,
                "{path}/{:?}: short surface",
                texture.name
            );
            assert_eq!(image.width, texture.width as usize);
            assert_eq!(image.height, texture.height as usize);

            // The DDS carries the same bytes at the same offsets, so decoding
            // mip 0 out of it must give the same pixels back.
            let dds = image::to_dds(&texture).expect("write a DDS");
            let header = if format == TextureFormat::Bc7 {
                148
            } else {
                128
            };
            let (_, slice_pitch) = format.compute_pitch(image.width, image.height);
            assert!(
                dds.len() >= header + slice_pitch,
                "{path}/{:?}: DDS is too short for its own mip 0",
                texture.name
            );
            assert_eq!(
                &dds[header..header + slice_pitch],
                &texture.data[..slice_pitch],
                "{path}/{:?}: DDS mip 0 is not the stored mip 0",
                texture.name
            );

            *by_format.entry(format.label()).or_default() += 1;
            pixels += image.width * image.height;
            decoded += 1;
        }
    }

    assert!(
        decoded > 500,
        "only {decoded} textures decoded from the sample"
    );
    let formats: Vec<String> = by_format
        .iter()
        .map(|(name, count)| format!("{count} {name}"))
        .collect();
    println!(
        "{} files, {decoded} textures, {pixels} pixels decoded ({undecodable} not decodable)\n  {}",
        sample.len(),
        formats.join(", ")
    );
}

/// How many `.ytd` files in the install this port declines to read.
///
/// Not zero, and the number is pinned rather than tolerated. Six stock files
/// trip it, every one a resource whose payload does not inflate to the size its
/// page flags declare:
///
/// ```text
/// x64a.rpf\textures\parachute_decals.ytd                      794624 declared, 73349 extracted
/// x64f.rpf\...\des_setpiece.rpf\v_des_truck_txd.ytd           188416 declared, 123256
/// x64f.rpf\...\des_fib_ceiling2.rpf\v_des_ceil2_txd.ytd        49152 declared,  28123
/// x64f.rpf\...\des_hosp_ceil.rpf\des_hosp_ceil.ytd              8192 declared,      0
/// x64f.rpf\...\des_hosp_ceil.rpf\des_hosp_ceil_txd.ytd        139264 declared,  84675
/// x64f.rpf\...\des_hosp_ceil2.rpf\des_hosp_ceil2_txd.ytd        8192 declared,      0
/// ```
///
/// The reference throws an `ArgumentException` on all six, so this is a
/// property of the files and not of the port. If the budget ever needs raising,
/// the new file is a finding, not a nuisance.
const MAX_UNREADABLE_DICTIONARIES: usize = 6;

/// Every texture the whole install holds must at least *parse*, and its listed
/// shape must be self-consistent: a format we map means a mip chain that fits
/// the payload the file declares.
///
/// Broader and cheaper than the pass above — no decoding, so it can cover every
/// `.ytd` rather than a sample, which is what makes it useful for finding the
/// one dictionary in ten thousand with an unmapped format.
#[test]
#[ignore = "needs a local GTA V install"]
fn every_texture_dictionary_in_the_install_parses() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<String> = Vec::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if FileKind::from_name(&entry.name_lower).has_textures() {
                paths.push(entry.path.clone());
            }
        }
    }

    let mut dictionaries = 0usize;
    let mut textures = 0usize;
    let mut unmapped: std::collections::BTreeMap<u32, usize> = Default::default();
    let mut short_payloads = 0usize;
    let mut unreadable: Vec<String> = Vec::new();

    for path in &paths {
        let entry_ref = manager.find(path).expect("in the index");
        let entry = manager.entry(entry_ref);
        let Some((system_size, _)) = entry.resource_segments() else {
            continue;
        };
        let Ok(data) = manager.extract(entry_ref, &keys) else {
            continue;
        };
        let dictionary = match read_textures(FileKind::Texture, &data, system_size) {
            Ok(dictionary) => dictionary,
            Err(e) => {
                unreadable.push(format!("{path}: {e}"));
                continue;
            }
        };

        // The two parallel arrays come from two pointers. A dictionary where
        // they disagree would silently mislabel every texture after the gap;
        // the reader refuses those outright, so reaching here means they agree.
        assert_eq!(
            dictionary.hashes.len(),
            dictionary.textures.len(),
            "{path}: {} name hashes for {} textures",
            dictionary.hashes.len(),
            dictionary.textures.len()
        );

        for texture in &dictionary.textures {
            match texture.format {
                None => *unmapped.entry(texture.format_raw).or_default() += 1,
                Some(_) if !texture.is_decodable() => short_payloads += 1,
                Some(_) => {}
            }
            textures += 1;
        }
        dictionaries += 1;
    }

    assert!(
        dictionaries > 1000,
        "only {dictionaries} dictionaries found; the install should hold thousands"
    );
    assert!(
        unmapped.is_empty(),
        "formats with no decoder, as {{ D3DFORMAT: count }}: {unmapped:?}"
    );
    assert!(
        unreadable.len() <= MAX_UNREADABLE_DICTIONARIES,
        "{} dictionaries would not read, past the budget of {MAX_UNREADABLE_DICTIONARIES}:\n  {}",
        unreadable.len(),
        unreadable.join("\n  ")
    );
    println!(
        "{dictionaries} dictionaries, {textures} textures parse; \
         {short_payloads} with a payload shorter than their mip 0, \
         {} refused:\n  {}",
        unreadable.len(),
        unreadable.join("\n  ")
    );
}
