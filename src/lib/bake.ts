/**
 * Typed wrappers over the bake and cache commands in `src-tauri/src/bake.rs`
 * and `src-tauri/src/cache.rs`, plus the pure helpers the panel draws with.
 *
 * The `invoke` wrappers need a Tauri host, so the tests here cover the pure
 * helpers: how a progress report turns into a percentage and a line of text.
 */
import { Channel, invoke } from "@tauri-apps/api/core";

/** Where the bake is. Mirrors `bake::Phase`. */
export type BakePhase = "maps" | "assets" | "done" | "cancelled" | "full";

/** One progress report, sent at most four times a second. */
export type BakeProgress = {
  readonly phase: BakePhase;
  readonly done: number;
  readonly total: number;
  readonly bytes: number;
  readonly budgetBytes: number;
  readonly errors: number;
  readonly elapsedMs: number;
};

/** What the cache holds. `null` when no folder is open or it is unavailable. */
export type CacheStats = {
  readonly bytes: number;
  readonly budgetBytes: number;
  readonly records: number;
  readonly packs: number;
  readonly hits: number;
  readonly misses: number;
  readonly writeErrors: number;
};

export function cacheStats(): Promise<CacheStats | null> {
  return invoke<CacheStats | null>("cache_stats");
}

export function cacheClear(): Promise<void> {
  return invoke<void>("cache_clear");
}

export function bakeRunning(): Promise<boolean> {
  return invoke<boolean>("bake_running");
}

export function bakeCancel(): Promise<void> {
  return invoke<void>("bake_cancel");
}

/**
 * Start a bake over the given content groups.
 *
 * `onProgress` receives every report; the returned promise resolves with the
 * final one. A caller that only wants the live view can ignore the promise.
 */
export function bakeStart(
  groups: readonly string[],
  onProgress: (progress: BakeProgress) => void,
): Promise<BakeProgress> {
  const channel = new Channel<BakeProgress>();
  // Tauri's Channel is not an EventTarget; `onmessage` is the whole API.
  // eslint-disable-next-line unicorn/prefer-add-event-listener
  channel.onmessage = onProgress;
  return invoke<BakeProgress>("bake_start", { groups, on: channel });
}

/** Whether the pass has stopped, whatever the reason. */
export function bakeFinished(progress: BakeProgress): boolean {
  return progress.phase === "done" || progress.phase === "cancelled" || progress.phase === "full";
}

/**
 * How far along, 0 to 1.
 *
 * The map phase has no total to divide by, so it reports zero rather than
 * pretending: a bar that jumps backwards when the real total arrives is worse
 * than one that waits.
 */
export function bakeProgressRatio(progress: BakeProgress): number {
  if (progress.total <= 0) return 0;
  return Math.min(1, progress.done / progress.total);
}

/** One line of plain English for the panel. */
export function bakeLabel(progress: BakeProgress): string {
  const errors = progress.errors > 0 ? `, ${progress.errors} failed` : "";
  switch (progress.phase) {
    case "maps":
      return "reading maps to find what is placed";
    case "assets":
      return `${progress.done} of ${progress.total} models${errors}`;
    case "done":
      return `baked ${progress.done} models in ${(progress.elapsedMs / 1000).toFixed(1)}s${errors}`;
    case "cancelled":
      return `stopped after ${progress.done} models${errors}`;
    case "full":
      return `budget reached at ${progress.done} models${errors}`;
  }
}
