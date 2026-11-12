//! Read-only audit of loose vehicle streaming assets (`.yft` / `.ytd`) of a
//! FiveM server: texture sizes and formats, polygons per LOD, and flags for
//! the usual problems. Writes one JSON report.
//!
//! Damage is told apart from "this tool cannot read it": `damaged` means the
//! file claims to be RSC7 but does not decode (bad deflate, invalid resource
//! pointers, parser panic); `unsupported-format` means another container
//! (Gen9 or unknown magic), which proves nothing about the file itself.
//!
//! ```text
//! cargo run --release --example vehicle_audit -- <resources dir> [--out audit.json]
//! ```

use std::collections::BTreeMap;
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use cw_core::filetype::FileKind;
use cw_core::mesh::MeshBundle;
use cw_core::resources::{read_named_fragments, read_textures};
use cw_core::rpf::entry::size_from_flags;
use flate2::read::DeflateDecoder;
use rayon::prelude::*;
use serde::Serialize;

const RSC7: u32 = 0x3743_5352;
/// Flag thresholds. Placeholders to tune against the server's real budget.
const HEAVY_TRIS: usize = 100_000;
const LOWRES_MAX_DIM: u16 = 512;
const MB: u64 = 1024 * 1024;
/// A vehicle whose texture dictionaries add up to more than this is its own category.
const HEAVY_YTD_BYTES: u64 = 16 * MB;

#[derive(Serialize, Clone)]
struct Tex {
    name: String,
    w: u16,
    h: u16,
    format: String,
    levels: u8,
    bytes: usize,
}

#[derive(Serialize, Clone, Default)]
struct Lods {
    high: usize,
    medium: usize,
    low: usize,
    very_low: usize,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Cause {
    /// RSC7 container that does not decode.
    Corrupt,
    /// Not an RSC7 container (Gen9 or unknown): not proof of damage.
    UnsupportedFormat,
}

#[derive(Serialize, Clone)]
struct Problem {
    file: String,
    cause: Cause,
    message: String,
}

type Fail = (Cause, String);

struct Scanned {
    path: PathBuf,
    file_bytes: u64,
    kind: Kind,
}

enum Kind {
    Ytd(Result<Vec<Tex>, Fail>),
    Yft(Result<(usize, Lods), Fail>),
}

#[derive(Serialize)]
struct Vehicle {
    name: String,
    resource: String,
    yft_files: usize,
    ytd_files: usize,
    yft_bytes: u64,
    ytd_bytes: u64,
    drawables: usize,
    triangles: Lods,
    textures: usize,
    max_texture_dim: u16,
    lowres_textures: usize,
    formats: BTreeMap<String, usize>,
    ytd_class: &'static str,
    flags: Vec<String>,
    problems: Vec<Problem>,
}

#[derive(Serialize)]
struct Report {
    generated_with: &'static str,
    root: String,
    totals: Totals,
    vehicles: Vec<Vehicle>,
}

#[derive(Serialize, Default)]
struct Totals {
    vehicles: usize,
    yft_files: usize,
    ytd_files: usize,
    damaged_files: usize,
    unsupported_files: usize,
    flagged: usize,
    ytd_over_16mb_bytes: u64,
    by_flag: BTreeMap<String, usize>,
    by_ytd_class: BTreeMap<String, usize>,
}

fn load_loose(path: &Path) -> Result<(Vec<u8>, usize), Fail> {
    let bytes = std::fs::read(path).map_err(|e| (Cause::Corrupt, e.to_string()))?;
    if bytes.len() < 16 {
        return Err((Cause::Corrupt, "shorter than an RSC7 header".into()));
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    if magic != RSC7 {
        return Err((
            Cause::UnsupportedFormat,
            format!("magic {magic:08x}, not RSC7 (Gen9?)"),
        ));
    }
    let system_flags = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let mut out = Vec::new();
    DeflateDecoder::new(&bytes[16..])
        .read_to_end(&mut out)
        .map_err(|e| (Cause::Corrupt, format!("inflate: {e}")))?;
    Ok((out, size_from_flags(system_flags) as usize))
}

fn scan_ytd(path: &Path) -> Result<Vec<Tex>, Fail> {
    let (data, system) = load_loose(path)?;
    let dict = read_textures(FileKind::Texture, &data, system)
        .map_err(|e| (Cause::Corrupt, e.to_string()))?;
    Ok(dict
        .textures
        .iter()
        .map(|t| Tex {
            name: t.name.clone().unwrap_or_default(),
            w: t.width,
            h: t.height,
            format: t
                .format
                .map(|f| f.label().to_string())
                .unwrap_or_else(|| format!("raw:{:#x}", t.format_raw)),
            levels: t.levels,
            bytes: t.data.len(),
        })
        .collect())
}

fn scan_yft(path: &Path) -> Result<(usize, Lods), Fail> {
    let (data, system) = load_loose(path)?;
    let drawables = read_named_fragments(FileKind::Fragment, &data, system)
        .map_err(|e| (Cause::Corrupt, e.to_string()))?;
    let mut lods = Lods::default();
    for d in &drawables {
        for bundle in MeshBundle::all(&d.drawable) {
            match bundle.lod {
                "high" => lods.high += bundle.triangle_count,
                "medium" => lods.medium += bundle.triangle_count,
                "low" => lods.low += bundle.triangle_count,
                _ => lods.very_low += bundle.triangle_count,
            }
        }
    }
    Ok((drawables.len(), lods))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("yft") || ext.eq_ignore_ascii_case("ytd") {
                out.push(p);
            }
        }
    }
}

/// `adder_hi.yft` and `adder+hi.ytd` belong to `adder`.
fn model_of(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_lowercase();
    stem.trim_end_matches("_hi")
        .trim_end_matches("+hi")
        .to_string()
}

/// The folder that owns the file, up to its `stream` directory.
fn resource_of(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut parts: Vec<String> = Vec::new();
    for c in rel.components() {
        let s = c.as_os_str().to_string_lossy().to_string();
        if s.eq_ignore_ascii_case("stream") {
            break;
        }
        parts.push(s);
    }
    if parts.last().is_some_and(|l| l.contains('.')) {
        parts.pop();
    }
    parts.join("/")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .unwrap_or_else(|| r"E:\FiveM\Astra\resources".into()),
    );
    let mut out_path = PathBuf::from("audit.json");
    while let Some(a) = args.next() {
        if a == "--out" {
            out_path = PathBuf::from(args.next().expect("--out needs a path"));
        }
    }

    let mut files = Vec::new();
    walk(&root, &mut files);
    eprintln!("{} files under {}", files.len(), root.display());

    let scanned: Vec<Scanned> = files
        .par_iter()
        .map(|path| {
            let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let is_ytd = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("ytd"));
            let kind = if is_ytd {
                Kind::Ytd(
                    catch_unwind(AssertUnwindSafe(|| scan_ytd(path)))
                        .unwrap_or_else(|_| Err((Cause::Corrupt, "parser panicked".into()))),
                )
            } else {
                Kind::Yft(
                    catch_unwind(AssertUnwindSafe(|| scan_yft(path)))
                        .unwrap_or_else(|_| Err((Cause::Corrupt, "parser panicked".into()))),
                )
            };
            Scanned {
                path: path.clone(),
                file_bytes,
                kind,
            }
        })
        .collect();

    let mut vehicles: BTreeMap<(String, String), Vehicle> = BTreeMap::new();
    let mut totals = Totals::default();
    for s in &scanned {
        let key = (resource_of(&root, &s.path), model_of(&s.path));
        let v = vehicles.entry(key.clone()).or_insert_with(|| Vehicle {
            name: key.1.clone(),
            resource: key.0.clone(),
            yft_files: 0,
            ytd_files: 0,
            yft_bytes: 0,
            ytd_bytes: 0,
            drawables: 0,
            triangles: Lods::default(),
            textures: 0,
            max_texture_dim: 0,
            lowres_textures: 0,
            formats: BTreeMap::new(),
            ytd_class: "none",
            flags: Vec::new(),
            problems: Vec::new(),
        });
        match &s.kind {
            Kind::Ytd(r) => {
                totals.ytd_files += 1;
                v.ytd_files += 1;
                v.ytd_bytes += s.file_bytes;
                match r {
                    Ok(texs) => {
                        for t in texs {
                            v.textures += 1;
                            let dim = t.w.max(t.h);
                            v.max_texture_dim = v.max_texture_dim.max(dim);
                            if dim <= LOWRES_MAX_DIM {
                                v.lowres_textures += 1;
                            }
                            *v.formats.entry(t.format.clone()).or_default() += 1;
                        }
                    }
                    Err((cause, message)) => {
                        match cause {
                            Cause::Corrupt => totals.damaged_files += 1,
                            Cause::UnsupportedFormat => totals.unsupported_files += 1,
                        }
                        v.problems.push(Problem {
                            file: s.path.file_name().unwrap().to_string_lossy().to_string(),
                            cause: *cause,
                            message: message.clone(),
                        });
                    }
                }
            }
            Kind::Yft(r) => {
                totals.yft_files += 1;
                v.yft_files += 1;
                v.yft_bytes += s.file_bytes;
                match r {
                    Ok((n, lods)) => {
                        v.drawables += n;
                        v.triangles.high += lods.high;
                        v.triangles.medium += lods.medium;
                        v.triangles.low += lods.low;
                        v.triangles.very_low += lods.very_low;
                    }
                    Err((cause, message)) => {
                        match cause {
                            Cause::Corrupt => totals.damaged_files += 1,
                            Cause::UnsupportedFormat => totals.unsupported_files += 1,
                        }
                        v.problems.push(Problem {
                            file: s.path.file_name().unwrap().to_string_lossy().to_string(),
                            cause: *cause,
                            message: message.clone(),
                        });
                    }
                }
            }
        }
    }

    let mut list: Vec<Vehicle> = vehicles.into_values().collect();
    for v in &mut list {
        let damaged = v.problems.iter().any(|p| p.cause == Cause::Corrupt);
        let unsupported = v
            .problems
            .iter()
            .any(|p| p.cause == Cause::UnsupportedFormat);
        v.ytd_class = match v.ytd_bytes {
            0 => "none",
            n if n <= 4 * MB => "<=4MB",
            n if n <= 8 * MB => "4-8MB",
            n if n <= HEAVY_YTD_BYTES => "8-16MB",
            _ => ">16MB",
        };
        if v.ytd_files > 0 && v.textures == 0 && v.problems.is_empty() {
            v.flags.push("no-textures".into());
        }
        if v.textures > 0 && v.max_texture_dim <= LOWRES_MAX_DIM {
            v.flags.push("low-res".into());
        }
        if v.triangles.high > HEAVY_TRIS {
            v.flags.push("heavy-tris".into());
        }
        if v.ytd_bytes > HEAVY_YTD_BYTES {
            v.flags.push("ytd-over-16mb".into());
            totals.ytd_over_16mb_bytes += v.ytd_bytes;
        }
        if v.triangles.high > 20_000 && v.triangles.medium == 0 && v.triangles.low == 0 {
            v.flags.push("no-lods".into());
        }
        if v.triangles.high > 5_000
            && (v.triangles.medium == 0 || v.triangles.low == 0 || v.triangles.very_low == 0)
        {
            v.flags.push("missing-lods".into());
        }
        if v.yft_files > 0
            && v.problems.iter().all(|p| p.file.ends_with(".ytd"))
            && v.triangles.high == 0
        {
            v.flags.push("empty-mesh".into());
        }
        if damaged {
            v.flags.push("damaged".into());
        }
        if unsupported {
            v.flags.push("unsupported-format".into());
        }
        if !v.flags.is_empty() {
            totals.flagged += 1;
        }
        for f in &v.flags {
            *totals.by_flag.entry(f.clone()).or_default() += 1;
        }
        *totals
            .by_ytd_class
            .entry(v.ytd_class.to_string())
            .or_default() += 1;
    }
    totals.vehicles = list.len();

    println!(
        "vehicles {}  yft {}  ytd {}  damaged files {}  unsupported files {}  flagged {}",
        totals.vehicles,
        totals.yft_files,
        totals.ytd_files,
        totals.damaged_files,
        totals.unsupported_files,
        totals.flagged
    );
    for (c, n) in &totals.by_ytd_class {
        println!("  ytd {c}: {n}");
    }
    for (f, n) in &totals.by_flag {
        println!("  {f}: {n}");
    }

    let report = Report {
        generated_with: "cw-core vehicle_audit 0.2",
        root: root.display().to_string(),
        totals,
        vehicles: list,
    };
    std::fs::write(&out_path, serde_json::to_vec(&report).unwrap()).expect("write report");
    eprintln!("wrote {}", out_path.display());
}
