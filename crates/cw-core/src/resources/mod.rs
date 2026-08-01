//! RSC7 resources: the paged pointer format the geometry and texture files use.
//!
//! Port of `CodeWalker.Core/GameFiles/Resources/`. Only the drawable path is
//! here so far — see `docs/formats.md` for what each sibling still owes.

pub mod drawable;
pub mod reader;
pub mod vertex;

pub use drawable::{
    Aabb, Drawable, DrawableDictionary, DrawableGeometry, DrawableModel, Lod, ShaderFx,
};
pub use reader::{version_from_flags, ResourceReader, GRAPHICS_BASE, SYSTEM_BASE};
pub use vertex::{ComponentType, VertexDeclaration, VertexSemantic};

use crate::filetype::FileKind;
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
