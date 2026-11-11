import { describe, expect, test } from "bun:test";
import { ModelCache, modelBudgetBytes } from "./modelCache";

const MB = 1024 * 1024;
const model = (mb: number): { byteLength: number } => ({ byteLength: mb * MB });

describe("ModelCache", () => {
  test("counts bytes as models come and go", () => {
    const cache = new ModelCache();
    cache.set("a", model(2));
    cache.set("b", model(3));
    expect(cache.bytes).toBe(5 * MB);
    cache.set("a", model(1));
    expect(cache.bytes).toBe(4 * MB);
  });

  test("does nothing while under budget", () => {
    const cache = new ModelCache();
    cache.set("a", model(2), [0, 0, 0]);
    expect(cache.evict(10 * MB, [0, 0, 0], new Set())).toEqual([]);
    expect(cache.has("a")).toBe(true);
  });

  test("evicts the farthest first, until it fits", () => {
    const cache = new ModelCache();
    cache.set("near", model(4), [10, 0, 0]);
    cache.set("mid", model(4), [500, 0, 0]);
    cache.set("far", model(4), [5000, 0, 0]);
    const dropped = cache.evict(8 * MB, [0, 0, 0], new Set());
    expect(dropped).toEqual(["far"]);
    expect(cache.bytes).toBe(8 * MB);
    expect(cache.has("near") && cache.has("mid")).toBe(true);
  });

  test("distance is measured from the camera now, not from when it was cached", () => {
    const cache = new ModelCache();
    cache.set("west", model(4), [-3000, 0, 0]);
    cache.set("east", model(4), [3000, 0, 0]);
    expect(cache.evict(4 * MB, [-3000, 0, 0], new Set())).toEqual(["east"]);
  });

  test("never evicts a model something draws, and stops when only those are left", () => {
    const cache = new ModelCache();
    cache.set("drawn", model(6), [9000, 0, 0]);
    cache.set("spare", model(6), [10, 0, 0]);
    const dropped = cache.evict(1 * MB, [0, 0, 0], new Set(["drawn"]));
    expect(dropped).toEqual(["spare"]);
    expect(cache.has("drawn")).toBe(true);
    expect(cache.bytes).toBe(6 * MB);
  });

  test("a model nothing placed goes before any that was", () => {
    const cache = new ModelCache();
    cache.set("placed", model(4), [100, 0, 0]);
    cache.set("orphan", model(4));
    expect(cache.evict(4 * MB, [0, 0, 0], new Set())).toEqual(["orphan"]);
  });

  test("touch moves a model's anchor, so a re-used model is kept over a stale one", () => {
    const cache = new ModelCache();
    cache.set("reused", model(4), [8000, 0, 0]);
    cache.set("stale", model(4), [1000, 0, 0]);
    cache.touch("reused", [20, 0, 0]);
    expect(cache.evict(4 * MB, [0, 0, 0], new Set())).toEqual(["stale"]);
  });

  test("clear empties it", () => {
    const cache = new ModelCache();
    cache.set("a", model(2));
    cache.clear();
    expect(cache.bytes).toBe(0);
    expect(cache.size).toBe(0);
  });
});

describe("modelBudgetBytes", () => {
  const gb = (n: number): number => n * 1024 * MB;

  test("is a fifth of what is free, capped at a tenth of the total", () => {
    expect(modelBudgetBytes({ total: gb(32), available: gb(10) })).toBe(gb(2));
    expect(modelBudgetBytes({ total: gb(16), available: gb(14) })).toBe(Math.round(gb(1.6)));
  });

  test("never drops below 256 MB or rises above 3 GB", () => {
    expect(modelBudgetBytes({ total: gb(8), available: gb(0.5) })).toBe(256 * MB);
    expect(modelBudgetBytes({ total: gb(256), available: gb(200) })).toBe(gb(3));
  });
});
