/**
 * How much ground stays resident, and how far out it is asked for.
 *
 * The ground is a whole-map layer (#5): wanted from anywhere, never dropped for
 * being far. That is what keeps an area you left drawn when you come back to it,
 * and it is also unbounded: the base game's ground is 4,103 models and 1.7 GB of
 * geometry before textures. So the layer has a byte budget of its own, spent
 * nearest the camera first.
 *
 * Two numbers come out of one pass over the loaded ground:
 *
 * - **what to drop**: when the distinct models loaded ground uses pass the
 *   budget, the farthest pieces go until it is back under `LOW_WATER` of it;
 * - **the reach**: the distance of the nearest piece dropped, which the streamer
 *   then refuses to ask for beyond. Without it the layer would load the piece it
 *   just dropped on the next pass, and again, forever.
 *
 * The reach only grows when there is room again (`grownReach`), and slowly, so
 * the edge of the layer settles instead of flickering. Pure, so the policy is
 * tested without a renderer.
 */

/** A trim brings the layer down to this share of its budget, so it does not
 *  trim again on the next model that arrives. */
export const LOW_WATER = 0.9;

/** Below this share of the budget the reach may grow. */
export const ROOM_WATER = 0.75;

/** One loaded ground entity: where it is, and the model it draws. */
export type GroundPiece = {
  readonly key: string;
  readonly distance: number;
  readonly modelPath: string;
  /** The model's size in bytes; counted once however many pieces share it. */
  readonly bytes: number;
};

export type TerrainTrim = {
  /** Distinct model bytes the loaded ground holds before the trim. */
  readonly bytes: number;
  /** Keys to unload, farthest first. Empty when the layer is within budget. */
  readonly drop: readonly string[];
  /** The distance of the nearest dropped piece, or null when nothing dropped. */
  readonly reach: number | null;
};

/**
 * Decide what to drop from the loaded ground. `pieces` need not be sorted.
 */
export function trimTerrain(pieces: readonly GroundPiece[], budget: number): TerrainTrim {
  const seen = new Set<string>();
  let bytes = 0;
  for (const piece of pieces) {
    if (seen.has(piece.modelPath)) continue;
    seen.add(piece.modelPath);
    bytes += piece.bytes;
  }
  if (bytes <= budget) return { bytes, drop: [], reach: null };

  // Keep the nearest pieces while the models they use fit under the low-water
  // mark. A piece whose model a nearer piece already holds costs nothing.
  const target = budget * LOW_WATER;
  // oxlint-disable-next-line unicorn/no-array-sort -- a copy, sorted once.
  const nearest = [...pieces].sort((a, b) => a.distance - b.distance);
  const held = new Set<string>();
  let kept = 0;
  const drop: string[] = [];
  let reach: number | null = null;
  for (const piece of nearest) {
    const free = held.has(piece.modelPath);
    if (!free && kept + piece.bytes > target) {
      drop.push(piece.key);
      reach ??= piece.distance;
      continue;
    }
    if (!free) {
      held.add(piece.modelPath);
      kept += piece.bytes;
    }
    // A shared model that fits is kept; one that does not stays dropped above.
  }
  // A piece sharing a model with a dropped farther one stays: its model is held.
  // Report farthest first, which is the order they would leave in.
  drop.reverse();
  return { bytes, drop, reach };
}

/** The reach after a pass that dropped nothing: it grows only with room to spare. */
export function grownReach(reach: number, bytes: number, budget: number): number {
  if (!Number.isFinite(reach)) return reach;
  if (bytes >= budget * ROOM_WATER) return reach;
  return reach * 1.15 + 50;
}

/**
 * The ground's byte budget from the model cache's: half of it, within a floor
 * that keeps the near ground resident on a small machine and a ceiling that
 * bounds it on a large one. The ground is in use, so the cache cannot evict it:
 * it is a share of the cache's budget, not an addition to it, or the cache
 * would sit far over its own limit (`streaming-bounded` measures that).
 */
export function terrainBudget(modelBudget: number): number {
  const MB = 1024 * 1024;
  return Math.min(1024 * MB, Math.max(256 * MB, modelBudget * 0.5));
}
