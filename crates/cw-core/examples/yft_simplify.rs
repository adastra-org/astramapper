//! Experiment: simplify the high LOD of a loose vehicle `.yft` with
//! meshoptimizer and report how far each ratio goes and at what error.
//! `--emit <ratio> <file.bin>` also writes the new index buffers per
//! geometry, for a writer (CodeWalker.Core, in C#) to put back.
//!
//! ```text
//! cargo run --release --example yft_simplify -- <file.yft> [--lock-border] [--emit 0.25 out.bin]
//! ```

use std::io::Read;
use std::time::Instant;

use cw_core::filetype::FileKind;
use cw_core::mesh::MeshBundle;
use cw_core::resources::{read_named_fragments, Lod};
use cw_core::rpf::entry::size_from_flags;
use flate2::read::DeflateDecoder;
use meshopt::{simplify, simplify_with_attributes_and_locks, SimplifyOptions, VertexDataAdapter};

fn load(path: &str) -> (Vec<u8>, usize) {
    let bytes = std::fs::read(path).expect("read file");
    assert_eq!(
        u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
        0x3743_5352,
        "not RSC7"
    );
    let sys = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let mut out = Vec::new();
    DeflateDecoder::new(&bytes[16..])
        .read_to_end(&mut out)
        .expect("inflate");
    (out, size_from_flags(sys) as usize)
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("yft path");
    let lock = args.iter().any(|a| a == "--lock-border");
    let emit = args.iter().position(|a| a == "--emit").map(|i| {
        (
            args[i + 1].parse::<f32>().expect("ratio"),
            args[i + 2].clone(),
        )
    });

    let (data, system) = load(path);
    let drawables = read_named_fragments(FileKind::Fragment, &data, system).expect("parse yft");
    let main = &drawables.first().expect("no drawable").drawable;
    let bundle = MeshBundle::from_drawable(main, Lod::High).expect("no high LOD");
    println!(
        "{}  high LOD: {} parts, {} verts, {} tris, border {}",
        path.rsplit(['/', '\\']).next().unwrap(),
        bundle.parts.len(),
        bundle.vertex_count,
        bundle.triangle_count,
        if lock { "locked" } else { "free" }
    );
    println!("ratio | tris after | % of original | geometric error: tri-weighted mean / p90 of parts (% of part extent) | time");

    let mut emitted: Vec<u8> = Vec::new();
    for ratio in [0.75f32, 0.5, 0.25, 0.1] {
        let started = Instant::now();
        let mut total = 0usize;
        let mut errs: Vec<(f32, usize)> = Vec::new();
        let mut geoms: Vec<(u32, u32, Vec<u32>)> = Vec::new();
        for part in &bundle.parts {
            let pos = part.positions.as_bytes();
            let idx = u32s(part.indices.as_bytes());
            let nrm = part.normals.as_ref().map(|b| f32s(b.as_bytes()));
            let uv = part.uvs.as_ref().map(|b| f32s(b.as_bytes()));
            let vcount = part.vertex_count;
            // attribute stream: normal (3) + uv (2); zeros when missing
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
            let adapter = VertexDataAdapter::new(pos, 12, 0).expect("adapter");
            let target = (((idx.len() as f32 * ratio) as usize) / 3 * 3).max(3);
            let mut err = 0f32;
            let options = if lock {
                SimplifyOptions::LockBorder
            } else {
                SimplifyOptions::None
            };
            let out = simplify_with_attributes_and_locks(
                &idx,
                &adapter,
                &attrs,
                &[1.0, 1.0, 1.0, 1.0, 1.0],
                5,
                &vec![false; vcount],
                target,
                1.0,
                options,
                Some(&mut err),
            );
            total += out.len() / 3;
            // Geometric error alone: position-only run on the same target, since
            // the attribute run folds UV and normal error into one number.
            let mut gerr = 0f32;
            let _ = simplify(&idx, &adapter, target, 1.0, options, Some(&mut gerr));
            let _ = err;
            errs.push((gerr, idx.len() / 3));
            if emit
                .as_ref()
                .is_some_and(|(r, _)| (*r - ratio).abs() < 1e-6)
            {
                // `model{m}.geometry{g}`
                let (m, g) = part
                    .name
                    .strip_prefix("model")
                    .and_then(|s| s.split_once(".geometry"))
                    .map(|(m, g)| (m.parse::<usize>().unwrap(), g.parse::<usize>().unwrap()))
                    .expect("part name");
                geoms.push((m as u32, g as u32, out.clone()));
            }
        }
        let wsum: usize = errs.iter().map(|e| e.1).sum();
        let mean = errs.iter().map(|e| e.0 * e.1 as f32).sum::<f32>() / wsum.max(1) as f32;
        let mut sorted: Vec<f32> = errs.iter().map(|e| e.0).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p90 = sorted[(sorted.len() as f32 * 0.9) as usize % sorted.len().max(1)];
        println!(
            "{:>5} | {:>10} | {:>12.1}% | {:>8.3}% / {:>6.3}% | {:?}",
            ratio,
            total,
            100.0 * total as f32 / bundle.triangle_count as f32,
            mean * 100.0,
            p90 * 100.0,
            started.elapsed()
        );
        if !geoms.is_empty() {
            // little-endian: u32 geometry count, then per geometry u32 model, u32 geometry,
            // u32 index count, index count x u32
            emitted.clear();
            emitted.extend_from_slice(&(geoms.len() as u32).to_le_bytes());
            for (m, g, idx) in &geoms {
                emitted.extend_from_slice(&m.to_le_bytes());
                emitted.extend_from_slice(&g.to_le_bytes());
                emitted.extend_from_slice(&(idx.len() as u32).to_le_bytes());
                for i in idx {
                    emitted.extend_from_slice(&i.to_le_bytes());
                }
            }
        }
    }
    if let Some((_, file)) = emit {
        std::fs::write(&file, emitted).expect("write emit");
        println!("wrote {file}");
    }
}
