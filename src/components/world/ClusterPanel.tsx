import { Download, Loader2, Ruler, Unplug } from "lucide-react";
import { useState } from "react";

import { worldIndexClusterMeasure } from "../../lib/world/cellApi";
import { formatBytes } from "../../lib/format";
import type { ClusterRow } from "../../lib/world/clusters";

/**
 * The zone clusters (North, City, Airport...): what each holds, how much of it
 * this window has, and the buttons to load a whole zone or let it go.
 *
 * "Load" pins the zone in the cell streamer, which fetches its leaves like any
 * other cells and keeps them until "Release". "Measure" asks the backend to
 * compile the zone's models and report their size, which is the number that
 * decides whether a zone is worth loading at once; it is slow on a cold cache.
 */
export function ClusterPanel({
  rows,
  onLoad,
  onRelease,
  onMeasured,
}: {
  rows: readonly ClusterRow[];
  onLoad: (key: string) => void;
  onRelease: (key: string) => void;
  /** A measure finished: the parent keeps the number, so it survives a re-mount. */
  onMeasured: (key: string, bytes: number) => void;
}): React.JSX.Element | null {
  const [measuring, setMeasuring] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);

  if (rows.length === 0) return null;

  const measure = (key: string): void => {
    setError(null);
    setMeasuring((now) => new Set(now).add(key));
    worldIndexClusterMeasure(key)
      .then((result) => onMeasured(key, result.bytes))
      .catch((cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() =>
        setMeasuring((now) => {
          const next = new Set(now);
          next.delete(key);
          return next;
        }),
      );
  };

  return (
    <div className="border-b border-edge/60 px-4 pb-2 pt-1 text-[11px] text-ink-dim">
      <div className="text-xs text-ink">Zones</div>
      <ul className="space-y-1.5 pt-1">
        {rows.map((row) => (
          <li key={row.key}>
            <div className="flex items-center justify-between gap-2">
              <span className="text-ink">{row.name}</span>
              <span className="flex items-center gap-1">
                <button
                  type="button"
                  title="Measure the compiled size of this zone's models"
                  aria-label={`Measure ${row.name}`}
                  disabled={measuring.has(row.key)}
                  onClick={() => measure(row.key)}
                  className="rounded bg-surface-2 p-1 text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
                >
                  {measuring.has(row.key) ? (
                    <Loader2 size={12} className="animate-spin" />
                  ) : (
                    <Ruler size={12} />
                  )}
                </button>
                {row.pinned ? (
                  <button
                    type="button"
                    title="Release this zone"
                    aria-label={`Release ${row.name}`}
                    onClick={() => onRelease(row.key)}
                    className="flex items-center gap-1 rounded bg-surface-2 px-1.5 py-1 text-ink transition-colors hover:bg-surface-2/70"
                  >
                    <Unplug size={12} />
                    Release
                  </button>
                ) : (
                  <button
                    type="button"
                    title="Load every cell of this zone"
                    aria-label={`Load ${row.name}`}
                    onClick={() => onLoad(row.key)}
                    className="flex items-center gap-1 rounded bg-surface-2 px-1.5 py-1 text-ink transition-colors hover:bg-surface-2/70"
                  >
                    <Download size={12} />
                    Load
                  </button>
                )}
              </span>
            </div>
            <div>
              {row.leaves.toLocaleString()} cells · {row.placements.toLocaleString()} placements ·{" "}
              {row.models.toLocaleString()} models
              {row.modelBytes !== null ? ` · ${formatBytes(row.modelBytes)}` : ""}
            </div>
            {(row.pinned || row.heldLeaves > 0) && (
              <div>
                Held {row.heldLeaves.toLocaleString()} of {row.leaves.toLocaleString()} cells (
                {row.percent}%)
                {row.pinned && !row.complete ? " · loading" : ""}
              </div>
            )}
          </li>
        ))}
      </ul>
      {error && <div className="pt-1 text-red-400">{error}</div>}
    </div>
  );
}
