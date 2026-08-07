/**
 * Loaded geometry, held outside React.
 *
 * # Why this exists
 *
 * Loaded models used to live in a `useState` array. Every load did an O(n)
 * scan to check for duplicates and an O(n) copy to append, the callback that
 * did it listed the array in its deps so it was rebuilt on every load, and the
 * streaming interval listed it too, so the interval was torn down and recreated
 * every time a single model arrived. Loading n entities cost O(n squared) work
 * across three separate places. Streaming hid it by never loading more than a
 * couple of hundred at once. **Preloading a whole map does not hide it.**
 *
 * So the map of loaded geometry lives here, mutated in place, and React
 * subscribes to a version counter through `useSyncExternalStore`.
 *
 * # Coalescing is the point, not an optimisation
 *
 * Notifications are deferred and collapsed: a burst of five hundred `add` calls
 * produces **one** notification, on the next frame. Without that, the renderer
 * would rebuild its scene bookkeeping five hundred times for a single preload
 * pass, and the quadratic cost would just move from React into three.js.
 *
 * The consequence to know about: `all()` can be ahead of the version React last
 * saw. That is harmless — the pending notification always arrives and always
 * carries the newest array — but it does mean **the version is a change signal,
 * not a length**. Read `size()` for a count.
 */
import type { GeometryLoad } from "../world";
import { keyFor } from "../world";

/** How a pending notification gets scheduled. Injectable so tests can drive it. */
export type Scheduler = (run: () => void) => void;

export type GeometryStore = {
  /** `useSyncExternalStore` half one. Returns the unsubscribe. */
  subscribe: (listener: () => void) => () => void;
  /** `useSyncExternalStore` half two. Changes when the contents change. */
  getVersion: () => number;
  has: (key: string) => boolean;
  get: (key: string) => GeometryLoad | undefined;
  /** Store one load. A key already present is left alone, not replaced. */
  add: (load: GeometryLoad) => void;
  remove: (key: string) => void;
  /** Drop many at once, for a streaming pass's unload list. */
  removeMany: (keys: Iterable<string>) => void;
  /** Drop everything belonging to one placed map. */
  removeMap: (mapKey: string) => void;
  clear: () => void;
  size: () => number;
  /**
   * Every load, as an array whose identity is stable until the contents change.
   *
   * The renderer's effect lists this in its deps, so a fresh array on every read
   * would make it run on every render whether or not anything loaded.
   */
  all: () => readonly GeometryLoad[];
};

/** One notify per frame when there is a frame to wait for; otherwise soon. */
const defaultScheduler: Scheduler = (run) => {
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(() => run());
  else setTimeout(run, 0);
};

export function createGeometryStore(schedule: Scheduler = defaultScheduler): GeometryStore {
  const byKey = new Map<string, GeometryLoad>();
  const listeners = new Set<() => void>();
  let version = 0;
  let notifyPending = false;
  /** The array `all()` hands out, and the version it was built from. */
  let cached: readonly GeometryLoad[] = [];
  let cachedVersion = -1;

  function changed(): void {
    version += 1;
    if (notifyPending) return;
    notifyPending = true;
    schedule(() => {
      notifyPending = false;
      for (const listener of listeners) listener();
    });
  }

  return {
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getVersion: () => version,
    has: (key) => byKey.has(key),
    get: (key) => byKey.get(key),
    add(load) {
      const key = keyFor(load.mapKey, load.entity.index);
      if (byKey.has(key)) return;
      byKey.set(key, load);
      changed();
    },
    remove(key) {
      if (byKey.delete(key)) changed();
    },
    removeMany(keys) {
      let any = false;
      for (const key of keys) any = byKey.delete(key) || any;
      if (any) changed();
    },
    removeMap(mapKey) {
      let any = false;
      for (const [key, load] of byKey) {
        if (load.mapKey === mapKey) any = byKey.delete(key) || any;
      }
      if (any) changed();
    },
    clear() {
      if (byKey.size === 0) return;
      byKey.clear();
      changed();
    },
    size: () => byKey.size,
    all() {
      if (cachedVersion !== version) {
        cached = [...byKey.values()];
        cachedVersion = version;
      }
      return cached;
    },
  };
}
