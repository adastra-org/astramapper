import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { InstancedModels, type PoolLods } from "./instancedModels";

/** A model with `bands` bundles of one unit-triangle part each. */
function model(bands = 2, parts = 1): PoolLods {
  return Array.from({ length: bands }, () =>
    Array.from({ length: parts }, () => {
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute(
        "position",
        new THREE.BufferAttribute(new Float32Array([-1, -1, 0, 1, -1, 0, 0, 1, 0]), 3),
      );
      return { geometry, material: new THREE.MeshBasicMaterial({ side: THREE.DoubleSide }) };
    }),
  );
}

function at(x: number, y = 0, z = 0): THREE.Matrix4 {
  return new THREE.Matrix4().makeTranslation(x, y, z);
}

function setup(shadowed = false): { group: THREE.Group; models: InstancedModels } {
  const group = new THREE.Group();
  return { group, models: new InstancedModels(group, shadowed) };
}

describe("InstancedModels draw calls", () => {
  test("any number of entities of one model in one band is one mesh per part", () => {
    const { group, models } = setup();
    const lods = model(2, 3);
    for (let i = 0; i < 50; i += 1) {
      models.add(`m:${i}`, "m", i, lods, at(i));
      models.setBand(`m:${i}`, 0);
    }
    expect(models.stats()).toEqual({ drawn: 3, meshes: 3, instances: 150 });
    expect(group.children.length).toBe(3);
  });

  test("an entity that is not banded yet draws nothing", () => {
    const { models } = setup();
    models.add("m:0", "m", 0, model(), at(0));
    expect(models.stats().drawn).toBe(0);
    expect(models.bandOf("m:0")).toBe(-1);
  });

  test("distinct models do not share a mesh", () => {
    const { models } = setup();
    for (let i = 0; i < 4; i += 1) {
      models.add(`m:${i}`, "m", i, model(), at(i));
      models.setBand(`m:${i}`, 0);
    }
    expect(models.stats().drawn).toBe(4);
  });
});

describe("InstancedModels bands", () => {
  test("a band swap moves the instance between pools and parks the empty one", () => {
    const { models } = setup();
    const lods = model(2);
    models.add("m:0", "m", 0, lods, at(0));
    models.setBand("m:0", 0);
    models.setBand("m:0", 1);
    expect(models.bandOf("m:0")).toBe(1);
    expect(models.stats()).toEqual({ drawn: 1, meshes: 2, instances: 1 });
  });

  test("setting the same band again does nothing", () => {
    const { models } = setup();
    models.add("m:0", "m", 0, model(), at(0));
    models.setBand("m:0", 0);
    models.setBand("m:0", 0);
    expect(models.stats().instances).toBe(1);
  });

  test("a band the model does not have leaves the entity undrawn", () => {
    const { models } = setup();
    models.add("m:0", "m", 0, model(1), at(0));
    models.setBand("m:0", 3);
    expect(models.stats().drawn).toBe(0);
    expect(models.bandOf("m:0")).toBe(-1);
  });
});

describe("InstancedModels capacity", () => {
  test("grows past the first capacity and keeps every matrix", () => {
    const { group, models } = setup();
    const lods = model(1);
    for (let i = 0; i < 37; i += 1) {
      models.add(`m:${i}`, "m", i, lods, at(i));
      models.setBand(`m:${i}`, 0);
    }
    const mesh = group.children[0] as THREE.InstancedMesh;
    expect(group.children.length).toBe(1);
    expect(mesh.count).toBe(37);
    expect(mesh.instanceMatrix.count).toBeGreaterThanOrEqual(37);
    const out = new THREE.Matrix4();
    for (let i = 0; i < 37; i += 1) {
      mesh.getMatrixAt(i, out);
      expect(out.elements[12]).toBe(i);
    }
  });

  test("removal keeps the survivors' matrices and shrinks the mesh", () => {
    const { group, models } = setup();
    const lods = model(1);
    for (let i = 0; i < 40; i += 1) {
      models.add(`m:${i}`, "m", i, lods, at(i));
      models.setBand(`m:${i}`, 0);
    }
    for (let i = 0; i < 36; i += 1) models.remove(`m:${i}`);
    const mesh = group.children[0] as THREE.InstancedMesh;
    expect(group.children.length).toBe(1);
    expect(mesh.count).toBe(4);
    expect(mesh.instanceMatrix.count).toBeLessThan(40);
    const xs: number[] = [];
    const out = new THREE.Matrix4();
    for (let i = 0; i < mesh.count; i += 1) {
      mesh.getMatrixAt(i, out);
      xs.push(out.elements[12]!);
    }
    expect(new Set(xs)).toEqual(new Set([36, 37, 38, 39]));
  });

  test("removing the last entity of a model drops its meshes", () => {
    const { group, models } = setup();
    const lods = model(2, 2);
    models.add("m:0", "m", 0, lods, at(0));
    models.setBand("m:0", 0);
    models.add("m:1", "m", 1, lods, at(1));
    models.setBand("m:1", 1);
    models.remove("m:0");
    expect(group.children.length).toBeGreaterThan(0);
    models.remove("m:1");
    expect(group.children.length).toBe(0);
    expect(models.stats()).toEqual({ drawn: 0, meshes: 0, instances: 0 });
    expect(models.size).toBe(0);
  });
});

describe("InstancedModels edits", () => {
  test("setMatrix rewrites the slot and survives a band swap", () => {
    const { group, models } = setup();
    const lods = model(2);
    models.add("m:0", "m", 0, lods, at(0));
    models.add("m:1", "m", 1, lods, at(5));
    models.setBand("m:0", 0);
    models.setBand("m:1", 0);
    models.setMatrix("m:1", at(99));
    const near = group.children[0] as THREE.InstancedMesh;
    const out = new THREE.Matrix4();
    near.getMatrixAt(1, out);
    expect(out.elements[12]).toBe(99);
    models.setBand("m:1", 1);
    const far = group.children[1] as THREE.InstancedMesh;
    far.getMatrixAt(0, out);
    expect(out.elements[12]).toBe(99);
  });

  test("an edit before the first band is used when the band arrives", () => {
    const { group, models } = setup();
    models.add("m:0", "m", 0, model(1), at(0));
    models.setMatrix("m:0", at(7));
    models.setBand("m:0", 0);
    const out = new THREE.Matrix4();
    (group.children[0] as THREE.InstancedMesh).getMatrixAt(0, out);
    expect(out.elements[12]).toBe(7);
  });
});

function ray(x: number, y = 0): THREE.Raycaster {
  return new THREE.Raycaster(new THREE.Vector3(x, y, 5), new THREE.Vector3(0, 0, -1));
}

describe("InstancedModels picking", () => {
  test("resolves an instance id to its entity", () => {
    const { group, models } = setup();
    const lods = model(1);
    for (let i = 0; i < 5; i += 1) {
      models.add(`m:${10 + i}`, "m", 10 + i, lods, at(i * 10));
      models.setBand(`m:${10 + i}`, 0);
    }
    group.updateMatrixWorld(true);
    expect(models.pick(ray(30, 0))).toMatchObject({ mapKey: "m", entityIndex: 13 });
    expect(models.pick(ray(500, 0))).toBeNull();
  });

  test("still resolves correctly after a swap-remove moved an instance", () => {
    const { group, models } = setup();
    const lods = model(1);
    for (let i = 0; i < 4; i += 1) {
      models.add(`m:${i}`, "m", i, lods, at(i * 10));
      models.setBand(`m:${i}`, 0);
    }
    models.remove("m:1");
    group.updateMatrixWorld(true);
    expect(models.pick(ray(30, 0))).toMatchObject({ entityIndex: 3 });
    expect(models.pick(ray(10, 0))).toBeNull();
  });

  test("only the drawn band is pickable", () => {
    const { group, models } = setup();
    const lods = model(2);
    models.add("m:0", "m", 0, lods, at(0));
    models.setBand("m:0", 0);
    models.setBand("m:0", 1);
    group.updateMatrixWorld(true);
    // One hit, from the far bundle's mesh: the near one is empty.
    expect(models.pick(ray(0, 0))).toMatchObject({ entityIndex: 0 });
    models.remove("m:0");
    group.updateMatrixWorld(true);
    expect(models.pick(ray(0, 0))).toBeNull();
  });

  test("the nearest of two entities wins", () => {
    const { group, models } = setup();
    models.add("a:0", "a", 0, model(1), new THREE.Matrix4().makeTranslation(0, 0, 0));
    models.add("b:0", "b", 0, model(1), new THREE.Matrix4().makeTranslation(0, 0, 3));
    models.setBand("a:0", 0);
    models.setBand("b:0", 0);
    group.updateMatrixWorld(true);
    expect(models.pick(ray(0, 0))).toMatchObject({ mapKey: "b" });
  });
});

describe("InstancedModels culling", () => {
  test("the bounding sphere follows the instances and tightens after removals", () => {
    const { group, models } = setup();
    const lods = model(1);
    for (let i = 0; i < 10; i += 1) {
      models.add(`m:${i}`, "m", i, lods, at(i * 1000));
      models.setBand(`m:${i}`, 0);
    }
    const mesh = group.children[0] as THREE.InstancedMesh;
    expect(mesh.boundingSphere!.radius).toBeGreaterThan(4000);
    for (let i = 1; i < 10; i += 1) models.remove(`m:${i}`);
    // Stale until enough operations have passed, then exact again.
    for (let i = 0; i < 20; i += 1) models.setMatrix("m:0", at(0));
    expect(mesh.boundingSphere!.radius).toBeLessThan(5);
  });

  test("dispose clears everything", () => {
    const { group, models } = setup();
    models.add("m:0", "m", 0, model(), at(0));
    models.setBand("m:0", 0);
    models.dispose();
    expect(group.children.length).toBe(0);
    expect(models.size).toBe(0);
  });
});
