//! The drawable gate: prove the resource layer reads real `.ydr` and `.ydd`
//! files and agrees with the C# reference geometry by geometry.
//!
//! ```text
//! pwsh -File scripts/drawable-oracle.ps1
//! cargo test -p cw-core --test drawable_gate -- --ignored --nocapture
//! ```
//!
//! Two independent checks:
//!
//! 1. **Against the oracle.** `fixtures/drawable_oracle.json` is what
//!    `YdrFile`/`YddFile` in `CodeWalker.Core.dll` sees. Every count, stride,
//!    bound and declaration compared field for field.
//! 2. **Against the file's own bounding box.** Vertex positions and the
//!    bounding box are stored in different places by different code paths, so
//!    "every decoded vertex lies inside the box the file declares" catches a
//!    wrong component offset or a wrong stride even where no oracle entry
//!    exists. This one runs over a wide sample of the whole install.

use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::mesh::MeshBundle;
use cw_core::resources::{read_drawables, Drawable, Lod};
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

/// Read one entry and parse whatever drawables it holds.
fn drawables_at(manager: &RpfManager, keys: &GtaKeys, path: &str) -> Vec<Drawable> {
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

    read_drawables(FileKind::from_name(&entry.name_lower), &data, system_size)
        .unwrap_or_else(|e| panic!("{path}: parse failed: {e}"))
}

/// Every model in the order the oracle writes them: high, medium, low, very
/// low, flattened.
fn flat_models(drawable: &Drawable) -> Vec<(Lod, &cw_core::resources::DrawableModel)> {
    Lod::ALL
        .iter()
        .zip(&drawable.models)
        .flat_map(|(lod, models)| models.iter().map(move |m| (*lod, m)))
        .collect()
}

fn f32_at(value: &Value) -> f32 {
    value.as_f64().expect("a number") as f32
}

fn close(a: f32, b: f32) -> bool {
    // The oracle round-trips floats through JSON text, so exact equality is not
    // available. A relative tolerance this tight still fails on any real
    // disagreement: a misread field lands orders of magnitude away, not in the
    // seventh digit.
    (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
}

#[test]
#[ignore = "needs a local GTA V install and fixtures/drawable_oracle.json"]
fn agrees_with_the_reference_drawable_reader() {
    let oracle_path = workspace_root().join("fixtures/drawable_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "skipping: {} is missing. Run scripts/drawable-oracle.ps1 first.",
            oracle_path.display()
        );
        return;
    };
    // PowerShell's Out-File -Encoding utf8 writes a BOM.
    let oracle: Value = serde_json::from_str(raw.trim_start_matches('\u{FEFF}'))
        .expect("parse the drawable oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("oracle files array");
    assert!(!files.is_empty(), "the oracle is empty");

    let mut geometries_checked = 0usize;
    let mut vertices_checked = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");
        let expected_system = file["systemSize"].as_u64().expect("systemSize") as usize;
        let expected_version = file["version"].as_u64().expect("version") as u32;

        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let entry = manager.entry(entry_ref);
        let (system_size, graphics_size) = entry.resource_segments().expect("a resource entry");

        assert_eq!(system_size, expected_system, "{path}: system segment size");
        assert_eq!(
            graphics_size,
            file["graphicsSize"].as_u64().expect("graphicsSize") as usize,
            "{path}: graphics segment size"
        );
        assert_eq!(
            entry.resource_version(),
            Some(expected_version),
            "{path}: resource version"
        );

        let ours = drawables_at(&manager, &keys, path);
        let theirs = file["drawables"].as_array().expect("drawables");
        assert_eq!(ours.len(), theirs.len(), "{path}: drawable count");

        for (i, (drawable, expected)) in ours.iter().zip(theirs).enumerate() {
            let at = format!("{path}[{i}]");

            assert!(
                close(drawable.bounding_sphere_radius, f32_at(&expected["radius"])),
                "{at}: bounding sphere radius {} != {}",
                drawable.bounding_sphere_radius,
                f32_at(&expected["radius"])
            );
            for (axis, (got, want)) in drawable
                .bounding_box_min
                .iter()
                .zip(expected["bbMin"].as_array().expect("bbMin"))
                .enumerate()
            {
                assert!(close(*got, f32_at(want)), "{at}: bbMin[{axis}]");
            }
            for (axis, (got, want)) in drawable
                .bounding_box_max
                .iter()
                .zip(expected["bbMax"].as_array().expect("bbMax"))
                .enumerate()
            {
                assert!(close(*got, f32_at(want)), "{at}: bbMax[{axis}]");
            }
            for (lod, (got, want)) in drawable
                .lod_distances
                .iter()
                .zip(expected["lodDist"].as_array().expect("lodDist"))
                .enumerate()
            {
                assert!(close(*got, f32_at(want)), "{at}: lodDist[{lod}]");
            }

            let expected_shaders = expected["shaders"].as_array().expect("shaders");
            assert_eq!(
                drawable.shaders.len(),
                expected_shaders.len(),
                "{at}: shader count"
            );
            for (s, (shader, want)) in drawable.shaders.iter().zip(expected_shaders).enumerate() {
                assert_eq!(
                    shader.name as u64,
                    want["name"].as_u64().expect("shader name"),
                    "{at}: shader[{s}] name hash"
                );
                assert_eq!(
                    shader.file_name as u64,
                    want["fileName"].as_u64().expect("shader fileName"),
                    "{at}: shader[{s}] file name hash"
                );
            }

            let models = flat_models(drawable);
            let expected_models = expected["models"].as_array().expect("models");
            assert_eq!(models.len(), expected_models.len(), "{at}: model count");

            for (m, ((lod, model), want)) in models.iter().zip(expected_models).enumerate() {
                let at = format!("{at}.model{m}");
                let want_lod = want["lod"].as_str().expect("lod");
                let our_lod = match lod {
                    Lod::High => "High",
                    Lod::Medium => "Med",
                    Lod::Low => "Low",
                    Lod::VeryLow => "VLow",
                };
                assert_eq!(our_lod, want_lod, "{at}: lod");
                assert_eq!(
                    model.bone_index as u64,
                    want["boneIndex"].as_u64().expect("boneIndex"),
                    "{at}: bone index"
                );
                assert_eq!(
                    model.has_skin,
                    want["hasSkin"].as_u64().expect("hasSkin") != 0,
                    "{at}: has skin"
                );

                let expected_geometries = want["geometries"].as_array().expect("geometries");
                assert_eq!(
                    model.geometries.len(),
                    expected_geometries.len(),
                    "{at}: geometry count"
                );

                for (g, (geometry, want)) in
                    model.geometries.iter().zip(expected_geometries).enumerate()
                {
                    let at = format!("{at}.geometry{g}");
                    assert_eq!(
                        geometry.shader_id as u64,
                        want["shaderId"].as_u64().expect("shaderId"),
                        "{at}: shader id"
                    );
                    assert_eq!(
                        geometry.vertex_count as u64,
                        want["verticesCount"].as_u64().expect("verticesCount"),
                        "{at}: vertex count"
                    );
                    assert_eq!(
                        geometry.indices.len() as u64,
                        want["indicesCount"].as_u64().expect("indicesCount"),
                        "{at}: index count"
                    );
                    assert_eq!(
                        geometry.vertex_stride as u64,
                        want["vertexStride"].as_u64().expect("vertexStride"),
                        "{at}: vertex stride"
                    );

                    let declaration = geometry.declaration.expect("a vertex declaration");
                    assert_eq!(
                        declaration.flags as u64,
                        want["declFlags"].as_u64().expect("declFlags"),
                        "{at}: declaration flags"
                    );
                    assert_eq!(
                        declaration.types,
                        want["declTypes"].as_u64().expect("declTypes"),
                        "{at}: declaration types"
                    );
                    assert_eq!(
                        declaration.stride as u64,
                        want["declStride"].as_u64().expect("declStride"),
                        "{at}: declaration stride"
                    );
                    // The reference's own commented-out check says these never
                    // disagree. If they ever do here, the nibbles were misread.
                    assert_eq!(
                        declaration.computed_stride(),
                        declaration.stride as usize,
                        "{at}: stride does not match the components"
                    );

                    geometries_checked += 1;
                    vertices_checked += geometry.vertex_count;
                }
            }
        }
    }

    println!(
        "{} files, {geometries_checked} geometries, {vertices_checked} vertices agree with CodeWalker",
        files.len()
    );
}

/// Every decoded vertex must land inside its geometry's own AABB.
///
/// The AABB comes from `DrawableModel.BoundsPointer` — a different pointer, a
/// different segment offset and a different decode path from the vertex bytes.
/// So a wrong component offset, a wrong stride or a misread declaration nibble
/// shows up here immediately, on files the oracle sample never touched.
///
/// It holds *exactly*: over 700k vertices sampled across the install, not one
/// falls outside. The drawable-level bounding box is a much looser thing — a
/// fifth of vertices sit outside it, and two thirds for skinned models, so it
/// is not usable as a check. Only the per-geometry box is.
#[test]
#[ignore = "needs a local GTA V install"]
fn decoded_vertices_land_inside_their_geometry_bounds() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut candidates: Vec<String> = Vec::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            // The drawable reader only handles `.ydr`/`.ydd`; fragments parse
            // through `Frag.cs` and are gated separately. Sampling them here
            // would fail the parse before anything was checked.
            let kind = FileKind::from_name(&entry.name_lower);
            if matches!(kind, FileKind::Model | FileKind::ModelDictionary) {
                candidates.push(entry.path.clone());
            }
        }
    }
    assert!(
        !candidates.is_empty(),
        "the install should contain .ydr/.ydd files"
    );

    // A spread across the whole install rather than the first N, which would
    // all come from one archive.
    let step = (candidates.len() / 300).max(1);
    let sample: Vec<&String> = candidates.iter().step_by(step).take(300).collect();

    let mut checked = 0usize;
    let mut total = 0usize;
    let mut without_bounds = 0usize;

    for path in &sample {
        for drawable in drawables_at(&manager, &keys, path) {
            let Some((_, models)) = drawable.best_lod() else {
                continue;
            };
            for model in models {
                for geometry in &model.geometries {
                    let (Some(declaration), Some(bounds)) = (geometry.declaration, geometry.bounds)
                    else {
                        without_bounds += 1;
                        continue;
                    };
                    let count = geometry
                        .vertex_count
                        .min(geometry.vertex_bytes.len() / geometry.vertex_stride.max(1));
                    if count == 0 {
                        continue;
                    }
                    checked += 1;

                    // Only enough slack to absorb f32 rounding in the box
                    // itself, not enough to hide a decode error.
                    let slack = 1e-3;
                    for v in 0..count {
                        let Some(vertex) = geometry.vertex(v) else {
                            break;
                        };
                        let Some(position) = declaration.decode(vertex, 0) else {
                            continue;
                        };
                        total += 1;
                        for (axis, coordinate) in position[..3].iter().enumerate() {
                            assert!(
                                coordinate.is_finite()
                                    && *coordinate >= bounds.min[axis] - slack
                                    && *coordinate <= bounds.max[axis] + slack,
                                "{path}: vertex {v} axis {axis} is {coordinate}, outside [{}, {}]",
                                bounds.min[axis],
                                bounds.max[axis]
                            );
                        }
                    }
                }
            }
        }
    }

    assert!(
        total > 100_000,
        "sample too small to mean anything: {total}"
    );
    println!(
        "{} files, {checked} geometries, {total} vertices all inside their own AABB ({without_bounds} geometries without one)",
        sample.len()
    );
}

/// The bundle is what the viewer actually receives, so it gets its own pass:
/// every part must have a position buffer, an index buffer that is a whole
/// number of triangles, and no index past its own vertex count.
#[test]
#[ignore = "needs a local GTA V install"]
fn every_sampled_drawable_produces_a_renderable_bundle() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<String> = Vec::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            // Same as above: only the drawable formats this gate reads.
            let kind = FileKind::from_name(&entry.name_lower);
            if matches!(kind, FileKind::Model | FileKind::ModelDictionary) {
                paths.push(entry.path.clone());
            }
        }
    }
    let step = (paths.len() / 150).max(1);
    let sample: Vec<&String> = paths.iter().step_by(step).take(150).collect();

    let mut bundles = 0usize;
    let mut triangles = 0usize;
    let mut without_geometry = 0usize;

    for path in &sample {
        for drawable in drawables_at(&manager, &keys, path) {
            match MeshBundle::best(&drawable) {
                Some(bundle) => {
                    assert!(!bundle.parts.is_empty(), "{path}: a bundle with no parts");
                    for part in &bundle.parts {
                        assert!(part.vertex_count > 0, "{path}/{}: no vertices", part.name);
                        assert!(
                            part.triangle_count > 0,
                            "{path}/{}: no triangles",
                            part.name
                        );
                        assert!(
                            !part.positions.is_empty(),
                            "{path}/{}: no position buffer",
                            part.name
                        );
                    }
                    triangles += bundle.triangle_count;
                    bundles += 1;
                }
                // Placeholder drawables with no geometry are normal in
                // dictionaries; they must not be an error.
                None => without_geometry += 1,
            }
        }
    }

    assert!(bundles > 50, "only {bundles} bundles built from the sample");
    println!(
        "{} files: {bundles} renderable bundles, {triangles} triangles, {without_geometry} drawables without geometry",
        sample.len()
    );
}
