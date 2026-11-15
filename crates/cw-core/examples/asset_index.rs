//! Offline index of the loose streaming models of a FiveM server: polygons per
//! LOD, vertices, LOD distances and weight in bytes of every `.yft`, `.ydr` and
//! `.ydd` drawable, joined with its `.ytd`. The in-game debug resource reads it
//! to show what the targeted entity costs, because no native reports that.
//!
//! ```text
//! cargo run --release --example asset_index -- <resources dir> [--out asset-index.json]
//!     [--pretty] [--min] [--only-resources a,b] [--generated-at 2026-01-01T00:00:00Z]
//! ```
//!
//! Output contract, rules and limits: `docs/vehicle-tools.md` section 7 and
//! `docs/asset-index.md`. Read-only: nothing under the resources folder is touched.

use std::collections::{BTreeMap, HashMap};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use cw_core::filetype::FileKind;
use cw_core::loose::{
    flags, is_hi, load_loose, model_of, resource_of, walk, Cause, Fail, FlagInput,
};
use cw_core::mesh::MeshBundle;
use cw_core::resources::{
    read_named_drawables, read_named_fragments, read_textures, NamedDrawable,
};
use cw_core::utils::jenk::gen_hash;
use rayon::prelude::*;
use serde::Serialize;

/// Version of the JSON contract. Bump only with the consumer.
const VERSION: u32 = 1;

// ---------------------------------------------------------------- contract

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
struct Tris {
    high: usize,
    med: usize,
    low: usize,
    vlow: usize,
}

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
struct Verts {
    high: usize,
}

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
struct Bytes {
    model: u64,
    ytd: u64,
    total: u64,
}

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Textures {
    count: usize,
    max_size: u16,
}

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
struct LodDist {
    high: f64,
    med: f64,
    low: f64,
    vlow: f64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Model {
    hash: u32,
    resource: String,
    kind: &'static str,
    tris: Tris,
    verts: Verts,
    bytes: Bytes,
    textures: Textures,
    lod_dist: LodDist,
    flags: Vec<String>,
}

/// One candidate of a name that more than one resource provides.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    resource: String,
    path: String,
    kind: &'static str,
    tris_high: usize,
    bytes_total: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    version: u32,
    generated_at: String,
    source: String,
    models: BTreeMap<String, Model>,
    /// Names provided more than once. The first candidate is the one kept in
    /// `models`; the rule is in the docs.
    duplicates: BTreeMap<String, Vec<Candidate>>,
}

// ---------------------------------------------------------------- scanning

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Yft,
    Ydr,
    Ydd,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Yft => "yft",
            Kind::Ydr => "ydr",
            Kind::Ydd => "ydd",
        }
    }
}

/// One drawable, reduced to what the index keeps.
#[derive(Clone, Debug, Default)]
struct Draw {
    name: String,
    hash: u32,
    /// High, medium, low, very low.
    tris: [usize; 4],
    verts_high: usize,
    lod_dist: [f32; 4],
}

#[derive(Clone, Debug, Default)]
struct TexInfo {
    count: usize,
    max_dim: u16,
}

enum Payload {
    Model(Kind, Result<Vec<Draw>, Fail>),
    Ytd(Result<TexInfo, Fail>),
}

struct Scanned {
    path: PathBuf,
    bytes: u64,
    payload: Payload,
}

fn summarize(nd: &NamedDrawable) -> Draw {
    let mut d = Draw {
        name: nd.name.to_lowercase(),
        hash: nd.hash,
        lod_dist: nd.drawable.lod_distances,
        ..Draw::default()
    };
    for bundle in MeshBundle::all(&nd.drawable) {
        let i = match bundle.lod {
            "high" => 0,
            "medium" => 1,
            "low" => 2,
            _ => 3,
        };
        d.tris[i] += bundle.triangle_count;
        if i == 0 {
            d.verts_high += bundle.vertex_count;
        }
    }
    d
}

/// A fragment is one model: its main drawable plus the drawables of its parts
/// (wheels, doors). The parts' `drawable2` is the damaged variant of the same
/// part, never drawn together with `drawable1`, so it is left out; `vehicle_audit`
/// counts it (and every physics LOD), which is why its totals can be higher.
fn collapse_fragment(parts: &[NamedDrawable]) -> Vec<Draw> {
    let mut out = Draw::default();
    let mut lod_from_main = false;
    // `lod0` is the first physics LOD the fragment has; `lod1` and `lod2` repeat
    // the same parts at another physics detail and would count them again.
    let counted = |nd: &&NamedDrawable| {
        !nd.name.ends_with(".drawable2")
            && !(nd.name.starts_with("lod") && !nd.name.starts_with("lod0."))
    };
    for nd in parts.iter().filter(counted) {
        let d = summarize(nd);
        for (acc, t) in out.tris.iter_mut().zip(d.tris) {
            *acc += t;
        }
        out.verts_high += d.verts_high;
        // LOD distances: the main drawable's, else the first part that has any.
        if !lod_from_main && d.lod_dist.iter().any(|v| *v > 0.0) {
            out.lod_dist = d.lod_dist;
            lod_from_main = nd.name == "drawable";
        }
    }
    vec![out]
}

fn scan_model(path: &Path, kind: Kind) -> Result<Vec<Draw>, Fail> {
    let (data, system) = load_loose(path)?;
    let corrupt = |e: cw_core::CwError| (Cause::Corrupt, e.to_string());
    match kind {
        Kind::Yft => {
            let parts = read_named_fragments(FileKind::Fragment, &data, system).map_err(corrupt)?;
            Ok(collapse_fragment(&parts))
        }
        Kind::Ydr => {
            let parts = read_named_drawables(FileKind::Model, &data, system).map_err(corrupt)?;
            Ok(parts.iter().map(summarize).collect())
        }
        Kind::Ydd => {
            let parts =
                read_named_drawables(FileKind::ModelDictionary, &data, system).map_err(corrupt)?;
            Ok(parts.iter().map(summarize).collect())
        }
    }
}

fn scan_ytd(path: &Path) -> Result<TexInfo, Fail> {
    let (data, system) = load_loose(path)?;
    let dict = read_textures(FileKind::Texture, &data, system)
        .map_err(|e| (Cause::Corrupt, e.to_string()))?;
    Ok(TexInfo {
        count: dict.textures.len(),
        max_dim: dict
            .textures
            .iter()
            .map(|t| t.width.max(t.height))
            .max()
            .unwrap_or(0),
    })
}

fn kind_of(path: &Path) -> Option<Option<Kind>> {
    match path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "yft" => Some(Some(Kind::Yft)),
        "ydr" => Some(Some(Kind::Ydr)),
        "ydd" => Some(Some(Kind::Ydd)),
        "ytd" => Some(None),
        _ => None,
    }
}

fn scan_file(path: &Path) -> Option<Scanned> {
    let kind = kind_of(path)?;
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    fn panicked<T>() -> Result<T, Fail> {
        Err((Cause::Corrupt, "parser panicked".to_string()))
    }
    let payload = match kind {
        Some(k) => Payload::Model(
            k,
            catch_unwind(AssertUnwindSafe(|| scan_model(path, k))).unwrap_or_else(|_| panicked()),
        ),
        None => Payload::Ytd(
            catch_unwind(AssertUnwindSafe(|| scan_ytd(path))).unwrap_or_else(|_| panicked()),
        ),
    };
    Some(Scanned {
        path: path.to_path_buf(),
        bytes,
        payload,
    })
}

// ---------------------------------------------------------------- building

/// Everything the `.ytd` files of one model add up to.
#[derive(Default, Clone, Debug)]
struct YtdAgg {
    files: usize,
    bytes: u64,
    textures: usize,
    max_dim: u16,
    corrupt: bool,
    unsupported: bool,
}

/// `.ytd` files by (resource, model name). `adder.ytd` and `adder+hi.ytd` both
/// land on `adder`.
fn collect_ytds(root: &Path, scanned: &[Scanned]) -> HashMap<(String, String), YtdAgg> {
    let mut map: HashMap<(String, String), YtdAgg> = HashMap::new();
    for s in scanned {
        let Payload::Ytd(r) = &s.payload else {
            continue;
        };
        let agg = map
            .entry((resource_of(root, &s.path), model_of(&s.path)))
            .or_default();
        agg.files += 1;
        agg.bytes += s.bytes;
        match r {
            Ok(t) => {
                agg.textures += t.count;
                agg.max_dim = agg.max_dim.max(t.max_dim);
            }
            Err((Cause::Corrupt, _)) => agg.corrupt = true,
            Err((Cause::UnsupportedFormat, _)) => agg.unsupported = true,
        }
    }
    map
}

fn round2(v: f32) -> f64 {
    if v.is_finite() {
        (f64::from(v) * 100.0).round() / 100.0
    } else {
        0.0
    }
}

/// One model, from its drawable and the files it came from.
struct Built<'a> {
    hash: u32,
    resource: &'a str,
    kind: Kind,
    draw: Draw,
    model_files: usize,
    model_bytes: u64,
    corrupt: bool,
    unsupported: bool,
    ytd: Option<&'a YtdAgg>,
}

fn build_model(b: Built) -> Model {
    let ytd = b.ytd.cloned().unwrap_or_default();
    let tris = Tris {
        high: b.draw.tris[0],
        med: b.draw.tris[1],
        low: b.draw.tris[2],
        vlow: b.draw.tris[3],
    };
    let corrupt = b.corrupt || ytd.corrupt;
    let unsupported = b.unsupported || ytd.unsupported;
    let flags = flags(&FlagInput {
        yft_files: b.model_files,
        ytd_files: ytd.files,
        ytd_bytes: ytd.bytes,
        textures: ytd.textures,
        max_texture_dim: ytd.max_dim,
        tris: b.draw.tris,
        corrupt,
        unsupported,
        no_problems: !corrupt && !unsupported,
        problems_only_ytd: !b.corrupt && !b.unsupported,
    });
    Model {
        hash: b.hash,
        resource: b.resource.to_string(),
        kind: b.kind.label(),
        tris,
        verts: Verts {
            high: b.draw.verts_high,
        },
        bytes: Bytes {
            model: b.model_bytes,
            ytd: ytd.bytes,
            total: b.model_bytes + ytd.bytes,
        },
        textures: Textures {
            count: ytd.textures,
            max_size: ytd.max_dim,
        },
        lod_dist: LodDist {
            high: round2(b.draw.lod_dist[0]),
            med: round2(b.draw.lod_dist[1]),
            low: round2(b.draw.lod_dist[2]),
            vlow: round2(b.draw.lod_dist[3]),
        },
        flags,
    }
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

struct Cand {
    name: String,
    path: String,
    model: Model,
}

/// A `.yft` or `.ydr`: one model named after the file, with an optional `_hi`
/// companion. High detail is whichever of the two has more triangles (the
/// companion is the one drawn up close); the other LODs come from the base file.
fn model_files_to_cand(
    root: &Path,
    base: Option<&Scanned>,
    hi: Option<&Scanned>,
    kind: Kind,
    name: &str,
    resource: &str,
    ytd: Option<&YtdAgg>,
) -> Cand {
    let ok = |s: Option<&Scanned>| match s.map(|s| &s.payload) {
        Some(Payload::Model(_, Ok(d))) => d.first().cloned(),
        _ => None,
    };
    let (base_draw, hi_draw) = (ok(base), ok(hi));
    let mut draw = base_draw
        .clone()
        .or_else(|| hi_draw.clone())
        .unwrap_or_default();
    if let (Some(_), Some(h)) = (&base_draw, &hi_draw) {
        if h.tris[0] > draw.tris[0] {
            draw.tris[0] = h.tris[0];
            draw.verts_high = h.verts_high;
        }
    }
    let files: Vec<&Scanned> = [base, hi].into_iter().flatten().collect();
    let failed = |c: Cause| {
        files
            .iter()
            .any(|s| matches!(&s.payload, Payload::Model(_, Err((cause, _))) if *cause == c))
    };
    let first = files[0];
    let model = build_model(Built {
        hash: gen_hash(name),
        resource,
        kind,
        draw,
        model_files: files.len(),
        model_bytes: files.iter().map(|s| s.bytes).sum(),
        corrupt: failed(Cause::Corrupt),
        unsupported: failed(Cause::UnsupportedFormat),
        ytd,
    });
    Cand {
        name: name.to_string(),
        path: rel_path(root, &first.path),
        model,
    }
}

#[derive(Default)]
struct Stats {
    files: usize,
    model_files: usize,
    ytd_files: usize,
    failed_models: Vec<(String, String)>,
    failed_ytds: usize,
}

fn build_candidates(root: &Path, scanned: &[Scanned], stats: &mut Stats) -> Vec<Cand> {
    let ytds = collect_ytds(root, scanned);
    let mut cands: Vec<Cand> = Vec::new();

    // `.yft` / `.ydr`: grouped by (folder, kind, name) so `x.yft` and `x_hi.yft` meet.
    type Group<'a> = (Option<&'a Scanned>, Option<&'a Scanned>);
    let mut groups: BTreeMap<(String, &'static str, String), Group> = BTreeMap::new();

    for s in scanned {
        match &s.payload {
            Payload::Ytd(r) => {
                stats.ytd_files += 1;
                if r.is_err() {
                    stats.failed_ytds += 1;
                }
            }
            Payload::Model(kind, r) => {
                stats.model_files += 1;
                let name = model_of(&s.path);
                let resource = resource_of(root, &s.path);
                if let Err((_, msg)) = r {
                    stats
                        .failed_models
                        .push((rel_path(root, &s.path), msg.clone()));
                }
                if *kind == Kind::Ydd {
                    // A dictionary: one entry per drawable, named after it.
                    let ytd = ytds.get(&(resource.clone(), name.clone()));
                    // A dictionary that fails has no drawable names to key
                    // entries by; the failure is counted in the stats instead.
                    if let Ok(draws) = r {
                        for d in draws {
                            cands.push(Cand {
                                name: d.name.clone(),
                                path: rel_path(root, &s.path),
                                model: build_model(Built {
                                    hash: d.hash,
                                    resource: &resource,
                                    kind: Kind::Ydd,
                                    draw: d.clone(),
                                    model_files: 1,
                                    model_bytes: s.bytes,
                                    corrupt: false,
                                    unsupported: false,
                                    ytd,
                                }),
                            });
                        }
                    }
                    continue;
                }
                let dir = rel_path(root, s.path.parent().unwrap_or(root));
                let slot = groups.entry((dir, kind.label(), name)).or_default();
                if is_hi(&s.path) {
                    slot.1 = Some(s);
                } else {
                    slot.0 = Some(s);
                }
            }
        }
    }

    for ((_, kind_label, name), (base, hi)) in groups {
        let kind = if kind_label == "yft" {
            Kind::Yft
        } else {
            Kind::Ydr
        };
        let first = base.or(hi).expect("a group has a file");
        let resource = resource_of(root, &first.path);
        let ytd = ytds.get(&(resource.clone(), name.clone()));
        cands.push(model_files_to_cand(
            root, base, hi, kind, &name, &resource, ytd,
        ));
    }
    cands
}

/// Collapse candidates into the index. A name provided more than once keeps the
/// candidate whose (resource, path) sorts first, and every candidate (that one
/// included) goes to `duplicates`: which one FiveM loads depends on the start
/// order in the server config, which is not read here.
fn assemble(cands: Vec<Cand>, generated_at: String, source: String) -> Index {
    let mut by_name: BTreeMap<String, Vec<Cand>> = BTreeMap::new();
    for c in cands {
        by_name.entry(c.name.clone()).or_default().push(c);
    }
    let mut models = BTreeMap::new();
    let mut duplicates = BTreeMap::new();
    for (name, mut list) in by_name {
        list.sort_by(|a, b| {
            (&a.model.resource, &a.path, a.model.kind).cmp(&(
                &b.model.resource,
                &b.path,
                b.model.kind,
            ))
        });
        if list.len() > 1 {
            duplicates.insert(
                name.clone(),
                list.iter()
                    .map(|c| Candidate {
                        resource: c.model.resource.clone(),
                        path: c.path.clone(),
                        kind: c.model.kind,
                        tris_high: c.model.tris.high,
                        bytes_total: c.model.bytes.total,
                    })
                    .collect(),
            );
        }
        let winner = list.into_iter().next().expect("non-empty");
        models.insert(name, winner.model);
    }
    Index {
        version: VERSION,
        generated_at,
        source,
        models,
        duplicates,
    }
}

// ---------------------------------------------------------------- misc

/// `2026-11-13T20:05:00Z` from a Unix time. Civil-from-days, no date crate.
fn iso8601(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

struct Opts {
    root: PathBuf,
    out: PathBuf,
    pretty: bool,
    only: Vec<String>,
    generated_at: Option<String>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Opts, String> {
    let mut o = Opts {
        root: PathBuf::from(r"E:\FiveM\Astra\resources"),
        out: PathBuf::from("asset-index.json"),
        pretty: false,
        only: Vec::new(),
        generated_at: None,
    };
    let mut positional = false;
    while let Some(a) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{what} needs a value"));
        match a.as_str() {
            "--out" => o.out = PathBuf::from(value("--out")?),
            "--pretty" => o.pretty = true,
            // Compact is already the default; `--min` is kept so a script can say it.
            "--min" => o.pretty = false,
            "--only-resources" => {
                o.only = value("--only-resources")?
                    .split(',')
                    .map(|s| s.trim().replace('\\', "/").to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "--generated-at" => o.generated_at = Some(value("--generated-at")?),
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with("--") => return Err(format!("unknown flag {s}")),
            s if !positional => {
                o.root = PathBuf::from(s);
                positional = true;
            }
            s => return Err(format!("unexpected argument {s}")),
        }
    }
    Ok(o)
}

/// `--only-resources` keeps a file whose resource path starts with any entry.
fn wanted(only: &[String], resource: &str) -> bool {
    only.is_empty() || {
        let r = resource.to_lowercase();
        only.iter().any(|p| r.starts_with(p.as_str()))
    }
}

fn run(o: &Opts) -> (Index, Stats) {
    let mut files = Vec::new();
    walk(&o.root, &["yft", "ydr", "ydd", "ytd"], &mut files);
    files.retain(|p| wanted(&o.only, &resource_of(&o.root, p)));
    let scanned: Vec<Scanned> = files.par_iter().filter_map(|p| scan_file(p)).collect();
    let mut stats = Stats {
        files: scanned.len(),
        ..Stats::default()
    };
    let cands = build_candidates(&o.root, &scanned, &mut stats);
    let source = o
        .root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "resources".into());
    let generated_at = o.generated_at.clone().unwrap_or_else(|| {
        iso8601(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        )
    });
    (assemble(cands, generated_at, source), stats)
}

fn main() {
    let o = match parse_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}\n");
            }
            eprintln!(
                "asset_index <resources dir> [--out asset-index.json] [--pretty] [--min]\n  \
                 [--only-resources a,b] [--generated-at ISO]"
            );
            std::process::exit(if msg.is_empty() { 0 } else { 2 });
        }
    };
    let started = Instant::now();
    let (index, stats) = run(&o);
    let json = if o.pretty {
        serde_json::to_vec_pretty(&index)
    } else {
        serde_json::to_vec(&index)
    }
    .expect("serialize");
    if let Some(dir) = o.out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).expect("create output folder");
    }
    std::fs::write(&o.out, &json).expect("write index");

    println!(
        "models {}  duplicates {}  files {} (model {}, ytd {})  unreadable: model {}, ytd {}",
        index.models.len(),
        index.duplicates.len(),
        stats.files,
        stats.model_files,
        stats.ytd_files,
        stats.failed_models.len(),
        stats.failed_ytds
    );
    println!(
        "{:.1} MiB -> {}  in {:.1?}",
        json.len() as f64 / 1_048_576.0,
        o.out.display(),
        started.elapsed()
    );
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(tris: [usize; 4]) -> Draw {
        Draw {
            tris,
            verts_high: tris[0] / 2,
            lod_dist: [100.0, 200.0, 300.0, 33.3],
            ..Draw::default()
        }
    }

    fn model_scan(path: &str, kind: Kind, bytes: u64, tris: [usize; 4]) -> Scanned {
        Scanned {
            path: PathBuf::from(path),
            bytes,
            payload: Payload::Model(kind, Ok(vec![draw(tris)])),
        }
    }

    fn ytd_scan(path: &str, bytes: u64, count: usize, max_dim: u16) -> Scanned {
        Scanned {
            path: PathBuf::from(path),
            bytes,
            payload: Payload::Ytd(Ok(TexInfo { count, max_dim })),
        }
    }

    fn index_of(scanned: Vec<Scanned>) -> Index {
        let root = Path::new("/r");
        let mut stats = Stats::default();
        let cands = build_candidates(root, &scanned, &mut stats);
        assemble(cands, "T".into(), "resources".into())
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(1_794_600_300), "2026-11-13T20:05:00Z");
    }

    #[test]
    fn joaat_is_the_model_hash_and_serializes_unsigned() {
        // GetHashKey("adder") is -1216765807 as a signed Lua integer.
        assert_eq!(gen_hash("adder"), 3_078_201_489);
        assert_eq!(gen_hash("adder") as i32, -1_216_765_807);
        let i = index_of(vec![model_scan(
            "/r/[a]/cars/stream/Adder.yft",
            Kind::Yft,
            10,
            [10, 5, 2, 1],
        )]);
        assert_eq!(i.models["adder"].hash, 3_078_201_489);
        assert!(serde_json::to_string(&i)
            .unwrap()
            .contains("\"hash\":3078201489"));
    }

    #[test]
    fn ytd_rule_is_same_name_plus_hi_in_the_same_resource() {
        let i = index_of(vec![
            model_scan(
                "/r/[a]/cars/stream/adder.yft",
                Kind::Yft,
                1_000,
                [10, 5, 2, 1],
            ),
            ytd_scan("/r/[a]/cars/stream/adder.ytd", 2_000, 3, 1024),
            ytd_scan("/r/[a]/cars/stream/adder+hi.ytd", 4_000, 2, 2048),
            // Same name, other resource: not this model's.
            ytd_scan("/r/[b]/other/stream/adder.ytd", 9_999, 7, 4096),
        ]);
        let m = &i.models["adder"];
        assert_eq!(
            m.bytes,
            Bytes {
                model: 1_000,
                ytd: 6_000,
                total: 7_000
            }
        );
        assert_eq!(
            m.textures,
            Textures {
                count: 5,
                max_size: 2048
            }
        );
        assert_eq!(m.resource, "[a]/cars");
        assert_eq!(m.kind, "yft");
    }

    #[test]
    fn hi_companion_raises_high_and_adds_its_weight() {
        let i = index_of(vec![
            model_scan(
                "/r/[a]/c/stream/car.yft",
                Kind::Yft,
                1_000,
                [30_000, 15_000, 8_000, 1_000],
            ),
            model_scan(
                "/r/[a]/c/stream/car_hi.yft",
                Kind::Yft,
                2_500,
                [90_000, 0, 0, 0],
            ),
        ]);
        let m = &i.models["car"];
        assert_eq!(
            m.tris,
            Tris {
                high: 90_000,
                med: 15_000,
                low: 8_000,
                vlow: 1_000
            }
        );
        assert_eq!(m.verts.high, 45_000);
        assert_eq!(m.bytes.model, 3_500);
        // Lower than the base: the base wins.
        let i = index_of(vec![
            model_scan("/r/[a]/c/stream/car.yft", Kind::Yft, 1, [30_000, 1, 1, 1]),
            model_scan("/r/[a]/c/stream/car_hi.yft", Kind::Yft, 1, [10, 0, 0, 0]),
        ]);
        assert_eq!(i.models["car"].tris.high, 30_000);
    }

    #[test]
    fn lod_distances_are_rounded_and_flags_match_the_audit() {
        let i = index_of(vec![
            model_scan("/r/[a]/c/stream/big.ydr", Kind::Ydr, 1, [120_000, 0, 0, 0]),
            ytd_scan("/r/[a]/c/stream/big.ytd", 20 * 1_048_576, 1, 512),
        ]);
        let m = &i.models["big"];
        assert_eq!(
            m.lod_dist,
            LodDist {
                high: 100.0,
                med: 200.0,
                low: 300.0,
                vlow: 33.3
            }
        );
        assert_eq!(
            m.flags,
            [
                "low-res",
                "heavy-tris",
                "ytd-over-16mb",
                "no-lods",
                "missing-lods"
            ]
        );
    }

    #[test]
    fn failed_files_are_listed_with_the_audit_flag_not_dropped() {
        let bad = |p: &str, c: Cause| Scanned {
            path: PathBuf::from(p),
            bytes: 7,
            payload: Payload::Model(Kind::Yft, Err((c, "x".into()))),
        };
        let i = index_of(vec![
            bad("/r/[a]/c/stream/gen9.yft", Cause::UnsupportedFormat),
            bad("/r/[a]/c/stream/broken.yft", Cause::Corrupt),
        ]);
        assert!(i.models["gen9"]
            .flags
            .contains(&"unsupported-format".to_string()));
        assert!(i.models["broken"].flags.contains(&"damaged".to_string()));
        assert_eq!(i.models["broken"].tris, Tris::default());
    }

    #[test]
    fn dictionaries_give_one_entry_per_drawable() {
        let s = Scanned {
            path: PathBuf::from("/r/[a]/peds/stream/mp_m^uppr_000.ydd"),
            bytes: 500,
            payload: Payload::Model(
                Kind::Ydd,
                Ok(vec![
                    Draw {
                        name: "uppr_000_u".into(),
                        hash: 11,
                        ..draw([10, 0, 0, 0])
                    },
                    Draw {
                        name: "uppr_001_u".into(),
                        hash: 12,
                        ..draw([20, 0, 0, 0])
                    },
                ]),
            ),
        };
        let i = index_of(vec![s]);
        assert_eq!(i.models["uppr_000_u"].hash, 11);
        assert_eq!(i.models["uppr_001_u"].tris.high, 20);
        assert_eq!(i.models["uppr_001_u"].kind, "ydd");
        // The whole dictionary streams as a unit.
        assert_eq!(i.models["uppr_001_u"].bytes.model, 500);
    }

    #[test]
    fn duplicates_pick_the_first_resource_and_list_all() {
        let i = index_of(vec![
            model_scan("/r/[b]/z/stream/dup.yft", Kind::Yft, 1, [200, 0, 0, 0]),
            model_scan("/r/[a]/z/stream/dup.yft", Kind::Yft, 1, [100, 0, 0, 0]),
            model_scan("/r/[a]/z/stream/solo.yft", Kind::Yft, 1, [1, 0, 0, 0]),
        ]);
        assert_eq!(i.models["dup"].resource, "[a]/z");
        assert_eq!(i.models["dup"].tris.high, 100);
        let d = &i.duplicates["dup"];
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].resource, "[a]/z");
        assert_eq!(d[1].resource, "[b]/z");
        assert!(!i.duplicates.contains_key("solo"));
    }

    #[test]
    fn output_is_stable_and_sorted() {
        let make = |order: [&str; 3]| {
            let scanned = order
                .iter()
                .map(|n| {
                    model_scan(
                        &format!("/r/[a]/c/stream/{n}.ydr"),
                        Kind::Ydr,
                        1,
                        [1, 0, 0, 0],
                    )
                })
                .collect();
            serde_json::to_string_pretty(&index_of(scanned)).unwrap()
        };
        let a = make(["zz", "aa", "mm"]);
        assert_eq!(a, make(["mm", "zz", "aa"]));
        let (aa, mm, zz) = (
            a.find("\"aa\"").unwrap(),
            a.find("\"mm\"").unwrap(),
            a.find("\"zz\"").unwrap(),
        );
        assert!(aa < mm && mm < zz);
        let v: serde_json::Value = serde_json::from_str(&a).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["models"]["aa"]["lodDist"]["vlow"], 33.3);
        assert_eq!(v["models"]["aa"]["textures"]["maxSize"], 0);
    }

    #[test]
    fn only_resources_is_a_case_insensitive_prefix() {
        let only = vec!["[astra]/[coches".to_string()];
        assert!(wanted(&only, "[Astra]/[cochesfacciones]/Facciones"));
        assert!(!wanted(&only, "[standalone]/x"));
        assert!(wanted(&[], "anything"));
    }

    #[test]
    fn arguments() {
        let args = |v: &[&str]| parse_args(v.iter().map(|s| s.to_string()));
        let o = args(&[
            "res",
            "--out",
            "o.json",
            "--pretty",
            "--only-resources",
            "a/b, c",
        ])
        .unwrap();
        assert_eq!(o.root, PathBuf::from("res"));
        assert!(o.pretty);
        assert_eq!(o.only, ["a/b", "c"]);
        assert!(args(&["--nope"]).is_err());
        assert!(args(&["--out"]).is_err());
    }

    /// Layout of the synthetic drawable of `resources::drawable`'s own tests:
    /// one model, one geometry, one triangle, LOD high distance 100.
    fn synthetic_ydr() -> Vec<u8> {
        const BASE: u64 = 0x5000_0000;
        let mut d = vec![0u8; 0x400];
        let u16_ =
            |d: &mut Vec<u8>, at: usize, v: u16| d[at..at + 2].copy_from_slice(&v.to_le_bytes());
        let u32_ =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let u64_ =
            |d: &mut Vec<u8>, at: usize, v: u64| d[at..at + 8].copy_from_slice(&v.to_le_bytes());
        let f32_ =
            |d: &mut Vec<u8>, at: usize, v: f32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        u64_(&mut d, 0x50, BASE + 0x100);
        f32_(&mut d, 0x70, 100.0);
        u64_(&mut d, 0xA8, BASE + 0x350);
        u64_(&mut d, 0x100, BASE + 0x110);
        u16_(&mut d, 0x108, 1);
        u16_(&mut d, 0x10A, 1);
        u64_(&mut d, 0x110, BASE + 0x120);
        u64_(&mut d, 0x128, BASE + 0x160);
        u16_(&mut d, 0x130, 1);
        u64_(&mut d, 0x140, BASE + 0x170);
        u64_(&mut d, 0x160, BASE + 0x180);
        u16_(&mut d, 0x170, 5);
        u64_(&mut d, 0x180 + 0x18, BASE + 0x220);
        u64_(&mut d, 0x180 + 0x38, BASE + 0x2A0);
        u32_(&mut d, 0x180 + 0x58, 3);
        u16_(&mut d, 0x180 + 0x60, 3);
        u16_(&mut d, 0x180 + 0x70, 12);
        u16_(&mut d, 0x220 + 0x08, 12);
        u64_(&mut d, 0x220 + 0x10, BASE + 0x310);
        u32_(&mut d, 0x220 + 0x18, 3);
        u64_(&mut d, 0x220 + 0x30, BASE + 0x300);
        u32_(&mut d, 0x2A0 + 0x08, 3);
        u64_(&mut d, 0x2A0 + 0x10, BASE + 0x340);
        u32_(&mut d, 0x300, 0b1);
        u16_(&mut d, 0x304, 12);
        d[0x307] = 1;
        u64_(&mut d, 0x308, 0x7755_5555_5599_6996);
        for (i, v) in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .into_iter()
            .enumerate()
        {
            for (k, c) in v.into_iter().enumerate() {
                f32_(&mut d, 0x310 + i * 12 + k * 4, c);
            }
        }
        for (i, v) in [0u16, 1, 2].into_iter().enumerate() {
            u16_(&mut d, 0x340 + i * 2, v);
        }
        d[0x350..0x358].copy_from_slice(b"tri_test");
        d
    }

    /// Wrap a payload as a loose RSC7 file. The system segment is 0x400 bytes
    /// (one page of 0x400), which is what `size_from_flags` returns for these flags.
    fn rsc7(payload: &[u8]) -> Vec<u8> {
        use flate2::write::DeflateEncoder;
        use std::io::Write;
        let mut enc = DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(payload).unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(&0x3743_5352u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&((1u32 << 27) | 1).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&enc.finish().unwrap());
        out
    }

    /// The whole pipeline over real files on disk: walk, parse, join, write.
    #[test]
    fn end_to_end_on_a_synthetic_resource_folder() {
        let root = std::env::temp_dir().join(format!("asset-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let stream = root.join("[cat]").join("res").join("stream");
        std::fs::create_dir_all(&stream).unwrap();
        std::fs::write(stream.join("Tri_Prop.ydr"), rsc7(&synthetic_ydr())).unwrap();
        std::fs::write(stream.join("gen9.yft"), [0u8; 64]).unwrap();
        std::fs::write(stream.join("broken.ydr"), rsc7(&[0u8; 0x40])).unwrap();
        std::fs::write(stream.join("notes.txt"), "ignored").unwrap();

        let opts = Opts {
            root: root.clone(),
            out: root.join("out.json"),
            pretty: false,
            only: vec![],
            generated_at: Some("2026-01-02T03:04:05Z".into()),
        };
        let (index, stats) = run(&opts);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(stats.model_files, 3);
        let m = &index.models["tri_prop"];
        assert_eq!(m.kind, "ydr");
        assert_eq!(m.resource, "[cat]/res");
        assert_eq!(m.hash, gen_hash("tri_prop"));
        assert_eq!(m.tris.high, 1);
        assert_eq!(m.verts.high, 3);
        assert_eq!(m.lod_dist.high, 100.0);
        assert_eq!(m.bytes.ytd, 0);
        assert!(m.bytes.model > 0 && m.bytes.total == m.bytes.model);
        assert!(index.models["gen9"]
            .flags
            .contains(&"unsupported-format".to_string()));
        assert_eq!(index.generated_at, "2026-01-02T03:04:05Z");
        assert_eq!(index.source, root.file_name().unwrap().to_string_lossy());
        assert!(index.models.contains_key("broken"));
    }
}
