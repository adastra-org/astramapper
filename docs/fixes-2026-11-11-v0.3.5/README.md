# Fixes and improvements, 2026-11-11 (v0.3.5)

`[ ]` pending, `[~]` partial (see the note), `[x]` done and verified.
Reported from a screenshot of the base game flown over the Sandy Shores side:
hex zones outlined, terrain missing in the area already visited.

## Terrain

- [ ] Terrain disappears from an area already visited instead of staying loaded
  with its LODs (far ground should stay, drawing its coarse bundle). The whole-map
  terrain layer (#5) is meant to be never evicted; check what the hex cell
  streaming (#3/#4) and the model cache (#6) drop.
- [ ] Terrain has no textures (flat grey). Known gap from `docs/phases.md`:
  `terrain_cb_w_4lyr` blends four layers by `COLOUR0` and `TEXCOORD1`, which
  `MeshPart` does not decode yet.

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
