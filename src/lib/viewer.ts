/**
 * Typed wrappers over the viewer IPC commands in `src-tauri/src/viewer.rs`.
 *
 * These types mirror the Rust structs exactly. The backend serializes camelCase,
 * so the shapes here are the wire format, not a translation of it.
 *
 * One file is open at a time on the Rust side. `openEntry` extracts and holds
 * it; every read below operates on whatever that left open.
 */
import { invoke } from "@tauri-apps/api/core";

/** What a file is, from its extension. Mirrors `cw_core::filetype::FileKind`. */
export type FileKind =
  | "archive"
  | "model"
  | "modelDictionary"
  | "fragment"
  | "particle"
  | "texture"
  | "image"
  | "bounds"
  | "meta"
  | "clipDictionary"
  | "clip"
  | "nav"
  | "audio"
  | "audioMetadata"
  | "script"
  | "gxt2"
  | "text"
  | "data"
  | "unknown";

/** Which panel shows the file. */
export type ViewerId =
  | "text"
  | "hex"
  | "model"
  | "bounds"
  | "audio"
  | "texture"
  | "clip"
  | "meta"
  | "gxt2";

export type ResourceInfo = {
  readonly systemSize: number;
  readonly graphicsSize: number;
  readonly version: number;
};

export type EntryInfo = {
  readonly path: string;
  readonly name: string;
  readonly kind: "directory" | "archive" | "binary" | "resource";
  readonly fileKind: FileKind;
  readonly fileKindLabel: string;
  readonly sizeOnDisk: number;
  /** Length of the payload once decrypted and decompressed. */
  readonly byteLength: number;
  readonly encrypted: boolean;
  readonly resource: ResourceInfo | null;
  readonly viewer: ViewerId;
  readonly viewers: readonly ViewerId[];
  /** Why the extension's viewer is not the one that opened, if it is not. */
  readonly note: string | null;
};

/** The encoding a text file was read as, and is written back as. */
export type Encoding = "utf8" | "utf8Bom" | "utf16Le" | "utf16Be";

export type TextFile = {
  readonly text: string;
  readonly encoding: Encoding;
  /** Some bytes were invalid and became U+FFFD; saving would not round-trip. */
  readonly lossy: boolean;
  readonly truncated: boolean;
  readonly totalBytes: number;
};

export type HexPage = {
  readonly offset: number;
  /** Base64 of the raw bytes in this range. */
  readonly bytes: string;
  readonly length: number;
  readonly total: number;
};

export type MeshPart = {
  readonly name: string;
  /** Jenkins hash of the shader name. */
  readonly shader: number;
  /** Jenkins hash of the texture in the diffuse slot, if bound. */
  readonly diffuse: number | null;
  /** Jenkins hash of the texture in the normal slot, if bound. */
  readonly normal: number | null;
  /** Jenkins hash of the texture in the specular slot, if bound. */
  readonly specular: number | null;
  readonly vertexCount: number;
  readonly triangleCount: number;
  /** Base64 of packed `f32` triples. */
  readonly positions: string;
  readonly normals: string | null;
  readonly uvs: string | null;
  /** Base64 of packed `u32` triples. */
  readonly indices: string;
};

export type MeshBundle = {
  readonly name: string | null;
  readonly lod: string;
  readonly availableLods: readonly string[];
  readonly boundingBoxMin: readonly [number, number, number];
  readonly boundingBoxMax: readonly [number, number, number];
  readonly boundingSphereRadius: number;
  readonly vertexCount: number;
  readonly triangleCount: number;
  readonly parts: readonly MeshPart[];
};

/** One drawable in a `.ydr` or `.ydd`. */
export type ModelDrawable = {
  /** Position in the dictionary, and the handle the viewer selects by. */
  readonly index: number;
  /** The name from the dictionary's hash list, or `hash_XXXXXXXX`. */
  readonly name: string;
  readonly hash: number;
  /** Every LOD that has renderable geometry, highest first. Empty when none. */
  readonly lods: readonly MeshBundle[];
};

/** One resolved texture a model's parts reference, decoded to a PNG. */
export type ModelTexture = {
  readonly hash: number;
  readonly name: string;
  readonly width: number;
  readonly height: number;
  /** Base64 of a complete PNG. */
  readonly png: string;
};

export type ModelPreview = {
  readonly path: string;
  readonly drawables: readonly ModelDrawable[];
  readonly emptyDrawables: number;
  /** Textures the parts reference, keyed by `hash`. */
  readonly textures: readonly ModelTexture[];
};

/** One collision geometry in a `.ybn`, as uploadable buffers. */
export type BoundsPart = {
  /** The tree path from the bounds root, e.g. `child0`. */
  readonly name: string;
  readonly vertexCount: number;
  readonly triangleCount: number;
  /** Base64 of packed `f32` triples, in world space. */
  readonly positions: string;
  /** Base64 of packed `u32` triples. */
  readonly indices: string;
};

export type BoundsPreview = {
  readonly path: string;
  /** The root bounds type, e.g. `Composite`. */
  readonly boundsType: string;
  /** Number of children in the composite tree, when the root is one. */
  readonly children: number;
  readonly vertexCount: number;
  readonly triangleCount: number;
  readonly parts: readonly BoundsPart[];
};

/** One texture in a `.ytd`. */
export type TextureEntry = {
  /** Position in the dictionary, and the handle for `readTextureImage`. */
  readonly index: number;
  readonly name: string;
  readonly nameHash: number;
  readonly width: number;
  readonly height: number;
  readonly levels: number;
  /** The `D3DFORMAT` name, e.g. `D3DFMT_DXT5`. */
  readonly format: string;
  readonly usage: string;
  readonly byteLength: number;
  /** False when the format has no decoder or the payload is short. */
  readonly decodable: boolean;
};

export type TexturePreview = {
  readonly path: string;
  readonly textures: readonly TextureEntry[];
};

export type TextureImage = {
  readonly index: number;
  readonly level: number;
  readonly width: number;
  readonly height: number;
  /** Base64 of a complete PNG. */
  readonly png: string;
};

/** What `saveTexture` writes: the stored mip chain, or one decoded mip. */
export type TextureExport = "dds" | "png";

/** One clip in a `.ycd`. */
export type ClipEntry = {
  readonly index: number;
  /** The clip's short name, or `hash_XXXXXXXX`. */
  readonly name: string;
  /** The authoring path the clip was built from, if it carries one. */
  readonly path: string | null;
  readonly hash: number;
  /** `animation` or `animation list`. */
  readonly kind: string;
  readonly duration: number;
  readonly rate: number;
  readonly startTime: number;
  readonly endTime: number;
  /** Hashes of the animations this clip plays. */
  readonly animations: readonly number[];
  /** Counted, not read: ClipTag and ClipProperty are not ported. */
  readonly tagCount: number;
  readonly propertyCount: number;
};

/** One animation in a `.ycd`. */
export type AnimationEntry = {
  readonly index: number;
  readonly hash: number;
  readonly name: string;
  readonly frames: number;
  readonly duration: number;
  readonly frameRate: number;
  readonly sequences: number;
  readonly boneTracks: number;
  readonly usageCount: number;
};

export type ClipPreview = {
  readonly path: string;
  readonly clips: readonly ClipEntry[];
  readonly animations: readonly AnimationEntry[];
};

/** One entry of a `.gxt2` text table. */
export type Gxt2Entry = {
  readonly hash: number;
  readonly text: string;
};

export type Gxt2Preview = {
  readonly path: string;
  readonly entries: readonly Gxt2Entry[];
};

/** One named value in a metadata file, and its children. */
export type MetaNode = {
  readonly name: string;
  /** The structure or element type, when the file names one. */
  readonly typeName: string | null;
  /** The formatted value, for a leaf. */
  readonly value: string | null;
  /** The lookup name, for an entry of a map. */
  readonly key: string | null;
  readonly children: readonly MetaNode[];
  /** The reader could not walk this node, and the value says why. */
  readonly error: boolean;
};

export type MetaPreview = {
  readonly path: string;
  /** `RSC`, `PSO` or `RBF`. */
  readonly format: string;
  readonly rootName: string;
  readonly nodeCount: number;
  readonly tree: MetaNode;
};

/** One stream in an `.awc`. */
export type AudioTrack = {
  /** Position in the container, and the handle for `readAudioTrack`. */
  readonly index: number;
  /** The 29-bit name hash. There is no name: the tables are not ported. */
  readonly id: number;
  readonly codec: string;
  readonly sampleRate: number;
  readonly samples: number;
  readonly seconds: number;
  readonly playable: boolean;
  readonly midi: boolean;
  readonly chunks: readonly string[];
};

export type AudioPreview = {
  readonly path: string;
  readonly version: number;
  readonly multiChannel: boolean;
  readonly wholeFileEncrypted: boolean;
  readonly tracks: readonly AudioTrack[];
};

export type AudioClip = {
  readonly index: number;
  readonly sampleRate: number;
  readonly samples: number;
  /** Base64 of a complete RIFF/WAVE file. */
  readonly wav: string;
};

/** Extract an entry and classify it. Everything below reads what this opened. */
export function openEntry(path: string): Promise<EntryInfo> {
  return invoke<EntryInfo>("open_entry", { path });
}

export function readText(): Promise<TextFile> {
  return invoke<TextFile>("read_text");
}

export function readHex(offset: number, length: number): Promise<HexPage> {
  return invoke<HexPage>("read_hex", { offset, length });
}

export function readModel(): Promise<ModelPreview> {
  return invoke<ModelPreview>("read_model");
}

export function readBounds(): Promise<BoundsPreview> {
  return invoke<BoundsPreview>("read_bounds");
}

/** List the clips and animations in the open `.ycd`. */
export function readClips(): Promise<ClipPreview> {
  return invoke<ClipPreview>("read_clips");
}

/** Read the hash → text table the open `.gxt2` holds. */
export function readGxt2(): Promise<Gxt2Preview> {
  return invoke<Gxt2Preview>("read_gxt2");
}

/** Walk the open `.ymt` into a tree of named values. */
export function readMeta(): Promise<MetaPreview> {
  return invoke<MetaPreview>("read_meta");
}

/** List the textures in the open `.ytd`, without decoding any of them. */
export function readTexture(): Promise<TexturePreview> {
  return invoke<TexturePreview>("read_texture");
}

/** Decode one texture's mip level to a PNG. */
export function readTextureImage(index: number, level: number): Promise<TextureImage> {
  return invoke<TextureImage>("read_texture_image", { index, level });
}

/** Write one texture to disk. Returns the bytes written. */
export function saveTexture(
  index: number,
  level: number,
  format: TextureExport,
  destination: string,
): Promise<number> {
  return invoke<number>("save_texture", { index, level, format, destination });
}

/** List the streams in the open `.awc`, without decoding any of them. */
export function readAudio(): Promise<AudioPreview> {
  return invoke<AudioPreview>("read_audio");
}

/** Decode one stream to a `.wav`. */
export function readAudioTrack(index: number): Promise<AudioClip> {
  return invoke<AudioClip>("read_audio_track", { index });
}

/** Write one decoded stream to disk as a `.wav`. Returns the bytes written. */
export function saveAudioTrack(index: number, destination: string): Promise<number> {
  return invoke<number>("save_audio_track", { index, destination });
}

/** Write the open file's payload to disk. Returns the byte count written. */
export function extractTo(destination: string): Promise<number> {
  return invoke<number>("extract_to", { destination });
}

/** Write edited text to disk in `encoding`. Returns the byte count written. */
export function saveText(
  destination: string,
  contents: string,
  encoding: Encoding,
): Promise<number> {
  return invoke<number>("save_text", { destination, contents, encoding });
}

/**
 * Decode a base64 buffer from the backend into bytes.
 *
 * `atob` gives one character per byte, which is exactly what the typed-array
 * constructors want. Going through `fetch("data:...")` would be async for no
 * gain, and `Buffer` does not exist in a WebView.
 */
export function decodeBase64(encoded: string): Uint8Array {
  const binary = atob(encoded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/**
 * Reinterpret a base64 buffer as floats.
 *
 * The bytes are copied into a fresh buffer rather than viewed in place: the
 * decoded array's `byteOffset` is zero but its length need not be a multiple of
 * four if a buffer were ever truncated, and `Float32Array` throws on that.
 */
export function decodeFloats(encoded: string): Float32Array {
  const bytes = decodeBase64(encoded);
  const usable = bytes.length - (bytes.length % 4);
  return new Float32Array(bytes.buffer.slice(0, usable));
}

/** Reinterpret a base64 buffer as unsigned 32-bit integers. */
export function decodeUints(encoded: string): Uint32Array {
  const bytes = decodeBase64(encoded);
  const usable = bytes.length - (bytes.length % 4);
  return new Uint32Array(bytes.buffer.slice(0, usable));
}

/** The file name at the end of an archive path, for a save dialog. */
export function baseName(path: string): string {
  const cut = path.lastIndexOf("\\");
  return cut < 0 ? path : path.slice(cut + 1);
}

/** A duration in seconds as `m:ss`, the way the reference shows stream lengths. */
export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const whole = Math.round(seconds);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}
