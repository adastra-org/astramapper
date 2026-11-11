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

/**
 * The archetype class carried per name (`nameClasses` in the envelope metadata),
 * as `cw-core`'s `ArchetypeClass`. The numbers are the wire contract; append,
 * never renumber. No class for terrain: nothing in the archetype record
 * identifies ground safely, so terrain stays on its name.
 */
export const ARCHETYPE_CLASS = { none: 0, decal: 1, vegetation: 2 } as const;

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
  /** Per name slot, an `ARCHETYPE_CLASS`. Absent when the backend sent none. */
  readonly nameClasses?: readonly number[];
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

  /** What the archetype record says this row is, an `ARCHETYPE_CLASS`; 0 for nothing. */
  archetypeClass(row: number): number {
    return this.source.nameClasses?.[this.source.nameSlots[row]!] ?? 0;
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
    // Only a row the data classified carries it, so a placement with no class
    // stays the same shape as before.
    const archetypeClass = s.nameClasses?.[slot] ?? 0;
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
      ...(archetypeClass !== 0 && { archetypeClass }),
    };
  }

  /** The placement with an entity index, or `undefined`. */
  find(entityIndex: number): Placed | undefined {
    const row = this.rowOf(entityIndex);
    return row < 0 ? undefined : this.at(row);
  }
}

/** Bytes per row across every column of a `ColumnSource`. */
const BYTES_PER_ROW = 4 + 4 + 12 + 16 + 4 + 4 + 4 + 4 + 4 + 2 + 1;

/**
 * Every column of `count` rows, as views over one buffer.
 *
 * One allocation, not eleven: V8 zeroes each large typed array as it makes it
 * (about 0.6 ms for eleven arrays of 40,000 rows against 0.03 ms for one buffer,
 * measured in Node), and a merge made a set of them on the main thread each time
 * a cell arrived or left. Wider elements come first so every view is aligned.
 */
export function allocateColumns(count: number): {
  indexes: Uint32Array;
  nameSlots: Uint32Array;
  positions: Float32Array;
  rotations: Float32Array;
  scaleXy: Float32Array;
  scaleZ: Float32Array;
  parents: Int32Array;
  lodDists: Float32Array;
  children: Uint32Array;
  lodSlots: Uint16Array;
  flags: Uint8Array;
} {
  const buffer = new ArrayBuffer(count * BYTES_PER_ROW);
  let offset = 0;
  const take = <T,>(make: new (buffer: ArrayBuffer, byteOffset: number, length: number) => T, length: number, size: number): T => {
    const view = new make(buffer, offset, length);
    offset += length * size;
    return view;
  };
  return {
    indexes: take(Uint32Array, count, 4),
    nameSlots: take(Uint32Array, count, 4),
    positions: take(Float32Array, count * 3, 4),
    rotations: take(Float32Array, count * 4, 4),
    scaleXy: take(Float32Array, count, 4),
    scaleZ: take(Float32Array, count, 4),
    parents: take(Int32Array, count, 4),
    lodDists: take(Float32Array, count, 4),
    children: take(Uint32Array, count, 4),
    lodSlots: take(Uint16Array, count, 2),
    flags: take(Uint8Array, count, 1),
  };
}

/** The name and LOD tables a merge builds up, so a later merge can carry on from them. */
type MergeTables = {
  names: string[];
  nameHashes: number[];
  nameClasses: number[];
  lodLevels: string[];
  nameSlotOf: Map<string, number>;
  lodSlotOf: Map<string, number>;
};

function emptyTables(): MergeTables {
  return {
    names: [],
    nameHashes: [],
    nameClasses: [],
    lodLevels: [],
    nameSlotOf: new Map(),
    lodSlotOf: new Map(),
  };
}

/**
 * One map's columns, kept up to date as its parts (one per cell held) come and go.
 *
 * # Why not `mergeColumns` every time
 *
 * A cell arriving or leaving changes a part of every map it touches, a hundred
 * or so, and each of those used to be merged again from all of its parts: every
 * part's names re-keyed, every row copied and re-translated, eleven arrays made.
 * That was 0.66 ms per map on average and 20 to 60 ms per cell, on the main
 * thread, inside the render that follows the change.
 *
 * What changed is one part (or a few), so this keeps the previous result and the
 * tables it was built with, copies the rows of the parts that stayed as a block
 * (their slots are already in the merged tables' numbering), and translates only
 * the parts that are new. A part is "the same" when its `PlacementColumns` is the
 * same object: a cell held again is a new one.
 *
 * Name and LOD tables only ever grow while parts stay; they start again when
 * nothing stays. Nothing is shared with a result handed out earlier, which keeps
 * its own arrays, so one still being read is never touched.
 */
export class ColumnsMerger {
  private tables = emptyTables();
  private keys: string[] = [];
  private refs: PlacementColumns[] = [];
  /** The row each kept part starts at, parallel to `keys`. */
  private starts: number[] = [];
  private merged: PlacementColumns | null = null;

  /** Forget everything: the next update builds from scratch. */
  private reset(): void {
    this.tables = emptyTables();
    this.keys = [];
    this.refs = [];
    this.starts = [];
    this.merged = null;
  }

  /**
   * The columns of every part given. Rows of parts already merged come first, in
   * the order they were, then the new parts' in the map's order. A single part is
   * returned as it is.
   */
  update(parts: ReadonlyMap<string, PlacementColumns>): PlacementColumns {
    if (parts.size === 1) {
      this.reset();
      return parts.values().next().value!;
    }
    const old = this.merged;

    // Which of the old parts survive, as runs of rows to copy in one go.
    const keptKeys = new Set<string>();
    const nextKeys: string[] = [];
    const nextRefs: PlacementColumns[] = [];
    const nextStarts: number[] = [];
    const runs: { from: number; to: number }[] = [];
    let count = 0;
    if (old) {
      for (let at = 0; at < this.keys.length; at += 1) {
        const key = this.keys[at]!;
        const ref = this.refs[at]!;
        if (parts.get(key) !== ref) continue;
        const from = this.starts[at]!;
        const to = from + ref.length;
        const last = runs[runs.length - 1];
        if (last && last.to === from) last.to = to;
        else runs.push({ from, to });
        keptKeys.add(key);
        nextKeys.push(key);
        nextRefs.push(ref);
        nextStarts.push(count);
        count += ref.length;
      }
    }
    if (count === 0) this.tables = emptyTables();
    const tables = this.tables;

    const added: [string, PlacementColumns][] = [];
    for (const entry of parts) {
      if (keptKeys.has(entry[0])) continue;
      added.push(entry);
      nextKeys.push(entry[0]);
      nextRefs.push(entry[1]);
      nextStarts.push(count);
      count += entry[1].length;
    }

    const out = allocateColumns(count);
    let at = 0;
    if (old) {
      const src = old.raw;
      for (const { from, to } of runs) {
        const n = to - from;
        if (n === 0) continue;
        out.indexes.set(src.indexes.subarray(from, to), at);
        out.nameSlots.set(src.nameSlots.subarray(from, to), at);
        out.flags.set(src.flags.subarray(from, to), at);
        out.positions.set(src.positions.subarray(from * 3, to * 3), at * 3);
        out.rotations.set(src.rotations.subarray(from * 4, to * 4), at * 4);
        out.scaleXy.set(src.scaleXy.subarray(from, to), at);
        out.scaleZ.set(src.scaleZ.subarray(from, to), at);
        out.parents.set(src.parents.subarray(from, to), at);
        out.lodSlots.set(src.lodSlots.subarray(from, to), at);
        out.lodDists.set(src.lodDists.subarray(from, to), at);
        out.children.set(src.children.subarray(from, to), at);
        at += n;
      }
    }
    for (const [, part] of added) {
      appendPart(out, at, part, tables);
      at += part.length;
    }

    const merged = new PlacementColumns({
      count,
      ...out,
      names: tables.names,
      nameHashes: tables.nameHashes,
      nameClasses: tables.nameClasses,
      lodLevels: tables.lodLevels,
    });
    this.keys = nextKeys;
    this.refs = nextRefs;
    this.starts = nextStarts;
    this.merged = merged;
    return merged;
  }
}

/** Write `part`'s rows at `at`, translating its slots into the merged `tables`. */
function appendPart(
  out: ReturnType<typeof allocateColumns>,
  at: number,
  part: PlacementColumns,
  tables: MergeTables,
): void {
  const src = part.raw;
  // This part's slots, translated once per distinct name rather than per row.
  const nameMap = new Uint32Array(src.names.length);
  for (let slot = 0; slot < src.names.length; slot += 1) {
    const name = src.names[slot]!;
    const hash = src.nameHashes[slot] ?? 0;
    const key = `${hash}:${name}`;
    let merged = tables.nameSlotOf.get(key);
    if (merged === undefined) {
      merged = tables.names.length;
      tables.names.push(name);
      tables.nameHashes.push(hash);
      tables.nameClasses.push(src.nameClasses?.[slot] ?? 0);
      tables.nameSlotOf.set(key, merged);
    }
    nameMap[slot] = merged;
  }
  const lodMap = new Uint16Array(src.lodLevels.length);
  for (let slot = 0; slot < src.lodLevels.length; slot += 1) {
    const label = src.lodLevels[slot]!;
    let merged = tables.lodSlotOf.get(label);
    if (merged === undefined) {
      merged = tables.lodLevels.length;
      tables.lodLevels.push(label);
      tables.lodSlotOf.set(label, merged);
    }
    lodMap[slot] = merged;
  }

  const n = part.length;
  out.indexes.set(src.indexes.subarray(0, n), at);
  out.flags.set(src.flags.subarray(0, n), at);
  out.positions.set(src.positions.subarray(0, n * 3), at * 3);
  out.rotations.set(src.rotations.subarray(0, n * 4), at * 4);
  out.scaleXy.set(src.scaleXy.subarray(0, n), at);
  out.scaleZ.set(src.scaleZ.subarray(0, n), at);
  out.parents.set(src.parents.subarray(0, n), at);
  out.lodDists.set(src.lodDists.subarray(0, n), at);
  out.children.set(src.children.subarray(0, n), at);
  for (let row = 0; row < n; row += 1) {
    out.nameSlots[at + row] = nameMap[src.nameSlots[row]!]!;
    out.lodSlots[at + row] = lodMap[src.lodSlots[row]!]!;
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
 * dropped as soon as this returns. A map that changes a part at a time wants
 * `ColumnsMerger` instead.
 */
export function mergeColumns(parts: readonly PlacementColumns[]): PlacementColumns {
  if (parts.length === 1) return parts[0]!;
  return new ColumnsMerger().update(new Map(parts.map((part, at) => [String(at), part])));
}

/** Columns from a list of `Placed`: the demo world, and tests. */
export function columnsFromPlaced(list: readonly Placed[]): PlacementColumns {
  const count = list.length;
  const names: string[] = [];
  const nameHashes: number[] = [];
  const nameClasses: number[] = [];
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
    const archetypeClass = item.archetypeClass ?? 0;
    const nameKey = `${item.nameHash}:${archetypeClass}:${item.name}`;
    let nameSlot = nameSlotOf.get(nameKey);
    if (nameSlot === undefined) {
      nameSlot = names.length;
      names.push(item.name);
      nameHashes.push(item.nameHash);
      nameClasses.push(archetypeClass);
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
    nameClasses,
    lodLevels,
  });
}
