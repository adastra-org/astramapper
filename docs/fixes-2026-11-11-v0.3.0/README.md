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
- [ ] JS heap bounded while flying (#23): the heap reached 5.5 GB and does not
  come back, with the model cache at its budget.
- [ ] Code Mode on the bridge channel (scripted scenarios, assertions on gauges).
