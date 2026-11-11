/**
 * One `InstancedMesh` per model, LOD bundle and part, with a matrix per entity.
 *
 * # What this replaces
 *
 * Geometry and materials were already shared per model, but every entity still
 * wrapped them in its own `Mesh`, so every entity of every visible bundle was a
 * draw call: 1,000 to 4,600 in the city. Now all the entities of a model that
 * are in the same LOD band are **one** draw call per part, whatever their number.
 *
 * # How the pieces map
 *
 * - **Pool**: one `InstancedMesh` for one shared part. Its slots are managed by
 *   `InstanceSlots` (dense, swap-remove; see there). The mesh is created on the
 *   first instance, replaced by a bigger one when full, and replaced by a
 *   smaller one when mostly empty.
 * - **Model pools**: a pool for every part of every LOD bundle of a model, kept
 *   while any entity of it is resident, so an entity crossing a band boundary
 *   moves between existing pools instead of building and tearing down meshes.
 * - **Band swap**: an entity is in the pools of exactly one bundle at a time.
 *   `setBand` removes it from the old bundle's pools and adds it to the new
 *   one's. Before its first band it is in none, so it is not drawn.
 * - **Picking**: a raycast hit carries an `instanceId`; the pool's slots turn it
 *   into an owner, the owner into the entity. Only bundles that are drawn are
 *   tested, as before.
 * - **Edits**: `setMatrix` rewrites the entity's slot in whichever pool holds
 *   it, and the stored matrix is what a later band swap re-uses, so an edit
 *   survives LOD changes.
 *
 * # Culling
 *
 * An instanced mesh is culled as a whole, by its bounding sphere. The sphere is
 * grown as instances are added or moved and rebuilt from scratch once enough
 * removals and moves have made it loose (amortised constant per operation), so a
 * pool far behind the camera is skipped and one that has shrunk stops being
 * drawn for what left it. A stale sphere is only ever too big, never wrong.
 *
 * Geometry and materials are **not** owned here: they are shared per model and
 * freed by their own refcount. This only owns the instanced meshes.
 */
import * as THREE from "three";
import { InstanceSlots } from "../../lib/world/instanceSlots";

/** One part's shared GPU resources. The identity of this object is the pool's key. */
export type PoolPart = { readonly geometry: THREE.BufferGeometry; readonly material: THREE.Material };

/** The LOD bundles of one model: `lods[band][part]`. The array's identity is the model's. */
export type PoolLods = readonly (readonly PoolPart[])[];

/** What a raycast hit resolves to. */
export type InstanceHit = {
  readonly mapKey: string;
  readonly entityIndex: number;
  readonly distance: number;
};

/** Counters for the F3 panel and the tests. */
export type InstanceStats = {
  /** Instanced meshes currently drawing something: the draw calls this adds. */
  readonly drawn: number;
  /** Instanced meshes that exist, drawing or parked empty. */
  readonly meshes: number;
  /** Entity instances across every drawn mesh. */
  readonly instances: number;
};

const scratchSphere = new THREE.Sphere();
const scratchMatrix = new THREE.Matrix4();

class Pool {
  readonly slots = new InstanceSlots();
  mesh: THREE.InstancedMesh | null = null;
  /** Removals and moves since the sphere was last exact. */
  #stale = 0;
  readonly #sphere = new THREE.Sphere();
  #sphereSet = false;

  constructor(
    readonly part: PoolPart,
    readonly parent: THREE.Object3D,
    readonly shadowed: boolean,
  ) {}

  add(owner: number, matrix: Float32Array): void {
    if (this.slots.has(owner)) {
      this.write(owner, matrix);
      return;
    }
    if (this.slots.full) {
      this.slots.grow();
      this.#remake();
    } else if (!this.mesh) {
      this.#remake();
    }
    const slot = this.slots.add(owner)!;
    this.#put(slot, matrix);
    this.#grow(matrix);
    this.#sync();
  }

  write(owner: number, matrix: Float32Array): void {
    const slot = this.slots.slotOf(owner);
    if (slot === undefined || !this.mesh) return;
    this.#put(slot, matrix);
    this.#grow(matrix);
    this.#stale += 1;
    this.#sync();
  }

  remove(owner: number): void {
    const result = this.slots.remove(owner);
    if (!result || !this.mesh) return;
    if (result.moved) {
      const array = this.mesh.instanceMatrix.array as Float32Array;
      array.copyWithin(result.moved.to * 16, result.moved.from * 16, result.moved.from * 16 + 16);
      this.mesh.instanceMatrix.needsUpdate = true;
    }
    this.#stale += 1;
    const target = this.slots.shrinkTarget();
    if (target !== undefined) {
      this.slots.resize(target);
      this.#remake();
    }
    this.#sync();
  }

  /** Drop the mesh. The shared geometry and material stay. */
  dispose(): void {
    this.#release(this.mesh);
    this.mesh = null;
  }

  #put(slot: number, matrix: Float32Array): void {
    const mesh = this.mesh!;
    (mesh.instanceMatrix.array as Float32Array).set(matrix, slot * 16);
    mesh.instanceMatrix.needsUpdate = true;
  }

  /** Expand the cull sphere by one instance. */
  #grow(matrix: Float32Array): void {
    const geometry = this.part.geometry;
    if (!geometry.boundingSphere) geometry.computeBoundingSphere();
    const base = geometry.boundingSphere;
    if (!base) return;
    scratchMatrix.fromArray(matrix);
    scratchSphere.copy(base).applyMatrix4(scratchMatrix);
    if (this.#sphereSet) this.#sphere.union(scratchSphere);
    else {
      this.#sphere.copy(scratchSphere);
      this.#sphereSet = true;
    }
  }

  /** Make the sphere exact again. O(count), paid once per `count` removals or moves. */
  #rebuildSphere(): void {
    this.#sphereSet = false;
    this.#stale = 0;
    const mesh = this.mesh;
    if (!mesh) return;
    const array = mesh.instanceMatrix.array as Float32Array;
    for (let slot = 0; slot < this.slots.count; slot += 1) {
      this.#grow(array.subarray(slot * 16, slot * 16 + 16) as Float32Array);
    }
  }

  /** Push `count` and visibility onto the mesh, and keep the cull sphere honest. */
  #sync(): void {
    const mesh = this.mesh;
    if (!mesh) return;
    const count = this.slots.count;
    mesh.count = count;
    // An empty instanced mesh is still a draw call if it is visible.
    mesh.visible = count > 0;
    if (this.#stale > Math.max(count, 4)) this.#rebuildSphere();
  }

  /** A mesh of the table's current capacity, with the live matrices carried over. */
  #remake(): void {
    const old = this.mesh;
    const mesh = new THREE.InstancedMesh(this.part.geometry, this.part.material, this.slots.capacity);
    mesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
    mesh.castShadow = this.shadowed;
    mesh.receiveShadow = this.shadowed;
    // The pool's own sphere, so growing it needs no per-update allocation. It is
    // only read while the mesh is visible, which is once it holds an instance.
    mesh.boundingSphere = this.#sphere;
    if (old) {
      const live = this.slots.count * 16;
      (mesh.instanceMatrix.array as Float32Array).set(
        (old.instanceMatrix.array as Float32Array).subarray(0, live),
      );
      mesh.instanceMatrix.needsUpdate = true;
    }
    this.parent.add(mesh);
    // The mesh never moves on its own: its instances do. Its world matrix is
    // taken once, here, because the parent group stops updating its children.
    mesh.matrixAutoUpdate = false;
    mesh.updateMatrixWorld(true);
    this.#release(old);
    this.mesh = mesh;
  }

  #release(mesh: THREE.InstancedMesh | null): void {
    if (!mesh) return;
    this.parent.remove(mesh);
    // Frees the matrix buffer. The geometry and material are shared.
    mesh.dispose();
  }
}

type Rec = {
  readonly handle: number;
  readonly key: string;
  readonly mapKey: string;
  readonly entityIndex: number;
  readonly model: ModelPools;
  /** The entity's current transform, what every band's slot is written from. */
  readonly matrix: Float32Array;
  /** The bundle drawn, or -1 before the first `setBand`. */
  band: number;
};

type ModelPools = {
  readonly lods: PoolLods;
  /** `pools[band][part]`, parallel to `lods`. */
  readonly pools: readonly (readonly Pool[])[];
  refs: number;
};

export class InstancedModels {
  readonly #parent: THREE.Object3D;
  readonly #shadowed: boolean;
  readonly #byKey = new Map<string, Rec>();
  readonly #byHandle = new Map<number, Rec>();
  readonly #models = new Map<PoolLods, ModelPools>();
  #nextHandle = 1;

  constructor(parent: THREE.Object3D, shadowed: boolean) {
    this.#parent = parent;
    this.#shadowed = shadowed;
  }

  get size(): number {
    return this.#byKey.size;
  }

  has(key: string): boolean {
    return this.#byKey.has(key);
  }

  /** The bundle an entity is drawn in: -1 before its first band, `undefined` if unknown. */
  bandOf(key: string): number | undefined {
    return this.#byKey.get(key)?.band;
  }

  /**
   * Register an entity. It is not drawn until `setBand`. A key already present
   * is left alone.
   */
  add(
    key: string,
    mapKey: string,
    entityIndex: number,
    lods: PoolLods,
    matrix: THREE.Matrix4,
  ): void {
    if (this.#byKey.has(key)) return;
    let model = this.#models.get(lods);
    if (!model) {
      model = {
        lods,
        pools: lods.map((parts) => parts.map((part) => new Pool(part, this.#parent, this.#shadowed))),
        refs: 0,
      };
      this.#models.set(lods, model);
    }
    model.refs += 1;
    const rec: Rec = {
      handle: this.#nextHandle,
      key,
      mapKey,
      entityIndex,
      model,
      matrix: Float32Array.from(matrix.elements),
      band: -1,
    };
    this.#nextHandle += 1;
    this.#byKey.set(key, rec);
    this.#byHandle.set(rec.handle, rec);
  }

  /** Move an entity to a bundle: out of the old one's pools, into the new one's. */
  setBand(key: string, band: number): void {
    const rec = this.#byKey.get(key);
    if (!rec || rec.band === band) return;
    this.#leave(rec);
    const pools = rec.model.pools[band];
    if (!pools) {
      rec.band = -1;
      return;
    }
    for (const pool of pools) pool.add(rec.handle, rec.matrix);
    rec.band = band;
  }

  /** Re-place an entity: an edit, or an edit undone. */
  setMatrix(key: string, matrix: THREE.Matrix4): void {
    const rec = this.#byKey.get(key);
    if (!rec) return;
    rec.matrix.set(matrix.elements);
    const pools = rec.model.pools[rec.band];
    if (!pools) return;
    for (const pool of pools) pool.write(rec.handle, rec.matrix);
  }

  remove(key: string): void {
    const rec = this.#byKey.get(key);
    if (!rec) return;
    this.#leave(rec);
    this.#byKey.delete(key);
    this.#byHandle.delete(rec.handle);
    rec.model.refs -= 1;
    if (rec.model.refs > 0) return;
    for (const pools of rec.model.pools) for (const pool of pools) pool.dispose();
    this.#models.delete(rec.model.lods);
  }

  /** The nearest entity under the ray among the bundles being drawn. */
  pick(raycaster: THREE.Raycaster): InstanceHit | null {
    let best: InstanceHit | null = null;
    for (const model of this.#models.values()) {
      for (const pools of model.pools) {
        for (const pool of pools) {
          const mesh = pool.mesh;
          if (!mesh || pool.slots.count === 0) continue;
          const first = raycaster.intersectObject(mesh, false)[0];
          if (!first || first.instanceId === undefined) continue;
          if (best && first.distance >= best.distance) continue;
          const owner = pool.slots.ownerAt(first.instanceId);
          const rec = owner === undefined ? undefined : this.#byHandle.get(owner);
          if (!rec) continue;
          best = { mapKey: rec.mapKey, entityIndex: rec.entityIndex, distance: first.distance };
        }
      }
    }
    return best;
  }

  stats(): InstanceStats {
    let drawn = 0;
    let meshes = 0;
    let instances = 0;
    for (const model of this.#models.values()) {
      for (const pools of model.pools) {
        for (const pool of pools) {
          if (!pool.mesh) continue;
          meshes += 1;
          if (pool.slots.count === 0) continue;
          drawn += 1;
          instances += pool.slots.count;
        }
      }
    }
    return { drawn, meshes, instances };
  }

  /** Drop every mesh. The shared geometry and materials are the caller's. */
  dispose(): void {
    for (const model of this.#models.values()) {
      for (const pools of model.pools) for (const pool of pools) pool.dispose();
    }
    this.#models.clear();
    this.#byKey.clear();
    this.#byHandle.clear();
  }

  #leave(rec: Rec): void {
    const pools = rec.model.pools[rec.band];
    if (!pools) return;
    for (const pool of pools) pool.remove(rec.handle);
    rec.band = -1;
  }
}
