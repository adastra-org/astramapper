/**
 * Zone clusters on the frontend: the leaves of each named zone, and how much of
 * a zone this window holds.
 *
 * The backend decides which leaf belongs to which zone (`src-tauri/src/clusters.rs`,
 * from `src-tauri/data/zones.json`) and sends one cluster index per leaf. This is
 * only the bookkeeping on top of that; loading a zone is the streamer's `pin`.
 */
import type { CellGrid } from "./cellData";
import type { AdaptiveCell } from "./hexAdaptive";

/** The leaves of every cluster, indexed like `grid.clusters`. */
export function leavesByCluster(grid: CellGrid): AdaptiveCell[][] {
  const out: AdaptiveCell[][] = grid.clusters.map(() => []);
  grid.leaves.forEach((leaf, i) => {
    out[grid.leafCluster[i]!]?.push(leaf);
  });
  return out;
}

/** How much of a cluster is held here. */
export type ClusterProgress = {
  readonly key: string;
  /** Leaves held, of `leaves`. */
  readonly heldLeaves: number;
  readonly leaves: number;
  /** Placements held in those leaves, of `placements`. */
  readonly heldPlacements: number;
  readonly placements: number;
  /** The whole cluster was asked for and is not released. */
  readonly pinned: boolean;
};

/** What the panel shows for one cluster. */
export type ClusterRow = {
  readonly key: string;
  readonly name: string;
  readonly leaves: number;
  readonly placements: number;
  readonly models: number;
  readonly heldLeaves: number;
  readonly heldPlacements: number;
  /** Share of the cluster's leaves held, 0 to 100. */
  readonly percent: number;
  readonly pinned: boolean;
  /** Loading finished: every leaf is held (or the cluster has none). */
  readonly complete: boolean;
  /** Compiled model bytes, once measured. */
  readonly modelBytes: number | null;
};

/**
 * Join the partition's clusters with what is held and what has been measured,
 * for the panel. Clusters with no leaves are left out: an empty countryside is
 * not worth a row.
 */
export function clusterRows(
  grid: CellGrid,
  progress: readonly ClusterProgress[],
  measuredBytes: ReadonlyMap<string, number>,
): ClusterRow[] {
  const rows: ClusterRow[] = [];
  grid.clusters.forEach((info, at) => {
    if (info.leaves === 0) return;
    const held = progress[at];
    const heldLeaves = held?.heldLeaves ?? 0;
    rows.push({
      key: info.key,
      name: info.name,
      leaves: info.leaves,
      placements: info.placements,
      models: info.models,
      heldLeaves,
      heldPlacements: held?.heldPlacements ?? 0,
      percent: info.leaves > 0 ? Math.round((heldLeaves / info.leaves) * 100) : 0,
      pinned: held?.pinned ?? false,
      complete: heldLeaves >= info.leaves,
      modelBytes: measuredBytes.get(info.key) ?? null,
    });
  });
  return rows;
}
