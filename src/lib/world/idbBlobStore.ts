/**
 * `BlobStore` over IndexedDB, for the persistent model tier (#7).
 *
 * Three object stores: `meta` (the scope in force), `index` (one small record
 * per model, read whole on open) and `blobs` (the response buffers, read one at
 * a time). Metadata and bytes are split so opening the tier reads kilobytes,
 * not the gigabytes it indexes.
 */
import type { BlobStore, IndexRecord } from "./persistentModels";

const DB_NAME = "astramapper-models";
const DB_VERSION = 1;
const META = "meta";
const INDEX = "index";
const BLOBS = "blobs";
const SCOPE_KEY = "scope";

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.addEventListener("success", () => resolve(req.result));
    req.addEventListener("error", () => reject(req.error));
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.addEventListener("complete", () => resolve());
    tx.addEventListener("error", () => reject(tx.error));
    tx.addEventListener("abort", () =>
      reject(tx.error ?? new Error("the transaction was aborted")),
    );
  });
}

export class IdbBlobStore implements BlobStore {
  private db: Promise<IDBDatabase> | null = null;

  constructor(
    private readonly factory: IDBFactory = indexedDB,
    private readonly name: string = DB_NAME,
  ) {}

  private open(): Promise<IDBDatabase> {
    this.db ??= new Promise((resolve, reject) => {
      const opening = this.factory.open(this.name, DB_VERSION);
      opening.addEventListener("upgradeneeded", () => {
        const db = opening.result;
        db.createObjectStore(META);
        db.createObjectStore(INDEX, { keyPath: "path" });
        db.createObjectStore(BLOBS);
      });
      opening.addEventListener("success", () => resolve(opening.result));
      opening.addEventListener("error", () => {
        this.db = null;
        reject(opening.error);
      });
    });
    return this.db;
  }

  async readScope(): Promise<string | null> {
    const db = await this.open();
    const value = await request<unknown>(db.transaction(META).objectStore(META).get(SCOPE_KEY));
    return typeof value === "string" ? value : null;
  }

  async writeScope(scope: string): Promise<void> {
    const db = await this.open();
    const tx = db.transaction(META, "readwrite");
    tx.objectStore(META).put(scope, SCOPE_KEY);
    await done(tx);
  }

  async index(): Promise<IndexRecord[]> {
    const db = await this.open();
    return request<IndexRecord[]>(db.transaction(INDEX).objectStore(INDEX).getAll());
  }

  async get(path: string): Promise<ArrayBuffer | null> {
    const db = await this.open();
    const value = await request<unknown>(db.transaction(BLOBS).objectStore(BLOBS).get(path));
    return value instanceof ArrayBuffer ? value : null;
  }

  async put(record: IndexRecord, buffer: ArrayBuffer): Promise<void> {
    const db = await this.open();
    const tx = db.transaction([INDEX, BLOBS], "readwrite");
    tx.objectStore(BLOBS).put(buffer, record.path);
    tx.objectStore(INDEX).put(record);
    await done(tx);
  }

  async touch(record: IndexRecord): Promise<void> {
    const db = await this.open();
    const tx = db.transaction(INDEX, "readwrite");
    tx.objectStore(INDEX).put(record);
    await done(tx);
  }

  async delete(paths: readonly string[]): Promise<void> {
    const db = await this.open();
    const tx = db.transaction([INDEX, BLOBS], "readwrite");
    for (const path of paths) {
      tx.objectStore(BLOBS).delete(path);
      tx.objectStore(INDEX).delete(path);
    }
    await done(tx);
  }

  async clear(): Promise<void> {
    const db = await this.open();
    const tx = db.transaction([INDEX, BLOBS], "readwrite");
    tx.objectStore(BLOBS).clear();
    tx.objectStore(INDEX).clear();
    await done(tx);
  }
}
