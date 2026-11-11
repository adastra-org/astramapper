import { beforeEach, describe, expect, test } from "bun:test";

import { fps, frame, histQuantile, measure, measureAsync, record, report, reset } from "./perf";

beforeEach(reset);

describe("record", () => {
  test("accumulates rather than replacing", () => {
    for (let i = 0; i < 5; i += 1) record("a", 10, 3);

    const row = report().find((r) => r.name === "a")!;
    expect(row.calls).toBe(5);
    expect(row.totalMs).toBeCloseTo(50, 6);
    expect(row.meanMs).toBeCloseTo(10, 6);
    expect(row.bytes).toBe(15);
  });

  /** The window bounds memory; min and max must not be bounded with it. */
  test("keeps min and max over every call, not just the window", () => {
    record("a", 1);
    for (let i = 0; i < 300; i += 1) record("a", 10);
    record("a", 99);

    const row = report().find((r) => r.name === "a")!;
    expect(row.calls).toBe(302);
    expect(row.minMs).toBe(1);
    expect(row.maxMs).toBe(99);
  });

  test("reports percentiles off the retained window", () => {
    for (let value = 1; value <= 100; value += 1) record("a", value);

    const row = report().find((r) => r.name === "a")!;
    expect(row.p50Ms).toBe(50);
    expect(row.p95Ms).toBe(95);
  });

  /** The window is the last 256 samples; a flight is thousands of frames. */
  test("run percentiles cover every call since the reset, not just the window", () => {
    for (let i = 0; i < 1000; i += 1) record("a", 10);
    for (let i = 0; i < 100; i += 1) record("a", 60);
    for (let i = 0; i < 256; i += 1) record("a", 10);

    const row = report().find((r) => r.name === "a")!;
    // The window now holds only 10s, so it cannot see the 100 slow calls.
    expect(row.p95Ms).toBe(10);
    // 100 of 1,356 calls (7%) took 60 ms, so the run's 95th percentile is one of them.
    expect(row.runP50Ms).toBe(10.5);
    expect(row.runP95Ms).toBe(60.5);
  });

  test("run percentiles forget on reset", () => {
    record("a", 30);
    reset();
    record("a", 2);

    expect(report().find((r) => r.name === "a")!.runP95Ms).toBe(2.5);
  });

  test("a sample past the histogram's range counts in its last bin", () => {
    record("a", 90_000);

    const row = report().find((r) => r.name === "a")!;
    expect(row.runP95Ms).toBe(500);
    expect(row.maxMs).toBe(90_000);
  });

  test("orders the report by where the time went", () => {
    record("small", 1);
    record("large", 500);
    record("medium", 50);

    expect(report().map((r) => r.name)).toEqual(["large", "medium", "small"]);
  });

  test("an untouched recorder reports nothing", () => {
    expect(report()).toEqual([]);
  });
});

describe("measure", () => {
  test("returns the body's value and records a call", () => {
    expect(measure("a", () => 42)).toBe(42);
    expect(report().find((r) => r.name === "a")!.calls).toBe(1);
  });

  /** A body that throws still spent the time it spent. */
  test("records a throwing body before rethrowing", () => {
    expect(() =>
      measure("a", () => {
        throw new Error("nope");
      }),
    ).toThrow("nope");
    expect(report().find((r) => r.name === "a")!.calls).toBe(1);
  });
});

describe("measureAsync", () => {
  test("records the resolved value's size", async () => {
    const value = await measureAsync(
      "a",
      () => Promise.resolve([1, 2, 3]),
      (v) => v.length,
    );

    expect(value).toEqual([1, 2, 3]);
    const row = report().find((r) => r.name === "a")!;
    expect(row.calls).toBe(1);
    expect(row.bytes).toBe(3);
  });

  test("records a rejection with no bytes, and still rejects", async () => {
    await expect(
      measureAsync(
        "a",
        () => Promise.reject(new Error("nope")),
        () => 99,
      ),
    ).rejects.toThrow("nope");

    const row = report().find((r) => r.name === "a")!;
    expect(row.calls).toBe(1);
    expect(row.bytes).toBe(0);
  });
});

describe("fps", () => {
  test("is null before any frame has been recorded", () => {
    expect(fps()).toBeNull();
  });

  test("comes off the median frame", () => {
    for (let i = 0; i < 60; i += 1) frame(16);
    expect(fps()).toBeCloseTo(1000 / 16, 6);
  });

  /**
   * The reason it is the median. One long hitch among sixty good frames is a
   * hitch, not a frame rate — and it is already visible as p95 and max.
   */
  test("a single hitch does not drag the reported rate down", () => {
    for (let i = 0; i < 59; i += 1) frame(16);
    frame(300);

    expect(fps()).toBeCloseTo(1000 / 16, 6);
    expect(report().find((r) => r.name === "frame")!.maxMs).toBe(300);
  });
});

describe("histQuantile", () => {
  test("is the upper edge of the bin the rank falls in", () => {
    const hist = new Uint32Array(10);
    hist[2] = 5;
    hist[6] = 5;
    expect(histQuantile(hist, 10, 0.5)).toBe(1.5);
    expect(histQuantile(hist, 10, 0.51)).toBe(3.5);
    expect(histQuantile(hist, 10, 1)).toBe(3.5);
  });

  test("is zero with no calls", () => {
    expect(histQuantile(new Uint32Array(4), 0, 0.95)).toBe(0);
  });
});
