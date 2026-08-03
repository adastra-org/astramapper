/**
 * Typed wrappers over the World Mode IPC commands in `src-tauri/src/world.rs`,
 * plus the pure geometry helpers the world renderer builds on.
 *
 * These types mirror the Rust structs exactly; the backend serializes camelCase.
 * The `invoke` wrappers need a Tauri host, so the tests cover the pure helpers
 * here (demo world, axis conversion, bounds, colours), not the wrappers.
 */
import { invoke } from "@tauri-apps/api/core";
import type { ModelPreview } from "./viewer";

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
};

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
  readonly entities: readonly Placed[];
};

export function worldStatus(): Promise<WorldStatus> {
  return invoke<WorldStatus>("world_status");
}

export function worldDlcYmaps(group: string): Promise<YmapListingPage> {
  return invoke<YmapListingPage>("world_dlc_ymaps", { group });
}

export function worldMap(path: string): Promise<WorldMap> {
  return invoke<WorldMap>("world_map", { path });
}

/** Resolve an archetype name to a model file, or `null` when there is none. */
export function worldAsset(name: string): Promise<string | null> {
  return invoke<string | null>("world_asset", { name });
}

/** Read a model file's geometry, every LOD, for the world view to draw. */
export function worldModel(path: string): Promise<ModelPreview> {
  return invoke<ModelPreview>("world_model", { path });
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
  readonly model: ModelPreview;
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
 * The unloaded entities nearest the camera, for the streamer.
 *
 * Returns up to `limit` candidates within `radius` of the camera (all in the
 * game's Z-up space, where rotation preserves distance) whose keys are not in
 * `excluded`, nearest first.
 */
export function nearestCandidates(
  cameraGta: readonly [number, number, number],
  entries: readonly { key: string; placements: readonly Placed[] }[],
  excluded: ReadonlySet<string>,
  radius: number,
  limit: number,
): readonly { mapKey: string; entityIndex: number; distance: number }[] {
  const candidates: { mapKey: string; entityIndex: number; distance: number }[] = [];
  for (const entry of entries) {
    for (const placement of entry.placements) {
      const key = keyFor(entry.key, placement.index);
      if (excluded.has(key)) continue;
      const dx = placement.position[0] - cameraGta[0];
      const dy = placement.position[1] - cameraGta[1];
      const dz = placement.position[2] - cameraGta[2];
      const distance = Math.hypot(dx, dy, dz);
      if (distance <= radius) {
        candidates.push({ mapKey: entry.key, entityIndex: placement.index, distance });
      }
    }
  }
  candidates.sort((a, b) => a.distance - b.distance);
  return candidates.slice(0, limit);
}

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
