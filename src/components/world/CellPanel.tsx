import type { CellGrid } from "../../lib/world/cellData";
import type { CellStreamerStats } from "../../lib/world/cellStreamer";

/**
 * What the backend index holds against what this window holds: the base game's
 * placements live in Rust, and only the cells around the camera are here.
 */
export function CellPanel({
  grid,
  stats,
}: {
  grid: CellGrid;
  stats: CellStreamerStats;
}): React.JSX.Element {
  const share = grid.placements > 0 ? (stats.placements / grid.placements) * 100 : 0;
  return (
    <div className="border-b border-edge/60 px-4 pb-2 pt-1 text-[11px] text-ink-dim">
      <div className="text-xs text-ink">
        Base game · {grid.maps.toLocaleString()} maps, {grid.placements.toLocaleString()} placements
      </div>
      <div>
        Held here: {stats.placements.toLocaleString()} placements in {stats.cells.toLocaleString()}{" "}
        of {grid.leaves.length.toLocaleString()} cells ({share.toFixed(1)}%)
        {stats.pending > 0 ? ` · ${stats.pending} loading` : ""}
      </div>
      <div>
        Reach {Math.round(stats.reach).toLocaleString()} m · {stats.prefetchCells} cells ahead ·
        ground {grid.terrain.toLocaleString()}
      </div>
    </div>
  );
}
