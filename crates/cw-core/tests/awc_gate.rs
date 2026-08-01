//! The AWC gate: prove the audio port reads real containers and decodes the
//! same samples the C# reference does.
//!
//! ```text
//! pwsh -File scripts/awc-oracle.ps1
//! cargo test -p cw-core --test awc_gate -- --ignored --nocapture
//! ```
//!
//! The oracle carries a SHA-256 of each stream's decoded PCM, truncated to the
//! declared sample count. That is the check that matters: it covers the header,
//! the chunk tables, the block de-interleave, the decryption and the ADPCM
//! decoder in one number. Everything else compared here is a way of localising a
//! failure once that number disagrees.

use std::path::PathBuf;

use cw_core::audio::{AwcFile, Codec};
use cw_core::filetype::FileKind;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use serde_json::Value;
use sha2::{Digest, Sha256};

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

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

/// The reference's `AwcCodecType` values, as the oracle writes them.
fn codec_number(codec: Option<Codec>) -> i64 {
    match codec {
        Some(Codec::Pcm) => 0,
        Some(Codec::Adpcm) => 4,
        Some(Codec::Unknown(v)) => v as i64,
        None => -1,
    }
}

fn pcm_sha256(samples: &[i16]) -> String {
    let mut hasher = Sha256::new();
    for sample in samples {
        hasher.update(sample.to_le_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
#[ignore = "needs a local GTA V install and fixtures/awc_oracle.json"]
fn agrees_with_the_reference_awc_reader() {
    let oracle_path = workspace_root().join("fixtures/awc_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "skipping: {} is missing. Run scripts/awc-oracle.ps1 first.",
            oracle_path.display()
        );
        return;
    };
    // PowerShell's Out-File -Encoding utf8 writes a BOM.
    let oracle: Value =
        serde_json::from_str(raw.trim_start_matches('\u{FEFF}')).expect("parse the awc oracle");

    let keys = load_keys();
    let manager = RpfManager::scan(&gta_path(), &keys).expect("scan the game folder");

    let files = oracle["files"].as_array().expect("oracle files array");
    assert!(!files.is_empty(), "the oracle is empty");

    let mut streams_checked = 0usize;
    let mut samples_checked = 0usize;
    let mut multi_channel_files = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");

        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let data = manager
            .extract(entry_ref, &keys)
            .unwrap_or_else(|e| panic!("{path}: extract failed: {e}"));

        let awc = AwcFile::read(&data, &keys.awc_key)
            .unwrap_or_else(|e| panic!("{path}: parse failed: {e}"));

        assert_eq!(
            awc.version as u64,
            file["version"].as_u64().expect("version"),
            "{path}: version"
        );
        assert_eq!(
            awc.flags as u64,
            file["flags"].as_u64().expect("flags"),
            "{path}: flags"
        );
        assert_eq!(
            awc.multi_channel,
            file["multiChannel"].as_u64().expect("multiChannel") != 0,
            "{path}: multi-channel flag"
        );
        assert_eq!(
            awc.whole_file_encrypted,
            file["wholeFileEncrypted"]
                .as_u64()
                .expect("wholeFileEncrypted")
                != 0,
            "{path}: whole-file encryption"
        );

        let expected = file["streams"].as_array().expect("streams");
        assert_eq!(awc.streams.len(), expected.len(), "{path}: stream count");
        if awc.multi_channel {
            multi_channel_files += 1;
        }

        for (i, (stream, want)) in awc.streams.iter().zip(expected).enumerate() {
            let at = format!("{path}[{i}]");

            assert_eq!(
                stream.id as u64,
                want["id"].as_u64().expect("id"),
                "{at}: id"
            );
            assert_eq!(
                codec_number(stream.format.map(|f| f.codec)),
                want["codec"].as_i64().expect("codec"),
                "{at}: codec"
            );
            assert_eq!(
                stream.format.map(|f| f.sample_rate).unwrap_or(0) as u64,
                want["sampleRate"].as_u64().expect("sampleRate"),
                "{at}: sample rate"
            );
            assert_eq!(
                stream.format.map(|f| f.samples).unwrap_or(0) as u64,
                want["samples"].as_u64().expect("samples"),
                "{at}: sample count"
            );
            assert_eq!(
                stream.midi.is_some(),
                want["isMidi"].as_u64().expect("isMidi") != 0,
                "{at}: midi"
            );
            assert_eq!(
                stream.chunks.len() as u64,
                want["chunkCount"].as_u64().expect("chunkCount"),
                "{at}: chunk count"
            );

            // The reference skips PCM for the multi-channel carrier and for
            // MIDI streams, so there is nothing to compare for those.
            let Some(want_hash) = want["pcmSha256"].as_str() else {
                continue;
            };

            let samples = stream.samples();
            assert_eq!(
                samples.len() * 2,
                want["pcmLength"].as_i64().expect("pcmLength") as usize,
                "{at}: decoded byte count"
            );
            assert_eq!(
                pcm_sha256(&samples),
                want_hash,
                "{at}: decoded PCM does not match the reference"
            );

            streams_checked += 1;
            samples_checked += samples.len();
        }
    }

    println!(
        "{} files ({multi_channel_files} multi-channel), {streams_checked} streams, \
         {samples_checked} samples decode identically to CodeWalker",
        files.len()
    );
}

/// Every `.awc` in the install has to open. This is the breadth check the
/// oracle sample cannot be: a container that panics or errors is a bug even if
/// nothing in it is playable.
#[test]
#[ignore = "needs a local GTA V install"]
fn opens_every_container_in_the_install() {
    let keys = load_keys();
    let manager = RpfManager::scan(&gta_path(), &keys).expect("scan the game folder");

    let mut paths = Vec::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if FileKind::from_name(&entry.name_lower).has_audio() {
                paths.push(entry.path.clone());
            }
        }
    }
    assert!(!paths.is_empty(), "the install should contain .awc files");

    let step = (paths.len() / 400).max(1);
    let sample: Vec<&String> = paths.iter().step_by(step).take(400).collect();

    let mut failures = Vec::new();
    let mut playable = 0usize;
    let mut seconds = 0.0f32;
    let mut adpcm = 0usize;
    let mut pcm = 0usize;

    for path in &sample {
        let entry_ref = manager.find(path).expect("indexed");
        let Ok(data) = manager.extract(entry_ref, &keys) else {
            failures.push(format!("{path}: extract failed"));
            continue;
        };
        match AwcFile::read(&data, &keys.awc_key) {
            Ok(awc) => {
                for stream in &awc.streams {
                    if !stream.is_playable() {
                        continue;
                    }
                    playable += 1;
                    seconds += stream.format.map(|f| f.seconds()).unwrap_or(0.0);
                    match stream.format.map(|f| f.codec) {
                        Some(Codec::Adpcm) => adpcm += 1,
                        Some(Codec::Pcm) => pcm += 1,
                        _ => {}
                    }
                    // Decoding must not panic, and a playable stream must yield
                    // samples.
                    assert!(
                        !stream.samples().is_empty(),
                        "{path}: a playable stream decoded to nothing"
                    );
                }
            }
            Err(e) => failures.push(format!("{path}: {e}")),
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} containers failed to open:\n{}",
        failures.len(),
        sample.len(),
        failures.join("\n")
    );
    assert!(
        playable > 100,
        "only {playable} playable streams in the sample"
    );

    println!(
        "{} containers, {playable} playable streams ({adpcm} ADPCM, {pcm} PCM), {:.0} seconds of audio",
        sample.len(),
        seconds
    );
}
