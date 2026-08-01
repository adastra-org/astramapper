import { useCallback, useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Download, Loader2, Music, Play } from "lucide-react";
import {
  decodeBase64,
  formatDuration,
  readAudio,
  readAudioTrack,
  saveAudioTrack,
  type AudioPreview,
  type AudioTrack,
} from "../../lib/viewer";
import { formatHash } from "../../lib/format";

/**
 * Play the streams in an `.awc`.
 *
 * The container is listed on open but nothing is decoded until a stream is
 * picked: `resident.awc` holds hundreds of streams and hours of audio. Decoding
 * happens in Rust — no browser plays Rockstar's ADPCM framing, and shipping the
 * raw blocks over would be handing the WebView archive bytes to parse.
 *
 * Streams have hashes, not names. Resolving those needs the name tables from
 * `MetaNames.cs` and the `.rel` metadata that pairs `_left`/`_right` channels;
 * neither is ported, so a stereo song shows as two mono streams.
 */
export function AudioViewer({ path }: { path: string }): React.JSX.Element {
  const [preview, setPreview] = useState<AudioPreview | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [source, setSource] = useState<string | null>(null);
  const [loading, setLoading] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setSelected(null);
    setStatus(null);
    readAudio()
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

  // A blob URL is a document-lifetime resource, so the previous one is revoked
  // whenever it is replaced or the panel goes away.
  useEffect(() => {
    if (source === null) return;
    return () => URL.revokeObjectURL(source);
  }, [source]);

  const play = useCallback(async (index: number) => {
    setLoading(index);
    setError(null);
    try {
      const clip = await readAudioTrack(index);
      const blob = new Blob([decodeBase64(clip.wav)], { type: "audio/wav" });
      setSource(URL.createObjectURL(blob));
      setSelected(index);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(null);
    }
  }, []);

  // Autoplay once the new source is attached, which is a frame after `src`
  // changes. Failing to start is not an error worth surfacing: the controls are
  // right there.
  useEffect(() => {
    if (source === null) return;
    audio.current?.load();
    void audio.current?.play().catch(() => undefined);
  }, [source]);

  // Takes the track, not its index: `track.index` is the stream's position in
  // the container, while `preview.tracks` has the carrier stream filtered out,
  // so the two do not line up.
  const download = useCallback(async (track: AudioTrack) => {
    const destination = await save({
      defaultPath: `${formatHash(track.id)}.wav`,
      title: "Save this stream as a WAV",
    });
    if (typeof destination !== "string") return;
    try {
      await saveAudioTrack(track.index, destination);
      setStatus(`Wrote ${destination}`);
    } catch (cause) {
      setError(String(cause));
    }
  }, []);

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

  const playable = preview.tracks.filter((t) => t.playable).length;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-edge px-4 text-xs text-ink-dim">
        <span className="flex items-center gap-1.5">
          <Music size={13} strokeWidth={1.75} />
          {preview.tracks.length} {preview.tracks.length === 1 ? "stream" : "streams"},{" "}
          {playable} playable
        </span>
        {preview.multiChannel && <span>multi-channel</span>}
        {preview.wholeFileEncrypted && <span>encrypted</span>}
        <span>v{preview.version}</span>
      </div>

      {error && (
        <p className="shrink-0 border-b border-edge px-4 py-2 text-xs text-red-400">
          {error}
        </p>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto">
        {preview.tracks.map((track) => (
          <div
            key={track.index}
            className={`flex items-center gap-3 px-4 py-1.5 text-xs ${
              track.index === selected ? "bg-surface-2" : "hover:bg-surface-1"
            }`}
          >
            <button
              type="button"
              onClick={() => void play(track.index)}
              disabled={!track.playable || loading !== null}
              className="rounded p-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink disabled:opacity-30 disabled:hover:bg-transparent"
              aria-label={`Play stream ${track.index}`}
            >
              {loading === track.index ? (
                <Loader2 size={13} className="animate-spin" />
              ) : (
                <Play size={13} strokeWidth={1.75} />
              )}
            </button>

            <span className="w-24 shrink-0 font-mono text-ink">
              {formatHash(track.id)}
            </span>
            <span className="w-14 shrink-0 text-ink-dim">
              {formatDuration(track.seconds)}
            </span>
            <span className="w-16 shrink-0 text-ink-dim">{track.codec}</span>
            <span className="w-20 shrink-0 text-ink-dim">
              {track.sampleRate.toLocaleString()} Hz
            </span>
            <span className="min-w-0 flex-1 truncate text-ink-dim">
              {track.midi ? "MIDI" : track.chunks.join(", ")}
            </span>

            <button
              type="button"
              onClick={() => void download(track)}
              disabled={!track.playable}
              className="shrink-0 rounded p-0.5 text-ink-dim transition-colors hover:bg-surface-2 hover:text-ink disabled:opacity-30 disabled:hover:bg-transparent"
              aria-label={`Save stream ${track.index} as WAV`}
            >
              <Download size={13} strokeWidth={1.75} />
            </button>
          </div>
        ))}

        {preview.tracks.length === 0 && (
          <p className="px-4 py-3 text-xs text-ink-dim">
            This container holds no streams.
          </p>
        )}
      </div>

      <div className="shrink-0 border-t border-edge px-4 py-2">
        {source ? (
          <audio ref={audio} src={source} controls className="h-8 w-full" />
        ) : (
          <p className="text-xs text-ink-dim">
            {status ?? "Pick a stream to play it. Streams are named by hash: the name tables are not ported yet."}
          </p>
        )}
      </div>
    </div>
  );
}
