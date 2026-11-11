/**
 * Fetched models, held under a byte budget (#6).
 *
 * # Why
 *
 * `modelCache` used to be a bare `Map` that only grew: every model fetched in a
 * session stayed, so the JS heap climbed with the length of the flight (922 MB
 * after one long run over the base game, against a 300 MB target). A model is a
 * set of typed-array views over one IPC response buffer, so dropping the entry
 * frees the whole buffer once nothing draws it.
 *
 * # What is evicted
 *
 * Only models no loaded geometry uses: anything the renderer holds is in use by
 * definition, and pulling it out of the cache would not free it anyway. Among
 * the rest, the farthest goes first, measured from where the model was last
 * placed to the camera now. That is the ring order the zone design wants, and
 * it is not least-recently-used: a model the camera just flew past is as
 * likely to be wanted again as one it flew past a minute ago, but one 5 km away
 * is not.
 *
 * Evicting costs a re-fetch if the area is re-entered, which is a read of the
 * backend's compiled-model cache (~1 ms) plus the IPC, never a recompile.
 */

/** What the cache needs to know about a model. */
export type Sized = { readonly byteLength: number };

type Entry<M extends Sized> = {
  readonly model: M;
  /** Where an entity using this model was last seen, GTA space; `null` if none. */
  anchor: readonly [number, number, number] | null;
};

export class ModelCache<M extends Sized> {
  private readonly entries = new Map<string, Entry<M>>();
  private held = 0;

  /** Bytes held by every cached model. */
  get bytes(): number {
    return this.held;
  }

  get size(): number {
    return this.entries.size;
  }

  get(path: string): M | undefined {
    return this.entries.get(path)?.model;
  }

  has(path: string): boolean {
    return this.entries.has(path);
  }

  /** Cache a model; one already there is replaced and its bytes recounted. */
  set(path: string, model: M, anchor: readonly [number, number, number] | null = null): void {
    const previous = this.entries.get(path);
    if (previous) this.held -= previous.model.byteLength;
    this.entries.set(path, { model, anchor: anchor ?? previous?.anchor ?? null });
    this.held += model.byteLength;
  }

  /** Note that an entity at `anchor` uses this model, for the distance order. */
  touch(path: string, anchor: readonly [number, number, number]): void {
    const entry = this.entries.get(path);
    if (entry) entry.anchor = anchor;
  }

  /**
   * Drop models until the cache fits `budget`, farthest from `camera` first and
   * never one `inUse` says something draws. Returns the paths dropped. Stops
   * early when only models in use are left: the budget is then the streamer's
   * to meet, by unloading entities, not the cache's.
   */
  evict(
    budget: number,
    camera: readonly [number, number, number],
    inUse: ReadonlySet<string>,
  ): string[] {
    if (this.held <= budget) return [];
    const candidates: { path: string; distance: number }[] = [];
    for (const [path, entry] of this.entries) {
      if (inUse.has(path)) continue;
      const a = entry.anchor;
      // No anchor means nothing ever placed it near anywhere: first to go.
      const distance = a
        ? Math.hypot(a[0] - camera[0], a[1] - camera[1], a[2] - camera[2])
        : Infinity;
      candidates.push({ path, distance });
    }
    candidates.sort((a, b) => b.distance - a.distance);

    const dropped: string[] = [];
    for (const { path } of candidates) {
      if (this.held <= budget) break;
      this.held -= this.entries.get(path)!.model.byteLength;
      this.entries.delete(path);
      dropped.push(path);
    }
    return dropped;
  }

  clear(): void {
    this.entries.clear();
    this.held = 0;
  }
}

/** Bytes of the machine's memory, as the backend reports them. */
export type SystemMemory = { readonly total: number; readonly available: number };

const MB = 1024 * 1024;

/**
 * The model cache's budget, as a share of the machine's memory rather than a
 * model count, so a dense zone does not meet the cap of a sparse one.
 *
 * A fifth of what is free, never more than a tenth of the total, within
 * 256 MB and 3 GB. It is recomputed from fresh readings, so it follows what
 * else is running.
 */
export function modelBudgetBytes(memory: SystemMemory): number {
  const share = Math.min(memory.available * 0.2, memory.total * 0.1);
  return Math.round(Math.min(Math.max(share, 256 * MB), 3072 * MB));
}

/** What to assume until the backend has answered. */
export const FALLBACK_MODEL_BUDGET = 512 * MB;
