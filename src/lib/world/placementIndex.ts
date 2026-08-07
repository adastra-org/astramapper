/**
 * Placements looked up by identity, in constant time.
 *
 * # Why this exists
 *
 * A placed entity is addressed everywhere by `(mapKey, entityIndex)` — the
 * streamer names its candidates that way, a raycast hit carries it, the
 * selection panel and the outline resolve it every render, and the box markers
 * are restored through it. Every one of those sites was doing
 * `entries.find(...)` and then `placements.find(...)`: a linear scan of the
 * placed maps followed by a linear scan of one map's placements.
 *
 * That is affordable once. It is not affordable at the rates these sites run
 * at. The selection outline recomputes on every `edits` change, which during a
 * gizmo drag is **once per frame**, so dragging one entity walked six figures
 * of placements sixty times a second. The streaming pass resolved twenty-four
 * candidates the same way every 250ms. `preload.ts` already documents the
 * problem in `PreloadTarget` and dodges it by carrying the `Placed` itself; the
 * sites that only have an index could not.
 *
 * So the lookup is built once per `entries` change and shared. It is a nested
 * map rather than one keyed by `keyFor` on purpose: a flat map would build a
 * concatenated string per placement, which at the base game's ~693,000
 * placements is 693,000 strings to hold for the lifetime of the index. The
 * nested form keys on the map path once and on a number after that.
 *
 * The index is a *view*, not a copy — it holds the same `Placed` objects the
 * entries do, so building one costs a map insert per placement and no
 * duplication. It is only valid for the entries it was built from; rebuild it
 * whenever they change.
 */
import type { Placed } from "../world";

/** The least an entry must be for the index to key it. Both `CanvasEntry` and
 *  `RegionEntry` satisfy it, so one index serves the renderer and the
 *  scheduler. */
export type IndexableEntry = {
  readonly key: string;
  readonly placements: readonly Placed[];
};

export type PlacementIndex<E extends IndexableEntry = IndexableEntry> = {
  /** One placed map by its key, or `undefined` when it is not placed. */
  entry: (mapKey: string) => E | undefined;
  /**
   * One placement by its map and its **file index** — the identity a raycast
   * hit and the streamer both carry, not a position in the array. A filtered or
   * reordered list would make an array position select the wrong entity.
   */
  placement: (mapKey: string, entityIndex: number) => Placed | undefined;
  /** How many maps are indexed. */
  readonly maps: number;
  /** How many placements are indexed, across every map. */
  readonly placements: number;
};

/**
 * Index a set of placed maps.
 *
 * A duplicate entity index within one map keeps the first placement, matching
 * what the `find` calls this replaces would have returned.
 */
export function createPlacementIndex<E extends IndexableEntry>(
  entries: readonly E[],
): PlacementIndex<E> {
  const byMap = new Map<string, E>();
  const byIndex = new Map<string, Map<number, Placed>>();
  let placements = 0;

  for (const entry of entries) {
    // A repeated map key would otherwise give the two halves different
    // winners: `find` keeps the first entry, so the placements must too.
    if (byMap.has(entry.key)) continue;
    byMap.set(entry.key, entry);
    const inMap = new Map<number, Placed>();
    for (const placement of entry.placements) {
      if (inMap.has(placement.index)) continue;
      inMap.set(placement.index, placement);
      placements += 1;
    }
    byIndex.set(entry.key, inMap);
  }

  return {
    entry: (mapKey) => byMap.get(mapKey),
    placement: (mapKey, entityIndex) => byIndex.get(mapKey)?.get(entityIndex),
    maps: byMap.size,
    placements,
  };
}

/** The index of nothing, for a canvas with no placed maps. */
export const EMPTY_PLACEMENT_INDEX: PlacementIndex = createPlacementIndex([]);
