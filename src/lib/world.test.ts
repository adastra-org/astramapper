import { describe, expect, test } from "bun:test";
import {
  boundsOf,
  colorForLod,
  demoWorld,
  gtaToWorld,
  keyFor,
  lodBandForDistance,
  nearestCandidates,
  placementScale,
  worldToGta,
  type Placed,
} from "./world";

/** A map entry with one placed entity per position, for the streamer tests. */
function entry(key: string, positions: readonly (readonly [number, number, number])[]): {
  key: string;
  placements: Placed[];
} {
  return {
    key,
    placements: positions.map((position, index) => ({
      index,
      name: "x",
      nameHash: 0,
      isMlo: false,
      position,
      rotation: [0, 0, 0, 1],
      scaleXy: 1,
      scaleZ: 1,
      parentIndex: -1,
      lodLevel: "HD",
      lodDist: 100,
      children: 0,
    })),
  };
}

/**
 * The `invoke` wrappers need a Tauri host to say anything, so only the pure
 * geometry and colour helpers are covered here; the commands behind the
 * wrappers are exercised by `src-tauri/tests/world_gate.rs`.
 */
describe("placementScale", () => {
  test("maps the ground-plane scale onto the world's X and Z", () => {
    expect(placementScale(2, 5)).toEqual([2, 5, 2]);
  });

  test("keeps a uniform scale uniform", () => {
    expect(placementScale(3, 3)).toEqual([3, 3, 3]);
  });
});

describe("gtaToWorld", () => {
  test("raises the Z-up axis into three.js's Y-up world", () => {
    expect(gtaToWorld([1, 2, 3])).toEqual([1, 3, -2]);
  });

  test("keeps the ground plane flat", () => {
    const world = gtaToWorld([0, 0, 0]);
    // `toBe` treats `-0` and `0` as different, and the sign is irrelevant here.
    expect(world[0] === 0).toBe(true);
    expect(world[1] === 0).toBe(true);
    expect(world[2] === 0).toBe(true);
  });

  test("round-trips with worldToGta", () => {
    const gta: [number, number, number] = [12.5, -3.25, 700];
    expect(worldToGta(gtaToWorld(gta))).toEqual(gta);
  });
});

describe("keyFor", () => {
  test("joins a map and an entity into a stable identity", () => {
    expect(keyFor("m", 0)).toBe("m:0");
    expect(keyFor("m", 12)).toBe("m:12");
  });
});

describe("lodBandForDistance", () => {
  test("uses the entity's LOD distance as the HD boundary", () => {
    const lodDist = 100;
    expect(lodBandForDistance(10, lodDist, 4)).toBe(0);
    expect(lodBandForDistance(100, lodDist, 4)).toBe(0);
    expect(lodBandForDistance(120, lodDist, 4)).toBe(1);
    expect(lodBandForDistance(200, lodDist, 4)).toBe(1);
    expect(lodBandForDistance(300, lodDist, 4)).toBe(2);
    expect(lodBandForDistance(1000, lodDist, 4)).toBe(3);
  });

  test("falls back to fixed bands without a LOD distance", () => {
    expect(lodBandForDistance(50, 0, 4)).toBe(0);
    expect(lodBandForDistance(200, 0, 4)).toBe(1);
    expect(lodBandForDistance(500, 0, 4)).toBe(2);
    expect(lodBandForDistance(900, 0, 4)).toBe(2);
    expect(lodBandForDistance(5000, 0, 4)).toBe(3);
  });

  test("clamps to the bundles the model actually has", () => {
    expect(lodBandForDistance(100000, 100, 2)).toBe(1);
    expect(lodBandForDistance(100000, 100, 1)).toBe(0);
  });
});

describe("nearestCandidates", () => {
  const entries = [
    entry("m1", [
      [0, 0, 0],
      [100, 0, 0],
    ]),
    entry("m2", [[30, 0, 0]]),
  ];

  test("returns the nearest unexcluded entities, nearest first", () => {
    const got = nearestCandidates([0, 0, 0], entries, new Set(), 200, 10);
    // Per-map indexes: m1's second entity (index 1) is farthest.
    expect(got.map((c) => c.entityIndex)).toEqual([0, 0, 1]);
    expect(got[0]).toMatchObject({ mapKey: "m1", entityIndex: 0 });
  });

  test("honours the excluded set", () => {
    const got = nearestCandidates([0, 0, 0], entries, new Set(["m1:0"]), 200, 10);
    expect(got.map((c) => c.entityIndex)).toEqual([0, 1]);
  });

  test("culls by radius and caps by limit", () => {
    const near = nearestCandidates([0, 0, 0], entries, new Set(), 50, 10);
    expect(near.map((c) => c.entityIndex)).toEqual([0, 0]);
    const capped = nearestCandidates([0, 0, 0], entries, new Set(), 200, 1);
    expect(capped).toHaveLength(1);
  });
});

describe("colorForLod", () => {
  test("steps the brightness down as LOD distance grows", () => {
    const hd = colorForLod("HD", false);
    const lod = colorForLod("LOD", false);
    const slod = colorForLod("SLOD1", false);
    expect(hd).not.toBe(lod);
    expect(lod).not.toBe(slod);
  });

  test("gives interior instances their own hue over the LOD colour", () => {
    expect(colorForLod("HD", true)).toBe(colorForLod("LOD", true));
    expect(colorForLod("HD", true)).not.toBe(colorForLod("HD", false));
  });

  test("has a stable colour for unknown LOD labels", () => {
    expect(colorForLod("", false)).toBe(colorForLod("weird", false));
  });
});

describe("boundsOf", () => {
  test("is a single point for one placement", () => {
    const { min, max, center, radius } = boundsOf([{ position: [1, 2, 3] }]);
    expect(min).toEqual([1, 2, 3]);
    expect(max).toEqual([1, 2, 3]);
    expect(center).toEqual([1, 2, 3]);
    expect(radius).toBeGreaterThanOrEqual(1);
  });

  test("is degenerate but finite for nothing", () => {
    const { center, radius } = boundsOf([]);
    expect(center).toEqual([0, 0, 0]);
    expect(radius).toBe(1);
  });

  test("contains every placement", () => {
    const placements = [
      { position: [-10, -20, -30] as const },
      { position: [5, 6, 7] as const },
    ];
    const { min, max, center } = boundsOf(placements);
    expect(min).toEqual([-10, -20, -30]);
    expect(max).toEqual([5, 6, 7]);
    expect(center).toEqual([-2.5, -7, -11.5]);
  });
});

describe("demoWorld", () => {
  test("renders a deterministic, non-empty plaza", () => {
    const a = demoWorld();
    const b = demoWorld();
    expect(a.name).toBe("demo_plaza");
    expect(a.placements.length).toBeGreaterThan(0);
    expect(a.placements.length).toBe(b.placements.length);
    expect(a.placements[0]).toEqual(b.placements[0]);
  });

  test("every placement is finite and standing on the ground", () => {
    const { placements } = demoWorld();
    for (const placement of placements) {
      expect(placement.position.every(Number.isFinite)).toBe(true);
      expect(placement.scaleXy).toBeGreaterThan(0);
      expect(placement.scaleZ).toBeGreaterThan(0);
      expect(placement.position[2] + placement.scaleZ / 2).toBeGreaterThanOrEqual(0);
    }
  });

  test("has an interior marker so the MLO colour shows", () => {
    const { placements } = demoWorld();
    expect(placements.some((p) => p.isMlo)).toBe(true);
  });
});
