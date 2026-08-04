//! MLO interior expansion: placing an interior's contents in world space.
//!
//! Port of `MloInstanceData.CreateYmapEntities` from
//! `CodeWalker.Core/World/Entities/MloInstanceData.cs`. A `.ymap` places MLO
//! *instances* — a position, a rotation and the archetype hash — but the
//! interior's actual contents (furniture, props) live in the MLO archetype's
//! own entity list inside a `.ytyp`, in the interior's local space. This module
//! expands those local entities into world space at the instance's transform,
//! which is the step a world view needs to show interiors as placed content.
//!
//! The transform mirrors the reference: `world = R * local + T` for position
//! and `world = R_instance * R_local` for rotation — no instance scale, which
//! `CreateYmapEntities` does not apply.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use rayon::prelude::*;
use serde::Serialize;

use crate::filetype::FileKind;
use crate::filetypes::{read_ytyp, Archetype, MloArchetype, MloEntity};
use crate::meta::typed::{AssetType, LodLevel};
use crate::meta::{read_meta, MetaFile};
use crate::rpf::{EntryRef, RpfManager};
use crate::utils::GtaKeys;

/// One interior entity, expanded into world space.
#[derive(Debug, Clone)]
pub struct MloWorldEntity {
    /// The interior entity's own archetype, resolved by the caller.
    pub archetype_name: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale_xy: f32,
    pub scale_z: f32,
    pub lod_level: LodLevel,
    pub lod_dist: f32,
}

/// Expand an MLO interior's entities into world space at an instance's
/// transform. Pure of any archive I/O, so the transform math is unit-testable.
pub fn expand_mlo_entities(
    instance_position: [f32; 3],
    instance_rotation: [f32; 4],
    interior: &[MloEntity],
) -> Vec<MloWorldEntity> {
    let rotation = Quat::from_array(instance_rotation);
    let translation = Vec3::from_array(instance_position);
    interior
        .iter()
        .map(|entry| {
            let local = Vec3::from_array(entry.entity.position);
            let world = rotation * local + translation;
            let combined = rotation * Quat::from_array(entry.entity.rotation);
            MloWorldEntity {
                archetype_name: entry.entity.archetype_name,
                position: world.to_array(),
                rotation: combined.to_array(),
                scale_xy: entry.entity.scale_xy,
                scale_z: entry.entity.scale_z,
                lod_level: entry.entity.lod_level,
                lod_dist: entry.entity.lod_dist,
            }
        })
        .collect()
}

/// What a placed entity's archetype says about the asset behind it.
///
/// A `.ymap` entity names an archetype and nothing else; everything about what
/// to actually load — which model, which texture dictionary, which collision —
/// lives on the archetype. The reference reaches these through
/// `GameFileCache.GetArchetype`; this is the same information, flattened, and
/// deliberately small so indexing all of them costs a few MB rather than
/// holding every `.ytyp` in memory.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeInfo {
    pub name: u32,
    /// The file to load, when it differs from `name`.
    pub asset_name: u32,
    pub asset_type: AssetType,
    /// The `.ydd` holding the model, when it is not a standalone `.ydr`.
    pub drawable_dictionary: u32,
    /// The `.ytd` holding the pixels. This is what the reference consults
    /// before falling back to scanning a folder.
    pub texture_dictionary: u32,
    /// The `.ybn` holding the collision bounds.
    pub physics_dictionary: u32,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub lod_dist: f32,
    pub flags: u32,
    /// True for a `CMloArchetypeDef`, an interior.
    pub is_mlo: bool,
}

/// Everything one pass over the install's `.ytyp` files yields.
///
/// Both indexes come out of the same pass because reading 2,759 `.ytyp` files
/// is the expensive part and neither index is worth a second one.
#[derive(Debug, Default)]
pub struct ArchetypeIndexes {
    /// Interiors, keyed as an MLO instance's `archetypeName` refers to them.
    pub mlo: HashMap<u32, MloArchetype>,
    /// Every archetype, interiors included, keyed the same way.
    pub all: HashMap<u32, ArchetypeInfo>,
}

impl ArchetypeIndexes {
    fn absorb(&mut self, archetypes: Vec<Archetype>) {
        for archetype in archetypes {
            let hash = archetype.hash();
            let (base, is_mlo) = match &archetype {
                Archetype::Base(base) => (base, false),
                Archetype::Time(time) => (&time.base, false),
                Archetype::Mlo(mlo) => (&mlo.base, true),
            };
            self.all.insert(
                hash,
                ArchetypeInfo {
                    name: base.name,
                    asset_name: base.asset_name,
                    asset_type: base.asset_type,
                    drawable_dictionary: base.drawable_dictionary,
                    texture_dictionary: base.texture_dictionary,
                    physics_dictionary: base.physics_dictionary,
                    bb_min: base.bb_min,
                    bb_max: base.bb_max,
                    lod_dist: base.lod_dist,
                    flags: base.flags,
                    is_mlo,
                },
            );
            if let Archetype::Mlo(mlo) = archetype {
                self.mlo.insert(hash, mlo);
            }
        }
    }

    fn merge(mut self, other: Self) -> Self {
        self.mlo.extend(other.mlo);
        self.all.extend(other.all);
        self
    }
}

/// Index every archetype in the install by its lookup hash.
///
/// The map keys on the archetype's `hash()` — `assetName`, else `name` — the
/// same value a `.ymap` entity's `archetypeName` carries.
///
/// Parallel across `.ytyp` files. The reference is serial and so was this;
/// there are 2,759 of them and reading one tells you nothing about the next,
/// so the whole pass used to stall the first map that placed an interior. The
/// fold-then-reduce shape keeps peak memory to one partial index per thread
/// rather than every file's archetypes at once.
pub fn index_archetypes(manager: &RpfManager, keys: &GtaKeys) -> ArchetypeIndexes {
    let refs: Vec<(EntryRef, usize)> = manager
        .archives
        .iter()
        .flat_map(|archive| archive.entries.iter())
        .filter(|entry| entry.name_lower.ends_with(".ytyp"))
        .filter_map(|entry| {
            let reference = manager.find(&entry.path)?;
            let (system_size, _) = entry.resource_segments()?;
            Some((reference, system_size))
        })
        .collect();

    refs.par_iter()
        .fold(ArchetypeIndexes::default, |mut acc, (reference, size)| {
            acc.absorb(read_archetypes(manager, keys, *reference, *size));
            acc
        })
        .reduce(ArchetypeIndexes::default, ArchetypeIndexes::merge)
}

/// One `.ytyp`'s archetypes, or nothing if it will not read.
///
/// A `.ytyp` that fails to extract or parse costs its own archetypes and not
/// the pass: a modded install with one broken file must still index the other
/// 2,758.
fn read_archetypes(
    manager: &RpfManager,
    keys: &GtaKeys,
    reference: EntryRef,
    system_size: usize,
) -> Vec<Archetype> {
    let Ok(data) = manager.extract(reference, keys) else {
        return Vec::new();
    };
    let Ok(meta) = read_meta(FileKind::Meta, &data, Some(system_size)) else {
        return Vec::new();
    };
    match meta {
        MetaFile::Rsc(meta) => read_ytyp(&meta).map(|y| y.archetypes).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Index every MLO archetype in the install by its lookup hash.
///
/// The interiors half of [`index_archetypes`]. Prefer that when the caller
/// wants both, since the two share one pass over the `.ytyp` files.
pub fn index_mlo_archetypes(manager: &RpfManager, keys: &GtaKeys) -> HashMap<u32, MloArchetype> {
    index_archetypes(manager, keys).mlo
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::typed::{Array, EntityDef, PriorityLevel};
    use std::f32::consts::FRAC_1_SQRT_2;

    fn interior_entity(position: [f32; 3], rotation: [f32; 4]) -> MloEntity {
        MloEntity {
            entity: EntityDef {
                archetype_name: 0xAB,
                flags: 0,
                guid: 0,
                position,
                rotation,
                scale_xy: 1.0,
                scale_z: 1.0,
                parent_index: -1,
                lod_dist: 0.0,
                child_lod_dist: 0.0,
                lod_level: LodLevel::Hd,
                num_children: 0,
                priority_level: PriorityLevel::Required,
                extensions: Array {
                    pointer: 0,
                    count: 0,
                    capacity: 0,
                    pad: 0,
                },
                ambient_occlusion_multiplier: 0,
                artificial_ambient_occlusion: 0,
                tint_value: 0,
            },
            extensions: Vec::new(),
        }
    }

    fn identity() -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    #[test]
    fn identity_instance_leaves_positions_alone() {
        let out = expand_mlo_entities(
            [0.0, 0.0, 0.0],
            identity(),
            &[interior_entity([1.0, 2.0, 3.0], identity())],
        );
        assert_eq!(out[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(out[0].rotation, identity());
        assert_eq!(out[0].archetype_name, 0xAB);
    }

    #[test]
    fn instance_translation_moves_the_interior() {
        let out = expand_mlo_entities(
            [100.0, 0.0, 50.0],
            identity(),
            &[interior_entity([1.0, 2.0, 3.0], identity())],
        );
        assert_eq!(out[0].position, [101.0, 2.0, 53.0]);
    }

    #[test]
    fn quarter_turn_about_z_maps_x_to_y() {
        let qz = [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2];
        let out = expand_mlo_entities(
            [0.0, 0.0, 0.0],
            qz,
            &[interior_entity([1.0, 0.0, 0.0], identity())],
        );
        let [x, y, _] = out[0].position;
        assert!((x - 0.0).abs() < 1e-5, "x={x}");
        assert!((y - 1.0).abs() < 1e-5, "y={y}");
        // The world rotation is the instance's, since the local one is identity.
        assert!((out[0].rotation[2] - qz[2]).abs() < 1e-5);
        assert!((out[0].rotation[3] - qz[3]).abs() < 1e-5);
    }

    #[test]
    fn combined_rotation_stays_a_unit_quaternion() {
        let qx = [0.5, 0.0, 0.0, 0.8660254];
        let qz = [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2];
        let out = expand_mlo_entities([0.0, 0.0, 0.0], qz, &[interior_entity([0.0, 0.0, 0.0], qx)]);
        let [x, y, z, w] = out[0].rotation;
        let norm = (x * x + y * y + z * z + w * w).sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm={norm}");
    }
}
