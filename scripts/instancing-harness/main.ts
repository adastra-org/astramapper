/**
 * Browser harness for issue #10: per-entity meshes against `InstancedModels`,
 * in a real WebGL renderer, on a synthetic city (4,600 entities, 273 models,
 * two LOD bundles of two parts). Serve with `bun run dev` and open
 * `/scripts/instancing-harness/index.html`. Result: `window.instancingResult`.
 *
 * It exercises what the World view uses: add, band swap, edit, remove, growth,
 * and picking by instance id. It does not need Tauri.
 */
import * as THREE from "three";
import { InstancedModels, type PoolLods } from "../../src/components/world/instancedModels";

const ENTITIES = 4600;
const MODELS = 273;
const SPAN = 2000;

let seed = 12345;
const rand = (): number => {
  seed = (seed * 1103515245 + 12345) & 0x7fffffff;
  return seed / 0x7fffffff;
};

function makeModel(): PoolLods {
  return [0, 1].map((band) =>
    [0, 1].map((part) => {
      const geometry = new THREE.BoxGeometry(4 + part, 4, 4 + band);
      return { geometry, material: new THREE.MeshStandardMaterial({ side: THREE.DoubleSide }) };
    }),
  );
}

type Spec = { key: string; model: number; x: number; y: number; band: number };
const models = Array.from({ length: MODELS }, makeModel);
const specs: Spec[] = Array.from({ length: ENTITIES }, (_, i) => ({
  key: `m:${i}`,
  model: Math.floor(rand() * MODELS),
  x: (rand() - 0.5) * SPAN,
  y: (rand() - 0.5) * SPAN,
  band: rand() < 0.5 ? 0 : 1,
}));

function mount(): { renderer: THREE.WebGLRenderer; scene: THREE.Scene; camera: THREE.PerspectiveCamera; world: THREE.Group } {
  const renderer = new THREE.WebGLRenderer({ antialias: false });
  renderer.setSize(800, 600);
  document.body.appendChild(renderer.domElement);
  const scene = new THREE.Scene();
  scene.add(new THREE.HemisphereLight(0xffffff, 0x333333, 1));
  const camera = new THREE.PerspectiveCamera(60, 800 / 600, 0.1, 20000);
  camera.position.set(0, 0, 1500);
  camera.lookAt(0, 0, 0);
  const world = new THREE.Group();
  scene.add(world);
  return { renderer, scene, camera, world };
}

function measure(renderer: THREE.WebGLRenderer, scene: THREE.Scene, camera: THREE.Camera): { calls: number; triangles: number; ms: number } {
  const t0 = performance.now();
  for (let i = 0; i < 20; i += 1) renderer.render(scene, camera);
  const ms = (performance.now() - t0) / 20;
  return { calls: renderer.info.render.calls, triangles: renderer.info.render.triangles, ms };
}

function matrixOf(s: { x: number; y: number }): THREE.Matrix4 {
  return new THREE.Matrix4().makeTranslation(s.x, s.y, 0);
}

function rayAt(x: number, y: number): THREE.Raycaster {
  return new THREE.Raycaster(new THREE.Vector3(x, y, 100), new THREE.Vector3(0, 0, -1));
}

// ---- legacy: one Mesh per entity per part of the drawn band ----
function legacy() {
  const { renderer, scene, camera, world } = mount();
  const holders = new Map<string, THREE.Group>();
  const meshesOf: THREE.Mesh[] = [];
  for (const s of specs) {
    const holder = new THREE.Group();
    holder.applyMatrix4(matrixOf(s));
    models[s.model]!.forEach((parts, band) => {
      for (const part of parts) {
        const mesh = new THREE.Mesh(part.geometry, part.material);
        mesh.visible = band === s.band;
        holder.add(mesh);
        meshesOf.push(mesh);
      }
    });
    world.add(holder);
    holders.set(s.key, holder);
  }
  world.updateMatrixWorld(true);
  const stats = measure(renderer, scene, camera);
  renderer.dispose();
  renderer.domElement.remove();
  return stats;
}

// ---- instanced ----
function instanced() {
  const { renderer, scene, camera, world } = mount();
  const pools = new InstancedModels(world, false);
  const t0 = performance.now();
  for (const s of specs) {
    pools.add(s.key, "m", Number(s.key.slice(2)), models[s.model]!, matrixOf(s));
    pools.setBand(s.key, s.band);
  }
  const build = performance.now() - t0;
  world.updateMatrixWorld(true);
  const stats = measure(renderer, scene, camera);
  const checks: Record<string, unknown> = {};

  // Picking: aim at an entity's centre; the nearest hit must be an entity at that spot.
  let pickOk = 0;
  const sample = specs.slice(0, 200);
  for (const s of sample) {
    const hit = pools.pick(rayAt(s.x, s.y));
    const hitSpec = hit ? specs[hit.entityIndex] : undefined;
    if (hit && hitSpec && Math.abs(hitSpec.x - s.x) < 5 && Math.abs(hitSpec.y - s.y) < 5) pickOk += 1;
  }
  checks.pickedOf200 = pickOk;

  // Edit: move one entity far away; pick follows it.
  const e = specs[5]!;
  pools.setMatrix(e.key, new THREE.Matrix4().makeTranslation(3000, 3000, 0));
  world.updateMatrixWorld(true);
  const moved = pools.pick(rayAt(3000, 3000));
  checks.editFollowed = moved?.entityIndex === 5;

  // Band swap on every entity, then back; draw-call count must come back.
  const swapT0 = performance.now();
  for (const s of specs) pools.setBand(s.key, 1 - s.band);
  for (const s of specs) pools.setBand(s.key, s.band);
  checks.swapAllMs = performance.now() - swapT0;

  // Stream out half, then back in (growth and shrink).
  for (let i = 0; i < ENTITIES; i += 2) pools.remove(specs[i]!.key);
  const half = pools.stats();
  for (let i = 0; i < ENTITIES; i += 2) {
    const s = specs[i]!;
    pools.add(s.key, "m", i, models[s.model]!, matrixOf(s));
    pools.setBand(s.key, s.band);
  }
  pools.setMatrix(e.key, matrixOf(e));
  world.updateMatrixWorld(true);
  const after = measure(renderer, scene, camera);
  checks.callsAfterChurn = after.calls;
  checks.instancesHalf = half.instances;
  checks.instancesAfter = pools.stats().instances;
  renderer.dispose();
  renderer.domElement.remove();
  return { ...stats, build, checks, stats: pools.stats() };
}

const before = legacy();
const after = instanced();
const result = { entities: ENTITIES, models: MODELS, before, after };
(window as unknown as { instancingResult: unknown }).instancingResult = result;
document.getElementById("out")!.textContent = JSON.stringify(result, null, 2);
