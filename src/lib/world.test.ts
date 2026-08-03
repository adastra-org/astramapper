import { describe, expect, test } from "bun:test";
import {
  applyEdit,
  axisRotate,
  boundsOf,
  colorForLod,
  commitHistory,
  demoWorld,
  editOf,
  gtaToWorld,
  keyFor,
  lodBandForDistance,
  nearestCandidates,
  placementScale,
  popHistory,
  worldToGta,
  type Placed,
  type TransformEdit,
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

describe("applyEdit and editOf", () => {
  const placement: Placed = {
    index: 0,
    name: "x",
    nameHash: 0,
    isMlo: false,
    position: [1, 2, 3],
    rotation: [0, 0, 0, 1],
    scaleXy: 1,
    scaleZ: 1,
    parentIndex: -1,
    lodLevel: "HD",
    lodDist: 100,
    children: 0,
  };

  test("is the identity for no edit", () => {
    expect(applyEdit(placement, undefined)).toBe(placement);
  });

  test("replaces the stored transform when edited", () => {
    const edit: TransformEdit = {
      position: [10, 20, 30],
      rotation: [0, 0, 1, 0],
      scaleXy: 2,
      scaleZ: 3,
    };
    const edited = applyEdit(placement, edit);
    expect(edited.position).toEqual([10, 20, 30]);
    expect(edited.rotation).toEqual([0, 0, 1, 0]);
    expect(edited.scaleXy).toBe(2);
    expect(edited.scaleZ).toBe(3);
    expect(edited.name).toBe("x");
    expect(placement.position).toEqual([1, 2, 3]);
  });

  test("round-trips through editOf", () => {
    const edit = editOf(placement);
    expect(applyEdit(placement, edit)).toEqual(placement);
  });
});

describe("axisRotate", () => {
  test("rotates the identity around Z", () => {
    const q = axisRotate([0, 0, 0, 1], [0, 0, 1], Math.PI / 2);
    expect(q[0]).toBeCloseTo(0, 5);
    expect(q[1]).toBeCloseTo(0, 5);
    expect(q[2]).toBeCloseTo(Math.SQRT1_2, 5);
    expect(q[3]).toBeCloseTo(Math.SQRT1_2, 5);
  });

  test("is the identity for a zero angle", () => {
    const q = axisRotate([0.1, 0.2, 0.3, 0.9], [1, 0, 0], 0);
    expect(q).toEqual([0.1, 0.2, 0.3, 0.9]);
  });

  test("stays a unit quaternion", () => {
    const start: [number, number, number, number] = [0, 0, Math.SQRT1_2, Math.SQRT1_2];
    const q = axisRotate(start, [0, 1, 0], 0.7);
    const norm = Math.hypot(q[0], q[1], q[2], q[3]);
    expect(norm).toBeCloseTo(1, 5);
  });
});

describe("history", () => {
  test("committing appends and clears redo", () => {
    const undo = commitHistory([], step("a", editOfPlace(0), editOfPlace(1)));
    const undone = popHistory(undo);
    expect(undone.entry?.after.position).toEqual([1, 0, 0]);
    expect(undone.stack).toHaveLength(0);
  });

  test("popping an empty stack yields nothing", () => {
    expect(popHistory([])).toEqual({ stack: [], entry: null });
  });
});

/** A one-axis move for building history fixtures. */
function editOfPlace(x: number): TransformEdit {
  return { position: [x, 0, 0], rotation: [0, 0, 0, 1], scaleXy: 1, scaleZ: 1 };
}

/** A history entry fixture. */
function step(key: string, before: TransformEdit, after: TransformEdit): {
  key: string;
  before: TransformEdit;
  after: TransformEdit;
} {
  return { key, before, after };
}
