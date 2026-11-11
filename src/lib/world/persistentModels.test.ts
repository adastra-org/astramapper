import { describe, expect, test } from "bun:test";
import { IDBFactory } from "fake-indexeddb";

import { IdbBlobStore } from "./idbBlobStore";
import {
  diskBudgetBytes,
  loadThroughTier,
  PersistentModelTier,
  scopeOf,
  type BlobStore,
  type IndexRecord,
} from "./persistentModels";

const MB = 1024 * 1024;
const NO_CAPS = { s3tc: false, rgtc: false, bptc: false };
const ALL_CAPS = { s3tc: true, rgtc: true, bptc: true };

/** A store in a `Map`, to test the tier's decisions without IndexedDB. */
class MemoryStore implements BlobStore {
  scope: string | null = null;
  records = new Map<string, IndexRecord>();
  blobs = new Map<string, ArrayBuffer>();
  async readScope() {
    return this.scope;
  }
  async writeScope(scope: string) {
    this.scope = scope;
  }
  async index() {
    return [...this.records.values()];
  }
  async get(path: string) {
    return this.blobs.get(path) ?? null;
  }
  async put(record: IndexRecord, buffer: ArrayBuffer) {
    this.records.set(record.path, record);
    this.blobs.set(record.path, buffer);
  }
  async touch(record: IndexRecord) {
    this.records.set(record.path, record);
  }
  async delete(paths: readonly string[]) {
    for (const path of paths) {
      this.records.delete(path);
      this.blobs.delete(path);
    }
  }
  async clear() {
    this.records.clear();
    this.blobs.clear();
  }
}

const bytes = (mb: number, fill = 1): ArrayBuffer => new Uint8Array(mb * MB).fill(fill).buffer;

describe("scopeOf", () => {
  test("differs by build, by GPU caps and by nothing else", () => {
    const base = scopeOf("aaaa", NO_CAPS);
    expect(scopeOf("aaaa", NO_CAPS)).toBe(base);
    expect(scopeOf("bbbb", NO_CAPS)).not.toBe(base);
    expect(scopeOf("aaaa", ALL_CAPS)).not.toBe(base);
    expect(scopeOf("aaaa", { ...NO_CAPS, bptc: true })).not.toBe(
      scopeOf("aaaa", { ...NO_CAPS, rgtc: true }),
    );
  });
});

describe("diskBudgetBytes", () => {
  test("a quarter of the quota, within 128 MB and 4 GB", () => {
    expect(diskBudgetBytes(2048 * MB)).toBe(512 * MB);
    expect(diskBudgetBytes(100 * MB)).toBe(128 * MB);
    expect(diskBudgetBytes(100_000 * MB)).toBe(4096 * MB);
  });
  test("a gigabyte when the quota is unknown", () => {
    expect(diskBudgetBytes(null)).toBe(1024 * MB);
    expect(diskBudgetBytes(Number.NaN)).toBe(1024 * MB);
  });
});

describe("PersistentModelTier", () => {
  test("is closed, and reads nothing, until opened", async () => {
    const tier = new PersistentModelTier(new MemoryStore());
    expect(tier.enabled).toBe(false);
    tier.write("a", bytes(1));
    await tier.flush();
    expect(tier.size).toBe(0);
    expect(await tier.read("a")).toBeNull();
  });

  test("a model written in one session is read back in the next", async () => {
    const store = new MemoryStore();
    const scope = scopeOf("build1", NO_CAPS);

    const first = new PersistentModelTier(store);
    await first.open(scope, 100 * MB);
    first.write("models/a.yft", bytes(2, 7));
    await first.flush();

    const second = new PersistentModelTier(store);
    await second.open(scope, 100 * MB);
    expect(second.has("models/a.yft")).toBe(true);
    expect(second.bytes).toBe(2 * MB);
    const buffer = await second.read("models/a.yft");
    expect(buffer?.byteLength).toBe(2 * MB);
    expect(new Uint8Array(buffer!)[0]).toBe(7);
    expect(second.hits).toBe(1);
  });

  test("a different game build drops everything", async () => {
    const store = new MemoryStore();
    const before = new PersistentModelTier(store);
    await before.open(scopeOf("build1", NO_CAPS), 100 * MB);
    before.write("a", bytes(1));
    await before.flush();
    expect(store.blobs.size).toBe(1);

    const after = new PersistentModelTier(store);
    await after.open(scopeOf("build2", NO_CAPS), 100 * MB);
    expect(after.size).toBe(0);
    expect(store.blobs.size).toBe(0);
    expect(await after.read("a")).toBeNull();
    expect(store.scope).toBe(scopeOf("build2", NO_CAPS));
  });

  test("a different GPU drops everything too", async () => {
    const store = new MemoryStore();
    const a = new PersistentModelTier(store);
    await a.open(scopeOf("build1", ALL_CAPS), 100 * MB);
    a.write("a", bytes(1));
    await a.flush();

    const b = new PersistentModelTier(store);
    await b.open(scopeOf("build1", NO_CAPS), 100 * MB);
    expect(await b.read("a")).toBeNull();
    expect(store.blobs.size).toBe(0);
  });

  test("reopening under another scope in one session drops the old scope's writes", async () => {
    const store = new MemoryStore();
    const tier = new PersistentModelTier(store);
    await tier.open("scope-a", 100 * MB);
    tier.write("a", bytes(1));
    await tier.open("scope-b", 100 * MB);
    await tier.flush();
    expect(tier.has("a")).toBe(false);
    expect(store.blobs.size).toBe(0);
  });

  test("evicts the least recently used when over its budget", async () => {
    const store = new MemoryStore();
    let clock = 0;
    const tier = new PersistentModelTier(store, () => (clock += 1000));
    await tier.open("s", 5 * MB);
    tier.write("old", bytes(2));
    tier.write("mid", bytes(2));
    tier.write("new", bytes(2));
    await tier.flush();
    expect(tier.has("old")).toBe(false);
    expect(tier.has("mid")).toBe(true);
    expect(tier.has("new")).toBe(true);
    expect(tier.bytes).toBe(4 * MB);
    expect(store.blobs.has("old")).toBe(false);
  });

  test("a read counts as a use, so a hot model survives eviction", async () => {
    const store = new MemoryStore();
    let clock = 0;
    const tier = new PersistentModelTier(store, () => (clock += 120_000));
    await tier.open("s", 5 * MB);
    tier.write("hot", bytes(2));
    tier.write("cold", bytes(2));
    await tier.flush();
    await tier.read("hot");
    tier.write("fresh", bytes(2));
    await tier.flush();
    expect(tier.has("hot")).toBe(true);
    expect(tier.has("cold")).toBe(false);
  });

  test("lowering the budget trims what is held", async () => {
    const store = new MemoryStore();
    const tier = new PersistentModelTier(store);
    await tier.open("s", 100 * MB);
    tier.write("a", bytes(3));
    tier.write("b", bytes(3));
    await tier.flush();
    tier.setBudget(4 * MB);
    await tier.flush();
    expect(tier.size).toBe(1);
  });

  test("a model larger than the whole budget is not stored", async () => {
    const tier = new PersistentModelTier(new MemoryStore());
    await tier.open("s", 1 * MB);
    tier.write("huge", bytes(2));
    await tier.flush();
    expect(tier.size).toBe(0);
  });

  test("rewriting a model recounts its bytes", async () => {
    const tier = new PersistentModelTier(new MemoryStore());
    await tier.open("s", 100 * MB);
    tier.write("a", bytes(2));
    tier.write("a", bytes(3));
    await tier.flush();
    expect(tier.bytes).toBe(3 * MB);
    expect(tier.size).toBe(1);
  });

  test("an index entry the store has lost is dropped and counts as a miss", async () => {
    const store = new MemoryStore();
    const tier = new PersistentModelTier(store);
    await tier.open("s", 100 * MB);
    tier.write("a", bytes(1));
    await tier.flush();
    store.blobs.delete("a");
    expect(await tier.read("a")).toBeNull();
    expect(tier.has("a")).toBe(false);
    expect(tier.misses).toBe(1);
  });

  test("a store that fails to open leaves the tier disabled, not throwing", async () => {
    const errors: unknown[] = [];
    const broken = new MemoryStore();
    broken.readScope = () => Promise.reject(new Error("no disk"));
    const tier = new PersistentModelTier(broken, Date.now, (cause) => errors.push(cause));
    await tier.open("s", 100 * MB);
    expect(tier.enabled).toBe(false);
    expect(errors).toHaveLength(1);
  });
});

const decode = (buffer: ArrayBuffer): number => new Uint8Array(buffer)[0]!;

const picky = (buffer: ArrayBuffer): number => {
  if (buffer.byteLength === 1) throw new Error("stale layout");
  return new Uint8Array(buffer)[0]!;
};

const fresh = (): IdbBlobStore => new IdbBlobStore(new IDBFactory(), "test");

describe("loadThroughTier", () => {
  test("goes to the backend once, then never again for the same model", async () => {
    const store = new MemoryStore();
    const tier = new PersistentModelTier(store);
    await tier.open("s", 100 * MB);
    let ipc = 0;
    const fetchBuffer = async (): Promise<ArrayBuffer> => {
      ipc += 1;
      return new Uint8Array([42, 0, 0, 0]).buffer;
    };

    const first = await loadThroughTier("a", tier, fetchBuffer, decode);
    expect(first).toEqual({ model: 42, source: "ipc" });
    await tier.flush();

    // A new session: a fresh tier over the same store.
    const next = new PersistentModelTier(store);
    await next.open("s", 100 * MB);
    const second = await loadThroughTier("a", next, fetchBuffer, decode);
    expect(second).toEqual({ model: 42, source: "disk" });
    expect(ipc).toBe(1);
  });

  test("a stored record that will not decode is dropped and refetched", async () => {
    const store = new MemoryStore();
    const tier = new PersistentModelTier(store);
    await tier.open("s", 100 * MB);
    tier.write("a", new Uint8Array([9]).buffer);
    await tier.flush();

    let ipc = 0;
    const result = await loadThroughTier(
      "a",
      tier,
      async () => {
        ipc += 1;
        return new Uint8Array([5, 5]).buffer;
      },
      picky,
    );
    expect(result).toEqual({ model: 5, source: "ipc" });
    expect(ipc).toBe(1);
    await tier.flush();
    expect((await store.get("a"))?.byteLength).toBe(2);
  });

  test("a closed tier is the plain IPC path", async () => {
    const tier = new PersistentModelTier(new MemoryStore());
    const result = await loadThroughTier("a", tier, async () => new Uint8Array([1]).buffer, decode);
    expect(result.source).toBe("ipc");
  });
});

describe("IdbBlobStore", () => {
  test("round-trips scope, index and bytes", async () => {
    const store = fresh();
    expect(await store.readScope()).toBeNull();
    await store.writeScope("s1");
    expect(await store.readScope()).toBe("s1");

    const record = { path: "a", bytes: 4, lastUsed: 10 };
    await store.put(record, new Uint8Array([1, 2, 3, 4]).buffer);
    expect(await store.index()).toEqual([record]);
    expect(new Uint8Array((await store.get("a"))!)).toEqual(new Uint8Array([1, 2, 3, 4]));
    expect(await store.get("missing")).toBeNull();

    await store.touch({ ...record, lastUsed: 99 });
    expect((await store.index())[0]!.lastUsed).toBe(99);
  });

  test("delete and clear remove both the record and the bytes", async () => {
    const store = fresh();
    await Promise.all(
      ["a", "b", "c"].map((path) =>
        store.put({ path, bytes: 1, lastUsed: 0 }, new Uint8Array([1]).buffer),
      ),
    );
    await store.delete(["a"]);
    const left = (await store.index()).map((r) => r.path);
    left.sort();
    expect(left).toEqual(["b", "c"]);
    expect(await store.get("a")).toBeNull();
    await store.clear();
    expect(await store.index()).toEqual([]);
    expect(await store.get("b")).toBeNull();
  });

  test("the tier survives a restart over real IndexedDB, and a build change wipes it", async () => {
    const factory = new IDBFactory();
    const scope = scopeOf("build1", NO_CAPS);

    const session1 = new PersistentModelTier(new IdbBlobStore(factory, "restart"));
    await session1.open(scope, 100 * MB);
    session1.write("a", bytes(1, 3));
    await session1.flush();

    const session2 = new PersistentModelTier(new IdbBlobStore(factory, "restart"));
    await session2.open(scope, 100 * MB);
    const buffer = await session2.read("a");
    expect(new Uint8Array(buffer!)[0]).toBe(3);

    const session3 = new PersistentModelTier(new IdbBlobStore(factory, "restart"));
    await session3.open(scopeOf("build2", NO_CAPS), 100 * MB);
    expect(await session3.read("a")).toBeNull();
    expect(session3.size).toBe(0);
  });
});
