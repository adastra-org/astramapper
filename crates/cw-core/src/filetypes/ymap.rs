//! The `.ymap` reader: a placement dictionary as a typed model.
//!
//! Port of `YmapFile.Load` from
//! `CodeWalker.Core/GameFiles/FileTypes/YmapFile.cs`. A `.ymap` is an RSC meta
//! whose root structure is `CMapData`; it names every placed entity, plus the
//! car generators, timecycle modifiers, occluders, LOD lights and the entity
//! hierarchy that joins parent entities to their children.
//!
//! Read-only, like the rest of the metadata layer.

use serde::Serialize;

use crate::meta::typed::{
    convert_data_array, get_byte_array, get_float_array, get_hash_array, get_pointer_array,
    get_strings, get_typed_data, get_typed_pointer_array, get_uint_array, get_vec4_array, Array,
    BlockDesc, BoxOccluder, CarGen, DistantLodLight, EntityDef, GrassInstanceData,
    GrassInstanceListDef, InstancedMapData, LodLight, MapData, MloInstanceDef, OccludeModel,
    TimeCycleModifier,
};
use crate::meta::Meta;
use crate::Result;

use super::names;

/// A parsed `.ymap`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YmapFile {
    pub name: u32,
    pub parent: u32,
    /// `flag 1` is the SCRIPTED flag (destruction maps).
    pub flags: u32,
    pub content_flags: u32,
    pub streaming_extents_min: [f32; 3],
    pub streaming_extents_max: [f32; 3],
    pub entities_extents_min: [f32; 3],
    pub entities_extents_max: [f32; 3],
    pub physics_dictionaries: Vec<u32>,
    pub strings: Vec<String>,
    /// Every entity, in the order the reference reads them: `CEntityDef`
    /// records first, then `CMloInstanceDef` records.
    pub entities: Vec<Entity>,
    /// The entities with no parent in this file.
    pub root_entities: Vec<usize>,
    /// The indices of the MLO instances among `entities`.
    pub mlo_entities: Vec<usize>,
    pub time_cycle_modifiers: Vec<TimeCycleModifier>,
    pub car_generators: Vec<CarGen>,
    pub box_occluders: Vec<BoxOccluder>,
    pub occlude_models: Vec<OccludeModel>,
    pub lod_lights: LodLights,
    pub distant_lod_lights: DistantLodLights,
    pub grass_instance_batches: Vec<GrassBatch>,
    pub block: BlockDesc,
}

/// One placed entity. A `.ymap`'s `entities` array is a mix of `CEntityDef`
/// and `CMloInstanceDef` records; the latter are interior instances.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    pub def: EntityDef,
    /// True when this is a `CMloInstanceDef`, i.e. an interior instance.
    pub is_mlo: bool,
    /// The `defaultEntitySets` of an MLO instance.
    pub default_entity_sets: Vec<u32>,
    pub extensions: Vec<crate::filetypes::ytyp::Extension>,
    pub children: Vec<usize>,
}

impl Entity {
    /// `YmapEntityDef.LodInParentYmap`: flag bit 3.
    pub fn lod_in_parent_ymap(&self) -> bool {
        (self.def.flags >> 3) & 1 != 0
    }
}

/// The LOD lights, resolved from the SOA in `CMapData`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LodLights {
    pub direction: Vec<[f32; 4]>,
    pub falloff: Vec<f32>,
    pub falloff_exponent: Vec<f32>,
    pub time_and_state_flags: Vec<u32>,
    pub hash: Vec<u32>,
    pub cone_inner_angle: Vec<u8>,
    pub cone_outer_angle_or_cap_ext: Vec<u8>,
    pub corona_intensity: Vec<u8>,
}

/// The distant LOD lights, resolved from their SOA.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistantLodLights {
    pub position: Vec<[f32; 4]>,
    pub colours: Vec<u32>,
    pub num_street_lights: u16,
    pub category: u16,
}

/// One grass instance batch, from `rage__fwGrassInstanceListDef`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrassBatch {
    pub batch_aabb_min: [f32; 4],
    pub batch_aabb_max: [f32; 4],
    pub archetype_name: u32,
    pub lod_dist: u32,
    pub lod_fade_start_dist: f32,
    pub lod_inst_fade_range: f32,
    pub orient_to_terrain: f32,
    pub instances: Vec<GrassInstanceData>,
}

/// Read a `.ymap` payload.
pub fn read_ymap(meta: &Meta) -> Result<YmapFile> {
    let Some(map_data) = get_typed_data::<MapData>(meta, names::C_MAP_DATA) else {
        return Err(crate::CwError::Invalid {
            what: "ymap",
            detail: "no CMapData block".into(),
        });
    };

    let physics_dictionaries =
        get_hash_array(meta, &map_data.physics_dictionaries).unwrap_or_default();
    let strings = get_strings(meta).unwrap_or_default();

    let (entities, root_entities, mlo_entities) = ensure_entities(meta, &map_data.entities);
    let time_cycle_modifiers = convert_data_array::<TimeCycleModifier>(
        meta,
        names::C_TIME_CYCLE_MODIFIER,
        map_data.time_cycle_modifiers.pointer,
        map_data.time_cycle_modifiers.count as u32,
    )
    .unwrap_or_default();
    let car_generators = convert_data_array::<CarGen>(
        meta,
        names::C_CAR_GEN,
        map_data.car_generators.pointer,
        map_data.car_generators.count as u32,
    )
    .unwrap_or_default();
    let box_occluders = convert_data_array::<BoxOccluder>(
        meta,
        names::BOX_OCCLUDER,
        map_data.box_occluders.pointer,
        map_data.box_occluders.count as u32,
    )
    .unwrap_or_default();
    let occlude_models = convert_data_array::<OccludeModel>(
        meta,
        names::OCCLUDE_MODEL,
        map_data.occlude_models.pointer,
        map_data.occlude_models.count as u32,
    )
    .unwrap_or_default();

    let lod_lights = ensure_lod_lights(meta, &map_data.lod_lights_soa);
    let distant_lod_lights = ensure_distant_lod_lights(meta, &map_data.distant_lod_lights_soa);
    let grass_instance_batches = ensure_grass(meta, &map_data.instanced_data);

    Ok(YmapFile {
        name: map_data.name,
        parent: map_data.parent,
        flags: map_data.flags,
        content_flags: map_data.content_flags,
        streaming_extents_min: map_data.streaming_extents_min,
        streaming_extents_max: map_data.streaming_extents_max,
        entities_extents_min: map_data.entities_extents_min,
        entities_extents_max: map_data.entities_extents_max,
        physics_dictionaries,
        strings,
        entities,
        root_entities,
        mlo_entities,
        time_cycle_modifiers,
        car_generators,
        box_occluders,
        occlude_models,
        lod_lights,
        distant_lod_lights,
        grass_instance_batches,
        block: map_data.block,
    })
}

/// `YmapFile.EnsureEntities`: split the pointer array into `CEntityDef` and
/// `CMloInstanceDef`, then build the parent/child hierarchy.
///
/// The hierarchy rule, verbatim from the reference: an entity is a root when
/// its parent index is out of range, when it is flagged `LodInParentYmap`,
/// when the parent's LOD level is <= its own, or when the parent is an
/// orphan-HD and the child is not. `AllEntities` order is CEntityDefs first,
/// then CMloInstanceDefs, and `parentIndex` indexes that combined list.
fn ensure_entities(meta: &Meta, entities: &Array) -> (Vec<Entity>, Vec<usize>, Vec<usize>) {
    let Some(eptrs) = get_pointer_array(meta, entities) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };

    let entity_defs = get_typed_pointer_array::<EntityDef>(meta, names::C_ENTITY_DEF, &eptrs);
    let mlo_defs =
        get_typed_pointer_array::<MloInstanceDef>(meta, names::C_MLO_INSTANCE_DEF, &eptrs);

    let entity_count = entity_defs.as_ref().map_or(0, Vec::len);
    let mlo_count = mlo_defs.as_ref().map_or(0, Vec::len);
    let total = entity_count + mlo_count;
    if total == 0 {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    // Build the combined list in reference order: entities, then MLO instances.
    let mut all: Vec<Entity> = Vec::with_capacity(total);
    if let Some(defs) = entity_defs {
        for def in defs {
            all.push(Entity {
                extensions: Vec::new(),
                children: Vec::new(),
                def,
                is_mlo: false,
                default_entity_sets: Vec::new(),
            });
        }
    }
    let mut mlo_indices = Vec::new();
    if let Some(defs) = mlo_defs {
        for def in defs {
            let sets = get_hash_array(meta, &def.default_entity_sets).unwrap_or_default();
            mlo_indices.push(all.len());
            all.push(Entity {
                extensions: Vec::new(),
                children: Vec::new(),
                def: def.entity,
                is_mlo: true,
                default_entity_sets: sets,
            });
        }
    }

    // The hierarchy pass.
    let mut roots: Vec<usize> = Vec::new();
    for i in 0..all.len() {
        let pind = all[i].def.parent_index;
        let is_root = if pind < 0 || pind as usize >= all.len() || all[i].lod_in_parent_ymap() {
            true
        } else {
            let p = &all[pind as usize];
            let d = &all[i];
            p.def.lod_level.raw() <= d.def.lod_level.raw()
                || (p.def.lod_level == crate::meta::typed::LodLevel::OrphanHd
                    && d.def.lod_level != crate::meta::typed::LodLevel::OrphanHd)
        };
        if is_root {
            roots.push(i);
        } else {
            all[pind as usize].children.push(i);
        }
    }

    // Extensions for every entity, read from the raw def's extension array.
    for entity in all.iter_mut() {
        entity.extensions = crate::filetypes::ytyp::get_extensions(meta, &entity.def.extensions);
    }

    (all, roots, mlo_indices)
}

/// `YmapFile.EnsureLODLights`.
fn ensure_lod_lights(meta: &Meta, soa: &LodLight) -> LodLights {
    if soa.direction.count == 0 {
        return LodLights::default();
    }
    LodLights {
        direction: get_vec4_array(meta, &soa.direction).unwrap_or_default(),
        falloff: get_float_array(meta, &soa.falloff).unwrap_or_default(),
        falloff_exponent: get_float_array(meta, &soa.falloff_exponent).unwrap_or_default(),
        time_and_state_flags: get_uint_array(meta, &soa.time_and_state_flags).unwrap_or_default(),
        hash: get_uint_array(meta, &soa.hash).unwrap_or_default(),
        cone_inner_angle: get_byte_array(meta, &soa.cone_inner_angle).unwrap_or_default(),
        cone_outer_angle_or_cap_ext: get_byte_array(meta, &soa.cone_outer_angle_or_cap_ext)
            .unwrap_or_default(),
        corona_intensity: get_byte_array(meta, &soa.corona_intensity).unwrap_or_default(),
    }
}

/// `YmapFile.EnsureDistantLODLights`.
fn ensure_distant_lod_lights(meta: &Meta, soa: &DistantLodLight) -> DistantLodLights {
    if soa.position.count == 0 {
        return DistantLodLights::default();
    }
    DistantLodLights {
        position: get_vec4_array(meta, &soa.position).unwrap_or_default(),
        colours: get_uint_array(meta, &soa.rgbi).unwrap_or_default(),
        num_street_lights: soa.num_street_lights,
        category: soa.category,
    }
}

/// `YmapFile.EnsureInstances`: the grass batches only — the prop list is read
/// for its count but not parsed, the way the reference leaves it.
fn ensure_grass(meta: &Meta, instanced: &InstancedMapData) -> Vec<GrassBatch> {
    if instanced.grass_instance_list.count == 0 {
        return Vec::new();
    }
    let Some(defs) = convert_data_array::<GrassInstanceListDef>(
        meta,
        names::RAGE_FW_GRASS_INSTANCE_LIST_DEF,
        instanced.grass_instance_list.pointer,
        instanced.grass_instance_list.count as u32,
    ) else {
        return Vec::new();
    };
    defs.into_iter()
        .map(|def| GrassBatch {
            batch_aabb_min: def.batch_aabb.min,
            batch_aabb_max: def.batch_aabb.max,
            archetype_name: def.archetype_name,
            lod_dist: def.lod_dist,
            lod_fade_start_dist: def.lod_fade_start_dist,
            lod_inst_fade_range: def.lod_inst_fade_range,
            orient_to_terrain: def.orient_to_terrain,
            instances: convert_data_array::<GrassInstanceData>(
                meta,
                names::RAGE_FW_GRASS_INSTANCE_LIST_DEF_INSTANCE_DATA,
                def.instance_list.pointer,
                def.instance_list.count as u32,
            )
            .unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::reader::{ResourceReader, SYSTEM_BASE};

    fn put_u16(d: &mut [u8], at: usize, v: u16) {
        d[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u32(d: &mut [u8], at: usize, v: u32) {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u64(d: &mut [u8], at: usize, v: u64) {
        d[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn put_f32(d: &mut [u8], at: usize, v: f32) {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn addr(o: u64) -> u64 {
        SYSTEM_BASE + o
    }

    /// A `.ymap` with a parent entity and its child: three blocks, two entity
    /// records, one POINTER block.
    fn synthetic_ymap() -> Vec<u8> {
        let mut d = vec![0u8; 0x900];
        put_u32(&mut d, 0x1C, 1); // root block
        put_u64(&mut d, 0x20, addr(0x300)); // structures
        put_u64(&mut d, 0x30, addr(0x500)); // blocks
        put_u16(&mut d, 0x48, 2); // structure count
        put_u16(&mut d, 0x4C, 3); // block count

        put_u32(&mut d, 0x300, names::C_MAP_DATA);
        put_u32(&mut d, 0x320, names::C_ENTITY_DEF);

        // Block 1 CMapData @0x700, block 2 POINTER @0x740, block 3 CEntityDef @0x760.
        put_u32(&mut d, 0x500, names::C_MAP_DATA);
        put_u32(&mut d, 0x504, 512);
        put_u64(&mut d, 0x508, addr(0x700));
        put_u32(&mut d, 0x510, crate::meta::rsc::POINTER_BLOCK);
        put_u32(&mut d, 0x514, 16);
        put_u64(&mut d, 0x518, addr(0x740));
        put_u32(&mut d, 0x520, names::C_ENTITY_DEF);
        put_u32(&mut d, 0x524, 256);
        put_u64(&mut d, 0x528, addr(0x760));

        // CMapData: name, entities array at 96 -> block 2, count 2.
        // Array layout: {u64 pointer; u16 count1; u16 count2; u32 pad}.
        put_u32(&mut d, 0x700 + 8, 0xCAFE);
        put_u32(&mut d, 0x700 + 12, 0xBEEF); // parent
        put_u64(&mut d, 0x700 + 96, 2);
        put_u16(&mut d, 0x700 + 96 + 8, 2);
        put_u16(&mut d, 0x700 + 96 + 10, 2);
        put_u32(&mut d, 0x700 + 96 + 12, 0);

        // POINTER block: two pointers to block 3 at offsets 0 and 128.
        put_u64(&mut d, 0x740, 3);
        put_u64(&mut d, 0x748, ((128u64) << 12) | 3);

        // Entity 0: parent, at offset 0. Level LOD (1) > child's HD (0), so
        // the hierarchy rule makes entity 1 a child rather than a root.
        put_u32(&mut d, 0x760 + 8, 0xAAAA); // archetypeName
        put_f32(&mut d, 0x760 + 32, 1.0);
        put_f32(&mut d, 0x760 + 36, 2.0);
        put_f32(&mut d, 0x760 + 40, 3.0);
        put_i32(&mut d, 0x760 + 72, -1); // parentIndex root
        put_i32(&mut d, 0x760 + 84, 1); // lodLevel LOD
                                        // Entity 1: child, at offset 128.
        let c = 0x760 + 128;
        put_u32(&mut d, c + 8, 0xBBBB);
        put_i32(&mut d, c + 72, 0); // parentIndex -> entity 0
        put_i32(&mut d, c + 84, 0); // lodLevel HD

        d
    }

    fn put_i32(d: &mut [u8], at: usize, v: i32) {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn reads_a_synthetic_ymap() {
        let data = synthetic_ymap();
        let mut r = ResourceReader::new(&data, data.len());
        let meta = Meta::read(&mut r).expect("meta");
        let ymap = read_ymap(&meta).expect("ymap");

        assert_eq!(ymap.name, 0xCAFE);
        assert_eq!(ymap.parent, 0xBEEF);
        assert_eq!(ymap.entities.len(), 2);
        assert_eq!(ymap.root_entities, vec![0]);
        assert_eq!(ymap.entities[1].children, Vec::<usize>::new());
        assert_eq!(ymap.entities[0].def.archetype_name, 0xAAAA);
        assert_eq!(ymap.entities[1].def.archetype_name, 0xBBBB);
        assert!(!ymap.entities[0].is_mlo);
    }
}
