import { describe, expect, test } from "bun:test";
import { grownReach, terrainBudget, trimTerrain, type GroundPiece } from "./terrainBudget";

const MB = 1024 * 1024;
const piece = (key: string, distance: number, bytes: number, modelPath = key): GroundPiece => ({
  key,
  distance,
  bytes: bytes * MB,
  modelPath,
});

describe("trimTerrain", () => {
  test("leaves the ground alone while it fits", () => {
    const trim = trimTerrain([piece("a", 10, 40), piece("b", 5000, 40)], 100 * MB);
    expect(trim.drop).toEqual([]);
    expect(trim.reach).toBeNull();
    expect(trim.bytes).toBe(80 * MB);
  });

  test("past the budget the farthest go first, and the reach is the nearest dropped", () => {
    const pieces = [
      piece("far", 3000, 40),
      piece("near", 100, 40),
      piece("mid", 900, 40),
      piece("farther", 6000, 40),
    ];
    const trim = trimTerrain(pieces, 100 * MB);
    // 160 MB loaded, a 100 MB budget, trimmed under 90 MB: two fit.
    expect(trim.drop).toEqual(["farther", "far"]);
    expect(trim.reach).toBe(3000);
  });

  test("a model many pieces share is counted once, and a piece sharing a kept model stays", () => {
    const pieces = [
      piece("rock1", 10, 30, "rock"),
      piece("rock2", 20, 30, "rock"),
      piece("rock3", 9000, 30, "rock"),
      piece("land", 50, 70, "land"),
    ];
    // 100 MB distinct against an 80 MB budget: the land (70) and the rock (30)
    // cannot both fit under 72 MB, so the farther of the two models goes whole.
    const trim = trimTerrain(pieces, 80 * MB);
    expect(trim.bytes).toBe(100 * MB);
    expect(trim.drop).toEqual(["land"]);
    expect(trim.reach).toBe(50);
  });

  test("a ground piece larger than the whole budget is dropped, not looped on", () => {
    const trim = trimTerrain([piece("huge", 10, 500)], 100 * MB);
    expect(trim.drop).toEqual(["huge"]);
    expect(trim.reach).toBe(10);
  });
});

describe("reach and budget", () => {
  test("the reach only grows with room to spare, and never from unlimited", () => {
    expect(grownReach(Infinity, 0, 100)).toBe(Infinity);
    expect(grownReach(1000, 90, 100)).toBe(1000);
    expect(grownReach(1000, 10, 100)).toBeGreaterThan(1000);
  });

  test("the ground's budget follows the model cache's, within a floor and a ceiling", () => {
    expect(terrainBudget(100 * MB)).toBe(256 * MB);
    expect(terrainBudget(1000 * MB)).toBe(500 * MB);
    expect(terrainBudget(10_000 * MB)).toBe(1024 * MB);
  });
});
