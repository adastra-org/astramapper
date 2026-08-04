//! The metadata file types that sit *above* the generic meta tree.
//!
//! Port of `YtypFile.cs` and `YmapFile.cs` from
//! `CodeWalker.Core/GameFiles/FileTypes/`. A `.ytyp` is an archetype
//! dictionary and a `.ymap` is a placement dictionary; both are RSC meta
//! containers whose blocks carry a schema, so the generic [`crate::meta::tree`]
//! walk can already render them as XML. What these readers add is the typed
//! model on top — the structures `MetaTypes.GetTypedData` and friends build,
//! which is what the world view needs to place things.
//!
//! Read-only. Writing is not ported: the `MetaBuilder` half of `MetaTypes.cs`
//! stays out, so neither format round-trips yet.

pub mod mlo;
pub mod ymap;
pub mod ytyp;

pub use mlo::{
    expand_mlo_entities, index_archetypes, index_mlo_archetypes, ArchetypeIndexes, ArchetypeInfo,
    MloWorldEntity,
};
pub use ymap::{read_ymap, DistantLodLights, Entity, GrassBatch, LodLights, YmapFile};
pub use ytyp::{
    read_ytyp, Archetype, BaseArchetype, Extension, MloArchetype, MloEntity, MloEntitySetData,
    MloPortal, MloRoom, TimeArchetype, YtypFile,
};

/// `MetaName` structure hashes the two readers dispatch on. Values come from
/// `MetaNames.cs`, verbatim.
pub mod names {
    /// `CMapTypes` — the root of a `.ytyp`.
    pub const C_MAP_TYPES: u32 = 3649811809;
    /// `CBaseArchetypeDef` — every archetype's shared first half.
    pub const C_BASE_ARCHETYPE_DEF: u32 = 2195127427;
    /// `CTimeArchetypeDef` — a base archetype plus time flags.
    pub const C_TIME_ARCHETYPE_DEF: u32 = 1991296364;
    /// `CMloArchetypeDef` — a base archetype plus the interior tail.
    pub const C_MLO_ARCHETYPE_DEF: u32 = 273704021;
    /// `CEntityDef` — one placed entity.
    pub const C_ENTITY_DEF: u32 = 3461354627;
    /// `CMloInstanceDef` — an entity plus interior instance state.
    pub const C_MLO_INSTANCE_DEF: u32 = 164374718;
    /// `CMloRoomDef`
    pub const C_MLO_ROOM_DEF: u32 = 186126833;
    /// `CMloPortalDef`
    pub const C_MLO_PORTAL_DEF: u32 = 2572186314;
    /// `CMloEntitySet`
    pub const C_MLO_ENTITY_SET: u32 = 3601308153;
    /// `CMloTimeCycleModifier`
    pub const C_MLO_TIME_CYCLE_MODIFIER: u32 = 807246248;
    /// `CCompositeEntityType` — the destruction-entity types in a `.ytyp`.
    pub const C_COMPOSITE_ENTITY_TYPE: u32 = 1185771007;
    /// `CMapData` — the root of a `.ymap`.
    pub const C_MAP_DATA: u32 = 3545841574;
    /// `CCarGen`
    pub const C_CAR_GEN: u32 = 1860713439;
    /// `CTimeCycleModifier`
    pub const C_TIME_CYCLE_MODIFIER: u32 = 1733268304;
    /// `BoxOccluder`
    pub const BOX_OCCLUDER: u32 = 975711773;
    /// `OccludeModel`
    pub const OCCLUDE_MODEL: u32 = 2741784237;
    /// `CLODLight` — the ymap's LOD light SOA.
    pub const C_LOD_LIGHT: u32 = 2276214072;
    /// `CDistantLODLight`
    pub const C_DISTANT_LOD_LIGHT: u32 = 2033467892;
    /// `CBlockDesc` — the ymap's metadata header block.
    pub const C_BLOCK_DESC: u32 = 3072355914;
    /// `FloatXYZ` — the VECTOR3 records LOD lights and portals store.
    pub const FLOAT_XYZ: u32 = 3805007828;
    /// `rage__fwInstancedMapData`
    pub const RAGE_FW_INSTANCED_MAP_DATA: u32 = 4048164286;
    /// `rage__spdAABB`
    pub const RAGE_SPD_AABB: u32 = 4084721864;
    /// `rage__fwGrassInstanceListDef`
    pub const RAGE_FW_GRASS_INSTANCE_LIST_DEF: u32 = 2085051229;
    /// `rage__fwGrassInstanceListDef__InstanceData`
    pub const RAGE_FW_GRASS_INSTANCE_LIST_DEF_INSTANCE_DATA: u32 = 3985044770;

    /// `CExtensionDefParticleEffect`
    pub const EXT_PARTICLE_EFFECT: u32 = 975627745;
    /// `CExtensionDefAudioCollisionSettings`
    pub const EXT_AUDIO_COLLISION_SETTINGS: u32 = 366926375;
    /// `CExtensionDefAudioEmitter`
    pub const EXT_AUDIO_EMITTER: u32 = 637823035;
    /// `CExtensionDefSpawnPoint`
    pub const EXT_SPAWN_POINT: u32 = 3300062776;
    /// `CExtensionDefExplosionEffect`
    pub const EXT_EXPLOSION_EFFECT: u32 = 104349545;
    /// `CExtensionDefLadder`
    pub const EXT_LADDER: u32 = 2182960161;
    /// `CExtensionDefBuoyancy`
    pub const EXT_BUOYANCY: u32 = 749982947;
    /// `CExtensionDefExpression`
    pub const EXT_EXPRESSION: u32 = 3870521079;
    /// `CExtensionDefLightShaft`
    pub const EXT_LIGHT_SHAFT: u32 = 2718997053;
    /// `CExtensionDefWindDisturbance`
    pub const EXT_WIND_DISTURBANCE: u32 = 569228403;
    /// `CExtensionDefProcObject`
    pub const EXT_PROC_OBJECT: u32 = 2565191912;
    /// `CExtensionDefLightEffect`
    pub const EXT_LIGHT_EFFECT: u32 = 663891011;
    /// `CExtensionDefSpawnPointOverride`
    pub const EXT_SPAWN_POINT_OVERRIDE: u32 = 2716862120;
    /// `CExtensionDefDoor`
    pub const EXT_DOOR: u32 = 1965932561;
    /// `rage__phVerletClothCustomBounds`
    pub const EXT_VERLET_CLOTH_CUSTOM_BOUNDS: u32 = 847348117;
}
