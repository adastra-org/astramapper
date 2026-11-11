/** Number crunching over sampled series. Pure. */

export type Stat = "last" | "first" | "min" | "max" | "mean" | "slopePerMin" | "halfRatio";

export type Point = { t: number; v: number };

/** Least-squares slope of `v` against time, in units per minute. 0 for fewer than two points. */
export function slopePerMin(points: readonly Point[]): number {
  if (points.length < 2) return 0;
  const n = points.length;
  const t0 = points[0]!.t;
  let st = 0;
  let sv = 0;
  let stt = 0;
  let stv = 0;
  for (const p of points) {
    const t = (p.t - t0) / 60_000;
    st += t;
    sv += p.v;
    stt += t * t;
    stv += t * p.v;
  }
  const denom = n * stt - st * st;
  return denom === 0 ? 0 : (n * stv - st * sv) / denom;
}

/**
 * Largest value of the second half of the series over the largest of the first.
 * Near 1 or below: the series stopped growing. Well above 1: it is still climbing.
 * Needs four points; NaN otherwise.
 */
export function halfRatio(points: readonly Point[]): number {
  if (points.length < 4) return Number.NaN;
  const mid = Math.floor(points.length / 2);
  const first = Math.max(...points.slice(0, mid).map((p) => p.v));
  const second = Math.max(...points.slice(mid).map((p) => p.v));
  return first === 0 ? (second === 0 ? 1 : Number.POSITIVE_INFINITY) : second / first;
}

/** One statistic of a series; NaN when the series is empty (or too short for the stat). */
export function stat(points: readonly Point[], which: Stat): number {
  if (points.length === 0) return Number.NaN;
  const values = points.map((p) => p.v);
  switch (which) {
    case "last":
      return values[values.length - 1]!;
    case "first":
      return values[0]!;
    case "min":
      return Math.min(...values);
    case "max":
      return Math.max(...values);
    case "mean":
      return values.reduce((a, b) => a + b, 0) / values.length;
    case "slopePerMin":
      return slopePerMin(points);
    case "halfRatio":
      return halfRatio(points);
  }
}

export type Op = "<" | "<=" | ">" | ">=" | "==" | "!=";

/** NaN never satisfies any comparison, so a missing number fails an assertion. */
export function compare(actual: number, op: Op, expected: number): boolean {
  if (Number.isNaN(actual)) return false;
  switch (op) {
    case "<":
      return actual < expected;
    case "<=":
      return actual <= expected;
    case ">":
      return actual > expected;
    case ">=":
      return actual >= expected;
    case "==":
      return actual === expected;
    case "!=":
      return actual !== expected;
  }
}
