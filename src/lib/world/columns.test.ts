import { describe, expect, test } from "bun:test";

import type { Placed } from "../world";
import { columnsFromPlaced } from "./columns";

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
