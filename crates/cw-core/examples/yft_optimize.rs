//! Vehicle mesh optimizer with a safety net: simplify the high LOD and/or
//! generate the missing LODs of a loose `.yft`, verify the result, and keep a
//! backup that can be rolled back.
//!
//! cw-core is read-only, so the new index buffers are computed here
//! (meshoptimizer) and written by `tools/yft-writer` (CodeWalker.Core).
//!
//! ```text
//! yft_optimize optimize <file.yft> --out-dir <dir>
//!     [--high-ratio 0.5]   simplify the high LOD to this share of its triangles (default 1 = untouched)
//!     [--lods]             generate the medium/low/very-low LODs that are missing
//!     [--regen-lods]       with --lods, also replace LODs that exist (not implemented: refuses)
//!     [--lock-border]      keep open mesh borders fixed
//!     [--apply --backup-dir <dir>]   replace the original, after backing it up
//! yft_optimize rollback <manifest.json> [--force]
//! ```
//!
//! Without `--apply` nothing outside `--out-dir` is touched. The report
//! (`<name>.report.json` and `.report.md`) compares before and after per LOD.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use cw_core::filetype::FileKind;
use cw_core::mesh::{MeshBundle, MeshPart};
use cw_core::resources::{read_named_fragments, Lod};
use cw_core::rpf::entry::size_from_flags;
use flate2::read::DeflateDecoder;
use meshopt::{
    optimize_vertex_cache, simplify, simplify_with_attributes_and_locks, SimplifyOptions,
    VertexDataAdapter,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const RSC7: u32 = 0x3743_5352;
/// Share of the (new) high LOD's triangles that each generated LOD keeps.
const LOD_RATIOS: [(Lod, f32); 3] = [(Lod::Medium, 0.5), (Lod::Low, 0.25), (Lod::VeryLow, 0.1)];
/// Largest geometric error meshoptimizer may accept per part (share of the
/// part's extent). The triangle target is a goal; this is the limit.
const MAX_ERROR_HIGH: f32 = 0.01;
const MAX_ERROR_LOD: [f32; 4] = [0.01, 0.02, 0.05, 0.15];
/// Parts at or below this many triangles are copied as they are: not worth simplifying.
const TINY_TRIS: usize = 12;

type Res<T> = Result<T, String>;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn load_rsc7(bytes: &[u8]) -> Res<(Vec<u8>, usize)> {
    if bytes.len() < 16 {
        return Err("shorter than an RSC7 header".into());
    }
    if u32::from_le_bytes(bytes[0..4].try_into().unwrap()) != RSC7 {
        return Err("not an RSC7 file".into());
    }
    let sys = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let mut out = Vec::new();
    DeflateDecoder::new(&bytes[16..])
        .read_to_end(&mut out)
        .map_err(|e| format!("inflate: {e}"))?;
    Ok((out, size_from_flags(sys) as usize))
}

fn f32s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}
fn u32s(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// One snapshot of a file: per drawable, per LOD, triangles and vertices.
#[derive(Clone)]
struct Snapshot {
    drawables: Vec<DrawableSnap>,
}
#[derive(Clone)]
struct DrawableSnap {
    name: String,
    /// indexed like `Lod::ALL`
    lods: [Option<LodSnap>; 4],
    parts_high: Vec<MeshPart>,
    bbox: Option<([f32; 3], [f32; 3])>,
}
#[derive(Clone)]
struct LodSnap {
    tris: usize,
    verts: usize,
    max_index_ok: bool,
}

fn snapshot(bytes: &[u8]) -> Res<Snapshot> {
    let (data, system) = load_rsc7(bytes)?;
    let named =
        read_named_fragments(FileKind::Fragment, &data, system).map_err(|e| e.to_string())?;
    let mut drawables = Vec::new();
    for d in &named {
        let mut lods: [Option<LodSnap>; 4] = [None, None, None, None];
        let mut parts_high = Vec::new();
        let mut bbox = None;
        for (i, lod) in Lod::ALL.iter().enumerate() {
            let Some(b) = MeshBundle::from_drawable(&d.drawable, *lod) else {
                continue;
            };
            let ok = b.parts.iter().all(|p| {
                let max = u32s(p.indices.as_bytes()).into_iter().max().unwrap_or(0) as usize;
                p.triangle_count == 0 || max < p.vertex_count
            });
            lods[i] = Some(LodSnap {
                tris: b.triangle_count,
                verts: b.vertex_count,
                max_index_ok: ok,
            });
            if *lod == Lod::High {
                let mut mn = [f32::MAX; 3];
                let mut mx = [f32::MIN; 3];
                for p in &b.parts {
                    for v in f32s(p.positions.as_bytes()).chunks_exact(3) {
                        for k in 0..3 {
                            mn[k] = mn[k].min(v[k]);
                            mx[k] = mx[k].max(v[k]);
                        }
                    }
                }
                bbox = Some((mn, mx));
                parts_high = b.parts;
            }
        }
        drawables.push(DrawableSnap {
            name: d.name.clone(),
            lods,
            parts_high,
            bbox,
        });
    }
    Ok(Snapshot { drawables })
}

struct Planned {
    drawable: u32,
    lod: u32,
    model: u32,
    geometry: u32,
    indices: Vec<u32>,
}

/// meshoptimizer, same settings as the `yft_simplify` experiment. Returns the
/// new indices and the geometric error as a share of the part's extent.
fn reduce(
    part: &MeshPart,
    target_tris: usize,
    max_error: f32,
    lock_border: bool,
) -> (Vec<u32>, f32) {
    let pos = part.positions.as_bytes();
    let idx = u32s(part.indices.as_bytes());
    if idx.len() / 3 <= TINY_TRIS || target_tris * 3 >= idx.len() {
        return (idx, 0.0);
    }
    let vcount = part.vertex_count;
    let nrm = part.normals.as_ref().map(|b| f32s(b.as_bytes()));
    let uv = part.uvs.as_ref().map(|b| f32s(b.as_bytes()));
    let mut attrs = Vec::with_capacity(vcount * 5);
    for v in 0..vcount {
        match &nrm {
            Some(n) => attrs.extend_from_slice(&n[v * 3..v * 3 + 3]),
            None => attrs.extend_from_slice(&[0.0; 3]),
        }
        match &uv {
            Some(t) => attrs.extend_from_slice(&t[v * 2..v * 2 + 2]),
            None => attrs.extend_from_slice(&[0.0; 2]),
        }
    }
    let adapter = VertexDataAdapter::new(pos, 12, 0).expect("position adapter");
    let target = (target_tris * 3).max(3);
    let options = if lock_border {
        SimplifyOptions::LockBorder
    } else {
        SimplifyOptions::None
    };
    let out = simplify_with_attributes_and_locks(
        &idx,
        &adapter,
        &attrs,
        &[1.0; 5],
        5,
        &vec![false; vcount],
        target,
        max_error,
        options,
        None,
    );
    let mut gerr = 0f32;
    let _ = simplify(&idx, &adapter, target, max_error, options, Some(&mut gerr));
    (optimize_vertex_cache(&out, vcount), gerr)
}

fn part_ids(name: &str) -> Res<(u32, u32)> {
    name.strip_prefix("model")
        .and_then(|s| s.split_once(".geometry"))
        .and_then(|(m, g)| Some((m.parse().ok()?, g.parse().ok()?)))
        .ok_or_else(|| format!("unexpected part name {name}"))
}

fn write_plan(entries: &[Planned], path: &Path) -> Res<()> {
    let mut b: Vec<u8> = Vec::new();
    b.extend_from_slice(&0x4E4C_5059u32.to_le_bytes());
    b.extend_from_slice(&2u32.to_le_bytes());
    b.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        for v in [
            e.drawable,
            e.lod,
            e.model,
            e.geometry,
            e.indices.len() as u32,
        ] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for i in &e.indices {
            b.extend_from_slice(&i.to_le_bytes());
        }
    }
    std::fs::write(path, b).map_err(|e| e.to_string())
}

fn writer_exe() -> PathBuf {
    if let Ok(p) = std::env::var("YFT_WRITER") {
        return PathBuf::from(p);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/yft-writer/bin/YftWriter.exe")
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn atomic_replace(dest: &Path, bytes: &[u8]) -> Res<()> {
    let tmp = dest.with_extension("yft.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dest).map_err(|e| format!("replace {}: {e}", dest.display()))
}

fn optimize(args: &[String]) -> Res<()> {
    let input = PathBuf::from(args.first().ok_or("missing <file.yft>")?);
    let out_dir = PathBuf::from(arg_value(args, "--out-dir").ok_or("--out-dir is required")?);
    let high_ratio: f32 = arg_value(args, "--high-ratio")
        .map(|v| v.parse().map_err(|_| "bad --high-ratio"))
        .transpose()?
        .unwrap_or(1.0);
    let want_lods = args.iter().any(|a| a == "--lods");
    let lock = args.iter().any(|a| a == "--lock-border");
    let apply = args.iter().any(|a| a == "--apply");
    if args.iter().any(|a| a == "--regen-lods") {
        return Err("--regen-lods is not implemented: existing LODs are never overwritten".into());
    }
    if !(0.05..=1.0).contains(&high_ratio) {
        return Err("--high-ratio must be between 0.05 and 1".into());
    }
    let backup_dir = arg_value(args, "--backup-dir").map(PathBuf::from);
    if apply && backup_dir.is_none() {
        return Err("--apply needs --backup-dir".into());
    }
    let name = input
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad file name")?
        .to_string();

    let original = std::fs::read(&input).map_err(|e| format!("read {}: {e}", input.display()))?;
    let sha_before = sha256_hex(&original);
    let before = snapshot(&original)?;

    // --- plan -------------------------------------------------------------
    let mut plan: Vec<Planned> = Vec::new();
    // (drawable, lod index) -> (weighted error sum, tris, max error)
    let mut errors: std::collections::BTreeMap<(usize, usize), (f64, usize, f32)> =
        Default::default();
    let mut expected: std::collections::BTreeMap<(usize, usize), usize> = Default::default();

    for (di, d) in before.drawables.iter().enumerate() {
        if d.lods[0].is_none() {
            continue;
        }
        let high_tris = d.lods[0].as_ref().unwrap().tris;
        let mut new_high_tris = high_tris;

        if high_ratio < 1.0 {
            new_high_tris = 0;
            for p in &d.parts_high {
                let (m, g) = part_ids(&p.name)?;
                let target = (p.triangle_count as f32 * high_ratio) as usize;
                let (idx, err) = reduce(p, target, MAX_ERROR_HIGH, lock);
                new_high_tris += idx.len() / 3;
                let e = errors.entry((di, 0)).or_insert((0.0, 0, 0.0));
                e.0 += err as f64 * p.triangle_count as f64;
                e.1 += p.triangle_count;
                e.2 = e.2.max(err);
                plan.push(Planned {
                    drawable: di as u32,
                    lod: 0,
                    model: m,
                    geometry: g,
                    indices: idx,
                });
            }
            expected.insert((di, 0), new_high_tris);
        }
        if want_lods {
            for (lod, ratio) in LOD_RATIOS {
                let li = Lod::ALL.iter().position(|l| *l == lod).unwrap();
                if d.lods[li].is_some() {
                    continue;
                }
                let mut total = 0;
                for p in &d.parts_high {
                    let (m, g) = part_ids(&p.name)?;
                    // relative to the new high LOD, from the ORIGINAL geometry
                    let share = p.triangle_count as f32 / high_tris.max(1) as f32;
                    let target = ((new_high_tris as f32 * share) * ratio) as usize;
                    let (idx, err) = reduce(p, target.max(1), MAX_ERROR_LOD[li], lock);
                    total += idx.len() / 3;
                    let e = errors.entry((di, li)).or_insert((0.0, 0, 0.0));
                    e.0 += err as f64 * p.triangle_count as f64;
                    e.1 += p.triangle_count;
                    e.2 = e.2.max(err);
                    plan.push(Planned {
                        drawable: di as u32,
                        lod: li as u32,
                        model: m,
                        geometry: g,
                        indices: idx,
                    });
                }
                expected.insert((di, li), total);
            }
        }
    }
    if plan.is_empty() {
        println!(
            "{name}: nothing to change (no --high-ratio below 1, and no missing LOD to generate)"
        );
        return Ok(());
    }

    // --- write ------------------------------------------------------------
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let plan_path = out_dir.join(format!("{name}.plan.bin"));
    let out_path = out_dir.join(format!("{name}.yft"));
    write_plan(&plan, &plan_path)?;
    let writer = writer_exe();
    if !writer.exists() {
        return Err(format!(
            "{} not found: run tools/yft-writer/build.cmd (or set YFT_WRITER)",
            writer.display()
        ));
    }
    let run = Command::new(&writer)
        .arg("apply")
        .arg(&input)
        .arg(&plan_path)
        .arg(&out_path)
        .output()
        .map_err(|e| format!("run writer: {e}"))?;
    if !run.status.success() {
        return Err(format!(
            "writer failed: {}",
            String::from_utf8_lossy(&run.stderr).trim()
        ));
    }
    let writer_stats: Value = serde_json::from_slice(&run.stdout).unwrap_or(Value::Null);

    // --- verify: re-read the result with the independent reader ------------
    let result = std::fs::read(&out_path).map_err(|e| e.to_string())?;
    let sha_after = sha256_hex(&result);
    let after = snapshot(&result).map_err(|e| format!("result does not parse: {e}"))?;
    let mut problems: Vec<String> = Vec::new();
    if after.drawables.len() != before.drawables.len() {
        problems.push(format!(
            "drawable count {} -> {}",
            before.drawables.len(),
            after.drawables.len()
        ));
    }
    for (di, (b, a)) in before.drawables.iter().zip(&after.drawables).enumerate() {
        for li in 0..4 {
            let want = expected
                .get(&(di, li))
                .copied()
                .or(b.lods[li].as_ref().map(|l| l.tris));
            let got = a.lods[li].as_ref().map(|l| l.tris);
            if want != got {
                problems.push(format!(
                    "drawable {di} lod {li}: expected {want:?} triangles, found {got:?}"
                ));
            }
            if a.lods[li].as_ref().is_some_and(|l| !l.max_index_ok) {
                problems.push(format!(
                    "drawable {di} lod {li}: index outside the vertex buffer"
                ));
            }
        }
        if let (Some((bmin, bmax)), Some((amin, amax))) = (b.bbox, a.bbox) {
            let ext = (0..3)
                .map(|k| bmax[k] - bmin[k])
                .fold(0f32, f32::max)
                .max(1e-6);
            let drift = (0..3)
                .map(|k| (amin[k] - bmin[k]).abs().max((amax[k] - bmax[k]).abs()))
                .fold(0f32, f32::max)
                / ext;
            if drift > 0.01 {
                problems.push(format!(
                    "drawable {di}: high LOD bounding box moved {:.2}% of its extent",
                    drift * 100.0
                ));
            }
        }
    }
    let ok = problems.is_empty();

    // --- report -----------------------------------------------------------
    let lod_names = ["high", "medium", "low", "veryLow"];
    let mut rows = Vec::new();
    for (di, (b, a)) in before.drawables.iter().zip(&after.drawables).enumerate() {
        for (li, lod_name) in lod_names.iter().enumerate() {
            let (bt, bv) = b.lods[li]
                .as_ref()
                .map(|l| (l.tris, l.verts))
                .unwrap_or((0, 0));
            let (at, av) = a.lods[li]
                .as_ref()
                .map(|l| (l.tris, l.verts))
                .unwrap_or((0, 0));
            if bt == 0 && at == 0 {
                continue;
            }
            let (mean, max) = errors
                .get(&(di, li))
                .map(|(s, n, m)| ((*s / (*n).max(1) as f64) as f32 * 100.0, *m * 100.0))
                .unwrap_or((0.0, 0.0));
            rows.push(json!({
                "drawable": di, "name": a.name, "lod": lod_name,
                "tris_before": bt, "tris_after": at, "verts_before": bv, "verts_after": av,
                "generated": bt == 0 && at > 0,
                "geometric_error_mean_pct": mean, "geometric_error_max_pct": max,
            }));
        }
    }
    let report = json!({
        "tool": "yft_optimize 0.1",
        "input": input.display().to_string(),
        "sha256_before": sha_before, "sha256_after": sha_after,
        "bytes_before": original.len(), "bytes_after": result.len(),
        "params": { "high_ratio": high_ratio, "lods": want_lods, "lock_border": lock },
        "rows": rows,
        "verification": { "ok": ok, "problems": problems },
        "writer": writer_stats,
    });
    std::fs::write(
        out_dir.join(format!("{name}.report.json")),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    let mut md = format!(
        "# {name}\n\nfile {} -> {} bytes ({:+.1}%)  |  verification: {}\n\n| drawable | LOD | tris before | tris after | verts before | verts after | error mean / max (% of part) |\n|---|---|---:|---:|---:|---:|---:|\n",
        original.len(),
        result.len(),
        100.0 * (result.len() as f64 - original.len() as f64) / original.len() as f64,
        if ok { "ok" } else { "FAILED" },
    );
    for r in &rows {
        md.push_str(&format!(
            "| {} | {}{} | {} | {} | {} | {} | {:.3} / {:.3} |\n",
            r["name"].as_str().unwrap_or("?"),
            r["lod"].as_str().unwrap(),
            if r["generated"].as_bool() == Some(true) {
                " (new)"
            } else {
                ""
            },
            r["tris_before"],
            r["tris_after"],
            r["verts_before"],
            r["verts_after"],
            r["geometric_error_mean_pct"].as_f64().unwrap_or(0.0),
            r["geometric_error_max_pct"].as_f64().unwrap_or(0.0),
        ));
    }
    if !ok {
        md.push_str("\nProblems:\n");
        for p in &problems {
            md.push_str(&format!("- {p}\n"));
        }
    }
    std::fs::write(out_dir.join(format!("{name}.report.md")), &md).map_err(|e| e.to_string())?;
    println!("{md}");

    // --- apply with backup ---------------------------------------------------
    if !apply {
        println!("not applied: result in {}", out_path.display());
        return if ok {
            Ok(())
        } else {
            Err("verification failed".into())
        };
    }
    if !ok {
        return Err("verification failed: original left untouched".into());
    }
    let backup_dir = backup_dir.unwrap();
    std::fs::create_dir_all(&backup_dir).map_err(|e| e.to_string())?;
    let backup_path = backup_dir.join(format!("{name}.{}.orig.yft", &sha_before[..12]));
    if !backup_path.exists() {
        std::fs::write(&backup_path, &original).map_err(|e| e.to_string())?;
    }
    let readback = std::fs::read(&backup_path).map_err(|e| e.to_string())?;
    if sha256_hex(&readback) != sha_before {
        return Err("backup does not match the original: original left untouched".into());
    }
    let manifest = json!({
        "version": 1,
        "created": now_secs(),
        "target": input.canonicalize().unwrap_or(input.clone()).display().to_string(),
        "backup": backup_path.canonicalize().unwrap_or(backup_path.clone()).display().to_string(),
        "sha256_before": sha_before,
        "sha256_after": sha_after,
        "report": out_dir.join(format!("{name}.report.json")).display().to_string(),
        "params": report["params"],
    });
    let manifest_path = backup_dir.join(format!("{name}.{}.manifest.json", now_secs()));
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    atomic_replace(&input, &result)?;
    println!(
        "applied. backup {}\nrollback: yft_optimize rollback \"{}\"",
        backup_path.display(),
        manifest_path.display()
    );
    Ok(())
}

fn rollback(args: &[String]) -> Res<()> {
    let path = args.first().ok_or("missing <manifest.json>")?;
    let force = args.iter().any(|a| a == "--force");
    let m: Value = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let target = PathBuf::from(m["target"].as_str().ok_or("manifest has no target")?);
    let backup = PathBuf::from(m["backup"].as_str().ok_or("manifest has no backup")?);
    let want_before = m["sha256_before"]
        .as_str()
        .ok_or("manifest has no sha256_before")?;
    let want_after = m["sha256_after"]
        .as_str()
        .ok_or("manifest has no sha256_after")?;

    let saved = std::fs::read(&backup).map_err(|e| format!("backup missing or unreadable: {e}"))?;
    if sha256_hex(&saved) != want_before {
        return Err("backup file no longer matches its recorded hash: refusing to restore".into());
    }
    let current = std::fs::read(&target).map_err(|e| format!("target unreadable: {e}"))?;
    let cur = sha256_hex(&current);
    if cur == want_before {
        println!("already at the original version");
        return Ok(());
    }
    if cur != want_after && !force {
        return Err(format!(
            "{} changed since the optimization (sha256 {}): use --force to overwrite",
            target.display(),
            &cur[..12]
        ));
    }
    atomic_replace(&target, &saved)?;
    let restored = sha256_hex(&std::fs::read(&target).map_err(|e| e.to_string())?);
    if restored != want_before {
        return Err("restored file does not match the original hash".into());
    }
    println!("restored {} from {}", target.display(), backup.display());
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("optimize") => optimize(&args[1..]),
        Some("rollback") => rollback(&args[1..]),
        _ => Err("usage: yft_optimize optimize <file.yft> --out-dir <dir> [...] | rollback <manifest.json> [--force]".into()),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
