/**
 * Slot bookkeeping for one `InstancedMesh`, with no three.js in it.
 *
 * # Why dense, not a free list
 *
 * An `InstancedMesh` draws `count` instances from slot 0 up. A free list leaves
 * holes below `count`, and a hole is still sent to the vertex shader: it has to
 * be parked as a zero-scale matrix, so a street that streamed in and out keeps
 * paying for what left. So a removal here **swaps the last live slot into the
 * hole** and shrinks `count`. The slots are always `0..count-1`, nothing is
 * drawn that should not be, and the cost is one matrix copy, which the caller
 * is told about (`moved`) so it can do it.
 *
 * # Capacity
 *
 * The matrix buffer of an `InstancedMesh` is fixed at construction. `add`
 * reports when the table is full and `grow` doubles it; the renderer swaps in a
 * bigger mesh (the geometry and material are shared, so nothing is re-uploaded
 * but the matrices). `shrinkTo` gives memory back, but only when the table is
 * under a quarter full, so a pool that oscillates around a size does not
 * reallocate on every other call.
 *
 * Owners are small integers (the renderer hands one out per entity) so the
 * table is a typed array and a hit's `instanceId` resolves to its owner in O(1).
 */

/** Smallest table. Most models are placed once or twice. */
export const MIN_CAPACITY = 4;

export type Move = {
  /** The slot whose contents now live in `to`. It is no longer in use. */
  readonly from: number;
  readonly to: number;
};

export class InstanceSlots {
  #owners: Int32Array;
  readonly #slotOf = new Map<number, number>();
  #count = 0;

  constructor(capacity: number = MIN_CAPACITY) {
    this.#owners = new Int32Array(Math.max(capacity, MIN_CAPACITY));
  }

  /** Live instances: the `count` of the mesh. */
  get count(): number {
    return this.#count;
  }

  get capacity(): number {
    return this.#owners.length;
  }

  get full(): boolean {
    return this.#count === this.#owners.length;
  }

  has(owner: number): boolean {
    return this.#slotOf.has(owner);
  }

  slotOf(owner: number): number | undefined {
    return this.#slotOf.get(owner);
  }

  /** Who holds a slot, or `undefined` past `count`. What a raycast hit resolves through. */
  ownerAt(slot: number): number | undefined {
    return slot >= 0 && slot < this.#count ? this.#owners[slot] : undefined;
  }

  /**
   * Give an owner the next slot. Returns its slot, or `undefined` when the
   * table is full (call `grow` and add again) or the owner already has one.
   */
  add(owner: number): number | undefined {
    if (this.full || this.#slotOf.has(owner)) return undefined;
    const slot = this.#count;
    this.#owners[slot] = owner;
    this.#slotOf.set(owner, slot);
    this.#count += 1;
    return slot;
  }

  /**
   * Free an owner's slot. `null` when it had none; otherwise the copy the
   * caller owes the mesh (`moved` is absent when the removed slot was last).
   */
  remove(owner: number): { readonly slot: number; readonly moved?: Move } | null {
    const slot = this.#slotOf.get(owner);
    if (slot === undefined) return null;
    this.#slotOf.delete(owner);
    const last = this.#count - 1;
    this.#count = last;
    if (slot === last) return { slot };
    const mover = this.#owners[last]!;
    this.#owners[slot] = mover;
    this.#slotOf.set(mover, slot);
    return { slot, moved: { from: last, to: slot } };
  }

  /** Double the table. Slots are unchanged. Returns the new capacity. */
  grow(): number {
    return this.resize(this.#owners.length * 2);
  }

  /**
   * The capacity worth shrinking to, or `undefined` to leave it. Under a
   * quarter full and above the minimum: half the table, still room to add.
   */
  shrinkTarget(): number | undefined {
    const cap = this.#owners.length;
    if (cap <= MIN_CAPACITY || this.#count * 4 > cap) return undefined;
    return Math.max(cap >> 1, MIN_CAPACITY);
  }

  /** Reallocate. Never below `count`. Slots are unchanged. */
  resize(capacity: number): number {
    const next = new Int32Array(Math.max(capacity, this.#count, MIN_CAPACITY));
    next.set(this.#owners.subarray(0, this.#count));
    this.#owners = next;
    return next.length;
  }

  /** Every live owner, in slot order. */
  *owners(): IterableIterator<number> {
    for (let slot = 0; slot < this.#count; slot += 1) yield this.#owners[slot]!;
  }

  clear(): void {
    this.#slotOf.clear();
    this.#count = 0;
  }
}
