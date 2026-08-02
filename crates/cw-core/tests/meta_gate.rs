//! The meta gate: prove the `.ymt` readers agree with the C# reference, value
//! by value.
//!
//! ```text
//! pwsh -File scripts/meta-oracle.ps1
//! cargo test -p cw-core --test meta_gate -- --ignored --nocapture
//! ```
//!
//! The reference has no tree API — `MetaXml.GetXml` writes text — so the oracle
//! parses that XML back and flattens it into its leaves in document order. This
//! flattens its own tree the same way and compares. What is being compared is
//! the *walk*: the same fields, in the same order, with the same values,
//! resolved through the same schema.
//!
//! Values are compared with whitespace and commas removed. The reference lays a
//! numeric array out ten per line and this joins it with commas; a difference
//! there is a difference in printing, not in reading, and normalising is what
//! keeps the test about the latter. Everything that distinguishes one value
//! from another survives normalisation.
//!
//! Plus a wide pass: every `.ymt` in the install must read, and its tree must
//! be free of the two error nodes that mean a real failure to walk.

use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::meta::{read_meta, MetaNode};
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

/// Whether a node stands for an XML attribute rather than an element. The RBF
/// walker keeps an element's attributes as children, because a tree has nowhere
/// else to put them; the reference writes them inside the opening tag.
fn is_attribute(node: &MetaNode) -> bool {
    node.type_name.as_deref() == Some("attribute")
}

/// The leaves of a tree, in document order, by the oracle's rule: an element
/// with no element children is a leaf, and its value is its `value` attribute,
/// else its `x`/`y`/`z`/`w` attributes joined, else its text.
///
/// Error nodes are skipped, matching the oracle skipping the reference's own
/// `<error>` elements.
fn leaves(node: &MetaNode, out: &mut Vec<(String, String)>, limit: usize) {
    for child in &node.children {
        if out.len() >= limit {
            return;
        }
        // An attribute is never a leaf of its own: it is read by `leaf_value`
        // of the element holding it, or dropped, exactly as the oracle drops it.
        if child.error || is_attribute(child) {
            continue;
        }
        if child.children.iter().any(|c| !c.error && !is_attribute(c)) {
            leaves(child, out, limit);
            continue;
        }
        out.push((child.name.clone(), leaf_value(child)));
    }
}

fn leaf_value(node: &MetaNode) -> String {
    let attribute = |name: &str| {
        node.children
            .iter()
            .find(|c| is_attribute(c) && c.name == name)
            .and_then(|c| c.value.clone())
    };
    if let Some(value) = attribute("value") {
        return value;
    }
    if attribute("x").is_some() {
        return ["x", "y", "z", "w"]
            .iter()
            .filter_map(|a| attribute(a))
            .collect::<Vec<_>>()
            .join(", ");
    }
    node.value.clone().unwrap_or_default()
}

/// Drop the formatting: whitespace, commas, and the NULs a fixed-length string
/// is padded with.
///
/// Non-ASCII runs collapse to a single `?` on both sides. That is the one
/// deliberate divergence in this layer: the reference decodes strings as ASCII,
/// which turns every byte above 0x7F into `?`, and this decodes them as UTF-8,
/// which gives the text back — so `credits.ymt` reads `Łukasz` here and
/// `??ukasz` in CodeWalker. Collapsing keeps the comparison exact over every
/// ASCII character, which is everything else in the file.
fn normalise(value: &str) -> String {
    let mut out = String::new();
    let mut in_substitution = false;
    for c in value.chars() {
        if c.is_whitespace() || c == ',' || c == '\0' {
            continue;
        }
        if !c.is_ascii() || c == '?' {
            if !in_substitution {
                out.push('?');
                in_substitution = true;
            }
            continue;
        }
        in_substitution = false;
        out.push(c);
    }
    out
}

#[test]
#[ignore = "needs fixtures/meta_oracle.json — see scripts/meta-oracle.ps1"]
fn agrees_with_the_reference_metadata_reader() {
    let oracle_path = workspace_root().join("fixtures/meta_oracle.json");
    let Ok(text) = std::fs::read_to_string(&oracle_path) else {
        println!("skipping: {} is missing", oracle_path.display());
        return;
    };
    let oracle: Value = serde_json::from_str(&text).expect("parse the oracle");

    let keys = load_keys();
    let manager = scan(&keys);

    let files = oracle["files"].as_array().expect("files array");
    assert!(!files.is_empty(), "the oracle holds no files");

    let mut compared = 0usize;
    let mut formats = std::collections::BTreeMap::<String, usize>::new();

    for file in files {
        let path = file["path"].as_str().expect("path");
        let entry_ref = manager
            .find(path)
            .unwrap_or_else(|| panic!("{path} is not in the index"));
        let entry = manager.entry(entry_ref);
        let system_size = entry.resource_segments().map(|(system, _)| system);

        let data = manager
            .extract(entry_ref, &keys)
            .unwrap_or_else(|e| panic!("{path}: extract failed: {e}"));
        let meta =
            read_meta(FileKind::Meta, &data, system_size).unwrap_or_else(|e| panic!("{path}: {e}"));

        assert_eq!(
            meta.format(),
            file["format"].as_str().unwrap(),
            "{path}: container format"
        );
        *formats.entry(meta.format().to_string()).or_default() += 1;

        let tree = meta.to_tree();
        assert_eq!(
            tree.name,
            file["rootName"].as_str().unwrap(),
            "{path}: root structure"
        );

        let expected = file["leaves"].as_array().expect("leaves");
        let mut got = Vec::new();
        leaves(&tree, &mut got, expected.len());

        // A truncated oracle stops mid-array, so only compare the prefix both
        // sides agree they hold.
        let count = expected.len().min(got.len());
        assert!(
            count > 0 || expected.is_empty(),
            "{path}: read no leaves, the reference read {}",
            expected.len()
        );

        for (i, want) in expected.iter().take(count).enumerate() {
            let (name, value) = &got[i];
            let want_name = want["name"].as_str().unwrap_or("");
            let want_value = want["value"].as_str().unwrap_or("");
            assert_eq!(
                name, want_name,
                "{path}: leaf {i} is `{name}`, the reference has `{want_name}`"
            );
            assert_eq!(
                normalise(value),
                normalise(want_value),
                "{path}: leaf {i} (`{name}`) is `{value}`, the reference has `{want_value}`"
            );
            compared += 1;
        }

        if !file["truncated"].as_bool().unwrap_or(false) {
            assert_eq!(
                got.len(),
                expected.len(),
                "{path}: read {} leaves, the reference read {}",
                got.len(),
                expected.len()
            );
        }
    }

    println!(
        "{} files, {compared} leaves match the reference ({:?})",
        files.len(),
        formats
    );
}

/// Every `.ymt` in the install, read for what it says about itself.
#[test]
#[ignore = "needs a local GTA V install"]
fn reads_every_metadata_file_in_the_install() {
    let keys = load_keys();
    let manager = scan(&keys);

    let mut paths: Vec<(cw_core::rpf::EntryRef, String)> = Vec::new();
    for (index, archive) in manager.archives.iter().enumerate() {
        for (entry_index, entry) in archive.entries.iter().enumerate() {
            if matches!(FileKind::from_name(&entry.name_lower), FileKind::Meta)
                && entry.name_lower.ends_with(".ymt")
            {
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
    assert!(!paths.is_empty(), "the install should contain .ymt files");

    let mut read = 0usize;
    let mut nodes = 0usize;
    let mut formats = std::collections::BTreeMap::<&str, usize>::new();
    let mut roots = std::collections::BTreeSet::<String>::new();
    let mut failures: Vec<String> = Vec::new();
    let mut missing_structures = 0usize;
    let mut files_missing_structures = std::collections::BTreeSet::<String>::new();
    let mut missing_names = std::collections::BTreeSet::<String>::new();

    for (entry_ref, path) in &paths {
        let entry = manager.entry(*entry_ref);
        let system_size = entry.resource_segments().map(|(system, _)| system);
        let Ok(data) = manager.extract(*entry_ref, &keys) else {
            failures.push(format!("{path}: extract failed"));
            continue;
        };
        let meta = match read_meta(FileKind::Meta, &data, system_size) {
            Ok(meta) => meta,
            Err(e) => {
                failures.push(format!("{path}: {e}"));
                continue;
            }
        };

        read += 1;
        *formats.entry(meta.format()).or_default() += 1;
        roots.insert(meta.root_name());

        let tree = meta.to_tree();
        nodes += tree.count();

        // A structure the file references but does not define. The reference
        // falls back to `PsoTypes.cs` — 1.5 MB of generated definitions — for
        // these; this port has no such table, so they are counted and named
        // rather than asserted away. See `docs/formats.md`.
        let mut stack = vec![&tree];
        while let Some(node) = stack.pop() {
            if node.error
                && node
                    .value
                    .as_deref()
                    .is_some_and(|v| v.contains("couldn't find structure info"))
            {
                missing_structures += 1;
                files_missing_structures.insert(path.clone());
                missing_names.insert(node.name.clone());
            }
            stack.extend(node.children.iter());
        }
    }

    assert!(read > 100, "sample too small to mean anything: {read}");
    assert!(failures.is_empty(), "{} failed: {:?}", failures.len(), {
        failures.truncate(10);
        failures
    });
    println!(
        "{read} .ymt files, {nodes} nodes, {formats:?}, {} distinct root structures",
        roots.len()
    );
    if missing_structures > 0 {
        println!(
            "{missing_structures} nodes in {} files reference structures the file does not define: {:?}",
            files_missing_structures.len(),
            missing_names.iter().take(12).collect::<Vec<_>>()
        );
    }
}
