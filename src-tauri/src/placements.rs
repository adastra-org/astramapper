//! A `.ymap`'s placements, encoded column by column into the wire envelope.
//!
//! # Why this exists
//!
//! `world_map` was a plain `#[tauri::command]`, so every placed entity crossed
//! the IPC boundary as JSON text. Measured over the base game — 4,601 maps,
//! 763,716 placements — that is **201.5 MB of JSON and 2.3s to serialize**,
//! before the WebView parses a byte of it or allocates the objects it decodes
//! into. Reading the same maps costs 2.0s, so the wire was more expensive than
//! the work.
//!
//! Most of those bytes are not data. A placement is thirteen fields, and its
//! JSON repeats every field *name* per entity: about 120 bytes of keys against
//! roughly 60 bytes of actual values. Columns delete the keys outright.
//!
//! This is the same move 4.3 made for textures and 4.4 for geometry, on the one
//! payload that was left behind — the difference being that a placement is
//! thirteen small fields rather than one big buffer, so the saving comes from
//! the schema rather than from avoiding a re-encode.
//!
//! # The layout
//!
//! One section per field, in the order below, all little-endian and all `count`
//! elements long. Section order is the contract with `src/lib/world.ts`; adding
//! a column means appending, never inserting.
//!
//! | # | Field | Type |
//! | --- | --- | --- |
//! | 0 | `index` | `u32` |
//! | 1 | `nameIndex` — into the metadata's name table | `u32` |
//! | 2 | `flags` — bit 0 `isMlo`, bit 1 `fromMlo` | `u8` |
//! | 3 | `position` | `f32` × 3 |
//! | 4 | `rotation` | `f32` × 4 |
//! | 5 | `scaleXy` | `f32` |
//! | 6 | `scaleZ` | `f32` |
//! | 7 | `parentIndex` | `i32` |
//! | 8 | `lodLevel` — into the metadata's level table | `u16` |
//! | 9 | `lodDist` | `f32` |
//! | 10 | `children` | `u32` |
//!
//! # The archetype class
//!
//! Which load phase an archetype belongs to — decal, vegetation, or "the data
//! says nothing" — is decided from the `.ytyp` archetype record (flags, asset
//! type, bounding box; see `cw_core::filetypes::class`). It rides in the
//! metadata as `nameClasses`, one `u8` per row of the name table, rather than as
//! a twelfth column: the class is a function of the archetype, so a street of
//! one prop pays for it once, and the section order stays untouched. A map read
//! without the archetype index (tests, `encode`) writes class 0 everywhere, and
//! the frontend falls back to the name tokens.
//!
//! # Two string tables, not two string columns
//!
//! `name` and `lodLevel` are the only strings, and both repeat heavily — a
//! street of one prop is one name several hundred times, and there are seven
//! LOD levels in the whole game. Both become a deduplicated table in the JSON
//! metadata with an integer column pointing into it.
//!
//! **The name table is keyed on the pair, not on the string.** `nameHash` is
//! almost derivable from `name`, and deriving it would drop four bytes per
//! entity — but "almost" is doing real work in that sentence, and an entity
//! silently acquiring a neighbour's archetype hash is not a failure anyone
//! would catch by looking. Two names that hash differently get two rows.

use cw_core::filetypes::{classify_archetype, ArchetypeInfo};
use cw_core::wire::{Envelope, EnvelopeView, Kind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::world::{WorldEntity, WorldMap};

/// The metadata written into the envelope, mirrored by `worldMap` in
/// `src/lib/world.ts`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Meta<'a> {
    path: &'a str,
    name: &'a str,
    name_hash: u32,
    parent: &'a str,
    entity_count: usize,
    mlo_entities: usize,
    mlo_expanded: usize,
    extents_min: [f32; 3],
    extents_max: [f32; 3],
    /// How many placements each column holds.
    count: usize,
    /// Distinct archetype names, indexed by the `nameIndex` column.
    names: Vec<&'a str>,
    /// Each name's archetype hash, parallel to `names`.
    name_hashes: Vec<u32>,
    /// Each name's archetype class (`ArchetypeClass` as a `u8`, 0 = none),
    /// parallel to `names`.
    name_classes: Vec<u8>,
    /// Distinct LOD level labels, indexed by the `lodLevel` column.
    lod_levels: Vec<&'a str>,
}

/// Encode one read map into envelope bytes.
///
/// Fails only if a map somehow carries more than 65,535 distinct LOD labels.
/// `LodLevel::Unknown` passes an arbitrary `i32` through as its own label, so
/// the bound is reachable in principle by a corrupt file and is checked rather
/// than assumed — a wrapped index would put every entity in the wrong band with
/// nothing to show for it.
pub fn encode(map: &WorldMap) -> Result<Vec<u8>, String> {
    encode_with(map, None)
}

/// [`encode`] with the archetype index the classes are read from. `None`, or a
/// hash the index does not know, writes class 0 and leaves the name to decide.
pub fn encode_with(
    map: &WorldMap,
    archetypes: Option<&HashMap<u32, ArchetypeInfo>>,
) -> Result<Vec<u8>, String> {
    let count = map.entities.len();

    let mut names: Vec<&str> = Vec::new();
    let mut name_hashes: Vec<u32> = Vec::new();
    let mut name_classes: Vec<u8> = Vec::new();
    let mut name_index: HashMap<(&str, u32), u32> = HashMap::new();
    let mut lod_levels: Vec<&str> = Vec::new();
    let mut lod_index: HashMap<&str, u16> = HashMap::new();

    let mut col_index = Vec::with_capacity(count * 4);
    let mut col_name = Vec::with_capacity(count * 4);
    let mut col_flags = Vec::with_capacity(count);
    let mut col_position = Vec::with_capacity(count * 12);
    let mut col_rotation = Vec::with_capacity(count * 16);
    let mut col_scale_xy = Vec::with_capacity(count * 4);
    let mut col_scale_z = Vec::with_capacity(count * 4);
    let mut col_parent = Vec::with_capacity(count * 4);
    let mut col_lod_level = Vec::with_capacity(count * 2);
    let mut col_lod_dist = Vec::with_capacity(count * 4);
    let mut col_children = Vec::with_capacity(count * 4);

    for entity in &map.entities {
        let WorldEntity {
            index,
            name,
            name_hash,
            is_mlo,
            from_mlo,
            position,
            rotation,
            scale_xy,
            scale_z,
            parent_index,
            lod_level,
            lod_dist,
            children,
        } = entity;

        let key = (name.as_str(), *name_hash);
        let slot = match name_index.get(&key) {
            Some(slot) => *slot,
            None => {
                let slot = names.len() as u32;
                names.push(name.as_str());
                name_hashes.push(*name_hash);
                name_classes.push(
                    archetypes
                        .and_then(|index| index.get(name_hash))
                        .map_or(0, |info| classify_archetype(info) as u8),
                );
                name_index.insert(key, slot);
                slot
            }
        };

        let level = match lod_index.get(lod_level.as_str()) {
            Some(level) => *level,
            None => {
                let level = u16::try_from(lod_levels.len()).map_err(|_| {
                    format!("{} carries more than 65,535 distinct LOD levels", map.path)
                })?;
                lod_levels.push(lod_level.as_str());
                lod_index.insert(lod_level.as_str(), level);
                level
            }
        };

        col_index.extend_from_slice(&(*index as u32).to_le_bytes());
        col_name.extend_from_slice(&slot.to_le_bytes());
        col_flags.push(u8::from(*is_mlo) | (u8::from(*from_mlo) << 1));
        for value in position {
            col_position.extend_from_slice(&value.to_le_bytes());
        }
        for value in rotation {
            col_rotation.extend_from_slice(&value.to_le_bytes());
        }
        col_scale_xy.extend_from_slice(&scale_xy.to_le_bytes());
        col_scale_z.extend_from_slice(&scale_z.to_le_bytes());
        col_parent.extend_from_slice(&parent_index.to_le_bytes());
        col_lod_level.extend_from_slice(&level.to_le_bytes());
        col_lod_dist.extend_from_slice(&lod_dist.to_le_bytes());
        col_children.extend_from_slice(&(*children as u32).to_le_bytes());
    }

    let meta = Meta {
        path: &map.path,
        name: &map.name,
        name_hash: map.name_hash,
        parent: &map.parent,
        entity_count: map.entity_count,
        mlo_entities: map.mlo_entities,
        mlo_expanded: map.mlo_expanded,
        extents_min: map.extents_min,
        extents_max: map.extents_max,
        count,
        names,
        name_hashes,
        name_classes,
        lod_levels,
    };
    let meta = serde_json::to_string(&meta)
        .map_err(|e| format!("could not write the placement metadata: {e}"))?;

    let mut envelope = Envelope::new();
    // Push order is the contract. See the table in the module note.
    envelope.push(col_index);
    envelope.push(col_name);
    envelope.push(col_flags);
    envelope.push(col_position);
    envelope.push(col_rotation);
    envelope.push(col_scale_xy);
    envelope.push(col_scale_z);
    envelope.push(col_parent);
    envelope.push(col_lod_level);
    envelope.push(col_lod_dist);
    envelope.push(col_children);

    Ok(envelope.finish(Kind::Placements, &meta))
}

/// The metadata [`encode`] writes, read back.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetaIn {
    path: String,
    name: String,
    entity_count: usize,
    extents_min: [f32; 3],
    extents_max: [f32; 3],
    count: usize,
    names: Vec<String>,
    name_hashes: Vec<u32>,
    /// Absent in an envelope written before archetype classes: all none.
    #[serde(default)]
    name_classes: Vec<u8>,
    lod_levels: Vec<String>,
}

/// A map's placements read back out of an envelope, column by column.
///
/// The backend's cell index is built from the same envelopes the cache holds,
/// so a map is flattened once, by [`encode`], and every consumer reads what that
/// wrote. Nothing here is an object per placement.
#[derive(Debug, Default)]
pub struct DecodedMap {
    pub path: String,
    pub name: String,
    pub entity_count: usize,
    pub extents_min: [f32; 3],
    pub extents_max: [f32; 3],
    /// Distinct archetype names, indexed by `name_slot`.
    pub names: Vec<String>,
    pub name_hashes: Vec<u32>,
    /// Per name slot, an archetype class as in `cw-core`'s `ArchetypeClass`.
    pub name_classes: Vec<u8>,
    pub lod_levels: Vec<String>,
    pub index: Vec<u32>,
    pub name_slot: Vec<u32>,
    pub flags: Vec<u8>,
    pub position: Vec<[f32; 3]>,
    pub rotation: Vec<[f32; 4]>,
    pub scale_xy: Vec<f32>,
    pub scale_z: Vec<f32>,
    pub parent: Vec<i32>,
    pub lod_slot: Vec<u16>,
    pub lod_dist: Vec<f32>,
    pub children: Vec<u32>,
}

impl DecodedMap {
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

fn floats<const N: usize>(bytes: &[u8]) -> Vec<[f32; N]> {
    bytes
        .chunks_exact(4 * N)
        .map(|row| {
            let mut out = [0.0f32; N];
            for (value, c) in out.iter_mut().zip(row.chunks_exact(4)) {
                *value = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            }
            out
        })
        .collect()
}

fn le_u32(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn le_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Read an envelope [`encode`] wrote. Refuses one of another kind, and one whose
/// columns do not all hold `count` elements, because a short column would put
/// the wrong value on every placement after it.
pub fn decode(bytes: &[u8]) -> Result<DecodedMap, String> {
    let view = EnvelopeView::read(bytes).map_err(|e| format!("not a placement envelope: {e}"))?;
    if view.kind != Kind::Placements {
        return Err(format!("expected placements, found {:?}", view.kind));
    }
    let meta: MetaIn = serde_json::from_str(view.meta)
        .map_err(|e| format!("could not read the placement metadata: {e}"))?;
    if view.section_count() != SECTIONS {
        return Err(format!(
            "expected {SECTIONS} columns, found {}",
            view.section_count()
        ));
    }
    let section = |i: usize| view.section(i).unwrap_or(&[]);

    let decoded = DecodedMap {
        index: le_u32(section(0)),
        name_slot: le_u32(section(1)),
        flags: section(2).to_vec(),
        position: floats::<3>(section(3)),
        rotation: floats::<4>(section(4)),
        scale_xy: le_f32(section(5)),
        scale_z: le_f32(section(6)),
        parent: section(7)
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        lod_slot: section(8)
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect(),
        lod_dist: le_f32(section(9)),
        children: le_u32(section(10)),
        path: meta.path,
        name: meta.name,
        entity_count: meta.entity_count,
        extents_min: meta.extents_min,
        extents_max: meta.extents_max,
        names: meta.names,
        name_hashes: meta.name_hashes,
        name_classes: meta.name_classes,
        lod_levels: meta.lod_levels,
    };

    let count = meta.count;
    let lengths = [
        decoded.index.len(),
        decoded.name_slot.len(),
        decoded.flags.len(),
        decoded.position.len(),
        decoded.rotation.len(),
        decoded.scale_xy.len(),
        decoded.scale_z.len(),
        decoded.parent.len(),
        decoded.lod_slot.len(),
        decoded.lod_dist.len(),
        decoded.children.len(),
    ];
    if let Some(bad) = lengths.iter().position(|&len| len != count) {
        return Err(format!(
            "column {bad} holds {} elements, the metadata says {count}",
            lengths[bad]
        ));
    }
    Ok(decoded)
}

/// How many columns a placements envelope has.
const SECTIONS: usize = 11;

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::filetypes::class::{FLAG_DONT_CAST_SHADOWS, FLAG_DOUBLE_SIDED};
    use cw_core::meta::typed::AssetType;
    use cw_core::wire::EnvelopeView;

    fn archetype(flags: u32, extent: [f32; 3]) -> ArchetypeInfo {
        ArchetypeInfo {
            name: 0,
            asset_name: 0,
            asset_type: AssetType::AssetDrawable,
            drawable_dictionary: 0,
            texture_dictionary: 0,
            physics_dictionary: 0,
            bb_min: [0.0; 3],
            bb_max: extent,
            lod_dist: 100.0,
            flags,
            is_mlo: false,
        }
    }

    fn entity(index: usize, name: &str, hash: u32, lod: &str) -> WorldEntity {
        WorldEntity {
            index,
            name: name.to_string(),
            name_hash: hash,
            is_mlo: false,
            from_mlo: false,
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale_xy: 1.0,
            scale_z: 1.0,
            parent_index: -1,
            lod_level: lod.to_string(),
            lod_dist: 100.0,
            children: 0,
        }
    }

    fn map(entities: Vec<WorldEntity>) -> WorldMap {
        WorldMap {
            path: "x64a.rpf\\a.ymap".to_string(),
            name: "a".to_string(),
            name_hash: 7,
            parent: String::new(),
            entity_count: entities.len(),
            mlo_entities: 0,
            extents_min: [-1.0, -2.0, -3.0],
            extents_max: [1.0, 2.0, 3.0],
            entities,
            mlo_expanded: 0,
        }
    }

    fn meta_of(bytes: &[u8]) -> serde_json::Value {
        let view = EnvelopeView::read(bytes).expect("read");
        assert_eq!(view.kind, Kind::Placements);
        serde_json::from_str(view.meta).expect("meta json")
    }

    #[test]
    fn every_column_holds_one_element_per_placement() {
        let bytes = encode(&map(vec![
            entity(0, "prop_a", 1, "HD"),
            entity(1, "prop_b", 2, "LOD"),
            entity(2, "prop_a", 1, "HD"),
        ]))
        .expect("encode");
        let view = EnvelopeView::read(&bytes).expect("read");

        assert_eq!(view.section_count(), 11);
        let widths = [4, 4, 1, 12, 16, 4, 4, 4, 2, 4, 4];
        for (index, width) in widths.iter().enumerate() {
            let section = view.section(index).expect("section");
            assert_eq!(section.len(), 3 * width, "section {index}");
        }
    }

    #[test]
    fn a_repeated_name_is_stored_once_and_pointed_at_twice() {
        let bytes = encode(&map(vec![
            entity(0, "prop_a", 1, "HD"),
            entity(1, "prop_b", 2, "HD"),
            entity(2, "prop_a", 1, "HD"),
        ]))
        .expect("encode");
        let meta = meta_of(&bytes);

        assert_eq!(meta["names"], serde_json::json!(["prop_a", "prop_b"]));
        assert_eq!(meta["nameHashes"], serde_json::json!([1, 2]));
        assert_eq!(meta["lodLevels"], serde_json::json!(["HD"]));

        let view = EnvelopeView::read(&bytes).expect("read");
        let column = view.section(1).expect("name column");
        let slots: Vec<u32> = column
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(slots, vec![0, 1, 0]);
    }

    /// The point of keying the table on the pair. One name reached through two
    /// different archetype hashes must not collapse into one row, because the
    /// row is what carries the hash back.
    #[test]
    fn one_name_under_two_hashes_keeps_both() {
        let bytes = encode(&map(vec![
            entity(0, "prop_a", 1, "HD"),
            entity(1, "prop_a", 999, "HD"),
        ]))
        .expect("encode");
        let meta = meta_of(&bytes);

        assert_eq!(meta["names"], serde_json::json!(["prop_a", "prop_a"]));
        assert_eq!(meta["nameHashes"], serde_json::json!([1, 999]));
    }

    #[test]
    fn the_two_mlo_bits_are_independent() {
        let mut both = entity(0, "a", 1, "HD");
        both.is_mlo = true;
        both.from_mlo = true;
        let mut instance = entity(1, "b", 2, "HD");
        instance.is_mlo = true;
        let mut interior = entity(2, "c", 3, "HD");
        interior.from_mlo = true;
        let plain = entity(3, "d", 4, "HD");

        let bytes = encode(&map(vec![both, instance, interior, plain])).expect("encode");
        let view = EnvelopeView::read(&bytes).expect("read");
        assert_eq!(view.section(2), Some(&[0b11u8, 0b01, 0b10, 0b00][..]));
    }

    #[test]
    fn the_floats_survive_exactly() {
        let mut one = entity(0, "a", 1, "HD");
        one.position = [-0.0, 1e30, f32::MIN_POSITIVE];
        one.rotation = [0.5, -0.5, 0.5, -0.5];
        let bytes = encode(&map(vec![one])).expect("encode");
        let view = EnvelopeView::read(&bytes).expect("read");

        let read = |section: &[u8]| -> Vec<f32> {
            section
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        };
        let position = read(view.section(3).expect("position"));
        assert_eq!(position, vec![-0.0, 1e30, f32::MIN_POSITIVE]);
        assert!(position[0].is_sign_negative(), "-0.0 kept its sign");
        assert_eq!(
            read(view.section(4).expect("rotation")),
            vec![0.5, -0.5, 0.5, -0.5]
        );
    }

    /// A map with no entities still has to produce a readable envelope: the
    /// world places it, and an empty one must not look like a decode failure.
    #[test]
    fn an_empty_map_still_encodes() {
        let bytes = encode(&map(vec![])).expect("encode");
        let view = EnvelopeView::read(&bytes).expect("read");
        assert_eq!(view.section_count(), 11);
        for index in 0..11 {
            assert_eq!(view.section(index).map(<[u8]>::len), Some(0));
        }
        assert_eq!(meta_of(&bytes)["count"], 0);
    }

    #[test]
    fn a_negative_parent_index_stays_negative() {
        let mut child = entity(1, "b", 2, "HD");
        child.parent_index = 4;
        let bytes = encode(&map(vec![entity(0, "a", 1, "HD"), child])).expect("encode");
        let view = EnvelopeView::read(&bytes).expect("read");
        let parents: Vec<i32> = view
            .section(7)
            .expect("parent column")
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(parents, vec![-1, 4]);
    }

    #[test]
    fn decode_reads_back_what_encode_wrote() {
        let mut second = entity(1, "prop_b", 2, "LOD");
        second.position = [-4.5, 9.25, 100.0];
        second.rotation = [0.5, -0.5, 0.5, -0.5];
        second.scale_xy = 2.0;
        second.scale_z = 3.0;
        second.parent_index = 4;
        second.from_mlo = true;
        second.lod_dist = 250.0;
        second.children = 7;
        let bytes = encode(&map(vec![entity(0, "prop_a", 1, "HD"), second])).expect("encode");
        let back = decode(&bytes).expect("decode");

        assert_eq!(back.path, r"x64a.rpf\a.ymap");
        assert_eq!(back.len(), 2);
        assert_eq!(back.names, vec!["prop_a", "prop_b"]);
        assert_eq!(back.name_hashes, vec![1, 2]);
        assert_eq!(back.lod_levels, vec!["HD", "LOD"]);
        assert_eq!(back.index, vec![0, 1]);
        assert_eq!(back.name_slot, vec![0, 1]);
        assert_eq!(back.flags, vec![0, 0b10]);
        assert_eq!(back.position[1], [-4.5, 9.25, 100.0]);
        assert_eq!(back.rotation[1], [0.5, -0.5, 0.5, -0.5]);
        assert_eq!(back.scale_xy, vec![1.0, 2.0]);
        assert_eq!(back.scale_z, vec![1.0, 3.0]);
        assert_eq!(back.parent, vec![-1, 4]);
        assert_eq!(back.lod_slot, vec![0, 1]);
        assert_eq!(back.lod_dist, vec![100.0, 250.0]);
        assert_eq!(back.children, vec![0, 7]);
        assert_eq!(back.extents_min, [-1.0, -2.0, -3.0]);
    }

    #[test]
    fn decode_reads_an_empty_map_and_refuses_the_wrong_kind() {
        let back = decode(&encode(&map(vec![])).expect("encode")).expect("decode");
        assert!(back.is_empty());

        let other = Envelope::new().finish(Kind::Model, "{}");
        assert!(decode(&other).is_err());
        assert!(decode(b"not an envelope").is_err());
    }

    #[test]
    fn each_name_row_carries_the_class_the_archetype_index_decides() {
        let index: HashMap<u32, ArchetypeInfo> = HashMap::from([
            (1, archetype(FLAG_DOUBLE_SIDED, [2.0; 3])),
            (3, archetype(FLAG_DONT_CAST_SHADOWS, [6.0, 9.0, 0.01])),
            (4, archetype(0, [6.0, 9.0, 4.0])),
        ]);
        let bytes = encode_with(
            &map(vec![
                entity(0, "prop_bush", 1, "HD"),
                entity(1, "prop_unindexed", 2, "HD"),
                entity(2, "ap1_puddle", 3, "HD"),
                entity(3, "prop_bush", 1, "HD"),
                entity(4, "prop_wall", 4, "HD"),
            ]),
            Some(&index),
        )
        .expect("encode");
        let meta = meta_of(&bytes);

        // Parallel to `names`: vegetation, none (not indexed), decal, none.
        assert_eq!(
            meta["names"],
            serde_json::json!(["prop_bush", "prop_unindexed", "ap1_puddle", "prop_wall"])
        );
        assert_eq!(meta["nameClasses"], serde_json::json!([2, 0, 1, 0]));
    }

    #[test]
    fn without_an_index_every_class_is_none_and_the_name_decides() {
        let bytes = encode(&map(vec![
            entity(0, "prop_a", 1, "HD"),
            entity(1, "prop_b", 2, "HD"),
        ]))
        .expect("encode");
        assert_eq!(meta_of(&bytes)["nameClasses"], serde_json::json!([0, 0]));
    }

    /// The class lives in the metadata, not in a column: the section order is the
    /// contract with `src/lib/world/placements.ts` and must not move.
    #[test]
    fn the_class_does_not_add_a_section() {
        let index: HashMap<u32, ArchetypeInfo> =
            HashMap::from([(1, archetype(FLAG_DOUBLE_SIDED, [2.0; 3]))]);
        let bytes = encode_with(&map(vec![entity(0, "a", 1, "HD")]), Some(&index)).expect("encode");
        assert_eq!(
            EnvelopeView::read(&bytes).expect("read").section_count(),
            11
        );
    }
}
