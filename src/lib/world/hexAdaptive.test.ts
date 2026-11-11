import { describe, expect, test } from "bun:test";
import {
  adaptiveLeafAt,
  adaptiveWorkingSet,
  buildAdaptive,
  tallyAdaptiveLoaded,
} from "./hexAdaptive";

/** A dense knot at the origin and a few strays far away. */
function world() {
  const xs: number[] = [];
  const ys: number[] = [];
  for (let i = 0; i < 400; i += 1) {
    xs.push((i % 20) * 8 - 80);
    ys.push(Math.floor(i / 20) * 8 - 80);
  }
  for (const [x, y] of [
    [5000, 5000],
    [5100, 5050],
    [-6000, 2000],
  ] as const) {
    xs.push(x);
    ys.push(y);
  }
  return {
    xs: Float64Array.from(xs),
    ys: Float64Array.from(ys),
    zs: new Float64Array(xs.length).fill(10),
  };
}
const sizes = [2000, 500, 125];

describe("adaptive cells", () => {
  test("every placement lands in exactly one leaf", () => {
    const { xs, ys, zs } = world();
    const grid = buildAdaptive(xs, ys, zs, sizes, 50);
    expect(grid.cells.reduce((sum, c) => sum + c.count, 0)).toBe(xs.length);
  });

  test("the dense knot is split finer than the empty country", () => {
    const { xs, ys, zs } = world();
    const grid = buildAdaptive(xs, ys, zs, sizes, 50);
    const city = adaptiveLeafAt(grid, 0, 0);
    const country = adaptiveLeafAt(grid, 5000, 5000);
    expect(city).not.toBeNull();
    expect(country).not.toBeNull();
    expect(city!.size).toBeLessThan(country!.size);
    expect(country!.level).toBe(0);
    expect(city!.z).toBe(10);
  });

  test("a cell over the threshold only survives at the finest level", () => {
    const { xs, ys, zs } = world();
    const grid = buildAdaptive(xs, ys, zs, sizes, 20);
    for (const cell of grid.cells) {
      if (cell.count > 20) expect(cell.level).toBe(sizes.length - 1);
    }
  });

  test("resident geometry is tallied into the leaf it falls in", () => {
    const { xs, ys, zs } = world();
    const grid = buildAdaptive(xs, ys, zs, sizes, 50);
    tallyAdaptiveLoaded(grid, [
      [0, 0, 0],
      [8, 8, 0],
      [5000, 5000, 0],
    ]);
    expect(grid.cells.reduce((sum, c) => sum + c.loaded, 0)).toBe(3);
  });

  test("the working set spends a placement budget, nearest cells first", () => {
    const { xs, ys, zs } = world();
    const grid = buildAdaptive(xs, ys, zs, sizes, 50);
    const small = adaptiveWorkingSet(grid, 0, 0, 100);
    const large = adaptiveWorkingSet(grid, 0, 0, 1000);
    expect(small.placements).toBeLessThanOrEqual(
      Math.max(100, adaptiveLeafAt(grid, 0, 0)!.count),
    );
    expect(large.placements).toBe(xs.length);
    expect(large.reach).toBeGreaterThan(small.reach);
    expect(small.cells.has(adaptiveLeafAt(grid, 0, 0)!)).toBe(true);
  });
});
