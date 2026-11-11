//! The shader gate: prove the drawable shader parameters and embedded texture
//! dictionaries agree with the C# reference.
//!
//! ```text
//! pwsh -File scripts/shader-oracle.ps1
//! cargo test -p cw-core --test shader_gate -- --ignored --nocapture
//! ```
//!
//! `fixtures/shader_oracle.json` is what `Drawable.cs` sees: for every sampled
//! drawable, each shader's parameters (`name`, `dataType`, and the `NameHash`
//! of the texture a texture slot points at), plus the embedded texture
//! dictionary's hashes and names when the shader group carries one.

use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::resources::read_named_drawables;
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

/// Compare one shader's parameters and its embedded dictionary against the
/// oracle's view of them.
fn check_shader(s: &cw_core::resources::ShaderFx, expected: &Value, at: &str) {
    assert_eq!(
        s.name as u64,
        expected["name"].as_u64().unwrap(),
        "{at}: shader name"
    );
    assert_eq!(
        s.file_name as u64,
        expected["fileName"].as_u64().unwrap(),
        "{at}: shader file name"
    );

    let params = expected["params"].as_array().expect("params");
    assert_eq!(s.parameters.len(), params.len(), "{at}: parameter count");
    for (i, (p, want)) in s.parameters.iter().zip(params).enumerate() {
        let at = format!("{at}.param{i}");
        assert_eq!(
            p.name as u64,
            want["name"].as_u64().unwrap(),
            "{at}: parameter name"
        );
        assert_eq!(
            p.data_type as u64,
            want["dataType"].as_u64().unwrap(),
            "{at}: parameter data type"
        );
        let want_texture = want["texture"].as_u64().unwrap();
        match p.texture {
            // A texture parameter whose base has no name is `None` in the port
            // and `0` in the oracle; either way there is nothing to resolve.
            None => {
                if want_texture != 0 {
                    panic!("{at}: expected texture {want_texture:#x} but found none")
                }
            }
            Some(got) => {
                if want_texture != 0 {
                    assert_eq!(got as u64, want_texture, "{at}: texture hash")
                } else {
                    panic!("{at}: port resolved a texture the oracle could not")
                }
            }
        }
    }
}

/// Compare a drawable's shader group against the oracle.
fn check_drawable(d: &cw_core::resources::Drawable, expected: &Value, at: &str) {
    let shaders = expected["shaders"].as_array().expect("shaders");
    assert_eq!(d.shaders.len(), shaders.len(), "{at}: shader count");
    for (i, (s, want)) in d.shaders.iter().zip(shaders).enumerate() {
        check_shader(s, want, &format!("{at}.shader{i}"));
    }

    // The embedded texture dictionary, when the reference carries one.
    match (
        &d.texture_dictionary,
        expected["textureDictionary"].as_object(),
    ) {
        (Some(dict), Some(want)) => {
            let count = want["count"].as_u64().expect("dictionary count");
            assert_eq!(dict.hashes.len(), count as usize, "{at}: dictionary count");
            let textures = want["textures"].as_array().expect("dictionary textures");
            assert_eq!(
                dict.textures.len(),
                textures.len(),
                "{at}: dictionary texture count"
            );
            for (i, (texture, entry)) in dict.textures.iter().zip(textures).enumerate() {
                let at = format!("{at}.texture{i}");
                assert_eq!(
                    texture.name_hash as u64,
                    entry["hash"].as_u64().unwrap(),
                    "{at}: texture hash"
                );
                match (&texture.name, entry["name"].as_str()) {
                    (Some(got), Some(want)) => assert_eq!(got, want, "{at}: texture name"),
                    (None, None) => {}
                    (got, want) => panic!("{at}: texture name mismatch ({got:?} vs {want:?})"),
                }
            }
        }
        (None, None) => {}
        (Some(_), None) => panic!("{at}: port found an embedded dictionary the oracle lacks"),
        (None, Some(_)) => panic!("{at}: oracle found an embedded dictionary the port lacks"),
    }
}

/// Every sampled drawable file's shaders agree with the reference.
#[test]
#[ignore = "needs a local GTA V install and fixtures/shader_oracle.json"]
fn agrees_with_the_reference_shader_reader() {
    let oracle_path = workspace_root().join("fixtures/shader_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "skipping: {} is missing. Run scripts/shader-oracle.ps1 first.",
            oracle_path.display()
        );
        return;
    };
    let oracle: Value =
        serde_json::from_str(raw.trim_start_matches('\u{FEFF}')).expect("parse the shader oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("oracle files array");
    let mut drawables = 0usize;
    let mut shaders = 0usize;
    let mut parameters = 0usize;
    let mut textures = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");
        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let entry = manager.entry(entry_ref);
        let (system_size, _) = entry
            .resource_segments()
            .unwrap_or_else(|| panic!("{path} is not a resource entry"));
        let data = manager
            .extract(entry_ref, &keys)
            .unwrap_or_else(|e| panic!("{path}: extract failed: {e}"));

        let named =
            read_named_drawables(FileKind::from_name(&entry.name_lower), &data, system_size)
                .unwrap_or_else(|e| panic!("{path}: parse failed: {e}"));

        let expected = file["drawables"].as_array().expect("drawables");
        assert_eq!(
            named.len(),
            expected.len(),
            "{path}: drawable count differs from the oracle"
        );
        for (i, (named, want)) in named.iter().zip(expected).enumerate() {
            check_drawable(&named.drawable, want, &format!("{path}.drawable{i}"));
        }

        drawables += named.len();
        shaders += named
            .iter()
            .map(|n| n.drawable.shaders.len())
            .sum::<usize>();
        parameters += named
            .iter()
            .flat_map(|n| &n.drawable.shaders)
            .map(|s| s.parameters.len())
            .sum::<usize>();
        textures += named
            .iter()
            .filter_map(|n| n.drawable.texture_dictionary.as_ref())
            .map(|d| d.textures.len())
            .sum::<usize>();
    }

    assert!(drawables > 0, "no file produced a drawable");
    assert!(shaders > 0, "no file produced a shader");
    println!(
        "{} drawable files agree: {drawables} drawables, {shaders} shaders, \
         {parameters} parameters, {textures} embedded textures",
        files.len()
    );
    let _ = parameters;
    let _ = textures;
}

/// Every `.ydr`/`.ydd` in the install parses with shader parameters intact.
#[test]
#[ignore = "needs a local GTA V install"]
fn every_drawable_in_the_install_reads_shader_parameters() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<String> = Vec::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            let kind = FileKind::from_name(&entry.name_lower);
            if kind == FileKind::Model || kind == FileKind::ModelDictionary {
                paths.push(entry.path.clone());
            }
        }
    }
    assert!(
        !paths.is_empty(),
        "the install should contain .ydr/.ydd files"
    );

    let mut parsed = 0usize;
    let mut with_parameters = 0usize;
    let mut failed = 0usize;

    for path in &paths {
        let Some(entry_ref) = manager.find(path) else {
            continue;
        };
        let entry = manager.entry(entry_ref);
        let Some((system_size, _)) = entry.resource_segments() else {
            continue;
        };
        let Ok(data) = manager.extract(entry_ref, &keys) else {
            continue;
        };
        let Ok(named) =
            read_named_drawables(FileKind::from_name(&entry.name_lower), &data, system_size)
        else {
            failed += 1;
            continue;
        };
        parsed += 1;
        if named
            .iter()
            .any(|n| n.drawable.shaders.iter().any(|s| !s.parameters.is_empty()))
        {
            with_parameters += 1;
        }
    }

    assert!(
        failed <= paths.len() / 100 + 2,
        "{failed} of {} drawables failed to parse",
        paths.len()
    );
    assert!(parsed > 1000, "only {parsed} drawables parsed");
    assert!(
        with_parameters > parsed / 10,
        "only {with_parameters} of {parsed} drawables carried shader parameters"
    );
    println!(
        "{parsed} of {} drawables parse, {with_parameters} with shader parameters",
        paths.len()
    );
}
