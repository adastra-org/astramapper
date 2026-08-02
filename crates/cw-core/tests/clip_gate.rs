//! The clip gate: prove the `.ycd` reader agrees with the C# reference clip by
//! clip and animation by animation.
//!
//! ```text
//! pwsh -File scripts/clip-oracle.ps1
//! cargo test -p cw-core --test clip_gate -- --ignored --nocapture
//! ```
//!
//! Two checks:
//!
//! 1. **Against the oracle.** `fixtures/clip_oracle.json` is what `YcdFile` in
//!    `CodeWalker.Core.dll` sees. Both maps are hash tables, so entries are
//!    matched by hash rather than by position — order is a property of the
//!    bucket walk, not of the format.
//! 2. **Structurally, over a wide sample.** Every `.ycd` in the install must
//!    read, and what it reads must hold together: the clip count must match the
//!    header's own `ClipsMapEntries`, and every animation a clip names must be
//!    in the animation map. The second is the check that matters, because the
//!    hash that links the two is written by the map entry rather than stored in
//!    the animation block — see the note in `resources/clip.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::resources::{read_clips, ClipType};
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use serde_json::Value;

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

fn scan(keys: &GtaKeys) -> RpfManager {
    RpfManager::scan(&gta_path(), keys).expect("scan the game folder")
}

fn f32_at(value: &Value) -> f32 {
    value.as_f64().unwrap_or(0.0) as f32
}

/// The oracle serializes `float` through .NET's round-trip formatting and this
/// reads it back through `f64`, so the two agree to within a float's last bit,
/// not exactly.
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
}

#[test]
#[ignore = "needs fixtures/clip_oracle.json — see scripts/clip-oracle.ps1"]
fn agrees_with_the_reference_clip_reader() {
    let oracle_path = workspace_root().join("fixtures/clip_oracle.json");
    let Ok(text) = std::fs::read_to_string(&oracle_path) else {
        println!("skipping: {} is missing", oracle_path.display());
        return;
    };
    let oracle: Value = serde_json::from_str(&text).expect("parse the oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("files array");
    assert!(!files.is_empty(), "the oracle holds no files");

    let mut clips_checked = 0usize;
    let mut animations_checked = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");
        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let entry = manager.entry(entry_ref);
        let (system_size, _) = entry
            .resource_segments()
            .unwrap_or_else(|| panic!("{path} is not a resource entry"));
        assert_eq!(
            system_size as i64,
            file["systemSize"].as_i64().unwrap(),
            "{path}: system size"
        );

        let data = manager
            .extract(entry_ref, &keys)
            .unwrap_or_else(|e| panic!("{path}: extract failed: {e}"));
        let dictionary = read_clips(FileKind::ClipDictionary, &data, system_size)
            .unwrap_or_else(|e| panic!("{path}: {e}"));

        let by_hash: HashMap<u32, &cw_core::resources::Clip> =
            dictionary.clips.iter().map(|c| (c.hash, c)).collect();
        let animations: HashMap<u32, &cw_core::resources::Animation> =
            dictionary.animations.iter().map(|a| (a.hash, a)).collect();

        for expected in file["clips"].as_array().expect("clips") {
            let hash = expected["hash"].as_u64().unwrap() as u32;
            let clip = by_hash
                .get(&hash)
                .unwrap_or_else(|| panic!("{path}: clip {hash:#010X} is missing"));
            clips_checked += 1;

            let kind = match expected["type"].as_i64().unwrap() {
                1 => ClipType::Animation,
                2 => ClipType::AnimationList,
                other => ClipType::Unknown(other as u32),
            };
            assert_eq!(clip.clip_type, kind, "{path}: {hash:#010X} type");
            assert_eq!(
                clip.name.as_deref().unwrap_or(""),
                expected["name"].as_str().unwrap_or(""),
                "{path}: {hash:#010X} name"
            );
            assert_eq!(
                clip.short_name.as_deref().unwrap_or(""),
                expected["shortName"].as_str().unwrap_or(""),
                "{path}: {hash:#010X} short name"
            );
            for (label, got, want) in [
                ("start", clip.start_time, f32_at(&expected["startTime"])),
                ("end", clip.end_time, f32_at(&expected["endTime"])),
                ("rate", clip.rate, f32_at(&expected["rate"])),
                ("duration", clip.duration, f32_at(&expected["duration"])),
            ] {
                assert!(
                    close(got, want),
                    "{path}: {hash:#010X} {label}: {got} != {want}"
                );
            }
            assert_eq!(
                clip.tag_count as i64,
                expected["tagCount"].as_i64().unwrap(),
                "{path}: {hash:#010X} tag count"
            );
            assert_eq!(
                clip.property_count as i64,
                expected["propertyCount"].as_i64().unwrap(),
                "{path}: {hash:#010X} property count"
            );

            let want: Vec<u32> = expected["animations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u32)
                .collect();
            assert_eq!(
                clip.animations, want,
                "{path}: {hash:#010X} animation hashes"
            );
        }

        for expected in file["animations"].as_array().expect("animations") {
            let hash = expected["hash"].as_u64().unwrap() as u32;
            let animation = animations
                .get(&hash)
                .unwrap_or_else(|| panic!("{path}: animation {hash:#010X} is missing"));
            animations_checked += 1;

            assert_eq!(
                animation.frames as i64,
                expected["frames"].as_i64().unwrap(),
                "{path}: {hash:#010X} frames"
            );
            assert_eq!(
                animation.sequence_frame_limit as i64,
                expected["sequenceFrameLimit"].as_i64().unwrap(),
                "{path}: {hash:#010X} sequence frame limit"
            );
            assert!(
                close(animation.duration, f32_at(&expected["duration"])),
                "{path}: {hash:#010X} duration"
            );
            assert_eq!(
                animation.usage_count as i64,
                expected["usageCount"].as_i64().unwrap(),
                "{path}: {hash:#010X} usage count"
            );

            let want_frames: Vec<u16> = expected["sequenceFrames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap() as u16)
                .collect();
            assert_eq!(
                animation.sequence_frames, want_frames,
                "{path}: {hash:#010X} sequence frames"
            );

            let want_bones = expected["boneIds"].as_array().unwrap();
            assert_eq!(
                animation.bone_ids.len(),
                want_bones.len(),
                "{path}: {hash:#010X} bone id count"
            );
            for (got, want) in animation.bone_ids.iter().zip(want_bones) {
                assert_eq!(got.bone_id as i64, want["boneId"].as_i64().unwrap());
                assert_eq!(got.track as i64, want["track"].as_i64().unwrap());
                assert_eq!(got.unknown as i64, want["unk0"].as_i64().unwrap());
            }
        }
    }

    println!(
        "{} dictionaries, {clips_checked} clips and {animations_checked} animations match the reference",
        files.len()
    );
}

/// Every `.ycd` in the install, read for what it says about itself.
#[test]
#[ignore = "needs a local GTA V install"]
fn reads_every_clip_dictionary_in_the_install() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<(cw_core::rpf::EntryRef, String)> = Vec::new();
    for (index, archive) in manager.archives.iter().enumerate() {
        for (entry_index, entry) in archive.entries.iter().enumerate() {
            if FileKind::from_name(&entry.name_lower).has_clips() {
                paths.push((
                    cw_core::rpf::EntryRef {
                        archive: index,
                        entry: entry_index,
                    },
                    entry.path.clone(),
                ));
            }
        }
    }
    assert!(!paths.is_empty(), "the install should contain .ycd files");

    let mut files = 0usize;
    let mut clips = 0usize;
    let mut animations = 0usize;
    let mut unresolved = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut truncated: Vec<String> = Vec::new();

    for (entry_ref, path) in &paths {
        let entry = manager.entry(*entry_ref);
        let Some((system_size, _)) = entry.resource_segments() else {
            continue;
        };
        let Ok(data) = manager.extract(*entry_ref, &keys) else {
            failures.push(format!("{path}: extract failed"));
            continue;
        };
        let dictionary = match read_clips(FileKind::ClipDictionary, &data, system_size) {
            Ok(dictionary) => dictionary,
            // A payload that did not inflate to the size its page flags claim
            // is the one accepted failure, and it is the same handful of
            // destruction archives that break the `.ytd` reader — see the note
            // in `docs/formats.md`. The reference throws on these too; what
            // matters is that nothing *else* fails.
            Err(e) if data.len() < system_size => {
                truncated.push(format!(
                    "{path}: {} of {system_size} bytes ({e})",
                    data.len()
                ));
                continue;
            }
            Err(e) => {
                failures.push(format!("{path}: {e}"));
                continue;
            }
        };

        files += 1;
        clips += dictionary.clips.len();
        animations += dictionary.animations.len();

        assert_eq!(
            dictionary.clips.len(),
            dictionary.declared_clips as usize,
            "{path}: walked {} clips, header declares {}",
            dictionary.clips.len(),
            dictionary.declared_clips
        );
        assert_eq!(
            dictionary.animations.len(),
            dictionary.declared_animations as usize,
            "{path}: walked {} animations, header declares {}",
            dictionary.animations.len(),
            dictionary.declared_animations
        );

        let known: std::collections::HashSet<u32> =
            dictionary.animations.iter().map(|a| a.hash).collect();
        for clip in &dictionary.clips {
            for hash in &clip.animations {
                if !known.contains(hash) {
                    unresolved += 1;
                }
            }
        }
    }

    assert!(files > 100, "sample too small to mean anything: {files}");
    assert!(failures.is_empty(), "{} failed: {:?}", failures.len(), {
        failures.truncate(10);
        failures
    });
    assert_eq!(
        unresolved, 0,
        "{unresolved} clip animation pointers did not resolve to a map entry"
    );

    println!("{files} clip dictionaries, {clips} clips, {animations} animations, all resolved");
    if !truncated.is_empty() {
        println!(
            "{} refused as truncated payloads:\n  {}",
            truncated.len(),
            truncated.join("\n  ")
        );
    }
}
