import { describe, expect, test } from "bun:test";

import { Kind, readEnvelope } from "../wire";
import { STREAM_PHASE_PENALTY, streamPenaltyOf } from "../world";
import {
  adaptiveGridOf,
  cellKey,
  decodeCellBatch,
  decodeCellGrid,
  gridBounds,
  TERRAIN_KEY,
  TERRAIN_LEVEL,
} from "./cellData";
import { batchBuffer, buildEnvelope, emptyModelEnvelope, gridBuffer, u8 } from "./cellFixtures";
import { hexCenter } from "./hex";
import { adaptiveLeafAt } from "./hexAdaptive";

describe("the partition", () => {
  const sizes = [800, 400, 200];

  test("every leaf comes back with its cell, its centre and its count", () => {
    const grid = decodeCellGrid(
      gridBuffer(sizes, [
        { level: 0, q: 1, r: -2, count: 40, z: 12.5 },
        { level: 2, q: 3, r: 4, count: 900 },
      ]),
    );
    expect(grid.group).toBe("base");
    expect(grid.sizes).toEqual(sizes);
    expect(grid.leaves).toHaveLength(2);

    const [first, second] = grid.leaves;
    expect(first).toMatchObject({ level: 0, size: 800, q: 1, r: -2, count: 40, z: 12.5 });
    const [x, y] = hexCenter({ q: 1, r: -2 }, 800);
    expect([first!.x, first!.y]).toEqual([x, y]);
    expect(second).toMatchObject({ level: 2, size: 200, count: 900 });
  });

  test("a leaf at a level past the sizes is refused", () => {
    expect(() => decodeCellGrid(gridBuffer(sizes, [{ level: 7, q: 0, r: 0, count: 1 }]))).toThrow();
  });

  test("the wrong kind is refused", () => {
    expect(() => decodeCellGrid(buildEnvelope(Kind.Model, {}, []))).toThrow();
  });

  test("the Hex view's grid finds a leaf by point and tells split from empty", () => {
    const grid = decodeCellGrid(
      gridBuffer(
        sizes,
        [
          { level: 1, q: 0, r: 0, count: 10 },
          { level: 0, q: 5, r: 5, count: 3 },
        ],
        [{ level: 0, q: 0, r: 0 }],
      ),
    );
    const adaptive = adaptiveGridOf(grid);
    expect(adaptive.cells).toBe(grid.leaves);
    expect(adaptiveLeafAt(adaptive, 0, 0)).toBe(grid.leaves[0]!);
    const [fx, fy] = hexCenter({ q: 5, r: 5 }, 800);
    expect(adaptiveLeafAt(adaptive, fx, fy)).toBe(grid.leaves[1]!);
    // Nothing was ever counted far away.
    expect(adaptiveLeafAt(adaptive, 90_000, 90_000)).toBeNull();
  });

  test("the footprint is framed from the leaves, weighted by their height", () => {
    const grid = decodeCellGrid(
      gridBuffer(sizes, [
        { level: 0, q: 0, r: 0, count: 1, z: 0 },
        { level: 0, q: 2, r: 0, count: 3, z: 40 },
      ]),
    );
    const { center, radius } = gridBounds(grid);
    const [x2] = hexCenter({ q: 2, r: 0 }, 800);
    expect(center[0]).toBeCloseTo(x2 / 2);
    expect(center[2]).toBeCloseTo(30);
    expect(radius).toBeCloseTo(x2 / 2);
    expect(gridBounds(decodeCellGrid(gridBuffer(sizes, [])))).toEqual({
      center: [0, 0, 0],
      radius: 0,
    });
  });
});

describe("a cell batch", () => {
  const maps = [
    { path: "a.ymap", name: "a" },
    { path: "b.ymap", name: "b" },
  ];

  test("placements are split into one part per map, per cell", () => {
    const batch = decodeCellBatch(
      batchBuffer({
        cells: [
          { level: 1, q: 2, r: 3, start: 0, count: 3 },
          { level: TERRAIN_LEVEL, q: 0, r: 0, start: 3, count: 1 },
        ],
        rows: [
          { map: 0, index: 10, name: 0, x: 1, y: 2, z: 3 },
          { map: 1, index: 4, name: 1, x: 5, y: 6, z: 7 },
          { map: 0, index: 11, name: 0, x: 8, y: 9, z: 10 },
          { map: 1, index: 0, name: 1, x: -1, y: -2, z: -3 },
        ],
        maps,
        names: ["lamp", "bin"],
      }),
    );

    expect(batch.cells.map((c) => c.key)).toEqual([cellKey({ level: 1, q: 2, r: 3 }), TERRAIN_KEY]);
    const [street, ground] = batch.cells;
    expect(street!.count).toBe(3);
    expect(street!.parts.map((p) => [p.mapKey, p.columns.length])).toEqual([
      ["a.ymap", 2],
      ["b.ymap", 1],
    ]);
    const a = street!.parts[0]!.columns;
    expect(a.entityIndex(0)).toBe(10);
    expect(a.entityIndex(1)).toBe(11);
    expect([a.x(1), a.y(1), a.z(1)]).toEqual([8, 9, 10]);
    expect(a.name(0)).toBe("lamp");
    expect(street!.parts[1]!.columns.name(0)).toBe("bin");
    expect(street!.parts[0]!.mapName).toBe("a");
    expect(ground!.parts).toHaveLength(1);
    expect(ground!.parts[0]!.mapKey).toBe("b.ymap");
  });

  test("a part owns its rows: it does not point into the reply", () => {
    const buffer = batchBuffer({
      cells: [{ level: 0, q: 0, r: 0, start: 0, count: 1 }],
      rows: [{ map: 0, index: 1, name: 0, x: 4, y: 5, z: 6 }],
      maps,
      names: ["lamp"],
    });
    const batch = decodeCellBatch(buffer);
    const columns = batch.cells[0]!.parts[0]!.columns;
    expect(columns.positions.buffer).not.toBe(buffer);
    new Uint8Array(buffer).fill(0);
    expect(columns.x(0)).toBe(4);
  });

  test("an empty cell is a cell with no parts", () => {
    const batch = decodeCellBatch(
      batchBuffer({
        cells: [{ level: 0, q: 9, r: 9, start: 0, count: 0 }],
        rows: [],
        maps,
        names: [],
      }),
    );
    expect(batch.cells).toHaveLength(1);
    expect(batch.cells[0]!.parts).toEqual([]);
    expect(batch.cells[0]!.count).toBe(0);
  });

  test("assets are handed through, including a name with no file", () => {
    const batch = decodeCellBatch(
      batchBuffer({
        cells: [],
        rows: [],
        maps,
        names: [],
        assets: [
          { name: "lamp", path: "models\\lamp.ydr" },
          { name: "hash_1234abcd", path: null },
        ],
      }),
    );
    expect(batch.assets).toEqual([
      { name: "lamp", path: "models\\lamp.ydr" },
      { name: "hash_1234abcd", path: null },
    ]);
  });

  test("a nested model is decoded in place, aligned, from the reply's own buffer", () => {
    const seen: { offset: number; length: number; path: string }[] = [];
    const buffer = batchBuffer({
      cells: [{ level: 0, q: 0, r: 0, start: 0, count: 1 }],
      rows: [{ map: 0, index: 1, name: 0, x: 0, y: 0 }],
      maps,
      names: ["lamp"],
      models: [
        { path: "models\\lamp.ydr", envelope: emptyModelEnvelope("models\\lamp.ydr") },
        { path: "models\\bin.ydr", envelope: emptyModelEnvelope("models\\bin.ydr") },
      ],
    });
    const batch = decodeCellBatch(buffer, (outer, offset, length, path) => {
      seen.push({ offset, length, path });
      // The nested envelope reads as itself, without a copy.
      const inner = readEnvelope(outer, offset, length);
      expect(inner.kind).toBe(Kind.Model);
      expect((inner.meta as { path: string }).path).toBe(path);
      expect([...inner.section(0)!]).toEqual([1, 2, 3, 4]);
      expect(inner.section(0)!.byteOffset % 16).toBe(0);
      return { path, byteLength: length } as never;
    });

    expect(seen.map((s) => s.path)).toEqual(["models\\lamp.ydr", "models\\bin.ydr"]);
    for (const s of seen) expect(s.offset % 16).toBe(0);
    expect([...batch.models.keys()]).toEqual(["models\\lamp.ydr", "models\\bin.ydr"]);
  });

  test("the real model decoder reads a nested model", () => {
    const batch = decodeCellBatch(
      batchBuffer({
        cells: [],
        rows: [],
        maps,
        names: [],
        models: [{ path: "m.ydr", envelope: emptyModelEnvelope("m.ydr") }],
      }),
    );
    const model = batch.models.get("m.ydr")!;
    expect(model.path).toBe("m.ydr");
    expect(model.drawables).toEqual([]);
    expect(model.byteLength).toBe(emptyModelEnvelope("m.ydr").byteLength);
  });

  test("the wrong kind, and a short column, are refused", () => {
    expect(() => decodeCellBatch(buildEnvelope(Kind.Model, {}, []))).toThrow();
    expect(() =>
      decodeCellBatch(
        buildEnvelope(
          Kind.CellData,
          {
            cells: [],
            count: 2,
            maps: [],
            names: [],
            nameHashes: [],
            lodLevels: [],
            assets: [],
            models: [],
          },
          Array.from({ length: 12 }, () => u8([])),
        ),
      ),
    ).toThrow();
  });
});

describe("the ground layer", () => {
  /**
   * The backend sets terrain aside by the same name rule `streamPenaltyOf`
   * applies here (`is_terrain_name` in `world_index.rs` pins the same names), so
   * a placement is in the ground layer exactly when the streamer would rank it
   * as terrain.
   */
  test("the names the backend calls ground are the names the streamer calls terrain", () => {
    for (const name of ["terrain_01", "Ter_Road", "grnd_a", "prop_terr_x", "x_TERRAIN"]) {
      expect(streamPenaltyOf(name, "HD")).toBe(STREAM_PHASE_PENALTY.terrain);
    }
    for (const name of ["prop_terrace", "interior_a", "prop_lamp", "hash_1234abcd"]) {
      expect(streamPenaltyOf(name, "HD")).not.toBe(STREAM_PHASE_PENALTY.terrain);
    }
  });
});
