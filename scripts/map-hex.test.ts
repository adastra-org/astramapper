import { describe, expect, test } from "bun:test";

import { cellsIn, lodClass, loadPerCell, proposeMerges, type AuditYmap } from "./map-hex";

function ymap(over: Partial<AuditYmap> & { at: [number, number]; half?: number }): AuditYmap {
  const h = over.half ?? 100;
  const [x, y] = over.at;
  return {
    file: over.file ?? `r/stream/${x}_${y}.ymap`,
    resource: over.resource ?? "[astra]/Map",
    name: over.name ?? `${x}_${y}`,
    streamed: over.streamed ?? true,
    vanillaOverride: over.vanillaOverride ?? false,
    streamingMin: [x - h, y - h, 0],
    streamingMax: [x + h, y + h, 50],
    entitiesMin: [x - 5, y - 5, 0],
    entitiesMax: [x + 5, y + 5, 10],
    entities: over.entities ?? 10,
    lods: over.lods ?? { hd: 10 },
    mloInstances: over.mloInstances ?? 0,
    issues: [],
    problem: null,
  };
}

describe("map-hex", () => {
  test("cellsIn covers an area with whole cells", () => {
    const cells = cellsIn({ min: [0, 0], max: [1000, 1000] }, 250);
    // 1 km² over cells of ~0.16 km² each: about six, a couple more at the edges.
    expect(cells.length).toBeGreaterThanOrEqual(6);
    expect(new Set(cells.map((c) => `${c.q},${c.r}`)).size).toBe(cells.length);
  });

  test("lodClass tells detail, coarse and mixed apart", () => {
    expect(lodClass(ymap({ at: [0, 0], lods: { hd: 1, "orphan-hd": 2 } }))).toBe("detail");
    expect(lodClass(ymap({ at: [0, 0], lods: { lod: 1, slod1: 2 } }))).toBe("coarse");
    expect(lodClass(ymap({ at: [0, 0], lods: { hd: 1, lod: 2 } }))).toBe("mixed");
  });

  test("a cell counts the boxes over its centre, overrides apart, unstreamed never", () => {
    const ys = [
      ymap({ at: [0, 0], half: 5000 }),
      ymap({ at: [0, 0], half: 5000, vanillaOverride: true }),
      ymap({ at: [0, 0], half: 5000, streamed: false }),
      ymap({ at: [4000, 7000], half: 10 }),
    ];
    const cells = loadPerCell(ys, 500);
    const max = Math.max(...cells.map((c) => c.added));
    expect(max).toBe(1);
    expect(Math.max(...cells.map((c) => c.overrides))).toBe(1);
  });

  test("merges group by resource, LOD class and cell, and respect the span cap", () => {
    const ys = [
      ymap({ at: [10, 10] }),
      ymap({ at: [20, 20] }),
      ymap({ at: [30, 30], resource: "[astra]/Other" }),
      ymap({ at: [15, 15], lods: { lod: 3 } }),
      ymap({ at: [25, 25], vanillaOverride: true }),
      ymap({ at: [12, 12], mloInstances: 1 }),
    ];
    const merges = proposeMerges(ys, 250, 1000);
    expect(merges).toHaveLength(1);
    expect(merges[0]?.files).toHaveLength(2);
    expect(merges[0]?.saved).toBe(1);
    expect(proposeMerges([ymap({ at: [10, 10], half: 900 }), ymap({ at: [20, 20] })], 250, 1000)).toHaveLength(0);
  });
});
