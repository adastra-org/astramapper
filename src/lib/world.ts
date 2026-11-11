/**
 * Typed wrappers over the World Mode IPC commands in `src-tauri/src/world.rs`,
 * plus the pure geometry helpers the world renderer builds on.
 *
 * These types mirror the Rust structs exactly; the backend serializes camelCase.
 * The `invoke` wrappers need a Tauri host, so the tests cover the pure helpers
 * here (demo world, axis conversion, bounds, colours), not the wrappers.
 */
import { invoke } from "@tauri-apps/api/core";

import * as perf from "./perf";
import { ARCHETYPE_CLASS, type PlacementColumns } from "./world/columns";
import { decodePlacements } from "./world/placements";
import { asFloat32, asUint16, asUint32, Kind, readEnvelope, splitMips } from "./wire";

/** One content group: the archives that make up a slice of the install. */
export type ContentGroup = {
  readonly id: string;
  readonly label: string;
  readonly archives: number;
  readonly entries: number;
};

/** The world view's first question: what is mounted, and what is in it. */
export type WorldStatus = {
  readonly gameOpen: boolean;
  readonly folder: string | null;
  readonly dlcGroups: readonly ContentGroup[];
  /** The `.ymap` open in the Explorer, when there is one. */
  readonly openMapPath: string | null;
  /** Identifies the installed build (hex): the persistent model cache's key. */
  readonly installStamp: string | null;
};

/** What the machine has, for the precompile recommendation (#7). */
export type HardwareInfo = {
  readonly cores: number;
  readonly totalMemory: number;
  readonly availableMemory: number;
  /** Free bytes on the volume holding the asset cache; `null` if unknown. */
  readonly freeDiskBytes: number | null;
};

export function hardwareInfo(): Promise<HardwareInfo> {
  return invoke<HardwareInfo>("hardware_info");
}

/** One `.ymap` a content group holds. */
export type YmapListing = {
  readonly path: string;
  readonly name: string;
};

export type YmapListingPage = {
  readonly total: number;
  readonly ymaps: readonly YmapListing[];
};

/**
 * One placed entity in the world, flattened. Whether it comes from a `.ymap`
 * over IPC or from the synthetic demo, it is the same shape — the renderer
 * never cares where a placement came from.
 */
export type Placed = {
  readonly index: number;
  readonly name: string;
  readonly nameHash: number;
  readonly isMlo: boolean;
  /** True for an interior entity expanded out of an MLO archetype. */
  readonly fromMlo: boolean;
  readonly position: readonly [number, number, number];
  /** The stored quaternion `(x, y, z, w)`, handed to three.js unchanged. */
  readonly rotation: readonly [number, number, number, number];
  readonly scaleXy: number;
  readonly scaleZ: number;
  readonly parentIndex: number;
  readonly lodLevel: string;
  /** The entity's own LOD distance, the streamer's HD→LOD boundary. */
  readonly lodDist: number;
  readonly children: number;
  /**
   * What the archetype record says this is (`ARCHETYPE_CLASS`, from flags, asset
   * type and bounding box: #9). Absent or 0 means the data says nothing and the
   * name decides.
   */
  readonly archetypeClass?: number;
};

/** A `.ymap` read for the world. */
export type WorldMap = {
  readonly path: string;
  readonly name: string;
  readonly nameHash: number;
  readonly parent: string;
  readonly entityCount: number;
  readonly mloEntities: number;
  readonly extentsMin: readonly [number, number, number];
  readonly extentsMax: readonly [number, number, number];
  /** Columnar, not one object per entity: see `world/columns.ts`. */
  readonly entities: PlacementColumns;
  /** How many interior entities were expanded out of the map's MLO instances. */
  readonly mloExpanded: number;
};

export function worldStatus(): Promise<WorldStatus> {
  return invoke<WorldStatus>("world_status");
}

export function worldDlcYmaps(group: string): Promise<YmapListingPage> {
  return invoke<YmapListingPage>("world_dlc_ymaps", { group });
}

/** Every `.ymap` in a content group, uncapped — for loading a whole group. */
export function worldMapsInGroup(group: string): Promise<readonly YmapListing[]> {
  return invoke<YmapListing[]>("world_maps_in_group", { group });
}

/** Parsed maps kept for the session, so re-entering World (or re-placing the
 *  same map) skips the backend parse entirely. Maps are immutable by contract —
 *  edits live separately — so sharing the object is safe. */
const mapCache = new Map<string, WorldMap>();

/** Drop the parsed-map cache — called when the mounted game folder changes. */
export function clearWorldCaches(): void {
  mapCache.clear();
}

/**
 * Read a `.ymap` into flattened world placements.
 *
 * Comes back as a wire envelope rather than JSON: the base game's 763,716
 * placements weigh 201.5 MB as JSON, which is more to serialize than the maps
 * cost to read. See `src/lib/world/placements.ts`.
 */
export function worldMap(path: string): Promise<WorldMap> {
  const cached = mapCache.get(path);
  if (cached) {
    perf.record("worldMap (memo)", 0);
    return Promise.resolve(cached);
  }
  return perf
    .measureAsync(
      "worldMap ipc",
      () => invoke<ArrayBuffer>("world_map", { path }),
      (buffer) => buffer.byteLength,
    )
    .then((buffer) => {
      const map = perf.measure("worldMap decode", () =>
        decodePlacements(readEnvelope(buffer)),
      );
      mapCache.set(path, map);
      return map;
    });
}

/** Place a `.ymap` the user picked from disk, custom/mod maps included. */
export function worldMapFile(path: string): Promise<WorldMap> {
  return invoke<ArrayBuffer>("world_map_file", { path }).then((buffer) =>
    decodePlacements(readEnvelope(buffer)),
  );
}

/** Resolve an archetype name to a model file, or `null` when there is none. */
export function worldAsset(name: string): Promise<string | null> {
  // A reply of a few bytes: its round trip is the floor of what one IPC call
  // costs, to read `worldModel ipc` against.
  return perf.measureAsync("worldAsset ipc", () => invoke<string | null>("world_asset", { name }));
}

/** What the GL context can upload without the backend decoding first. */
export type GpuCaps = {
  /** `WEBGL_compressed_texture_s3tc` — BC1/2/3. */
  readonly s3tc: boolean;
  /** `EXT_texture_compression_rgtc` — BC4/BC5, GTA's normal maps. */
  readonly rgtc: boolean;
  /** `EXT_texture_compression_bptc` — BC7. */
  readonly bptc: boolean;
};

/**
 * Tell the backend which compressed formats this context takes.
 *
 * Must resolve before the first `worldModel` call. Until it does the backend
 * decodes every texture to RGBA: correct, four times the VRAM, and avoidable by
 * making this the first thing world mode does.
 */
export function setGpuCapabilities(caps: GpuCaps): Promise<void> {
  return invoke<void>("set_gpu_capabilities", { caps });
}

/** One mip level's dimensions and byte length inside a texture's section. */
export type WireMip = {
  readonly width: number;
  readonly height: number;
  readonly byteLength: number;
};

/** One texture as the world view receives it: metadata, pixels in a section. */
export type WireTexture = {
  readonly hash: number;
  readonly name: string;
  readonly width: number;
  readonly height: number;
  /** `bc1`..`bc7` for blocks, `rgba8` for the decoded fallback. */
  readonly format: string;
  readonly compressed: boolean;
  /** The chain, largest first. Never empty. */
  readonly mips: readonly WireMip[];
  readonly section: number;
  readonly source: string | null;
};

/** One draw call, decoded into views over the response buffer. */
export type WorldPart = {
  readonly name: string;
  readonly shader: number;
  /** Jenkins hash of the `.sps` file name; decides alpha test and hidden parts (`partDraw`).
   *  Absent in a model cached before v0.3.8. */
  readonly shaderFile?: number;
  /** The shader's render bucket: 3 is a cutout. Absent in a model cached before v0.3.8. */
  readonly renderBucket?: number;
  readonly diffuse: number | null;
  readonly normal: number | null;
  readonly specular: number | null;
  readonly vertexCount: number;
  readonly triangleCount: number;
  readonly positions: Float32Array;
  readonly normals: Float32Array | null;
  readonly uvs: Float32Array | null;
  /** `Uint16Array` unless the part indexes 65,536 vertices or more. */
  readonly indices: Uint16Array | Uint32Array;
};

export type WorldBundle = {
  readonly name: string | null;
  readonly lod: string;
  readonly availableLods: readonly string[];
  readonly boundingBoxMin: readonly [number, number, number];
  readonly boundingBoxMax: readonly [number, number, number];
  readonly boundingSphereRadius: number;
  readonly vertexCount: number;
  readonly triangleCount: number;
  readonly parts: readonly WorldPart[];
};

export type WorldDrawable = {
  readonly index: number;
  readonly name: string;
  readonly hash: number;
  readonly lods: readonly WorldBundle[];
};

/**
 * A streamed model: geometry and textures as views over one response buffer.
 *
 * **Nothing here is a copy.** Every typed array aliases the `ArrayBuffer` the
 * IPC response arrived in, so this object owns the only reference keeping those
 * bytes alive — and dropping it frees the lot. That is the point: the base64
 * path copied every buffer three times on the main thread before three.js ever
 * saw it.
 */
export type WorldModel = {
  readonly path: string;
  /** Size of the response buffer every array below aliases: what dropping this
   *  model frees, and what the model cache budgets against (#6). */
  readonly byteLength: number;
  readonly drawables: readonly WorldDrawable[];
  readonly emptyDrawables: number;
  readonly textures: readonly (WireTexture & { readonly levels: readonly Uint8Array[] })[];
};

/** The metadata shape the backend writes into the envelope, before decoding. */
type MetaPart = Omit<WorldPart, "positions" | "normals" | "uvs" | "indices"> & {
  readonly positions: number;
  readonly normals: number | null;
  readonly uvs: number | null;
  readonly indices: number;
  readonly indexBits: number;
};

/**
 * Read a model file's geometry, every LOD, for the world view to draw.
 *
 * Comes back as a wire envelope rather than JSON: the pixels used to be decoded
 * out of their block format, re-encoded as PNG and base64'd, which measured
 * 11.60s against 2.57s over 58 real city models, for a *larger* payload. See
 * `src-tauri/src/textures.rs`.
 */
// In-flight `world_model` invokes. Measures the queue behind the IPC, which the
// span alone cannot separate from transport: a 1s round trip is either 1s on
// the wire or 1 call behind 20 others, and those need different fixes.
let worldModelInFlight = 0;
let worldModelPeak = 0;

export async function worldModel(path: string): Promise<WorldModel> {
  return decodeModel(await worldModelBuffer(path), path);
}

/**
 * The raw envelope for a model, as the backend sent it. Split from decoding so
 * the persistent cache can keep the bytes and skip this call next session (#7).
 */
export async function worldModelBuffer(path: string): Promise<ArrayBuffer> {
  worldModelInFlight += 1;
  worldModelPeak = Math.max(worldModelPeak, worldModelInFlight);
  perf.gauge("worldModel in flight", worldModelInFlight);
  perf.gauge("worldModel in flight (peak)", worldModelPeak);
  let buffer: ArrayBuffer;
  try {
    const sent = performance.now();
    buffer = await perf.measureAsync(
      "worldModel ipc",
      () => invoke<ArrayBuffer>("world_model", { path }),
      (b) => b.byteLength,
    );
    // The same round trip again, split by response size. Latency that tracks
    // bytes says to send less per model; latency that does not says the
    // transport itself is the cost.
    const bytes = buffer.byteLength;
    const bucket = bytes < 256 * 1024 ? "<256 KB" : bytes < 1024 * 1024 ? "256 KB-1 MB" : ">1 MB";
    perf.record(`worldModel ipc ${bucket}`, performance.now() - sent, bytes);
  } finally {
    worldModelInFlight -= 1;
    perf.gauge("worldModel in flight", worldModelInFlight);
  }
  return buffer;
}

/**
 * Decode a model envelope into typed-array views over it. Throws on a buffer
 * that is not a model envelope, which is how a persisted record from another
 * build of the wire format is told apart from a good one.
 */
export function decodeModel(buffer: ArrayBuffer, path: string): WorldModel {
  return decodeWorldModel(buffer, 0, buffer.byteLength, path);
}

/**
 * Decode a compiled model's envelope into a `WorldModel`.
 *
 * `byteOffset` and `byteLength` pick the envelope out of a larger buffer: a cell
 * reply carries each model as a nested envelope, byte for byte what `world_model`
 * returns, and decodes it here without copying a section.
 */
export function decodeWorldModel(
  buffer: ArrayBuffer,
  byteOffset: number,
  byteLength: number,
  path: string,
): WorldModel {
  const started = performance.now();
  const envelope = readEnvelope(buffer, byteOffset, byteLength);
  if (envelope.kind !== Kind.Model) {
    throw new Error(`${path} came back as envelope kind ${envelope.kind}, not a model`);
  }
  const meta = envelope.meta as {
    readonly path: string;
    readonly emptyDrawables: number;
    readonly textures: readonly WireTexture[];
    readonly drawables: readonly (Omit<WorldDrawable, "lods"> & {
      readonly lods: readonly (Omit<WorldBundle, "parts"> & {
        readonly parts: readonly MetaPart[];
      })[];
    })[];
  };

  const floats = (index: number | null): Float32Array | null => {
    if (index === null) return null;
    const section = envelope.section(index);
    return section ? asFloat32(section) : null;
  };

  const decoded: WorldModel = {
    path: meta.path,
    byteLength,
    emptyDrawables: meta.emptyDrawables,
    drawables: meta.drawables.map((drawable) => ({
      ...drawable,
      lods: drawable.lods.map((bundle) => ({
        ...bundle,
        parts: bundle.parts.map((part): WorldPart => {
          const indices = envelope.section(part.indices);
          return {
            ...part,
            // A part with no positions cannot be drawn; an empty view is what
            // the caller already skips, rather than a throw from here.
            positions: floats(part.positions) ?? new Float32Array(0),
            normals: floats(part.normals),
            uvs: floats(part.uvs),
            indices: !indices
              ? new Uint16Array(0)
              : part.indexBits === 16
                ? asUint16(indices)
                : asUint32(indices),
          };
        }),
      })),
    })),
    textures: meta.textures.map((texture) => {
      const section = envelope.section(texture.section);
      return {
        ...texture,
        levels: section ? splitMips(section, texture.mips.map((mip) => mip.byteLength)) : [],
      };
    }),
  };
  // Not `measure`, because the decode straddles an object literal rather than
  // sitting inside a closure. Same span, taken by hand.
  perf.record("worldModel decode", performance.now() - started);
  return decoded;
}

/** The machine's total and available memory in bytes, for the model budget. */
export function systemMemory(): Promise<{ total: number; available: number }> {
  return invoke("system_memory");
}

/** Save a world project payload to disk. Returns the byte count written. */
export function projectSave(path: string, contents: string): Promise<number> {
  return invoke<number>("project_save", { path, contents });
}

/** Read a world project payload back from disk. */
export function projectLoad(path: string): Promise<string> {
  return invoke<string>("project_load", { path });
}

/**
 * A `.cwproj` file: the placed maps by archive path, plus every transform edit
 * keyed by entity. The format is this app's own; CodeWalker's `.cwproj` is an
 * editor session file with no stable on-disk shape to copy.
 */
export type ProjectFile = {
  readonly format: "astramapper";
  readonly version: 1;
  readonly maps: readonly string[];
  readonly edits: Readonly<Record<string, TransformEdit>>;
};

/** Serialize a world's placed maps and edits as a `.cwproj` document. */
export function serializeProject(
  maps: readonly string[],
  edits: ReadonlyMap<string, TransformEdit>,
): string {
  const project: ProjectFile = {
    format: "astramapper",
    version: 1,
    maps,
    edits: Object.fromEntries(edits),
  };
  return JSON.stringify(project, null, 2);
}

/** A parsed `.cwproj`, ready to be placed back into the world. */
export type ParsedProject = {
  readonly maps: readonly string[];
  readonly edits: readonly { key: string; edit: TransformEdit }[];
};

/** Parse and validate a `.cwproj` document. Throws on anything unexpected. */
export function parseProject(text: string): ParsedProject {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new Error("not valid JSON");
  }
  if (typeof raw !== "object" || raw === null) {
    throw new Error("not a project file");
  }
  const project = raw as Partial<ProjectFile>;
  if (project.format !== "astramapper") {
    throw new Error("not an AstraMapper project");
  }
  if (project.version !== 1) {
    throw new Error(`unsupported project version ${project.version}`);
  }
  const maps = Array.isArray(project.maps)
    ? project.maps.filter((m): m is string => typeof m === "string")
    : [];
  const edits = Object.entries(project.edits ?? {})
    .filter((pair): pair is [string, TransformEdit] => isValidEdit(pair[1]))
    .map(([key, edit]) => ({ key, edit }));
  return { maps, edits };
}

function isValidEdit(value: unknown): value is TransformEdit {
  if (typeof value !== "object" || value === null) return false;
  const edit = value as TransformEdit;
  return (
    Array.isArray(edit.position) &&
    edit.position.length === 3 &&
    Array.isArray(edit.rotation) &&
    edit.rotation.length === 4 &&
    typeof edit.scaleXy === "number" &&
    typeof edit.scaleZ === "number"
  );
}

/**
 * A placed entity that has real geometry loaded in the world.
 *
 * `model` is the asset's parsed mesh bundles (all LODs); the renderer picks
 * which LOD shows by the distance the entity is from the camera.
 */
export type GeometryLoad = {
  readonly mapKey: string;
  readonly entity: Placed;
  readonly modelPath: string;
  readonly model: WorldModel;
};

/** An edited transform, overriding a placement's stored one. */
export type TransformEdit = {
  readonly position: readonly [number, number, number];
  readonly rotation: readonly [number, number, number, number];
  readonly scaleXy: number;
  readonly scaleZ: number;
};

/** One undo step: the transform of one entity before and after a drag. */
export type HistoryEntry = {
  readonly key: string;
  readonly before: TransformEdit;
  readonly after: TransformEdit;
};

/**
 * A placement with an edit applied on top. The edit replaces the stored
 * transform wholesale, so it is the source of truth while a drag is live and
 * the placement file itself stays untouched.
 */
export function applyEdit(placement: Placed, edit: TransformEdit | undefined): Placed {
  if (!edit) return placement;
  return {
    ...placement,
    position: edit.position,
    rotation: edit.rotation,
    scaleXy: edit.scaleXy,
    scaleZ: edit.scaleZ,
  };
}

/** A placement's transform as an edit, for history before/after values. */
export function editOf(placement: Placed): TransformEdit {
  return {
    position: placement.position,
    rotation: placement.rotation,
    scaleXy: placement.scaleXy,
    scaleZ: placement.scaleZ,
  };
}

/**
 * Rotate a stored quaternion around an axis, in the game's Z-up space.
 *
 * `axis` is a unit vector in GTA space; the result is the input quaternion
 * rotated by `angle` radians about that axis in world (parent) space, which is
 * what a gizmo ring drag should feel like. Computed with the Hamilton product
 * so the helper stays free of any three.js dependency.
 */
export function axisRotate(
  quaternion: readonly [number, number, number, number],
  axis: readonly [number, number, number],
  angle: number,
): [number, number, number, number] {
  const half = angle / 2;
  const s = Math.sin(half);
  const [ax, ay, az, aw] = quaternion;
  // The delta quaternion for `angle` about `axis`.
  const [dx, dy, dz, dw] = [axis[0] * s, axis[1] * s, axis[2] * s, Math.cos(half)];
  return [
    dw * ax + dx * aw + dy * az - dz * ay,
    dw * ay - dx * az + dy * aw + dz * ax,
    dw * az + dx * ay - dy * ax + dz * aw,
    dw * aw - dx * ax - dy * ay - dz * az,
  ];
}

/** Record a completed drag, clearing the redo stack the way editors do. */
export function commitHistory(
  undo: readonly HistoryEntry[],
  entry: HistoryEntry,
): readonly HistoryEntry[] {
  return [...undo, entry];
}

/** Step back one edit. Returns the popped entry, or `null` at the bottom. */
export function popHistory(
  stack: readonly HistoryEntry[],
): { stack: readonly HistoryEntry[]; entry: HistoryEntry | null } {
  if (stack.length === 0) return { stack, entry: null };
  return { stack: stack.slice(0, -1), entry: stack[stack.length - 1] };
}

/** The stable identity of one placed entity, across maps and reloads. */
export function keyFor(mapKey: string, entityIndex: number): string {
  return `${mapKey}:${entityIndex}`;
}

/**
 * The inverse of `keyFor`, or `null` for anything that is not one of its keys.
 *
 * Split on the **last** separator, not the first: a map key is an archive path
 * and carries colons of its own (`x64a.rpf\levels\...`, and a Windows drive
 * letter for a map placed from disk). The entity index is a plain integer, so
 * the tail after the last colon is unambiguous.
 *
 * This exists so a key can be resolved without keeping a reverse map of every
 * placement — with the base game placed that map held around 693,000 keys, all
 * to serve the handful of entities that actually have an edit.
 */
export function parseKey(key: string): { mapKey: string; entityIndex: number } | null {
  const at = key.lastIndexOf(":");
  if (at <= 0 || at === key.length - 1) return null;
  const tail = key.slice(at + 1);
  // `Number` would take " 12", "0x0" and "1e3"; an entity index is decimal
  // digits and nothing else, because that is all `keyFor` can ever produce.
  if (!/^\d+$/.test(tail)) return null;
  return { mapKey: key.slice(0, at), entityIndex: Number(tail) };
}

/**
 * A world-space point back into the game's Z-up space.
 *
 * The inverse of `gtaToWorld`: the renderer's `-PI / 2` X rotation maps GTA
 * `(x, y, z)` to world `(x, z, -y)`, so the way back is `(x, -z, y)`.
 */
export function worldToGta(p: readonly [number, number, number]): [number, number, number] {
  return [p[0], -p[2], p[1]];
}

/**
 * Which LOD bundle a model should show at a given camera distance.
 *
 * The entity's own `lodDist` is the boundary the game uses to switch HD
 * geometry off; this maps it onto the drawable's High→VeryLow bundles. A
 * degenerate or absent `lodDist` falls back to fixed metre bands. The result
 * is clamped to the number of bundles the model actually has.
 */
export function lodBandForDistance(
  distance: number,
  lodDist: number,
  availableCount: number,
): number {
  const near = lodDist > 0 ? lodDist : 100;
  const mid = lodDist > 0 ? lodDist * 2 : 400;
  const far = lodDist > 0 ? lodDist * 4 : 1000;
  const band = distance <= near ? 0 : distance <= mid ? 1 : distance <= far ? 2 : 3;
  return Math.min(band, Math.max(availableCount - 1, 0));
}

/**
 * The nearest unloaded entities for the streamer, per-map load regions.
 *
 * Returns up to `limit` candidates within `radius` of the camera (all in the
 * game's Z-up space, where rotation preserves distance) whose keys are not in
 * `excluded`, nearest first.
 */
export function nearestCandidates(
  cameraGta: readonly [number, number, number],
  entries: readonly { key: string; placements: PlacementColumns }[],
  excluded: ReadonlySet<string>,
  radius: number,
  limit: number,
): readonly { mapKey: string; entityIndex: number; distance: number }[] {
  const candidates: { mapKey: string; entityIndex: number; distance: number }[] = [];
  for (const entry of entries) {
    const cols = entry.placements;
    for (let row = 0; row < cols.length; row += 1) {
      const entityIndex = cols.entityIndex(row);
      const key = keyFor(entry.key, entityIndex);
      if (excluded.has(key)) continue;
      const dx = cols.x(row) - cameraGta[0];
      const dy = cols.y(row) - cameraGta[1];
      const dz = cols.z(row) - cameraGta[2];
      const distance = Math.hypot(dx, dy, dz);
      if (distance <= radius) {
        candidates.push({ mapKey: entry.key, entityIndex, distance });
      }
    }
  }
  candidates.sort((a, b) => a.distance - b.distance);
  return candidates.slice(0, limit);
}

/** A placed map as the scheduler sees it: its streaming box and its entities. */
export type RegionEntry = {
  readonly key: string;
  readonly extentsMin: readonly [number, number, number];
  readonly extentsMax: readonly [number, number, number];
  readonly placements: PlacementColumns;
};

/** How far out an entity's geometry streams, by its placement LOD tier.
 *  The factors are generous so a whole neighbourhood fills in around the
 *  camera, not just a handful of nearby models — the LOD swap keeps the far
 *  ones cheap. */
export function entityStreamRadius(entity: Placed): number {
  return streamRadiusOf(entity.lodDist, entity.lodLevel);
}

/** `entityStreamRadius` from the two fields it reads, for a scan over columns. */
export function streamRadiusOf(lodDist: number, lodLevel: string): number {
  const base = Math.max(lodDist, 50);
  switch (lodLevel) {
    case "ORPHAN_HD":
      return base * 4;
    case "HD":
      return base * 7;
    case "LOD":
      return base * 14;
    default:
      return base * 20;
  }
}

/**
 * Where the camera is looking, for ordering a streaming pass by what is on
 * screen. Optional: without it the pass orders by distance alone.
 */
export type StreamView = {
  /** The camera's forward direction in the game's Z-up space. Unit length. */
  readonly forward: readonly [number, number, number];
  /**
   * Cosine of the half-angle of the cone standing in for the view frustum.
   *
   * A cone rather than the six real frustum planes, deliberately. This decides
   * *order*, not visibility, so the cost of being generous is that something
   * just off screen loads a little sooner than it had to — while the cost of
   * being exact would be coupling a pure function to the projection matrix and
   * the aspect ratio, and a set that churns every time the window is resized.
   */
  readonly cosHalfAngle: number;
};

/**
 * The cone that stands in for the frustum: 60 degrees off the view axis.
 *
 * The camera is 60 degrees vertical, so at 16:9 its frustum corner sits about
 * 50 degrees off axis. Sixty covers that with margin, which is what keeps a
 * small turn of the head from reshuffling the queue.
 */
export const STREAM_VIEW_COS_HALF_ANGLE = Math.cos((60 * Math.PI) / 180);

/**
 * Inside this distance an entity counts as on screen whatever the angle says.
 *
 * A cone has nothing at its apex: without this, the models closest to the
 * camera — the ones at its feet, the ones it is standing among — score as
 * behind it and load last.
 */
export const STREAM_VIEW_NEAR = 60;

/**
 * How much of its tier radius an entity outside the view cone may load from.
 *
 * Out of view is not out of the sphere (turning must not find an empty world),
 * but it is a lot less urgent: past this fraction of the radius, what is behind
 * or beside the camera is not a candidate until it comes into view. Unloading
 * keeps the full radius (x1.5), so a loaded entity that falls out of view stays
 * put: the gap between 0.5 and 1.5 is the hysteresis.
 */
export const STREAM_OFF_VIEW_RADIUS = 0.5;

/**
 * Whether something `distance` away, at offset `(dx, dy, dz)`, counts as on
 * screen: inside the near radius, or inside the view cone. No view means
 * everything is.
 */
export function inStreamView(
  dx: number,
  dy: number,
  dz: number,
  distance: number,
  view?: StreamView | null,
): boolean {
  if (!view || distance <= STREAM_VIEW_NEAR) return true;
  return (
    (dx * view.forward[0] + dy * view.forward[1] + dz * view.forward[2]) /
      Math.max(distance, 1e-6) >=
    view.cosHalfAngle
  );
}

/** One entity the pass decided to load. */
export type StreamCandidate = {
  readonly mapKey: string;
  readonly entityIndex: number;
  readonly distance: number;
  /** Whether it fell inside the view cone. Ordering puts these first. */
  readonly inView: boolean;
  /** What ordering actually sorts by, lower first: distance, plus the phase
   *  penalty, plus a wall for anything outside the view cone. */
  readonly rank: number;
};

/**
 * The streaming decision for one pass: what to load, and what to unload.
 *
 * A map streams only while the camera is inside its declared streaming box
 * (plus a margin); out of range, its loaded geometry is dropped. In range,
 * the nearest unloaded entities within their own LOD-tier radius load, up to
 * `batchLimit`. Loaded entities unload once they drift beyond their radius
 * with hysteresis so a camera parked on a boundary does not thrash.
 *
 * # What `view` changes, and what it deliberately does not
 *
 * Given a view, the batch is filled with what is **on screen** first and only
 * then with what is behind the camera, and what is off screen is only a
 * candidate inside `STREAM_OFF_VIEW_RADIUS` of its tier radius. The unload rule
 * does not look at the view at all: it stays the full sphere (x1.5).
 *
 * That split is the design. Culling the candidates to the frustum outright
 * would mean turning around and finding nothing there, then a reload storm as
 * the same geometry streams back in. A smaller sphere for what is off screen
 * keeps the near side of it resident and spends no IPC or memory on the far
 * side until it is looked at, and because loading starts at 0.5 of the radius
 * and unloading at 1.5, an entity that drifts in and out of the cone never
 * thrashes. Ordering still does most of the work: with a batch of 24 against a
 * sphere holding thousands, order is what the user experiences as load speed.
 *
 * # Why this is not a collect-then-sort
 *
 * It used to be. With the base game placed, a pass pushed every one of ~46,000
 * in-radius placements into an array, sorted it, and took the first 24 —
 * measured at **76ms, four times a second, on the main thread**. It also built
 * a key string per placement per pass, 763,102 of them, before deciding it
 * wanted none. Now the top `batchLimit` are kept by insertion as the scan runs,
 * and the loaded and pending sets are regrouped once per pass by map so the
 * inner loop compares integers instead of allocating strings.
 */
export function streamingDecision(
  cameraGta: readonly [number, number, number],
  entries: readonly RegionEntry[],
  loadedKeys: ReadonlySet<string>,
  pendingKeys: ReadonlySet<string>,
  batchLimit: number,
  view?: StreamView | null,
): { load: readonly StreamCandidate[]; unload: readonly string[] } {
  const unload: string[] = [];
  if (batchLimit <= 0 && loadedKeys.size === 0) return { load: [], unload };

  const loadedByMap = groupKeysByMap(loadedKeys);
  const pendingByMap = groupKeysByMap(pendingKeys);

  // Kept sorted best-first and never longer than the batch, so the scan pays
  // one comparison against the current worst for the overwhelming majority of
  // placements and an insert for almost none.
  const best: StreamCandidate[] = [];

  // The ground is a layer of its own: every terrain placement of every placed
  // map is wanted whatever the camera or the map's box says, and none unloads
  // (#5). It is scanned here, over each map's precomputed terrain rows, and the
  // per-entity loop below steps over those rows.
  for (const entry of entries) {
    const cols = entry.placements;
    const terrain = terrainRowsOf(cols);
    if (terrain.length === 0) continue;
    const loadedHere = loadedByMap.get(entry.key);
    const pendingHere = pendingByMap.get(entry.key);
    for (const row of terrain) {
      const entityIndex = cols.entityIndex(row);
      if (loadedHere?.has(entityIndex) || pendingHere?.has(entityIndex)) continue;
      const dx = cols.x(row) - cameraGta[0];
      const dy = cols.y(row) - cameraGta[1];
      const dz = cols.z(row) - cameraGta[2];
      const distance = Math.sqrt(dx * dx + dy * dy + dz * dz);
      if (
        best.length >= batchLimit &&
        batchLimit > 0 &&
        distance >= best[best.length - 1]!.rank
      ) {
        continue;
      }
      const inView = inStreamView(dx, dy, dz, distance, view);
      const rank = (inView ? distance : STREAM_OFF_VIEW_PENALTY + distance) + STREAM_PHASE_PENALTY.terrain;
      insertCandidate(
        best,
        { mapKey: entry.key, entityIndex, distance, inView, rank },
        batchLimit,
      );
    }
  }

  for (const entry of entries) {
    const loadedHere = loadedByMap.get(entry.key);
    const pendingHere = pendingByMap.get(entry.key);

    // The map's footprint decides in-range, not its height: a camera overhead
    // still streams what is below it, and the per-entity radius governs how
    // far out geometry actually loads.
    const inRange =
      cameraGta[0] >= entry.extentsMin[0] - STREAM_REGION_MARGIN &&
      cameraGta[0] <= entry.extentsMax[0] + STREAM_REGION_MARGIN &&
      cameraGta[1] >= entry.extentsMin[1] - STREAM_REGION_MARGIN &&
      cameraGta[1] <= entry.extentsMax[1] + STREAM_REGION_MARGIN;

    // Out of range the whole map goes, and it goes without touching a single
    // placement: what is loaded is already the answer.
    const cols = entry.placements;
    const terrain = terrainRowsOf(cols);
    if (!inRange) {
      if (loadedHere) {
        for (const index of loadedHere) {
          // The ground stays resident when its map goes out of range.
          if (terrain.length > 0 && isTerrainRow(terrain, cols.rowOf(index))) continue;
          unload.push(keyFor(entry.key, index));
        }
      }
      continue;
    }

    let nextTerrain = 0;
    for (let row = 0; row < cols.length; row += 1) {
      if (nextTerrain < terrain.length && terrain[nextTerrain] === row) {
        nextTerrain += 1;
        continue;
      }
      const entityIndex = cols.entityIndex(row);
      const dx = cols.x(row) - cameraGta[0];
      const dy = cols.y(row) - cameraGta[1];
      const dz = cols.z(row) - cameraGta[2];
      const distanceSq = dx * dx + dy * dy + dz * dz;
      const lodLevel = cols.lodLevel(row);
      const radius = streamRadiusOf(cols.lodDist(row), lodLevel);

      if (loadedHere?.has(entityIndex)) {
        const drop = radius * 1.5;
        if (distanceSq > drop * drop) unload.push(keyFor(entry.key, entityIndex));
        continue;
      }
      if (distanceSq > radius * radius) continue;
      if (pendingHere?.has(entityIndex)) continue;

      const distance = Math.sqrt(distanceSq);
      const penalty = streamPenaltyOf(cols.name(row), lodLevel, cols.archetypeClass(row));
      // The view only ever adds to the rank, so distance plus penalty is a floor
      // on it: anything whose floor already loses to the worst candidate kept
      // cannot win, and skips the cone test.
      if (
        best.length >= batchLimit &&
        batchLimit > 0 &&
        distance + penalty >= best[best.length - 1]!.rank
      ) {
        continue;
      }
      const inView = inStreamView(dx, dy, dz, distance, view);
      // Off view, only the inner part of the sphere is worth loading now.
      if (!inView && distance > radius * STREAM_OFF_VIEW_RADIUS) continue;
      const rank = (inView ? distance : STREAM_OFF_VIEW_PENALTY + distance) + penalty;
      insertCandidate(
        best,
        { mapKey: entry.key, entityIndex, distance, inView, rank },
        batchLimit,
      );
    }
  }

  return { load: best, unload };
}

/** Added to the rank of anything outside the view cone, so it sorts after
 *  everything inside it. Far beyond any streaming radius. */
const STREAM_OFF_VIEW_PENALTY = 1_000_000;

/**
 * The same order `streamingDecision` fills a batch in — on screen first, then
 * nearest after the phase penalty — as one number, lower first. For ranking
 * requests that are already waiting, which the decision itself never sees again.
 */
export function streamRank(
  cameraGta: readonly [number, number, number],
  entity: Placed,
  view?: StreamView | null,
): number {
  const dx = entity.position[0] - cameraGta[0];
  const dy = entity.position[1] - cameraGta[1];
  const dz = entity.position[2] - cameraGta[2];
  const distance = Math.sqrt(dx * dx + dy * dy + dz * dz);
  const inView = inStreamView(dx, dy, dz, distance, view);
  return (inView ? distance : STREAM_OFF_VIEW_PENALTY + distance) + streamPenalty(entity);
}

/** `streamRank` for a candidate the decision already measured. */
export function candidateRank(candidate: StreamCandidate): number {
  return candidate.rank;
}

/**
 * Gap between load phases in the rank. Far above any distance or the off-view
 * wall (1e6), so a phase is a strict tier: nothing in a later one is ordered
 * ahead of anything in an earlier one, whatever the distances.
 */
const STREAM_PHASE_STRIDE = 10_000_000;

/**
 * Load phases, in order: every terrain piece in range, then structures at full
 * detail, decals, vegetation, and last the coarse LOD tiers. Within a phase the
 * order is on-screen first, then nearest.
 *
 * Strict on purpose: with structures streaming in before the ground under them
 * the world reads as buildings floating over a void. The cost is that a later
 * phase waits for every earlier one in range, so a misclassified name can only
 * delay an entity, never drop it.
 */
export const STREAM_PHASE_PENALTY = {
  terrain: 0,
  structure: STREAM_PHASE_STRIDE,
  decal: 2 * STREAM_PHASE_STRIDE,
  vegetation: 3 * STREAM_PHASE_STRIDE,
  farLod: 4 * STREAM_PHASE_STRIDE,
} as const;

const TERRAIN_TOKENS = new Set(["terrain", "ter", "terr", "grnd"]);
const DECAL_TOKENS = new Set(["decal", "decals", "dcl"]);
const VEGETATION_TOKENS = new Set([
  "tree", "trees", "bush", "bushes", "palm", "plant", "plants", "veg", "vegetation",
  "grass", "cactus", "fern", "shrub", "hedge", "weed", "weeds", "flower", "flowers",
  "reed", "reeds", "vine", "ivy", "foliage", "yucca", "agave", "sapling",
]);

/** Penalty by archetype name, or -1 when the name says nothing and the entity's
 *  LOD tier decides. Names repeat across thousands of placements, so each is
 *  classified once. */
const namePenalty = new Map<string, number>();

function classifyName(name: string): number {
  const tokens = name.toLowerCase().split(/[^a-z]+/);
  let found = -1;
  for (const token of tokens) {
    if (TERRAIN_TOKENS.has(token)) return STREAM_PHASE_PENALTY.terrain;
    if (DECAL_TOKENS.has(token)) found = STREAM_PHASE_PENALTY.decal;
    else if (VEGETATION_TOKENS.has(token) && found < 0) found = STREAM_PHASE_PENALTY.vegetation;
  }
  return found;
}

/** Per map: the rows the terrain phase claims, ascending. A map's columns never
 *  change, and the scan runs four times a second, so it is classified once. */
const terrainRows = new WeakMap<PlacementColumns, Uint32Array>();

/** The ascending rows of `cols` that are terrain, classified once per map. */
export function terrainRowsOf(cols: PlacementColumns): Uint32Array {
  let rows = terrainRows.get(cols);
  if (!rows) {
    const found: number[] = [];
    for (let row = 0; row < cols.length; row += 1) {
      if (
        streamPenaltyOf(cols.name(row), cols.lodLevel(row), cols.archetypeClass(row)) ===
        STREAM_PHASE_PENALTY.terrain
      ) {
        found.push(row);
      }
    }
    rows = Uint32Array.from(found);
    terrainRows.set(cols, rows);
  }
  return rows;
}

/** Whether `row` is one of the ascending `terrain` rows. */
function isTerrainRow(terrain: Uint32Array, row: number): boolean {
  let low = 0;
  let high = terrain.length - 1;
  while (low <= high) {
    const mid = (low + high) >>> 1;
    const value = terrain[mid]!;
    if (value === row) return true;
    if (value < row) low = mid + 1;
    else high = mid - 1;
  }
  return false;
}

/**
 * The phase penalty for one entity. Terrain is decided by name, because no
 * archetype data identifies ground safely (see `cw-core` `filetypes/class.rs`).
 * Decals and vegetation are decided by the archetype class the backend read from
 * the `.ytyp` flags and bounding box; where the data says nothing, by name
 * tokens. Otherwise by LOD tier: HD structures are the structure phase, the
 * coarse LOD and SLOD tiers go last. A misclassification only loads its entity
 * in the wrong order; nothing is dropped.
 */
export function streamPenalty(entity: Placed): number {
  return streamPenaltyOf(entity.name, entity.lodLevel, entity.archetypeClass);
}

/** `streamPenalty` from the fields it reads, for a scan over columns. */
export function streamPenaltyOf(name: string, lodLevel: string, archetypeClass = 0): number {
  let byName = namePenalty.get(name);
  if (byName === undefined) {
    byName = classifyName(name);
    namePenalty.set(name, byName);
  }
  if (byName === STREAM_PHASE_PENALTY.terrain) return byName;
  if (archetypeClass === ARCHETYPE_CLASS.decal) return STREAM_PHASE_PENALTY.decal;
  if (archetypeClass === ARCHETYPE_CLASS.vegetation) return STREAM_PHASE_PENALTY.vegetation;
  if (byName >= 0) return byName;
  return lodLevel === "ORPHAN_HD" || lodLevel === "HD"
    ? STREAM_PHASE_PENALTY.structure
    : STREAM_PHASE_PENALTY.farLod;
}

/**
 * Whether `streamingDecision` would still ask for this placement from this
 * camera: its map is in range and it is inside its own LOD-tier radius (the
 * reduced one when it is off view). The test a request waiting in a queue has
 * to keep passing to stay there.
 */
export function stillStreamWanted(
  cameraGta: readonly [number, number, number],
  entry: Pick<RegionEntry, "extentsMin" | "extentsMax">,
  placement: Placed,
  view?: StreamView | null,
): boolean {
  // The ground is wanted everywhere (#5), so a waiting terrain request never
  // goes stale however far the camera has moved.
  if (streamPenalty(placement) === STREAM_PHASE_PENALTY.terrain) return true;
  const inRange =
    cameraGta[0] >= entry.extentsMin[0] - STREAM_REGION_MARGIN &&
    cameraGta[0] <= entry.extentsMax[0] + STREAM_REGION_MARGIN &&
    cameraGta[1] >= entry.extentsMin[1] - STREAM_REGION_MARGIN &&
    cameraGta[1] <= entry.extentsMax[1] + STREAM_REGION_MARGIN;
  if (!inRange) return false;
  const dx = placement.position[0] - cameraGta[0];
  const dy = placement.position[1] - cameraGta[1];
  const dz = placement.position[2] - cameraGta[2];
  const radius = entityStreamRadius(placement);
  const distanceSq = dx * dx + dy * dy + dz * dz;
  if (distanceSq > radius * radius) return false;
  // The same off-view cut as the decision, so a request for something the
  // camera turned away from does not sit in the queue.
  if (!view) return true;
  const distance = Math.sqrt(distanceSq);
  return (
    inStreamView(dx, dy, dz, distance, view) || distance <= radius * STREAM_OFF_VIEW_RADIUS
  );
}

/**
 * `mapKey` → the entity indexes it holds, from a set of `keyFor` keys.
 *
 * Done once per pass so the scan can ask an integer question. The sets are the
 * loaded and pending geometry — thousands at most against three quarters of a
 * million placements, so this is the cheap side of the trade by three orders of
 * magnitude.
 */
function groupKeysByMap(keys: ReadonlySet<string>): Map<string, Set<number>> {
  const byMap = new Map<string, Set<number>>();
  for (const key of keys) {
    const parsed = parseKey(key);
    if (!parsed) continue;
    const set = byMap.get(parsed.mapKey);
    if (set) set.add(parsed.entityIndex);
    else byMap.set(parsed.mapKey, new Set([parsed.entityIndex]));
  }
  return byMap;
}

/** Lower rank first: on screen before off screen, then phase-adjusted distance. */
function betterCandidate(a: StreamCandidate, b: StreamCandidate): boolean {
  return a.rank < b.rank;
}

/** Insert into a best-first list capped at `limit`, dropping the worst. */
function insertCandidate(
  best: StreamCandidate[],
  candidate: StreamCandidate,
  limit: number,
): void {
  let at = best.length;
  while (at > 0 && betterCandidate(candidate, best[at - 1]!)) at -= 1;
  if (at >= limit) return;
  best.splice(at, 0, candidate);
  if (best.length > limit) best.length = limit;
}

/** The margin around a map's streaming box in which it still streams. */
export const STREAM_REGION_MARGIN = 300;

/**
 * The three.js scale for one placement.
 *
 * GTA V is Z-up and three.js is Y-up; the world renderer rotates the whole
 * world group `-PI / 2` about X to reconcile them, which maps a GTA axis
 * `(x, y, z)` onto world `(x, z, -y)`. The game's two scale values — uniform
 * in the ground plane, separate on Z — land on the world axes accordingly.
 */
export function placementScale(scaleXy: number, scaleZ: number): [number, number, number] {
  return [scaleXy, scaleZ, scaleXy];
}

/**
 * A GTA-space point as the world camera should see it.
 *
 * The world renderer's parent group rotates `-PI / 2` about X to put the
 * Z-up game into three.js's Y-up world, mapping `(x, y, z)` onto `(x, z, -y)`.
 * A point that lives inside that group must be mapped the same way when the
 * camera needs its position in world space.
 */
export function gtaToWorld(p: readonly [number, number, number]): [number, number, number] {
  return [p[0], p[2], -p[1]];
}

/**
 * The colour a placed marker should be, by its LOD level. MLO interior
 * instances get their own hue so they read apart from ordinary geometry at a
 * glance, and HD/LOD/SLOD step the brightness down with distance.
 */
export function colorForLod(lodLevel: string, isMlo: boolean): number {
  if (isMlo) return 0xe0a458;
  switch (lodLevel) {
    case "HD":
      return 0x7aa2f7;
    case "LOD":
      return 0x9db0c9;
    case "ORPHAN_HD":
      return 0x4caf7d;
    case "SLOD1":
    case "SLOD2":
    case "SLOD3":
    case "SLOD4":
      return 0x5c6b83;
    default:
      return 0x9aa6b4;
  }
}

/** `boundsOf` over placed maps' columns, without materializing a placement. */
export function boundsOfColumns(
  sets: readonly PlacementColumns[],
): ReturnType<typeof boundsOf> {
  let minX = Infinity;
  let minY = Infinity;
  let minZ = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  let maxZ = -Infinity;
  let any = false;
  for (const cols of sets) {
    for (let row = 0; row < cols.length; row += 1) {
      any = true;
      const x = cols.x(row);
      const y = cols.y(row);
      const z = cols.z(row);
      if (x < minX) minX = x;
      if (y < minY) minY = y;
      if (z < minZ) minZ = z;
      if (x > maxX) maxX = x;
      if (y > maxY) maxY = y;
      if (z > maxZ) maxZ = z;
    }
  }
  if (!any) return { min: [0, 0, 0], max: [0, 0, 0], center: [0, 0, 0], radius: 1 };
  return finishBounds(minX, minY, minZ, maxX, maxY, maxZ);
}

/** Axis-aligned bounds of a set of placed entities, for framing the camera. */
export function boundsOf(
  placements: readonly { position: readonly [number, number, number] }[],
): { min: [number, number, number]; max: [number, number, number]; center: [number, number, number]; radius: number } {
  if (placements.length === 0) {
    return { min: [0, 0, 0], max: [0, 0, 0], center: [0, 0, 0], radius: 1 };
  }
  let minX = Infinity;
  let minY = Infinity;
  let minZ = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  let maxZ = -Infinity;
  for (const { position } of placements) {
    minX = Math.min(minX, position[0]);
    minY = Math.min(minY, position[1]);
    minZ = Math.min(minZ, position[2]);
    maxX = Math.max(maxX, position[0]);
    maxY = Math.max(maxY, position[1]);
    maxZ = Math.max(maxZ, position[2]);
  }
  return finishBounds(minX, minY, minZ, maxX, maxY, maxZ);
}

function finishBounds(
  minX: number,
  minY: number,
  minZ: number,
  maxX: number,
  maxY: number,
  maxZ: number,
): ReturnType<typeof boundsOf> {
  const min: [number, number, number] = [minX, minY, minZ];
  const max: [number, number, number] = [maxX, maxY, maxZ];
  const center: [number, number, number] = [
    (minX + maxX) / 2,
    (minY + maxY) / 2,
    (minZ + maxZ) / 2,
  ];
  const radius = Math.max(
    Math.hypot(maxX - minX, maxY - minY, maxZ - minZ) / 2,
    1,
  );
  return { min, max, center, radius };
}

/**
 * The world the view shows when no game folder is open: a deterministic plaza
 * with a ground slab, a ring of buildings, props and a couple of interior
 * markers. Same `Placed` shape as a real map, so the renderer, the colours and
 * the raycast all exercise the real path without the game.
 */
export function demoWorld(): { name: string; placements: Placed[] } {
  const placements: Placed[] = [];
  let index = 0;
  const push = (
    name: string,
    position: [number, number, number],
    scaleXy: number,
    scaleZ: number,
    lodLevel: string,
    isMlo = false,
  ): void => {
    placements.push({
      index: index++,
      name,
      nameHash: 0,
      isMlo,
      fromMlo: false,
      position,
      rotation: [0, 0, 0, 1],
      scaleXy,
      scaleZ,
      parentIndex: -1,
      lodLevel,
      lodDist: 100,
      children: 0,
    });
  };

  // The plaza floor, a little above the ground plane so it reads as a slab.
  push("plaza_ground", [0, 0, 0.1], 110, 0.2, "HD");

  // Buildings around the plaza edge: four to a side, varying height.
  const block = (x: number, y: number, size: number, height: number, name: string): void => {
    push(name, [x, y, height / 2], size, height, "HD");
    push(`${name}_lod`, [x, y, height / 2], size, height, "LOD");
  };
  for (let i = 0; i < 4; i += 1) {
    const x = -60 + i * 40;
    block(x, 55, 26, 16 + (i % 3) * 5, `north_block_${i}`);
    block(x, -55, 26, 14 + (i % 2) * 7, `south_block_${i}`);
  }
  for (let i = 0; i < 4; i += 1) {
    const y = -50 + i * 32;
    block(55, y, 20, 20 + (i % 2) * 6, `east_block_${i}`);
    block(-55, y, 20, 18 + (i % 3) * 4, `west_block_${i}`);
  }

  // A central pavilion that reads as an interior instance.
  push("central_interior", [0, 0, 5], 12, 10, "HD", true);

  // A few trees as plain green boxes.
  for (let i = 0; i < 6; i += 1) {
    const x = -40 + i * 16;
    push(`tree_${i}`, [x, 35, 2], 2, 4, "HD");
  }

  return { name: "demo_plaza", placements };
}
