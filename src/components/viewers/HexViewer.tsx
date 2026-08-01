import { useCallback, useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, Loader2 } from "lucide-react";
import { decodeBase64, readHex } from "../../lib/viewer";
import { formatBytes, formatOffset, hexLines } from "../../lib/format";

/**
 * How many bytes one page holds. The backend caps a single read at 64 KB; this
 * is a screenful and a half, which is what makes paging feel like scrolling.
 */
const PAGE = 4096;

/**
 * The universal fallback: any file at all, as bytes.
 *
 * Pages rather than loading the whole payload, per `docs/architecture.md` — a
 * `.ytd` runs to 20 MB and rendering that as DOM rows would lock the WebView.
 */
export function HexViewer({ path }: { path: string }): React.JSX.Element {
  const [offset, setOffset] = useState(0);
  const [bytes, setBytes] = useState<Uint8Array | null>(null);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // A different file starts at the top again.
  useEffect(() => setOffset(0), [path]);

  useEffect(() => {
    let cancelled = false;
    setBusy(true);
    readHex(offset, PAGE)
      .then((page) => {
        if (cancelled) return;
        setBytes(decodeBase64(page.bytes));
        setTotal(page.total);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      })
      .finally(() => {
        if (!cancelled) setBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [path, offset]);

  // Clamped to a page boundary, not to `total`: landing on the last byte would
  // shift every row's offset off the 16-byte grid the whole view is read on.
  const step = useCallback(
    (delta: number) =>
      setOffset((at) => {
        const last = Math.floor(Math.max(0, total - 1) / PAGE) * PAGE;
        return Math.min(Math.max(0, at + delta), last);
      }),
    [total],
  );

  if (error) {
    return <p className="p-4 text-xs text-red-400">{error}</p>;
  }
  if (!bytes) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Loader2 size={16} className="animate-spin text-ink-dim" />
      </div>
    );
  }

  const lines = hexLines(bytes, offset);
  const end = Math.min(offset + bytes.length, total);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-edge px-4 text-xs text-ink-dim">
        <button
          type="button"
          onClick={() => step(-PAGE)}
          disabled={offset === 0 || busy}
          className="rounded p-0.5 transition-colors hover:bg-surface-2 hover:text-ink disabled:opacity-30 disabled:hover:bg-transparent"
          aria-label="Previous page"
        >
          <ChevronLeft size={14} />
        </button>
        <button
          type="button"
          onClick={() => step(PAGE)}
          disabled={end >= total || busy}
          className="rounded p-0.5 transition-colors hover:bg-surface-2 hover:text-ink disabled:opacity-30 disabled:hover:bg-transparent"
          aria-label="Next page"
        >
          <ChevronRight size={14} />
        </button>
        <span className="font-mono">
          {formatOffset(offset)} – {formatOffset(Math.max(offset, end - 1))}
        </span>
        <span>of {formatBytes(total)}</span>
        {busy && <Loader2 size={12} className="animate-spin" />}
      </div>

      <div className="min-h-0 flex-1 overflow-auto p-4 font-mono text-xs leading-5">
        {lines.map((line) => (
          <div key={line.offset} className="flex gap-4 whitespace-pre">
            <span className="text-ink-dim">{formatOffset(line.offset)}</span>
            <span className="text-ink">{line.hex}</span>
            <span className="text-accent">{line.ascii}</span>
          </div>
        ))}
        {lines.length === 0 && <p className="text-ink-dim">This file is empty.</p>}
      </div>
    </div>
  );
}
