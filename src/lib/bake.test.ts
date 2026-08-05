import { describe, expect, test } from "bun:test";

import {
  bakeFinished,
  bakeLabel,
  bakeProgressRatio,
  type BakeProgress,
  type BakePhase,
} from "./bake";

function progress(over: Partial<BakeProgress> = {}): BakeProgress {
  return {
    phase: "assets",
    done: 0,
    total: 0,
    bytes: 0,
    budgetBytes: 8 * 1024 * 1024 * 1024,
    errors: 0,
    elapsedMs: 0,
    ...over,
  };
}

describe("bakeProgressRatio", () => {
  test("is zero before there is a total to divide by", () => {
    expect(bakeProgressRatio(progress({ phase: "maps", done: 40 }))).toBe(0);
  });

  test("reports the fraction done", () => {
    expect(bakeProgressRatio(progress({ done: 25, total: 100 }))).toBe(0.25);
  });

  /** A report can arrive after the total shrank; the bar must not overflow. */
  test("never exceeds one", () => {
    expect(bakeProgressRatio(progress({ done: 120, total: 100 }))).toBe(1);
  });
});

describe("bakeFinished", () => {
  test("is false while the pass runs", () => {
    expect(bakeFinished(progress({ phase: "maps" }))).toBe(false);
    expect(bakeFinished(progress({ phase: "assets" }))).toBe(false);
  });

  test("is true however it ended", () => {
    for (const phase of ["done", "cancelled", "full"] satisfies BakePhase[]) {
      expect(bakeFinished(progress({ phase }))).toBe(true);
    }
  });
});

describe("bakeLabel", () => {
  test("says what the pass is doing", () => {
    expect(bakeLabel(progress({ phase: "maps" }))).toContain("reading maps");
    expect(bakeLabel(progress({ done: 3, total: 10 }))).toBe("3 of 10 models");
  });

  test("reports the time when it finishes", () => {
    expect(bakeLabel(progress({ phase: "done", done: 10, elapsedMs: 4200 }))).toBe(
      "baked 10 models in 4.2s",
    );
  });

  /** Failures have to be visible: a bake that silently skipped half the city
   *  would look identical to one that worked. */
  test("names failures when there are any", () => {
    expect(bakeLabel(progress({ done: 3, total: 10, errors: 2 }))).toBe("3 of 10 models, 2 failed");
    expect(bakeLabel(progress({ phase: "cancelled", done: 3, errors: 1 }))).toBe(
      "stopped after 3 models, 1 failed",
    );
  });

  test("says so when the budget stopped it", () => {
    expect(bakeLabel(progress({ phase: "full", done: 900 }))).toContain("budget reached");
  });
});
