//! Loose streaming assets (`.yft`, `.ydr`, `.ydd`, `.ytd` sitting in a FiveM
//! resource folder, as opposed to inside an RPF): loading, naming and the audit
//! flags. Shared by the `vehicle_audit` and `asset_index` examples so both give
//! the same verdict on the same file.

use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::DeflateDecoder;

use crate::rpf::entry::size_from_flags;

/// `RSC7` as a little-endian `u32`.
pub const RSC7: u32 = 0x3743_5352;
/// More than this many high-LOD triangles is `heavy-tris`. Placeholder to tune
/// against the server's real budget.
pub const HEAVY_TRIS: usize = 100_000;
/// A largest texture of this many pixels or less is `low-res`.
pub const LOWRES_MAX_DIM: u16 = 512;
pub const MB: u64 = 1024 * 1024;
/// A model whose texture dictionaries add up to more than this is its own category.
pub const HEAVY_YTD_BYTES: u64 = 16 * MB;

/// Why a file did not decode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cause {
    /// RSC7 container that does not decode.
    Corrupt,
    /// Not an RSC7 container (Gen9 or unknown): not proof of damage.
    UnsupportedFormat,
}

impl Cause {
    /// How the audit report names the cause of a problem file.
    pub fn label(self) -> &'static str {
        match self {
            Cause::Corrupt => "corrupt",
            Cause::UnsupportedFormat => "unsupported-format",
        }
    }

    /// The audit flag this cause raises.
    pub fn flag(self) -> &'static str {
        match self {
            Cause::Corrupt => "damaged",
            Cause::UnsupportedFormat => "unsupported-format",
        }
    }
}

pub type Fail = (Cause, String);

/// Read a loose RSC7 file and inflate it. Returns the payload and the size of
/// its system segment, which is what the resource reader wants.
pub fn load_loose(path: &Path) -> Result<(Vec<u8>, usize), Fail> {
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

/// Every file under `dir` whose extension is one of `exts` (case-insensitive,
/// without the dot), in directory order.
pub fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, exts, out);
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if exts.iter().any(|e| ext.eq_ignore_ascii_case(e)) {
                out.push(p);
            }
        }
    }
}

/// `adder_hi.yft` and `adder+hi.ytd` belong to `adder`. Lowercase.
pub fn model_of(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_lowercase();
    stem.trim_end_matches("_hi")
        .trim_end_matches("+hi")
        .to_string()
}

/// Whether the file is the high-detail companion of a model (`_hi` / `+hi`).
pub fn is_hi(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    stem.ends_with("_hi") || stem.ends_with("+hi")
}

/// The folder that owns the file, up to its `stream` directory, with `/`
/// separators.
pub fn resource_of(root: &Path, path: &Path) -> String {
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

/// What the flags are computed from. One model, or one aggregate of a model's
/// files.
#[derive(Default, Clone, Debug)]
pub struct FlagInput {
    pub yft_files: usize,
    pub ytd_files: usize,
    pub ytd_bytes: u64,
    pub textures: usize,
    pub max_texture_dim: u16,
    /// Triangles per LOD: high, medium, low, very low.
    pub tris: [usize; 4],
    pub corrupt: bool,
    pub unsupported: bool,
    /// No file of the model failed to decode.
    pub no_problems: bool,
    /// Every failed file is a `.ytd`.
    pub problems_only_ytd: bool,
}

/// The audit flags of one model, in the order the audit report lists them.
pub fn flags(i: &FlagInput) -> Vec<String> {
    let [high, medium, low, very_low] = i.tris;
    let mut f: Vec<String> = Vec::new();
    if i.ytd_files > 0 && i.textures == 0 && i.no_problems {
        f.push("no-textures".into());
    }
    if i.textures > 0 && i.max_texture_dim <= LOWRES_MAX_DIM {
        f.push("low-res".into());
    }
    if high > HEAVY_TRIS {
        f.push("heavy-tris".into());
    }
    if i.ytd_bytes > HEAVY_YTD_BYTES {
        f.push("ytd-over-16mb".into());
    }
    if high > 20_000 && medium == 0 && low == 0 {
        f.push("no-lods".into());
    }
    if high > 5_000 && (medium == 0 || low == 0 || very_low == 0) {
        f.push("missing-lods".into());
    }
    if i.yft_files > 0 && i.problems_only_ytd && high == 0 {
        f.push("empty-mesh".into());
    }
    if i.corrupt {
        f.push(Cause::Corrupt.flag().into());
    }
    if i.unsupported {
        f.push(Cause::UnsupportedFormat.flag().into());
    }
    f
}

/// Bucket of a model's `.ytd` bytes, as the audit reports it.
pub fn ytd_class(bytes: u64) -> &'static str {
    match bytes {
        0 => "none",
        n if n <= 4 * MB => "<=4MB",
        n if n <= 8 * MB => "4-8MB",
        n if n <= HEAVY_YTD_BYTES => "8-16MB",
        _ => ">16MB",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_name_drops_hi_suffixes() {
        assert_eq!(model_of(Path::new("a/Adder_hi.yft")), "adder");
        assert_eq!(model_of(Path::new("a/adder+hi.ytd")), "adder");
        assert_eq!(
            model_of(Path::new("a/prop_Bench_01a.ydr")),
            "prop_bench_01a"
        );
        assert!(is_hi(Path::new("adder_hi.yft")));
        assert!(is_hi(Path::new("adder+hi.ytd")));
        assert!(!is_hi(Path::new("adder.yft")));
    }

    #[test]
    fn resource_stops_at_stream() {
        let root = Path::new("/r");
        let p = Path::new("/r/[astra]/cars/adder/stream/sub/adder.yft");
        assert_eq!(resource_of(root, p), "[astra]/cars/adder");
        // A loose file with no `stream` folder: its directory is the resource.
        let p = Path::new("/r/[astra]/props/x.ydr");
        assert_eq!(resource_of(root, p), "[astra]/props");
    }

    #[test]
    fn flags_follow_the_audit_thresholds() {
        let f = flags(&FlagInput {
            yft_files: 1,
            ytd_files: 1,
            ytd_bytes: 17 * MB,
            textures: 4,
            max_texture_dim: 512,
            tris: [120_000, 0, 0, 0],
            no_problems: true,
            problems_only_ytd: true,
            ..Default::default()
        });
        assert_eq!(
            f,
            [
                "low-res",
                "heavy-tris",
                "ytd-over-16mb",
                "no-lods",
                "missing-lods"
            ]
        );
        assert_eq!(ytd_class(17 * MB), ">16MB");
        assert_eq!(ytd_class(0), "none");
        assert_eq!(ytd_class(4 * MB), "<=4MB");
    }

    #[test]
    fn bad_magic_is_unsupported_not_damaged() {
        let dir = std::env::temp_dir().join(format!("cw-loose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.yft");
        std::fs::write(&p, [0u8; 32]).unwrap();
        let err = load_loose(&p).unwrap_err();
        assert_eq!(err.0, Cause::UnsupportedFormat);
        std::fs::write(&p, [1u8; 4]).unwrap();
        assert_eq!(load_loose(&p).unwrap_err().0, Cause::Corrupt);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
