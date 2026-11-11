import { describe, expect, test } from "bun:test";

import { cellKey } from "./cellData";
import { DEFAULT_PLAN, planCells, travelHeading } from "./cellPlan";
import type { AdaptiveCell } from "./hexAdaptive";

let nextQ = 0;
/** A leaf at a point with a count; `q` only keeps keys distinct. */
function leaf(x: number, y: number, count: number): AdaptiveCell {
  nextQ += 1;
  return { level: 0, size: 100, q: nextQ, r: 0, x, y, z: 0, count, loaded: 0 };
}

/** A row of leaves along +x, one every 100 m, from the origin. */
function row(count: number, per: number): AdaptiveCell[] {
  return Array.from({ length: count }, (_, i) => leaf(i * 100, 0, per));
}

const params = { budget: 1000, keepFactor: 1.5, prefetchShare: 0.3, prefetchReach: 3 };

describe("planCells", () => {
  test("the working set is the nearest leaves until the budget is spent", () => {
    const leaves = row(20, 100);
    const plan = planCells(leaves, 0, 0, null, params);
    expect(plan.working).toHaveLength(10);
    expect(plan.placements).toBe(1000);
    expect(plan.working.map((c) => c.x)).toEqual(leaves.slice(0, 10).map((c) => c.x));
    expect(plan.reach).toBe(900);
  });

  test("the leaf under the camera is always taken, however crowded", () => {
    const plan = planCells([leaf(0, 0, 5000), leaf(100, 0, 1)], 0, 0, null, params);
    expect(plan.working).toHaveLength(1);
    expect(plan.working[0]!.count).toBe(5000);
  });

  test("dense country is held to a few hundred metres, sparse country reaches further", () => {
    const city = planCells(row(50, 200), 0, 0, null, params);
    const country = planCells(row(50, 20), 0, 0, null, params);
    expect(country.reach).toBeGreaterThan(city.reach);
    expect(city.placements).toBeLessThanOrEqual(params.budget);
    expect(country.placements).toBeLessThanOrEqual(params.budget);
  });

  test("the keep set is the working set plus slack, so a camera on a boundary does not thrash", () => {
    const leaves = row(30, 100);
    const plan = planCells(leaves, 0, 0, null, params);
    const kept = leaves.filter((c) => plan.keep.has(cellKey(c)));
    expect(kept).toHaveLength(15);
    for (const c of plan.working) expect(plan.keep.has(cellKey(c))).toBe(true);
  });

  test("with no heading the prefetch is the next nearest ring", () => {
    const leaves = row(30, 100);
    const plan = planCells(leaves, 0, 0, null, params);
    expect(plan.prefetch.map((c) => c.x)).toEqual([1000, 1100, 1200]);
  });

  test("the prefetch follows the direction of travel, and only reaches ahead", () => {
    // A cross of leaves around the camera, each 100 placements.
    const east = row(15, 100);
    const west = row(15, 100).map((c) => ({ ...c, q: c.q + 1000, x: -c.x - 100 }));
    const leaves = [...east, ...west];
    const flyingEast = planCells(leaves, 0, 0, [1, 0], {
      ...params,
      budget: 600,
      prefetchShare: 0.5,
      prefetchReach: 5,
    });
    expect(flyingEast.prefetch.length).toBeGreaterThan(0);
    for (const c of flyingEast.prefetch) expect(c.x).toBeGreaterThan(0);

    const flyingWest = planCells(leaves, 0, 0, [-1, 0], {
      ...params,
      budget: 600,
      prefetchShare: 0.5,
      prefetchReach: 5,
    });
    for (const c of flyingWest.prefetch) expect(c.x).toBeLessThan(0);
  });

  test("prefetch is capped by its share of the budget and by its reach", () => {
    const leaves = row(60, 100);
    const plan = planCells(leaves, 0, 0, [1, 0], { ...params, prefetchShare: 0.2, prefetchReach: 1.2 });
    const placements = plan.prefetch.reduce((sum, c) => sum + c.count, 0);
    expect(placements).toBeLessThanOrEqual(params.budget * 0.2);
    for (const c of plan.prefetch) expect(c.x).toBeLessThanOrEqual(plan.reach * 1.2);
    expect(planCells(leaves, 0, 0, [1, 0], { ...params, prefetchShare: 0 }).prefetch).toEqual([]);
  });

  test("a prefetched cell is kept, and nothing is both in the working set and the prefetch", () => {
    const leaves = row(30, 100);
    const plan = planCells(leaves, 0, 0, [1, 0], params);
    const working = new Set(plan.working.map(cellKey));
    for (const c of plan.prefetch) {
      expect(plan.keep.has(cellKey(c))).toBe(true);
      expect(working.has(cellKey(c))).toBe(false);
    }
  });

  test("an empty world plans nothing", () => {
    const plan = planCells([], 0, 0, [1, 0], DEFAULT_PLAN);
    expect(plan.working).toEqual([]);
    expect(plan.prefetch).toEqual([]);
    expect(plan.keep.size).toBe(0);
  });
});

describe("travelHeading", () => {
  test("a camera that moved heads the way it moved", () => {
    expect(travelHeading([0, 0], [10, 0], null, null)).toEqual([1, 0]);
    const [x, y] = travelHeading([0, 0], [0, -30], null, null)!;
    expect(x).toBeCloseTo(0);
    expect(y).toBeCloseTo(-1);
  });

  test("a camera at rest heads where it looks, then where it last went", () => {
    expect(travelHeading([0, 0], [1, 1], [0, 1], [1, 0])).toEqual([0, 1]);
    expect(travelHeading([0, 0], [1, 1], null, [1, 0])).toEqual([1, 0]);
    expect(travelHeading(null, [0, 0], null, null)).toBeNull();
    expect(travelHeading(null, [0, 0], [0, 0], null)).toBeNull();
  });

  test("a turn is smoothed against the last heading, but a reversal is not stuck", () => {
    const turned = travelHeading([0, 0], [0, 100], null, [1, 0], { smoothing: 0.5 })!;
    expect(turned[0]).toBeCloseTo(Math.SQRT1_2);
    expect(turned[1]).toBeCloseTo(Math.SQRT1_2);
    // Exactly opposite headings cancel; the newer one wins.
    expect(travelHeading([0, 0], [-50, 0], null, [1, 0], { smoothing: 0.5 })).toEqual([-1, 0]);
  });

  test("jitter below the minimum move is not a direction", () => {
    expect(travelHeading([0, 0], [1, 1], null, [0, 1], { minMove: 5 })).toEqual([0, 1]);
  });
});
