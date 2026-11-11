/**
 * A cache whose entries live exactly as long as something uses them.
 *
 * # Why this exists
 *
 * World uploads each texture once and shares it between every part that
 * references its hash. That cache was a bare `Map` that only grew, so a texture
 * stayed on the GPU, and its mip chain stayed in the JS heap, for the rest of
 * the session. The mips are views over the IPC response a model arrived in, so
 * the whole response (geometry included) was kept alive by one texture. Flying
 * across the base game took the JS heap from 1.1 GB to 5.5 GB with the model
 * cache sitting at its budget (#23).
 *
 * Each user `acquire`s a key and `release`s it when it goes; the last release
 * disposes the value and forgets the key.
 */
export type Disposable = { dispose(): void };

export class RefCache<T extends Disposable> {
  readonly #entries = new Map<string, { value: T; refs: number }>();

  /** How many distinct keys are held. */
  get size(): number {
    return this.#entries.size;
  }

  /** The value for `key`, made by `make` if there is none, with one more user. */
  acquire(key: string, make: () => T): T {
    const entry = this.#entries.get(key);
    if (entry) {
      entry.refs += 1;
      return entry.value;
    }
    const value = make();
    this.#entries.set(key, { value, refs: 1 });
    return value;
  }

  /** One user fewer; the last one disposes the value. An unknown key is ignored. */
  release(key: string): void {
    const entry = this.#entries.get(key);
    if (!entry) return;
    entry.refs -= 1;
    if (entry.refs > 0) return;
    this.#entries.delete(key);
    entry.value.dispose();
  }

  /** Dispose everything, whatever its users. */
  clear(): void {
    for (const { value } of this.#entries.values()) value.dispose();
    this.#entries.clear();
  }
}
