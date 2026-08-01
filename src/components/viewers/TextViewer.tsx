import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, Loader2, Save, Undo2 } from "lucide-react";
import { baseName, readText, saveText, type TextFile } from "../../lib/viewer";
import { formatBytes } from "../../lib/format";

/**
 * Read and edit a file as text.
 *
 * Saving writes to disk, not back into the archive. RPF *writing* is not ported
 * — `RpfFile.Write`, `CompressBytes` and NG encryption are all still open, see
 * `docs/formats.md` — so an in-place save would have to be a lie or a silent
 * no-op. The footer says which it is instead.
 */
export function TextViewer({ path }: { path: string }): React.JSX.Element {
  const [file, setFile] = useState<TextFile | null>(null);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setFile(null);
    setStatus(null);
    readText()
      .then((text) => {
        if (cancelled) return;
        setFile(text);
        setDraft(text.text);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (error) {
    return <p className="p-4 text-xs text-red-400">{error}</p>;
  }
  if (!file) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Loader2 size={16} className="animate-spin text-ink-dim" />
      </div>
    );
  }

  const edited = draft !== file.text;

  const onSave = async () => {
    const destination = await save({
      defaultPath: baseName(path),
      title: "Save a copy to disk",
    });
    if (typeof destination !== "string") return;
    try {
      const written = await saveText(destination, draft, file.encoding);
      setStatus(`Wrote ${formatBytes(written)} to ${destination}`);
      setError(null);
    } catch (cause) {
      setError(String(cause));
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-edge px-4 text-xs text-ink-dim">
        <button
          type="button"
          onClick={() => void onSave()}
          className="flex items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink"
        >
          <Save size={13} strokeWidth={1.75} />
          Save a copy
        </button>
        <button
          type="button"
          onClick={() => setDraft(file.text)}
          disabled={!edited}
          className="flex items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink disabled:opacity-30 disabled:hover:bg-transparent"
        >
          <Undo2 size={13} strokeWidth={1.75} />
          Revert
        </button>

        <span>{file.encoding === "utf8" ? "UTF-8" : file.encoding}</span>
        {edited && <span className="text-accent">edited</span>}

        {file.truncated && (
          <span className="flex items-center gap-1 text-amber-400">
            <AlertTriangle size={12} />
            showing the first {formatBytes(draft.length)} of{" "}
            {formatBytes(file.totalBytes)}
          </span>
        )}
        {file.lossy && (
          <span className="flex items-center gap-1 text-amber-400">
            <AlertTriangle size={12} />
            some bytes are not valid text; saving would not reproduce them
          </span>
        )}
      </div>

      <textarea
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        readOnly={file.truncated}
        spellCheck={false}
        className="min-h-0 flex-1 resize-none bg-transparent p-4 font-mono text-xs leading-5 text-ink outline-none selection:bg-accent/30"
      />

      <p className="shrink-0 border-t border-edge px-4 py-1.5 text-xs text-ink-dim">
        {status ??
          "Saving writes a copy to disk. Writing back into an archive needs the RPF writer, which is not ported yet."}
      </p>
    </div>
  );
}
