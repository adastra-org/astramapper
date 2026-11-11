/**
 * Reading what the backend's world index sends: the partition, and batches of
 * cells.
 *
 * Both are binary envelopes (`src-tauri/src/world_index.rs` writes them); the
 * layout is documented there and the section indices are named once, below.
 *
 * # What arrives, and what is kept
 *
 * A *batch* is the reply to one `world_index_cells` call: the placements of a
 * few cells as columns, and the models those cells need as nested model
 * envelopes. Decoding it copies the placement columns out, one
 * `PlacementColumns` per (cell, map), so the reply buffer is not what keeps a
 * cell alive; the models stay views over the reply buffer, as they always were,
 * which is what `ModelCache` accounts for.
 */
import { asFloat32, asInt32, asUint16, asUint32, Kind, readEnvelope } from "../wire";
import { decodeWorldModel, type WorldModel } from "../world";
import { PlacementColumns, type ColumnSource } from "./columns";
import { hexCenter, hexKey } from "./hex";
import type { AdaptiveCell, AdaptiveGrid } from "./hexAdaptive";

/** A cell as the backend names it. */
export type CellRef = {
  readonly level: number;
  readonly q: number;
  readonly r: number;
};

/** The level number that names the terrain layer, which is not a hex cell. */
export const TERRAIN_LEVEL = 255;

/** The terrain layer as a cell reference. */
export const TERRAIN_CELL: CellRef = { level: TERRAIN_LEVEL, q: 0, r: 0 };

/** A string key for a cell: stable, and cheap enough to build per plan. */
export function cellKey(cell: CellRef): string {
  return `${cell.level}:${cell.q}:${cell.r}`;
}

export const TERRAIN_KEY = cellKey(TERRAIN_CELL);

/** Whether a cell is the terrain layer. */
export function isTerrain(cell: CellRef): boolean {
  return cell.level === TERRAIN_LEVEL;
}

// ---- The partition ------------------------------------------------------

/** The world cut into leaves, as the backend built it. */
export type CellGrid = {
  readonly group: string;
  /** Centre-to-corner sizes in metres, coarse to fine. */
  readonly sizes: readonly number[];
  readonly threshold: number;
  readonly maps: number;
  /** Maps that could not be read. */
  readonly failed: number;
  /** Every placement the index holds, terrain included. */
  readonly placements: number;
  /** Ground placements: held outside the partition, as one layer. */
  readonly terrain: number;
  /** Every leaf. Each non-terrain placement is counted in exactly one. */
  readonly leaves: readonly AdaptiveCell[];
  /** Cells that were split finer, so a leaf can tell "split" from "empty". */
  readonly splits: readonly CellRef[];
  /** Named zones: sets of leaves (`clusters.ts`). Empty from an older backend. */
  readonly clusters: readonly ClusterInfo[];
  /** The cluster of each leaf, an index into `clusters`, parallel to `leaves`. */
  readonly leafCluster: Uint16Array;
};

/** One zone cluster, as the backend counted it. */
export type ClusterInfo = {
  readonly key: string;
  readonly name: string;
  /** Leaves in it. */
  readonly leaves: number;
  /** Placements in them (terrain is in none). */
  readonly placements: number;
  /** Distinct archetype names across those placements. */
  readonly models: number;
};

type GridMeta = {
  readonly group: string;
  readonly sizes: readonly number[];
  readonly threshold: number;
  readonly maps: number;
  readonly failed: number;
  readonly placements: number;
  readonly terrain: number;
  readonly clusters?: readonly ClusterInfo[];
};

/** Partition envelope section indices. */
const GRID = {
  level: 0,
  q: 1,
  r: 2,
  count: 3,
  z: 4,
  splitLevel: 5,
  splitQ: 6,
  splitR: 7,
  cluster: 8,
} as const;

/** Decode the partition `world_index_build` returns. */
export function decodeCellGrid(buffer: ArrayBuffer): CellGrid {
  const envelope = readEnvelope(buffer);
  if (envelope.kind !== Kind.Cells) {
    throw new Error(`envelope kind ${envelope.kind} is not a cell partition`);
  }
  const meta = envelope.meta as GridMeta;
  const section = (which: number): Uint8Array => {
    const found = envelope.section(which);
    if (!found) throw new Error(`the partition has no section ${which}`);
    return found;
  };
  const level = section(GRID.level);
  const q = asInt32(section(GRID.q));
  const r = asInt32(section(GRID.r));
  const count = asUint32(section(GRID.count));
  const z = asFloat32(section(GRID.z));

  const leaves: AdaptiveCell[] = [];
  for (let i = 0; i < level.length; i += 1) {
    const size = meta.sizes[level[i]!];
    if (size === undefined) throw new Error(`a leaf is at level ${level[i]}, past the sizes`);
    const [x, y] = hexCenter({ q: q[i]!, r: r[i]! }, size);
    leaves.push({
      level: level[i]!,
      size,
      q: q[i]!,
      r: r[i]!,
      x,
      y,
      z: z[i]!,
      count: count[i]!,
      loaded: 0,
    });
  }

  const splitLevel = section(GRID.splitLevel);
  const splitQ = asInt32(section(GRID.splitQ));
  const splitR = asInt32(section(GRID.splitR));
  const splits: CellRef[] = [];
  for (let i = 0; i < splitLevel.length; i += 1) {
    splits.push({ level: splitLevel[i]!, q: splitQ[i]!, r: splitR[i]! });
  }

  // Absent from a backend without zone clusters: every leaf is in none.
  const clusters = meta.clusters ?? [];
  const clusterSection = envelope.section(GRID.cluster);
  const leafCluster = clusterSection ? asUint16(clusterSection) : new Uint16Array(leaves.length);
  if (leafCluster.length !== leaves.length) {
    throw new Error(`the partition has ${leaves.length} leaves and ${leafCluster.length} clusters`);
  }
  for (const at of leafCluster) {
    if (at >= clusters.length && clusters.length > 0) {
      throw new Error(`a leaf is in cluster ${at}, past the ${clusters.length} listed`);
    }
  }

  return {
    group: meta.group,
    sizes: meta.sizes,
    threshold: meta.threshold,
    maps: meta.maps,
    failed: meta.failed,
    placements: meta.placements,
    terrain: meta.terrain,
    leaves,
    splits,
    clusters,
    leafCluster,
  };
}

/**
 * The partition in the shape the Hex view draws, so the panel and the overlay
 * show the cells the backend serves and not a second partition of their own.
 */
export function adaptiveGridOf(grid: CellGrid): AdaptiveGrid {
  const levels = grid.sizes.map(() => new Map<number, AdaptiveCell | null>());
  for (const leaf of grid.leaves) levels[leaf.level]!.set(hexKey(leaf.q, leaf.r), leaf);
  for (const split of grid.splits) levels[split.level]!.set(hexKey(split.q, split.r), null);
  return { cells: grid.leaves, sizes: grid.sizes, levels };
}

/** The reference the backend wants for a leaf. */
export function refOf(leaf: AdaptiveCell): CellRef {
  return { level: leaf.level, q: leaf.q, r: leaf.r };
}

// ---- A batch of cells ---------------------------------------------------

/** A model name resolved to a file, or to nothing. */
export type Asset = { readonly name: string; readonly path: string | null };

/** One cell's placements within one map. */
export type CellPart = {
  readonly mapKey: string;
  readonly mapName: string;
  readonly columns: PlacementColumns;
};

/** One cell of a batch: who it is and what is in it. */
export type DecodedCell = {
  readonly ref: CellRef;
  readonly key: string;
  readonly count: number;
  readonly parts: readonly CellPart[];
};

export type DecodedBatch = {
  readonly cells: readonly DecodedCell[];
  /** The names the frontend did not already hold, resolved. */
  readonly assets: readonly Asset[];
  /** Compiled models that rode in the reply, by file path. */
  readonly models: ReadonlyMap<string, WorldModel>;
  /** Bytes of the reply. */
  readonly bytes: number;
};

type BatchMeta = {
  readonly cells: readonly (CellRef & { readonly start: number; readonly count: number })[];
  readonly count: number;
  readonly maps: readonly { readonly path: string; readonly name: string }[];
  readonly names: readonly string[];
  readonly nameHashes: readonly number[];
  readonly nameClasses?: readonly number[];
  readonly lodLevels: readonly string[];
  readonly assets: readonly Asset[];
  readonly models: readonly { readonly path: string; readonly section: number }[];
};

/** Batch envelope section indices: the placement columns, then `mapSlot`. */
const BATCH = {
  index: 0,
  nameIndex: 1,
  flags: 2,
  position: 3,
  rotation: 4,
  scaleXy: 5,
  scaleZ: 6,
  parentIndex: 7,
  lodLevel: 8,
  lodDist: 9,
  children: 10,
  mapSlot: 11,
} as const;

/** The decode of a model, injectable so tests need no real model bytes. */
export type ModelDecoder = (
  buffer: ArrayBuffer,
  byteOffset: number,
  byteLength: number,
  path: string,
) => WorldModel;

/** Decode the reply of `world_index_cells`. */
export function decodeCellBatch(
  buffer: ArrayBuffer,
  decodeModel: ModelDecoder = decodeWorldModel,
): DecodedBatch {
  const envelope = readEnvelope(buffer);
  if (envelope.kind !== Kind.CellData) {
    throw new Error(`envelope kind ${envelope.kind} is not a cell batch`);
  }
  const meta = envelope.meta as BatchMeta;
  const count = meta.count;

  const column = (which: number): Uint8Array => {
    const section = envelope.section(which);
    if (!section) throw new Error(`the cell batch has no section ${which}`);
    return section;
  };
  const indexes = asUint32(column(BATCH.index));
  const nameSlots = asUint32(column(BATCH.nameIndex));
  const flags = column(BATCH.flags);
  const positions = asFloat32(column(BATCH.position));
  const rotations = asFloat32(column(BATCH.rotation));
  const scaleXy = asFloat32(column(BATCH.scaleXy));
  const scaleZ = asFloat32(column(BATCH.scaleZ));
  const parents = asInt32(column(BATCH.parentIndex));
  const lodSlots = asUint16(column(BATCH.lodLevel));
  const lodDists = asFloat32(column(BATCH.lodDist));
  const children = asUint32(column(BATCH.children));
  const mapSlots = asUint32(column(BATCH.mapSlot));
  if (
    indexes.length < count ||
    nameSlots.length < count ||
    flags.length < count ||
    positions.length < count * 3 ||
    rotations.length < count * 4 ||
    scaleXy.length < count ||
    scaleZ.length < count ||
    parents.length < count ||
    lodSlots.length < count ||
    lodDists.length < count ||
    children.length < count ||
    mapSlots.length < count
  ) {
    throw new Error(`the cell batch declares ${count} placements but a column is shorter`);
  }

  const cells: DecodedCell[] = meta.cells.map((span) => {
    // Rows of one map, in the order they appear: the backend orders a cell's
    // rows by id, so they are contiguous, but nothing here relies on it.
    const byMap = new Map<number, number[]>();
    for (let row = span.start; row < span.start + span.count; row += 1) {
      const slot = mapSlots[row]!;
      const rows = byMap.get(slot);
      if (rows) rows.push(row);
      else byMap.set(slot, [row]);
    }

    const parts: CellPart[] = [];
    for (const [slot, rows] of byMap) {
      const map = meta.maps[slot];
      if (!map) throw new Error(`a row names map ${slot}, past the table`);
      const n = rows.length;
      const part: ColumnSource = {
        count: n,
        indexes: new Uint32Array(n),
        nameSlots: new Uint32Array(n),
        flags: new Uint8Array(n),
        positions: new Float32Array(n * 3),
        rotations: new Float32Array(n * 4),
        scaleXy: new Float32Array(n),
        scaleZ: new Float32Array(n),
        parents: new Int32Array(n),
        lodSlots: new Uint16Array(n),
        lodDists: new Float32Array(n),
        children: new Uint32Array(n),
        names: meta.names,
        nameHashes: meta.nameHashes,
        nameClasses: meta.nameClasses,
        lodLevels: meta.lodLevels,
      };
      for (let at = 0; at < n; at += 1) {
        const row = rows[at]!;
        part.indexes[at] = indexes[row]!;
        part.nameSlots[at] = nameSlots[row]!;
        part.flags[at] = flags[row]!;
        part.positions.set(positions.subarray(row * 3, row * 3 + 3), at * 3);
        part.rotations.set(rotations.subarray(row * 4, row * 4 + 4), at * 4);
        part.scaleXy[at] = scaleXy[row]!;
        part.scaleZ[at] = scaleZ[row]!;
        part.parents[at] = parents[row]!;
        part.lodSlots[at] = lodSlots[row]!;
        part.lodDists[at] = lodDists[row]!;
        part.children[at] = children[row]!;
      }
      parts.push({
        mapKey: map.path,
        mapName: map.name,
        columns: new PlacementColumns(part),
      });
    }
    const ref: CellRef = { level: span.level, q: span.q, r: span.r };
    return { ref, key: cellKey(ref), count: span.count, parts };
  });

  const models = new Map<string, WorldModel>();
  for (const model of meta.models) {
    const section = envelope.section(model.section);
    if (!section) throw new Error(`the cell batch names a missing model section ${model.section}`);
    models.set(
      model.path,
      decodeModel(section.buffer as ArrayBuffer, section.byteOffset, section.byteLength, model.path),
    );
  }

  return { cells, assets: meta.assets, models, bytes: buffer.byteLength };
}

/**
 * Where the partition sits, for framing the camera before any cell has arrived:
 * the middle of its footprint and the radius that covers it, in game units.
 * Height is the placement-weighted mean of the leaves.
 */
export function gridBounds(grid: CellGrid): {
  readonly center: [number, number, number];
  readonly radius: number;
} {
  if (grid.leaves.length === 0) return { center: [0, 0, 0], radius: 0 };
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  let zSum = 0;
  let weight = 0;
  for (const leaf of grid.leaves) {
    minX = Math.min(minX, leaf.x);
    minY = Math.min(minY, leaf.y);
    maxX = Math.max(maxX, leaf.x);
    maxY = Math.max(maxY, leaf.y);
    zSum += leaf.z * leaf.count;
    weight += leaf.count;
  }
  return {
    center: [(minX + maxX) / 2, (minY + maxY) / 2, weight > 0 ? zSum / weight : 0],
    radius: Math.hypot(maxX - minX, maxY - minY) / 2,
  };
}
