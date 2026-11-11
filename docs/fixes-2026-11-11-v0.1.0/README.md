# Fixes and improvements, 2026-11-11 (v0.1.0)

`[ ]` pending, `[~]` partial (see the note), `[x]` done and verified.
Long-term items are GitHub issues in this repo and on the Ad Astra Roadmap.

## Loading

- [x] Cap in-flight `world_model` requests. F3: peak 30 down to 12.
- [~] Nearby models never finish loading. Queue is ranked by camera distance and
  re-ranked each pass; waiting requests that left range are dropped. Models near
  the camera now arrive first, but each one still waits ~0.7 s for the IPC
  round trip (#8).
- [~] Load order terrain, structures (LOD0), decals, vegetation, then coarse LOD.
  Strict tiers in `streamingDecision`, covered by unit tests; not yet checked by
  eye in the app. Terrain is only per-entity-radius, not map-wide (#5).
- [ ] Terrain of the whole map loaded first (#5).
- [~] Camera freezes when moving. Geometry and materials are shared per model
  and builds are spread over frames (`render` p50 8.2 to 4.4 ms, `frame` p95 344
  to 17 ms). Long tasks of ~320 ms remain from the 827 MB JS heap (#2).

## Performance (long term)

- [ ] Placements as typed arrays instead of objects (#2).
- [ ] IPC latency floor ~0.7 s per call (#8).
- [~] One draw call per entity; instancing (#10). `InstancedMesh` per model, LOD
  bundle and part. Browser harness: 7,915 to 1,090 draw calls (4,600 entities,
  273 models), picking and edits pass. Not yet confirmed in the app with real
  data (F3 `draw calls`).
- [ ] Classify phases from real data, not name tokens (#9).

## Hex view

- [~] Hex view as a panel (top-down). Checked in a browser harness with synthetic
  data; not yet confirmed in the app, where it was not found because its
  checkbox sat at the bottom of the sidebar.
- [~] Hex view in the real-time 3D scene. Implemented as an overlay of the cells
  the loader would hold; not yet checked in the app.
- [~] Cells variable by zone (large in the countryside, small in the city).
  Adaptive mode prototyped, defaults still to choose from real density (#4).
- [ ] Zone streaming on hex cells in the backend (#3).
- [ ] Dynamic memory budget with SSD offload (#6).
- [ ] Persistent cache and precompile in onboarding (#7).

## UI

- [~] Sidebar could not be collapsed. A toolbar over the canvas hides and shows
  it; not yet checked in the app.
- [x] React Doctor findings on `HexView.tsx` (ref writes during render, state
  adjusted in an effect, drag without cancel handler, component length).

See [../streaming.md](../streaming.md) and [../hex-streaming.md](../hex-streaming.md).
