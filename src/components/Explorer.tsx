import { useCallback, useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ChevronRight,
  FileBox,
  FileText,
  Folder,
  HardDrive,
  Loader2,
  Package,
} from "lucide-react";
import {
  breadcrumb,
  gameIsOpen,
  listDir,
  openGameFolder,
  parentOf,
  type EntryKind,
  type GameSummary,
  type Listing,
} from "../lib/explorer";
import { formatBytes } from "../lib/format";
import { getLastGameFolder, setLastGameFolder } from "../lib/settings";
import { FileViewer } from "./FileViewer";

const ICONS: Record<EntryKind, typeof Folder> = {
  directory: Folder,
  archive: Package,
  binary: FileText,
  resource: FileBox,
};

/**
 * Explorer Mode: pick a game folder, walk the archives as one tree, and preview
 * whatever is selected.
 *
 * Listings come from the Rust side already sorted and typed, and the preview
 * pane receives decoded text, mesh buffers or explicit byte ranges. Nothing
 * here parses archive bytes.
 */
export function Explorer(): React.JSX.Element {
  const [summary, setSummary] = useState<GameSummary | null>(null);
  const [path, setPath] = useState("");
  const [rows, setRows] = useState<Listing[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const navigate = useCallback(async (next: string) => {
    setBusy(true);
    setError(null);
    try {
      setRows(await listDir(next));
      setPath(next);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  // Opening a row either descends into it or previews it. A `.rpf` does both in
  // principle; descending is what the tree is for, so that wins.
  const open = useCallback(
    (row: Listing) => {
      if (row.expandable) {
        setSelected(null);
        void navigate(row.path);
        return;
      }
      setSelected(row.path);
    },
    [navigate],
  );

  const openFolder = useCallback(async (folder: string) => {
    setBusy(true);
    setError(null);
    setSelected(null);
    try {
      setSummary(await openGameFolder(folder));
      setRows(await listDir(""));
      setPath("");
      await setLastGameFolder(folder);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  // A folder opened earlier in this session survives a remount of this panel,
  // since the scan lives in Rust state rather than here. Across app restarts
  // there is no Rust state to recover, so fall back to the last folder saved
  // to disk and rescan it.
  useEffect(() => {
    gameIsOpen()
      .then(async (isOpen) => {
        if (isOpen) return navigate("");
        const last = await getLastGameFolder();
        if (last) await openFolder(last);
      })
      .catch((cause: unknown) => setError(String(cause)));
  }, [navigate, openFolder]);

  const pick = useCallback(async () => {
    const folder = await openDialog({
      directory: true,
      title: "Select the GTA V folder",
    });
    if (typeof folder !== "string") return;
    await openFolder(folder);
  }, [openFolder]);

  const crumbs = breadcrumb(path);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge px-4">
        <button
          type="button"
          onClick={pick}
          disabled={busy}
          className="flex items-center gap-2 rounded bg-surface-2 px-3 py-1.5 text-xs text-ink transition-colors hover:bg-surface-2/70 disabled:opacity-50"
        >
          <HardDrive size={14} strokeWidth={1.75} />
          {summary ? "Change folder" : "Open GTA V folder"}
        </button>

        {busy && <Loader2 size={14} className="animate-spin text-ink-dim" />}

        {summary && !busy && (
          <span className="truncate text-xs text-ink-dim">
            {summary.topLevelArchives} archives ({summary.totalArchives} with nested),{" "}
            {summary.totalEntries.toLocaleString()} entries in {summary.elapsedMs} ms
          </span>
        )}
      </div>

      {summary && (
        <Crumbs crumbs={crumbs} onNavigate={navigate} busy={busy} />
      )}

      {error && (
        <p className="border-b border-edge px-4 py-2 text-xs text-red-400">{error}</p>
      )}

      {summary && summary.failed.length > 0 && (
        <details className="border-b border-edge px-4 py-2 text-xs text-ink-dim">
          <summary className="cursor-pointer">
            {summary.failed.length} archives could not be read
          </summary>
          <ul className="mt-1 space-y-0.5 font-mono">
            {summary.failed.map((f) => (
              <li key={f}>{f}</li>
            ))}
          </ul>
        </details>
      )}

      {summary ? (
        <div className="flex min-h-0 flex-1">
          <div className="flex min-h-0 w-96 shrink-0 flex-col">
            <Rows
              rows={rows}
              path={path}
              selected={selected}
              onNavigate={navigate}
              onOpen={open}
            />
          </div>
          <FileViewer path={selected} onClose={() => setSelected(null)} />
        </div>
      ) : (
        <div className="flex flex-1 items-center justify-center">
          <p className="text-sm text-ink-dim">
            Open a GTA V folder to browse its archives.
          </p>
        </div>
      )}
    </div>
  );
}

type CrumbProps = {
  crumbs: readonly { label: string; path: string }[];
  onNavigate: (path: string) => void;
  busy: boolean;
};

function Crumbs({ crumbs, onNavigate, busy }: CrumbProps): React.JSX.Element {
  return (
    <nav className="flex h-9 shrink-0 items-center gap-1 overflow-x-auto border-b border-edge px-4 text-xs">
      <button
        type="button"
        onClick={() => onNavigate("")}
        disabled={busy}
        className="shrink-0 rounded px-1.5 py-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
      >
        Archives
      </button>
      {crumbs.map((crumb, i) => (
        <span key={crumb.path} className="flex shrink-0 items-center gap-1">
          <ChevronRight size={12} className="text-ink-dim" />
          <button
            type="button"
            onClick={() => onNavigate(crumb.path)}
            disabled={busy}
            className={`rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink ${
              i === crumbs.length - 1 ? "text-ink" : "text-ink-dim"
            }`}
          >
            {crumb.label}
          </button>
        </span>
      ))}
    </nav>
  );
}

type RowsProps = {
  rows: readonly Listing[];
  path: string;
  selected: string | null;
  onNavigate: (path: string) => void;
  onOpen: (row: Listing) => void;
};

function Rows({
  rows,
  path,
  selected,
  onNavigate,
  onOpen,
}: RowsProps): React.JSX.Element {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      {path !== "" && (
        <button
          type="button"
          onClick={() => onNavigate(parentOf(path))}
          className="flex w-full items-center gap-2 px-4 py-1.5 text-left text-xs text-ink-dim transition-colors hover:bg-surface-1"
        >
          <Folder size={14} strokeWidth={1.75} />
          ..
        </button>
      )}

      {rows.map((row) => {
        const Icon = ICONS[row.kind];
        const isSelected = row.path === selected;
        return (
          <button
            key={row.path}
            type="button"
            onClick={() => onOpen(row)}
            className={`flex w-full items-center gap-2 px-4 py-1.5 text-left text-xs transition-colors ${
              isSelected ? "bg-surface-2" : "hover:bg-surface-1"
            }`}
          >
            <Icon
              size={14}
              strokeWidth={1.75}
              className={row.expandable ? "text-accent" : "text-ink-dim"}
            />
            <span className="min-w-0 flex-1 truncate text-ink">{row.name}</span>
            <span className="shrink-0 font-mono text-ink-dim">
              {row.kind === "directory" ? "" : formatBytes(row.size)}
            </span>
          </button>
        );
      })}

      {rows.length === 0 && (
        <p className="px-4 py-3 text-xs text-ink-dim">This folder is empty.</p>
      )}
    </div>
  );
}
