//! Turning a Jenkins hash back into the name it came from.
//!
//! Port of `MetaNames.TryGetString` and `MetaXmlBase.HashString` from
//! `CodeWalker.Core/GameFiles/MetaTypes/`. The reference's table is a 20k-entry
//! `MetaName` enum and `Enum.IsDefined`; here it is a data asset,
//! `assets/metanames.bin`, generated from that enum by `scripts/metanames.ps1`
//! — see the script for the format and why it is not Rust source.
//!
//! What this cannot do: the reference also consults `JenkIndex`, a cache of
//! every string the app has seen this session. Most drawable and texture names
//! in the archives are only in there, not in `MetaNames`, so a `.ydd` entry
//! usually still resolves to `hash_XXXXXXXX` here exactly as it does in
//! CodeWalker before something has loaded the matching name.

/// The generated table. Sorted by hash, so lookup is a binary search over a
/// slice of the embedded bytes with no allocation and no startup cost.
static TABLE: &[u8] = include_bytes!("../../assets/metanames.bin");

const MAGIC: &[u8; 4] = b"CWNM";
const HEADER: usize = 12;
const ENTRY: usize = 8;

/// The index and blob halves of the table, or `None` if the asset is not the
/// shape this build expects.
fn table() -> Option<(&'static [u8], &'static [u8])> {
    if TABLE.len() < HEADER || &TABLE[..4] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes([TABLE[4], TABLE[5], TABLE[6], TABLE[7]]);
    if version != 1 {
        return None;
    }
    let count = u32::from_le_bytes([TABLE[8], TABLE[9], TABLE[10], TABLE[11]]) as usize;
    let blob_start = HEADER.checked_add(count.checked_mul(ENTRY)?)?;
    if blob_start > TABLE.len() {
        return None;
    }
    Some((&TABLE[HEADER..blob_start], &TABLE[blob_start..]))
}

/// The name a hash was generated from, if the reference's table knows it.
///
/// `0` is not a lookup: it is the reference's "no name" and answers `None`.
pub fn lookup(hash: u32) -> Option<&'static str> {
    if hash == 0 {
        return None;
    }
    let (index, blob) = table()?;

    let entry = |i: usize| -> (u32, u32) {
        let at = i * ENTRY;
        let e = &index[at..at + ENTRY];
        (
            u32::from_le_bytes([e[0], e[1], e[2], e[3]]),
            u32::from_le_bytes([e[4], e[5], e[6], e[7]]),
        )
    };

    let count = index.len() / ENTRY;
    let (mut low, mut high) = (0usize, count);
    while low < high {
        let mid = (low + high) / 2;
        let (key, offset) = entry(mid);
        match key.cmp(&hash) {
            std::cmp::Ordering::Less => low = mid + 1,
            std::cmp::Ordering::Greater => high = mid,
            std::cmp::Ordering::Equal => {
                let start = offset as usize;
                let end = blob[start..].iter().position(|b| *b == 0)? + start;
                return std::str::from_utf8(&blob[start..end]).ok();
            }
        }
    }
    None
}

/// A hash as the reference renders it: the name if known, `hash_XXXXXXXX` if
/// not, and the empty string for zero.
///
/// Verbatim from `MetaXmlBase.HashString`, including the empty string for zero
/// — a `.ymt` element whose name hash is 0 opens a tag with no name in
/// CodeWalker's XML too, and swapping in a placeholder here would make this
/// port's output differ from the reference's for a case that really occurs.
pub fn hash_string(hash: u32) -> String {
    if hash == 0 {
        return String::new();
    }
    match lookup(hash) {
        Some(name) => name.to_string(),
        None => format!("hash_{hash:08X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::{gen_hash, gen_hash_encoded, JenkEncoding};

    #[test]
    fn the_asset_parses() {
        let (index, blob) = table().expect("metanames.bin is missing or malformed");
        assert!(index.len() / ENTRY > 20_000, "table is suspiciously small");
        assert!(!blob.is_empty());
    }

    /// Names from the reference enum, and the hash each is keyed by. The enum
    /// generator hashes with UTF-8, so these double as a check that the table
    /// was built from the values the reference stores rather than from a
    /// re-hash under the wrong overload.
    #[test]
    fn resolves_names_from_the_reference_table() {
        for name in [
            "CScenarioPointRegion",
            "archetypeName",
            "CMapTypes",
            "CScenarioPointManifest",
        ] {
            let hash = gen_hash_encoded(name, JenkEncoding::Utf8);
            assert_eq!(lookup(hash), Some(name), "{name} is not in the table");
        }
    }

    /// The table is a name table, not a hash oracle: a name it has never seen
    /// falls back rather than inventing something.
    #[test]
    fn falls_back_to_the_hash_for_an_unknown_name() {
        let hash = gen_hash("astramapper_not_a_real_meta_name");
        assert_eq!(lookup(hash), None);
        assert_eq!(hash_string(hash), format!("hash_{hash:08X}"));
    }

    /// `HashString(0)` is the empty string in the reference, not `hash_00000000`.
    #[test]
    fn renders_zero_as_nothing() {
        assert_eq!(hash_string(0), "");
        assert_eq!(lookup(0), None);
    }
}
