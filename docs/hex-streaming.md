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
holding more than a threshold (2,000 by default) is split and its placements
move to the next finer grid, repeatedly. The working set is the nearest leaves
until a **placement budget** (50,000 by default) is spent. In the city the
budget is spent within a few hundred metres, in the country it reaches
kilometres, and the memory held is about the same.

Hexagons do not nest, so a fine cell near a split coarse one can overlap its
edge. Each placement still lands in exactly one leaf, which is what matters (H3
makes the same trade).

## Load order

Per zone: **terrain, then structures at full detail (LOD0), then decals, then
vegetation, then progressively coarser structure LODs.** The first version
(strict tiers over entities already in range) is in `streamingDecision`; see
[streaming.md](streaming.md). The terrain layer for the *whole* map is in (#5,
v0.1.2: always wanted, never unloaded, low detail by the distance LOD swap);
applying the phases per cell on top waits for the cell index (#3). Decal
and vegetation are classified from archetype flags and bounding boxes read in
Rust (#9, see [streaming.md](streaming.md#archetype-classes-9)); terrain and
anything the data does not classify still use name tokens, and drawable shader
names are not reachable at placement time.

## Memory

A byte budget, global, as a percentage of total and of available system memory
(a new Rust command; today the WebView cannot see real RAM). Evict farthest ring
first, not least recently used. Evicted models go to the backend's disk cache, so
returning is a read and not a recompile (#6; the model cache half is in as of
v0.1.3, see [streaming.md](streaming.md)).

## Persistence and precompile (#7)

The backend caches compiled models on disk (~1 ms a hit), but every session paid
the IPC round trip again for every model. Two pieces remove that and let the
user pay the compile once up front.

### The persistent tier

`src/lib/world/persistentModels.ts` keeps each model's **response buffer** (not
the decoded model, which is typed-array views over that buffer) in IndexedDB
(`idbBlobStore.ts`: a `meta` store with the scope, an `index` store with one
small record per model, a `blobs` store with the bytes). Order of lookup:

1. `ModelCache` in RAM (#6), farthest-unused eviction under the RAM budget.
2. The persistent tier: read, `decodeModel`, put in RAM. No IPC.
3. The backend over IPC; the buffer is written to the tier for next time.

An eviction from RAM is therefore a disk read on return, not an IPC.

**Invalidation.** Everything in the store belongs to one *scope*: the install
stamp (the backend's stamp of `GTA5.exe`, sent in `world_status.installStamp`),
the GPU capability bits (the same texture compiles to different bytes without
BPTC) and the wire format version. Opening under a different scope wipes the
store, so a game update, another GPU or a decoder change never serves stale
bytes. A record that no longer decodes is dropped and refetched.

**Budget.** A quarter of the WebView's storage quota, within 128 MB and 4 GB
(1 GB when the quota is unknown). The tier evicts least recently used first;
distance from a camera is meaningless between sessions. A failing store leaves
the tier closed and the app on the IPC path.

Gauges in the F3 panel: `model disk tier (MB)`, `... hits`, `... misses`.

### Precompile in onboarding

On the first open of an install, while the cache is cold, `Onboarding.tsx` offers
the existing `bake`. The recommendation is `planBake` (`src/lib/bakePlan.ts`),
from the new `hardware_info` command (cores and free space on the drive holding
the asset cache):

| Condition | Recommended |
|---|---|
| Room (cache budget less what it holds, capped by free disk less 5 GB) under 1 GB | Not now |
| Room fits with 5 GB spare, 4+ cores, more than one content group | Everything |
| Otherwise (few cores, tight disk, free space unreadable) | Base game only |

A bake never writes more than the cache budget, so "everything" means every
content group, most-placed models first, up to that budget. The answer is stored
per install stamp in the settings store, so it is asked once and again after a
game update. The Asset cache panel in World still bakes whatever groups are
ticked.

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
   view shows (#4).
4. Whole-map terrain layer and per-cell phases (#5).
5. Memory budget and eviction (#6); persistent cache and onboarding (#7), done.

## Open questions

- Default threshold and budget, from real density (the Hex view prints it).
- Compute the partition once and store it, or rebuild when maps change?
- How the working set interacts with each entity's own LOD radius
  (`entityStreamRadius`); related to the earlier idea in #1.
