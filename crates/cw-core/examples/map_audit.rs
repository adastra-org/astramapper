//! Read-only audit of the loose map files (`.ymap` / `.ytyp`) of a FiveM
//! server. One JSON report: every YMAP with its extents and LOD mix, every MLO
//! with its rooms and portals, and flags for the usual problems.
//!
//! The crash this exists for is `MapDataLoadedNode Pool Full, Size == 1100`:
//! too many YMAPs loaded at the same spot. The per-YMAP streaming extents in
//! the report feed `scripts/map-hex.ts`, which counts them per hex cell.
//!
//! ```text
//! cargo run --release -p cw-core --example map_audit -- <resources dir> \
//!     [--rules map-rules.json] [--gta <GTA V folder>] [--out map-audit.json]
//! ```
//!
//! `--gta` (or `ASTRAMAPPER_GTA_PATH`) reads the game's archives for the
//! vanilla YMAP names and archetype names: it tells overrides from new maps,
//! checks `parent` links against the game, and names door props. Without it
//! those checks are skipped, never guessed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use cw_core::filetype::FileKind;
use cw_core::filetypes::ytyp::{Archetype, MloArchetype};
use cw_core::filetypes::{read_ymap, read_ytyp, YmapFile};
use cw_core::meta::typed::LodLevel;
use cw_core::meta::{read_meta, Meta, MetaFile};
use cw_core::rpf::entry::size_from_flags;
use cw_core::rpf::RpfManager;
use cw_core::utils::{gen_hash, GtaKeys};
use flate2::read::DeflateDecoder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

const RSC7: u32 = 0x3743_5352;

/// Thresholds and the MLO conventions the server wants. Every field has a
/// default, so a rules file only names what it changes.
#[derive(Deserialize, Serialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct Rules {
    /// Bits every MLO room (except limbo, room 0) must have set.
    room_flags: u32,
    /// Bits a portal with a door attached must have set.
    door_portal_flags: u32,
    /// Bits a portal with a window or glass attached must have set.
    window_portal_flags: u32,
    /// Allowed room timecycles. Empty: only require that there is one.
    room_timecycles: Vec<String>,
    /// A YMAP with only HD entities whose streaming box is wider than this
    /// (metres, X or Y) stays loaded over too much of the map.
    huge_extent_m: f32,
    /// A YMAP with fewer entities than this is a merge candidate.
    tiny_entities: usize,
    /// How far (metres) an entity may sit outside the stored extents before
    /// they count as stale.
    extents_tolerance_m: f32,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            room_flags: 96,
            door_portal_flags: 64,
            window_portal_flags: 8,
            room_timecycles: vec!["mp_studio_lo".into()],
            huge_extent_m: 1000.0,
            tiny_entities: 5,
            extents_tolerance_m: 5.0,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct YmapRow {
    file: String,
    resource: String,
    name: String,
    /// Under a `stream` folder: FiveM serves it. Files elsewhere are listed
    /// but never flagged.
    streamed: bool,
    bytes: u64,
    /// Same name as a YMAP of the game. `None` when the game was not read.
    vanilla_override: Option<bool>,
    parent: Option<String>,
    flags: u32,
    content_flags: u32,
    streaming_min: [f32; 3],
    streaming_max: [f32; 3],
    entities_min: [f32; 3],
    entities_max: [f32; 3],
    entities: usize,
    lods: BTreeMap<String, usize>,
    mlo_instances: usize,
    occluders: usize,
    grass_batches: usize,
    lod_lights: usize,
    issues: Vec<String>,
    problem: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RoomRow {
    index: usize,
    name: String,
    flags: u32,
    timecycle: String,
    issues: Vec<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
enum PortalKind {
    Door,
    Window,
    /// Nothing attached that names a door or a window: not checked.
    Other,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PortalRow {
    index: usize,
    room_from: u32,
    room_to: u32,
    flags: u32,
    kind: PortalKind,
    attached: Vec<String>,
    issues: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MloRow {
    name: String,
    rooms: Vec<RoomRow>,
    portals: Vec<PortalRow>,
    issues: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct YtypRow {
    file: String,
    resource: String,
    name: String,
    streamed: bool,
    bytes: u64,
    archetypes: usize,
    mlos: Vec<MloRow>,
    issues: Vec<String>,
    problem: Option<String>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Totals {
    ymaps: usize,
    ymaps_streamed: usize,
    vanilla_overrides: usize,
    ytyps: usize,
    mlos: usize,
    unreadable: usize,
    by_issue: BTreeMap<String, usize>,
    ymaps_by_resource: BTreeMap<String, usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Duplicate {
    name: String,
    files: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    generated_with: &'static str,
    root: String,
    game_read: bool,
    rules: Rules,
    totals: Totals,
    duplicate_ymaps: Vec<Duplicate>,
    duplicate_ytyps: Vec<Duplicate>,
    duplicate_archetypes: Vec<Duplicate>,
    ymaps: Vec<YmapRow>,
    ytyps: Vec<YtypRow>,
}

fn load_meta(path: &Path) -> Result<Meta, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 16 {
        return Err("shorter than an RSC7 header".into());
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    if magic != RSC7 {
        return Err(format!("magic {magic:08x}, not RSC7"));
    }
    let system_flags = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let mut data = Vec::new();
    DeflateDecoder::new(&bytes[16..])
        .read_to_end(&mut data)
        .map_err(|e| format!("inflate: {e}"))?;
    match read_meta(
        FileKind::Meta,
        &data,
        Some(size_from_flags(system_flags) as usize),
    ) {
        Ok(MetaFile::Rsc(meta)) => Ok(meta),
        Ok(other) => Err(format!("{} metadata, expected RSC", other.format())),
        Err(e) => Err(e.to_string()),
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn ext_is(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn stem_lower(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_lowercase()
}

/// The folder that owns the file, up to its `stream` directory, and whether
/// there was one.
fn resource_of(root: &Path, path: &Path) -> (String, bool) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut parts: Vec<String> = Vec::new();
    let mut streamed = false;
    for c in rel.components() {
        let s = c.as_os_str().to_string_lossy().to_string();
        if s.eq_ignore_ascii_case("stream") {
            streamed = true;
            break;
        }
        parts.push(s);
    }
    if !streamed {
        parts.pop();
    }
    (parts.join("/"), streamed)
}

fn lod_label(level: LodLevel) -> String {
    match level {
        LodLevel::Hd => "hd".into(),
        LodLevel::OrphanHd => "orphan-hd".into(),
        LodLevel::Lod => "lod".into(),
        LodLevel::Slod1 => "slod1".into(),
        LodLevel::Slod2 => "slod2".into(),
        LodLevel::Slod3 => "slod3".into(),
        LodLevel::Slod4 => "slod4".into(),
        LodLevel::Unknown(n) => format!("unknown-{n}"),
    }
}

fn is_detail(label: &str) -> bool {
    label == "hd" || label == "orphan-hd"
}

/// HD and LOD entities in one file: the parent/child hierarchy wants them in
/// separate YMAPs linked by `parent`.
fn mixed_lod(lods: &BTreeMap<String, usize>) -> bool {
    let detail = lods.keys().any(|k| is_detail(k));
    let coarse = lods.keys().any(|k| !is_detail(k));
    detail && coarse
}

fn outside(p: [f32; 3], min: [f32; 3], max: [f32; 3], tol: f32) -> bool {
    (0..3).any(|i| p[i] < min[i] - tol || p[i] > max[i] + tol)
}

fn ymap_issues(row: &YmapRow, positions: &[[f32; 3]], rules: &Rules) -> Vec<String> {
    let mut issues = Vec::new();
    let only_detail = !row.lods.is_empty() && row.lods.keys().all(|k| is_detail(k));
    let span = (row.streaming_max[0] - row.streaming_min[0])
        .max(row.streaming_max[1] - row.streaming_min[1]);
    // A vanilla override keeps the game's own extents on purpose.
    if only_detail && span > rules.huge_extent_m && row.vanilla_override != Some(true) {
        issues.push("huge-extents".into());
    }
    if mixed_lod(&row.lods) {
        issues.push("mixed-lod".into());
    }
    if row.entities > 0 && row.entities < rules.tiny_entities && row.mlo_instances == 0 {
        issues.push("tiny-ymap".into());
    }
    let tol = rules.extents_tolerance_m;
    if positions.iter().any(|&p| {
        outside(p, row.entities_min, row.entities_max, tol)
            || outside(p, row.streaming_min, row.streaming_max, tol)
    }) {
        issues.push("extents-stale".into());
    }
    issues
}

fn classify_portal(attached: &[String]) -> PortalKind {
    if attached.iter().any(|n| n.contains("door")) {
        PortalKind::Door
    } else if attached
        .iter()
        .any(|n| n.contains("window") || n.contains("glass") || n.contains("_win"))
    {
        PortalKind::Window
    } else {
        PortalKind::Other
    }
}

fn has_bits(flags: u32, bits: u32) -> bool {
    flags & bits == bits
}

fn check_mlo(
    mlo: &MloArchetype,
    names: &HashMap<u32, String>,
    rules: &Rules,
    allowed_tc: &HashSet<u32>,
) -> MloRow {
    let name_of = |h: u32| {
        names
            .get(&h)
            .cloned()
            .unwrap_or_else(|| cw_core::utils::hash_string(h))
    };
    let mut rooms = Vec::new();
    for (i, room) in mlo.rooms.iter().enumerate() {
        let mut issues = Vec::new();
        // Room 0 is limbo, the outside: no timecycle and no room flags.
        if i > 0 {
            if !has_bits(room.flags, rules.room_flags) {
                issues.push("mlo-room-flags".into());
            }
            if room.timecycle_name == 0
                || (!allowed_tc.is_empty() && !allowed_tc.contains(&room.timecycle_name))
            {
                issues.push("mlo-timecycle".into());
            }
        }
        rooms.push(RoomRow {
            index: i,
            name: room.name.clone().unwrap_or_default(),
            flags: room.flags,
            timecycle: if room.timecycle_name == 0 {
                String::new()
            } else {
                name_of(room.timecycle_name)
            },
            issues,
        });
    }
    let mut portals = Vec::new();
    for (i, portal) in mlo.portals.iter().enumerate() {
        let attached: Vec<String> = portal
            .attached_objects
            .iter()
            .filter_map(|&idx| mlo.entities.get(idx as usize))
            .map(|e| name_of(e.entity.archetype_name).to_lowercase())
            .collect();
        let kind = classify_portal(&attached);
        let mut issues = Vec::new();
        match kind {
            PortalKind::Door if !has_bits(portal.flags, rules.door_portal_flags) => {
                issues.push("mlo-portal-flags".into());
            }
            PortalKind::Window if !has_bits(portal.flags, rules.window_portal_flags) => {
                issues.push("mlo-portal-flags".into());
            }
            _ => {}
        }
        portals.push(PortalRow {
            index: i,
            room_from: portal.room_from,
            room_to: portal.room_to,
            flags: portal.flags,
            kind,
            attached,
            issues,
        });
    }
    let mut issues: Vec<String> = rooms
        .iter()
        .flat_map(|r| r.issues.iter().cloned())
        .chain(portals.iter().flat_map(|p| p.issues.iter().cloned()))
        .collect();
    issues.sort();
    issues.dedup();
    MloRow {
        name: name_of(mlo.base.name),
        rooms,
        portals,
        issues,
    }
}

enum Scanned {
    Ymap(PathBuf, u64, Result<Box<YmapFile>, String>),
    Ytyp(
        PathBuf,
        u64,
        Result<cw_core::filetypes::ytyp::YtypFile, String>,
    ),
}

/// The game's YMAP names and every name the archives give (archetypes,
/// timecycles not included: those come from the rules).
fn read_game(folder: &Path) -> Result<(HashSet<String>, HashMap<u32, String>), String> {
    let exe = std::fs::read(folder.join("GTA5.exe")).map_err(|e| format!("GTA5.exe: {e}"))?;
    let keys = GtaKeys::from_exe(&exe).map_err(|e| e.to_string())?;
    let manager = RpfManager::scan(folder, &keys).map_err(|e| e.to_string())?;
    let mut ymaps = HashSet::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if let Some(stem) = entry.name_lower.strip_suffix(".ymap") {
                ymaps.insert(stem.to_string());
            }
        }
    }
    Ok((ymaps, manager.index_names()))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .unwrap_or_else(|| r"E:\FiveM\Astra\resources".into()),
    );
    let mut out_path = PathBuf::from("map-audit.json");
    let mut rules = Rules::default();
    let mut gta = std::env::var("ASTRAMAPPER_GTA_PATH")
        .ok()
        .map(PathBuf::from);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out_path = PathBuf::from(args.next().expect("--out needs a path")),
            "--rules" => {
                let p = args.next().expect("--rules needs a path");
                let text = std::fs::read_to_string(&p).expect("read rules");
                rules = serde_json::from_str(&text).expect("parse rules");
            }
            "--gta" => gta = Some(PathBuf::from(args.next().expect("--gta needs a path"))),
            other => panic!("unknown argument {other}"),
        }
    }

    let (vanilla, mut names) = match gta.as_deref().map(read_game) {
        Some(Ok(game)) => {
            eprintln!("game: {} vanilla ymaps", game.0.len());
            (Some(game.0), game.1)
        }
        Some(Err(e)) => {
            eprintln!("game not read ({e}): vanilla and parent checks skipped");
            (None, HashMap::new())
        }
        None => {
            eprintln!("no --gta: vanilla and parent checks skipped");
            (None, HashMap::new())
        }
    };

    let mut files = Vec::new();
    walk(&root, &mut files);
    // Every loose file's name is an archetype or asset name candidate.
    for f in &files {
        let stem = stem_lower(f);
        names.entry(gen_hash(&stem)).or_insert(stem);
    }
    for tc in &rules.room_timecycles {
        names.insert(gen_hash(&tc.to_lowercase()), tc.clone());
    }
    let allowed_tc: HashSet<u32> = rules
        .room_timecycles
        .iter()
        .map(|t| gen_hash(&t.to_lowercase()))
        .collect();

    let maps: Vec<&PathBuf> = files
        .iter()
        .filter(|p| ext_is(p, "ymap") || ext_is(p, "ytyp"))
        .collect();
    eprintln!("{} map files under {}", maps.len(), root.display());

    let scanned: Vec<Scanned> = maps
        .par_iter()
        .map(|&path| {
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if ext_is(path, "ymap") {
                let r = catch_unwind(AssertUnwindSafe(|| {
                    load_meta(path)
                        .and_then(|m| read_ymap(&m).map(Box::new).map_err(|e| e.to_string()))
                }))
                .unwrap_or_else(|_| Err("parser panicked".into()));
                Scanned::Ymap(path.clone(), bytes, r)
            } else {
                let r = catch_unwind(AssertUnwindSafe(|| {
                    load_meta(path).and_then(|m| read_ytyp(&m).map_err(|e| e.to_string()))
                }))
                .unwrap_or_else(|_| Err("parser panicked".into()));
                Scanned::Ytyp(path.clone(), bytes, r)
            }
        })
        .collect();

    // Names the files themselves carry, before any row resolves a hash.
    for s in &scanned {
        let strings = match s {
            Scanned::Ymap(_, _, Ok(y)) => &y.strings,
            Scanned::Ytyp(_, _, Ok(t)) => &t.strings,
            _ => continue,
        };
        for st in strings {
            let lower = st.to_lowercase();
            names.entry(gen_hash(&lower)).or_insert(lower);
        }
    }

    let rel = |p: &Path| {
        p.strip_prefix(&root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };

    let loose_ymaps: HashSet<String> = files
        .iter()
        .filter(|f| ext_is(f, "ymap"))
        .map(|f| stem_lower(f))
        .collect();
    let mut totals = Totals::default();
    let mut ymaps = Vec::new();
    let mut ytyps = Vec::new();
    let mut archetype_owners: BTreeMap<u32, Vec<String>> = BTreeMap::new();

    for s in &scanned {
        match s {
            Scanned::Ymap(path, bytes, r) => {
                let (resource, streamed) = resource_of(&root, path);
                let name = stem_lower(path);
                let mut row = YmapRow {
                    file: rel(path),
                    resource,
                    name: name.clone(),
                    streamed,
                    bytes: *bytes,
                    vanilla_override: vanilla.as_ref().map(|v| v.contains(&name)),
                    parent: None,
                    flags: 0,
                    content_flags: 0,
                    streaming_min: [0.0; 3],
                    streaming_max: [0.0; 3],
                    entities_min: [0.0; 3],
                    entities_max: [0.0; 3],
                    entities: 0,
                    lods: BTreeMap::new(),
                    mlo_instances: 0,
                    occluders: 0,
                    grass_batches: 0,
                    lod_lights: 0,
                    issues: Vec::new(),
                    problem: None,
                };
                match r {
                    Ok(y) => {
                        row.parent = (y.parent != 0).then(|| {
                            names
                                .get(&y.parent)
                                .cloned()
                                .unwrap_or_else(|| cw_core::utils::hash_string(y.parent))
                        });
                        row.flags = y.flags;
                        row.content_flags = y.content_flags;
                        row.streaming_min = y.streaming_extents_min;
                        row.streaming_max = y.streaming_extents_max;
                        row.entities_min = y.entities_extents_min;
                        row.entities_max = y.entities_extents_max;
                        row.entities = y.entities.len();
                        row.mlo_instances = y.mlo_entities.len();
                        row.occluders = y.box_occluders.len() + y.occlude_models.len();
                        row.grass_batches = y.grass_instance_batches.len();
                        row.lod_lights = y.lod_lights.hash.len();
                        for e in &y.entities {
                            *row.lods.entry(lod_label(e.def.lod_level)).or_default() += 1;
                        }
                        let positions: Vec<[f32; 3]> =
                            y.entities.iter().map(|e| e.def.position).collect();
                        if row.streamed {
                            row.issues = ymap_issues(&row, &positions, &rules);
                            if let (Some(v), Some(p)) = (&vanilla, &row.parent) {
                                if !v.contains(p) && !loose_ymaps.contains(p) {
                                    row.issues.push("orphan-parent".into());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        totals.unreadable += 1;
                        row.problem = Some(e.clone());
                    }
                }
                ymaps.push(row);
            }
            Scanned::Ytyp(path, bytes, r) => {
                let (resource, streamed) = resource_of(&root, path);
                let mut row = YtypRow {
                    file: rel(path),
                    resource,
                    name: stem_lower(path),
                    streamed,
                    bytes: *bytes,
                    archetypes: 0,
                    mlos: Vec::new(),
                    issues: Vec::new(),
                    problem: None,
                };
                match r {
                    Ok(t) => {
                        row.archetypes = t.archetypes.len();
                        for a in &t.archetypes {
                            if streamed {
                                archetype_owners
                                    .entry(a.hash())
                                    .or_default()
                                    .push(row.file.clone());
                            }
                            if let Archetype::Mlo(m) = a {
                                let mlo = check_mlo(m, &names, &rules, &allowed_tc);
                                if streamed {
                                    row.issues.extend(mlo.issues.iter().cloned());
                                }
                                row.mlos.push(mlo);
                            }
                        }
                        row.issues.sort();
                        row.issues.dedup();
                    }
                    Err(e) => {
                        totals.unreadable += 1;
                        row.problem = Some(e.clone());
                    }
                }
                ytyps.push(row);
            }
        }
    }

    let duplicates = |rows: Vec<(String, String)>| -> Vec<Duplicate> {
        let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, file) in rows {
            by.entry(name).or_default().push(file);
        }
        by.into_iter()
            .filter(|(_, f)| f.len() > 1)
            .map(|(name, files)| Duplicate { name, files })
            .collect()
    };
    let duplicate_ymaps = duplicates(
        ymaps
            .iter()
            .filter(|y| y.streamed)
            .map(|y| (y.name.clone(), y.file.clone()))
            .collect(),
    );
    let duplicate_ytyps = duplicates(
        ytyps
            .iter()
            .filter(|y| y.streamed)
            .map(|y| (y.name.clone(), y.file.clone()))
            .collect(),
    );
    let duplicate_archetypes: Vec<Duplicate> = archetype_owners
        .into_iter()
        .filter_map(|(hash, mut files)| {
            files.sort();
            files.dedup();
            (files.len() > 1).then(|| Duplicate {
                name: names
                    .get(&hash)
                    .cloned()
                    .unwrap_or_else(|| cw_core::utils::hash_string(hash)),
                files,
            })
        })
        .collect();

    let dup_ymap: HashSet<&str> = duplicate_ymaps.iter().map(|d| d.name.as_str()).collect();
    for y in &mut ymaps {
        if y.streamed && dup_ymap.contains(y.name.as_str()) {
            y.issues.push("duplicate-name".into());
        }
    }
    let dup_ytyp: HashSet<&str> = duplicate_ytyps.iter().map(|d| d.name.as_str()).collect();
    let dup_arch_files: HashSet<&str> = duplicate_archetypes
        .iter()
        .flat_map(|d| d.files.iter().map(|f| f.as_str()))
        .collect();
    for t in &mut ytyps {
        if t.streamed && dup_ytyp.contains(t.name.as_str()) {
            t.issues.push("duplicate-name".into());
        }
        if dup_arch_files.contains(t.file.as_str()) {
            t.issues.push("duplicate-archetype".into());
        }
    }

    for y in &ymaps {
        totals.ymaps += 1;
        if y.streamed {
            totals.ymaps_streamed += 1;
            *totals
                .ymaps_by_resource
                .entry(y.resource.clone())
                .or_default() += 1;
            if y.vanilla_override == Some(true) {
                totals.vanilla_overrides += 1;
            }
        }
        for i in &y.issues {
            *totals.by_issue.entry(i.clone()).or_default() += 1;
        }
    }
    for t in &ytyps {
        totals.ytyps += 1;
        totals.mlos += t.mlos.len();
        for i in &t.issues {
            *totals.by_issue.entry(i.clone()).or_default() += 1;
        }
    }

    println!(
        "ymaps {} (streamed {}, vanilla overrides {})  ytyps {}  mlos {}  unreadable {}",
        totals.ymaps,
        totals.ymaps_streamed,
        if vanilla.is_some() {
            totals.vanilla_overrides.to_string()
        } else {
            "?".into()
        },
        totals.ytyps,
        totals.mlos,
        totals.unreadable
    );
    println!(
        "duplicate ymaps {}  duplicate ytyps {}  duplicate archetypes {}",
        duplicate_ymaps.len(),
        duplicate_ytyps.len(),
        duplicate_archetypes.len()
    );
    for (i, n) in &totals.by_issue {
        println!("  {i}: {n}");
    }

    let report = Report {
        generated_with: "cw-core map_audit 0.1",
        root: root.display().to_string(),
        game_read: vanilla.is_some(),
        rules,
        totals,
        duplicate_ymaps,
        duplicate_ytyps,
        duplicate_archetypes,
        ymaps,
        ytyps,
    };
    std::fs::write(&out_path, serde_json::to_vec(&report).unwrap()).expect("write report");
    eprintln!("wrote {}", out_path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(lods: &[(&str, usize)], smin: [f32; 3], smax: [f32; 3]) -> YmapRow {
        YmapRow {
            file: "x.ymap".into(),
            resource: "r".into(),
            name: "x".into(),
            streamed: true,
            bytes: 0,
            vanilla_override: None,
            parent: None,
            flags: 0,
            content_flags: 0,
            streaming_min: smin,
            streaming_max: smax,
            entities_min: smin,
            entities_max: smax,
            entities: lods.iter().map(|l| l.1).sum(),
            lods: lods.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            mlo_instances: 0,
            occluders: 0,
            grass_batches: 0,
            lod_lights: 0,
            issues: Vec::new(),
            problem: None,
        }
    }

    #[test]
    fn hd_and_lod_together_is_mixed() {
        let r = row(&[("hd", 10), ("lod", 2)], [0.0; 3], [100.0; 3]);
        assert!(ymap_issues(&r, &[], &Rules::default()).contains(&"mixed-lod".to_string()));
        let r = row(&[("hd", 10), ("orphan-hd", 2)], [0.0; 3], [100.0; 3]);
        assert!(!ymap_issues(&r, &[], &Rules::default()).contains(&"mixed-lod".to_string()));
    }

    #[test]
    fn wide_hd_ymap_is_huge_but_a_lod_one_is_not() {
        let wide = ([-1000.0, 0.0, 0.0], [1000.0, 10.0, 10.0]);
        let r = row(&[("hd", 10)], wide.0, wide.1);
        assert!(ymap_issues(&r, &[], &Rules::default()).contains(&"huge-extents".to_string()));
        let r = row(&[("slod1", 10)], wide.0, wide.1);
        assert!(!ymap_issues(&r, &[], &Rules::default()).contains(&"huge-extents".to_string()));
    }

    #[test]
    fn tiny_and_stale() {
        let r = row(&[("hd", 2)], [0.0; 3], [10.0; 3]);
        let issues = ymap_issues(&r, &[[50.0, 5.0, 5.0]], &Rules::default());
        assert!(issues.contains(&"tiny-ymap".to_string()));
        assert!(issues.contains(&"extents-stale".to_string()));
        let issues = ymap_issues(&r, &[[12.0, 5.0, 5.0]], &Rules::default());
        assert!(!issues.contains(&"extents-stale".to_string()));
    }

    #[test]
    fn portal_kind_from_attached_names() {
        assert_eq!(
            classify_portal(&["v_ilev_fh_door01".into()]),
            PortalKind::Door
        );
        assert_eq!(
            classify_portal(&["prop_window_glass".into()]),
            PortalKind::Window
        );
        assert_eq!(classify_portal(&["prop_plant".into()]), PortalKind::Other);
        assert_eq!(classify_portal(&[]), PortalKind::Other);
    }

    #[test]
    fn flag_bits_are_a_subset_check() {
        assert!(has_bits(96, 96));
        assert!(has_bits(96 | 1, 96));
        assert!(!has_bits(64, 96));
        assert!(has_bits(0, 0));
    }

    #[test]
    fn stream_folder_decides_resource_and_streamed() {
        let root = Path::new("R");
        let (res, s) = resource_of(root, Path::new("R/[a]/Map/stream/Sub/x.ymap"));
        assert_eq!(res, "[a]/Map");
        assert!(s);
        let (res, s) = resource_of(root, Path::new("R/[a]/Map/disabled/x.ymap"));
        assert_eq!(res, "[a]/Map/disabled");
        assert!(!s);
    }

    #[test]
    fn rules_file_overrides_only_what_it_names() {
        let r: Rules = serde_json::from_str(r#"{"roomFlags": 32}"#).unwrap();
        assert_eq!(r.room_flags, 32);
        assert_eq!(r.door_portal_flags, 64);
        assert_eq!(r.room_timecycles, vec!["mp_studio_lo".to_string()]);
    }
}
