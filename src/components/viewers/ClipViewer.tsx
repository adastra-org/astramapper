import { useEffect, useMemo, useState } from "react";
import { Film, Loader2 } from "lucide-react";
import { formatDuration, readClips, type ClipPreview } from "../../lib/viewer";
import { formatHash } from "../../lib/format";

/**
 * List what a `.ycd` holds: its clips, and the animations they play.
 *
 * There is no playback here and there will not be one until two other things
 * are ported — the animation channel codecs in `Clip.cs`, and the skeleton in
 * `Drawable.cs` that an animation is posed against. What a clip *is* needs
 * neither, so that is what this shows.
 *
 * The two tabs are the file's two maps. A clip references animations by hash,
 * so picking a clip highlights the animations it plays rather than jumping
 * away from the list.
 */
export function ClipViewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<ClipPreview | null>(null);
  const [tab, setTab] = useState<"clips" | "animations">("clips");
  const [selected, setSelected] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setSelected(null);
    setTab("clips");
    readClips()
      .then((loaded) => {
        if (cancelled) return;
        setPreview(loaded);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  const highlighted = useMemo(() => {
    if (preview === null || selected === null) return new Set<number>();
    return new Set(preview.clips[selected]?.animations ?? []);
  }, [preview, selected]);

  if (error) {
    return <p className="p-4 text-xs text-red-400">{error}</p>;
  }
  if (!preview) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Loader2 size={16} className="animate-spin text-ink-dim" />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-edge px-4 text-xs text-ink-dim">
        <span className="flex items-center gap-1.5">
          <Film size={13} strokeWidth={1.75} />
          {preview.clips.length} {preview.clips.length === 1 ? "clip" : "clips"},{" "}
          {preview.animations.length}{" "}
          {preview.animations.length === 1 ? "animation" : "animations"}
        </span>

        {(["clips", "animations"] as const).map((id) => (
          <button
            key={id}
            type="button"
            onClick={() => setTab(id)}
            className={`rounded px-1.5 py-0.5 transition-colors hover:bg-surface-2 hover:text-ink ${
              tab === id ? "bg-surface-2 text-ink" : ""
            }`}
          >
            {id === "clips" ? "Clips" : "Animations"}
          </button>
        ))}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {tab === "clips"
          ? preview.clips.map((clip) => (
              <button
                key={clip.index}
                type="button"
                onClick={() =>
                  setSelected((current) => (current === clip.index ? null : clip.index))
                }
                className={`flex w-full items-center gap-3 px-4 py-1.5 text-left text-xs ${
                  clip.index === selected ? "bg-surface-2" : "hover:bg-surface-1"
                }`}
                title={clip.path ?? undefined}
              >
                <span className="min-w-0 flex-1 truncate text-ink">{clip.name}</span>
                <span className="w-24 shrink-0 truncate text-ink-dim">{clip.kind}</span>
                <span className="w-14 shrink-0 text-right text-ink-dim">
                  {formatDuration(clip.duration)}
                </span>
                <span className="w-16 shrink-0 text-right text-ink-dim">
                  {clip.rate === 1 ? "" : `${clip.rate.toFixed(2)}x`}
                </span>
                <span className="w-20 shrink-0 text-right text-ink-dim">
                  {clip.animations.length}{" "}
                  {clip.animations.length === 1 ? "anim" : "anims"}
                </span>
                <span className="w-20 shrink-0 text-right text-ink-dim">
                  {clip.tagCount > 0 ? `${clip.tagCount} tags` : ""}
                </span>
                <span className="w-24 shrink-0 text-right font-mono text-ink-dim">
                  {formatHash(clip.hash)}
                </span>
              </button>
            ))
          : preview.animations.map((animation) => (
              <div
                key={animation.index}
                className={`flex items-center gap-3 px-4 py-1.5 text-xs ${
                  highlighted.has(animation.hash) ? "bg-surface-2" : ""
                }`}
              >
                <span className="min-w-0 flex-1 truncate font-mono text-ink">
                  {animation.name}
                </span>
                <span className="w-20 shrink-0 text-right text-ink-dim">
                  {animation.frames.toLocaleString()} f
                </span>
                <span className="w-14 shrink-0 text-right text-ink-dim">
                  {formatDuration(animation.duration)}
                </span>
                <span className="w-16 shrink-0 text-right text-ink-dim">
                  {animation.frameRate.toFixed(1)} fps
                </span>
                <span className="w-20 shrink-0 text-right text-ink-dim">
                  {animation.boneTracks} tracks
                </span>
                <span className="w-24 shrink-0 text-right text-ink-dim">
                  {animation.sequences}{" "}
                  {animation.sequences === 1 ? "sequence" : "sequences"}
                </span>
                <span className="w-16 shrink-0 text-right text-ink-dim">
                  {animation.usageCount} used
                </span>
              </div>
            ))}

        {tab === "clips" && preview.clips.length === 0 && (
          <p className="px-4 py-3 text-xs text-ink-dim">
            This dictionary holds no clips.
          </p>
        )}
      </div>

      <div className="shrink-0 border-t border-edge px-4 py-2 text-xs text-ink-dim">
        {selected !== null && preview.clips[selected] ? (
          <span className="font-mono">
            {preview.clips[selected].path ?? formatHash(preview.clips[selected].hash)}
            {" · "}
            {preview.clips[selected].startTime.toFixed(3)}s to{" "}
            {preview.clips[selected].endTime.toFixed(3)}s
            {preview.clips[selected].propertyCount > 0 &&
              ` · ${preview.clips[selected].propertyCount} properties`}
          </span>
        ) : (
          <span>
            Pick a clip to see its source path and which animations it plays. The
            animation data itself is not decoded: the channel codecs are not ported.
          </span>
        )}
      </div>
    </div>
  );
}
