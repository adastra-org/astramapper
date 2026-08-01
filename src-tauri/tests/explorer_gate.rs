//! The Explorer scaffold against a real install: does walking the archives as
//! one tree actually work, end to end, from the same function the IPC command
//! calls?
//!
//! ```text
//! cargo test -p astramapper -- --ignored --nocapture
//! ```

use astramapper_lib::explorer::listing_for;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use std::path::PathBuf;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn gta_path() -> PathBuf {
    PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.to_string()))
}

fn scan() -> RpfManager {
    let exe = gta_path().join("GTA5.exe");
    let data = std::fs::read(&exe).unwrap_or_else(|e| panic!("cannot read {}: {e}", exe.display()));
    let keys = GtaKeys::from_exe(&data).expect("key recovery failed");
    RpfManager::scan(&gta_path(), &keys).expect("scan the game folder")
}

/// The listing has to descend from the root through directories and across an
/// archive boundary without the caller knowing where the boundaries are. That
/// is the whole point of Explorer Mode.
#[test]
#[ignore = "needs a local GTA V install"]
fn walks_from_the_root_into_a_nested_archive() {
    let manager = scan();

    let root = listing_for(&manager, "").expect("list the root");
    assert_eq!(
        root.len(),
        179,
        "the root should list every top-level archive"
    );
    assert!(
        root.iter().all(|r| r.kind == "archive" && r.expandable),
        "every root row is an expandable archive"
    );
    assert!(
        root.windows(2).all(|w| w[0].path <= w[1].path),
        "the root listing should be sorted"
    );

    // Take a real nested archive and walk down to it the way the breadcrumb
    // does: one listing per path prefix, each step finding the next segment in
    // the previous listing. Crossing the `.rpf` boundary has to be invisible.
    let target = manager
        .archives
        .iter()
        .enumerate()
        .find(|(i, _)| manager.archive_parent[*i].is_some())
        .map(|(_, a)| a.path.clone())
        .expect("the install should contain nested archives");

    // Everything above the top-level archive is a filesystem folder, not an
    // archive entry, so the walk starts at the archive that holds it.
    let top = manager
        .top_level()
        .map(|a| a.path.clone())
        .filter(|p| target.starts_with(p))
        .max_by_key(String::len)
        .expect("a nested archive must sit under a top-level one");

    let mut steps = vec![String::new(), top.clone()];
    for segment in target[top.len()..].split('\\').filter(|s| !s.is_empty()) {
        let previous = steps.last().expect("steps is never empty");
        steps.push(format!("{previous}\\{segment}"));
    }

    for pair in steps.windows(2) {
        let (parent, child) = (&pair[0], &pair[1]);
        let rows = listing_for(&manager, parent).unwrap_or_else(|e| panic!("{parent}: {e}"));
        assert!(!rows.is_empty(), "{parent} listed as empty");

        // Directories sort first, so any directory after a non-directory means
        // the ordering the UI relies on is wrong.
        if let Some(i) = rows.iter().rposition(|r| r.kind == "directory") {
            assert!(
                rows[..=i].iter().all(|r| r.kind == "directory"),
                "{parent}: directories are not grouped first"
            );
        }
        if !parent.is_empty() {
            for row in &rows {
                assert!(
                    row.path.starts_with(parent),
                    "{}: child path escapes its parent {parent}",
                    row.path
                );
            }
        }

        assert!(
            rows.iter().any(|r| &r.path == child),
            "{parent}: does not list {child}"
        );
    }

    let inside = listing_for(&manager, &target).expect("list inside the nested archive");
    assert!(!inside.is_empty(), "{target} is empty from the inside");
    println!(
        "walked {} listings from the root to {target}: {} entries inside",
        steps.len(),
        inside.len()
    );
}

/// A `.rpf` entry must resolve to the archive it opens, not to a dead end and
/// not to the parent's own listing.
#[test]
#[ignore = "needs a local GTA V install"]
fn an_rpf_entry_lists_its_own_contents() {
    let manager = scan();

    let root = listing_for(&manager, "").unwrap();
    let update = root
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case("update.rpf"))
        .expect("update.rpf should be a top-level archive");

    let rows = listing_for(&manager, &update.path).expect("list update.rpf");
    assert!(!rows.is_empty());

    let nested = rows.iter().find(|r| r.kind == "archive");
    if let Some(nested) = nested {
        let inner = listing_for(&manager, &nested.path).expect("list the nested archive");
        assert!(
            !inner.is_empty(),
            "{}: a nested archive listed as empty",
            nested.path
        );
        assert!(
            inner.iter().all(|r| r.path != nested.path),
            "{}: listing returned itself",
            nested.path
        );
        println!("{}: {} entries", nested.path, inner.len());
    }
}

#[test]
#[ignore = "needs a local GTA V install"]
fn reports_an_unknown_path_rather_than_an_empty_listing() {
    let manager = scan();
    let err = listing_for(&manager, r"nope.rpf\does\not\exist").unwrap_err();
    assert!(err.contains("not in the index"), "unexpected error: {err}");
}
