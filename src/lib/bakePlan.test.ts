import { describe, expect, test } from "bun:test";

import {
  ALREADY_WARM_RECORDS,
  baseGroup,
  DISK_MARGIN_BYTES,
  planBake,
  shouldOfferBake,
  type PlanInput,
} from "./bakePlan";

const GB = 1024 * 1024 * 1024;

const input = (over: {
  free?: number | null;
  cores?: number;
  budget?: number;
  used?: number;
  groups?: string[];
}): PlanInput => ({
  hardware: {
    cores: over.cores ?? 8,
    totalMemory: 16 * GB,
    availableMemory: 8 * GB,
    freeDiskBytes: over.free === undefined ? 200 * GB : over.free,
  },
  cacheBudgetBytes: over.budget ?? 8 * GB,
  cacheUsedBytes: over.used ?? 0,
  groups: (over.groups ?? ["base", "dlc1", "dlc2"]).map((id) => ({ id })),
});

describe("planBake", () => {
  test("a roomy disk and enough cores: everything", () => {
    const plan = planBake(input({}));
    expect(plan.recommended).toBe("all");
    expect(plan.groupsFor.all).toEqual(["base", "dlc1", "dlc2"]);
    expect(plan.roomBytes).toBe(8 * GB);
  });

  test("a disk that fits the base game but not the spare margin: base only", () => {
    const plan = planBake(input({ free: 8 * GB + DISK_MARGIN_BYTES - 1 }));
    expect(plan.recommended).toBe("base");
    expect(plan.groupsFor.base).toEqual(["base"]);
  });

  test("the margin is exact: room plus margin is enough for everything", () => {
    expect(planBake(input({ free: 8 * GB + DISK_MARGIN_BYTES })).recommended).toBe("all");
  });

  test("a nearly full disk: skip, and say why", () => {
    const plan = planBake(input({ free: 3 * GB }));
    expect(plan.recommended).toBe("skip");
    expect(plan.reasons.join(" ")).toContain("3.0 GB free");
    expect(plan.groupsFor.skip).toEqual([]);
  });

  test("few cores: base only, even with a huge disk", () => {
    const plan = planBake(input({ cores: 2 }));
    expect(plan.recommended).toBe("base");
    expect(plan.reasons.join(" ")).toContain("Few cores");
  });

  test("the bake pool is cores minus one, never less than one", () => {
    expect(planBake(input({ cores: 8 })).reasons.join(" ")).toContain("uses 7");
    expect(planBake(input({ cores: 1 })).reasons.join(" ")).toContain("uses 1");
  });

  test("an unreadable disk is cautious: base only", () => {
    const plan = planBake(input({ free: null }));
    expect(plan.recommended).toBe("base");
    expect(plan.reasons.join(" ")).toContain("could not be read");
  });

  test("room is what the cache has left, not its whole budget", () => {
    const plan = planBake(input({ used: 7.5 * GB, free: 20 * GB }));
    expect(plan.roomBytes).toBe(0.5 * GB);
    expect(plan.recommended).toBe("skip");
  });

  test("an install with only the base game has nothing more to offer than base", () => {
    const plan = planBake(input({ groups: ["base"] }));
    expect(plan.recommended).toBe("base");
    expect(plan.groupsFor.base).toEqual(["base"]);
  });

  test("no groups: skip", () => {
    expect(planBake(input({ groups: [] })).recommended).toBe("skip");
  });
});

describe("baseGroup", () => {
  test("is the group called base, wherever it sits", () => {
    expect(baseGroup([{ id: "dlc" }, { id: "base" }])).toBe("base");
  });
  test("falls back to the first group, and to nothing", () => {
    expect(baseGroup([{ id: "x" }, { id: "y" }])).toBe("x");
    expect(baseGroup([])).toBeNull();
  });
});

describe("shouldOfferBake", () => {
  test("offers once, on a cold cache", () => {
    expect(shouldOfferBake(false, 0)).toBe(true);
    expect(shouldOfferBake(true, 0)).toBe(false);
  });
  test("not on a warm cache, nor when the cache is unavailable", () => {
    expect(shouldOfferBake(false, ALREADY_WARM_RECORDS)).toBe(false);
    expect(shouldOfferBake(false, null)).toBe(false);
  });
});
