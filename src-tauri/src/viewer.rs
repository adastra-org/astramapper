//! Looking at one file: open it, then read it as text, as hex pages, or as a
//! mesh.
//!
//! The rule from `docs/architecture.md` is that the WebView never parses archive
//! data. So every command here hands back something already interpreted — a
//! decoded string, a mesh bundle, or an explicitly requested byte range for the
//! hex view, which is the one case where bytes are the answer rather than the
//! input.
//!
//! One file is open at a time and its extracted payload is held in
//! [`ViewerState`]. Extraction means a seek, a decrypt and an inflate; a hex
//! view that pages through a 20 MB texture dictionary would otherwise redo all
//! three per scroll.

use std::path::PathBuf;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cw_core::audio::AwcFile;
use cw_core::filetype::FileKind;
use cw_core::image;
use cw_core::mesh::MeshBundle;
use cw_core::meta::{read_meta as read_metadata, MetaNode};
use cw_core::resources::{
    read_clips as read_clip_dictionary, read_drawables, read_named_drawables, read_textures,
    Drawable, Texture, TextureDictionary,
};
use cw_core::rpf::EntryKind;
use cw_core::text::{self, Encoding, TextFile};
use cw_core::utils::hash_string;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::explorer::GameState;

/// How many bytes one `read_hex` call will hand over. A page larger than this
/// is slower to serialize than the user can read.
const MAX_HEX_PAGE: usize = 64 * 1024;

/// The currently open file and its extracted payload.
pub struct OpenFile {
    pub info: EntryInfo,
    pub data: Vec<u8>,
    /// The AWC key, carried so the audio commands can decrypt without taking
    /// the game lock again.
    pub awc_key: [u32; 4],
}

#[derive(Default)]
pub struct ViewerState(pub Mutex<Option<OpenFile>>);

/// The system and graphics segment sizes of a resource, and its format version.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    pub system_size: usize,
    pub graphics_size: usize,
    /// 165 for `.ydr`/`.ydd`, 162 for `.yft`, 13 for `.ytd`.
    pub version: u32,
}

/// What the frontend needs to decide how to show a file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryInfo {
    pub path: String,
    pub name: String,
    /// `binary` or `resource`, as in a listing.
    pub kind: &'static str,
    pub file_kind: FileKind,
    /// Human label for `file_kind`, e.g. "drawable dictionary".
    pub file_kind_label: &'static str,
    /// Size of the entry as stored in the archive.
    pub size_on_disk: u64,
    /// Size of the payload once decrypted and decompressed.
    pub byte_length: usize,
    pub encrypted: bool,
    pub resource: Option<ResourceInfo>,
    /// The viewer the UI should open by default.
    pub viewer: &'static str,
    /// Every viewer that will work for this file. Hex is always in here.
    pub viewers: Vec<&'static str>,
    /// Why the preferred viewer is not the one the extension implies — a
    /// drawable that failed to parse, for instance. Shown, not swallowed.
    pub note: Option<String>,
}

/// One page of bytes for the hex view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HexPage {
    pub offset: usize,
    /// Base64 of the raw bytes in this range.
    pub bytes: String,
    pub length: usize,
    pub total: usize,
}

/// One drawable in an open `.ydr` or `.ydd`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDrawable {
    /// Position in the dictionary, and the handle the UI selects by.
    pub index: usize,
    /// The name from the dictionary's hash list, or `hash_XXXXXXXX`.
    pub name: String,
    pub hash: u32,
    /// Absent when this drawable holds nothing renderable, which is normal in
    /// a dictionary: it is still listed, so the count is never silently wrong.
    pub bundle: Option<MeshBundle>,
}

/// Everything drawable in one file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPreview {
    pub path: String,
    pub drawables: Vec<ModelDrawable>,
    /// How many of `drawables` produced no geometry.
    pub empty_drawables: usize,
}

/// Open an entry: extract its payload, classify it, and keep it for the reads
/// that follow.
///
/// Runs like `list_dir` rather than `open_game_folder`: the work is bounded by
/// one entry, not by 179 archives.
#[tauri::command]
pub fn open_entry(
    path: String,
    game: State<'_, GameState>,
    viewer: State<'_, ViewerState>,
) -> Result<EntryInfo, String> {
    let guard = game.0.lock().map_err(|_| "game state is poisoned")?;
    let loaded = guard.as_ref().ok_or("no game folder is open")?;

    let entry_ref = loaded
        .manager
        .find(&path)
        .ok_or_else(|| format!("{path} is not in the index"))?;
    let entry = loaded.manager.entry(entry_ref);

    if entry.is_directory() {
        return Err(format!("{path} is a directory"));
    }

    let data = loaded
        .manager
        .extract(entry_ref, &loaded.keys)
        .map_err(|e| format!("could not read {path}: {e}"))?;

    let file_kind = FileKind::from_name(&entry.name_lower);
    let resource = entry
        .resource_segments()
        .map(|(system, graphics)| ResourceInfo {
            system_size: system,
            graphics_size: graphics,
            version: entry.resource_version().unwrap_or(0),
        });

    let awc_key = loaded.keys.awc_key;
    let (viewer_name, viewers, note) = choose_viewer(file_kind, &data, resource.as_ref(), &awc_key);

    let info = EntryInfo {
        path: entry.path.clone(),
        name: entry.name.clone(),
        kind: match &entry.kind {
            EntryKind::Directory(_) => "directory",
            EntryKind::Binary(_) if file_kind == FileKind::Archive => "archive",
            EntryKind::Binary(_) => "binary",
            EntryKind::Resource(_) => "resource",
        },
        file_kind,
        file_kind_label: file_kind.label(),
        size_on_disk: entry.file_size_on_disk(),
        byte_length: data.len(),
        encrypted: entry.is_encrypted(),
        resource,
        viewer: viewer_name,
        viewers,
        note,
    };

    *viewer.0.lock().map_err(|_| "viewer state is poisoned")? = Some(OpenFile {
        info: info.clone(),
        data,
        awc_key,
    });

    Ok(info)
}

/// Pick the default viewer, and say what else would work.
///
/// A model that will not parse falls back rather than showing an empty canvas,
/// and the reason travels with it — silently degrading to hex would look like
/// the file was never a model.
fn choose_viewer(
    kind: FileKind,
    data: &[u8],
    resource: Option<&ResourceInfo>,
    awc_key: &[u32; 4],
) -> (&'static str, Vec<&'static str>, Option<String>) {
    let readable_as_text = text::is_text(data);
    let mut viewers = vec!["hex"];
    if readable_as_text {
        viewers.insert(0, "text");
    }

    if kind.has_audio() {
        return match AwcFile::read(data, awc_key) {
            Ok(awc) if awc.streams.iter().any(|s| s.is_playable()) => {
                viewers.insert(0, "audio");
                ("audio", viewers, None)
            }
            Ok(_) => (
                viewers[0],
                viewers.clone(),
                Some("this container holds no playable streams".into()),
            ),
            Err(e) => (viewers[0], viewers.clone(), Some(format!("{e}"))),
        };
    }

    if kind.has_clips() {
        match resource {
            Some(resource) => match read_clip_dictionary(kind, data, resource.system_size) {
                Ok(dictionary) if dictionary.has_content() => {
                    viewers.insert(0, "clip");
                    return ("clip", viewers, None);
                }
                Ok(_) => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("this dictionary holds no clips or animations".into()),
                    )
                }
                Err(e) => {
                    let note = format!("could not read the clips: {e}");
                    return (viewers[0], viewers.clone(), Some(note));
                }
            },
            None => {
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("a clip dictionary stored as a binary entry, not a resource".into()),
                )
            }
        }
    }

    if kind.has_metadata() {
        let system_size = resource.map(|r| r.system_size);
        match read_metadata(kind, data, system_size) {
            Ok(meta) if meta.has_content() => {
                viewers.insert(0, "meta");
                return ("meta", viewers, None);
            }
            Ok(_) => {
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("this metadata file holds no structures".into()),
                )
            }
            Err(e) => {
                let note = format!("could not read the metadata: {e}");
                return (viewers[0], viewers.clone(), Some(note));
            }
        }
    }

    if kind.has_textures() {
        match resource {
            Some(resource) => match read_textures(kind, data, resource.system_size) {
                Ok(dictionary) if dictionary.textures.iter().any(Texture::is_decodable) => {
                    viewers.insert(0, "texture");
                    return ("texture", viewers, None);
                }
                Ok(dictionary) if dictionary.textures.is_empty() => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("this dictionary holds no textures".into()),
                    )
                }
                Ok(_) => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("none of the textures here are in a format with a decoder".into()),
                    )
                }
                Err(e) => {
                    let note = format!("could not read the textures: {e}");
                    return (viewers[0], viewers.clone(), Some(note));
                }
            },
            None => {
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("a texture dictionary stored as a binary entry, not a resource".into()),
                )
            }
        }
    }

    if kind.has_model() {
        match resource {
            Some(resource) => match read_drawables(kind, data, resource.system_size) {
                Ok(drawables) if drawables.iter().any(Drawable::has_geometry) => {
                    viewers.insert(0, "model");
                    return ("model", viewers, None);
                }
                Ok(_) => {
                    return (
                        viewers[0],
                        viewers.clone(),
                        Some("this file holds no renderable geometry".into()),
                    )
                }
                Err(e) => {
                    let note = format!("could not read the geometry: {e}");
                    return (viewers[0], viewers.clone(), Some(note));
                }
            },
            None => {
                return (
                    viewers[0],
                    viewers.clone(),
                    Some("a drawable stored as a binary entry, not a resource".into()),
                )
            }
        }
    }

    let note = match kind {
        FileKind::Fragment | FileKind::Particle if !readable_as_text => {
            Some("fragments and particle dictionaries embed drawables, but Frag.cs and Particle.cs are not ported yet".into())
        }
        FileKind::AudioMetadata if !readable_as_text => {
            Some("RelFile.cs, which names and sequences the waves in the .awc files, is not ported yet".into())
        }
        FileKind::Clip | FileKind::Nav | FileKind::Bounds if !readable_as_text => {
            // `.yed`, `.yfd`, `.yld` and `.ypdb` land here: they are animation
            // files but not clip dictionaries, and each needs its own reader.
            Some(format!("no {} parser yet, so this opens as bytes", kind.label()))
        }
        _ => None,
    };

    (viewers[0], viewers, note)
}

/// Decode the open file as text.
#[tauri::command]
pub fn read_text(viewer: State<'_, ViewerState>) -> Result<TextFile, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    Ok(text::decode(&open.data))
}

/// One page of the open file's bytes.
#[tauri::command]
pub fn read_hex(
    offset: usize,
    length: usize,
    viewer: State<'_, ViewerState>,
) -> Result<HexPage, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let total = open.data.len();
    let start = offset.min(total);
    let end = start.saturating_add(length.min(MAX_HEX_PAGE)).min(total);
    let slice = &open.data[start..end];

    Ok(HexPage {
        offset: start,
        bytes: STANDARD.encode(slice),
        length: slice.len(),
        total,
    })
}

/// Build the mesh bundles for the open file.
#[tauri::command]
pub fn read_model(viewer: State<'_, ViewerState>) -> Result<ModelPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let resource = open
        .info
        .resource
        .as_ref()
        .ok_or("this file is not a resource, so it has no geometry")?;

    let named = read_named_drawables(open.info.file_kind, &open.data, resource.system_size)
        .map_err(|e| format!("could not read the geometry: {e}"))?;

    let mut empty_drawables = 0;
    let drawables: Vec<ModelDrawable> = named
        .into_iter()
        .map(|entry| {
            let bundle = MeshBundle::best(&entry.drawable);
            if bundle.is_none() {
                empty_drawables += 1;
            }
            ModelDrawable {
                index: entry.index,
                name: entry.name,
                hash: entry.hash,
                bundle,
            }
        })
        .collect();

    if drawables.iter().all(|d| d.bundle.is_none()) {
        return Err("this file holds no renderable geometry".into());
    }

    Ok(ModelPreview {
        path: open.info.path.clone(),
        drawables,
        empty_drawables,
    })
}

/// One texture in an open `.ytd`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureEntry {
    /// Position in the dictionary, and the handle for `read_texture_image`.
    pub index: usize,
    /// The name the file carries. Textures are named, unlike AWC streams.
    pub name: String,
    pub name_hash: u32,
    pub width: u16,
    pub height: u16,
    pub levels: u8,
    /// The `D3DFORMAT` name, e.g. `D3DFMT_DXT5`.
    pub format: String,
    pub usage: &'static str,
    /// Size of the pixel payload, all mips together.
    pub byte_length: usize,
    /// False when the format has no decoder or the payload is short. The entry
    /// is still listed — a dictionary showing eight of nine textures with no
    /// explanation for the ninth is worse than showing it greyed out.
    pub decodable: bool,
}

/// What an open `.ytd` contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TexturePreview {
    pub path: String,
    pub textures: Vec<TextureEntry>,
}

/// One decoded mip, as a PNG.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureImage {
    pub index: usize,
    pub level: usize,
    pub width: usize,
    pub height: usize,
    /// Base64 of a complete PNG, ready for an `<img>` element.
    pub png: String,
}

/// Parse the open file as a texture dictionary. Kept private so all three
/// texture commands agree on what "the open file" means.
fn open_ytd(open: &OpenFile) -> Result<TextureDictionary, String> {
    let resource = open
        .info
        .resource
        .as_ref()
        .ok_or("this file is not a resource, so it holds no textures")?;
    read_textures(open.info.file_kind, &open.data, resource.system_size)
        .map_err(|e| format!("could not read the textures: {e}"))
}

/// List the textures in the open `.ytd`.
///
/// The pixels are not decoded here: a dictionary runs to dozens of textures and
/// tens of megabytes, and the listing is what the UI needs first — the same
/// split as `read_audio`.
#[tauri::command]
pub fn read_texture(viewer: State<'_, ViewerState>) -> Result<TexturePreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let dictionary = open_ytd(open)?;

    let textures = dictionary
        .textures
        .iter()
        .enumerate()
        .map(|(index, texture)| TextureEntry {
            index,
            name: texture
                .name
                .clone()
                .unwrap_or_else(|| format!("{:08x}", texture.name_hash)),
            name_hash: texture.name_hash,
            width: texture.width,
            height: texture.height,
            levels: texture.levels,
            format: texture
                .format
                .map(|f| f.label().to_string())
                .unwrap_or_else(|| format!("unknown ({:#x})", texture.format_raw)),
            usage: texture.usage.label(),
            byte_length: texture.data.len(),
            decodable: texture.is_decodable(),
        })
        .collect();

    Ok(TexturePreview {
        path: open.info.path.clone(),
        textures,
    })
}

/// Decode one texture's mip level to a PNG.
///
/// Decoded in Rust rather than in the WebView, per the boundary rule: no
/// browser reads BC-compressed blocks, and shipping the raw payload across
/// would be handing the frontend archive bytes to parse. PNG rather than raw
/// RGBA because a 2048² texture is 16 MB of pixels and 22 MB once base64'd.
#[tauri::command]
pub fn read_texture_image(
    index: usize,
    level: usize,
    viewer: State<'_, ViewerState>,
) -> Result<TextureImage, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let dictionary = open_ytd(open)?;

    let texture = dictionary
        .textures
        .get(index)
        .ok_or_else(|| format!("texture {index} is not in this dictionary"))?;

    let decoded = image::decode_mip(texture, level).map_err(|e| format!("{e}"))?;
    let png = image::to_png(&decoded).map_err(|e| format!("{e}"))?;

    Ok(TextureImage {
        index,
        level,
        width: decoded.width,
        height: decoded.height,
        png: STANDARD.encode(png),
    })
}

/// The file format `save_texture` writes.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextureExport {
    /// The stored pixels with a DDS header in front — no decode, no loss, and
    /// the whole mip chain.
    Dds,
    /// One decoded mip as RGBA. Lossless as an encoding, but a projection of
    /// the source format onto four 8-bit channels.
    Png,
}

/// Write one texture to disk as a `.dds` or a `.png`.
#[tauri::command]
pub fn save_texture(
    index: usize,
    level: usize,
    format: TextureExport,
    destination: String,
    viewer: State<'_, ViewerState>,
) -> Result<usize, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let dictionary = open_ytd(open)?;

    let texture = dictionary
        .textures
        .get(index)
        .ok_or_else(|| format!("texture {index} is not in this dictionary"))?;

    let bytes = match format {
        TextureExport::Dds => image::to_dds(texture).map_err(|e| format!("{e}"))?,
        TextureExport::Png => {
            let decoded = image::decode_mip(texture, level).map_err(|e| format!("{e}"))?;
            image::to_png(&decoded).map_err(|e| format!("{e}"))?
        }
    };

    write_file(&destination, &bytes)?;
    Ok(bytes.len())
}

/// One clip in an open `.ycd`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipEntry {
    /// Position in the listing.
    pub index: usize,
    /// The clip's short name, or `hash_XXXXXXXX` when it carries no path.
    pub name: String,
    /// The full authoring path the clip was built from, when it has one.
    pub path: Option<String>,
    /// The hash the game looks this clip up by.
    pub hash: u32,
    /// `animation` or `animation list`.
    pub kind: &'static str,
    pub duration: f32,
    pub rate: f32,
    pub start_time: f32,
    pub end_time: f32,
    /// Hashes of the animations this clip plays, resolvable in `animations`.
    pub animations: Vec<u32>,
    /// Tags and properties are counted, not read: `ClipTag` and
    /// `ClipProperty` are not ported. See `docs/formats.md`.
    pub tag_count: usize,
    pub property_count: usize,
}

/// One animation in an open `.ycd`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationEntry {
    pub index: usize,
    pub hash: u32,
    /// `hash_XXXXXXXX` unless the name happens to be in the reference's table.
    pub name: String,
    pub frames: u16,
    pub duration: f32,
    /// Frames divided by duration, as the file's own numbers give it.
    pub frame_rate: f32,
    pub sequences: usize,
    /// One per animated bone track.
    pub bone_tracks: usize,
    /// How many clips the file says reference this animation.
    pub usage_count: u32,
}

/// What an open `.ycd` contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipPreview {
    pub path: String,
    pub clips: Vec<ClipEntry>,
    pub animations: Vec<AnimationEntry>,
}

/// List the clips and animations in the open `.ycd`.
///
/// The animation *data* is not decoded, here or anywhere: the channel codecs
/// in `Clip.cs` are not ported, and playing an animation would also need a
/// skeleton. This is the listing, which is what the Explorer needs first.
#[tauri::command]
pub fn read_clips(viewer: State<'_, ViewerState>) -> Result<ClipPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    let resource = open
        .info
        .resource
        .as_ref()
        .ok_or("this file is not a resource, so it holds no clips")?;
    let dictionary = read_clip_dictionary(open.info.file_kind, &open.data, resource.system_size)
        .map_err(|e| format!("could not read the clips: {e}"))?;

    let clips = dictionary
        .clips
        .iter()
        .enumerate()
        .map(|(index, clip)| ClipEntry {
            index,
            name: clip.display_name(),
            path: clip.name.clone(),
            hash: clip.hash,
            kind: clip.clip_type.label(),
            duration: clip.duration,
            rate: clip.rate,
            start_time: clip.start_time,
            end_time: clip.end_time,
            animations: clip.animations.clone(),
            tag_count: clip.tag_count,
            property_count: clip.property_count,
        })
        .collect();

    let animations = dictionary
        .animations
        .iter()
        .enumerate()
        .map(|(index, animation)| AnimationEntry {
            index,
            hash: animation.hash,
            name: hash_string(animation.hash),
            frames: animation.frames,
            duration: animation.duration,
            frame_rate: animation.frame_rate(),
            sequences: animation.sequence_frames.len(),
            bone_tracks: animation.bone_ids.len(),
            usage_count: animation.usage_count,
        })
        .collect();

    Ok(ClipPreview {
        path: open.info.path.clone(),
        clips,
        animations,
    })
}

/// What an open `.ymt` contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaPreview {
    pub path: String,
    /// `RSC`, `PSO` or `RBF`: which of the three encodings this file uses.
    pub format: String,
    /// The name of the root structure, which is what the file *is*.
    pub root_name: String,
    /// Total nodes, so the view can say how much it is showing.
    pub node_count: usize,
    pub tree: MetaNode,
}

/// Walk the open metadata file into a tree of named values.
#[tauri::command]
pub fn read_meta(viewer: State<'_, ViewerState>) -> Result<MetaPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;

    // A metadata file is RSC only when the archive says it is a resource; the
    // other two encodings are plain binary entries.
    let system_size = open.info.resource.as_ref().map(|r| r.system_size);
    let meta = read_metadata(open.info.file_kind, &open.data, system_size)
        .map_err(|e| format!("could not read the metadata: {e}"))?;

    let tree = meta.to_tree();
    Ok(MetaPreview {
        path: open.info.path.clone(),
        format: meta.format().to_string(),
        root_name: meta.root_name(),
        node_count: tree.count(),
        tree,
    })
}

/// One stream in an open `.awc`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    /// Position in the container, and the handle for `read_audio_track`.
    pub index: usize,
    /// The 29-bit name hash. Resolving it to a name needs the name tables from
    /// `MetaNames.cs`, which are not ported.
    pub id: u32,
    pub codec: &'static str,
    pub sample_rate: u16,
    pub samples: u32,
    pub seconds: f32,
    pub playable: bool,
    /// True when the stream is MIDI rather than audio.
    pub midi: bool,
    /// The chunk kinds this stream carries, for the detail line.
    pub chunks: Vec<&'static str>,
}

/// What an open `.awc` contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPreview {
    pub path: String,
    pub version: u16,
    pub multi_channel: bool,
    pub whole_file_encrypted: bool,
    pub tracks: Vec<AudioTrack>,
}

/// One decoded stream, as a `.wav`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioClip {
    pub index: usize,
    pub sample_rate: u16,
    pub samples: usize,
    /// Base64 of a complete RIFF/WAVE file, ready for an `<audio>` element.
    pub wav: String,
}

/// Parse the open file as an AWC. Kept private so both audio commands agree on
/// what "the open file" means.
fn open_awc(open: &OpenFile) -> Result<AwcFile, String> {
    if !open.info.file_kind.has_audio() {
        return Err(format!("{} is not an audio container", open.info.name));
    }
    AwcFile::read(&open.data, &open.awc_key).map_err(|e| format!("{e}"))
}

/// List the streams in the open `.awc`.
///
/// The waves themselves are not decoded here: a container runs to hundreds of
/// streams and tens of megabytes, and the listing is what the UI needs first.
#[tauri::command]
pub fn read_audio(viewer: State<'_, ViewerState>) -> Result<AudioPreview, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let awc = open_awc(open)?;

    let tracks = awc
        .streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| !stream.is_carrier)
        .map(|(index, stream)| AudioTrack {
            index,
            id: stream.id,
            codec: stream.format.map(|f| f.codec.label()).unwrap_or("none"),
            sample_rate: stream.format.map(|f| f.sample_rate).unwrap_or(0),
            samples: stream.format.map(|f| f.samples).unwrap_or(0),
            seconds: stream.format.map(|f| f.seconds()).unwrap_or(0.0),
            playable: stream.is_playable(),
            midi: stream.midi.is_some(),
            chunks: stream.chunks.iter().map(|c| c.kind.label()).collect(),
        })
        .collect();

    Ok(AudioPreview {
        path: open.info.path.clone(),
        version: awc.version,
        multi_channel: awc.multi_channel,
        whole_file_encrypted: awc.whole_file_encrypted,
        tracks,
    })
}

/// Decode one stream to a `.wav`.
///
/// ADPCM is decoded in Rust rather than in the WebView, per the boundary rule:
/// browsers do not play Rockstar's ADPCM framing, and shipping the raw blocks
/// across would be handing the frontend archive bytes to parse.
#[tauri::command]
pub fn read_audio_track(index: usize, viewer: State<'_, ViewerState>) -> Result<AudioClip, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let awc = open_awc(open)?;

    let stream = awc
        .streams
        .get(index)
        .ok_or_else(|| format!("stream {index} is not in this container"))?;
    if !stream.is_playable() {
        return Err(format!("stream {index} holds no playable audio"));
    }

    let samples = stream.samples();
    Ok(AudioClip {
        index,
        sample_rate: stream.format.map(|f| f.sample_rate).unwrap_or(0),
        samples: samples.len(),
        wav: STANDARD.encode(stream.to_wav()),
    })
}

/// Write the open file's payload to `destination`, as extracted — decrypted and
/// decompressed, which is what every other tool expects a `.ydr` on disk to be.
#[tauri::command]
pub fn extract_to(destination: String, viewer: State<'_, ViewerState>) -> Result<usize, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    write_file(&destination, &open.data)?;
    Ok(open.data.len())
}

/// Write one decoded stream to disk as a `.wav`.
#[tauri::command]
pub fn save_audio_track(
    index: usize,
    destination: String,
    viewer: State<'_, ViewerState>,
) -> Result<usize, String> {
    let guard = viewer.0.lock().map_err(|_| "viewer state is poisoned")?;
    let open = guard.as_ref().ok_or("no file is open")?;
    let awc = open_awc(open)?;

    let stream = awc
        .streams
        .get(index)
        .ok_or_else(|| format!("stream {index} is not in this container"))?;
    let wav = stream.to_wav();
    write_file(&destination, &wav)?;
    Ok(wav.len())
}

/// The encoding an edited file should be written back in. Mirrors
/// [`cw_core::text::Encoding`] on the wire.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

impl From<WireEncoding> for Encoding {
    fn from(value: WireEncoding) -> Self {
        match value {
            WireEncoding::Utf8 => Encoding::Utf8,
            WireEncoding::Utf8Bom => Encoding::Utf8Bom,
            WireEncoding::Utf16Le => Encoding::Utf16Le,
            WireEncoding::Utf16Be => Encoding::Utf16Be,
        }
    }
}

/// Write edited text to disk in the encoding it was read as.
///
/// To disk, not back into the archive: RPF *writing* is not ported — see
/// `docs/formats.md`. The UI says so rather than offering a save that would
/// quietly do something else.
#[tauri::command]
pub fn save_text(
    destination: String,
    contents: String,
    encoding: WireEncoding,
) -> Result<usize, String> {
    let bytes = text::encode(&contents, encoding.into());
    write_file(&destination, &bytes)?;
    Ok(bytes.len())
}

fn write_file(destination: &str, bytes: &[u8]) -> Result<(), String> {
    let path = PathBuf::from(destination);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(system_size: usize) -> ResourceInfo {
        ResourceInfo {
            system_size,
            graphics_size: 0,
            version: 165,
        }
    }

    /// The AWC key only matters for `.awc`, which none of these cases are.
    fn choose_viewer_for_test(
        kind: FileKind,
        data: &[u8],
        resource: Option<&ResourceInfo>,
    ) -> (&'static str, Vec<&'static str>, Option<String>) {
        choose_viewer(kind, data, resource, &[0; 4])
    }

    #[test]
    fn offers_text_for_a_meta_file_and_hex_as_a_fallback() {
        let xml = b"<?xml version=\"1.0\"?>\n<CVehicleModelInfo />\n";
        let (viewer, viewers, note) = choose_viewer_for_test(FileKind::Text, xml, None);
        assert_eq!(viewer, "text");
        assert_eq!(viewers, vec!["text", "hex"]);
        assert!(note.is_none());
    }

    /// Hex is always available, whatever else is.
    #[test]
    fn always_offers_hex() {
        for (kind, data) in [
            (FileKind::Text, b"plain".as_slice()),
            (FileKind::Data, &[0u8, 1, 2, 3]),
            (FileKind::Unknown, &[0xFF; 64]),
        ] {
            let (_, viewers, _) = choose_viewer_for_test(kind, data, None);
            assert!(viewers.contains(&"hex"), "{kind:?} does not offer hex");
        }
    }

    /// A `.ydr` whose bytes are not a drawable must say why rather than opening
    /// an empty 3D view.
    #[test]
    fn explains_a_model_that_will_not_parse() {
        let junk = vec![0xFFu8; 64];
        let (viewer, _, note) =
            choose_viewer_for_test(FileKind::Model, &junk, Some(&resource(4096)));
        assert_eq!(viewer, "hex");
        let note = note.expect("a reason");
        assert!(note.contains("geometry"), "unhelpful note: {note}");
    }

    /// A `.ydr` that parses without error but carries nothing to draw is the
    /// common failure, not a parse error: the reader zero-fills past the end of
    /// a short payload, so junk reads as a drawable with no models.
    #[test]
    fn does_not_open_the_3d_view_for_a_drawable_with_no_geometry() {
        let empty = vec![0u8; 512];
        let (viewer, viewers, note) =
            choose_viewer_for_test(FileKind::Model, &empty, Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"model"));
        assert!(note.unwrap().contains("renderable"));
    }

    /// A `.ytd` whose bytes hold no decodable texture must say so rather than
    /// opening an empty gallery. Zeros parse into an empty dictionary, the same
    /// way they parse into a drawable with no models.
    #[test]
    fn does_not_open_the_texture_view_for_an_empty_dictionary() {
        let empty = vec![0u8; 512];
        let (viewer, viewers, note) =
            choose_viewer_for_test(FileKind::Texture, &empty, Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"texture"));
        assert!(note.unwrap().contains("no textures"));
    }

    /// A `.ycd` whose bytes hold neither a clip nor an animation must say so
    /// rather than opening an empty listing. Zeros parse into an empty
    /// dictionary, the same way they parse into a drawable with no models.
    #[test]
    fn does_not_open_the_clip_view_for_an_empty_dictionary() {
        let empty = vec![0u8; 512];
        let (viewer, viewers, note) =
            choose_viewer_for_test(FileKind::ClipDictionary, &empty, Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"clip"));
        assert!(note.unwrap().contains("no clips"));
    }

    /// A `.ymt` that is neither a resource nor one of the two binary encodings
    /// has to say so: it is a metadata file the reader does not recognise, not
    /// an empty one.
    #[test]
    fn does_not_open_the_metadata_view_for_bytes_with_no_container() {
        let junk: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        let (viewer, viewers, note) = choose_viewer_for_test(FileKind::Meta, &junk, None);
        assert_eq!(viewer, "hex");
        assert!(!viewers.contains(&"meta"));
        assert!(note.unwrap().contains("could not read the metadata"));
    }

    /// An RBF payload is a plain binary entry, so the metadata view has to open
    /// without any resource information.
    #[test]
    fn opens_the_metadata_view_for_a_binary_entry() {
        // `RBF0`, then a descriptor for an element named `root` with one
        // uint child, then the two close markers.
        let mut rbf = b"RBF0".to_vec();
        rbf.extend_from_slice(&[0, 0, 4, 0]);
        rbf.extend_from_slice(b"root");
        rbf.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        rbf.extend_from_slice(&[1, 0x10, 5, 0]);
        rbf.extend_from_slice(b"count");
        rbf.extend_from_slice(&7u32.to_le_bytes());
        rbf.extend_from_slice(&[0xFF, 0xFF]);

        let (viewer, viewers, note) = choose_viewer_for_test(FileKind::Meta, &rbf, None);
        assert_eq!(viewer, "meta", "{note:?}");
        assert!(viewers.contains(&"meta"));
    }

    /// `.yed` and its siblings are animation files with no ported reader, so
    /// they must fall back with a reason rather than trying the `.ycd` path.
    #[test]
    fn does_not_treat_the_other_animation_formats_as_clip_dictionaries() {
        assert!(!FileKind::from_name("x.yed").has_clips());
        let (viewer, _, note) =
            choose_viewer_for_test(FileKind::Clip, &vec![0u8; 512], Some(&resource(512)));
        assert_eq!(viewer, "hex");
        assert!(note.is_some());
    }

    /// A `.ytd` stored as a binary entry has no page flags, so the payload
    /// cannot be split into its two segments.
    #[test]
    fn refuses_a_texture_dictionary_with_no_resource_header() {
        let (viewer, _, note) = choose_viewer_for_test(FileKind::Texture, &[0u8; 256], None);
        assert_eq!(viewer, "hex");
        assert!(note.unwrap().contains("binary entry"));
    }

    /// The formats that are not ported yet each get a note, so "opens as hex"
    /// never looks like a bug.
    #[test]
    fn names_the_formats_that_are_not_ported() {
        for kind in [
            FileKind::Fragment,
            FileKind::Particle,
            FileKind::Meta,
            FileKind::Bounds,
        ] {
            let binary = vec![0u8; 512];
            let (viewer, _, note) = choose_viewer_for_test(kind, &binary, Some(&resource(512)));
            assert_eq!(viewer, "hex", "{kind:?}");
            assert!(note.is_some(), "{kind:?} has no explanation");
        }
    }

    /// A `.ydr` stored as a binary entry has no page flags, so there is no way
    /// to split the payload. That is a real case in mod archives.
    #[test]
    fn refuses_a_drawable_with_no_resource_header() {
        let (viewer, _, note) = choose_viewer_for_test(FileKind::Model, &[0u8; 256], None);
        assert_eq!(viewer, "hex");
        assert!(note.unwrap().contains("binary entry"));
    }
}
