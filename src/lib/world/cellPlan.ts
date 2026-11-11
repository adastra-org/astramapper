/**
 * Which cells to hold, which to fetch ahead, and which to let go.
 *
 * Pure: a partition, a camera and a heading in; a plan out. The streamer
 * (`cellStreamer.ts`) does the fetching.
 *
 * # The three sets
 *
 * - **Working set**: the nearest leaves until the placement budget is spent, the
 *   same rule the Hex view draws (`adaptiveWorkingSet`). In the city that is a
 *   few hundred metres and in the country kilometres, at the same memory.
 * - **Prefetch**: the next ring along the direction of travel. Leaves just past
 *   the working set, ranked by how far *ahead* of the camera they are, up to a
 *   share of the budget. Flying over the edge of the working set finds the next
 *   cells already there.
 * - **Keep**: what is not evicted. The working set grown by a factor, plus the
 *   prefetch. The slack is hysteresis: a camera parked on the boundary would
 *   otherwise fetch and drop the same cell every pass.
 */
import { cellKey } from "./cellData";
import type { AdaptiveCell } from "./hexAdaptive";

export type PlanParams = {
  /** Placements the working set may hold. */
  readonly budget: number;
  /** The keep set holds this many times the budget. At least 1. */
  readonly keepFactor: number;
  /** Prefetch up to this share of the budget, in placements. */
  readonly prefetchShare: number;
  /**
   * Prefetch only leaves within this multiple of the working set's reach, so a
   * heading across open country does not reach for cells a long way past it.
   */
  readonly prefetchReach: number;
};

export const DEFAULT_PLAN: PlanParams = {
  budget: 50_000,
  keepFactor: 1.3,
  prefetchShare: 0.25,
  prefetchReach: 1.6,
};

export type Plan = {
  /** Nearest first. */
  readonly working: readonly AdaptiveCell[];
  /** Best first: most ahead of the camera. */
  readonly prefetch: readonly AdaptiveCell[];
  /** Keys of every cell that should stay held. */
  readonly keep: ReadonlySet<string>;
  /** Distance to the farthest cell of the working set, in metres. */
  readonly reach: number;
  /** Placements in the working set. */
  readonly placements: number;
};

/** A 2D heading in the ground plane. Need not be unit length. */
export type Heading = readonly [number, number];

/**
 * Plan the cells for a camera.
 *
 * `heading` is the direction of travel in the ground plane, or `null` for none
 * (a camera at rest): without one the prefetch is simply the next nearest.
 */
export function planCells(
  leaves: readonly AdaptiveCell[],
  camX: number,
  camY: number,
  heading: Heading | null,
  params: PlanParams = DEFAULT_PLAN,
): Plan {
  const measured = leaves.map((cell) => ({
    cell,
    distance: Math.hypot(cell.x - camX, cell.y - camY),
  }));
  // Nothing else holds `measured`, so sorting it in place is fine. Stable, so
  // equidistant leaves keep the partition's order and the plan does not flicker.
  // oxlint-disable-next-line unicorn/no-array-sort
  measured.sort((a, b) => a.distance - b.distance);

  const working: AdaptiveCell[] = [];
  const keep = new Set<string>();
  let placements = 0;
  let reach = 0;
  let keptPlacements = 0;
  let workingEnd = 0;
  const keepBudget = params.budget * Math.max(1, params.keepFactor);

  for (let i = 0; i < measured.length; i += 1) {
    const { cell, distance } = measured[i]!;
    // The cell under the camera is always taken, however crowded: a working set
    // with nothing in it is no use. Past that the budget is a hard stop.
    if (working.length > 0 && placements + cell.count > params.budget) break;
    working.push(cell);
    keep.add(cellKey(cell));
    placements += cell.count;
    keptPlacements = placements;
    reach = distance;
    workingEnd = i + 1;
  }

  // The slack ring: leaves past the working set that still fit the keep budget.
  for (let i = workingEnd; i < measured.length; i += 1) {
    const { cell } = measured[i]!;
    if (keptPlacements + cell.count > keepBudget) break;
    keep.add(cellKey(cell));
    keptPlacements += cell.count;
  }

  // The prefetch ring, by how far ahead each candidate sits.
  const prefetch: AdaptiveCell[] = [];
  const prefetchBudget = params.budget * params.prefetchShare;
  const maxDistance = Math.max(reach, 1) * params.prefetchReach;
  const length = heading ? Math.hypot(heading[0], heading[1]) : 0;
  if (prefetchBudget > 0) {
    const ahead: { cell: AdaptiveCell; score: number }[] = [];
    for (let i = workingEnd; i < measured.length; i += 1) {
      const { cell, distance } = measured[i]!;
      if (distance > maxDistance) break;
      if (length > 0) {
        const along = ((cell.x - camX) * heading![0] + (cell.y - camY) * heading![1]) / length;
        // Only what lies ahead is worth fetching early. Nearer and more ahead
        // scores lower.
        if (along <= 0) continue;
        ahead.push({ cell, score: distance - along });
      } else {
        ahead.push({ cell, score: distance });
      }
    }
    // oxlint-disable-next-line unicorn/no-array-sort
    ahead.sort((a, b) => a.score - b.score);
    let taken = 0;
    for (const { cell } of ahead) {
      if (taken + cell.count > prefetchBudget) break;
      prefetch.push(cell);
      keep.add(cellKey(cell));
      taken += cell.count;
    }
  }

  return { working, prefetch, keep, reach, placements };
}

/**
 * The direction of travel from where the camera was and is, smoothed.
 *
 * A camera that has barely moved has no direction of travel worth trusting, so
 * the look direction stands in (`facing`), then the previous heading. `smoothing`
 * is the weight of the previous heading, 0 to 1: a camera that turns sharply
 * should swing the prefetch quickly, one that jitters should not.
 */
export function travelHeading(
  previous: readonly [number, number] | null,
  current: readonly [number, number],
  facing: Heading | null,
  last: Heading | null,
  options: { readonly minMove?: number; readonly smoothing?: number } = {},
): Heading | null {
  const minMove = options.minMove ?? 5;
  const smoothing = options.smoothing ?? 0.5;
  if (previous) {
    const dx = current[0] - previous[0];
    const dy = current[1] - previous[1];
    const moved = Math.hypot(dx, dy);
    if (moved >= minMove) {
      const unit: Heading = [dx / moved, dy / moved];
      if (!last) return unit;
      const lastLength = Math.hypot(last[0], last[1]) || 1;
      const mixed: Heading = [
        smoothing * (last[0] / lastLength) + (1 - smoothing) * unit[0],
        smoothing * (last[1] / lastLength) + (1 - smoothing) * unit[1],
      ];
      const length = Math.hypot(mixed[0], mixed[1]);
      // Two opposite headings cancel: take the newer one.
      return length > 1e-6 ? [mixed[0] / length, mixed[1] / length] : unit;
    }
  }
  if (facing && Math.hypot(facing[0], facing[1]) > 1e-6) return facing;
  return last;
}
