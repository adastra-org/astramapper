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

use crate::filetype::FileKind;
use crate::filetypes::{read_ytyp, Archetype, MloArchetype, MloEntity};
use crate::meta::typed::LodLevel;
use crate::meta::{read_meta, MetaFile};
use crate::rpf::RpfManager;
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

/// Index every MLO archetype in the install by its lookup hash.
///
/// The map keys on the archetype's `hash()` — `assetName`, else `name` — the
/// same value an MLO instance's `archetypeName` carries, so an instance can
/// find its interior. Reading every `.ytyp` is the expensive part, so callers
/// build this once per scan and keep it.
pub fn index_mlo_archetypes(manager: &RpfManager, keys: &GtaKeys) -> HashMap<u32, MloArchetype> {
    let mut index = HashMap::new();
    for archive in &manager.archives {
        for entry in &archive.entries {
            if !entry.name_lower.ends_with(".ytyp") {
                continue;
            }
            let Some(reference) = manager.find(&entry.path) else {
                continue;
            };
            let Some((system_size, _)) = entry.resource_segments() else {
                continue;
            };
            let Ok(data) = manager.extract(reference, keys) else {
                continue;
            };
            let Ok(meta) = read_meta(FileKind::Meta, &data, Some(system_size)) else {
                continue;
            };
            let ytyp = match meta {
                MetaFile::Rsc(meta) => read_ytyp(&meta).ok(),
                _ => None,
            };
            let Some(ytyp) = ytyp else {
                continue;
            };
            for archetype in ytyp.archetypes {
                if let Archetype::Mlo(mlo) = archetype {
                    index.insert(mlo.base.hash(), mlo);
                }
            }
        }
    }
    index
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
