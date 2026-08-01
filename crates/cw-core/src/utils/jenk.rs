//! Jenkins one-at-a-time hash, the identity of every name in the game files.
//!
//! Port of `CodeWalker.Core/GameFiles/Utils/Jenk.cs` (`JenkHash`).
//!
//! Two details decide correctness here:
//!
//! 1. C# `uint` arithmetic is unchecked by default, so every `+` wraps. Rust
//!    panics on overflow in debug builds, so each addition is `wrapping_add`.
//!    The shift distances are all below 32, so `<<` and `>>` carry over as-is.
//! 2. The three C# overloads are *not* interchangeable on non-ASCII input.
//!    `GenHash(string)` casts each UTF-16 code unit to `byte`, truncating to the
//!    low 8 bits; `GenHash(string, UTF8)` hashes UTF-8 bytes; and
//!    `GenHash(string, ASCII)` substitutes `?` for anything above U+007F.
//!    They agree on ASCII, which is every real game file name, but the port
//!    keeps them distinct so nothing silently changes on odd input.

/// Text encoding applied before hashing, matching `JenkHashInputEncoding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JenkEncoding {
    Utf8 = 0,
    Ascii = 1,
}

/// The hash body shared by all three overloads.
#[inline]
fn hash_bytes(data: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &b in data {
        h = h.wrapping_add(b as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// Hash raw bytes. Port of `JenkHash.GenHash(byte[])`.
#[inline]
pub fn gen_hash_bytes(data: &[u8]) -> u32 {
    hash_bytes(data)
}

/// Hash a string the way the game does. Port of `JenkHash.GenHash(string)`.
///
/// Each UTF-16 code unit is truncated to its low byte, exactly as the C#
/// `(byte)text[i]` cast does. This is the overload CodeWalker uses everywhere,
/// and the one that produces the hashes stored in the archives.
pub fn gen_hash(text: &str) -> u32 {
    let mut h: u32 = 0;
    for unit in text.encode_utf16() {
        h = h.wrapping_add((unit as u8) as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// Hash a string under an explicit encoding.
/// Port of `JenkHash.GenHash(string, JenkHashInputEncoding)`.
pub fn gen_hash_encoded(text: &str, encoding: JenkEncoding) -> u32 {
    match encoding {
        JenkEncoding::Utf8 => hash_bytes(text.as_bytes()),
        JenkEncoding::Ascii => {
            // .NET's ASCIIEncoding maps every char above U+007F to '?'.
            let bytes: Vec<u8> = text
                .encode_utf16()
                .map(|u| if u < 0x80 { u as u8 } else { b'?' })
                .collect();
            hash_bytes(&bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected values produced by the reference implementation itself:
    /// `CodeWalker.Core.dll` (built 2025-03-17) loaded into PowerShell and
    /// called directly, rather than by re-deriving the arithmetic here.
    #[test]
    fn matches_reference_gen_hash() {
        let cases: &[(&str, u32)] = &[
            ("", 0x0000_0000),
            ("a", 0xCA2E_9442),
            ("prop_bench_01a", 0x6BA5_14AC),
            ("hei_prop_heist_binbag", 0x43E1_F3DE),
            ("v_ilev_carmodshop", 0x6A91_DC05),
            ("x64a.rpf", 0x23D9_D79E),
            ("common.rpf", 0x413D_055A),
            ("dlc.rpf", 0x777F_AAB0),
            ("vehicles.meta", 0x427C_BD2C),
            ("handling.meta", 0x7148_BA86),
            ("ymap", 0xCBAD_ADE4),
            ("The quick brown fox", 0x43CF_4EFF),
            ("MixedCase_NAME", 0xF3B3_6806),
            ("0123456789", 0x9218_93BE),
            // Displayed by the app shell as a live check that cw-core is linked.
            ("astramapper", 0x79B0_5FD5),
        ];
        for &(text, expected) in cases {
            assert_eq!(gen_hash(text), expected, "gen_hash({text:?})");
        }
    }

    /// The three overloads must stay distinct. "\u{e9}test" is the case that
    /// separates them in the reference implementation.
    #[test]
    fn encodings_diverge_on_non_ascii() {
        let s = "\u{e9}test";
        assert_eq!(gen_hash_encoded(s, JenkEncoding::Utf8), 0xA91B_0629);
        assert_eq!(gen_hash_encoded(s, JenkEncoding::Ascii), 0x84E0_894A);
        assert_eq!(gen_hash(s), 0xC709_AB9C);
    }

    /// On ASCII input — every real game file name — they must all agree.
    #[test]
    fn encodings_agree_on_ascii() {
        for name in ["x64a.rpf", "prop_bench_01a", "vehicles.meta", ""] {
            let h = gen_hash(name);
            assert_eq!(gen_hash_encoded(name, JenkEncoding::Utf8), h, "{name}");
            assert_eq!(gen_hash_encoded(name, JenkEncoding::Ascii), h, "{name}");
            assert_eq!(gen_hash_bytes(name.as_bytes()), h, "{name}");
        }
    }

    /// The additions must wrap rather than panic. A long input overflows `u32`
    /// many times over; in debug builds a plain `+` would abort here.
    #[test]
    fn arithmetic_wraps_instead_of_panicking() {
        let long = "z".repeat(4096);
        let _ = gen_hash(&long);
        let _ = gen_hash_bytes(&vec![0xFF; 4096]);
    }
}
