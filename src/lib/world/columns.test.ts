import { describe, expect, test } from "bun:test";

import type { Placed } from "../world";
import { allocateColumns, ColumnsMerger, columnsFromPlaced, type PlacementColumns } from "./columns";

function placed(index: number, over: Partial<Placed> = {}): Placed {
  return {
    index,
    name: `prop_${index % 3}`,
    nameHash: index % 3,
    isMlo: index % 2 === 0,
    fromMlo: index % 5 === 0,
    position: [index, index * 2, index * 3],
    rotation: [0, 0.5, 0, 0.5],
    scaleXy: 2,
    scaleZ: 4,
    parentIndex: index - 1,
    lodLevel: index % 2 === 0 ? "HD" : "LOD",
    lodDist: 100 + index,
    children: index,
    ...over,
  };
}

describe("PlacementColumns", () => {
  test("a row round-trips through the columns unchanged", () => {
    const list = [placed(0), placed(1), placed(2), placed(3)];
    const cols = columnsFromPlaced(list);

    expect(cols.length).toBe(4);
    for (let row = 0; row < list.length; row += 1) expect(cols.at(row)).toEqual(list[row]!);
  });

  test("field readers agree with the materialized row", () => {
    const cols = columnsFromPlaced([placed(0), placed(7)]);
    const row = cols.at(1);

    expect(cols.x(1)).toBe(row.position[0]);
    expect(cols.y(1)).toBe(row.position[1]);
    expect(cols.z(1)).toBe(row.position[2]);
    expect(cols.name(1)).toBe(row.name);
    expect(cols.lodLevel(1)).toBe(row.lodLevel);
    expect(cols.lodDist(1)).toBe(row.lodDist);
    expect(cols.isMlo(1)).toBe(row.isMlo);
    expect(cols.scaleXy(1)).toBe(row.scaleXy);
    expect(cols.scaleZ(1)).toBe(row.scaleZ);
    expect(cols.entityIndex(1)).toBe(7);
  });

  test("a class survives the round trip and rides the name slot", () => {
    const list = [
      placed(0, { name: "prop_kelp", nameHash: 9, archetypeClass: 2 }),
      placed(1, { name: "prop_kelp", nameHash: 9, archetypeClass: 2 }),
      placed(2, { name: "prop_plain", nameHash: 8 }),
    ];
    const cols = columnsFromPlaced(list);

    expect(cols.archetypeClass(0)).toBe(2);
    expect(cols.archetypeClass(1)).toBe(2);
    expect(cols.archetypeClass(2)).toBe(0);
    for (let row = 0; row < list.length; row += 1) expect(cols.at(row)).toEqual(list[row]!);
  });

  test("rowOf is the identity for an unfiltered map", () => {
    const cols = columnsFromPlaced([placed(0), placed(1), placed(2)]);

    expect(cols.rowOf(2)).toBe(2);
    expect(cols.rowOf(3)).toBe(-1);
    expect(cols.rowOf(-1)).toBe(-1);
    expect(cols.distinct).toBe(3);
  });

  test("rowOf resolves sparse, out-of-order indexes and keeps the first duplicate", () => {
    const cols = columnsFromPlaced([placed(12), placed(3), placed(40), placed(3, { name: "dup" })]);

    expect(cols.rowOf(12)).toBe(0);
    expect(cols.rowOf(40)).toBe(2);
    expect(cols.rowOf(3)).toBe(1);
    expect(cols.find(3)?.name).not.toBe("dup");
    expect(cols.rowOf(0)).toBe(-1);
    expect(cols.distinct).toBe(3);
  });

  test("an empty set is empty and finds nothing", () => {
    const cols = columnsFromPlaced([]);

    expect(cols.length).toBe(0);
    expect(cols.find(0)).toBeUndefined();
    expect(cols.distinct).toBe(0);
  });

  test("names sharing a string but not a hash keep their own hash", () => {
    const cols = columnsFromPlaced([placed(0, { name: "a", nameHash: 1 }), placed(1, { name: "a", nameHash: 2 })]);

    expect(cols.at(0).nameHash).toBe(1);
    expect(cols.at(1).nameHash).toBe(2);
  });
});

/** A part of `n` rows whose entity indexes start at `base`, with names of its own. */
function part(base: number, n: number, lod = "HD"): PlacementColumns {
  return columnsFromPlaced(
    Array.from({ length: n }, (_, i) =>
      placed(base + i, {
        name: `prop_${(base + i) % 4}`,
        nameHash: (base + i) % 4,
        lodLevel: i % 2 === 0 ? lod : "LOD",
      }),
    ),
  );
}

/** Every row of `cols`, as `Placed`, in order. */
function rowsOf(cols: PlacementColumns): Placed[] {
  return Array.from({ length: cols.length }, (_, row) => cols.at(row));
}

/** The rows of `parts`, sorted by entity index, to compare without caring about order. */
function sorted(rows: Placed[]): Placed[] {
  // oxlint-disable-next-line unicorn/no-array-sort
  return [...rows].sort((a, b) => a.index - b.index);
}

describe("allocateColumns", () => {
  test("every column has the length its row count says, and they do not overlap", () => {
    const cols = allocateColumns(7);
    expect(cols.indexes.length).toBe(7);
    expect(cols.positions.length).toBe(21);
    expect(cols.rotations.length).toBe(28);
    expect(cols.lodSlots.length).toBe(7);
    expect(cols.flags.length).toBe(7);
    cols.indexes.fill(0xffffffff);
    cols.positions.fill(1.5);
    cols.flags.fill(0xff);
    expect(Array.from(cols.nameSlots)).toEqual([0, 0, 0, 0, 0, 0, 0]);
    expect(Array.from(cols.lodSlots)).toEqual([0, 0, 0, 0, 0, 0, 0]);
    expect(Array.from(cols.children)).toEqual([0, 0, 0, 0, 0, 0, 0]);
  });
});

describe("ColumnsMerger", () => {
  test("a single part comes back as it is", () => {
    const only = part(0, 3);
    expect(new ColumnsMerger().update(new Map([["a", only]]))).toBe(only);
  });

  test("the first merge holds every part's rows in the map's order", () => {
    const a = part(0, 3);
    const b = part(10, 2);
    const merged = new ColumnsMerger().update(
      new Map([
        ["a", a],
        ["b", b],
      ]),
    );
    expect(rowsOf(merged)).toEqual([...rowsOf(a), ...rowsOf(b)]);
  });

  test("a part added later is appended and the rest stay as they were", () => {
    const merger = new ColumnsMerger();
    const a = part(0, 3);
    const b = part(10, 2);
    const c = part(20, 4, "SLOD");
    const parts = new Map([
      ["a", a],
      ["b", b],
    ]);
    merger.update(parts);
    parts.set("c", c);
    const merged = merger.update(parts);
    expect(rowsOf(merged)).toEqual([...rowsOf(a), ...rowsOf(b), ...rowsOf(c)]);
  });

  test("a part removed from the middle takes only its own rows", () => {
    const merger = new ColumnsMerger();
    const a = part(0, 3);
    const b = part(10, 2);
    const c = part(20, 4);
    const parts = new Map([
      ["a", a],
      ["b", b],
      ["c", c],
    ]);
    merger.update(parts);
    parts.delete("b");
    const merged = merger.update(parts);
    expect(rowsOf(merged)).toEqual([...rowsOf(a), ...rowsOf(c)]);
  });

  test("a part held again as a new object replaces the old rows", () => {
    const merger = new ColumnsMerger();
    const a = part(0, 3);
    const b = part(10, 2);
    const parts = new Map([
      ["a", a],
      ["b", b],
    ]);
    merger.update(parts);
    const again = part(30, 5);
    parts.set("a", again);
    const merged = merger.update(parts);
    expect(rowsOf(merged)).toEqual([...rowsOf(b), ...rowsOf(again)]);
  });

  test("the previous result is not touched by the next merge", () => {
    const merger = new ColumnsMerger();
    const parts = new Map([
      ["a", part(0, 3)],
      ["b", part(10, 2)],
    ]);
    const before = merger.update(parts);
    const snapshot = rowsOf(before);
    parts.delete("a");
    parts.set("c", part(40, 6));
    merger.update(parts);
    expect(rowsOf(before)).toEqual(snapshot);
  });

  test("falling to one part and growing again starts the tables afresh", () => {
    const merger = new ColumnsMerger();
    const a = part(0, 3);
    const b = part(10, 2);
    const c = part(20, 2, "SLOD");
    const parts = new Map([
      ["a", a],
      ["b", b],
    ]);
    merger.update(parts);
    parts.delete("a");
    expect(merger.update(parts)).toBe(b);
    parts.set("c", c);
    expect(rowsOf(merger.update(parts))).toEqual([...rowsOf(b), ...rowsOf(c)]);
  });

  test("names and LODs are one entry each however many parts carry them", () => {
    const merger = new ColumnsMerger();
    const parts = new Map<string, PlacementColumns>();
    for (let at = 0; at < 6; at += 1) parts.set(`p${at}`, part(at * 10, 8));
    merger.update(parts);
    parts.delete("p1");
    parts.set("p9", part(90, 8));
    const merged = merger.update(parts);
    expect(merged.raw.names.length).toBe(4);
    expect(merged.raw.lodLevels).toEqual(["HD", "LOD"]);
  });

  /**
   * Any history of adds and removals ends on the same rows as merging the parts
   * that are left from scratch.
   */
  test("agrees with a merge from scratch after any history", () => {
    let seed = 12345;
    const random = (n: number): number => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed % n;
    };
    const merger = new ColumnsMerger();
    const parts = new Map<string, PlacementColumns>();
    let nextBase = 0;
    for (let step = 0; step < 300; step += 1) {
      if (parts.size > 0 && random(3) === 0) {
        const keys = [...parts.keys()];
        parts.delete(keys[random(keys.length)]!);
      } else {
        const key = `cell${random(12)}`;
        // A cell held again is a new object under the same key.
        parts.set(key, part(nextBase, 1 + random(6), random(2) === 0 ? "HD" : "ORPHAN"));
        nextBase += 10;
      }
      if (parts.size === 0) continue;
      const merged = merger.update(parts);
      const fresh = new ColumnsMerger().update(new Map(parts));
      expect(merged.length).toBe(fresh.length);
      expect(sorted(rowsOf(merged))).toEqual(sorted(rowsOf(fresh)));
    }
  });
});
