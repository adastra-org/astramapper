/**
 * The diagnostics overlay: F3.
 *
 * Both halves of `perf` in one place — the WebView's spans and gauges, and the
 * backend's, which arrive over IPC in the same shape. Nothing here computes
 * anything; the arithmetic is in `src/lib/perf.ts` and tested there.
 *
 * # Why an overlay and not a sidebar section
 *
 * The numbers that matter are the ones that move while you fly the camera —
 * frame time, draw calls, the streaming pass. A panel you have to navigate to
 * is a panel that is not open when the stutter happens. F3 is where a game
 * modder already looks for this.
 *
 * # It polls, and only while it is open
 *
 * Reading the report is a `Mutex` and a sort on each side; doing that per frame
 * would make the instrument a source of what it measures. Twice a second is
 * faster than anyone reads and slow enough to disappear, and the interval is
 * torn down with the panel.
 */
import { useCallback, useEffect, useState } from "react";
import { Check, Copy, Gauge, RotateCcw, X } from "lucide-react";

import { formatBytes } from "../lib/format";
import * as perf from "../lib/perf";

/** How often the panel re-reads both reports. */
const POLL_MS = 500;

export function DebugPanel({ onClose }: { onClose: () => void }): React.JSX.Element {
  const [frontend, setFrontend] = useState<perf.SpanReport[]>([]);
  const [backend, setBackend] = useState<perf.SpanReport[]>([]);
  const [gauges, setGauges] = useState<perf.GaugeReport[]>([]);
  const [fps, setFps] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const poll = (): void => {
      if (cancelled) return;
      setFrontend(perf.report());
      setGauges(perf.gauges());
      setFps(perf.fps());
      perf
        .backendReport()
        .then((rows) => {
          if (cancelled) return;
          setBackend(rows);
          setError(null);
        })
        // No backend is a normal state — the panel opens before a folder does.
        .catch((cause: unknown) => !cancelled && setError(String(cause)));
    };
    poll();
    const interval = window.setInterval(poll, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  const clear = useCallback(() => {
    perf.reset();
    void perf.backendReset().catch(() => undefined);
    setFrontend([]);
    setBackend([]);
    setGauges([]);
  }, []);

  // Copy the whole report as plain text, so it can be pasted without a screenshot.
  // A failed clipboard write (no permission, no focus) leaves the button unchanged.
  const copy = useCallback(() => {
    const text = reportText(fps, gauges, frontend, backend, error);
    navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => undefined);
  }, [fps, gauges, frontend, backend, error]);

  return (
    <aside className="pointer-events-auto absolute right-3 top-3 z-20 flex max-h-[calc(100%-1.5rem)] w-[34rem] max-w-[calc(100%-1.5rem)] flex-col overflow-hidden rounded-lg border border-edge bg-surface-1/95 shadow-xl backdrop-blur">
      <header className="flex items-center gap-2 border-b border-edge px-3 py-2">
        <Gauge className="size-4 text-ink-dim" />
        <h2 className="text-sm font-medium">Diagnostics</h2>
        {fps !== null && (
          <span className="text-xs tabular-nums text-ink-dim">{fps.toFixed(0)} fps</span>
        )}
        <div className="ml-auto flex items-center gap-1">
          <button
            type="button"
            onClick={copy}
            title="Copy the whole report as text"
            className="rounded p-1 text-ink-dim hover:bg-surface-2 hover:text-ink"
          >
            {copied ? <Check className="size-4" /> : <Copy className="size-4" />}
          </button>
          <button
            type="button"
            onClick={clear}
            title="Reset both counters"
            className="rounded p-1 text-ink-dim hover:bg-surface-2 hover:text-ink"
          >
            <RotateCcw className="size-4" />
          </button>
          <button
            type="button"
            onClick={onClose}
            title="Close (F3)"
            className="rounded p-1 text-ink-dim hover:bg-surface-2 hover:text-ink"
          >
            <X className="size-4" />
          </button>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-2">
        {gauges.length > 0 && (
          <section className="mb-3">
            <h3 className="mb-1 text-xs font-medium uppercase tracking-wide text-ink-dim">
              Now
            </h3>
            <div className="grid grid-cols-3 gap-x-4 gap-y-1 text-xs">
              {gauges.map((g) => (
                <div key={g.name} className="flex justify-between gap-2">
                  <span className="truncate text-ink-dim">{g.name}</span>
                  <span className="tabular-nums">{g.value.toLocaleString()}</span>
                </div>
              ))}
            </div>
          </section>
        )}

        <SpanTable title="WebView" rows={frontend} />
        <SpanTable title="Backend" rows={backend} />
        {error && <p className="mt-2 text-xs text-ink-dim">Backend: {error}</p>}
        {frontend.length === 0 && backend.length === 0 && !error && (
          <p className="py-6 text-center text-xs text-ink-dim">
            Nothing recorded yet. Open a folder and fly around.
          </p>
        )}
      </div>
    </aside>
  );
}

function SpanTable({
  title,
  rows,
}: {
  title: string;
  rows: readonly perf.SpanReport[];
}): React.JSX.Element | null {
  if (rows.length === 0) return null;
  return (
    <section className="mb-3">
      <h3 className="mb-1 text-xs font-medium uppercase tracking-wide text-ink-dim">
        {title}
      </h3>
      <table className="w-full text-xs tabular-nums">
        <thead className="text-ink-dim">
          <tr>
            <th className="text-left font-normal">span</th>
            <th className="text-right font-normal">n</th>
            <th className="text-right font-normal">total</th>
            <th className="text-right font-normal">p50</th>
            <th className="text-right font-normal">p95</th>
            <th className="text-right font-normal">max</th>
            <th className="text-right font-normal">bytes</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.name} className="border-t border-edge/50">
              <td className="max-w-[12rem] truncate py-0.5 pr-2 font-mono">{row.name}</td>
              <td className="text-right">{row.calls.toLocaleString()}</td>
              <td className="text-right">{ms(row.totalMs)}</td>
              <td className="text-right">{ms(row.p50Ms)}</td>
              <td className="text-right">{ms(row.p95Ms)}</td>
              <td className="text-right">{ms(row.maxMs)}</td>
              <td className="text-right text-ink-dim">
                {row.bytes > 0 ? formatBytes(row.bytes) : "—"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/** The report as tab-separated text: gauges, then each span table. */
function reportText(
  fps: number | null,
  gauges: readonly perf.GaugeReport[],
  frontend: readonly perf.SpanReport[],
  backend: readonly perf.SpanReport[],
  error: string | null,
): string {
  const lines: string[] = [];
  lines.push(`fps\t${fps === null ? "-" : fps.toFixed(0)}`);
  lines.push("NOW");
  for (const g of gauges) lines.push(`${g.name}\t${g.value.toLocaleString()}`);
  const table = (title: string, rows: readonly perf.SpanReport[]): void => {
    if (rows.length === 0) return;
    lines.push(title);
    lines.push("span\tn\ttotal\tp50\tp95\tmax\tbytes");
    for (const r of rows) {
      lines.push(
        [
          r.name,
          r.calls,
          ms(r.totalMs),
          ms(r.p50Ms),
          ms(r.p95Ms),
          ms(r.maxMs),
          r.bytes > 0 ? formatBytes(r.bytes) : "-",
        ].join("\t"),
      );
    }
  };
  table("WEBVIEW", frontend);
  table("BACKEND", backend);
  if (error) lines.push(`Backend error\t${error}`);
  return lines.join("\n");
}

/** Milliseconds at a precision that stays readable across five orders of
 *  magnitude — a 0.04ms cache hit and a 3,400ms cold bake in one column. */
function ms(value: number): string {
  if (value === 0) return "0";
  if (value >= 1000) return `${(value / 1000).toFixed(2)}s`;
  if (value >= 10) return value.toFixed(0);
  if (value >= 1) return value.toFixed(1);
  return value.toFixed(2);
}
