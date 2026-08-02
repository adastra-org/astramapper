//! Clip dictionaries: what a `.ycd` holds.
//!
//! Port of `ClipDictionary` / `AnimationMap` / `AnimationMapEntry` /
//! `Animation` / `ClipMapEntry` / `ClipBase` / `ClipAnimation` /
//! `ClipAnimationList` / `ClipAnimationsEntry` / `Sequence` from
//! `CodeWalker.Core/GameFiles/Resources/Clip.cs`, driven by `YcdFile.Load`.
//!
//! Scope: the two maps and everything needed to say what a clip *is* — its
//! name, type, timing, and which animations it plays. Deliberately not ported:
//! the animation channels themselves (`AnimChannel*` and
//! `AnimChannelDataReader`, about 900 lines of bit-packed float codecs), the
//! clip tags and the clip property map. Each is read past rather than into, so
//! the counts are reported and the blocks stay `TODO(port)`. Playing an
//! animation also needs a skeleton, which lives in `Drawable.cs` and is not
//! ported either.
//!
//! Three things about the layout that are not guessable from the field list:
//!
//! 1. **Both maps are hash tables, not arrays.** `ClipsMapCapacity` is the
//!    bucket count, and each bucket is a chain through `Next`. Reading only the
//!    bucket heads finds most clips in a small file and misses most of them in
//!    a large one.
//! 2. **`ClipsMapEntries` is the entry count and is not what you index by.**
//!    The pointer array is `Capacity` long; `Entries` is how many clips the
//!    chains hold in total.
//! 3. **A clip's type is a byte read 16 bytes into a block that has not been
//!    read yet.** `ClipBase.GetType` peeks at offset 0x10 to choose which of the
//!    two 112-byte layouts to parse, and both share the first 0x50 bytes.

use std::collections::HashMap;

use serde::Serialize;

use crate::utils::hash_string;
use crate::Result;

use super::drawable::ResourceFileBase;
use super::reader::ResourceReader;

/// Guard on a map capacity or list count read out of a file. A stock `.ycd`
/// runs to a few thousand clips; this only has to stop a corrupt header from
/// naming a four-billion-element allocation.
const MAX_ELEMENTS: usize = 1_000_000;

/// Guard on a bucket chain. The chain is a linked list in file order and
/// nothing stops a malformed file pointing one at itself.
const MAX_CHAIN: usize = 65_536;

/// What a clip plays. `ClipType` in the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipType {
    /// One animation, with its own start, end and rate.
    Animation,
    /// A list of animations, each with its own timing, under one duration.
    AnimationList,
    /// Neither 1 nor 2. The reference's `ConstructClip` returns null here and
    /// the clip is dropped; this keeps it, so an unknown type is visible.
    Unknown(u32),
}

impl ClipType {
    fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Animation,
            2 => Self::AnimationList,
            other => Self::Unknown(other),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Animation => "animation",
            Self::AnimationList => "animation list",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// A `.ycd`: a map of clips and a map of the animations they play.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipDictionary {
    pub base: ResourceFileBase,
    /// Clips, in bucket order then chain order.
    pub clips: Vec<Clip>,
    /// Animations, likewise. A clip references these by hash.
    pub animations: Vec<Animation>,
    /// What the header claims each map holds, kept because a disagreement with
    /// the lists above is the clearest sign a payload is not a clip dictionary.
    pub declared_clips: u16,
    pub declared_animations: u16,
}

impl ClipDictionary {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let base = ResourceFileBase::read(r)?;

        let _unknown_10h = r.read_u32()?;
        let _unknown_14h = r.read_u32()?;
        let animations_pointer = r.read_u64()?;
        let _unknown_20h = r.read_u32()?;
        let _unknown_24h = r.read_u32()?;
        let clips_pointer = r.read_u64()?;
        let clips_map_capacity = r.read_u16()?;
        let clips_map_entries = r.read_u16()?;
        let _unknown_34h = r.read_u32()?;
        let _unknown_38h = r.read_u32()?;
        let _unknown_3ch = r.read_u32()?;

        // The animation map is read first because it is what names the
        // animations: the hash lives in the map entry, not in the animation
        // block, and a clip points straight at the block. The reference gets
        // this for free — `ReadBlockAt` caches by position, so the clip and the
        // map entry share one `Animation` instance and see the hash the entry
        // wrote into it. Here the block's address is the identity instead.
        let (animations, by_address, declared_animations) =
            read_animation_map(r, animations_pointer)?;
        let clips = read_clip_map(r, clips_pointer, clips_map_capacity as usize, &by_address)?;

        Ok(Self {
            base,
            clips,
            animations,
            declared_clips: clips_map_entries,
            declared_animations,
        })
    }

    /// Whether this looks like a clip dictionary at all.
    ///
    /// The same question `TextureDictionary` answers structurally: a junk
    /// payload parses, because the reader zero-fills past the end and a null
    /// pointer reads as an absent block. A dictionary with nothing in either
    /// map is not worth opening a viewer for.
    pub fn has_content(&self) -> bool {
        !self.clips.is_empty() || !self.animations.is_empty()
    }
}

/// Walk a bucket array, following each chain.
fn read_clip_map(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    capacity: usize,
    animations_by_address: &HashMap<u64, u32>,
) -> Result<Vec<Clip>> {
    if capacity > MAX_ELEMENTS {
        return Ok(Vec::new());
    }
    let buckets = r.read_u64s_at(pointer, capacity)?.unwrap_or_default();

    let mut clips = Vec::new();
    for bucket in buckets {
        let mut next = bucket;
        let mut steps = 0;
        while next != 0 && steps < MAX_CHAIN {
            let Some((clip, following)) =
                r.at(next, |r| read_clip_map_entry(r, animations_by_address))?
            else {
                break;
            };
            if let Some(clip) = clip {
                clips.push(clip);
            }
            next = following;
            steps += 1;
        }
    }
    Ok(clips)
}

/// The `AnimationMap` block, which is a second bucket array behind its own
/// 48-byte header.
///
/// Returns the animations, an address-to-hash table for the clips to resolve
/// their animation pointers through, and the entry count the header declares.
fn read_animation_map(
    r: &mut ResourceReader<'_>,
    pointer: u64,
) -> Result<(Vec<Animation>, HashMap<u64, u32>, u16)> {
    let Some((array_pointer, capacity, declared)) = r.at(pointer, |r| {
        let _vft = r.read_u32()?;
        for _ in 0..5 {
            let _unknown = r.read_u32()?;
        }
        let array_pointer = r.read_u64()?;
        let capacity = r.read_u16()? as usize;
        let declared = r.read_u16()?;
        Ok((array_pointer, capacity, declared))
    })?
    else {
        return Ok((Vec::new(), HashMap::new(), 0));
    };

    let mut by_address = HashMap::new();
    if capacity > MAX_ELEMENTS {
        return Ok((Vec::new(), by_address, declared));
    }
    let buckets = r.read_u64s_at(array_pointer, capacity)?.unwrap_or_default();

    let mut animations = Vec::new();
    for bucket in buckets {
        let mut next = bucket;
        let mut steps = 0;
        while next != 0 && steps < MAX_CHAIN {
            let Some((entry, following)) = r.at(next, read_animation_map_entry)? else {
                break;
            };
            if let Some((address, animation)) = entry {
                by_address.insert(address, animation.hash);
                animations.push(animation);
            }
            next = following;
            steps += 1;
        }
    }
    Ok((animations, by_address, declared))
}

/// A clip: one entry in the clip map.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    /// The map key. This is the name the game looks the clip up by, and it is
    /// not always `gen_hash(name)`.
    pub hash: u32,
    pub clip_type: ClipType,
    /// The path the clip was built from, e.g. `x:/anim/.../walk.clip`.
    pub name: Option<String>,
    /// `walk` out of the path above: after the last separator, before the
    /// first dot, lowercased. `ClipBase.ShortName`.
    pub short_name: Option<String>,
    /// Seconds. `EndTime - StartTime` for a single animation, the list's own
    /// duration for a list.
    pub duration: f32,
    /// Playback rate, for a single animation. A list has one per entry.
    pub rate: f32,
    pub start_time: f32,
    pub end_time: f32,
    /// The animations this clip plays, by hash. One for `Animation`, however
    /// many the list holds for `AnimationList`.
    pub animations: Vec<u32>,
    /// Tags and properties are counted, not read — see the module note.
    pub tag_count: usize,
    pub property_count: usize,
}

impl Clip {
    /// The name to show: the short name when there is one, else the hash.
    pub fn display_name(&self) -> String {
        match &self.short_name {
            Some(name) if !name.is_empty() => name.clone(),
            _ => hash_string(self.hash),
        }
    }
}

/// `ClipMapEntry`: 32 bytes, a hash, a clip pointer and the next entry in the
/// bucket. Returns the clip and the next entry's address.
fn read_clip_map_entry(
    r: &mut ResourceReader<'_>,
    animations_by_address: &HashMap<u64, u32>,
) -> Result<(Option<Clip>, u64)> {
    let hash = r.read_u32()?;
    let _unknown_04h = r.read_u32()?;
    let clip_pointer = r.read_u64()?;
    let next_pointer = r.read_u64()?;

    let clip = r.at(clip_pointer, |r| read_clip(r, hash, animations_by_address))?;
    Ok((clip, next_pointer))
}

/// The 112-byte clip block, in either of its two shapes.
///
/// `ClipBase.GetType` decides which by peeking at the type byte and both
/// layouts share their first 0x50 bytes, so this reads the common half and then
/// switches — the same discrimination, without the block having to be read
/// twice.
fn read_clip(
    r: &mut ResourceReader<'_>,
    hash: u32,
    animations_by_address: &HashMap<u64, u32>,
) -> Result<Clip> {
    let _vft = r.read_u32()?;
    let _unknown_04h = r.read_u32()?;
    let _unknown_08h = r.read_u32()?;
    let _unknown_0ch = r.read_u32()?;
    let clip_type = ClipType::from_raw(r.read_u32()?);
    let _unknown_14h = r.read_u32()?;
    let name_pointer = r.read_u64()?;
    let _name_length = r.read_u16()?;
    let _name_capacity = r.read_u16()?;
    let _unknown_24h = r.read_u32()?;
    let _unknown_28h_pointer = r.read_u64()?;
    let _unknown_30h = r.read_u32()?;
    let _unknown_34h = r.read_u32()?;
    let tags_pointer = r.read_u64()?;
    let properties_pointer = r.read_u64()?;
    let _unknown_48h = r.read_u32()?;
    let _unknown_4ch = r.read_u32()?;

    let mut clip = Clip {
        hash,
        clip_type,
        name: None,
        short_name: None,
        duration: 0.0,
        rate: 0.0,
        start_time: 0.0,
        end_time: 0.0,
        animations: Vec::new(),
        tag_count: tag_count(r, tags_pointer)?,
        property_count: property_count(r, properties_pointer)?,
    };

    match clip_type {
        ClipType::Animation => {
            let animation_pointer = r.read_u64()?;
            clip.start_time = r.read_f32()?;
            clip.end_time = r.read_f32()?;
            clip.rate = r.read_f32()?;
            clip.duration = clip.end_time - clip.start_time;
            if animation_pointer != 0 {
                clip.animations.push(
                    animations_by_address
                        .get(&animation_pointer)
                        .copied()
                        .unwrap_or(0),
                );
            }
        }
        ClipType::AnimationList => {
            let animations_pointer = r.read_u64()?;
            let count = r.read_u16()? as usize;
            let _count2 = r.read_u16()?;
            let _unknown_5ch = r.read_u32()?;
            clip.duration = r.read_f32()?;
            clip.end_time = clip.duration;

            // `ResourceSimpleArray<ClipAnimationsEntry>`: 24 bytes each, laid
            // out end to end rather than behind a pointer each.
            if count <= MAX_ELEMENTS {
                for i in 0..count {
                    let at = animations_pointer + (i * 24) as u64;
                    let entry = r.at(at, |r| {
                        let _start = r.read_f32()?;
                        let _end = r.read_f32()?;
                        let rate = r.read_f32()?;
                        let _unknown_0ch = r.read_u32()?;
                        let animation_pointer = r.read_u64()?;
                        Ok((rate, animation_pointer))
                    })?;
                    let Some((rate, animation_pointer)) = entry else {
                        continue;
                    };
                    if i == 0 {
                        clip.rate = rate;
                    }
                    if animation_pointer != 0 {
                        clip.animations.push(
                            animations_by_address
                                .get(&animation_pointer)
                                .copied()
                                .unwrap_or(0),
                        );
                    }
                }
            }
        }
        ClipType::Unknown(_) => {}
    }

    clip.name = r.read_string_at(name_pointer)?;
    clip.short_name = clip.name.as_deref().map(short_name);
    Ok(clip)
}

/// `ClipBase.ShortName`: the file name out of the clip's source path, without
/// its extension, lowercased.
fn short_name(name: &str) -> String {
    let name = name.replace('\\', "/");
    let tail = match name.rfind('/') {
        Some(i) if i + 1 < name.len() => &name[i + 1..],
        _ => name.as_str(),
    };
    let stem = match tail.find('.') {
        Some(i) if i > 0 => &tail[..i],
        _ => tail,
    };
    stem.to_lowercase()
}

/// How many tags a clip carries. `ClipTagList`: pointer, then two counts that
/// agree. The tags themselves are not read — see the module note.
fn tag_count(r: &mut ResourceReader<'_>, pointer: u64) -> Result<usize> {
    let count = r.at(pointer, |r| {
        let _tags_pointer = r.read_u64()?;
        Ok(r.read_u16()? as usize)
    })?;
    Ok(count.unwrap_or(0).min(MAX_ELEMENTS))
}

/// How many properties a clip carries. `ClipPropertyMap` is another bucket
/// array, so the *second* count is the one to report: the first is the bucket
/// capacity, and reading it as the count overstates a sparse map.
fn property_count(r: &mut ResourceReader<'_>, pointer: u64) -> Result<usize> {
    let count = r.at(pointer, |r| {
        let _entries_pointer = r.read_u64()?;
        let _capacity = r.read_u16()?;
        Ok(r.read_u16()? as usize)
    })?;
    Ok(count.unwrap_or(0).min(MAX_ELEMENTS))
}

/// An animation block, as `.ycd` viewers care about it.
///
/// The `Hash` field the reference sets is not in the block: `AnimationMapEntry`
/// copies its own key into it after reading. That is reproduced here, which is
/// why the hash is a parameter rather than something this reads.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Animation {
    pub hash: u32,
    pub frames: u16,
    /// The frame count one sequence covers. An animation longer than this is
    /// split across several sequences.
    pub sequence_frame_limit: u16,
    pub duration: f32,
    /// How many clips reference this animation, as the file records it.
    pub usage_count: u32,
    /// One per animated bone track: which bone, and which of its channels.
    pub bone_ids: Vec<AnimationBoneId>,
    /// Per sequence, the frame count it holds. The channel data behind each is
    /// not decoded — see the module note.
    pub sequence_frames: Vec<u16>,
}

impl Animation {
    /// Frames per second, as the file's own numbers give it. Rockstar's
    /// animations are authored at 30 fps and the two agree in every stock
    /// file, but this reports what the file says rather than assuming.
    pub fn frame_rate(&self) -> f32 {
        if self.duration > 0.0 {
            self.frames as f32 / self.duration
        } else {
            0.0
        }
    }
}

/// One animated track: `AnimationBoneId`, 4 bytes.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationBoneId {
    pub bone_id: u16,
    pub unknown: u8,
    /// 0 is position, 1 rotation, 2 scale; 17 and 18 are the UV tracks. The
    /// reference names none of these, so neither does this.
    pub track: u8,
}

/// `AnimationMapEntry`: 32 bytes. Returns the animation with the address its
/// block sits at, and the next entry in the bucket.
///
/// The address travels with the animation because that is the only handle a
/// clip has on it: `Animation.Hash` is written here by the entry, not stored
/// in the block.
#[allow(clippy::type_complexity)]
fn read_animation_map_entry(r: &mut ResourceReader<'_>) -> Result<(Option<(u64, Animation)>, u64)> {
    let hash = r.read_u32()?;
    let _unknown_04h = r.read_u32()?;
    let animation_pointer = r.read_u64()?;
    let next_pointer = r.read_u64()?;

    let animation = r.at(animation_pointer, |r| read_animation(r, hash))?;
    Ok((animation.map(|a| (animation_pointer, a)), next_pointer))
}

/// The 96-byte `Animation` block.
fn read_animation(r: &mut ResourceReader<'_>, hash: u32) -> Result<Animation> {
    let _vft = r.read_u32()?;
    let _unknown_04h = r.read_u32()?;
    let _unused_08h = r.read_u32()?;
    let _unused_0ch = r.read_u32()?;
    let _unknown_10h = r.read_u8()?;
    let _unknown_11h = r.read_u8()?;
    let _unused_12h = r.read_u16()?;
    let frames = r.read_u16()?;
    let sequence_frame_limit = r.read_u16()?;
    let duration = r.read_f32()?;
    let _unknown_1ch = r.read_u32()?;
    for _ in 0..6 {
        let _unused = r.read_u32()?;
    }
    let _max_sequence_block_length = r.read_u32()?;
    let usage_count = r.read_u32()?;

    // ResourcePointerList64<Sequence>: pointers are stored at `capacity` and
    // items read at `count`, as everywhere else in the resource layer.
    let sequences_pointer = r.read_u64()?;
    let sequences_count = r.read_u16()? as usize;
    let sequences_capacity = r.read_u16()? as usize;
    let _sequences_pad = r.read_u32()?;

    // ResourceSimpleList64_s<AnimationBoneId>: structs read at `count`.
    let bone_ids_pointer = r.read_u64()?;
    let bone_ids_count = r.read_u16()? as usize;
    let _bone_ids_capacity = r.read_u16()?;
    let _bone_ids_pad = r.read_u32()?;

    let bone_ids = read_bone_ids(r, bone_ids_pointer, bone_ids_count)?;

    let mut sequence_frames = Vec::new();
    if sequences_capacity <= MAX_ELEMENTS {
        let pointers = r
            .read_u64s_at(sequences_pointer, sequences_capacity)?
            .unwrap_or_default();
        for pointer in pointers.into_iter().take(sequences_count) {
            // Only the sequence header: the channel data behind it is the
            // 900-line codec that is not ported.
            let frames = r.at(pointer, |r| {
                r.set_position(r.position() + 0x16);
                r.read_u16()
            })?;
            sequence_frames.push(frames.unwrap_or(0));
        }
    }

    Ok(Animation {
        hash,
        frames,
        sequence_frame_limit,
        duration,
        usage_count,
        bone_ids,
        sequence_frames,
    })
}

fn read_bone_ids(
    r: &mut ResourceReader<'_>,
    pointer: u64,
    count: usize,
) -> Result<Vec<AnimationBoneId>> {
    if count == 0 || count > MAX_ELEMENTS {
        return Ok(Vec::new());
    }
    let Some(bytes) = r.read_bytes_at(pointer, count * 4)? else {
        return Ok(Vec::new());
    };
    Ok(bytes
        .chunks_exact(4)
        .map(|c| AnimationBoneId {
            bone_id: u16::from_le_bytes([c[0], c[1]]),
            unknown: c[2],
            track: c[3],
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::reader::SYSTEM_BASE;

    #[test]
    fn shortens_a_clip_path_the_way_the_reference_does() {
        assert_eq!(short_name(r"x:\anim\Move\walk.clip"), "walk");
        assert_eq!(short_name("x:/anim/idle_a.clip"), "idle_a");
        assert_eq!(short_name("NoPath"), "nopath");
        assert_eq!(short_name(""), "");
    }

    /// Zeros parse into an empty dictionary rather than failing, the same way
    /// a junk drawable does. `has_content` is what the viewer asks instead.
    #[test]
    fn reads_an_empty_dictionary_without_failing() {
        let data = vec![0u8; 0x100];
        let mut r = ResourceReader::new(&data, data.len());
        let dictionary = ClipDictionary::read(&mut r).unwrap();
        assert!(dictionary.clips.is_empty());
        assert!(dictionary.animations.is_empty());
        assert!(!dictionary.has_content());
    }

    /// A bucket chain that points at itself must not hang the reader. No real
    /// file needs this; the tool opens files it did not create.
    #[test]
    fn refuses_to_follow_a_bucket_chain_forever() {
        let mut data = vec![0u8; 0x200];
        let addr = |o: u64| SYSTEM_BASE + o;
        // Dictionary: one bucket at 0x100, pointing at an entry at 0x110.
        data[0x28..0x30].copy_from_slice(&addr(0x100).to_le_bytes());
        data[0x30..0x32].copy_from_slice(&1u16.to_le_bytes()); // bucket count
        data[0x32..0x34].copy_from_slice(&1u16.to_le_bytes()); // declared entries
        data[0x100..0x108].copy_from_slice(&addr(0x110).to_le_bytes());
        // The entry's clip pointer is null and its next pointer is itself.
        data[0x120..0x128].copy_from_slice(&addr(0x110).to_le_bytes());

        let mut r = ResourceReader::new(&data, data.len());
        let dictionary = ClipDictionary::read(&mut r).unwrap();
        assert!(dictionary.clips.is_empty());
    }
}
