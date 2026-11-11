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

- [ ] Many structures are misplaced. Suspected: YMAP entity position, rotation
  (quaternion convention) or scale, or parent/child entity offsets. Not confirmed.
  Reproduce with the bridge at a spot with visible misplacement, then compare a
  placement's decoded transform against CodeWalker's.

## Models and LOD

- [ ] Trees still do not load their geometry correctly (canopies solid, no alpha
  test, or wrong part). Not diagnosed.
- [ ] Power-line towers switch to a very far LOD as soon as the camera moves a
  little away. Suspected: LOD distance or `lodDist` of the archetype, or the
  bundle swap thresholds for this class.

## Not in this batch

- Water shader is missing. Not discussed yet.
