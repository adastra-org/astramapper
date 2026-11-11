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
   tiers. Decal and vegetation come from the archetype class the backend reads
   from the `.ytyp` record (below); terrain, and anything the data does not
   classify, from whole-word tokens in the archetype name (`streamPenalty`);
   anything else is decided by `lodLevel`. A misclassification only changes
   order, never drops an entity.
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

## IPC cost: where the remaining ~130 ms is (#8)

Measured with `scripts/ipc-bench.ts` (release build, base game, WebView2,
driven over CDP from inside the page, nothing else loading). The ~130 ms is not
a per-call floor. It is **queueing behind bytes**.

An idle call costs about 1 ms, whatever the command:

| probe | p50 |
|---|---|
| `setTimeout(0)` (JS floor) | 4.6 ms |
| `app_info` / `game_is_open` (sync, trivial) | 0.8 ms |
| `world_status` (sync, `content_groups` over 5,378 archives) | 1.5 ms |
| `world_asset` (async, `spawn_blocking`, tiny reply) | 0.9 ms |
| `world_model`, backend only (`perf_report`, 247 hits of 4.4 MB) | 1.2 ms |
| `world_model` round trip, one at a time, 4.4 MB | 37 ms (~120 MB/s) |

The transport moves about 115 MB/s on one stream and saturates near **177 MB/s**
(1, 2, 4, 8, 12 in flight: 114, 148, 166, 176, 177 MB/s). Replies share that
pipe in order, so *any* call, including a one-byte ping, waits for the bytes
already in front of it:

| `world_status` while N x 4.4 MB model transfers are in flight | p50 |
|---|---|
| 0 | 1.4 ms |
| 4 | 74.5 ms |
| 12 | 261 ms |

That is the `ipc ping` p50 of 128 ms in the table above: with 12 requests in
flight and models averaging about 1 MB, the ping waits ~130 ms for the replies
queued ahead of it. The main thread is not the cause (a ping under a 50 ms
long-task loop is still 1.5 ms p50, 52 ms max), and neither is the JS heap.

What this rules out:

- **A custom URI scheme for model bytes.** Prototyped
  (`register_asynchronous_uri_scheme_protocol`, `fetch` on the JS side): 37.2 ms
  vs 37.6 ms for `invoke` sequentially, 630 ms vs 643 ms with 48 in flight.
  Tauri's `invoke` already is a custom-protocol request on WebView2 and
  `tauri::ipc::Response` already ships raw bytes, so there is nothing to gain.
  Not merged.
- **Batching models per call (#3).** It saves the ~1 ms per call and nothing
  else; the bytes still go through the same pipe at the same rate.
- **Serialization.** Replies are raw `Vec<u8>`, the backend side is 1.2 ms.

What would move it, none of it done:

- **Fewer bytes per model.** Every LOD is sent whether or not the camera needs
  it, and textures shared between models are sent once per model. A 12-in-flight
  window of 4.4 MB models is 53 MB, ~300 ms on this pipe.
- **A lower in-flight limit.** Throughput is within 6% of its ceiling at 4 in
  flight (166 vs 177 MB/s) and a ping behind them costs 75 ms instead of 261 ms.
  Not changed here: this was measured on cache hits of one large model, where
  concurrency buys nothing. On a cold cache the backend compiles for tens of ms
  per model and concurrency hides that, so the limit needs a measurement on a
  real flight before it moves.

Streaming throughput is not hurt by this: a model arrives when its bytes have
crossed, however many are queued. It only shows as latency on a call that
should have been quick (Show model, `world_asset`, the F3 ping), and that call
waits at most one window of transfers.

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
- #8 IPC latency: the ~0.7 s floor was the main thread, not the IPC. The
  ~130 ms left is replies queueing behind model bytes on a ~177 MB/s transport,
  not a per-call cost; a URI scheme and batching do not move it (see "IPC
  cost"). Closed. What could: fewer bytes per model, a lower in-flight limit.
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
  coarse bundle. Still open: it is by name tokens (terrain has no safe data
  signal, see #9), it costs memory with no eviction (#6) and a draw call per piece (#10), and the phases are not yet
  applied per cell (needs the cell index, #3). Not measured against the base
  game yet: with strict tiers, structures wait for every terrain piece map-wide.
- #9 Decal and vegetation are classified from archetype data (v0.1.6), terrain
  is still by name; see [Archetype classes](#archetype-classes-9). Not done:
  drawable shader names.
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

## Archetype classes (#9)

`world_map` already needs the archetype index (`Loaded::archetypes()`, one pass
over every `.ytyp`, shared with the MLO expansion), so classifying costs nothing
extra: `classify_archetype` (`crates/cw-core/src/filetypes/class.rs`) runs over
159,375 archetypes in 2.4 ms. The class rides in the placement envelope's
metadata as `nameClasses`, a `u8` per row of the name table (not a column, so the
section order is untouched): `0` none, `1` decal, `2` vegetation. The frontend
reads it with `PlacementColumns.archetypeClass(row)`; a placement carries
`archetypeClass` only when it is non-zero, and an envelope without `nameClasses`
(an old cache) decodes as no class. The map cache key mixes in
`PLACEMENTS_FORMAT`, so existing cached maps are rebuilt once.

What decides it, measured on the base game with
`cargo run --release -p cw-core --example archetype_flags`:

| Class | Rule | Evidence |
|---|---|---|
| vegetation | drawable or fragment with *Double-sided rendering* (`1 << 16`) | 206 of the 262 archetypes it hits are named as vegetation, and the other 56 are kelp, weeds, cattails, palmetto saplings the name tokens miss; it is on 0.07% of the rest |
| decal | drawable with *Dont Cast Shadows* (`1 << 13`), thinnest bounding-box axis under 0.1 m, second-thinnest at least 1 m | 887 archetypes; beyond the 52 named decals they are puddles, runway paint, overlays, rail clips, dirt, and also thin glass and billboard planes (loaded in the decal phase, which is harmless) |
| terrain | none | no flag separates it, and "large and flat" hits 444 of 932 terrain names but also 9,558 others (road splines, walls, wires). The ground layer is never unloaded, so a false positive would stay resident; terrain stays on its name |

Precedence in `streamPenaltyOf`: terrain by name, then the data class, then the
other name tokens, then the LOD tier. A class from the data overrides a name that
disagrees.

**Shader names are not reachable cheaply.** A drawable's shader names (the decal
shaders are the best signal for decals and the only one for terrain) live in the
model file, not in the `.ytyp`. Placement time knows archetype hashes only; to
read shaders it would have to extract and parse every distinct model of every
map (tens of thousands) inside `world_map`, which today reads all 4,597 maps in
about 2 s. `WirePart.shader` already reaches the frontend with each compiled
model, so the class could be refined after load (and a per-install table could be
built by `bake`), but that is a separate piece of work, not done here.
