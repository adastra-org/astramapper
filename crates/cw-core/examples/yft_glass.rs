//! Dump the collision materials and glass-window table of loose `.yft` files.
//!
//! The material *type* of a bound polygon is what the game uses to decide how
//! a surface reacts to bullets (glass that resists, glass that never breaks).
//! This tool only reads: it prints, per file, how many polygons use each
//! material type, and the vehicle glass-window table. Names for the numbers
//! come from the game's own `materials.dat`, which is not read here.
//!
//! ```text
//! yft_glass <file.yft> [<file.yft> ...]
//! ```

use std::collections::BTreeMap;
use std::io::Read;

use cw_core::filetype::FileKind;
use cw_core::resources::{read_fragments, Bounds, BoundsData};
use cw_core::rpf::entry::size_from_flags;
use flate2::read::DeflateDecoder;

const RSC7: u32 = 0x3743_5352;

fn load(bytes: &[u8]) -> Result<(Vec<u8>, usize), String> {
    if bytes.len() < 16 || u32::from_le_bytes(bytes[0..4].try_into().unwrap()) != RSC7 {
        return Err("not an RSC7 file".into());
    }
    let sys = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let mut out = Vec::new();
    DeflateDecoder::new(&bytes[16..])
        .read_to_end(&mut out)
        .map_err(|e| format!("inflate: {e}"))?;
    Ok((out, size_from_flags(sys) as usize))
}

/// polygons per (material type, flags) in one bound, recursing into composites.
fn collect(b: &Bounds, out: &mut BTreeMap<(u8, u16), usize>) {
    match &b.data {
        BoundsData::Geometry(g) => {
            for p in 0..g.polygons_count {
                let idx = g.material_index_of(p) as usize;
                if let Some(m) = g.materials.get(idx) {
                    *out.entry((m.material_type(), m.flags())).or_default() += 1;
                }
            }
        }
        BoundsData::Composite(c) => {
            for child in &c.children {
                collect(child, out);
            }
        }
        _ => {}
    }
}

fn main() {
    for path in std::env::args().skip(1) {
        println!("== {path}");
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                println!("  read: {e}");
                continue;
            }
        };
        let frag = match load(&bytes)
            .and_then(|(d, s)| read_fragments(FileKind::Fragment, &d, s).map_err(|e| e.to_string()))
        {
            Ok(f) => f,
            Err(e) => {
                println!("  parse: {e}");
                continue;
            }
        };

        let mut mats: BTreeMap<(u8, u16), usize> = BTreeMap::new();
        if let Some(g) = &frag.physics_lod_group {
            for lod in g.all_lods() {
                if let Some(b) = &lod.bound {
                    collect(b, &mut mats);
                }
            }
        }
        println!("  collision materials (type, flags -> polygons):");
        for ((t, f), n) in &mats {
            println!("    type {t:>3}  flags {f:#06x}  {n}");
        }

        match &frag.vehicle_glass_windows {
            Some(v) => {
                println!("  vehicle glass windows: {}", v.item_count);
                for (i, w) in v.windows.iter().enumerate() {
                    println!("    window {i}: {w:?}");
                }
            }
            None => println!("  vehicle glass windows: none"),
        }
        println!("  glass_windows (breakable): {}", frag.glass_windows.len());
    }
}
