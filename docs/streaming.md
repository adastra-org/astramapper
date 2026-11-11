# World streaming: how models load, and what was measured

How the World view decides what to load, in what order, and what it cost when
measured. Written from the v0.1.0 work on "nearby models never finish loading".
The zone-based redesign lives in [hex-streaming.md](hex-streaming.md).

## The pipeline

1. **Streaming pass** (`World.tsx`, every 250 ms). `streamingDecision`
   (`src/lib/world.ts`) scans the placed maps and picks up to 24 entities to
   load and the ones to unload.
2. **Order.** A candidate's rank is `phase + (in view ? 0 : wall) + distance`.
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
- #6 `modelCache` never evicts: the heap grows with every model fetched.
- #5 Terrain is only loaded inside each entity's own radius, not map-wide.
- #9 Phase classification is by name tokens.
- #10 One draw call per entity (1,000-4,600 in the city).
