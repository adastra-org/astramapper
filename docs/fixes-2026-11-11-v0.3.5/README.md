# Fixes and improvements, 2026-11-11 (v0.3.5)

`[ ]` pending, `[~]` partial (see the note), `[x]` done and verified.
Reported from a screenshot of the base game flown over the Sandy Shores side:
hex zones outlined, terrain missing in the area already visited.

## Terrain

- [x] Terrain disappears from an area already visited instead of staying loaded
  with its LODs (far ground should stay, drawing its coarse bundle). The whole-map
  terrain layer (#5) is meant to be never evicted; check what the hex cell
  streaming (#3/#4) and the model cache (#6) drop.
  Fixed in v0.3.9. Cause: #5 finds terrain by name tokens, which catch 395 ground
  placements in the base game; the drawables' own shaders (`terrain_cb_w_4lyr*`
  and kin) mark 5,355. The other ~4,900 (`ch2_04_land02b`, `cs1_11b_land_01`...)
  were ordinary hex-cell models and left with their cell. Now the backend reads
  the shaders once (cached) and tags them class 3, so they are wanted map-wide
  under a byte budget (half the model cache's, 256 MB to 1 GB; the far token
  pieces are exempt). Verified with the `terrain-revisit` scenario (fixed pose
  over Sandy Shores, flight to the LS airport and back at 120 m/s): the distant
  ground band differs by 12.6% of pixels between the first visit and the return
  before, 2-3.5% after (`imageDiff`, tolerance 30), and the return shows no gaps
  (`img/terrain/return-before.jpg` / `return-after.jpg`). `streaming-bounded`
  passes (model cache 2,081 MB against 3,193 MB allowed), `terrain-distance`
  passes (0.25 and 0.28 of the band drawn, minimum 0.05).
- [~] Terrain has no textures (flat grey). Known gap from `docs/phases.md`:
  `terrain_cb_w_4lyr` blends four layers by `COLOUR0` and `TEXCOORD1`, which
  `MeshPart` does not decode yet.
  Partial in v0.3.9: `COLOUR0`, `COLOUR1` and `TEXCOORD1` are decoded and the four
  layers are blended as `TerrainPS.hlsl` does (`docs/formats.md`); the mountains
  and hills now show rock, scrub and soil. Left: about half of a typical ground
  part's layer textures live in other maps' dictionaries, reached through the
  parent TXD chain (`CMapParentTxds`) that `cw-core` does not read yet, so those
  layers blend as flat grey and the lowland reads pale (`img/terrain/first-visit-after.jpg`).

## Placement

- [~] Many structures are misplaced. Cause: a `CEntityDef` stores the *inverse*
  of its drawn orientation (CodeWalker `Quaternion.Invert`) and the world view
  drew the stored one, mirroring every rotation; interior entities of MLOs had
  the same error; and the placement scale put `scaleZ` on the wrong axis. Fixed
  in v0.3.6 (see "Notes on YMAP entity transforms" in `docs/formats.md`), pinned by
  Rust and TS tests. Not ticked until checked in the app at fixed poses.

## Models and LOD

- [x] Trees still do not load their geometry correctly (canopies solid, no alpha
  test, or wrong part). Two causes, both in how a part's shader was read (v0.3.8):
  cutout shaders (`trees_*`, `cutout*`, `normal_*cutout*`; render bucket 3) drew
  with no alpha test, so leaves were solid cards, and `trees_shadow_proxy` /
  `cpv_only` parts, which the reference never draws, were drawn as solid hulls.
  Verified in the app, same poses before and after:
  [hedge](img/models-lod/trees-hedge-before.jpg) /
  [after](img/models-lod/trees-hedge-after.jpg),
  [inside an oak](img/models-lod/trees-oak-before.jpg) /
  [after](img/models-lod/trees-oak-after.jpg). See formats.md "Shaders: cutout,
  proxies and the render bucket".
- [x] Power-line towers switch to a very far LOD as soon as the camera moves a
  little away. The bundle choice is right (checked with `probe`, see
  streaming.md "Pylons (v0.3.8)"): `prop_pylon_01` is bundle 0 to 242 m, 1 to
  484 m, 2 beyond, which is later than the game's own 120/180 m. What read as a
  far LOD was the *content* of the lower bundles and of the `cs1_10_cable_pylon_lod_*`
  SLOD entities: lattice cut out of a card by alpha, drawn without the alpha test
  as a solid dark slab next to the real tower even at 60 m. With the test they are
  lattices at every distance:
  [60 m](img/models-lod/pylon-60m-before.jpg) / [after](img/models-lod/pylon-60m-after.jpg),
  [260 m](img/models-lod/pylon-260m-before.jpg) / [after](img/models-lod/pylon-260m-after.jpg),
  [700 m](img/models-lod/pylon-700m-before.jpg) / [after](img/models-lod/pylon-700m-after.jpg).
- [ ] Follow-up, not done: an entity whose `lodDist` is -1 or 0 (many props, e.g.
  `prop_streetlight_01`) falls back to 100/200/400 m bands and a 50 m streaming
  base, where the reference uses the archetype's `lodDist`; and a parent LOD
  entity is drawn together with its HD children (hierarchy visibility is not
  applied).

## Not in this batch

- Water shader is missing. Not discussed yet.
