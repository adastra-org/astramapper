# Zone streaming on hex cells

The design for loading the world by zones. The motivation is measured in
[streaming.md](streaming.md): the WebView holds every placement of every placed
map, which costs ~830 MB of JS heap and 300 ms collection pauses, and each model
costs an IPC round trip of ~0.7 s.

## Goals

- The WebView holds only the zone around the camera and prepares the next one.
- Cell size follows density: small in the city, large in the countryside.
- The whole map's terrain is always there; detail streams in on top.
- Memory is a share of the machine's, and what is evicted is not lost.

## Cells

Pointy-top hexagons in axial `(q, r)` on the ground plane (`src/lib/world/hex.ts`;
height does not pick a cell). A disk of radius 4 is 61 cells; at 250 m a cell
that is roughly a kilometre around the camera.

### Uniform

One size, a number of rings around the camera's cell. Simple, and the wrong shape
for GTA: the same ring count is a few city blocks in one place and open country
in another.

### Adaptive (`hexAdaptive.ts`)

Several sizes at once, coarse to fine (finest 100 m by default, each coarser
level double, five levels). Every placement starts in the coarsest grid; a cell
holding more than a threshold (1,000 by default) is split and its placements
move to the next finer grid, repeatedly. The working set is the nearest leaves
until a **placement budget** (50,000 by default) is spent. In the city the
budget is spent within a few hundred metres, in the country it reaches
kilometres, and the memory held is about the same.

Hexagons do not nest, so a fine cell near a split coarse one can overlap its
edge. Each placement still lands in exactly one leaf, which is what matters (H3
makes the same trade).

## The backend index (#3, #4)

The placements no longer live in the WebView. `world_index_build("base")`
(`src-tauri/src/world_index.rs`) reads every map of a group from the same cached
envelopes `world_map` serves, into one set of columns with names and LOD labels
interned across the whole index, and partitions them with `cells.rs`, the Rust
port of `hex.ts` and `hexAdaptive.ts` (pinned to the same fixtures by tests). It
returns only the partition: one row per leaf (level, `q`, `r`, count, mean `z`)
and the split cells. The Hex view draws that partition, so the panel shows the
cells the backend serves.

`world_index_cells(cells, known)` returns, for a set of cells, their placements as
the columns of `placements.rs` plus a `mapSlot` column, **and the models those
cells need**: each as a nested `Kind::Model` envelope, byte for byte what
`world_model` returns, so one reply replaces a `world_asset` and a `world_model`
call per name. `known` is the names the frontend already holds, which are left
out. Models that fail to compile are left out too and the frontend asks for them
on its own.

A reply is bounded: only the first 48 MB of models ride along
(`REPLY_MODEL_BYTES`), compiled a wave at a time on a pool of their own with large
worker stacks. The rest of the names still arrive with their files, and the
per-model loader fetches them as before, paced by the model cache budget. This is
measured, not a guess: against the base game the terrain cell alone names 395
models and 1.8 GB of geometry, the densest leaf 777 models and 2.0 GB, a median
leaf 70 models and 235 MB (`cell_replies_over_the_base_game` in the gate). Without
the cap a single call held that whole reply in the backend and again in the
WebView, and the app ran out of memory.

Terrain is a pseudo-cell (level 255): wanted map-wide and never evicted (#5), so it
is outside the partition. The backend classifies it by the same name tokens as the
frontend (a test pins the two); real flags are #9.

On the frontend, `CellStreamer` (`src/lib/world/cellStreamer.ts`) plans each pass
(`cellPlan.ts`): the working set is the nearest leaves under the budget; the
prefetch ring is the leaves just past it that lie ahead of the direction of travel
(a smoothed camera velocity, the look direction at rest), up to a quarter of the
budget; the keep set is the working set grown by 1.3x, so a camera on a boundary
does not fetch and drop the same cell. Cells go out three to a call, two calls at
once, nearest first. Evicted cells take their geometry with them. `CellStore`
merges each map's parts across the held cells, so `(mapKey, entityIndex)` stays the
identity everywhere (geometry store, edits, selection, projects).

What is no longer in the WebView: the 763,716 placements. The box markers are not
drawn for cell-streamed placements (they were 100 MB of heap), and an individual
`place` of a DLC or custom map still works the old way.

### Defaults, from the real base game

Measured with `cargo test -p astramapper --release --test world_gate -- --ignored
--nocapture density` against the base game (4,597 maps read in 2.2 s, 763,716
placements, 395 of them terrain):

| finest / split above | leaves | leaf size p50 / p99 / max |
|---|---|---|
| 100 m / 500 | 2,312 | 277 / 1,270 / 2,632 |
| 100 m / **1,000** | **1,939** | **331 / 1,303 / 2,632** |
| 100 m / 2,000 | 1,070 | 640 / 1,903 / 2,632 |
| 100 m / 4,000 | 570 | 1,110 / 3,898 / 3,976 |
| 50 m / 1,000 | 2,329 | 274 / 971 / 1,222 |

Reach of the working set (distance to the farthest cell) at a budget of 50,000:
Legion Square 718 m, Sandy Shores 1,058 m, Mount Chiliad 1,150 m, Paleto Bay
1,472 m, LS airport 1,684 m. At 20,000 the city is 420 m; at 100,000 it is 1,075 m
and the open country 1.5-2.2 km. A split threshold of 2,000 doubles the median leaf
(so a cell reply's models) for no gain in city reach; 500 only adds cells. So the
defaults are **finest 100 m, split above 1,000, budget 50,000**. The budget is a
placement count and placements are cheap as columns (~60 bytes); what it really
bounds is how many cells, and so models, are in play, with the byte budget (#6)
bounding the models. To re-measure on another install, run the command above.

The partition is built once per session and kept in memory (the build is the one
slow call, ~2 s on a warm cache); it is rebuilt by `world_index_build`, not when a
single map is placed, since only the base group is indexed.

## Zone clusters (v0.3.0)

Named zones (Airport, City, North...) as **sets of leaf cells**, a layer above
the partition and not a second way of loading. They exist so memory and model
cost can be read per zone, and so a whole zone can be asked for at once.

**Regions** are drawn by hand in `src-tauri/data/zones.json`: named boxes and
polygons in game coordinates (X east, Y north), in priority order. A leaf belongs
to the *first* region that contains its **centre**; the leaves in none fall into a
built-in `countryside` cluster, always present so cluster ids are stable. Every
leaf is in exactly one cluster. A leaf straddling a border goes wholly to one
side, so a border is accurate to the leaf size (100 m to 1.6 km). The terrain
pseudo-cell is map-wide and is in no cluster. `clusters.rs` parses and validates
the file (unique keys, shapes that enclose an area) and a test refuses a bad one,
so it cannot ship broken.

**Wire**: `world_index_build`'s partition envelope gains section 8 (a `u16`
cluster index per leaf) and `clusters` in its metadata (key, name, leaves,
placements, distinct models). A frontend without the section reads every leaf as
in none. `world_index_cluster_measure(key)` compiles (or reads from the cache)
every distinct model of a zone on the model pool and returns the total compiled
bytes, **keeping none of them**; it is on demand and fills the persistent cache
as a side effect.

**Loading** is `CellStreamer.pin(key)`: the zone's leaves are asked for in the
same batches as every other cell, nearest the camera first, after the working set
and before the prefetch ring, and are exempt from eviction until `unpin(key)`,
which evicts them at once (except cells the working set still wants) and returns
their placements so the geometry goes too. The limits are the existing ones: a
reply carries at most 48 MB of models (`REPLY_MODEL_BYTES`) and the models go
through the model cache, which stays under its own byte budget. So pinning a zone
holds its **placements** (columns, ~60 bytes each) and as many of its models as
the cache budget keeps; it does not, and cannot, hold the zone's compiled
models. The **Zones** panel (`ClusterPanel.tsx`, under the cells panel) lists
each zone with its cells, placements, models, how much is held, **Load** /
**Release**, and **Measure**.

### Measured over the base game

`zone_clusters_over_the_base_game` and `zone_cluster_model_sizes_over_the_base_game`
in `src-tauri/tests/world_gate.rs`, defaults (100 m, split above 1,000), 1,939
leaves, 763,321 partitioned placements. Models are distinct archetype names
(LODs and `unresolved` names with no model file included); bytes are the compiled
size of each distinct file once.

| Zone | cells | placements | names | files | compiled | largest | measure |
|---|---|---|---|---|---|---|---|
| Airport | 40 | 10,996 | 2,384 | 1,443 | 3.50 GB | 33.9 MB | 11 s |
| Port | 16 | 8,176 | 1,591 | 1,103 | 2.69 GB | 34.0 MB | 9 s |
| City | 271 | 174,865 | 35,009 | 23,430 | 63.23 GB | 96.5 MB | 152 s |
| Sandy Shores | 118 | 51,301 | 8,833 | 4,332 | 14.03 GB | 64.1 MB | 22 s |
| Paleto Bay | 61 | 22,874 | 5,146 | 2,716 | 8.81 GB | 56.1 MB | 15 s |
| North | 826 | 273,971 | 44,573 | 12,753 | 42.08 GB | 99.0 MB | 78 s |
| South | 592 | 220,526 | 37,844 | 19,071 | 53.10 GB | 143.0 MB | 153 s |
| Countryside | 15 | 612 | 198 | 195 | 0.82 GB | 92.7 MB | 1 s |

No model failed to compile. The takeaway: **no zone's models fit in memory**, the
smallest (Port) is 2.7 GB and City is 63 GB, so "load a whole zone" means its
cells and placements, with models streamed under the cache budget, never its
models at once. The numbers are the reason the cap and the budget exist. Names
shared between zones are counted in each (the sum, 188 GB, is not the game's
size). Countryside is only 15 edge leaves of the map (612 placements), so the
hand-drawn regions cover the playable world.

Not done: an automatic fallback (connected components of leaves by density). The
hand-drawn regions already cover the map, and a density-based split would name
nothing; revisit it only if DLC or custom maps need zones the file does not draw.

## Load order

Per zone: **terrain, then structures at full detail (LOD0), then decals, then
vegetation, then progressively coarser structure LODs.** The first version
(strict tiers over entities already in range) is in `streamingDecision`; see
[streaming.md](streaming.md). The terrain layer for the *whole* map is in (#5,
v0.1.2: always wanted, never unloaded, low detail by the distance LOD swap);
applying the phases per cell on top waits for the cell index (#3). Better
classification than name tokens needs archetype flags and shader names from the
backend (#9).

## Memory

A byte budget, global, as a percentage of total and of available system memory
(a new Rust command; today the WebView cannot see real RAM). Evict farthest ring
first, not least recently used. Evicted models go to the backend's disk cache, so
returning is a read and not a recompile (#6; the model cache half is in as of
v0.1.3, see [streaming.md](streaming.md)).

## Persistence and precompile

The backend already caches compiled models on disk and has `bake` to precompile
them. Two gaps: the frontend re-fetches over IPC every session (a persistent
frontend cache, keyed so it invalidates on a different install, would remove the
wait), and `bake` is not part of onboarding. Offer "precompile everything" there,
with a recommendation from free disk space and core count (#7).

## What is prototyped

- `src/lib/world/hex.ts`, `hexAdaptive.ts`, `hexModel.ts`: the maths, the
  adaptive partition, the shared model (all tested).
- The **Hex view**: a panel (`HexView.tsx`) and an overlay drawn in the 3D scene,
  from the toolbar over the canvas. Uniform and Adaptive modes, with the numbers
  that decide the design: how many placements the working set holds and what share
  of the world that is, and how much of it already has geometry.

It is derived from placements already in memory, so it runs before any backend
work and can be used to choose the defaults against the real base game.

## Rollout

1. **Typed-array placements** (#2): the frontend-only step that removes the heap
   and the pauses, independent of cells.
2. Re-measure `ipc ping` (#8). If the floor remains, batch per cell.
3. **Backend cell index and a batched command** (#3), using the partition the Hex
   view shows (#4). Done in v0.2.1; see [the backend index](#the-backend-index-3-4).
4. Whole-map terrain layer and per-cell phases (#5).
   Zone clusters on top of the leaves in v0.3.0, see
   [zone clusters](#zone-clusters-v030).
5. Memory budget and eviction (#6); persistent cache and onboarding (#7).

## Open questions

- Fly the base game in the app and check that cells stream in and out without a
  spike; tune cells per call, concurrency and the 1.3x keep factor from F3
  (`cells held`, `cell placements held`, `cells pending`, `cell pass`,
  `worldIndex cells ipc`).
- Maps placed one by one (DLC, custom) are not in the index and still load whole.
  Index them too, or keep them as the small-scale path?
- Box markers for cell-streamed placements, and selecting an entity before its
  geometry loads (the selection also goes when its cell is evicted).
- How the working set interacts with each entity's own LOD radius
  (`entityStreamRadius`); related to the earlier idea in #1.
