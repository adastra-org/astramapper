//! RSC7 resources: the paged pointer format the geometry and texture files use.
//!
//! Port of `CodeWalker.Core/GameFiles/Resources/`. Only the drawable and
//! texture paths are here so far — see `docs/formats.md` for what each sibling
//! still owes.

pub mod bounds;
pub mod clip;
pub mod drawable;
pub mod reader;
pub mod texture;
pub mod vertex;

pub use bounds::{
    Bounds, BoundsData, BoundsType, BoundComposite, BoundGeomOctants, BoundGeometry, BoundMaterial,
    BoundPolygon, Bvh, CollisionMesh, CollisionPart, Matrix4F,
};
pub use clip::{Animation, AnimationBoneId, Clip, ClipDictionary, ClipType};
pub use drawable::{
    Aabb, Drawable, DrawableDictionary, DrawableGeometry, DrawableModel, Lod, ShaderFx,
};
pub use reader::{version_from_flags, ResourceReader, GRAPHICS_BASE, SYSTEM_BASE};
pub use texture::{MipLevel, Texture, TextureDictionary, TextureFormat, TextureUsage};
pub use vertex::{ComponentType, VertexDeclaration, VertexSemantic};

use crate::filetype::FileKind;
use crate::utils::{gen_hash, hash_string};
use crate::{CwError, Result};

/// Read whatever drawables a payload holds.
///
/// `system_size` is the entry's system page size — the payload's split point.
/// A `.ydr` yields one drawable, a `.ydd` yields however many the dictionary
/// holds. Anything else is refused rather than guessed at: `.yft` and `.ypt`
/// also embed drawables, but behind `Frag.cs` and `Particle.cs`, which are not
/// ported.
pub fn read_drawables(kind: FileKind, data: &[u8], system_size: usize) -> Result<Vec<Drawable>> {
    let mut r = ResourceReader::new(data, system_size);
    match kind {
        FileKind::Model => Ok(vec![Drawable::read(&mut r, true)?]),
        FileKind::ModelDictionary => Ok(DrawableDictionary::read(&mut r)?.drawables),
        _ => Err(CwError::Invalid {
            what: "drawable",
            detail: format!("no drawable reader for a {} file", kind.label()),
        }),
    }
}

/// One drawable and the name the file gives it.
#[derive(Debug, Clone)]
pub struct NamedDrawable {
    /// Position in the dictionary. A `.ydr` has exactly one, at zero.
    pub index: usize,
    /// Jenkins hash of the name, from the dictionary's parallel hash list.
    pub hash: u32,
    /// The resolved name: the drawable's own when it matches the hash, else
    /// `hash_XXXXXXXX`. See [`name_drawables`].
    pub name: String,
    pub drawable: Drawable,
}

/// Read a payload as a named dictionary of drawables.
///
/// Port of `YddFile.Load`, which is where a `.ydd`'s drawables acquire their
/// names — the dictionary keeps a hash list parallel to the drawable list, and
/// `read_drawables` alone throws that half away.
///
/// A `.ydr` goes through here too, as a dictionary of one: `YdrFile` has no
/// hash list, so the hash is derived from the drawable's own name the way
/// `JenkHash.GenHash` would have produced it.
pub fn read_named_drawables(
    kind: FileKind,
    data: &[u8],
    system_size: usize,
) -> Result<Vec<NamedDrawable>> {
    let mut r = ResourceReader::new(data, system_size);
    match kind {
        FileKind::Model => {
            let drawable = Drawable::read(&mut r, true)?;
            let name = drawable.name.clone().unwrap_or_default();
            Ok(vec![NamedDrawable {
                index: 0,
                hash: gen_hash(&name),
                name,
                drawable,
            }])
        }
        FileKind::ModelDictionary => {
            let dictionary = DrawableDictionary::read(&mut r)?;
            Ok(name_drawables(dictionary))
        }
        _ => Err(CwError::Invalid {
            what: "drawable",
            detail: format!("no drawable reader for a {} file", kind.label()),
        }),
    }
}

/// Pair a dictionary's drawables with its hash list.
///
/// The reference's rule, verbatim: a drawable keeps the name it carries only
/// when that name hashes to the entry's hash. Otherwise the hash *is* the name,
/// rendered as `hash_XXXXXXXX` — most dictionary entries carry no name at all,
/// and the few that do sometimes carry the source path of a different asset.
///
/// One divergence: `YddFile` keys a `Dictionary<uint, Drawable>` by hash and
/// then takes its values, so two entries sharing a hash collapse into one. This
/// keeps every entry at its own position, because a viewer that silently lists
/// nine of ten drawables is worse than one that shows a repeated name.
fn name_drawables(dictionary: DrawableDictionary) -> Vec<NamedDrawable> {
    dictionary
        .drawables
        .into_iter()
        .enumerate()
        .map(|(index, drawable)| {
            let hash = dictionary.hashes.get(index).copied().unwrap_or(0);
            let name = match &drawable.name {
                Some(name) if gen_hash(name) == hash => name.clone(),
                _ => hash_string(hash),
            };
            NamedDrawable {
                index,
                hash,
                name,
                drawable,
            }
        })
        .collect()
}

/// Read the clip dictionary a payload holds.
///
/// `.ycd` only. `.yed` and `.yfd` are also `FileKind::Clip` but hold expression
/// and frame-filter dictionaries, which are `Expression.cs` and `Filter.cs` and
/// are not ported.
pub fn read_clips(kind: FileKind, data: &[u8], system_size: usize) -> Result<ClipDictionary> {
    match kind {
        FileKind::ClipDictionary => {
            let mut r = ResourceReader::new(data, system_size);
            ClipDictionary::read(&mut r)
        }
        _ => Err(CwError::Invalid {
            what: "clip dictionary",
            detail: format!("no clip reader for a {} file", kind.label()),
        }),
    }
}

/// Read the texture dictionary a payload holds.
///
/// `.ytd` only. A `.ydr` also embeds one — `ShaderGroup.TextureDictionary` —
/// but that pointer is read past rather than into, so reaching it is a change
/// in `drawable.rs` rather than a case here.
pub fn read_textures(kind: FileKind, data: &[u8], system_size: usize) -> Result<TextureDictionary> {
    match kind {
        FileKind::Texture => {
            let mut r = ResourceReader::new(data, system_size);
            TextureDictionary::read(&mut r)
        }
        _ => Err(CwError::Invalid {
            what: "texture dictionary",
            detail: format!("no texture reader for a {} file", kind.label()),
        }),
    }
}

/// Read the single collision bounds block a payload holds.
///
/// `.ybn` only. The root of a `.ybn` is one `Bounds`, per `YbnFile.Load`; a
/// `.ydr` and a `.yft` also carry bounds, but behind pointers the drawable and
/// fragment readers do not follow yet.
pub fn read_bounds(kind: FileKind, data: &[u8], system_size: usize) -> Result<Bounds> {
    match kind {
        FileKind::Bounds => {
            let mut r = ResourceReader::new(data, system_size);
            Bounds::read(&mut r)
        }
        _ => Err(CwError::Invalid {
            what: "bounds",
            detail: format!("no bounds reader for a {} file", kind.label()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::reader::SYSTEM_BASE;

    /// A dictionary of two drawables: one whose own name matches its hash, one
    /// whose name is a leftover from somewhere else.
    fn synthetic_dictionary() -> Vec<u8> {
        let mut d = vec![0u8; 0x500];
        let put_u16 = |d: &mut Vec<u8>, at: usize, v: u16| {
            d[at..at + 2].copy_from_slice(&v.to_le_bytes());
        };
        let put_u32 = |d: &mut Vec<u8>, at: usize, v: u32| {
            d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put_u64 = |d: &mut Vec<u8>, at: usize, v: u64| {
            d[at..at + 8].copy_from_slice(&v.to_le_bytes());
        };
        let addr = |offset: u64| SYSTEM_BASE + offset;

        put_u64(&mut d, 0x20, addr(0x100)); // hashes
        put_u16(&mut d, 0x28, 2);
        put_u64(&mut d, 0x30, addr(0x110)); // drawable pointers
        put_u16(&mut d, 0x38, 2);

        put_u32(&mut d, 0x100, gen_hash("prop_bench_01a"));
        put_u32(&mut d, 0x104, 0x1234_5678);

        put_u64(&mut d, 0x110, addr(0x200));
        put_u64(&mut d, 0x118, addr(0x300));

        put_u64(&mut d, 0x200 + 0xA8, addr(0x400));
        put_u64(&mut d, 0x300 + 0xA8, addr(0x420));
        d[0x400..0x400 + 14].copy_from_slice(b"prop_bench_01a");
        d[0x420..0x420 + 9].copy_from_slice(b"leftovers");

        d
    }

    /// `YddFile.Load`'s rule: keep the carried name only when it hashes to the
    /// entry's hash, otherwise the hash is the name.
    #[test]
    fn names_dictionary_drawables_from_the_hash_list() {
        let data = synthetic_dictionary();
        let entries =
            read_named_drawables(FileKind::ModelDictionary, &data, data.len()).expect("read");

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "prop_bench_01a");
        assert_eq!(entries[0].hash, gen_hash("prop_bench_01a"));
        assert_eq!(entries[1].name, "hash_12345678");
        assert_eq!(entries[1].drawable.name.as_deref(), Some("leftovers"));
    }

    /// A `.ydr` has no hash list, so its one drawable is keyed by its own name.
    #[test]
    fn reads_a_single_drawable_as_a_dictionary_of_one() {
        let mut data = vec![0u8; 0x200];
        data[0xA8..0xB0].copy_from_slice(&(SYSTEM_BASE + 0x100).to_le_bytes());
        data[0x100..0x100 + 5].copy_from_slice(b"bench");

        let entries = read_named_drawables(FileKind::Model, &data, data.len()).expect("read");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "bench");
        assert_eq!(entries[0].hash, gen_hash("bench"));
    }

    #[test]
    fn refuses_a_kind_with_no_drawable_reader() {
        assert!(read_named_drawables(FileKind::Texture, &[0u8; 64], 64).is_err());
    }
}
