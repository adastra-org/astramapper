//! Metadata: the three interchangeable encodings a `.ymt` can be in.
//!
//! Port of `YmtFile.Load` from
//! `CodeWalker.Core/GameFiles/FileTypes/YmtFile.cs`, plus the readers it
//! dispatches to. A `.ymt` — and a `.ymf`, and the `.pso` files — is one of:
//!
//! | Encoding | How it is recognised | Reader |
//! | --- | --- | --- |
//! | RSC | the archive entry is a *resource*, not a binary file | [`rsc`] |
//! | RBF | the payload starts with `RBF0` | [`rbf`] |
//! | PSO | the payload starts with `PSIN` | [`pso`] |
//!
//! That first test is the one worth stating: **the container is decided by the
//! archive entry, not by the bytes**. A resource-encoded `.ymt` has no magic of
//! its own — it is an RSC7 payload whose system segment starts with a `Meta`
//! block — so the same file name means different things depending on how it was
//! stored. `YmtFile.Load` tests `entry is RpfResourceFileEntry` first for
//! exactly this reason.
//!
//! What is not ported: the *typed* readers on top. The reference recognises
//! three root structures (`CScenarioPointRegion`, `CScenarioPointManifest`,
//! `CMapParentTxds`) and builds world objects out of them, backed by the
//! generated `MetaTypes.cs` and `PsoTypes.cs` tables. Every other `.ymt` in the
//! game falls through as a generic tree, which is what [`tree`] produces here
//! for all of them. Nothing about that is lossy — a PSO and an RSC meta both
//! carry the schema needed to read them — but it does mean no scenario editing.

pub mod pso;
pub mod rbf;
pub mod rsc;
pub mod tree;
pub mod typed;

pub use pso::PsoFile;
pub use rbf::RbfStructure;
pub use rsc::Meta;
pub use tree::MetaNode;
pub use typed::{
    convert_data_array, convert_data_array_ptr, decode_at, get_byte_array, get_float_array,
    get_hash_array, get_pointer_array, get_string, get_strings, get_typed_data,
    get_typed_pointer_array, get_uint_array, get_vec4_array, Array, AssetType, BaseArchetypeDef,
    BlockDesc, BoxOccluder, CarGen, CompositeEntityType, DataBlockPointer, Decode, DistantLodLight,
    EntityDef, GrassInstanceData, GrassInstanceListDef, InstancedMapData, LodLevel, LodLight,
    MapData, MloArchetypeDef, MloEntitySet, MloInstanceDef, MloPortalDef, MloRoomDef,
    MloTimeCycleModifier, OccludeModel, PriorityLevel, SpdAabb, TimeArchetypeDef,
    TimeCycleModifier, MapTypes,
};

use crate::filetype::FileKind;
use crate::resources::reader::ResourceReader;
use crate::{CwError, Result};

/// Which of the three encodings a metadata file turned out to be.
#[derive(Debug, Clone)]
pub enum MetaFile {
    /// RSC: the resource-paged encoding, also used by `.ymap` and `.ytyp`.
    Rsc(Meta),
    /// PSO: big-endian, schema in the file, one flat data blob.
    Pso(PsoFile),
    /// RBF: the token stream.
    Rbf(RbfStructure),
}

impl MetaFile {
    /// `RSC`, `PSO` or `RBF`, as `YmtFileFormat` names them.
    pub fn format(&self) -> &'static str {
        match self {
            Self::Rsc(_) => "RSC",
            Self::Pso(_) => "PSO",
            Self::Rbf(_) => "RBF",
        }
    }

    /// The name of the root structure: what this file *is*, e.g.
    /// `CScenarioPointRegion` or `CMapParentTxds`.
    pub fn root_name(&self) -> String {
        match self {
            Self::Rsc(meta) => crate::utils::hash_string(meta.root_structure().unwrap_or(0)),
            Self::Pso(pso) => pso
                .block(pso.root_id)
                .map(|b| crate::utils::hash_string(b.name_hash))
                .unwrap_or_default(),
            Self::Rbf(root) => root.name.clone(),
        }
    }

    /// Whether there is anything to show.
    pub fn has_content(&self) -> bool {
        match self {
            Self::Rsc(meta) => meta.has_content(),
            Self::Pso(pso) => !pso.blocks.is_empty() && !pso.structures.is_empty(),
            Self::Rbf(root) => !root.children.is_empty() || !root.attributes.is_empty(),
        }
    }

    /// Walk the file into a tree of named values.
    pub fn to_tree(&self) -> MetaNode {
        match self {
            Self::Rsc(meta) => tree::from_meta(meta),
            Self::Pso(pso) => tree::from_pso(pso),
            Self::Rbf(root) => tree::from_rbf(root),
        }
    }
}

/// Read a metadata payload.
///
/// `system_size` is the entry's system page size when the entry is a resource,
/// and `None` when it is a binary file. That is the discriminator, not a hint:
/// see the module note.
pub fn read_meta(kind: FileKind, data: &[u8], system_size: Option<usize>) -> Result<MetaFile> {
    if !matches!(kind, FileKind::Meta) {
        return Err(CwError::Invalid {
            what: "metadata",
            detail: format!("no metadata reader for a {} file", kind.label()),
        });
    }

    if let Some(system_size) = system_size {
        let mut r = ResourceReader::new(data, system_size);
        return Ok(MetaFile::Rsc(Meta::read(&mut r)?));
    }
    if rbf::is_rbf(data) {
        return Ok(MetaFile::Rbf(rbf::read(data)?));
    }
    if pso::is_pso(data) {
        return Ok(MetaFile::Pso(pso::read(data)?));
    }

    Err(CwError::Invalid {
        what: "metadata",
        detail: "not a resource, and neither RBF nor PSO".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_kind_with_no_metadata_reader() {
        assert!(read_meta(FileKind::Texture, &[0u8; 64], None).is_err());
    }

    /// A binary entry that is neither RBF nor PSO is refused rather than
    /// guessed at as a resource: without page flags there is no split point.
    #[test]
    fn refuses_a_binary_payload_with_no_magic() {
        let error = read_meta(FileKind::Meta, b"not metadata at all", None).unwrap_err();
        assert!(format!("{error}").contains("neither RBF nor PSO"));
    }

    /// A resource entry has no magic of its own, so the entry decides.
    #[test]
    fn reads_a_resource_entry_as_rsc_without_looking_at_the_bytes() {
        let data = vec![0u8; 0x100];
        let file = read_meta(FileKind::Meta, &data, Some(data.len())).unwrap();
        assert_eq!(file.format(), "RSC");
        assert!(!file.has_content());
    }
}
