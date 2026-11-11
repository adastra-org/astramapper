/**
 * The persistent tier under the model cache (#7): models received over IPC are
 * kept in IndexedDB, so a second session reads them from disk in the WebView and
 * never makes the IPC round trip.
 *
 * # Why
 *
 * The backend's compiled-model cache answers in ~1 ms, but the cost of a model
 * is the IPC round trip (see `docs/streaming.md`), and that is paid again every
 * session for every model. This tier removes it for anything seen before.
 *
 * # What is stored
 *
 * The response buffer as it arrived, not the decoded model. A `WorldModel` is
 * typed-array views over one buffer, so keeping the buffer and decoding on the
 * way out is both the smallest thing to store and the same code path as a fresh
 * fetch (`decodeModel`).
 *
 * # Keyed so it invalidates
 *
 * Everything in the store belongs to one *scope*: the game build (the backend's
 * install stamp, which moves when `GTA5.exe` is replaced), the GPU capability
 * bits (the same texture compiles to different bytes without BPTC), and the wire
 * format version. Opening with a different scope wipes the store, so a game
 * update, a different GPU or a decoder change can never serve stale bytes. A
 * record that no longer decodes is dropped by the caller (`forget`).
 *
 * # How it meets the RAM budget
 *
 * `ModelCache` evicts the farthest unused model when over its RAM budget; with
 * this tier an eviction is a disk read on return, not an IPC. The tier has its
 * own byte budget on disk and evicts least recently used first, since distance
 * from a camera that is not there yet means nothing between sessions.
 */
import { ENVELOPE_VERSION } from "../wire";

/** Bumped when the layout of what is stored changes. */
const LAYOUT_VERSION = 4;

/** A record is only re-stamped on a read once it is this stale, so a hot model
 *  costs one write a minute and not one per use. */
const TOUCH_AFTER_MS = 60_000;

const MB = 1024 * 1024;

/** What the tier remembers about a stored model without reading it. */
export type IndexRecord = {
  readonly path: string;
  readonly bytes: number;
  readonly lastUsed: number;
};

/**
 * Where the bytes live. IndexedDB in the app, a `Map` in the tests; everything
 * the tier decides is above this line.
 */
export interface BlobStore {
  readScope(): Promise<string | null>;
  writeScope(scope: string): Promise<void>;
  /** Every record's metadata, read once when the tier opens. */
  index(): Promise<IndexRecord[]>;
  get(path: string): Promise<ArrayBuffer | null>;
  put(record: IndexRecord, buffer: ArrayBuffer): Promise<void>;
  /** Update a record's metadata only. */
  touch(record: IndexRecord): Promise<void>;
  delete(paths: readonly string[]): Promise<void>;
  clear(): Promise<void>;
}

/** The capability bits as the backend numbers them. */
export type CapsBits = { readonly s3tc: boolean; readonly rgtc: boolean; readonly bptc: boolean };

/**
 * What a store is valid for. Any difference between two of these means the
 * bytes of one must not be served to the other.
 */
export function scopeOf(installStamp: string, caps: CapsBits): string {
  const bits = (caps.s3tc ? 1 : 0) | (caps.rgtc ? 2 : 0) | (caps.bptc ? 4 : 0);
  return `l${LAYOUT_VERSION}|w${ENVELOPE_VERSION}|build:${installStamp}|caps:${bits}`;
}

/**
 * The disk budget as a share of what the WebView may store: a quarter of the
 * quota, within 128 MB and 4 GB; a gigabyte when the quota is not known.
 */
export function diskBudgetBytes(quota: number | null): number {
  if (quota === null || !Number.isFinite(quota) || quota <= 0) return 1024 * MB;
  return Math.round(Math.min(Math.max(quota * 0.25, 128 * MB), 4096 * MB));
}

export class PersistentModelTier {
  private scope: string | null = null;
  private entries = new Map<string, IndexRecord>();
  private held = 0;
  private budget = 0;
  /** Bumped on every `open`, so work queued for an old scope can tell. */
  private generation = 0;
  /** Writes run one after another, so the index and the store agree. */
  private queue: Promise<void> = Promise.resolve();

  hits = 0;
  misses = 0;

  constructor(
    private readonly store: BlobStore,
    private readonly now: () => number = Date.now,
    private readonly onError: (cause: unknown) => void = () => undefined,
  ) {}

  /** Whether `open` has succeeded: nothing is read or written before. */
  get enabled(): boolean {
    return this.scope !== null;
  }

  get bytes(): number {
    return this.held;
  }

  get size(): number {
    return this.entries.size;
  }

  has(path: string): boolean {
    return this.entries.has(path);
  }

  /**
   * Make `scope` the one in force. A store written under another scope is
   * wiped; one written under this scope is read back into the index.
   *
   * Never throws: a store that cannot be opened leaves the tier disabled and the
   * app on the IPC path, only slower.
   */
  async open(scope: string, budget: number): Promise<void> {
    if (this.scope === scope) {
      this.setBudget(budget);
      return;
    }
    const generation = ++this.generation;
    this.scope = null;
    this.entries = new Map();
    this.held = 0;
    this.budget = budget;
    try {
      // Behind anything still queued for the previous scope.
      await this.queue;
      const stored = await this.store.readScope();
      if (stored !== scope) {
        await this.store.clear();
        await this.store.writeScope(scope);
      } else {
        for (const record of await this.store.index()) {
          this.entries.set(record.path, record);
          this.held += record.bytes;
        }
      }
      if (generation !== this.generation) return;
      this.scope = scope;
      this.enqueue(generation, () => this.trim());
    } catch (cause) {
      this.entries = new Map();
      this.held = 0;
      this.onError(cause);
    }
  }

  setBudget(budget: number): void {
    this.budget = budget;
    if (this.scope === null) return;
    this.enqueue(this.generation, () => this.trim());
  }

  /** The stored bytes for a model, or `null` (never stored, or unreadable). */
  async read(path: string): Promise<ArrayBuffer | null> {
    const generation = this.generation;
    const record = this.entries.get(path);
    if (this.scope === null || !record) {
      this.misses += 1;
      return null;
    }
    try {
      const buffer = await this.store.get(path);
      if (generation !== this.generation) return null;
      if (!buffer) {
        // The index said yes and the store said no: believe the store.
        this.drop(path);
        this.misses += 1;
        return null;
      }
      this.hits += 1;
      const now = this.now();
      if (now - record.lastUsed > TOUCH_AFTER_MS) {
        const touched = { ...record, lastUsed: now };
        this.entries.set(path, touched);
        this.enqueue(generation, () => this.store.touch(touched));
      }
      return buffer;
    } catch (cause) {
      this.onError(cause);
      this.misses += 1;
      return null;
    }
  }

  /** Keep a model's bytes. Fire and forget: the caller already has the model. */
  write(path: string, buffer: ArrayBuffer): void {
    if (this.scope === null || buffer.byteLength > this.budget) return;
    const generation = this.generation;
    this.enqueue(generation, async () => {
      const record: IndexRecord = { path, bytes: buffer.byteLength, lastUsed: this.now() };
      await this.store.put(record, buffer);
      this.held += record.bytes - (this.entries.get(path)?.bytes ?? 0);
      this.entries.set(path, record);
      await this.trim();
    });
  }

  /** A stored record that would not decode: gone, so it is not served again. */
  forget(path: string): void {
    if (!this.entries.has(path)) return;
    this.drop(path);
    this.enqueue(this.generation, () => this.store.delete([path]));
  }

  /** Empty the store and the index, keeping the scope. */
  async clear(): Promise<void> {
    this.entries = new Map();
    this.held = 0;
    const generation = this.generation;
    this.enqueue(generation, () => this.store.clear());
    await this.queue;
  }

  /** Wait for every queued write. For the tests, and for a clean shutdown. */
  flush(): Promise<void> {
    return this.queue;
  }

  private drop(path: string): void {
    const record = this.entries.get(path);
    if (!record) return;
    this.held -= record.bytes;
    this.entries.delete(path);
  }

  /** Least recently used out, until the store fits its budget. */
  private async trim(): Promise<void> {
    if (this.held <= this.budget) return;
    const oldestFirst: IndexRecord[] = [];
    for (const record of this.entries.values()) oldestFirst.push(record);
    oldestFirst.sort((a, b) => a.lastUsed - b.lastUsed);
    const gone: string[] = [];
    for (const record of oldestFirst) {
      if (this.held <= this.budget) break;
      this.drop(record.path);
      gone.push(record.path);
    }
    if (gone.length > 0) await this.store.delete(gone);
  }

  private enqueue(generation: number, job: () => Promise<void>): void {
    this.queue = this.queue.then(async () => {
      if (generation !== this.generation) return;
      try {
        await job();
      } catch (cause) {
        this.onError(cause);
      }
    });
  }
}

/** Where a model came from, for the diagnostics. */
export type ModelSource = "disk" | "ipc";

/**
 * Load a model through the tiers: the persistent one first, the backend when it
 * has nothing (or nothing that decodes), writing what the backend sent back
 * into the persistent one.
 *
 * The RAM tier (`ModelCache`) sits above this and is the caller's.
 */
export async function loadThroughTier<M>(
  path: string,
  tier: PersistentModelTier,
  fetchBuffer: (path: string) => Promise<ArrayBuffer>,
  decode: (buffer: ArrayBuffer, path: string) => M,
): Promise<{ model: M; source: ModelSource }> {
  const stored = await tier.read(path);
  if (stored) {
    try {
      return { model: decode(stored, path), source: "disk" };
    } catch {
      // Written by a decoder that no longer agrees, or damaged: refetch.
      tier.forget(path);
    }
  }
  const buffer = await fetchBuffer(path);
  const model = decode(buffer, path);
  tier.write(path, buffer);
  return { model, source: "ipc" };
}
