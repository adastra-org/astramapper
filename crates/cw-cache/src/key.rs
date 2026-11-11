//! What identifies a cached artifact.
//!
//! **Keyed by source, not by content.** Content-addressing would mean
//! extracting, inflating and parsing the asset to find out whether the cache
//! already holds it, which is exactly the work the cache exists to avoid. So a
//! key describes *where the bytes came from and what was done to them*: the
//! archive's identity on disk, the entry's position and size inside it, the
//! kind of artifact, and the compiler settings that would change the output.
//!
//! The cost of that choice is that a key can go stale without the bytes
//! changing — and the mitigation is that every input which could change the
//! output is in the key. If the compiler changes, [`CacheKey::kind_version`]
//! moves. If the format changes, [`FORMAT_VERSION`] moves and the whole cache
//! is wiped. Getting this wrong means serving wrong geometry silently, which is
//! worse than having no cache, so the rule is: **when in doubt, put it in the
//! key.**

use serde::{Deserialize, Serialize};

/// Bumped when the on-disk layout changes in a way older files cannot be read
/// as. Opening a cache with a different version wipes it rather than migrating:
/// this is derived data, and a migration path is a bug surface for no gain.
pub const FORMAT_VERSION: u32 = 1;

/// What kind of artifact a record holds.
///
/// Carries its own version so one compiler can change without invalidating the
/// others. Bump the version whenever the bytes a kind produces would differ for
/// the same input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Kind {
    /// An install-scoped index. Small, always baked, pinned, never evicted.
    Index = 1,
    /// A compiled model envelope.
    Model = 2,
    /// A compiled texture.
    Texture = 3,
    /// A flattened `.ymap`.
    Map = 4,
    /// Collision geometry from a `.ybn`.
    Collision = 5,
    /// AI path nodes and links from a `.ynd`.
    Paths = 6,
}

impl Kind {
    /// Bump when this kind's compiler starts producing different bytes.
    pub fn version(self) -> u16 {
        match self {
            Kind::Index => 1,
            Kind::Model => 4,
            Kind::Texture => 1,
            Kind::Map => 1,
            Kind::Collision => 1,
            Kind::Paths => 1,
        }
    }

    /// Whether records of this kind live in the pinned pack, which eviction
    /// never touches.
    ///
    /// Only the install-scoped indexes. They are a few megabytes, they are what
    /// makes a warm start fast, and evicting them to make room for one more
    /// prop would be exactly backwards.
    pub fn is_pinned(self) -> bool {
        matches!(self, Kind::Index)
    }

    pub fn from_raw(value: u8) -> Option<Self> {
        Some(match value {
            1 => Kind::Index,
            2 => Kind::Model,
            3 => Kind::Texture,
            4 => Kind::Map,
            5 => Kind::Collision,
            6 => Kind::Paths,
            _ => return None,
        })
    }
}

/// Everything that decides which bytes an artifact should be.
///
/// Hashed into a [`CacheKey`]. Given as a struct rather than as loose arguments
/// so adding an input is a compile error at every construction site instead of
/// a silently unhashed field.
#[derive(Debug, Clone, Copy)]
pub struct KeyInputs<'a> {
    pub kind: Kind,
    /// The source archive's identity on disk. **Zero means unknown**, and an
    /// unknown stamp must never be cached — see [`CacheKey::of`].
    pub archive_stamp: u64,
    /// The entry's raw header words, which carry its offset and size inside the
    /// archive. Two different files in the same archive differ here.
    pub entry_header: (u32, u32),
    /// The entry's full lowercase path, so a rebuilt archive that happens to
    /// place a different file at the same offset does not collide.
    pub entry_path: &'a str,
    /// Compiler settings that change the output: GPU capabilities for textures,
    /// quality level, anything else that is a choice rather than an input.
    pub params: u64,
}

/// A 128-bit identity for one cached artifact.
///
/// 128 bits rather than 64 because the install has around 400,000 entries and
/// several kinds each: at 64 bits a birthday collision is a one-in-a-few-
/// million event across a user base, and a collision here serves the wrong
/// asset silently. 128 makes it never happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey(pub u64, pub u64);

impl CacheKey {
    /// Hash the inputs, or `None` when they cannot be trusted to identify
    /// anything.
    ///
    /// The one refusal is an unknown archive stamp. A file that could not be
    /// stat'd might have changed under us, and caching against "I don't know"
    /// is how a stale artifact outlives the thing it was made from.
    pub fn of(inputs: KeyInputs<'_>) -> Option<Self> {
        if inputs.archive_stamp == 0 {
            return None;
        }
        let mut hash = Hasher::new();
        hash.write_u64(FORMAT_VERSION as u64);
        hash.write_u64(inputs.kind as u64);
        hash.write_u64(inputs.kind.version() as u64);
        hash.write_u64(inputs.archive_stamp);
        hash.write_u64(inputs.entry_header.0 as u64);
        hash.write_u64(inputs.entry_header.1 as u64);
        hash.write_u64(inputs.params);
        hash.write_bytes(inputs.entry_path.as_bytes());
        Some(hash.finish())
    }

    /// A key for something that is not an archive entry at all — an
    /// install-scoped index, named by a string.
    pub fn named(kind: Kind, install_stamp: u64, name: &str) -> Option<Self> {
        if install_stamp == 0 {
            return None;
        }
        let mut hash = Hasher::new();
        hash.write_u64(FORMAT_VERSION as u64);
        hash.write_u64(kind as u64);
        hash.write_u64(kind.version() as u64);
        hash.write_u64(install_stamp);
        hash.write_bytes(name.as_bytes());
        Some(hash.finish())
    }

    pub fn to_bytes(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&self.0.to_le_bytes());
        out[8..].copy_from_slice(&self.1.to_le_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8; 16]) -> Self {
        let mut low = [0u8; 8];
        let mut high = [0u8; 8];
        low.copy_from_slice(&bytes[..8]);
        high.copy_from_slice(&bytes[8..]);
        Self(u64::from_le_bytes(low), u64::from_le_bytes(high))
    }
}

/// A 128-bit hash built from two independently seeded 64-bit mixes.
///
/// Not a general-purpose hash and not cryptographic; the inputs are our own and
/// nobody is choosing them adversarially. What it does have to be is
/// **identical across runs and across builds**, because a key that changes
/// shape between launches turns the whole cache into a miss. That rules out
/// `DefaultHasher`, which explicitly does not promise stability, and it is why
/// the constants are written out here rather than pulled from a dependency that
/// could change its algorithm in a point release.
struct Hasher {
    a: u64,
    b: u64,
}

impl Hasher {
    fn new() -> Self {
        // Two different odd seeds, so the halves never move together.
        Self {
            a: 0x9e37_79b9_7f4a_7c15,
            b: 0xc2b2_ae3d_27d4_eb4f,
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.a = mix(self.a ^ value, 0xff51_afd7_ed55_8ccd);
        self.b = mix(self.b.rotate_left(23) ^ value, 0xc4ce_b9fe_1a85_ec53);
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        // Length first, so "ab"+"c" and "a"+"bc" cannot hash alike.
        self.write_u64(bytes.len() as u64);
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let mut word = [0u8; 8];
            word.copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    fn finish(mut self) -> CacheKey {
        self.a = mix(self.a ^ self.b, 0xff51_afd7_ed55_8ccd);
        self.b = mix(self.b ^ self.a.rotate_left(31), 0xc4ce_b9fe_1a85_ec53);
        CacheKey(self.a, self.b)
    }
}

fn mix(mut x: u64, constant: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(constant);
    x ^= x >> 29;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> KeyInputs<'static> {
        KeyInputs {
            kind: Kind::Model,
            archive_stamp: 0x1234_5678_9abc_def0,
            entry_header: (0x1000, 0x2000),
            entry_path: r"x64a.rpf\props\prop_bin_04a.ydr",
            params: 7,
        }
    }

    #[test]
    fn the_same_inputs_give_the_same_key() {
        assert_eq!(CacheKey::of(inputs()), CacheKey::of(inputs()));
    }

    /// The property the whole cache rests on. If any of these did not move the
    /// key, the cache would serve one asset's bytes for another's.
    #[test]
    fn every_input_changes_the_key() {
        let base = CacheKey::of(inputs()).unwrap();

        let mut kind = inputs();
        kind.kind = Kind::Texture;
        assert_ne!(CacheKey::of(kind).unwrap(), base, "kind");

        let mut stamp = inputs();
        stamp.archive_stamp += 1;
        assert_ne!(CacheKey::of(stamp).unwrap(), base, "archive stamp");

        let mut offset = inputs();
        offset.entry_header.0 += 1;
        assert_ne!(CacheKey::of(offset).unwrap(), base, "entry offset");

        let mut size = inputs();
        size.entry_header.1 += 1;
        assert_ne!(CacheKey::of(size).unwrap(), base, "entry size");

        let mut path = inputs();
        path.entry_path = r"x64a.rpf\props\prop_bin_04b.ydr";
        assert_ne!(CacheKey::of(path).unwrap(), base, "entry path");

        let mut params = inputs();
        params.params += 1;
        assert_ne!(CacheKey::of(params).unwrap(), base, "params");
    }

    /// An unreadable archive is not evidence that nothing changed, so it must
    /// not be cached against at all.
    #[test]
    fn refuses_an_unknown_archive_stamp() {
        let mut unknown = inputs();
        unknown.archive_stamp = 0;
        assert_eq!(CacheKey::of(unknown), None);
        assert_eq!(CacheKey::named(Kind::Index, 0, "assets"), None);
    }

    #[test]
    fn a_named_key_depends_on_its_name() {
        let a = CacheKey::named(Kind::Index, 99, "assets").unwrap();
        let b = CacheKey::named(Kind::Index, 99, "names").unwrap();
        let c = CacheKey::named(Kind::Index, 100, "assets").unwrap();
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    /// Path lengths are hashed, so a boundary shift cannot alias.
    #[test]
    fn paths_that_split_the_same_bytes_differently_do_not_collide() {
        let mut a = inputs();
        a.entry_path = "ab";
        let mut b = inputs();
        b.entry_path = "a\u{0}b";
        assert_ne!(CacheKey::of(a).unwrap(), CacheKey::of(b).unwrap());
    }

    #[test]
    fn round_trips_through_bytes() {
        let key = CacheKey::of(inputs()).unwrap();
        assert_eq!(CacheKey::from_bytes(&key.to_bytes()), key);
    }

    /// The halves must be independent; a hash whose 128 bits are really 64
    /// buys nothing over a `u64`.
    #[test]
    fn the_two_halves_are_not_the_same_number() {
        for offset in 0..64u32 {
            let mut varied = inputs();
            varied.entry_header.0 = offset;
            let key = CacheKey::of(varied).unwrap();
            assert_ne!(key.0, key.1, "halves collapsed at {offset}");
        }
    }

    /// Not a distribution test, just a smoke check that nearby inputs do not
    /// produce nearby keys — which is what would happen with a weak mix.
    #[test]
    fn sequential_entries_spread_across_the_space() {
        let keys: std::collections::HashSet<CacheKey> = (0..1000u32)
            .map(|i| {
                let mut varied = inputs();
                varied.entry_header.0 = i * 2048;
                CacheKey::of(varied).unwrap()
            })
            .collect();
        assert_eq!(
            keys.len(),
            1000,
            "1,000 sequential entries must be distinct"
        );
    }
}
