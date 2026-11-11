# World streaming: how models load, and what was measured

How the World view decides what to load, in what order, and what it cost when
measured. Written from the v0.1.0 work on "nearby models never finish loading".
The zone-based redesign lives in [hex-streaming.md](hex-streaming.md).

## The pipeline

1. **Streaming pass** (`World.tsx`, every 250 ms). `streamingDecision`
   (`src/lib/world.ts`) scans the placed maps and picks up to 24 entities to
   load and the ones to unload.
2. **Order.** A candidate's rank is `phase + (in view ? 0 : wall) + distance`.
   Out of view it is a candidate only within half its tier radius (below).
   Phases are strict tiers, in order: terrain, structures at full detail
   (`HD` / `ORPHAN_HD`), decals, vegetation, then the coarse `LOD` / `SLOD`
   tiers. Terrain, decal and vegetation are recognised by whole-word tokens in
   the archetype name (`streamPenalty`); anything else is decided by
   `lodLevel`. A misclassified name only changes order, never drops an entity.
3. **Request queue** (`src/lib/world/requestQueue.ts`). At most 12
   `world_model` requests are in flight; the rest wait. Every pass re-ranks the
   waiting ones against the camera's current position and drops those whose
   placement has left streaming range, before they cost an IPC call. The queue
   holds at most 48 waiting streamed requests; past that, a better candidate
   evicts the worst. User actions (Show model, preload) go first and are never
   dropped.
4. **Caches.** An entity whose asset and model are already cached skips the
   queue and is added at once. Two entities sharing a model share one request.
5. **Geometry sync.** Models that arrived are built into meshes at most 6 per
   frame, so a burst of arrivals is not one upload spike.
6. **Shared geometry.** `BufferGeometry` and material are built once per model
   and shared by every entity of it, refcounted, freed with the last entity.
7. **Instanced drawing** (v0.1.5, `src/components/world/instancedModels.ts`).
   One `InstancedMesh` per model, LOD bundle and part; see below.

## Range LODs and the view (#1, v0.1.4)

Three layers decide what is on screen at a given distance, each at its own time:

| Layer | Where | What it does |
|---|---|---|
| Sphere loading | `streamingDecision` | An entity is a candidate inside its tier radius (`max(lodDist, 50)` x 4 / 7 / 14 / 20 for `ORPHAN_HD` / `HD` / `LOD` / `SLOD`); it unloads at 1.5x that. Terrain is exempt (#5). |
| View | `streamingDecision`, `stillStreamWanted` | Inside the view cone, or within 60 m, an entity loads out to its full radius and ranks first. Outside it, it is a candidate only within `STREAM_OFF_VIEW_RADIUS` (0.5) of its radius. |
| Distance LOD | per-frame swap, `lodBandForDistance` | One bundle per entity: `lodDist`, 2x, 4x, beyond. Hidden bundles are not drawn or uploaded. |
| Frustum culling | three.js | Every mesh keeps `frustumCulled = true`, so off-screen meshes are skipped at draw time. |

The off-view cut is a smaller sphere, not a frustum cull, on purpose: loading
starts at 0.5 of the radius and unloading at 1.5, so something that drifts in
and out of the cone does not thrash, and turning around finds the near half of
what was behind already resident. The queue applies the same cut
(`stillStreamWanted` takes the view), so a request for something the camera
turned away from is dropped before it costs an IPC call. With no view (preload,
tests) the whole sphere stays a candidate.

Not done: choosing the LOD at *load* time. `world_model` returns every bundle,
so a far entity still pays for the HD ones over IPC. That needs a bundle
selector in the command and a model cache keyed by it.

## What was measured (release build, F3)

| | before | after |
|---|---|---|
| `worldModel in flight` peak | 30 | 12 |
| `geometries` (829 vs 273 resident models) | 1,512 | 51 |
| `render` p50 / p95 | 8.2 / 15 ms | 4.4 / 6.2 ms |
| `frame` p95 (city, queue limit 4 vs 12) | 344 ms | 17 ms |

What did **not** move:

- `worldModel ipc` p50 stays at **~0.7-0.9 s** whatever the concurrency (4, 11
  or 30 in flight) and whatever the response size, and a call that returns a few
  bytes (`worldAsset ipc`) costs about the same. The backend work is 0.00-30 ms.
  So it is a per-call cost, not queueing. See #8.
- **`JS heap used`: 827 MB**, `long task` p50 **320 ms** (71 of them), `main
  thread lag` p95 **813 ms**. The WebView holds ~760,000 placements as objects,
  and every collection pauses the main thread. This, not the GPU (`render` p50
  5.6 ms), is what freezes the camera. See #2.

## After the columnar placements and the memoized list

The ~0.7 s per call was the main thread being busy, not the IPC. `World`
re-renders on every geometry store change, and the sidebar rendered one row per
placed map inline (4,601 with the base game): one ~350 ms task per batch of
models, while `tick js` stayed ~16 ms. The list is now `PlacedList`
(memoized). Release build, F3, a long flight over the base game:

| | before (heap fix only) | after |
|---|---|---|
| `fps` | 34 | 108 |
| `frame` p50 / p95 | 29 / 403 ms | 9.3 / 18 ms |
| `long task` p50 | 351 ms | 52 ms |
| `ipc ping` p50 | 749 ms | 128 ms |
| `worldModel ipc` p50 | 684 ms | 129 ms |
| `JS heap used` | 685 MB | 922 MB (longer run, no eviction) |

The heap is not under the 300 MB target: `modelCache` keeps every model fetched
(598 MB over the wire in this run). That is #6.

## F3 probes

- `tick js` (the whole frame callback), `tick lod update`.

- `worldModel ipc`, split by response size; `worldAsset ipc` (a tiny reply, the
  floor of one call); `ipc ping (world_status)` once a second.
- `main thread lag` (how late a 100 ms timer fires), `long task`, `JS heap used
  (MB)`, `render`, `stream pass`, `stream requeue`, `geometry sync`, `models
  built (last sync)`.
- `model queue active / waiting / dropped`.
- `hex bin (uniform)`, `hex build (adaptive)` when the Hex view is open.

## Known limits

- #2 Placements are columnar (`src/lib/world/columns.ts`): typed-array views
  over the response buffer, a `Placed` built only on demand. Done.
- #8 IPC latency: the ~0.7 s floor was the main thread, not the IPC (see below).
  What is left (~130 ms p50) is worth one more look.
- #6 `modelCache` is a `ModelCache` under a byte budget (v0.1.3): a fifth of
  available RAM, at most a tenth of the total, within 256 MB and 3 GB, from the
  new `system_memory` command, refreshed every 10 s. Over budget it drops the
  models no loaded geometry uses, farthest from the camera first (not LRU). What
  it does not do yet: shrink what the renderer holds (loaded entities and the
  terrain layer are never evicted by it), and the budget is not measured on the
  base game. F3: `model cache (MB)`, `model cache budget (MB)`,
  `model cache evicted (last pass)`, `model cache evict`.
- #5 Terrain is a whole-map layer (v0.1.2): `streamingDecision` scans each map's
  terrain rows (`terrainRowsOf`, classified once per map) ahead of everything
  else, ignoring the map's box and the entity's radius, and never unloads them.
  Detail comes from the existing distance LOD swap, so far ground draws its
  coarse bundle. Still open: it is by name tokens (#9), it costs memory with no
  eviction (#6) and a draw call per piece (#10), and the phases are not yet
  applied per cell (needs the cell index, #3). Not measured against the base
  game yet: with strict tiers, structures wait for every terrain piece map-wide.
- #9 Phase classification is by name tokens.
- #10 Instanced per model, LOD bundle and part (v0.1.5), see below. Not yet
  measured against the base game in the app.

## Instanced models (#10)

An entity is no longer a `Group` of `Mesh`es but an *instance* in the pools of
its model. A pool is one `InstancedMesh` for one shared part.

- **Draw calls** are one per (model, drawn bundle, part) with entities in it,
  not one per entity. F3: `draw calls`, `instanced meshes drawn`,
  `instanced meshes`, `instances drawn` (sampled every 15 frames).
- **Slots** (`src/lib/world/instanceSlots.ts`) are dense, `0..count-1`. Removing
  swaps the last live slot into the hole, so nothing drawn is a hole. A free
  list would leave holes that still cost vertex work (parked as zero scale).
- **Capacity** starts at 4, doubles when full, halves when under a quarter full
  (hysteresis). Resizing makes a new `InstancedMesh` over the same shared
  geometry and material: only the matrices are re-uploaded.
- **Band swap** moves the entity from the old bundle's pools to the new one's;
  pools are kept while any entity of the model is resident, so it is a slot move,
  not a mesh build. An entity not yet banded is drawn by none.
- **Picking** raycasts the drawn pools; `instanceId` -> slot owner -> entity.
- **Edits** rewrite the entity's slot, only when the edit object changed. The
  matrix is stored per entity, so an edit survives a band swap and a reload; an
  edit that goes away puts the entity back on its placement.
- **Culling** is per pool by bounding sphere: grown on add/move, rebuilt after
  as many removals/moves as instances (amortised constant). A stale sphere is
  only too big. Trade-off: a pool spread over the whole map is rarely culled, so
  triangles drawn rise while draw calls fall.

Measured in a browser harness (`scripts/instancing-harness/`, run `bun run dev`
and open `/scripts/instancing-harness/index.html`): 4,600 entities, 273 models,
two bundles of two parts, random over 2 km, software-rendered WebGL, camera
overhead.

| | per-entity meshes | instanced |
|---|---|---|
| draw calls | 7,915 | 1,090 |
| triangles | 94,980 | 110,376 |
| render (ms, software GL) | 54.9 | 24.5 |

The harness also checks picking by instance id (200 of 200), an edit moving an
entity's pick target, a band swap of every entity, and 4,600 -> 2,300 -> 4,600
churn (draw calls identical after). It drives `InstancedModels` directly: the
World view itself, with real data, needs the Tauri app and has not been run.
