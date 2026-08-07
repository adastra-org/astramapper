import { describe, expect, test } from "bun:test";

import { createGeometryStore, type Scheduler } from "./geometryStore";
import type { GeometryLoad, Placed, WorldModel } from "../world";

/** A scheduler the test drives by hand, so coalescing is observable. */
function manual(): { schedule: Scheduler; flush: () => void; pending: () => number } {
  let queued: (() => void)[] = [];
  return {
    schedule: (run) => queued.push(run),
    flush: () => {
      const runs = queued;
      queued = [];
      for (const run of runs) run();
    },
    pending: () => queued.length,
  };
}

function placement(index: number): Placed {
  return {
    index,
    name: `prop_${index}`,
    nameHash: index,
    isMlo: false,
    fromMlo: false,
    position: [index, 0, 0],
    rotation: [0, 0, 0, 1],
    scaleXy: 1,
    scaleZ: 1,
    parentIndex: -1,
    lodLevel: "HD",
    lodDist: 100,
    children: 0,
  };
}

function load(mapKey: string, index: number): GeometryLoad {
  return {
    mapKey,
    entity: placement(index),
    modelPath: `models/prop_${index}.ydr`,
    model: { path: "", drawables: [], emptyDrawables: 0, textures: [] } as unknown as WorldModel,
  };
}

describe("createGeometryStore", () => {
  test("stores and finds by key", () => {
    const store = createGeometryStore(manual().schedule);
    store.add(load("a.ymap", 1));

    expect(store.has("a.ymap:1")).toBe(true);
    expect(store.get("a.ymap:1")?.modelPath).toBe("models/prop_1.ydr");
    expect(store.has("a.ymap:2")).toBe(false);
    expect(store.size()).toBe(1);
  });

  /** The duplicate check that used to be an O(n) array scan on every load. */
  test("adding the same key twice is a no-op", () => {
    const store = createGeometryStore(manual().schedule);
    store.add(load("a.ymap", 1));
    const first = store.get("a.ymap:1");
    store.add({ ...load("a.ymap", 1), modelPath: "other.ydr" });

    expect(store.size()).toBe(1);
    expect(store.get("a.ymap:1")).toBe(first!);
  });

  test("removes one, many, and a whole map", () => {
    const store = createGeometryStore(manual().schedule);
    for (const index of [1, 2, 3]) store.add(load("a.ymap", index));
    for (const index of [1, 2]) store.add(load("b.ymap", index));

    store.remove("a.ymap:1");
    expect(store.size()).toBe(4);

    store.removeMany(["a.ymap:2", "a.ymap:3", "nothing:9"]);
    expect(store.size()).toBe(2);

    store.removeMap("b.ymap");
    expect(store.size()).toBe(0);
  });

  /**
   * The property the whole file exists for: five hundred loads must cost one
   * notification, not five hundred. Without this a preload rebuilds the scene
   * bookkeeping once per model and the quadratic cost just moves into three.js.
   */
  test("coalesces a burst of writes into one notification", () => {
    const clock = manual();
    const store = createGeometryStore(clock.schedule);
    let notified = 0;
    store.subscribe(() => (notified += 1));

    for (let index = 0; index < 500; index += 1) store.add(load("a.ymap", index));
    expect(clock.pending()).toBe(1);
    expect(notified).toBe(0);

    clock.flush();
    expect(notified).toBe(1);
    expect(store.size()).toBe(500);
  });

  test("a later burst notifies again", () => {
    const clock = manual();
    const store = createGeometryStore(clock.schedule);
    let notified = 0;
    store.subscribe(() => (notified += 1));

    store.add(load("a.ymap", 1));
    clock.flush();
    store.add(load("a.ymap", 2));
    clock.flush();
    expect(notified).toBe(2);
  });

  test("a write that changes nothing does not notify", () => {
    const clock = manual();
    const store = createGeometryStore(clock.schedule);
    let notified = 0;
    store.subscribe(() => (notified += 1));

    store.remove("missing:1");
    store.removeMany(["missing:2"]);
    store.removeMap("missing.ymap");
    store.clear();
    clock.flush();
    expect(notified).toBe(0);
  });

  test("unsubscribing stops the listener", () => {
    const clock = manual();
    const store = createGeometryStore(clock.schedule);
    let notified = 0;
    const stop = store.subscribe(() => (notified += 1));

    stop();
    store.add(load("a.ymap", 1));
    clock.flush();
    expect(notified).toBe(0);
  });

  test("the version moves with the contents", () => {
    const store = createGeometryStore(manual().schedule);
    const start = store.getVersion();
    store.add(load("a.ymap", 1));
    expect(store.getVersion()).toBeGreaterThan(start);
  });

  /**
   * The renderer lists `all()` in its effect deps, so a fresh array on every
   * read would rebuild the scene on every render rather than on every load.
   */
  test("all() keeps its identity until the contents change", () => {
    const store = createGeometryStore(manual().schedule);
    store.add(load("a.ymap", 1));

    const first = store.all();
    expect(store.all()).toBe(first);

    store.add(load("a.ymap", 2));
    const second = store.all();
    expect(second).not.toBe(first);
    expect(second).toHaveLength(2);
  });
});
