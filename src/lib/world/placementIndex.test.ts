import { describe, expect, test } from "bun:test";

import { createPlacementIndex, EMPTY_PLACEMENT_INDEX } from "./placementIndex";
import type { Placed } from "../world";
import { columnsFromPlaced } from "./columns";

function placement(index: number, name = `prop_${index}`): Placed {
  return {
    index,
    name,
    nameHash: index,
    isMlo: false,
    fromMlo: false,
    position: [index, 0, 0],
    rotation: [0, 0, 0, 1],
    scaleXy: 1,
    scaleZ: 1,
    parentIndex: -1,
    lodLevel: "HD",
    lodDist: 100,
    children: 0,
  };
}

describe("createPlacementIndex", () => {
  test("resolves a placement by its map and file index", () => {
    const index = createPlacementIndex([
      { key: "a.ymap", placements: columnsFromPlaced([placement(0), placement(7)]) },
      { key: "b.ymap", placements: columnsFromPlaced([placement(7, "other")]) },
    ]);

    expect(index.placement("a.ymap", 7)?.name).toBe("prop_7");
    expect(index.placement("b.ymap", 7)?.name).toBe("other");
    expect(index.maps).toBe(2);
    expect(index.placements).toBe(3);
  });

  test("keys on the file index, not the array position", () => {
    // The entity indexes are sparse and out of order, which is what a filtered
    // or MLO-expanded map looks like. Position 0 holds index 12.
    const index = createPlacementIndex([
      { key: "a.ymap", placements: columnsFromPlaced([placement(12), placement(3), placement(40)]) },
    ]);

    expect(index.placement("a.ymap", 12)?.index).toBe(12);
    expect(index.placement("a.ymap", 0)).toBeUndefined();
    expect(index.placement("a.ymap", 1)).toBeUndefined();
  });

  test("hands back the entry itself, not a copy", () => {
    const entry = { key: "a.ymap", name: "downtown", placements: columnsFromPlaced([placement(0)]) };
    const index = createPlacementIndex([entry]);

    expect(index.entry("a.ymap")).toBe(entry);
    expect(index.entry("a.ymap")?.name).toBe("downtown");
    expect(index.entry("missing.ymap")).toBeUndefined();
  });

  test("builds the placement from the columns, with every field intact", () => {
    const only = placement(5);
    const index = createPlacementIndex([{ key: "a.ymap", placements: columnsFromPlaced([only]) }]);

    expect(index.placement("a.ymap", 5)).toEqual(only);
  });

  test("a missing map resolves to undefined rather than throwing", () => {
    const index = createPlacementIndex([{ key: "a.ymap", placements: columnsFromPlaced([placement(0)]) }]);

    expect(index.placement("gone.ymap", 0)).toBeUndefined();
  });

  test("a duplicate keeps the first, the way find did", () => {
    const index = createPlacementIndex([
      { key: "a.ymap", placements: columnsFromPlaced([placement(1, "first"), placement(1, "second")]) },
      { key: "a.ymap", placements: columnsFromPlaced([placement(1, "third")]) },
    ]);

    expect(index.placement("a.ymap", 1)?.name).toBe("first");
    expect(index.maps).toBe(1);
    expect(index.placements).toBe(1);
  });

  test("the empty index answers everything with undefined", () => {
    expect(EMPTY_PLACEMENT_INDEX.entry("a.ymap")).toBeUndefined();
    expect(EMPTY_PLACEMENT_INDEX.placement("a.ymap", 0)).toBeUndefined();
    expect(EMPTY_PLACEMENT_INDEX.maps).toBe(0);
    expect(EMPTY_PLACEMENT_INDEX.placements).toBe(0);
  });
});
