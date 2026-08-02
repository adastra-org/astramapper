import { useEffect, useState } from "react";
import { Copy } from "lucide-react";
import {
  readGxt2,
  type Gxt2Entry,
  type Gxt2Preview,
} from "../../lib/viewer";

/**
 * A `.gxt2` text table: every hash → string pair the file carries.
 *
 * These are the game's UI strings, looked up by Jenkins hash at runtime. The
 * list is rendered like the reference's viewer, one line per entry; the hash
 * is shown so a string can be matched against a script or a mod.
 */
export function Gxt2Viewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<Gxt2Preview | null>(null);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    readGxt2()
      .then((result) => {
        if (live) setPreview(result);
      })
      .catch((e: unknown) => {
        if (live) setError(String(e));
      });
    return () => {
      live = false;
    };
  }, [path]);

  if (error) {
    return (
      <div className="flex flex-1 items-center justify-center gap-2 text-sm text-ink-dim">
        {error}
      </div>
    );
  }
  if (preview === null) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-ink-dim">
        Loading text table…
      </div>
    );
  }

  const needle = filter.trim().toLowerCase();
  const shown = needle
    ? preview.entries.filter(
        (e) =>
          e.text.toLowerCase().includes(needle) ||
          e.hash.toString(16).includes(needle),
      )
    : preview.entries;

  async function copyEntry(entry: Gxt2Entry): Promise<void> {
    await navigator.clipboard.writeText(`${entry.text}\t0x${entry.hash.toString(16)}`);
    setCopied(entry.hash.toString(16));
    setTimeout(() => setCopied(null), 1200);
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center gap-3 border-b border-edge px-4 py-2">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder={`Filter ${preview.entries.length} strings…`}
          className="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2.5 py-1.5 text-sm text-ink outline-none focus:border-accent"
        />
        <span className="shrink-0 text-xs text-ink-dim">
          {shown.length} of {preview.entries.length}
        </span>
      </div>
      <ul className="min-h-0 flex-1 divide-y divide-edge overflow-y-auto">
        {shown.map((entry) => (
          <li key={entry.hash} className="group flex items-start gap-3 px-4 py-1.5">
            <button
              type="button"
              onClick={() => void copyEntry(entry)}
              title="Copy text and hash"
              className="mt-0.5 shrink-0 text-ink-dim opacity-0 transition-opacity hover:text-accent group-hover:opacity-100"
            >
              {copied === entry.hash.toString(16) ? (
                <span className="text-xs text-accent">copied</span>
              ) : (
                <Copy size={13} />
              )}
            </button>
            <code className="shrink-0 font-mono text-xs leading-5 text-ink-dim">
              0x{entry.hash.toString(16).padStart(8, "0")}
            </code>
            <span className="min-w-0 break-words text-sm leading-5 text-ink">
              {entry.text}
            </span>
          </li>
        ))}
        {shown.length === 0 && (
          <li className="px-4 py-6 text-center text-sm text-ink-dim">
            No strings match.
          </li>
        )}
      </ul>
    </div>
  );
}
