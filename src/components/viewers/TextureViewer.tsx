import { useCallback, useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Download, Image as ImageIcon, Loader2 } from "lucide-react";
import {
  readTexture,
  readTextureImage,
  saveTexture,
  type TextureEntry,
  type TextureImage,
  type TexturePreview,
} from "../../lib/viewer";
import { formatBytes, shortFormat } from "../../lib/format";

/**
 * Look at the textures in a `.ytd`.
 *
 * The dictionary is listed on open but nothing is decoded until a texture is
 * picked — a `.ytd` runs to dozens of 2048² surfaces. Decoding happens in Rust:
 * no browser reads BC-compressed blocks, and the decoded mip comes back as a
 * PNG rather than as raw RGBA because 16 MB of pixels is slower to serialize
 * than to decode.
 *
 * Two exports, because they answer different questions. `.dds` is the stored
 * bytes with a header in front — every mip, no decode, lossless, and what other
 * tools want. `.png` is the mip on screen, projected onto eight-bit RGBA.
 */
export function TextureViewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<TexturePreview | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [level, setLevel] = useState(0);
  const [image, setImage] = useState<TextureImage | null>(null);
  const [loading, setLoading] = useState(false);
  const [checkered, setCheckered] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setSelected(null);
    setImage(null);
    setStatus(null);
    readTexture()
      .then((loaded) => {
        if (cancelled) return;
        setPreview(loaded);
        setError(null);
        // Open on the first thing that can actually be drawn, so a dictionary
        // whose first entry is an unsupported format still shows something.
        const first = loaded.textures.find((t) => t.decodable);
        if (first) setSelected(first.index);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  // Picking a different texture resets the mip: level 3 of a 2048² surface is
  // level 3 of a 64² one too, and that is rarely what was meant.
  useEffect(() => {
    setLevel(0);
  }, [selected]);

  useEffect(() => {
    if (selected === null) return;
    let cancelled = false;
    setLoading(true);
    readTextureImage(selected, level)
      .then((decoded) => {
        if (cancelled) return;
        setImage(decoded);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setImage(null);
        setError(String(cause));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selected, level]);

  const download = useCallback(
    async (texture: TextureEntry, format: "dds" | "png") => {
      const destination = await save({
        defaultPath: `${texture.name}.${format}`,
        title: format === "dds" ? "Save the texture as DDS" : "Save this mip as PNG",
      });
      if (typeof destination !== "string") return;
      try {
        const written = await saveTexture(texture.index, level, format, destination);
        setStatus(`Wrote ${formatBytes(written)} to ${destination}`);
      } catch (cause) {
        setError(String(cause));
      }
    },
    [level],
  );

  if (error && !preview) {
    return <p className="p-4 text-xs text-red-400">{error}</p>;
  }
  if (!preview) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Loader2 size={16} className="animate-spin text-ink-dim" />
      </div>
    );
  }

  const current = preview.textures.find((t) => t.index === selected) ?? null;
  const decodable = preview.textures.filter((t) => t.decodable).length;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-edge px-4 text-xs text-ink-dim">
        <span className="flex items-center gap-1.5">
          <ImageIcon size={13} strokeWidth={1.75} />
          {preview.textures.length}{" "}
          {preview.textures.length === 1 ? "texture" : "textures"}
          {decodable !== preview.textures.length && `, ${decodable} decodable`}
        </span>
        {current && (
          <span>
            {current.width}x{current.height} · {current.format} · {current.levels}{" "}
            {current.levels === 1 ? "mip" : "mips"} · {current.usage}
          </span>
        )}
      </div>

      {error && (
        <p className="shrink-0 border-b border-edge px-4 py-2 text-xs text-red-400">
          {error}
        </p>
      )}

      <div className="flex min-h-0 flex-1">
        <ul className="w-64 shrink-0 overflow-y-auto border-r border-edge">
          {preview.textures.map((texture) => (
            <li key={texture.index}>
              <button
                type="button"
                onClick={() => setSelected(texture.index)}
                disabled={!texture.decodable}
                className={`flex w-full flex-col items-start gap-0.5 px-4 py-1.5 text-left text-xs disabled:opacity-40 ${
                  texture.index === selected
                    ? "bg-surface-2 text-ink"
                    : "text-ink-dim hover:bg-surface-1 enabled:hover:text-ink"
                }`}
              >
                <span className="w-full truncate" title={texture.name}>
                  {texture.name}
                </span>
                <span className="text-ink-dim">
                  {texture.width}x{texture.height} · {shortFormat(texture.format)}
                </span>
              </button>
            </li>
          ))}

          {preview.textures.length === 0 && (
            <li className="px-4 py-3 text-xs text-ink-dim">
              This dictionary holds no textures.
            </li>
          )}
        </ul>

        <div
          className="flex min-h-0 min-w-0 flex-1 items-center justify-center overflow-auto p-4"
          style={
            checkered
              ? {
                  // A checkerboard rather than a flat colour: it is the only way
                  // to tell a transparent texel from a black one, and half the
                  // dictionary is alpha-masked foliage.
                  backgroundImage:
                    "conic-gradient(#2a2a2e 90deg, #202024 90deg 180deg, #2a2a2e 180deg 270deg, #202024 270deg)",
                  backgroundSize: "16px 16px",
                }
              : undefined
          }
        >
          {loading && <Loader2 size={16} className="animate-spin text-ink-dim" />}
          {!loading && image && (
            <img
              src={`data:image/png;base64,${image.png}`}
              alt={current?.name ?? "texture"}
              width={image.width}
              height={image.height}
              // Nearest-neighbour so a 4x4 mip reads as sixteen texels rather
              // than as a smear, which is the point of looking at one.
              className="max-h-full max-w-full object-contain [image-rendering:pixelated]"
            />
          )}
          {!loading && !image && !error && (
            <p className="text-xs text-ink-dim">Pick a texture to see it.</p>
          )}
        </div>
      </div>

      <div className="flex h-9 shrink-0 items-center gap-3 border-t border-edge px-4 text-xs">
        {current && current.levels > 1 && (
          <label className="flex items-center gap-2 text-ink-dim">
            Mip
            <input
              type="range"
              min={0}
              max={current.levels - 1}
              value={level}
              onChange={(event) => setLevel(Number(event.target.value))}
              className="w-32"
            />
            <span className="w-20 font-mono text-ink">
              {level} · {image ? `${image.width}x${image.height}` : "—"}
            </span>
          </label>
        )}

        <label className="flex items-center gap-1.5 text-ink-dim">
          <input
            type="checkbox"
            checked={checkered}
            onChange={(event) => setCheckered(event.target.checked)}
          />
          Alpha
        </label>

        <span className="min-w-0 flex-1 truncate text-ink-dim">
          {status ?? (current ? `${formatBytes(current.byteLength)} of pixels` : "")}
        </span>

        {current && (
          <>
            <button
              type="button"
              onClick={() => void download(current, "dds")}
              className="flex shrink-0 items-center gap-1.5 rounded px-1.5 py-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
            >
              <Download size={13} strokeWidth={1.75} />
              DDS
            </button>
            <button
              type="button"
              onClick={() => void download(current, "png")}
              className="flex shrink-0 items-center gap-1.5 rounded px-1.5 py-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink"
            >
              <Download size={13} strokeWidth={1.75} />
              PNG
            </button>
          </>
        )}
      </div>
    </div>
  );
}
