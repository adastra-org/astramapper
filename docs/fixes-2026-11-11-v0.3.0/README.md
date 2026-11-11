# Fixes and improvements, 2026-11-11 (v0.3.0)

`[ ]` pending, `[~]` partial (see the note), `[x]` done and verified.
Design and numbers: [../hex-streaming.md](../hex-streaming.md#zone-clusters-v030).

## Zone clusters

- [x] Named regions in a data file (`src-tauri/data/zones.json`), validated at
  parse time, assigned to leaves by centre, leaves in none to Countryside.
  Cargo tests (`clusters.rs`: containment, priority, validation, every leaf in
  exactly one cluster) and the gate against the base game: 1,939 leaves, all in
  exactly one cluster, 15 in Countryside.
- [x] Per-cluster stats: cells, placements, distinct models, in the partition
  envelope (section 8 and `clusters` metadata). Cargo test on the envelope.
- [x] Compiled model bytes per zone, measured: `world_index_cluster_measure`
  and the gate `zone_cluster_model_sizes_over_the_base_game`, run against the
  real base game (table in the design doc; Port 2.7 GB to City 63 GB).
- [x] `CellStreamer.pin` / `unpin`: a zone's leaves are fetched in the normal
  batches, survive eviction, and are released at once. TS tests (fetch order,
  batch size and concurrency, survives flying away, release reports placements,
  overlap with the working set, progress, new partition forgets pins).
- [~] Zones panel (`ClusterPanel.tsx`) with Load, Release and Measure. Typechecks,
  lints, row logic is tested (`clusters.test.ts`); not yet clicked through in the
  running app, and Measure from the panel has only been run through the same
  backend function the gate runs.
- [~] Load a whole zone while flying, with the model cache under its budget.
  Bounded by construction (48 MB of models per reply, the cache evicts); not yet
  watched in the app with F3 (`cells held`, `cell placements held`).
- [ ] Automatic zones (connected components of leaves by density). Not done:
  the hand-drawn regions cover the map.
- [ ] Zones for DLC and custom maps, which are not in the index.

## Debug bridge (v0.3.1)

- [x] Read the console and F3 from a shell, move the camera, click a button, take
  a screenshot (`docs/debug-bridge.md`). Run against the dev app: `/health`, auth
  rejects a request without the token, `/logs`, `/state`, `click world`, `camera`,
  `fly` (a five-waypoint flight over Los Santos) and `/screenshot` all worked.
  Cargo tests for the HTTP parsing, auth and queues; TS tests for the flight maths
  and the commands.
- [x] Cell streaming works in the running app: flying across the city, `cells held`
  stayed 20 to 75 and `cell placements held` under 35,000; the whole-map terrain
  layer is drawn at distance (screenshot). Frame p95 30 to 85 ms and long task p95
  100 to 150 ms during the flight, still above the targets.
- [x] Code Mode on the bridge channel (`docs/code-mode.md`, v0.3.4):
  `bun scripts/bridge.ts run <scenario>`. Unit tested against a fake app; run
  against the real dev app (debug build): `smoke`, `streaming-bounded` and
  `terrain-distance` pass, `heap-flight` fails as it should (below).
- [x] Cell streaming bounded while flying, re-checked by `streaming-bounded`
  (86 s loop at 120 m/s, v0.3.4 debug build): `cells held` 13 to 69, `cell
  placements held` max 29,932, model cache max 2,327 MB under its (moving) budget,
  0 console errors.
- [~] Terrain at distance: `terrain-distance` finds terrain far beyond the
  streamed cells (mountains and islands at the horizon toward Sandy Shores and
  Paleto Bay), but it is separate pieces over the dark grid, not continuous
  ground. Whether that is the intended whole-map layer is not decided here.
- [~] JS heap bounded while flying (#23): textures were never released and kept
  whole IPC responses alive; now reference-counted with their model parts. Same
  flight: heap idle 5.5 GB (growing) -> 3.2 GB (stable, comes back after peaks),
  textures and geometries follow what is near. Not at the 300 MB target: the
  model cache budget allows up to 3 GB.
  `heap-flight` (Code Mode) failed on dev without #25 (heap 6.3 to 9.3 GB in a
  146 s tour); rerun it on a build with #25 before ticking this.
