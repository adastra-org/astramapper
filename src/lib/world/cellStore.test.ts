import { describe, expect, test } from "bun:test";

import type { CellPart, DecodedCell } from "./cellData";
import { cellKey } from "./cellData";
import { CellStore } from "./cellStore";
import { columnsFromPlaced, mergeColumns } from "./columns";

function placed(index: number, name: string, x: number, y: number, hash = 1, lod = "HD") {
  return {
    index,
    name,
    nameHash: hash,
    isMlo: false,
    fromMlo: false,
    position: [x, y, 0] as [number, number, number],
    rotation: [0, 0, 0, 1] as [number, number, number, number],
    scaleXy: 1,
    scaleZ: 1,
    parentIndex: -1,
    lodLevel: lod,
    lodDist: 100,
    children: 0,
  };
}

function part(mapKey: string, rows: ReturnType<typeof placed>[]): CellPart {
  return { mapKey, mapName: mapKey.replace(".ymap", ""), columns: columnsFromPlaced(rows) };
}

function cell(q: number, parts: CellPart[]): DecodedCell {
  const ref = { level: 0, q, r: 0 };
  return {
    ref,
    key: cellKey(ref),
    count: parts.reduce((sum, p) => sum + p.columns.length, 0),
    parts,
  };
}

describe("mergeColumns", () => {
  test("rows keep their order and every field", () => {
    const a = columnsFromPlaced([placed(3, "lamp", 1, 2), placed(4, "bin", 3, 4, 2, "LOD")]);
    const b = columnsFromPlaced([placed(9, "bin", 5, 6, 2, "LOD"), placed(10, "tree", 7, 8, 3)]);
    const merged = mergeColumns([a, b]);
    expect(merged.length).toBe(4);
    expect(Array.from({ length: 4 }, (_, i) => merged.at(i))).toEqual([
      ...Array.from({ length: 2 }, (_, i) => a.at(i)),
      ...Array.from({ length: 2 }, (_, i) => b.at(i)),
    ]);
  });

  test("a name two parts share is one entry in the merged table", () => {
    const a = columnsFromPlaced([placed(0, "lamp", 0, 0)]);
    const b = columnsFromPlaced([placed(1, "bin", 0, 0), placed(2, "lamp", 0, 0)]);
    const merged = mergeColumns([a, b]);
    expect(merged.raw.names).toEqual(["lamp", "bin"]);
    expect(merged.name(2)).toBe("lamp");
    expect(merged.raw.lodLevels).toEqual(["HD"]);
  });

  test("one name under two hashes keeps both", () => {
    const merged = mergeColumns([
      columnsFromPlaced([placed(0, "lamp", 0, 0, 1)]),
      columnsFromPlaced([placed(1, "lamp", 0, 0, 999)]),
    ]);
    expect(merged.at(0).nameHash).toBe(1);
    expect(merged.at(1).nameHash).toBe(999);
  });

  test("the archetype class follows its name through a merge", () => {
    const a = columnsFromPlaced([{ ...placed(0, "prop_bush", 0, 0), archetypeClass: 2 }]);
    const b = columnsFromPlaced([placed(1, "prop_wall", 0, 0), { ...placed(2, "prop_bush", 0, 0), archetypeClass: 2 }]);
    const merged = mergeColumns([a, b]);
    expect([0, 1, 2].map((row) => merged.archetypeClass(row))).toEqual([2, 0, 2]);
  });

  test("one part is returned as it is, and rows are found by entity index", () => {
    const only = columnsFromPlaced([placed(7, "lamp", 0, 0)]);
    expect(mergeColumns([only])).toBe(only);
    const merged = mergeColumns([only, columnsFromPlaced([placed(2, "bin", 0, 0)])]);
    expect(merged.find(2)?.name).toBe("bin");
    expect(merged.find(7)?.name).toBe("lamp");
    expect(merged.find(5)).toBeUndefined();
  });
});

describe("CellStore", () => {
  test("a map is one entry however many cells it spans", () => {
    const store = new CellStore();
    store.add(cell(0, [part("a.ymap", [placed(0, "lamp", 0, 0)])]));
    store.add(cell(1, [part("a.ymap", [placed(1, "lamp", 100, 0)]), part("b.ymap", [placed(0, "bin", 90, 0)])]));

    const entries = store.entries();
    // oxlint-disable-next-line unicorn/no-array-sort
    expect(entries.map((e) => e.key).sort()).toEqual(["a.ymap", "b.ymap"]);
    const a = entries.find((e) => e.key === "a.ymap")!;
    expect(a.placements.length).toBe(2);
    expect(a.name).toBe("a");
    expect(store.placements).toBe(3);
    expect(store.size).toBe(2);
  });

  test("the streaming box is everywhere", () => {
    const store = new CellStore();
    store.add(cell(0, [part("a.ymap", [placed(0, "lamp", 0, 0)])]));
    const [entry] = store.entries();
    expect(entry!.extentsMin[0]).toBeLessThan(-1e6);
    expect(entry!.extentsMax[1]).toBeGreaterThan(1e6);
  });

  test("dropping a cell returns its placements and takes them out of the map", () => {
    const store = new CellStore();
    store.add(cell(0, [part("a.ymap", [placed(0, "lamp", 0, 0), placed(1, "lamp", 1, 0)])]));
    store.add(cell(1, [part("a.ymap", [placed(2, "lamp", 100, 0)])]));

    const dropped = store.remove(cellKey({ level: 0, q: 0, r: 0 }));
    expect(dropped).toEqual([
      { mapKey: "a.ymap", entityIndex: 0 },
      { mapKey: "a.ymap", entityIndex: 1 },
    ]);
    expect(store.entries()[0]!.placements.length).toBe(1);
    expect(store.entries()[0]!.placements.entityIndex(0)).toBe(2);
    expect(store.placements).toBe(1);
  });

  test("a map with no cells left disappears", () => {
    const store = new CellStore();
    store.add(cell(0, [part("a.ymap", [placed(0, "lamp", 0, 0)])]));
    store.remove(cellKey({ level: 0, q: 0, r: 0 }));
    expect(store.entries()).toEqual([]);
    expect(store.size).toBe(0);
    expect(store.remove("nothing")).toEqual([]);
  });

  test("entries keep their identity until a cell comes or goes, and only the touched map is rebuilt", () => {
    const store = new CellStore();
    store.add(cell(0, [part("a.ymap", [placed(0, "lamp", 0, 0)])]));
    store.add(cell(1, [part("b.ymap", [placed(0, "bin", 0, 0)])]));
    const first = store.entries();
    expect(store.entries()).toBe(first);
    const a = first.find((e) => e.key === "a.ymap")!;
    const b = first.find((e) => e.key === "b.ymap")!;

    store.add(cell(2, [part("b.ymap", [placed(1, "bin", 5, 0)])]));
    const second = store.entries();
    expect(second).not.toBe(first);
    expect(second.find((e) => e.key === "a.ymap")).toBe(a);
    expect(second.find((e) => e.key === "b.ymap")).not.toBe(b);
  });

  test("adding a cell that is already held replaces it", () => {
    const store = new CellStore();
    store.add(cell(0, [part("a.ymap", [placed(0, "lamp", 0, 0)])]));
    store.add(cell(0, [part("a.ymap", [placed(0, "lamp", 0, 0), placed(1, "lamp", 1, 1)])]));
    expect(store.size).toBe(1);
    expect(store.placements).toBe(2);
    expect(store.entries()[0]!.placements.length).toBe(2);
  });

  test("an empty cell is held and holds nothing", () => {
    const store = new CellStore();
    store.add(cell(0, []));
    expect(store.has(cellKey({ level: 0, q: 0, r: 0 }))).toBe(true);
    expect(store.entries()).toEqual([]);
  });
});
