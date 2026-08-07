/**
 * Preloading: fill a placed map completely, instead of streaming it by
 * distance.
 *
 * The streamer answers "what should be resident given where the camera is".
 * This answers "what would it take for all of it to be resident", which is a
 * different question with a different failure mode. The streamer is bounded by
 * geometry — a radius around the camera. A preload is bounded only by the cap
 * the caller sets, so **the cap is the whole safety mechanism** and the order
 * decides what survives it.
 *
 * Ordered nearest-first from the camera, so a cap that bites keeps what is in
 * front of you and drops what is across the city. With no camera position yet,
 * placement order stands in, which is stable but arbitrary.
 */
import type { Placed, RegionEntry } from "../world";
import { keyFor } from "../world";

export type PreloadTarget = {
  readonly mapKey: string;
  /** The placement itself, not its index. A caller handed an index would have
   *  to search the map for it, which is a linear scan per target over hundreds
   *  of thousands of placements — the exact quadratic this pass exists to
   *  avoid. */
  readonly entity: Placed;
  readonly distance: number;
};

/**
 * How many entities a preload would still have to load, and in what order.
 *
 * Excludes what is already loaded or in flight, and entities whose archetype
 * name never resolved — a `hash_XXXXXXXX` name has no file behind it, so
 * counting it would make the progress bar promise work that cannot happen.
 */
export function preloadTargets(
  entries: readonly RegionEntry[],
  excluded: ReadonlySet<string>,
  cameraGta: readonly [number, number, number] | null,
  cap: number,
): readonly PreloadTarget[] {
  if (cap <= 0) return [];
  const targets: PreloadTarget[] = [];

  for (const entry of entries) {
    for (const placement of entry.placements) {
      if (!isLoadable(placement)) continue;
      if (excluded.has(keyFor(entry.key, placement.index))) continue;
      targets.push({
        mapKey: entry.key,
        entity: placement,
        distance: cameraGta === null ? 0 : distanceTo(placement, cameraGta),
      });
    }
  }

  // A stable sort, so two entities at the same distance keep placement order
  // and a preload run twice loads the same things in the same order.
  targets.sort((a, b) => a.distance - b.distance);
  return targets.slice(0, cap);
}

/** Whether an entity has a model that could be loaded at all. */
export function isLoadable(placement: Placed): boolean {
  return !placement.name.startsWith("hash_");
}

function distanceTo(
  placement: Placed,
  cameraGta: readonly [number, number, number],
): number {
  return Math.hypot(
    placement.position[0] - cameraGta[0],
    placement.position[1] - cameraGta[1],
    placement.position[2] - cameraGta[2],
  );
}

/**
 * Everything a preload could ever load across the placed maps, ignoring the cap
 * and what is already resident.
 *
 * The number the UI shows next to the cap, so choosing one is an informed
 * decision rather than a guess.
 */
export function preloadableCount(entries: readonly RegionEntry[]): number {
  let total = 0;
  for (const entry of entries) {
    for (const placement of entry.placements) {
      if (isLoadable(placement)) total += 1;
    }
  }
  return total;
}
