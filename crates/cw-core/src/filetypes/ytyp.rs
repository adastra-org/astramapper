//! The `.ytyp` reader: an archetype dictionary as a typed model.
//!
//! Port of `YtypFile.Load` from
//! `CodeWalker.Core/GameFiles/FileTypes/YtypFile.cs`. A `.ytyp` is an RSC meta
//! whose root structure is `CMapTypes`; it names a list of archetypes — the
//! typed records that tell the engine (and a world view) what a model or an
//! interior is. The reference walks the archetype pointer array and dispatches
//! on each target block's structure name, building `Archetype`,
//! `TimeArchetype` or `MloArchetype`; the latter also reads its children —
//! rooms, portals, entity sets and timecycle modifiers.
//!
//! Read-only, like the rest of the metadata layer. `MetaBuilder` writing is not
//! ported.

use serde::Serialize;

use crate::meta::typed::{
    convert_data_array, convert_data_array_ptr, decode_at, get_pointer_array, get_string,
    get_strings, get_typed_data, get_uint_array, get_vec4_array, Array, AssetType,
    BaseArchetypeDef, CompositeEntityType, EntityDef, MapTypes, MloArchetypeDef, MloEntitySet,
    MloPortalDef, MloRoomDef, MloTimeCycleModifier, TimeArchetypeDef,
};
use crate::meta::Meta;
use crate::Result;

use super::names;

/// A parsed `.ytyp`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtypFile {
    /// The file's own name hash, from `CMapTypes.name`.
    pub name_hash: u32,
    /// Every string the file carries.
    pub strings: Vec<String>,
    pub archetypes: Vec<Archetype>,
    /// The `MetaWrapper[]` `MetaTypes.GetExtensions` builds for the map types.
    pub extensions: Vec<Extension>,
    pub composite_entity_types: Vec<CompositeEntityType>,
}

/// One archetype, discriminated by the block it came from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Archetype {
    /// `CBaseArchetypeDef`.
    Base(BaseArchetype),
    /// `CTimeArchetypeDef`.
    Time(TimeArchetype),
    /// `CMloArchetypeDef`.
    Mlo(MloArchetype),
}

impl Archetype {
    /// The `assetName` hash — what the engine loads — falling back to `name`.
    pub fn hash(&self) -> u32 {
        match self {
            Self::Base(a) => a.hash(),
            Self::Time(a) => a.base.hash(),
            Self::Mlo(a) => a.base.hash(),
        }
    }
}

/// The shared first half of every archetype.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseArchetype {
    pub name: u32,
    pub asset_name: u32,
    pub asset_type: AssetType,
    pub drawable_dictionary: u32,
    pub texture_dictionary: u32,
    pub clip_dictionary: u32,
    pub physics_dictionary: u32,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub bs_centre: [f32; 3],
    pub bs_radius: f32,
    pub lod_dist: f32,
    pub hd_texture_dist: f32,
    pub flags: u32,
    pub extensions: Vec<Extension>,
}

impl BaseArchetype {
    fn from_def(def: BaseArchetypeDef) -> Self {
        Self {
            name: def.name,
            asset_name: def.asset_name,
            asset_type: def.asset_type,
            drawable_dictionary: def.drawable_dictionary,
            texture_dictionary: def.texture_dictionary,
            clip_dictionary: def.clip_dictionary,
            physics_dictionary: def.physics_dictionary,
            bb_min: def.bb_min,
            bb_max: def.bb_max,
            bs_centre: def.bs_centre,
            bs_radius: def.bs_radius,
            lod_dist: def.lod_dist,
            hd_texture_dist: def.hd_texture_dist,
            flags: def.flags,
            extensions: Vec::new(),
        }
    }

    /// The `Hash` an `Archetype` exposes: `assetName`, else `name`.
    pub fn hash(&self) -> u32 {
        if self.asset_name != 0 {
            self.asset_name
        } else {
            self.name
        }
    }
}

/// `CTimeArchetypeDef`: a base archetype plus the hours it is active.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeArchetype {
    pub base: BaseArchetype,
    pub time_flags: u32,
}

/// `CMloArchetypeDef`: an interior. The base archetype plus its rooms, portals,
/// entity sets and timecycle modifiers — read by `LoadChildren`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloArchetype {
    pub base: BaseArchetype,
    pub mlo_flags: u32,
    pub entities: Vec<MloEntity>,
    pub rooms: Vec<MloRoom>,
    pub portals: Vec<MloPortal>,
    pub entity_sets: Vec<MloEntitySetData>,
    pub time_cycle_modifiers: Vec<MloTimeCycleModifier>,
}

/// One entity inside an MLO.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloEntity {
    pub entity: EntityDef,
    pub extensions: Vec<Extension>,
}

/// An MLO room, resolved from `CMloRoomDef`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloRoom {
    pub name: Option<String>,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub blend: f32,
    pub timecycle_name: u32,
    pub secondary_timecycle_name: u32,
    pub flags: u32,
    pub portal_count: u32,
    pub floor_id: i32,
    pub exterior_visibilty_depth: i32,
    pub attached_objects: Vec<u32>,
}

/// An MLO portal, resolved from `CMloPortalDef`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloPortal {
    pub room_from: u32,
    pub room_to: u32,
    pub flags: u32,
    pub mirror_priority: u32,
    pub opacity: u32,
    pub audio_occlusion: u32,
    /// The portal corners, read with 16-byte stride.
    pub corners: Vec<[f32; 4]>,
    pub attached_objects: Vec<u32>,
}

/// An MLO entity set, resolved from `CMloEntitySet`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloEntitySetData {
    pub name: u32,
    pub locations: Vec<u32>,
    pub entities: Vec<EntityDef>,
}

/// An extension block: `MetaTypes.GetExtensions` dispatches on the block's
/// structure name and loads each of the known extension structures.
///
/// The reference builds a typed `MetaWrapper` per extension with its own
/// fields; this port keeps the common `name` and `offsetPosition` — the only
/// fields every extension type carries at the same offsets — and remembers
/// which structure it was, so a viewer can say what an entity is rigged with
/// without needing all fifteen struct definitions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    pub structure: u32,
    pub name: u32,
    pub offset_position: [f32; 3],
}

/// Read a `.ytyp` payload.
///
/// `system_size` is the entry's system page size — the resource split point.
/// The `.ytyp` is one of the RSC-meta containers, so this is
/// [`crate::meta::read_meta`]'s RSC path followed by the typed walk.
pub fn read_ytyp(meta: &Meta) -> Result<YtypFile> {
    let Some(map_types) = get_typed_data::<MapTypes>(meta, names::C_MAP_TYPES) else {
        return Err(crate::CwError::Invalid {
            what: "ytyp",
            detail: "no CMapTypes block".into(),
        });
    };

    let mut archetypes = Vec::new();
    if let Some(ptrs) = get_pointer_array(meta, &map_types.archetypes) {
        for ptr in ptrs {
            let Some(block) = meta.block(ptr.block_id) else {
                continue;
            };
            // The reference's dispatch: CBase, CTime, CMlo, else skip.
            let archetype = match block.structure_name_hash {
                names::C_BASE_ARCHETYPE_DEF => {
                    let Some(def) = decode_at::<BaseArchetypeDef>(&block.data, ptr.offset) else {
                        continue;
                    };
                    let mut arch = BaseArchetype::from_def(def.clone());
                    arch.extensions = get_extensions(meta, &def.extensions);
                    Archetype::Base(arch)
                }
                names::C_TIME_ARCHETYPE_DEF => {
                    let Some(def) = decode_at::<TimeArchetypeDef>(&block.data, ptr.offset) else {
                        continue;
                    };
                    let mut base = BaseArchetype::from_def(def.base.clone());
                    base.extensions = get_extensions(meta, &def.base.extensions);
                    Archetype::Time(TimeArchetype {
                        base,
                        time_flags: def.time_flags,
                    })
                }
                names::C_MLO_ARCHETYPE_DEF => {
                    let Some(def) = decode_at::<MloArchetypeDef>(&block.data, ptr.offset) else {
                        continue;
                    };
                    let mut base = BaseArchetype::from_def(def.base.clone());
                    base.extensions = get_extensions(meta, &def.base.extensions);
                    let mlo = load_mlo_children(meta, &def);
                    Archetype::Mlo(MloArchetype {
                        base,
                        mlo_flags: def.mlo.mlo_flags,
                        entities: mlo.0,
                        rooms: mlo.1,
                        portals: mlo.2,
                        entity_sets: mlo.3,
                        time_cycle_modifiers: mlo.4,
                    })
                }
                _ => continue,
            };
            archetypes.push(archetype);
        }
    }

    let extensions = get_extensions(meta, &map_types.extensions);
    let composite_entity_types = convert_data_array::<CompositeEntityType>(
        meta,
        names::C_COMPOSITE_ENTITY_TYPE,
        map_types.composite_entity_types.pointer,
        map_types.composite_entity_types.count as u32,
    )
    .unwrap_or_default();

    let strings = get_strings(meta).unwrap_or_default();

    Ok(YtypFile {
        name_hash: map_types.name,
        strings,
        archetypes,
        extensions,
        composite_entity_types,
    })
}

/// `MloArchetype.LoadChildren`: the rooms, portals, entity sets and modifiers.
type MloChildren = (
    Vec<MloEntity>,
    Vec<MloRoom>,
    Vec<MloPortal>,
    Vec<MloEntitySetData>,
    Vec<MloTimeCycleModifier>,
);

fn load_mlo_children(meta: &Meta, def: &MloArchetypeDef) -> MloChildren {
    let mut entities = Vec::new();
    if let Some(defs) =
        convert_data_array_ptr::<EntityDef>(meta, names::C_ENTITY_DEF, &def.mlo.entities)
    {
        for ent in defs {
            let extensions = get_extensions(meta, &ent.extensions);
            entities.push(MloEntity {
                entity: ent,
                extensions,
            });
        }
    }

    let mut rooms = Vec::new();
    if let Some(defs) = convert_data_array::<MloRoomDef>(
        meta,
        names::C_MLO_ROOM_DEF,
        def.mlo.rooms.pointer,
        def.mlo.rooms.count as u32,
    ) {
        for room in defs {
            let name = get_string(meta, &room.name);
            let attached_objects = get_uint_array(meta, &room.attached_objects).unwrap_or_default();
            rooms.push(MloRoom {
                name,
                bb_min: room.bb_min,
                bb_max: room.bb_max,
                blend: room.blend,
                timecycle_name: room.timecycle_name,
                secondary_timecycle_name: room.secondary_timecycle_name,
                flags: room.flags,
                portal_count: room.portal_count,
                floor_id: room.floor_id,
                exterior_visibilty_depth: room.exterior_visibilty_depth,
                attached_objects,
            });
        }
    }

    let mut portals = Vec::new();
    if let Some(defs) = convert_data_array::<MloPortalDef>(
        meta,
        names::C_MLO_PORTAL_DEF,
        def.mlo.portals.pointer,
        def.mlo.portals.count as u32,
    ) {
        for portal in defs {
            let corners = get_vec4_array(meta, &portal.corners).unwrap_or_default();
            let attached_objects =
                get_uint_array(meta, &portal.attached_objects).unwrap_or_default();
            portals.push(MloPortal {
                room_from: portal.room_from,
                room_to: portal.room_to,
                flags: portal.flags,
                mirror_priority: portal.mirror_priority,
                opacity: portal.opacity,
                audio_occlusion: portal.audio_occlusion,
                corners,
                attached_objects,
            });
        }
    }

    let mut entity_sets = Vec::new();
    if let Some(defs) = convert_data_array::<MloEntitySet>(
        meta,
        names::C_MLO_ENTITY_SET,
        def.mlo.entity_sets.pointer,
        def.mlo.entity_sets.count as u32,
    ) {
        for set in defs {
            let locations = get_uint_array(meta, &set.locations).unwrap_or_default();
            let entities =
                convert_data_array_ptr::<EntityDef>(meta, names::C_ENTITY_DEF, &set.entities)
                    .unwrap_or_default();
            entity_sets.push(MloEntitySetData {
                name: set.name,
                locations,
                entities,
            });
        }
    }

    let time_cycle_modifiers = convert_data_array::<MloTimeCycleModifier>(
        meta,
        names::C_MLO_TIME_CYCLE_MODIFIER,
        def.mlo.time_cycle_modifiers.pointer,
        def.mlo.time_cycle_modifiers.count as u32,
    )
    .unwrap_or_default();

    (entities, rooms, portals, entity_sets, time_cycle_modifiers)
}

/// `MetaTypes.GetExtensions`: walk an extension pointer array, dispatch on each
/// block's structure name, and keep the common fields. Unknown structures are
/// skipped the way the reference leaves them null.
pub fn get_extensions(meta: &Meta, array: &Array) -> Vec<Extension> {
    let Some(ptrs) = get_pointer_array(meta, array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ptr in ptrs {
        let Some(block) = meta.block(ptr.block_id) else {
            continue;
        };
        if !matches!(
            block.structure_name_hash,
            names::EXT_PARTICLE_EFFECT
                | names::EXT_AUDIO_COLLISION_SETTINGS
                | names::EXT_AUDIO_EMITTER
                | names::EXT_SPAWN_POINT
                | names::EXT_EXPLOSION_EFFECT
                | names::EXT_LADDER
                | names::EXT_BUOYANCY
                | names::EXT_EXPRESSION
                | names::EXT_LIGHT_SHAFT
                | names::EXT_WIND_DISTURBANCE
                | names::EXT_PROC_OBJECT
                | names::EXT_LIGHT_EFFECT
                | names::EXT_SPAWN_POINT_OVERRIDE
                | names::EXT_DOOR
                | names::EXT_VERLET_CLOTH_CUSTOM_BOUNDS
        ) {
            continue;
        }
        // Every extension carries its name hash at 8 and offset position at 16.
        let Some(name) = block
            .data
            .get(8..12)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        else {
            continue;
        };
        let Some(off) = block.data.get(16..28) else {
            continue;
        };
        let offset_position = [
            f32::from_le_bytes(off[0..4].try_into().unwrap()),
            f32::from_le_bytes(off[4..8].try_into().unwrap()),
            f32::from_le_bytes(off[8..12].try_into().unwrap()),
        ];
        out.push(Extension {
            structure: block.structure_name_hash,
            name,
            offset_position,
        });
    }
    out
}

/// Helper so `get_extensions` can run from outside this module too.
pub use get_extensions as read_extensions;

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
    fn addr(o: u64) -> u64 {
        SYSTEM_BASE + o
    }

    /// A `.ytyp` with one base archetype: the minimal meta a typed reader needs.
    fn synthetic_ytyp() -> Vec<u8> {
        let mut d = vec![0u8; 0x900];
        // Meta header: one structure, one enum-free schema, two blocks.
        put_u32(&mut d, 0x1C, 1); // root block index
        put_u64(&mut d, 0x20, addr(0x300)); // structures
        put_u64(&mut d, 0x30, addr(0x500)); // blocks
        put_u16(&mut d, 0x48, 2); // structure count
        put_u16(&mut d, 0x4C, 3); // block count

        // Structures: CMapTypes(80), CBaseArchetypeDef(144).
        put_u32(&mut d, 0x300, names::C_MAP_TYPES);
        put_u32(&mut d, 0x320, names::C_BASE_ARCHETYPE_DEF);

        // Block 1: CMapTypes, payload at 0x700.
        put_u32(&mut d, 0x500, names::C_MAP_TYPES);
        put_u32(&mut d, 0x504, 80);
        put_u64(&mut d, 0x508, addr(0x700));
        // Block 2: POINTER block holding one archetype pointer, at 0x740.
        put_u32(&mut d, 0x510, crate::meta::rsc::POINTER_BLOCK);
        put_u32(&mut d, 0x514, 8);
        put_u64(&mut d, 0x518, addr(0x740));
        // Block 3: CBaseArchetypeDef, payload at 0x760.
        put_u32(&mut d, 0x520, names::C_BASE_ARCHETYPE_DEF);
        put_u32(&mut d, 0x524, 144);
        put_u64(&mut d, 0x528, addr(0x760));

        // CMapTypes payload at 0x700: archetypes array at offset 24 -> block 2.
        // Array layout: {u64 pointer; u16 count1; u16 count2; u32 pad}.
        put_u64(&mut d, 0x700 + 24, 2);
        put_u16(&mut d, 0x700 + 24 + 8, 1);
        put_u16(&mut d, 0x700 + 24 + 10, 1);
        put_u32(&mut d, 0x700 + 40, 0xABBA); // name hash

        // POINTER block payload: one pointer to block 3, offset 0.
        put_u64(&mut d, 0x740, 3);

        // Base archetype payload at 0x760.
        put_u32(&mut d, 0x760 + 8, 60); // lodDist
        put_u32(&mut d, 0x760 + 88, 0x1111); // name
        put_u32(&mut d, 0x760 + 112, 0x2222); // assetName
        put_u32(&mut d, 0x760 + 108, 2); // assetType drawable
        put_u32(&mut d, 0x760 + 100, 0x3333); // drawableDictionary

        d
    }

    #[test]
    fn reads_a_synthetic_ytyp() {
        let data = synthetic_ytyp();
        let mut r = ResourceReader::new(&data, data.len());
        let meta = Meta::read(&mut r).expect("meta");
        let ytyp = read_ytyp(&meta).expect("ytyp");

        assert_eq!(ytyp.name_hash, 0xABBA);
        assert_eq!(ytyp.archetypes.len(), 1);

        match &ytyp.archetypes[0] {
            Archetype::Base(base) => {
                assert_eq!(base.name, 0x1111);
                assert_eq!(base.asset_name, 0x2222);
                assert_eq!(base.drawable_dictionary, 0x3333);
                assert_eq!(base.hash(), 0x2222);
                assert!(matches!(base.asset_type, AssetType::AssetDrawable));
            }
            other => panic!("expected base archetype, got {other:?}"),
        }
    }
}
