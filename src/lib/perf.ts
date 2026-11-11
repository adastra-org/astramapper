/**
 * Timings and counters for the work the WebView does.
 *
 * The mirror of `src-tauri/src/perf.rs`, in the same shape and for the same
 * reason, so one panel can render both halves without knowing which side a row
 * came from.
 *
 * # Why a second recorder rather than reporting everything to Rust
 *
 * The interesting frontend spans are the ones that never cross the boundary: a
 * streaming pass, a geometry sync, a frame. Reporting each of them over IPC
 * would cost more than the span. So this side keeps its own table, and the
 * panel reads two reports.
 *
 * # Frames are a span like any other
 *
 * `frame` is just `record` with a fixed name. It runs sixty times a second, so
 * it is the one span where the recorder's own cost is worth a thought — which
 * is why recording is an object lookup and four arithmetic operations, and why
 * the retained window is a preallocated ring rather than a growing array.
 */

import { invoke } from "@tauri-apps/api/core";

/** How many recent samples a span keeps for percentiles. Mirrors `WINDOW`. */
const WINDOW = 256;

/**
 * Resolution and range of the run histogram: every call since the last reset
 * lands in a bin of `BIN_MS`, so a percentile can cover a whole flight and not
 * only the last `WINDOW` samples (8 seconds of frames). Past `BINS * BIN_MS`
 * a sample counts in the last bin.
 */
const BIN_MS = 0.5;
const BINS = 1000;

type Entry = {
  calls: number;
  totalMs: number;
  minMs: number;
  maxMs: number;
  bytes: number;
  /** A ring of the last `WINDOW` samples. */
  recent: Float64Array;
  filled: number;
  at: number;
  /** Every call since the last reset, in `BIN_MS` bins. */
  hist: Uint32Array;
};

const table = new Map<string, Entry>();

function entryFor(name: string): Entry {
  let entry = table.get(name);
  if (!entry) {
    entry = {
      calls: 0,
      totalMs: 0,
      minMs: Number.POSITIVE_INFINITY,
      maxMs: 0,
      bytes: 0,
      recent: new Float64Array(WINDOW),
      filled: 0,
      at: 0,
      hist: new Uint32Array(BINS),
    };
    table.set(name, entry);
  }
  return entry;
}

/** Record one completed span, in milliseconds. */
export function record(name: string, ms: number, bytes = 0): void {
  const entry = entryFor(name);
  entry.calls += 1;
  entry.totalMs += ms;
  if (ms < entry.minMs) entry.minMs = ms;
  if (ms > entry.maxMs) entry.maxMs = ms;
  entry.bytes += bytes;
  entry.hist[Math.min(BINS - 1, Math.max(0, Math.floor(ms / BIN_MS)))]! += 1;
  entry.recent[entry.at] = ms;
  entry.at = (entry.at + 1) % WINDOW;
  if (entry.filled < WINDOW) entry.filled += 1;
}

/** Time `body`, record it, and hand back what it returned. */
export function measure<T>(name: string, body: () => T): T {
  const started = performance.now();
  try {
    return body();
  } finally {
    record(name, performance.now() - started);
  }
}

/**
 * Time an awaited call.
 *
 * The timing is recorded whether it resolves or rejects, because a command that
 * fails slowly is exactly the one worth seeing. `bytes` is taken from the
 * resolved value only, since a rejection carries no payload.
 */
export async function measureAsync<T>(
  name: string,
  body: () => Promise<T>,
  bytes?: (value: T) => number,
): Promise<T> {
  const started = performance.now();
  try {
    const value = await body();
    record(name, performance.now() - started, bytes ? bytes(value) : 0);
    return value;
  } catch (cause) {
    record(name, performance.now() - started);
    throw cause;
  }
}

/** One rendered frame, in milliseconds. */
export function frame(ms: number): void {
  record("frame", ms);
}

/** One span's aggregates. Mirrors `SpanReport`, camelCase on both sides. */
export type SpanReport = {
  readonly name: string;
  readonly calls: number;
  readonly totalMs: number;
  readonly meanMs: number;
  readonly minMs: number;
  readonly p50Ms: number;
  readonly p95Ms: number;
  readonly maxMs: number;
  readonly bytes: number;
  /** Percentiles over every call since the last reset, to `BIN_MS`. */
  readonly runP50Ms?: number;
  readonly runP95Ms?: number;
};

/** The `fraction` quantile of a run histogram, as the upper edge of its bin. */
export function histQuantile(hist: ArrayLike<number>, calls: number, fraction: number): number {
  if (calls === 0) return 0;
  const target = Math.max(1, Math.ceil(calls * fraction));
  let seen = 0;
  for (let bin = 0; bin < hist.length; bin += 1) {
    seen += hist[bin]!;
    if (seen >= target) return (bin + 1) * BIN_MS;
  }
  return hist.length * BIN_MS;
}

/** Nearest-rank percentile over a sorted sequence. */
function percentile(sorted: ArrayLike<number>, fraction: number): number {
  if (sorted.length === 0) return 0;
  const rank = Math.ceil(sorted.length * fraction);
  return sorted[Math.min(Math.max(rank - 1, 0), sorted.length - 1)]!;
}

/**
 * A span's retained samples, sorted.
 *
 * `slice` then `sort` on the typed array rather than `Array.from(...).sort()`:
 * `slice` already copies, so sorting it in place mutates nothing shared, and a
 * typed array sorts numerically without a comparator instead of boxing every
 * sample into a JS array first.
 */
function sortedSamples(entry: Entry): Float64Array {
  const copy = entry.recent.slice(0, entry.filled);
  copy.sort();
  return copy;
}

/** Every span recorded so far, slowest total first. */
export function report(): SpanReport[] {
  const rows: SpanReport[] = [];
  for (const [name, entry] of table) {
    const sorted = sortedSamples(entry);
    rows.push({
      name,
      calls: entry.calls,
      totalMs: entry.totalMs,
      meanMs: entry.calls === 0 ? 0 : entry.totalMs / entry.calls,
      minMs: entry.calls === 0 ? 0 : entry.minMs,
      p50Ms: percentile(sorted, 0.5),
      p95Ms: percentile(sorted, 0.95),
      maxMs: entry.maxMs,
      bytes: entry.bytes,
      runP50Ms: histQuantile(entry.hist, entry.calls, 0.5),
      runP95Ms: histQuantile(entry.hist, entry.calls, 0.95),
    });
  }
  rows.sort((a, b) => b.totalMs - a.totalMs);
  return rows;
}

/**
 * Gauges: values that are true *now* rather than accumulated over calls.
 *
 * Draw calls this frame, resident models, placed maps. A span table cannot say
 * any of them — its fields are sums and extremes over a history, and "how many
 * models are loaded" summed over a history is not a number that means anything.
 * Kept separate rather than shoehorned into `bytes` for exactly that reason.
 */
const gaugeTable = new Map<string, number>();

export function gauge(name: string, value: number): void {
  gaugeTable.set(name, value);
}

export type GaugeReport = { readonly name: string; readonly value: number };

/** Every gauge, in insertion order — which is the order they are set in, and so
 *  groups related ones together without the panel having to know about them. */
export function gauges(): GaugeReport[] {
  return Array.from(gaugeTable, ([name, value]) => ({ name, value }));
}

/** Throw the counters away, so a measurement can start from a known point. */
export function reset(): void {
  table.clear();
  gaugeTable.clear();
}

/**
 * Frames per second from the retained frame samples, or `null` before any.
 *
 * From the **median** frame, not the mean: one 300ms hitch while a batch of
 * geometry uploads drags a mean far below what the display is actually doing,
 * and the hitch is already visible as `frame`'s p95 and max.
 */
export function fps(): number | null {
  const entry = table.get("frame");
  if (!entry || entry.filled === 0) return null;
  const sorted = sortedSamples(entry);
  const median = percentile(sorted, 0.5);
  return median > 0 ? 1000 / median : null;
}

/**
 * The backend's spans. Same shape as [`report`], because
 * `src-tauri/src/perf.rs` writes the same fields in camelCase — which is what
 * lets one table component render either side.
 */
export function backendReport(): Promise<SpanReport[]> {
  return invoke<SpanReport[]>("perf_report");
}

export function backendReset(): Promise<void> {
  return invoke<void>("perf_reset");
}
