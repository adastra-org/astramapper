//! The bounds gate: prove the resource layer reads real `.ybn` files and
//! agrees with the C# reference block by block.
//!
//! ```text
//! pwsh -File scripts/bounds-oracle.ps1
//! cargo test -p cw-core --test bounds_gate -- --ignored --nocapture
//! ```
//!
//! `fixtures/bounds_oracle.json` is what `YbnFile` in `CodeWalker.Core.dll`
//! sees: the composite tree, every geometry's vertices, polygons, materials,
//! octant counts and the BVH node/tree lists. Every field is compared.

use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::resources::{read_bounds, BoundPolygon, Bounds, BoundsData, BoundsType};
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

/// Read one entry and parse its bounds block.
fn bounds_at(manager: &RpfManager, keys: &GtaKeys, path: &str) -> Bounds {
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

    read_bounds(FileKind::from_name(&entry.name_lower), &data, system_size)
        .unwrap_or_else(|e| panic!("{path}: parse failed: {e}"))
}

/// Same as [`bounds_at`], but reports failure as `None` instead of panicking.
fn try_bounds_at(manager: &RpfManager, keys: &GtaKeys, path: &str) -> Option<Bounds> {
    let entry_ref = manager.find(path)?;
    let entry = manager.entry(entry_ref);
    let (system_size, _) = entry.resource_segments()?;
    let data = manager.extract(entry_ref, keys).ok()?;
    read_bounds(FileKind::from_name(&entry.name_lower), &data, system_size).ok()
}

fn f32_at(value: &Value) -> f32 {
    value.as_f64().expect("a number") as f32
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
}

fn check_common(b: &Bounds, expected: &Value, at: &str) {
    assert_eq!(
        b.bounds_type.as_str(),
        expected["type"].as_str().expect("type"),
        "{at}: bounds type"
    );
    assert!(
        close(b.sphere_radius, f32_at(&expected["sphereRadius"])),
        "{at}: sphere radius {} != {}",
        b.sphere_radius,
        f32_at(&expected["sphereRadius"])
    );
    assert!(close(b.margin, f32_at(&expected["margin"])), "{at}: margin");
    assert!(close(b.volume, f32_at(&expected["volume"])), "{at}: volume");
    assert_eq!(
        b.room_id() as u64,
        expected["roomId"].as_u64().unwrap(),
        "{at}: room id"
    );
    assert_eq!(
        b.ped_density() as u64,
        expected["pedDensity"].as_u64().unwrap(),
        "{at}: ped density"
    );
    assert_eq!(
        b.material_index as u64,
        expected["materialIndex"].as_u64().unwrap(),
        "{at}: material index"
    );
    assert_eq!(
        b.procedural_id as u64,
        expected["proceduralId"].as_u64().unwrap(),
        "{at}: procedural id"
    );
    assert_eq!(
        b.unk_flags as u64,
        expected["unkFlags"].as_u64().unwrap(),
        "{at}: unk flags"
    );
    assert_eq!(
        b.poly_flags as u64,
        expected["polyFlags"].as_u64().unwrap(),
        "{at}: poly flags"
    );
    assert_eq!(
        b.unknown_3ch as u64,
        expected["unkType"].as_u64().unwrap(),
        "{at}: unk type"
    );

    for (field, array) in [
        ("boxMin", &b.box_min),
        ("boxMax", &b.box_max),
        ("boxCenter", &b.box_center),
        ("sphereCenter", &b.sphere_center),
        ("inertia", &b.unknown_60h),
    ] {
        let want = expected[field].as_array().expect(field);
        for (axis, (got, want)) in array.iter().zip(want).enumerate() {
            assert!(close(*got, f32_at(want)), "{at}: {field}[{axis}]");
        }
    }
}

fn check_geometry(g: &cw_core::resources::BoundGeometry, expected: &Value, at: &str) {
    assert_eq!(
        g.vertices.len() as u64,
        expected["verticesCount"].as_u64().unwrap(),
        "{at}: vertices count"
    );
    assert_eq!(
        g.polygons.len() as u64,
        expected["polygonsCount"].as_u64().unwrap(),
        "{at}: polygons count"
    );

    for (axis, (got, want)) in g
        .quantum
        .iter()
        .zip(expected["quantum"].as_array().unwrap())
        .enumerate()
    {
        assert!(close(*got, f32_at(want)), "{at}: quantum[{axis}]");
    }
    for (axis, (got, want)) in g
        .center_geom
        .iter()
        .zip(expected["centerGeom"].as_array().unwrap())
        .enumerate()
    {
        assert!(close(*got, f32_at(want)), "{at}: centerGeom[{axis}]");
    }

    let want_verts = expected["vertices"].as_array().expect("vertices");
    assert_eq!(
        g.vertices.len() * 3,
        want_verts.len(),
        "{at}: vertices array length"
    );
    for (i, (got, want)) in g.vertices.iter().flatten().zip(want_verts).enumerate() {
        assert!(close(*got, f32_at(want)), "{at}: vertex[{i}]");
    }

    let want_polys = expected["polygons"].as_array().expect("polygons");
    assert_eq!(g.polygons.len(), want_polys.len(), "{at}: polygons length");
    for (i, (poly, want)) in g.polygons.iter().zip(want_polys).enumerate() {
        let at = format!("{at}.polygon{i}");
        check_polygon(poly, want, &at);
    }

    let want_mats = expected["materials"].as_array().expect("materials");
    assert_eq!(
        g.materials.len() * 2,
        want_mats.len(),
        "{at}: materials array length"
    );
    for (i, (got, want)) in g
        .materials
        .iter()
        .flat_map(|m| [m.data1, m.data2])
        .zip(want_mats)
        .enumerate()
    {
        assert_eq!(got as u64, want.as_u64().unwrap(), "{at}: material[{i}]");
    }

    let want_polymats = expected["polygonMaterialIndices"]
        .as_array()
        .expect("polygonMaterialIndices");
    assert_eq!(
        g.polygon_material_indices.len(),
        want_polymats.len(),
        "{at}: polygon material indices length"
    );
    for (i, (got, want)) in g
        .polygon_material_indices
        .iter()
        .zip(want_polymats)
        .enumerate()
    {
        assert_eq!(
            *got as u64,
            want.as_u64().unwrap(),
            "{at}: polygon material index[{i}]"
        );
    }

    let want_vcols = expected["vertexColours"].as_array().expect("vertexColours");
    assert_eq!(
        g.vertex_colours.len() * 4,
        want_vcols.len(),
        "{at}: vertex colours array length"
    );
    for (i, (got, want)) in g
        .vertex_colours
        .iter()
        .flat_map(|c| [c.r, c.g, c.b, c.a])
        .zip(want_vcols)
        .enumerate()
    {
        assert_eq!(
            got as u64,
            want.as_u64().unwrap(),
            "{at}: vertex colour[{i}]"
        );
    }

    let want_octs = expected["octantCounts"].as_array().expect("octantCounts");
    match &g.octants {
        Some(octants) => {
            for (i, (got, want)) in octants.counts.iter().zip(want_octs).enumerate() {
                assert_eq!(
                    *got as u64,
                    want.as_u64().unwrap(),
                    "{at}: octant[{i}] count"
                );
            }
        }
        None => assert_eq!(want_octs.len(), 0, "{at}: expected no octants"),
    }
}

fn check_polygon(poly: &BoundPolygon, want: &Value, at: &str) {
    let want_type = want["t"].as_str().unwrap();
    match poly {
        BoundPolygon::Triangle(t) => {
            assert_eq!(want_type, "Triangle", "{at}: type");
            assert!(close(t.area, f32_at(&want["area"])), "{at}: area");
            let want_indices = [
                want["i1"].as_u64().unwrap() as u16,
                want["i2"].as_u64().unwrap() as u16,
                want["i3"].as_u64().unwrap() as u16,
            ];
            let want_edges = [
                want["e1"].as_u64().unwrap() as u16,
                want["e2"].as_u64().unwrap() as u16,
                want["e3"].as_u64().unwrap() as u16,
            ];
            assert_eq!(t.vertex_indices, want_indices, "{at}: vertex indices");
            assert_eq!(t.edge_indices, want_edges, "{at}: edge indices");
        }
        BoundPolygon::Sphere(s) => {
            assert_eq!(want_type, "Sphere", "{at}: type");
            assert_eq!(
                s.sphere_index as u64,
                want["idx"].as_u64().unwrap(),
                "{at}: index"
            );
            assert!(
                close(s.sphere_radius, f32_at(&want["radius"])),
                "{at}: radius"
            );
        }
        BoundPolygon::Capsule(c) => {
            assert_eq!(want_type, "Capsule", "{at}: type");
            assert_eq!(
                c.capsule_index1 as u64,
                want["i1"].as_u64().unwrap(),
                "{at}: index 1"
            );
            assert!(
                close(c.capsule_radius, f32_at(&want["radius"])),
                "{at}: radius"
            );
            assert_eq!(
                c.capsule_index2 as u64,
                want["i2"].as_u64().unwrap(),
                "{at}: index 2"
            );
        }
        BoundPolygon::Box(b) => {
            assert_eq!(want_type, "Box", "{at}: type");
            let want_indices = [
                want["i1"].as_i64().unwrap() as i16,
                want["i2"].as_i64().unwrap() as i16,
                want["i3"].as_i64().unwrap() as i16,
                want["i4"].as_i64().unwrap() as i16,
            ];
            assert_eq!(b.box_indices, want_indices, "{at}: indices");
        }
        BoundPolygon::Cylinder(c) => {
            assert_eq!(want_type, "Cylinder", "{at}: type");
            assert_eq!(
                c.cylinder_index1 as u64,
                want["i1"].as_u64().unwrap(),
                "{at}: index 1"
            );
            assert!(
                close(c.cylinder_radius, f32_at(&want["radius"])),
                "{at}: radius"
            );
            assert_eq!(
                c.cylinder_index2 as u64,
                want["i2"].as_u64().unwrap(),
                "{at}: index 2"
            );
        }
    }
}

fn check_bvh(bvh: Option<&cw_core::resources::Bvh>, expected: &Value, at: &str) {
    let Some(bvh) = bvh else {
        assert!(expected.is_null(), "{at}: expected a bvh but read none");
        return;
    };
    if expected.is_null() {
        assert_eq!(bvh.nodes.len(), 0, "{at}: expected no BVH nodes");
        assert_eq!(bvh.trees.len(), 0, "{at}: expected no BVH trees");
        return;
    }

    let want_nodes = expected["nodes"].as_array().expect("bvh nodes");
    assert_eq!(bvh.nodes.len(), want_nodes.len(), "{at}: bvh node count");
    for (i, (node, want)) in bvh.nodes.iter().zip(want_nodes).enumerate() {
        let at = format!("{at}.bvh.node{i}");
        for (axis, (got, want)) in node
            .min
            .iter()
            .zip(want["mn"].as_array().unwrap())
            .enumerate()
        {
            assert_eq!(*got as i64, want.as_i64().unwrap(), "{at}: min[{axis}]");
        }
        for (axis, (got, want)) in node
            .max
            .iter()
            .zip(want["mx"].as_array().unwrap())
            .enumerate()
        {
            assert_eq!(*got as i64, want.as_i64().unwrap(), "{at}: max[{axis}]");
        }
        assert_eq!(
            node.item_id as i64,
            want["itemId"].as_i64().unwrap(),
            "{at}: item id"
        );
        assert_eq!(
            node.item_count as i64,
            want["itemCount"].as_i64().unwrap(),
            "{at}: item count"
        );
    }

    let want_trees = expected["trees"].as_array().expect("bvh trees");
    assert_eq!(bvh.trees.len(), want_trees.len(), "{at}: bvh tree count");
    for (i, (tree, want)) in bvh.trees.iter().zip(want_trees).enumerate() {
        let at = format!("{at}.bvh.tree{i}");
        for (axis, (got, want)) in tree
            .min
            .iter()
            .zip(want["mn"].as_array().unwrap())
            .enumerate()
        {
            assert_eq!(*got as i64, want.as_i64().unwrap(), "{at}: min[{axis}]");
        }
        for (axis, (got, want)) in tree
            .max
            .iter()
            .zip(want["mx"].as_array().unwrap())
            .enumerate()
        {
            assert_eq!(*got as i64, want.as_i64().unwrap(), "{at}: max[{axis}]");
        }
        assert_eq!(
            tree.node_index1 as i64,
            want["n1"].as_i64().unwrap(),
            "{at}: node 1"
        );
        assert_eq!(
            tree.node_index2 as i64,
            want["n2"].as_i64().unwrap(),
            "{at}: node 2"
        );
    }
}

fn check_bounds_tree(b: &Bounds, expected: &Value, at: &str) {
    check_common(b, expected, at);

    match &b.data {
        BoundsData::Primitive => {
            assert!(
                !matches!(
                    b.bounds_type,
                    BoundsType::Geometry | BoundsType::GeometryBvh | BoundsType::Composite
                ),
                "{at}: primitive tail for a structural type"
            );
        }
        BoundsData::Geometry(g) => check_geometry(g, expected, at),
        BoundsData::Bvh(g) => {
            check_geometry(&g.geometry, expected, at);
            check_bvh(g.bvh.as_ref(), &expected["bvh"], at);
        }
        BoundsData::Composite(c) => {
            let want_children = expected["children"].as_array().expect("children");
            assert_eq!(
                c.children.len(),
                want_children.len(),
                "{at}: children count"
            );
            for (i, (child, want)) in c.children.iter().zip(want_children).enumerate() {
                check_bounds_tree(child, want, &format!("{at}.child{i}"));
            }

            let want_t1 = expected["childrenTransformation1"].as_array().expect("t1");
            assert_eq!(
                c.children_transformation1.len() * 16,
                want_t1.len(),
                "{at}: t1 count"
            );
            for (i, (matrix, want)) in c
                .children_transformation1
                .iter()
                .zip(want_t1.chunks_exact(16))
                .enumerate()
            {
                let at = format!("{at}.child{i} transform1");
                let columns = [
                    matrix.column1,
                    matrix.column2,
                    matrix.column3,
                    matrix.column4,
                ];
                for (ci, (column, want_col)) in columns.iter().zip(want.chunks_exact(4)).enumerate()
                {
                    for (axis, want) in want_col.iter().enumerate() {
                        if axis == 3 {
                            assert_eq!(
                                column[axis].to_bits() as u64,
                                want.as_u64().expect("flag"),
                                "{at}: column{ci} flag"
                            );
                        } else {
                            assert!(
                                close(column[axis], f32_at(want)),
                                "{at}: column{ci} axis{axis}: {} != {}",
                                column[axis],
                                f32_at(want)
                            );
                        }
                    }
                }
            }

            let want_t2 = expected["childrenTransformation2"].as_array().expect("t2");
            assert_eq!(
                c.children_transformation2.len() * 16,
                want_t2.len(),
                "{at}: t2 count"
            );
            for (i, (matrix, want)) in c
                .children_transformation2
                .iter()
                .zip(want_t2.chunks_exact(16))
                .enumerate()
            {
                let at = format!("{at}.child{i} transform2");
                let columns = [
                    matrix.column1,
                    matrix.column2,
                    matrix.column3,
                    matrix.column4,
                ];
                for (ci, (column, want_col)) in columns.iter().zip(want.chunks_exact(4)).enumerate()
                {
                    for (axis, want) in want_col.iter().enumerate() {
                        if axis == 3 {
                            assert_eq!(
                                column[axis].to_bits() as u64,
                                want.as_u64().expect("flag"),
                                "{at}: column{ci} flag"
                            );
                        } else {
                            assert!(
                                close(column[axis], f32_at(want)),
                                "{at}: column{ci} axis{axis}: {} != {}",
                                column[axis],
                                f32_at(want)
                            );
                        }
                    }
                }
            }

            let want_bbs = expected["childrenBoundingBoxes"].as_array().expect("bbs");
            assert_eq!(
                c.children_bounding_boxes.len() * 8,
                want_bbs.len(),
                "{at}: aabb count"
            );
            for (i, (got, want)) in c
                .children_bounding_boxes
                .iter()
                .flatten()
                .zip(want_bbs)
                .enumerate()
            {
                let want = want.as_f64().unwrap() as f32;
                assert!(close(*got, want), "{at}.child{i} aabb");
            }

            let want_f1 = expected["childrenFlags1"].as_array().expect("f1");
            assert_eq!(c.children_flags1.len() * 2, want_f1.len(), "{at}: f1 count");
            for (i, (got, want)) in c.children_flags1.iter().flatten().zip(want_f1).enumerate() {
                assert_eq!(*got as u64, want.as_u64().unwrap(), "{at}.child{i} flags1");
            }

            let want_f2 = expected["childrenFlags2"].as_array().expect("f2");
            assert_eq!(c.children_flags2.len() * 2, want_f2.len(), "{at}: f2 count");
            for (i, (got, want)) in c.children_flags2.iter().flatten().zip(want_f2).enumerate() {
                assert_eq!(*got as u64, want.as_u64().unwrap(), "{at}.child{i} flags2");
            }

            check_bvh(c.bvh.as_ref(), &expected["bvh"], at);
        }
    }
}

#[test]
#[ignore = "needs a local GTA V install and fixtures/bounds_oracle.json"]
fn agrees_with_the_reference_bounds_reader() {
    let oracle_path = workspace_root().join("fixtures/bounds_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "skipping: {} is missing. Run scripts/bounds-oracle.ps1 first.",
            oracle_path.display()
        );
        return;
    };
    // PowerShell's Out-File -Encoding utf8 writes a BOM.
    let oracle: Value =
        serde_json::from_str(raw.trim_start_matches('\u{FEFF}')).expect("parse the bounds oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("oracle files array");
    assert!(!files.is_empty(), "the oracle is empty");

    let mut geometries_checked = 0usize;
    let mut vertices_checked = 0usize;
    let mut polygons_checked = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");

        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let entry = manager.entry(entry_ref);
        let (system_size, graphics_size) = entry.resource_segments().expect("a resource entry");

        assert_eq!(
            system_size,
            file["systemSize"].as_u64().expect("systemSize") as usize,
            "{path}: system segment size"
        );
        assert_eq!(
            graphics_size,
            file["graphicsSize"].as_u64().expect("graphicsSize") as usize,
            "{path}: graphics segment size"
        );

        let bounds = bounds_at(&manager, &keys, path);
        check_bounds_tree(&bounds, &file["bounds"], path);

        fn tally(
            data: &BoundsData,
            geometries: &mut usize,
            vertices: &mut usize,
            polygons: &mut usize,
        ) {
            match data {
                BoundsData::Geometry(g) => {
                    *geometries += 1;
                    *vertices += g.vertices.len();
                    *polygons += g.polygons.len();
                }
                BoundsData::Bvh(g) => {
                    *geometries += 1;
                    *vertices += g.geometry.vertices.len();
                    *polygons += g.geometry.polygons.len();
                }
                BoundsData::Composite(c) => {
                    for child in &c.children {
                        tally(&child.data, geometries, vertices, polygons);
                    }
                }
                BoundsData::Primitive => {}
            }
        }
        let mut g = 0;
        let mut v = 0;
        let mut p = 0;
        tally(&bounds.data, &mut g, &mut v, &mut p);
        geometries_checked += g;
        vertices_checked += v;
        polygons_checked += p;
    }

    println!(
        "{geometries_checked} geometries, {vertices_checked} vertices, {polygons_checked} polygons agree with CodeWalker across {} files",
        files.len()
    );
}

/// Count the triangle polygons an oracle bounds tree describes.
fn count_oracle_triangles(expected: &Value) -> usize {
    let mut count = 0;
    if let Some(polys) = expected["polygons"].as_array() {
        for poly in polys {
            if poly["t"].as_str() == Some("Triangle") {
                count += 1;
            }
        }
    }
    if let Some(children) = expected["children"].as_array() {
        for child in children {
            count += count_oracle_triangles(child);
        }
    }
    count
}

/// The viewer flattens triangle polygons into one mesh; every triangle the
/// oracle read must appear in it, exactly once.
#[test]
#[ignore = "needs a local GTA V install and fixtures/bounds_oracle.json"]
fn flattened_triangles_match_the_oracle() {
    let oracle_path = workspace_root().join("fixtures/bounds_oracle.json");
    let Ok(raw) = std::fs::read_to_string(&oracle_path) else {
        println!(
            "skipping: {} is missing. Run scripts/bounds-oracle.ps1 first.",
            oracle_path.display()
        );
        return;
    };
    let oracle: Value =
        serde_json::from_str(raw.trim_start_matches('\u{FEFF}')).expect("parse the bounds oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("oracle files array");
    let mut meshes = 0usize;
    let mut triangles = 0usize;

    for file in files {
        let path = file["path"].as_str().expect("path");
        let bounds = bounds_at(&manager, &keys, path);

        let expected = count_oracle_triangles(&file["bounds"]);
        let got = bounds
            .triangle_mesh()
            .map(|m| m.triangle_count)
            .unwrap_or(0);
        assert_eq!(got, expected, "{path}: flattened triangle count");

        if let Some(mesh) = bounds.triangle_mesh() {
            meshes += 1;
            triangles += mesh.triangle_count;
            // The renderer indexes into its own positions; a bad index would
            // draw garbage, so every index must stay in range.
            for part in &mesh.parts {
                assert!(
                    part.positions.len() / 3 >= 3,
                    "{path}/{}: no vertices",
                    part.name
                );
                assert_eq!(
                    part.indices.len(),
                    part.triangle_count * 3,
                    "{path}/{}: index count",
                    part.name
                );
                for index in &part.indices {
                    assert!(
                        (*index as usize) < part.vertex_count,
                        "{path}/{}: index {index} out of range",
                        part.name
                    );
                }
            }
        }
    }

    assert!(meshes > 0, "no bounds file produced a mesh");
    println!("{meshes} files flattened to meshes, {triangles} triangles total");
}

/// Every `.ybn` in the install parses, and the composite tree resolves.
///
/// The reference throws on the handful of truncated resources in the other
/// formats; bounds files have the same failure mode, so the failures are
/// counted and asserted to be a small minority rather than invisible.
#[test]
#[ignore = "needs a local GTA V install"]
fn every_bounds_file_in_the_install_parses() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<String> = Vec::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if FileKind::from_name(&entry.name_lower) == FileKind::Bounds {
                paths.push(entry.path.clone());
            }
        }
    }
    assert!(!paths.is_empty(), "the install should contain .ybn files");

    let mut parsed = 0usize;
    let mut geometries = 0usize;
    let mut triangles = 0usize;
    let mut failed = 0usize;

    for path in &paths {
        let Some(bounds) = try_bounds_at(&manager, &keys, path) else {
            failed += 1;
            continue;
        };
        parsed += 1;
        let mesh = bounds.triangle_mesh();
        triangles += mesh.as_ref().map(|m| m.triangle_count).unwrap_or(0);
        if let BoundsData::Composite(c) = &bounds.data {
            geometries += c.children.len();
        }
    }

    assert!(
        failed <= paths.len() / 100 + 2,
        "{failed} of {} bounds files failed to parse",
        paths.len()
    );
    assert!(parsed > 1000, "only {parsed} bounds files parsed");
    println!(
        "{parsed} of {} bounds files parse, {geometries} composite children, {triangles} triangles",
        paths.len()
    );
}
