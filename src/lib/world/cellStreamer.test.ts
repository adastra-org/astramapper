import { describe, expect, test } from "bun:test";

import {
  cellKey,
  TERRAIN_CELL,
  TERRAIN_KEY,
  type CellGrid,
  type CellRef,
  type DecodedBatch,
  type DecodedCell,
} from "./cellData";
import { CellStreamer, RETRY_MS } from "./cellStreamer";
import { columnsFromPlaced } from "./columns";
import type { AdaptiveCell } from "./hexAdaptive";

/** A leaf along +x, 100 m apart, holding `count` placements. */
function leaf(i: number, count = 100): AdaptiveCell {
  return { level: 0, size: 100, q: i, r: 0, x: i * 100, y: 0, z: 0, count, loaded: 0 };
}

function gridOf(leaves: AdaptiveCell[]): CellGrid {
  return {
    group: "base",
    sizes: [100],
    threshold: 1000,
    maps: 1,
    failed: 0,
    placements: leaves.reduce((sum, l) => sum + l.count, 0),
    terrain: 1,
    leaves,
    splits: [],
  };
}

function oneRow(x: number, index: number) {
  return columnsFromPlaced([
    {
      index,
      name: "lamp",
      nameHash: 1,
      isMlo: false,
      fromMlo: false,
      position: [x, 0, 0],
      rotation: [0, 0, 0, 1],
      scaleXy: 1,
      scaleZ: 1,
      parentIndex: -1,
      lodLevel: "HD",
      lodDist: 100,
      children: 0,
    },
  ]);
}

/** A cell holding one placement of `a.ymap`, at its own x. */
function decoded(ref: CellRef): DecodedCell {
  return {
    ref,
    key: cellKey(ref),
    count: 1,
    parts: [{ mapKey: "a.ymap", mapName: "a", columns: oneRow(ref.q * 100, ref.q + 1) }],
  };
}

/** A fake backend: every call waits until the test lets it through. */
function backend() {
  const calls: { cells: CellRef[]; known: readonly string[]; resolve: () => void; reject: (e: unknown) => void }[] = [];
  const fetch = (cells: readonly CellRef[], known: readonly string[]): Promise<DecodedBatch> =>
    new Promise((resolve, reject) => {
      calls.push({
        cells: [...cells],
        known,
        resolve: () =>
          resolve({
            cells: cells.map((c) => decoded(c)),
            assets: [{ name: "lamp", path: "lamp.ydr" }],
            models: new Map(),
            bytes: 10,
          }),
        reject,
      });
    });
  return { calls, fetch };
}

const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

function streamer(
  leaves: AdaptiveCell[],
  options: { cellsPerCall?: number; concurrency?: number; budget?: number } = {},
) {
  const fake = backend();
  const batches: DecodedBatch[] = [];
  const errors: unknown[] = [];
  let clock = 0;
  const s = new CellStreamer(
    {
      fetch: fake.fetch,
      known: () => ["known-name"],
      onBatch: (b) => batches.push(b),
      onError: (e) => errors.push(e),
      now: () => clock,
    },
    {
      cellsPerCall: options.cellsPerCall ?? 2,
      concurrency: options.concurrency ?? 1,
      plan: { budget: options.budget ?? 300, keepFactor: 1.4, prefetchShare: 0.4, prefetchReach: 4 },
    },
  );
  s.setGrid(gridOf(leaves));
  return { s, fake, batches, errors, advance: (ms: number) => (clock += ms) };
}

const keys = (cells: readonly CellRef[]): string[] => cells.map(cellKey);

describe("CellStreamer fetching", () => {
  test("asks for the ground first, then the working set nearest first, in small batches", () => {
    const { s, fake } = streamer(Array.from({ length: 10 }, (_, i) => leaf(i)), { cellsPerCall: 2 });
    s.update(0, 0, null);
    expect(fake.calls).toHaveLength(1);
    expect(keys(fake.calls[0]!.cells)).toEqual([TERRAIN_KEY, cellKey({ level: 0, q: 0, r: 0 })]);
    expect(fake.calls[0]!.known).toEqual(["known-name"]);
  });

  test("never has more calls out than its concurrency", () => {
    const { s, fake } = streamer(Array.from({ length: 10 }, (_, i) => leaf(i)), {
      cellsPerCall: 1,
      concurrency: 2,
    });
    s.update(0, 0, null);
    s.update(0, 0, null);
    expect(fake.calls).toHaveLength(2);
    expect(s.stats().pending).toBe(2);
  });

  test("a cell that arrived is held, its models handed on, and not asked for again", async () => {
    const { s, fake, batches } = streamer(Array.from({ length: 4 }, (_, i) => leaf(i)), {
      cellsPerCall: 4,
      concurrency: 1,
    });
    s.update(0, 0, null);
    expect(fake.calls).toHaveLength(1);
    fake.calls[0]!.resolve();
    await tick();

    expect(batches).toHaveLength(1);
    expect(s.store.has(TERRAIN_KEY)).toBe(true);
    expect(s.store.has(cellKey({ level: 0, q: 0, r: 0 }))).toBe(true);
    expect(s.entries().map((e) => e.key)).toEqual(["a.ymap"]);

    s.update(0, 0, null);
    // Whatever is left over goes out; what is held does not.
    for (const call of fake.calls.slice(1)) {
      for (const ref of call.cells) expect(s.store.has(cellKey(ref))).toBe(false);
    }
  });

  test("a cell the backend left out of its reply is held empty, not asked for again", async () => {
    let calls = 0;
    const s = new CellStreamer(
      {
        // Answers with the ground only, whatever was asked.
        fetch: (cells) => {
          calls += 1;
          const ground = cells.filter((c) => cellKey(c) === TERRAIN_KEY).map(decoded);
          return Promise.resolve({ cells: ground, assets: [], models: new Map(), bytes: 1 });
        },
        known: () => [],
        onBatch: () => {},
      },
      { cellsPerCall: 5, concurrency: 1, plan: { budget: 100 } },
    );
    s.setGrid(gridOf([leaf(0)]));
    s.update(0, 0, null);
    await tick();
    expect(s.store.has(cellKey({ level: 0, q: 0, r: 0 }))).toBe(true);
    expect(s.store.placements).toBe(1);

    s.update(0, 0, null);
    await tick();
    expect(calls).toBe(1);
  });

  test("the prefetch ring goes out after the working set", () => {
    const { s, fake } = streamer(Array.from({ length: 12 }, (_, i) => leaf(i)), {
      cellsPerCall: 20,
      budget: 300,
    });
    s.update(0, 0, [1, 0]);
    const asked = keys(fake.calls[0]!.cells);
    expect(asked[0]).toBe(TERRAIN_KEY);
    // Three cells of 100 are the working set; the next ones are the prefetch.
    expect(asked.slice(1, 4)).toEqual([0, 1, 2].map((q) => cellKey({ level: 0, q, r: 0 })));
    expect(asked.length).toBeGreaterThan(4);
    expect(s.stats().prefetchCells).toBeGreaterThan(0);
  });
});

describe("CellStreamer eviction", () => {
  async function loaded() {
    const t = streamer(Array.from({ length: 30 }, (_, i) => leaf(i)), {
      cellsPerCall: 50,
      budget: 300,
    });
    t.s.update(0, 0, null);
    t.fake.calls[0]!.resolve();
    await tick();
    return t;
  }

  test("flying away evicts the cells left behind and reports their placements", async () => {
    const { s } = await loaded();
    const before = s.store.keys().filter((k) => k !== TERRAIN_KEY);
    expect(before.length).toBeGreaterThanOrEqual(3);

    const dropped = s.update(2500, 0, null);
    expect(dropped.length).toBeGreaterThan(0);
    for (const row of dropped) expect(row.mapKey).toBe("a.ymap");
    // The cell at the origin is gone; the ground is not.
    expect(s.store.has(cellKey({ level: 0, q: 0, r: 0 }))).toBe(false);
    expect(s.store.has(TERRAIN_KEY)).toBe(true);
  });

  test("a camera that has not moved evicts nothing", async () => {
    const { s } = await loaded();
    expect(s.update(0, 0, null)).toEqual([]);
  });

  test("the slack keeps a cell just past the working set", async () => {
    const { s } = await loaded();
    // Two cells over: cell 0 is now past the three-cell working set but inside
    // the keep set (four cells), so it stays.
    const dropped = s.update(200, 0, null);
    expect(dropped).toEqual([]);
    expect(s.store.has(cellKey({ level: 0, q: 0, r: 0 }))).toBe(true);
  });

  test("the version moves when cells arrive and when they leave", async () => {
    const { s, fake } = streamer(Array.from({ length: 30 }, (_, i) => leaf(i)), {
      cellsPerCall: 50,
    });
    const seen: number[] = [];
    s.subscribe(() => seen.push(s.getVersion()));
    s.update(0, 0, null);
    fake.calls[0]!.resolve();
    await tick();
    const afterArrival = seen.length;
    expect(afterArrival).toBeGreaterThan(0);
    s.update(5000, 0, null);
    expect(seen.length).toBeGreaterThan(afterArrival);
  });
});

describe("CellStreamer failures and resets", () => {
  test("a failed fetch is reported once and retried only after the back-off", async () => {
    const { s, fake, errors, advance } = streamer([leaf(0)], { cellsPerCall: 5 });
    s.update(0, 0, null);
    fake.calls[0]!.reject(new Error("boom"));
    await tick();
    expect(errors).toHaveLength(1);

    s.update(0, 0, null);
    expect(fake.calls).toHaveLength(1);
    advance(RETRY_MS + 1);
    s.update(0, 0, null);
    expect(fake.calls).toHaveLength(2);
    expect(keys(fake.calls[1]!.cells)).toContain(TERRAIN_KEY);
  });

  test("a new partition drops everything, and a reply from the old one is ignored", async () => {
    const { s, fake, batches } = streamer(Array.from({ length: 5 }, (_, i) => leaf(i)), {
      cellsPerCall: 50,
    });
    s.update(0, 0, null);
    s.setGrid(gridOf([leaf(0)]));
    fake.calls[0]!.resolve();
    await tick();
    expect(batches).toHaveLength(0);
    expect(s.store.size).toBe(0);
    // And it is not stuck: the new partition gets its own call.
    s.update(0, 0, null);
    expect(fake.calls).toHaveLength(2);
  });

  test("with no partition there is nothing to do", () => {
    const { s, fake } = streamer([]);
    s.setGrid(null);
    expect(s.update(0, 0, [1, 0])).toEqual([]);
    expect(fake.calls).toHaveLength(0);
    expect(TERRAIN_CELL.level).toBe(255);
  });

  test("the budget can change between passes", async () => {
    const { s } = streamer(Array.from({ length: 30 }, (_, i) => leaf(i)), { budget: 300 });
    s.update(0, 0, null);
    expect(s.stats().workingCells).toBe(3);
    s.setBudget(600);
    s.update(0, 0, null);
    expect(s.stats().workingCells).toBe(6);
  });
});
