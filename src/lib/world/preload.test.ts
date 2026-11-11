import { describe, expect, test } from "bun:test";

import { isLoadable, preloadableCount, preloadTargets } from "./preload";
import type { Placed, RegionEntry } from "../world";
import { columnsFromPlaced } from "./columns";

function placement(index: number, over: Partial<Placed> = {}): Placed {
  return {
    index,
    name: `prop_${index}`,
    nameHash: index,
    isMlo: false,
    fromMlo: false,
    position: [index * 10, 0, 0],
    rotation: [0, 0, 0, 1],
    scaleXy: 1,
    scaleZ: 1,
    parentIndex: -1,
    lodLevel: "HD",
    lodDist: 100,
    children: 0,
    ...over,
  };
}

function region(key: string, placements: readonly Placed[]): RegionEntry {
  return { key, extentsMin: [0, 0, 0], extentsMax: [1000, 1000, 1000], placements: columnsFromPlaced(placements) };
}

const entries = [
  region("a.ymap", [placement(0), placement(1), placement(2)]),
  region("b.ymap", [placement(3), placement(4)]),
];

describe("preloadTargets", () => {
  test("covers every entity across every placed map", () => {
    const targets = preloadTargets(entries, new Set(), null, 100);
    expect(targets).toHaveLength(5);
    expect(new Set(targets.map((t) => t.mapKey))).toEqual(new Set(["a.ymap", "b.ymap"]));
  });

  test("skips what is already loaded or in flight", () => {
    const targets = preloadTargets(entries, new Set(["a.ymap:0", "b.ymap:3"]), null, 100);
    expect(targets.map((t) => `${t.mapKey}:${t.entity.index}`)).toEqual([
      "a.ymap:1",
      "a.ymap:2",
      "b.ymap:4",
    ]);
  });

  /**
   * A name the table could not resolve has no file behind it. Counting it would
   * make the progress bar promise work that can never happen.
   */
  test("skips entities whose archetype name never resolved", () => {
    const unresolved = [region("a.ymap", [placement(0), placement(1, { name: "hash_1a2b3c4d" })])];
    expect(preloadTargets(unresolved, new Set(), null, 100)).toHaveLength(1);
    expect(isLoadable(placement(9, { name: "hash_00000000" }))).toBe(false);
  });

  /** The cap is the only thing bounding a preload, so it has to hold exactly. */
  test("never returns more than the cap", () => {
    expect(preloadTargets(entries, new Set(), null, 2)).toHaveLength(2);
    expect(preloadTargets(entries, new Set(), null, 0)).toHaveLength(0);
    expect(preloadTargets(entries, new Set(), null, -5)).toHaveLength(0);
  });

  /** What the cap keeps has to be what is in front of you, not what sorted first. */
  test("orders nearest to the camera first, so a cap keeps what matters", () => {
    const targets = preloadTargets(entries, new Set(), [45, 0, 0], 3);
    expect(targets.map((t) => t.entity.index)).toEqual([4, 3, 2]);
    expect(targets[0].distance).toBeCloseTo(5);
  });

  test("falls back to placement order before the camera has reported", () => {
    const targets = preloadTargets(entries, new Set(), null, 100);
    expect(targets.map((t) => t.entity.index)).toEqual([0, 1, 2, 3, 4]);
    expect(targets.every((t) => t.distance === 0)).toBe(true);
  });

  test("is stable across runs", () => {
    const first = preloadTargets(entries, new Set(), [45, 0, 0], 5);
    const second = preloadTargets(entries, new Set(), [45, 0, 0], 5);
    expect(second).toEqual(first);
  });
});

describe("preloadableCount", () => {
  test("counts what a preload could ever reach, ignoring the cap", () => {
    expect(preloadableCount(entries)).toBe(5);
  });

  test("does not count unresolvable names", () => {
    expect(preloadableCount([region("a.ymap", [placement(0, { name: "hash_1" })])])).toBe(0);
  });

  test("is zero with nothing placed", () => {
    expect(preloadableCount([])).toBe(0);
  });
});
