//! The fragment gate: prove the resource layer reads real `.yft` files and
//! agrees with the C# reference block by block.
//!
//! ```text
//! pwsh -File scripts/frag-oracle.ps1
//! cargo test -p cw-core --test frag_gate -- --ignored --nocapture
//! ```
//!
//! `fixtures/frag_oracle.json` is what `YftFile` in `CodeWalker.Core.dll` sees:
//! the fragment header, the main drawable, the cloth, the alternative drawable
//! array and the whole physics LOD tree with every child's drawables.

use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::resources::{read_fragments, FragType};
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

/// Read one entry and parse its fragment.
fn frag_at(manager: &RpfManager, keys: &GtaKeys, path: &str) -> FragType {
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

    read_fragments(FileKind::from_name(&entry.name_lower), &data, system_size)
        .unwrap_or_else(|e| panic!("{path}: parse failed: {e}"))
}

/// Same as [`frag_at`], but reports failure as `None` instead of panicking.
fn try_frag_at(manager: &RpfManager, keys: &GtaKeys, path: &str) -> Option<FragType> {
    let entry_ref = manager.find(path)?;
    let entry = manager.entry(entry_ref);
    let (system_size, _) = entry.resource_segments()?;
    let data = manager.extract(entry_ref, keys).ok()?;
    read_fragments(FileKind::from_name(&entry.name_lower), &data, system_size).ok()
}

fn f32_at(value: &Value) -> f32 {
    value.as_f64().expect("a number") as f32
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
}

/// Compare a geometry's identifying fields against the oracle's view of it.
fn check_geometry(g: &cw_core::resources::DrawableGeometry, expected: &Value, at: &str) {
    assert_eq!(
        g.shader_id as u64,
        expected["shaderId"].as_u64().unwrap(),
        "{at}: shader id"
    );
    assert_eq!(
        g.vertex_count as u64,
        expected["verticesCount"].as_u64().unwrap(),
        "{at}: vertex count"
    );
    assert_eq!(
        g.indices.len() as u64,
        expected["indicesCount"].as_u64().unwrap(),
        "{at}: index count"
    );
    assert_eq!(
        g.vertex_stride as u64,
        expected["vertexStride"].as_u64().unwrap(),
        "{at}: vertex stride"
    );
    if let Some(declaration) = &g.declaration {
        assert_eq!(
            declaration.flags as u64,
            expected["declFlags"].as_u64().unwrap(),
            "{at}: declaration flags"
        );
        assert_eq!(
            declaration.types,
            expected["declTypes"].as_u64().unwrap(),
            "{at}: declaration types"
        );
        assert_eq!(
            declaration.stride as u64,
            expected["declStride"].as_u64().unwrap(),
            "{at}: declaration stride"
        );
    }
}

fn check_model(m: &cw_core::resources::DrawableModel, expected: &Value, at: &str) {
    assert_eq!(
        m.bone_index as u64,
        expected["boneIndex"].as_u64().unwrap(),
        "{at}: bone index"
    );
    assert_eq!(
        m.has_skin as u64,
        expected["hasSkin"].as_u64().unwrap(),
        "{at}: has skin"
    );
    let geoms = expected["geometries"].as_array().expect("geometries");
    assert_eq!(m.geometries.len(), geoms.len(), "{at}: geometry count");
    for (i, (g, want)) in m.geometries.iter().zip(geoms).enumerate() {
        check_geometry(g, want, &format!("{at}.geometry{i}"));
    }
}

/// Compare a drawable's LOD models and shaders against the oracle.
fn check_drawable(d: &cw_core::resources::Drawable, expected: &Value, at: &str) {
    if let Some(name) = &d.name {
        assert_eq!(name, expected["name"].as_str().unwrap_or(""), "{at}: name");
    }
    assert!(
        close(d.bounding_sphere_radius, f32_at(&expected["radius"])),
        "{at}: sphere radius"
    );
    for (axis, (got, want)) in d
        .bounding_box_min
        .iter()
        .zip(expected["bbMin"].as_array().expect("bbMin"))
        .enumerate()
    {
        assert!(close(*got, f32_at(want)), "{at}: bbMin[{axis}]");
    }
    for (axis, (got, want)) in d
        .bounding_box_max
        .iter()
        .zip(expected["bbMax"].as_array().expect("bbMax"))
        .enumerate()
    {
        assert!(close(*got, f32_at(want)), "{at}: bbMax[{axis}]");
    }

    let shaders = expected["shaders"].as_array().expect("shaders");
    assert_eq!(d.shaders.len(), shaders.len(), "{at}: shader count");
    for (i, (s, want)) in d.shaders.iter().zip(shaders).enumerate() {
        assert_eq!(
            s.name as u64,
            want["name"].as_u64().unwrap(),
            "{at}: shader {i} name"
        );
        assert_eq!(
            s.file_name as u64,
            want["fileName"].as_u64().unwrap(),
            "{at}: shader {i} file name"
        );
    }

    let models = expected["models"].as_array().expect("models");
    let got_models: Vec<_> = cw_core::resources::Lod::ALL
        .iter()
        .zip(&d.models)
        .flat_map(|(lod, list)| list.iter().map(move |m| (lod, m)))
        .collect();
    assert_eq!(got_models.len(), models.len(), "{at}: LOD model count");
    for (i, ((lod, m), want)) in got_models.iter().zip(models).enumerate() {
        // The oracle names the LODs `High`/`Med`/`Low`/`VLow`; map back to the
        // enum rather than comparing display strings.
        let want_lod = match want["lod"].as_str().expect("lod") {
            "High" => cw_core::resources::Lod::High,
            "Med" => cw_core::resources::Lod::Medium,
            "Low" => cw_core::resources::Lod::Low,
            "VLow" => cw_core::resources::Lod::VeryLow,
            other => panic!("{at}: unknown LOD name {other}"),
        };
        assert_eq!(**lod, want_lod, "{at}: model {i} lod");
        check_model(m, want, &format!("{at}.model{i}"));
    }
}

fn check_frag_drawable(fd: &cw_core::resources::FragDrawable, expected: &Value, at: &str) {
    if let Some(d) = expected["drawable"].as_object() {
        check_drawable(&fd.drawable, &Value::Object(d.clone()), at);
    }

    let want = expected["drawable"]["fragMatrix"]
        .as_array()
        .expect("fragMatrix");
    let columns = [
        fd.frag_matrix.column1,
        fd.frag_matrix.column2,
        fd.frag_matrix.column3,
        fd.frag_matrix.column4,
    ];
    for (ci, column) in columns.iter().enumerate() {
        let want_col = &want[ci * 4..ci * 4 + 4];
        for (axis, want) in want_col.iter().enumerate() {
            if axis == 3 {
                assert_eq!(
                    column[axis].to_bits() as u64,
                    want.as_u64().expect("flag"),
                    "{at}: fragMatrix column{ci} flag"
                );
            } else {
                assert!(
                    close(column[axis], f32_at(want)),
                    "{at}: fragMatrix column{ci} axis{axis}: {} != {}",
                    column[axis],
                    f32_at(want)
                );
            }
        }
    }

    let bound_type = expected["boundType"].as_str();
    match (&fd.bound, bound_type) {
        (Some(_), None) => panic!("{at}: expected no bound but got one"),
        (None, Some(_)) => panic!("{at}: expected a bound but got none"),
        (Some(b), Some(t)) => {
            assert_eq!(b.bounds_type.as_str(), t, "{at}: bound type");
        }
        _ => {}
    }
}

fn check_lod(lod: &cw_core::resources::FragPhysicsLOD, expected: &Value, at: &str) {
    let children = expected["children"].as_array().expect("children");
    assert_eq!(lod.children.len(), children.len(), "{at}: child count");

    let groups = expected["groups"].as_array().expect("groups");
    assert_eq!(lod.groups.len(), groups.len(), "{at}: group count");

    for (i, (child, want)) in lod.children.iter().zip(children).enumerate() {
        let at = format!("{at}.child{i}");
        assert!(
            close(child.pristine_mass, f32_at(&want["pristineMass"])),
            "{at}: pristine mass"
        );
        assert!(
            close(child.damaged_mass, f32_at(&want["damagedMass"])),
            "{at}: damaged mass"
        );
        assert_eq!(
            child.group_index as u64,
            want["groupIndex"].as_u64().unwrap(),
            "{at}: group index"
        );
        assert_eq!(
            child.bone_tag as u64,
            want["boneTag"].as_u64().unwrap(),
            "{at}: bone tag"
        );
        match &child.drawable1 {
            Some(d) => check_frag_drawable(
                d,
                &Value::Object(want["drawable1"].as_object().expect("drawable1").clone()),
                &format!("{at}.drawable1"),
            ),
            None => assert!(want["drawable1"].is_null(), "{at}: expected drawable1"),
        }
        match &child.drawable2 {
            Some(d) => check_frag_drawable(
                d,
                &Value::Object(want["drawable2"].as_object().expect("drawable2").clone()),
                &format!("{at}.drawable2"),
            ),
            None => assert!(want["drawable2"].is_null(), "{at}: expected drawable2"),
        }
    }

    for (i, (group, want)) in lod.groups.iter().zip(groups).enumerate() {
        let at = format!("{at}.group{i}");
        assert!(
            close(group.strength, f32_at(&want["strength"])),
            "{at}: strength"
        );
        assert_eq!(
            group.child_group_index as u64,
            want["childGroupIndex"].as_u64().unwrap(),
            "{at}: child group index"
        );
        assert_eq!(
            group.glass_window_index as u64,
            want["glassWindowIndex"].as_u64().unwrap(),
            "{at}: glass window index"
        );
        match (&group.name, want["name"].as_str()) {
            (Some(got), Some(want)) => assert_eq!(got, want, "{at}: group name"),
            (None, None) => {}
            (got, want) => panic!("{at}: group name mismatch ({got:?} vs {want:?})"),
        }
    }

    let bound_type = expected["boundType"].as_str();
    match (&lod.bound, bound_type) {
        (Some(_), None) => panic!("{at}: expected no bound but got one"),
        (None, Some(_)) => panic!("{at}: expected a bound but got none"),
        (Some(b), Some(t)) => assert_eq!(b.bounds_type.as_str(), t, "{at}: bound type"),
        _ => {}
    }
}

fn check_fragment(f: &FragType, expected: &Value, at: &str) {
    match (&f.name, expected["name"].as_str()) {
        (Some(got), Some(want)) => assert_eq!(got, want, "{at}: name"),
        (None, None) => {}
        (got, want) => panic!("{at}: name mismatch ({got:?} vs {want:?})"),
    }
    assert!(
        close(f.bounding_sphere_radius, f32_at(&expected["radius"])),
        "{at}: sphere radius"
    );
    for (axis, (got, want)) in f
        .bounding_sphere_center
        .iter()
        .zip(expected["center"].as_array().expect("center"))
        .enumerate()
    {
        assert!(close(*got, f32_at(want)), "{at}: center[{axis}]");
    }

    match (&f.drawable, expected["drawable"].as_object()) {
        (Some(d), Some(want)) => {
            check_frag_drawable(d, &Value::Object(want.clone()), &format!("{at}.drawable"))
        }
        (None, None) => {}
        (got, want) => panic!("{at}: drawable mismatch ({got:?} vs {want:?})"),
    }
    match (&f.drawable_cloth, expected["drawableCloth"].as_object()) {
        (Some(d), Some(want)) => check_frag_drawable(
            d,
            &Value::Object(want.clone()),
            &format!("{at}.drawableCloth"),
        ),
        (None, None) => {}
        (got, want) => panic!("{at}: cloth mismatch ({got:?} vs {want:?})"),
    }

    let arr = expected["drawableArray"].as_array().expect("drawableArray");
    assert_eq!(f.drawable_array.len(), arr.len(), "{at}: array length");
    for (i, (d, want)) in f.drawable_array.iter().zip(arr).enumerate() {
        check_frag_drawable(
            d,
            &Value::Object(want.as_object().expect("array item").clone()),
            &format!("{at}.drawableArray[{i}]"),
        );
    }

    let names = expected["drawableArrayNames"]
        .as_array()
        .expect("drawableArrayNames");
    assert_eq!(
        f.drawable_array_names.len(),
        names.len(),
        "{at}: array name count"
    );
    for (i, (got, want)) in f.drawable_array_names.iter().zip(names).enumerate() {
        assert_eq!(got.as_deref(), want.as_str(), "{at}: array name {i}");
    }

    let lodg = &expected["physicsLODGroup"];
    match (&f.physics_lod_group, lodg.as_object()) {
        (Some(g), Some(want)) => {
            for (name, lod) in [("lod1", &g.lod1), ("lod2", &g.lod2), ("lod3", &g.lod3)] {
                let at = format!("{at}.{name}");
                match (lod, want[name].as_object()) {
                    (Some(lod), Some(want)) => check_lod(lod, &Value::Object(want.clone()), &at),
                    (None, None) => {}
                    (got, want) => panic!("{at}: LOD mismatch ({got:?} vs {want:?})"),
                }
            }
        }
        (None, None) => {}
        (got, want) => panic!("{at}: physics LOD group mismatch ({got:?} vs {want:?})"),
    }
}

/// Every sampled `.yft` parses and agrees with the reference fragment by
/// fragment.
#[test]
#[ignore = "needs a local GTA V install and fixtures/frag_oracle.json"]
fn agrees_with_the_reference_fragment_reader() {
    let oracle_path = workspace_root().join("fixtures/frag_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "skipping: {} is missing. Run scripts/frag-oracle.ps1 first.",
            oracle_path.display()
        );
        return;
    };
    let oracle: Value = serde_json::from_str(raw.trim_start_matches('\u{FEFF}'))
        .expect("parse the fragment oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("oracle files array");
    let mut drawables = 0usize;
    let mut geometry = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");
        let fragment = frag_at(&manager, &keys, path);
        check_fragment(&fragment, &file["fragment"], path);

        let all = fragment.all_drawables();
        drawables += all.len();
        geometry += all
            .iter()
            .flat_map(|d| &d.drawable.models)
            .flatten()
            .map(|m| m.geometries.len())
            .sum::<usize>();
    }

    assert!(drawables > 0, "no fragment produced a drawable");
    assert!(geometry > 0, "no fragment produced geometry");
    println!(
        "{} yft files agree, {drawables} drawables, {geometry} geometries",
        files.len()
    );
}

/// Every `.yft` in the install parses, and the embedded drawables resolve.
#[test]
#[ignore = "needs a local GTA V install"]
fn every_fragment_in_the_install_parses() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<String> = Vec::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if FileKind::from_name(&entry.name_lower) == FileKind::Fragment {
                paths.push(entry.path.clone());
            }
        }
    }
    assert!(!paths.is_empty(), "the install should contain .yft files");

    let mut parsed = 0usize;
    let mut drawables = 0usize;
    let mut geometry = 0usize;
    let mut failed = 0usize;

    for path in &paths {
        let Some(fragment) = try_frag_at(&manager, &keys, path) else {
            failed += 1;
            continue;
        };
        parsed += 1;
        drawables += fragment.all_drawables().len();
        geometry += fragment
            .all_drawables()
            .iter()
            .flat_map(|d| &d.drawable.models)
            .flatten()
            .map(|m| m.geometries.len())
            .sum::<usize>();
    }

    assert!(
        failed <= paths.len() / 100 + 2,
        "{failed} of {} fragments failed to parse",
        paths.len()
    );
    assert!(parsed > 1000, "only {parsed} fragments parsed");
    println!(
        "{parsed} of {} fragments parse, {drawables} embedded drawables, {geometry} geometries",
        paths.len()
    );
}
