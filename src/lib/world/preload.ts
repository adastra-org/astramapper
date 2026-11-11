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

  // Candidates are (entry, row, distance) in parallel typed arrays, not one
  // object each: a preload considers every placement in the world, and the
  // columns exist to avoid exactly that many allocations.
  let total = 0;
  for (const entry of entries) total += entry.placements.length;
  const entryAt = new Uint32Array(total);
  const rowAt = new Uint32Array(total);
  const distances = new Float64Array(total);
  let count = 0;

  for (let e = 0; e < entries.length; e += 1) {
    const entry = entries[e]!;
    const cols = entry.placements;
    for (let row = 0; row < cols.length; row += 1) {
      if (!isLoadableName(cols.name(row))) continue;
      if (excluded.has(keyFor(entry.key, cols.entityIndex(row)))) continue;
      entryAt[count] = e;
      rowAt[count] = row;
      distances[count] =
        cameraGta === null
          ? 0
          : Math.hypot(
              cols.x(row) - cameraGta[0],
              cols.y(row) - cameraGta[1],
              cols.z(row) - cameraGta[2],
            );
      count += 1;
    }
  }

  // Typed-array sort is stable, so two entities at the same distance keep
  // placement order and a preload run twice loads the same things in the same
  // order.
  const order = new Uint32Array(count);
  for (let i = 0; i < count; i += 1) order[i] = i;
  order.sort((a, b) => distances[a]! - distances[b]!);

  const targets: PreloadTarget[] = [];
  for (let i = 0; i < Math.min(count, cap); i += 1) {
    const at = order[i]!;
    const entry = entries[entryAt[at]!]!;
    targets.push({
      mapKey: entry.key,
      entity: entry.placements.at(rowAt[at]!),
      distance: distances[at]!,
    });
  }
  return targets;
}

/** Whether an entity has a model that could be loaded at all. */
export function isLoadable(placement: Placed): boolean {
  return isLoadableName(placement.name);
}

function isLoadableName(name: string): boolean {
  return !name.startsWith("hash_");
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
    const cols = entry.placements;
    for (let row = 0; row < cols.length; row += 1) {
      if (isLoadableName(cols.name(row))) total += 1;
    }
  }
  return total;
}
