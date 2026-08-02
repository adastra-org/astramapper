import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { AlertCircle, Download, FileQuestion, Loader2, X } from "lucide-react";
import {
  baseName,
  extractTo,
  openEntry,
  type EntryInfo,
  type ViewerId,
} from "../lib/viewer";
import { formatBytes } from "../lib/format";
import { AudioViewer } from "./viewers/AudioViewer";
import { ClipViewer } from "./viewers/ClipViewer";
import { Gxt2Viewer } from "./viewers/Gxt2Viewer";
import { MetaViewer } from "./viewers/MetaViewer";
import { HexViewer } from "./viewers/HexViewer";
import { TextureViewer } from "./viewers/TextureViewer";
import { TextViewer } from "./viewers/TextViewer";

// three.js is most of the frontend bundle and is only needed once someone opens
// a drawable or a collision file, so it loads on demand rather than at startup.
const ModelViewer = lazy(async () => ({
  default: (await import("./viewers/ModelViewer")).ModelViewer,
}));
const BoundsViewer = lazy(async () => ({
  default: (await import("./viewers/BoundsViewer")).BoundsViewer,
}));

const VIEWER_LABELS: Record<ViewerId, string> = {
  model: "3D",
  bounds: "Collision",
  texture: "Texture",
  audio: "Audio",
  clip: "Clips",
  meta: "Metadata",
  text: "Text",
  hex: "Hex",
  gxt2: "Strings",
};

type Props = {
  /** Archive path of the file to show, or `null` for the empty state. */
  path: string | null;
  onClose: () => void;
};

/**
 * The right-hand pane: open one file and show it in whichever viewer suits it.
 *
 * The backend picks the default viewer, because that decision needs the payload
 * — a `.dat` is sometimes an INI file, and a `.ydr` is only a model if its
 * geometry actually parses. Every viewer it says will work is offered as a tab,
 * so the choice is never final.
 */
export function FileViewer({ path, onClose }: Props): React.JSX.Element {
  const [info, setInfo] = useState<EntryInfo | null>(null);
  const [viewer, setViewer] = useState<ViewerId>("hex");
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    if (path === null) {
      setInfo(null);
      return;
    }
    let cancelled = false;
    setInfo(null);
    setError(null);
    setStatus(null);
    openEntry(path)
      .then((entry) => {
        if (cancelled) return;
        setInfo(entry);
        setViewer(entry.viewer);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  const onExtract = useCallback(async () => {
    if (!info) return;
    const destination = await save({
      defaultPath: baseName(info.path),
      title: "Extract to disk",
    });
    if (typeof destination !== "string") return;
    try {
      const written = await extractTo(destination);
      setStatus(`Extracted ${formatBytes(written)} to ${destination}`);
    } catch (cause) {
      setError(String(cause));
    }
  }, [info]);

  if (path === null) {
    return (
      <div className="flex flex-1 items-center justify-center border-l border-edge">
        <p className="max-w-64 text-center text-sm text-ink-dim">
          Select a file to preview it.
        </p>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col border-l border-edge">
      <header className="flex h-11 shrink-0 items-center gap-3 border-b border-edge px-4">
        <span className="min-w-0 flex-1 truncate text-xs text-ink" title={path}>
          {info?.name ?? baseName(path)}
        </span>

        {info && (
          <>
            <span className="shrink-0 text-xs text-ink-dim">
              {info.fileKindLabel} · {formatBytes(info.byteLength)}
              {info.byteLength !== info.sizeOnDisk &&
                ` (${formatBytes(info.sizeOnDisk)} packed)`}
            </span>
            <button
              type="button"
              onClick={() => void onExtract()}
              className="flex shrink-0 items-center gap-1.5 rounded px-1.5 py-0.5 text-xs text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
            >
              <Download size={13} strokeWidth={1.75} />
              Extract
            </button>
          </>
        )}

        <button
          type="button"
          onClick={onClose}
          className="shrink-0 rounded p-1 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
          aria-label="Close the preview"
        >
          <X size={14} />
        </button>
      </header>

      {info && info.viewers.length > 1 && (
        <nav className="flex h-8 shrink-0 items-center gap-1 border-b border-edge px-4">
          {info.viewers.map((id) => (
            <button
              key={id}
              type="button"
              onClick={() => setViewer(id)}
              className={`rounded px-2 py-0.5 text-xs transition-colors ${
                id === viewer
                  ? "bg-surface-2 text-ink"
                  : "text-ink-dim hover:bg-surface-2/60 hover:text-ink"
              }`}
            >
              {VIEWER_LABELS[id]}
            </button>
          ))}
        </nav>
      )}

      {info?.note && (
        <p className="flex shrink-0 items-start gap-1.5 border-b border-edge px-4 py-2 text-xs text-amber-400">
          <AlertCircle size={13} className="mt-0.5 shrink-0" />
          {info.note}
        </p>
      )}

      {error && (
        <p className="shrink-0 border-b border-edge px-4 py-2 text-xs text-red-400">
          {error}
        </p>
      )}

      {info ? (
        <Body path={info.path} viewer={viewer} />
      ) : (
        !error && (
          <div className="flex flex-1 items-center justify-center">
            <Loader2 size={16} className="animate-spin text-ink-dim" />
          </div>
        )
      )}

      {status && (
        <p className="shrink-0 border-t border-edge px-4 py-1.5 text-xs text-ink-dim">
          {status}
        </p>
      )}
    </div>
  );
}

function Body({ path, viewer }: { path: string; viewer: ViewerId }): React.JSX.Element {
  switch (viewer) {
    case "model":
      return (
        <Suspense
          fallback={
            <div className="flex flex-1 items-center justify-center">
              <Loader2 size={16} className="animate-spin text-ink-dim" />
            </div>
          }
        >
          <ModelViewer path={path} />
        </Suspense>
      );
    case "bounds":
      return (
        <Suspense
          fallback={
            <div className="flex flex-1 items-center justify-center">
              <Loader2 size={16} className="animate-spin text-ink-dim" />
            </div>
          }
        >
          <BoundsViewer path={path} />
        </Suspense>
      );
    case "texture":
      return <TextureViewer path={path} />;
    case "audio":
      return <AudioViewer path={path} />;
    case "clip":
      return <ClipViewer path={path} />;
    case "gxt2":
      return <Gxt2Viewer path={path} />;
    case "meta":
      return <MetaViewer path={path} />;
    case "text":
      return <TextViewer path={path} />;
    case "hex":
      return <HexViewer path={path} />;
    default:
      return (
        <div className="flex flex-1 items-center justify-center gap-2 text-sm text-ink-dim">
          <FileQuestion size={16} />
          No viewer for this file.
        </div>
      );
  }
}
