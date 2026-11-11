/**
 * Adaptive cells: large where the world is sparse, small where it is dense.
 *
 * The same hexagons as `hex.ts`, at several sizes at once. See `buildAdaptive`.
 */
import { hexCenter, hexKey, worldToHex } from "./hex";

/** One leaf of the adaptive partition: a cell at one level of cell size. */
export type AdaptiveCell = {
  readonly level: number;
  /** Centre-to-corner radius in metres. */
  readonly size: number;
  readonly q: number;
  readonly r: number;
  /** Centre in metres. */
  readonly x: number;
  readonly y: number;
  /** Mean height of the placements in it. */
  readonly z: number;
  count: number;
  loaded: number;
};

export type AdaptiveGrid = {
  /** Every leaf. Each placement is counted in exactly one. */
  readonly cells: readonly AdaptiveCell[];
  readonly sizes: readonly number[];
  /** Per level: the leaf at a key, or `null` where the cell was split finer. */
  readonly levels: readonly Map<number, AdaptiveCell | null>[];
};

/**
 * Partition placements into cells whose size follows their density.
 *
 * `sizes` runs coarse to fine. Every placement starts in the coarsest grid; a
 * cell holding more than `threshold` of them is *split*, and its placements go
 * on to the next finer grid, where they are counted again — and split again if
 * they are still crowded. The finest level never splits.
 *
 * The levels are independent hex grids, not a nested tree: a hexagon does not
 * tile into smaller hexagons, so a fine cell near a split coarse one may
 * overlap its neighbours' edges. That costs nothing here — what matters is that
 * each placement lands in exactly one leaf — and it is the same trade H3 makes.
 *
 * Done level by level over *all* the placements still active, not parent by
 * parent, so two crowded neighbours that share a fine cell count into it once.
 */
export function buildAdaptive(
  xs: Float64Array,
  ys: Float64Array,
  zs: Float64Array,
  sizes: readonly number[],
  threshold: number,
): AdaptiveGrid {
  const levels = sizes.map(() => new Map<number, AdaptiveCell | null>());
  const cells: AdaptiveCell[] = [];
  let active: ArrayLike<number> = Int32Array.from({ length: xs.length }, (_, i) => i);

  for (let level = 0; level < sizes.length && active.length > 0; level += 1) {
    const size = sizes[level]!;
    const last = level === sizes.length - 1;
    const groups = new Map<number, number[]>();
    for (let n = 0; n < active.length; n += 1) {
      const i = active[n]!;
      const { q, r } = worldToHex(xs[i]!, ys[i]!, size);
      const key = hexKey(q, r);
      const group = groups.get(key);
      if (group) group.push(i);
      else groups.set(key, [i]);
    }
    const next: number[] = [];
    for (const [key, group] of groups) {
      if (!last && group.length > threshold) {
        levels[level]!.set(key, null);
        for (const i of group) next.push(i);
        continue;
      }
      const q = Math.floor(key / 65536) - 32768;
      const r = (key % 65536) - 32768;
      const [x, y] = hexCenter({ q, r }, size);
      let zSum = 0;
      for (const i of group) zSum += zs[i]!;
      const cell: AdaptiveCell = {
        level,
        size,
        q,
        r,
        x,
        y,
        z: zSum / group.length,
        count: group.length,
        loaded: 0,
      };
      levels[level]!.set(key, cell);
      cells.push(cell);
    }
    active = next;
  }
  return { cells, sizes, levels };
}

/** The leaf a point falls in, or `null` when no placement ever counted there. */
export function adaptiveLeafAt(grid: AdaptiveGrid, x: number, y: number): AdaptiveCell | null {
  for (let level = 0; level < grid.sizes.length; level += 1) {
    const { q, r } = worldToHex(x, y, grid.sizes[level]!);
    const found = grid.levels[level]!.get(hexKey(q, r));
    if (found === undefined) return null;
    if (found !== null) return found;
  }
  return null;
}

/** Recount which placements are resident, per leaf. */
export function tallyAdaptiveLoaded(
  grid: AdaptiveGrid,
  positions: Iterable<readonly [number, number, number]>,
): void {
  for (const cell of grid.cells) cell.loaded = 0;
  for (const p of positions) {
    const leaf = adaptiveLeafAt(grid, p[0], p[1]);
    if (leaf) leaf.loaded += 1;
  }
}

export type WorkingSet = {
  readonly cells: ReadonlySet<AdaptiveCell>;
  readonly placements: number;
  /** Distance to the farthest cell taken, in metres. */
  readonly reach: number;
};

/**
 * The cells a loader would hold for a camera, nearest first, until the next
 * one would pass `budget` placements.
 *
 * The budget is in placements, not in rings, and that is the point of variable
 * cells: in the city the budget is spent within a few hundred metres, in the
 * country it reaches kilometres, and the memory held is the same either way.
 */
export function adaptiveWorkingSet(
  grid: AdaptiveGrid,
  camX: number,
  camY: number,
  budget: number,
): WorkingSet {
  const measured = grid.cells.map((cell) => ({
    cell,
    distance: Math.hypot(cell.x - camX, cell.y - camY),
  }));
  // Nothing else holds `measured`, so sorting it in place is fine.
  // oxlint-disable-next-line unicorn/no-array-sort
  const byDistance = measured.sort((a, b) => a.distance - b.distance);
  const taken = new Set<AdaptiveCell>();
  let placements = 0;
  let reach = 0;
  for (const { cell, distance } of byDistance) {
    // Always take the cell under the camera, however crowded: a working set
    // with nothing in it is no use. Past that the budget is a hard stop.
    if (taken.size > 0 && placements + cell.count > budget) break;
    taken.add(cell);
    placements += cell.count;
    reach = distance;
  }
  return { cells: taken, placements, reach };
}
