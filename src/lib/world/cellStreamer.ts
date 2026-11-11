/**
 * Holds the cells around the camera, fetches the ones it is missing and the
 * next ring ahead, and lets go of the rest.
 *
 * The whole behaviour of "JS holds only the working set" lives here, away from
 * React and from Tauri: the transport is injected (`fetch`), so the tests drive
 * it with a fake backend and a controllable clock of promises.
 *
 * # One pass
 *
 * `update` is called by the streaming pass every few hundred milliseconds with
 * where the camera is and which way it is going. It plans (`planCells`), evicts
 * every held cell outside the plan's keep set, and starts fetches for the cells
 * it is missing: the working set first, nearest first, then the prefetch ring.
 * It returns the placements it evicted, so the caller can unload their geometry.
 *
 * # Fetching
 *
 * Cells go out in small batches, a few per call, and a few calls at once. A
 * batch is one IPC call that returns placements and the models those cells need,
 * so a cell arrives whole and the models of a street arrive with the street.
 * Fewer, larger calls are what removes the per-call cost; small enough batches
 * keep the nearest cell from waiting behind a far one.
 *
 * A cell that fails to load is not asked for again for `RETRY_MS`, so a backend
 * error shows up once rather than four times a second.
 *
 * # Pinned zones
 *
 * `pin` holds every leaf of a zone cluster on top of the working set: the leaves
 * are asked for in the same batches, after the working set and before the
 * prefetch ring, and are exempt from eviction until `unpin`. It is not a second
 * loading path, so the limits are the same ones: a reply carries at most the
 * backend's model byte cap, and the models it does carry go through the model
 * cache, which stays under its own byte budget whatever is pinned. What a pinned
 * zone holds here is its placements (columns, cheap); geometry is still built
 * around the camera only.
 */
import {
  cellKey,
  refOf,
  TERRAIN_CELL,
  TERRAIN_KEY,
  type CellGrid,
  type CellRef,
  type DecodedBatch,
} from "./cellData";
import { DEFAULT_PLAN, planCells, type Heading, type PlanParams } from "./cellPlan";
import { CellStore, type CellEntry, type DroppedRow } from "./cellStore";
import { leavesByCluster, type ClusterProgress } from "./clusters";
import type { AdaptiveCell } from "./hexAdaptive";

export type CellStreamerDeps = {
  /** Fetch cells and the models they need. `known` names are not resolved again. */
  readonly fetch: (cells: readonly CellRef[], known: readonly string[]) => Promise<DecodedBatch>;
  /** Archetype names the caller already holds a model (or a known absence) for. */
  readonly known: () => readonly string[];
  /** A batch arrived: put its assets and models where the loader looks. */
  readonly onBatch: (batch: DecodedBatch) => void;
  /** A fetch failed. */
  readonly onError?: (cause: unknown) => void;
  /** Wall clock in ms, for the retry back-off. */
  readonly now?: () => number;
};

export type CellStreamerOptions = {
  readonly plan?: Partial<PlanParams>;
  /** Cells per call. */
  readonly cellsPerCall?: number;
  /** Calls in flight at once. */
  readonly concurrency?: number;
};

/** How long a failed cell waits before it is asked for again. */
export const RETRY_MS = 5000;

export type CellStreamerStats = {
  readonly cells: number;
  readonly placements: number;
  readonly pending: number;
  readonly calls: number;
  readonly reach: number;
  readonly workingCells: number;
  readonly prefetchCells: number;
};

export class CellStreamer {
  readonly store = new CellStore();
  private readonly deps: CellStreamerDeps;
  private params: PlanParams;
  private readonly cellsPerCall: number;
  private readonly concurrency: number;
  private grid: CellGrid | null = null;
  /** Bumped when the held cells change, so React can follow with a store hook. */
  private version = 0;
  private readonly listeners = new Set<() => void>();
  /** Cells on their way, by key. */
  private readonly inFlight = new Set<string>();
  /** Cells that failed, by key, with when they may be tried again. */
  private readonly failedUntil = new Map<string, number>();
  private calls = 0;
  /** Zone clusters being held, by key: their leaves, ready to ask for. */
  private readonly pins = new Map<string, readonly AdaptiveCell[]>();
  /** The keys of every pinned leaf, for the eviction pass. */
  private pinnedKeys = new Set<string>();
  /** The leaves of each cluster of the current grid, by cluster index. */
  private clusterLeaves: AdaptiveCell[][] = [];
  /** What the last pass decided to keep, so `unpin` can evict without a camera. */
  private lastKeep: ReadonlySet<string> = new Set();
  /** Guards a batch from before a `setGrid` landing in the next index. */
  private epoch = 0;
  private last = { reach: 0, working: 0, prefetch: 0 };

  constructor(deps: CellStreamerDeps, options: CellStreamerOptions = {}) {
    this.deps = deps;
    this.params = { ...DEFAULT_PLAN, ...options.plan };
    this.cellsPerCall = Math.max(1, options.cellsPerCall ?? 3);
    this.concurrency = Math.max(1, options.concurrency ?? 2);
  }

  // `useSyncExternalStore` pair.
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  readonly getVersion = (): number => this.version;

  /** Use a new partition: everything held belongs to the old one and goes. */
  setGrid(grid: CellGrid | null): void {
    this.epoch += 1;
    this.grid = grid;
    this.inFlight.clear();
    this.failedUntil.clear();
    this.pins.clear();
    this.pinnedKeys = new Set();
    this.lastKeep = new Set();
    this.clusterLeaves = grid ? leavesByCluster(grid) : [];
    // Calls still out belong to the old epoch and no longer report back here.
    this.calls = 0;
    if (this.store.size > 0) this.store.clear();
    this.bump();
  }

  /**
   * Hold every leaf of a zone cluster, in addition to the working set, until
   * `unpin`. Returns false for a key the partition has no cluster for.
   */
  pin(key: string): boolean {
    const at = this.grid?.clusters.findIndex((c) => c.key === key) ?? -1;
    if (at < 0) return false;
    this.pins.set(key, this.clusterLeaves[at] ?? []);
    this.refreshPinned();
    this.bump();
    return true;
  }

  /**
   * Let go of a pinned cluster. Its cells leave now, except those the working set
   * still wants or another pin holds. Returns their placements so the caller can
   * unload the geometry.
   */
  unpin(key: string): DroppedRow[] {
    if (!this.pins.delete(key)) return [];
    this.refreshPinned();
    const dropped: DroppedRow[] = [];
    for (const held of this.store.keys()) {
      if (held === TERRAIN_KEY || this.lastKeep.has(held) || this.pinnedKeys.has(held)) continue;
      for (const row of this.store.remove(held)) dropped.push(row);
    }
    this.bump();
    return dropped;
  }

  isPinned(key: string): boolean {
    return this.pins.has(key);
  }

  /** How much of each cluster is held, parallel to `partition.clusters`. */
  clusterProgress(): readonly ClusterProgress[] {
    const grid = this.grid;
    if (!grid) return [];
    return grid.clusters.map((info, at) => {
      const leaves = this.clusterLeaves[at] ?? [];
      let heldLeaves = 0;
      let heldPlacements = 0;
      for (const leaf of leaves) {
        if (!this.store.has(cellKey(refOf(leaf)))) continue;
        heldLeaves += 1;
        heldPlacements += leaf.count;
      }
      return {
        key: info.key,
        heldLeaves,
        leaves: leaves.length,
        heldPlacements,
        placements: info.placements,
        pinned: this.pins.has(info.key),
      };
    });
  }

  private refreshPinned(): void {
    const keys = new Set<string>();
    for (const leaves of this.pins.values()) {
      for (const leaf of leaves) keys.add(cellKey(refOf(leaf)));
    }
    this.pinnedKeys = keys;
  }

  /** How many placements the working set may hold. Takes effect on the next pass. */
  setBudget(budget: number): void {
    if (budget === this.params.budget) return;
    this.params = { ...this.params, budget: Math.max(1, budget) };
  }

  get partition(): CellGrid | null {
    return this.grid;
  }

  /** The maps held, each as one entry. */
  entries(): readonly CellEntry[] {
    return this.store.entries();
  }

  stats(): CellStreamerStats {
    return {
      cells: this.store.size,
      placements: this.store.placements,
      pending: this.inFlight.size,
      calls: this.calls,
      reach: this.last.reach,
      workingCells: this.last.working,
      prefetchCells: this.last.prefetch,
    };
  }

  /**
   * One streaming pass. Returns the placements of every cell it evicted.
   *
   * The terrain layer is wanted from the first pass and never evicted.
   */
  update(camX: number, camY: number, heading: Heading | null): DroppedRow[] {
    const grid = this.grid;
    if (!grid) return [];
    const plan = planCells(grid.leaves, camX, camY, heading, this.params);
    this.last = { reach: plan.reach, working: plan.working.length, prefetch: plan.prefetch.length };
    this.lastKeep = plan.keep;

    // Let go of what the plan no longer keeps. A cell in flight is not here yet;
    // if it is no longer wanted when it lands, the next pass drops it.
    const dropped: DroppedRow[] = [];
    for (const key of this.store.keys()) {
      if (key === TERRAIN_KEY || plan.keep.has(key) || this.pinnedKeys.has(key)) continue;
      for (const row of this.store.remove(key)) dropped.push(row);
    }
    if (dropped.length > 0) this.bump();

    // What to ask for, best first.
    const now = (this.deps.now ?? Date.now)();
    const wanted: CellRef[] = [];
    const consider = (ref: CellRef): void => {
      const key = cellKey(ref);
      if (this.store.has(key) || this.inFlight.has(key)) return;
      const retry = this.failedUntil.get(key);
      if (retry !== undefined && retry > now) return;
      wanted.push(ref);
    };
    consider(TERRAIN_CELL);
    for (const leaf of plan.working) consider(refOf(leaf));
    for (const leaf of this.pinnedByDistance(camX, camY)) consider(refOf(leaf));
    for (const leaf of plan.prefetch) consider(refOf(leaf));
    this.dispatch(wanted);
    return dropped;
  }

  /** The pinned leaves, nearest the camera first, so a zone fills from where you are. */
  private pinnedByDistance(camX: number, camY: number): AdaptiveCell[] {
    if (this.pins.size === 0) return [];
    const all: { leaf: AdaptiveCell; distance: number }[] = [];
    for (const leaves of this.pins.values()) {
      for (const leaf of leaves) {
        all.push({ leaf, distance: Math.hypot(leaf.x - camX, leaf.y - camY) });
      }
    }
    // oxlint-disable-next-line unicorn/no-array-sort
    all.sort((a, b) => a.distance - b.distance);
    return all.map((entry) => entry.leaf);
  }

  private dispatch(wanted: readonly CellRef[]): void {
    let next = 0;
    while (next < wanted.length && this.calls < this.concurrency) {
      const batch = wanted.slice(next, next + this.cellsPerCall);
      next += batch.length;
      this.launch(batch);
    }
  }

  private launch(batch: readonly CellRef[]): void {
    const epoch = this.epoch;
    const keys = batch.map(cellKey);
    for (const key of keys) this.inFlight.add(key);
    this.calls += 1;
    this.deps
      .fetch(batch, this.deps.known())
      .then((reply) => {
        // A new index arrived while this was out: nothing in it is ours.
        if (epoch !== this.epoch) return;
        // Models and names first, so an entity of the cell that is streamed in
        // the same instant already finds its model cached.
        this.deps.onBatch(reply);
        for (const cell of reply.cells) this.store.add(cell);
        // A cell the reply left out is one the backend had nothing for: hold it
        // empty rather than ask again.
        for (const ref of batch) {
          const key = cellKey(ref);
          if (!this.store.has(key)) {
            this.store.add({ ref, key, count: 0, parts: [] });
          }
        }
        this.bump();
      })
      .catch((cause: unknown) => {
        if (epoch !== this.epoch) return;
        const until = (this.deps.now ?? Date.now)() + RETRY_MS;
        for (const key of keys) this.failedUntil.set(key, until);
        this.deps.onError?.(cause);
      })
      .finally(() => {
        if (epoch !== this.epoch) return;
        this.calls -= 1;
        for (const key of keys) this.inFlight.delete(key);
      });
  }

  private bump(): void {
    this.version += 1;
    for (const listener of this.listeners) listener();
  }
}
