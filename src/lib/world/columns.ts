/**
 * Placements held as columns, not as one object each.
 *
 * # Why
 *
 * With the base game placed, the WebView held ~760,000 `Placed` objects, each
 * with a `position` and a `rotation` tuple: millions of small heap cells. Every
 * collection of them paused the main thread for ~300 ms and the JS heap sat at
 * 827 MB. The wire envelope is already columnar, so this keeps it that way:
 * a map's placements are the typed-array views over the response buffer, and a
 * `Placed` exists only for the handful of entities something is actually
 * touching (a streamer candidate, the selection, a raycast hit).
 *
 * Hot loops (the streaming pass, hex binning, bounds, the box markers) read the
 * columns directly by row. Nothing here allocates per row except `at`.
 *
 * A *row* is a position in the columns. An *entity index* is the identity the
 * game file gives an entity, and the one the streamer, a raycast hit and the
 * selection carry. They are equal for an unfiltered map and not otherwise, so
 * `rowOf` resolves one to the other and never assumes.
 */
import type { Placed } from "../world";

/** Bit 0 of the flags column. */
const FLAG_IS_MLO = 1;
/** Bit 1 of the flags column. */
const FLAG_FROM_MLO = 2;

/** The raw columns and tables a `PlacementColumns` is laid over. */
export type ColumnSource = {
  readonly count: number;
  readonly indexes: Uint32Array;
  readonly nameSlots: Uint32Array;
  readonly flags: Uint8Array;
  /** `x, y, z` per row. */
  readonly positions: Float32Array;
  /** `x, y, z, w` per row. */
  readonly rotations: Float32Array;
  readonly scaleXy: Float32Array;
  readonly scaleZ: Float32Array;
  readonly parents: Int32Array;
  readonly lodSlots: Uint16Array;
  readonly lodDists: Float32Array;
  readonly children: Uint32Array;
  readonly names: readonly string[];
  readonly nameHashes: readonly number[];
  readonly lodLevels: readonly string[];
};

export class PlacementColumns {
  /** Rows. Named like an array's so counting call sites read the same. */
  readonly length: number;
  readonly indexes: Uint32Array;
  readonly positions: Float32Array;
  readonly rotations: Float32Array;
  readonly lodDists: Float32Array;
  private readonly source: ColumnSource;
  /** True when row `r` holds entity index `r` throughout, so `rowOf` is free. */
  private readonly identity: boolean;
  private byEntity: Map<number, number> | null = null;

  constructor(source: ColumnSource) {
    this.source = source;
    this.length = source.count;
    this.indexes = source.indexes;
    this.positions = source.positions;
    this.rotations = source.rotations;
    this.lodDists = source.lodDists;
    let identity = true;
    for (let row = 0; row < this.length; row += 1) {
      if (source.indexes[row] !== row) {
        identity = false;
        break;
      }
    }
    this.identity = identity;
  }

  x(row: number): number {
    return this.positions[row * 3]!;
  }

  y(row: number): number {
    return this.positions[row * 3 + 1]!;
  }

  z(row: number): number {
    return this.positions[row * 3 + 2]!;
  }

  scaleXy(row: number): number {
    return this.source.scaleXy[row]!;
  }

  scaleZ(row: number): number {
    return this.source.scaleZ[row]!;
  }

  entityIndex(row: number): number {
    return this.indexes[row]!;
  }

  name(row: number): string {
    return this.source.names[this.source.nameSlots[row]!] ?? "";
  }

  lodLevel(row: number): string {
    return this.source.lodLevels[this.source.lodSlots[row]!] ?? "";
  }

  lodDist(row: number): number {
    return this.lodDists[row]!;
  }

  isMlo(row: number): boolean {
    return (this.source.flags[row]! & FLAG_IS_MLO) !== 0;
  }

  /**
   * The row holding an entity index, or -1. A duplicate index resolves to its
   * first row, which is what the linear `find` this replaces returned.
   */
  rowOf(entityIndex: number): number {
    if (this.identity) {
      return Number.isInteger(entityIndex) && entityIndex >= 0 && entityIndex < this.length
        ? entityIndex
        : -1;
    }
    if (!this.byEntity) {
      const byEntity = new Map<number, number>();
      for (let row = 0; row < this.length; row += 1) {
        const index = this.indexes[row]!;
        if (!byEntity.has(index)) byEntity.set(index, row);
      }
      this.byEntity = byEntity;
    }
    return this.byEntity.get(entityIndex) ?? -1;
  }

  /** How many distinct entity indexes the columns hold. */
  get distinct(): number {
    if (this.identity) return this.length;
    this.rowOf(-1);
    return this.byEntity!.size;
  }

  /** The arrays and tables this is laid over, for building from several of them. */
  get raw(): ColumnSource {
    return this.source;
  }

  /** One row as a `Placed`: the only place an object is made. */
  at(row: number): Placed {
    const s = this.source;
    const slot = s.nameSlots[row]!;
    const flag = s.flags[row]!;
    const p = row * 3;
    const r = row * 4;
    return {
      index: s.indexes[row]!,
      name: s.names[slot] ?? "",
      nameHash: s.nameHashes[slot] ?? 0,
      isMlo: (flag & FLAG_IS_MLO) !== 0,
      fromMlo: (flag & FLAG_FROM_MLO) !== 0,
      position: [s.positions[p]!, s.positions[p + 1]!, s.positions[p + 2]!],
      rotation: [s.rotations[r]!, s.rotations[r + 1]!, s.rotations[r + 2]!, s.rotations[r + 3]!],
      scaleXy: s.scaleXy[row]!,
      scaleZ: s.scaleZ[row]!,
      parentIndex: s.parents[row]!,
      lodLevel: s.lodLevels[s.lodSlots[row]!] ?? "",
      lodDist: s.lodDists[row]!,
      children: s.children[row]!,
    };
  }

  /** The placement with an entity index, or `undefined`. */
  find(entityIndex: number): Placed | undefined {
    const row = this.rowOf(entityIndex);
    return row < 0 ? undefined : this.at(row);
  }
}

/**
 * One set of columns from several, rows in the order given.
 *
 * What a map is when its placements arrive a cell at a time: each cell's part
 * of it is its own `PlacementColumns`, and the streamer wants the map whole.
 * The name and LOD tables are rebuilt, because the parts come from different
 * replies and so number their names differently; a name is the same name when
 * its hash is, as in `columnsFromPlaced`.
 *
 * Copies every row once. Nothing about the parts is kept, so they can be
 * dropped as soon as this returns.
 */
export function mergeColumns(parts: readonly PlacementColumns[]): PlacementColumns {
  if (parts.length === 1) return parts[0]!;
  let count = 0;
  for (const part of parts) count += part.length;

  const names: string[] = [];
  const nameHashes: number[] = [];
  const lodLevels: string[] = [];
  const nameSlotOf = new Map<string, number>();
  const lodSlotOf = new Map<string, number>();

  const indexes = new Uint32Array(count);
  const nameSlots = new Uint32Array(count);
  const flags = new Uint8Array(count);
  const positions = new Float32Array(count * 3);
  const rotations = new Float32Array(count * 4);
  const scaleXy = new Float32Array(count);
  const scaleZ = new Float32Array(count);
  const parents = new Int32Array(count);
  const lodSlots = new Uint16Array(count);
  const lodDists = new Float32Array(count);
  const children = new Uint32Array(count);

  let at = 0;
  for (const part of parts) {
    const src = part.raw;
    // This part's slots, translated once per distinct name rather than per row.
    const nameMap = new Uint32Array(src.names.length);
    for (let slot = 0; slot < src.names.length; slot += 1) {
      const name = src.names[slot]!;
      const hash = src.nameHashes[slot] ?? 0;
      const key = `${hash}:${name}`;
      let merged = nameSlotOf.get(key);
      if (merged === undefined) {
        merged = names.length;
        names.push(name);
        nameHashes.push(hash);
        nameSlotOf.set(key, merged);
      }
      nameMap[slot] = merged;
    }
    const lodMap = new Uint16Array(src.lodLevels.length);
    for (let slot = 0; slot < src.lodLevels.length; slot += 1) {
      const label = src.lodLevels[slot]!;
      let merged = lodSlotOf.get(label);
      if (merged === undefined) {
        merged = lodLevels.length;
        lodLevels.push(label);
        lodSlotOf.set(label, merged);
      }
      lodMap[slot] = merged;
    }

    const n = part.length;
    indexes.set(src.indexes.subarray(0, n), at);
    flags.set(src.flags.subarray(0, n), at);
    positions.set(src.positions.subarray(0, n * 3), at * 3);
    rotations.set(src.rotations.subarray(0, n * 4), at * 4);
    scaleXy.set(src.scaleXy.subarray(0, n), at);
    scaleZ.set(src.scaleZ.subarray(0, n), at);
    parents.set(src.parents.subarray(0, n), at);
    lodDists.set(src.lodDists.subarray(0, n), at);
    children.set(src.children.subarray(0, n), at);
    for (let row = 0; row < n; row += 1) {
      nameSlots[at + row] = nameMap[src.nameSlots[row]!]!;
      lodSlots[at + row] = lodMap[src.lodSlots[row]!]!;
    }
    at += n;
  }

  return new PlacementColumns({
    count,
    indexes,
    nameSlots,
    flags,
    positions,
    rotations,
    scaleXy,
    scaleZ,
    parents,
    lodSlots,
    lodDists,
    children,
    names,
    nameHashes,
    lodLevels,
  });
}

/** Columns from a list of `Placed`: the demo world, and tests. */
export function columnsFromPlaced(list: readonly Placed[]): PlacementColumns {
  const count = list.length;
  const names: string[] = [];
  const nameHashes: number[] = [];
  const lodLevels: string[] = [];
  const nameSlotOf = new Map<string, number>();
  const lodSlotOf = new Map<string, number>();

  const indexes = new Uint32Array(count);
  const nameSlots = new Uint32Array(count);
  const flags = new Uint8Array(count);
  const positions = new Float32Array(count * 3);
  const rotations = new Float32Array(count * 4);
  const scaleXy = new Float32Array(count);
  const scaleZ = new Float32Array(count);
  const parents = new Int32Array(count);
  const lodSlots = new Uint16Array(count);
  const lodDists = new Float32Array(count);
  const children = new Uint32Array(count);

  for (let row = 0; row < count; row += 1) {
    const item = list[row]!;
    // Keyed by name and hash together, so two entities sharing a name but not a
    // hash keep their own hash.
    const nameKey = `${item.nameHash}:${item.name}`;
    let nameSlot = nameSlotOf.get(nameKey);
    if (nameSlot === undefined) {
      nameSlot = names.length;
      names.push(item.name);
      nameHashes.push(item.nameHash);
      nameSlotOf.set(nameKey, nameSlot);
    }
    let lodSlot = lodSlotOf.get(item.lodLevel);
    if (lodSlot === undefined) {
      lodSlot = lodLevels.length;
      lodLevels.push(item.lodLevel);
      lodSlotOf.set(item.lodLevel, lodSlot);
    }
    indexes[row] = item.index;
    nameSlots[row] = nameSlot;
    flags[row] = (item.isMlo ? FLAG_IS_MLO : 0) | (item.fromMlo ? FLAG_FROM_MLO : 0);
    positions.set(item.position, row * 3);
    rotations.set(item.rotation, row * 4);
    scaleXy[row] = item.scaleXy;
    scaleZ[row] = item.scaleZ;
    parents[row] = item.parentIndex;
    lodSlots[row] = lodSlot;
    lodDists[row] = item.lodDist;
    children[row] = item.children;
  }

  return new PlacementColumns({
    count,
    indexes,
    nameSlots,
    flags,
    positions,
    rotations,
    scaleXy,
    scaleZ,
    parents,
    lodSlots,
    lodDists,
    children,
    names,
    nameHashes,
    lodLevels,
  });
}
