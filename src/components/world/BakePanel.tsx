/**
 * The cache panel: bake the checked content groups, watch it fill, clear it.
 *
 * All the arithmetic lives in `src/lib/bake.ts` and is tested there. This file
 * is the wiring and the markup, per the phase 5 rule that anything worth
 * testing is lifted out of the component.
 */
import { Loader2, Trash2, Zap } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import {
  bakeCancel,
  bakeFinished,
  bakeLabel,
  bakeProgressRatio,
  bakeStart,
  cacheClear,
  cacheStats,
  type BakeProgress,
  type CacheStats,
} from "../../lib/bake";
import { formatBytes } from "../../lib/format";

export function BakePanel({ groups }: { groups: readonly string[] }): React.JSX.Element {
  const [stats, setStats] = useState<CacheStats | null>(null);
  const [progress, setProgress] = useState<BakeProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    void cacheStats()
      .then(setStats)
      .catch(() => setStats(null));
  }, []);

  useEffect(refresh, [refresh]);

  const running = progress !== null && !bakeFinished(progress);

  const start = useCallback(async () => {
    setError(null);
    setProgress(null);
    try {
      const final = await bakeStart(groups, setProgress);
      setProgress(final);
    } catch (e) {
      setError(String(e));
      setProgress(null);
    }
    refresh();
  }, [groups, refresh]);

  const clear = useCallback(async () => {
    try {
      await cacheClear();
    } catch (e) {
      setError(String(e));
    }
    setProgress(null);
    refresh();
  }, [refresh]);

  // The bake's own reports carry the size, so the panel does not poll while one
  // is running.
  const bytes = progress?.bytes ?? stats?.bytes ?? 0;
  const budget = progress?.budgetBytes ?? stats?.budgetBytes ?? 0;
  const filled = budget > 0 ? Math.min(1, bytes / budget) : 0;

  return (
    <div className="px-4 pb-1 text-xs">
      <p className="pb-2 text-[11px] text-ink-dim">
        Compiling a model is slow once and instant afterwards. Baking does it
        ahead of time, most-placed first, for the groups ticked above.
      </p>

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={running ? () => void bakeCancel() : () => void start()}
          disabled={groups.length === 0 && !running}
          className="flex items-center gap-1.5 rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
        >
          {running ? <Loader2 size={12} className="animate-spin" /> : <Zap size={12} />}
          {running ? "Stop" : "Bake"}
        </button>
        <button
          type="button"
          onClick={() => void clear()}
          disabled={running || stats === null}
          className="flex items-center gap-1.5 rounded bg-surface-2 px-2 py-1 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-40"
        >
          <Trash2 size={12} />
          Clear
        </button>
      </div>

      {progress && (
        <div className="pt-2">
          <div className="h-1 overflow-hidden rounded bg-surface-2">
            <div
              className="h-full bg-accent transition-[width] duration-200"
              style={{ width: `${bakeProgressRatio(progress) * 100}%` }}
            />
          </div>
          <p className="pt-1 text-[11px] text-ink-dim">{bakeLabel(progress)}</p>
        </div>
      )}

      {stats && (
        <div className="pt-2 text-[11px] text-ink-dim">
          <div className="h-1 overflow-hidden rounded bg-surface-2">
            <div className="h-full bg-ink-dim" style={{ width: `${filled * 100}%` }} />
          </div>
          <p className="pt-1">
            {formatBytes(bytes)} of {formatBytes(budget)}, {stats.records.toLocaleString()}{" "}
            artifacts
          </p>
        </div>
      )}

      {error && <p className="pt-2 text-[11px] text-red-400">{error}</p>}
    </div>
  );
}
