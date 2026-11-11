import { describe, expect, test } from "bun:test";
import { columnsFromPlaced } from "./columns";
import {
  binPlacements,
  hexCenter,
  hexCorners,
  hexDisk,
  hexDiskSize,
  hexDistance,
  hexKey,
  hexStats,
  worldToHex,
} from "./hex";

const at = (x: number, y: number) => ({
  index: 0,
  name: "x",
  nameHash: 0,
  isMlo: false,
  fromMlo: false,
  position: [x, y, 0] as [number, number, number],
  rotation: [0, 0, 0, 1] as [number, number, number, number],
  scaleXy: 1,
  scaleZ: 1,
  parentIndex: -1,
  lodLevel: "HD",
  lodDist: 100,
  children: 0,
});

describe("hex maths", () => {
  test("a cell's centre maps back to the same cell", () => {
    for (const hex of [
      { q: 0, r: 0 },
      { q: 3, r: -2 },
      { q: -5, r: 7 },
    ]) {
      const [x, y] = hexCenter(hex, 250);
      expect(worldToHex(x, y, 250)).toEqual(hex);
    }
  });

  test("points near a corner still land in a neighbouring cell consistently", () => {
    const [x, y] = hexCenter({ q: 1, r: 0 }, 100);
    // Just inside the cell on each side of its centre.
    expect(worldToHex(x + 40, y, 100)).toEqual({ q: 1, r: 0 });
    expect(worldToHex(x - 40, y, 100)).toEqual({ q: 1, r: 0 });
  });

  test("the six corners are all one radius from the centre", () => {
    const hex = { q: 2, r: -1 };
    const [cx, cy] = hexCenter(hex, 80);
    const corners = hexCorners(hex, 80);
    expect(corners).toHaveLength(6);
    for (const [x, y] of corners) {
      expect(Math.hypot(x - cx, y - cy)).toBeCloseTo(80, 6);
    }
  });

  test("distance counts steps between cells", () => {
    expect(hexDistance({ q: 0, r: 0 }, { q: 0, r: 0 })).toBe(0);
    expect(hexDistance({ q: 0, r: 0 }, { q: 1, r: 0 })).toBe(1);
    expect(hexDistance({ q: 0, r: 0 }, { q: 2, r: -1 })).toBe(2);
    expect(hexDistance({ q: -2, r: 3 }, { q: 3, r: -1 })).toBe(5);
  });

  test("a disk holds 1, 7, 19, 37, 61 cells and every one is within range", () => {
    const center = { q: 4, r: -3 };
    for (let radius = 0; radius <= 4; radius += 1) {
      const disk = hexDisk(center, radius);
      expect(disk).toHaveLength(hexDiskSize(radius));
      expect(disk.every((h) => hexDistance(center, h) <= radius)).toBe(true);
      expect(new Set(disk.map((h) => hexKey(h.q, h.r))).size).toBe(disk.length);
    }
    expect(hexDiskSize(4)).toBe(61);
  });
});

describe("binning", () => {
  test("counts placements per cell across entries", () => {
    const cells = binPlacements(
      [
        { placements: columnsFromPlaced([at(0, 0), at(10, 10), at(2000, 0)]) },
        { placements: columnsFromPlaced([at(5, -5)]) },
      ],
      250,
    );
    const origin = cells.get(hexKey(0, 0));
    expect(origin?.count).toBe(3);
    expect(cells.size).toBe(2);
  });

  test("stats say how much a disk would hold against the whole", () => {
    const cells = binPlacements(
      [
        {
          placements: columnsFromPlaced([at(0, 0), at(5, 5), at(3000, 0), at(-3000, 0), at(0, 3000)]),
        },
      ],
      250,
    );
    const stats = hexStats(cells, { q: 0, r: 0 }, 2);
    expect(stats.placements).toBe(5);
    expect(stats.placementsInDisk).toBe(2);
    expect(stats.cellsInDisk).toBe(1);
    expect(stats.maxCount).toBe(2);
  });
});
