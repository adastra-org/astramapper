import { describe, expect, test } from "bun:test";

import { compare, halfRatio, slopePerMin, stat, type Point } from "./stats";

const series = (values: number[], everyMs = 1000): Point[] =>
  values.map((v, i) => ({ t: i * everyMs, v }));

describe("slopePerMin", () => {
  test("a line rising 10 per second is 600 per minute", () => {
    expect(slopePerMin(series([0, 10, 20, 30, 40]))).toBeCloseTo(600);
  });
  test("a flat series is 0, and short ones do not divide by zero", () => {
    expect(slopePerMin(series([5, 5, 5, 5]))).toBeCloseTo(0);
    expect(slopePerMin(series([5]))).toBe(0);
    expect(slopePerMin([])).toBe(0);
    expect(slopePerMin([{ t: 1, v: 1 }, { t: 1, v: 9 }])).toBe(0);
  });
});

describe("halfRatio", () => {
  test("a climbing series is well above 1", () => {
    expect(halfRatio(series([100, 200, 300, 400, 500, 600, 700, 800]))).toBeCloseTo(800 / 400);
  });
  test("a series that levels off is about 1", () => {
    expect(halfRatio(series([100, 500, 900, 1000, 950, 1000, 990, 1000]))).toBeCloseTo(1);
  });
  test("it needs four points", () => {
    expect(halfRatio(series([1, 2, 3]))).toBeNaN();
  });
  test("growth from zero is infinite, zero to zero is 1", () => {
    expect(halfRatio(series([0, 0, 5, 5]))).toBe(Number.POSITIVE_INFINITY);
    expect(halfRatio(series([0, 0, 0, 0]))).toBe(1);
  });
});

describe("stat", () => {
  const s = series([4, 8, 6]);
  test("the plain ones", () => {
    expect(stat(s, "first")).toBe(4);
    expect(stat(s, "last")).toBe(6);
    expect(stat(s, "min")).toBe(4);
    expect(stat(s, "max")).toBe(8);
    expect(stat(s, "mean")).toBe(6);
  });
  test("an empty series has no statistic", () => {
    expect(stat([], "max")).toBeNaN();
  });
});

describe("compare", () => {
  test("every operator", () => {
    expect(compare(1, "<", 2)).toBe(true);
    expect(compare(2, "<=", 2)).toBe(true);
    expect(compare(3, ">", 2)).toBe(true);
    expect(compare(2, ">=", 2)).toBe(true);
    expect(compare(2, "==", 2)).toBe(true);
    expect(compare(2, "!=", 3)).toBe(true);
    expect(compare(2, "<", 2)).toBe(false);
  });
  test("NaN fails everything, including !=", () => {
    for (const op of ["<", "<=", ">", ">=", "==", "!="] as const) {
      expect(compare(Number.NaN, op, 1)).toBe(false);
    }
  });
});
