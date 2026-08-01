import { describe, expect, test } from "bun:test";
import { breadcrumb, parentOf } from "./explorer";

/**
 * Only the pure path helpers are covered here. The `invoke` wrappers need a
 * Tauri host to say anything, so they are exercised by the Rust gate tests
 * behind the commands instead.
 */
describe("breadcrumb", () => {
  test("is empty at the top level", () => {
    expect(breadcrumb("")).toEqual([]);
  });

  test("makes every prefix navigable", () => {
    expect(breadcrumb(String.raw`x64a.rpf\levels\gta5`)).toEqual([
      { label: "x64a.rpf", path: "x64a.rpf" },
      { label: "levels", path: String.raw`x64a.rpf\levels` },
      { label: "gta5", path: String.raw`x64a.rpf\levels\gta5` },
    ]);
  });

  test("keeps a nested archive as an ordinary segment", () => {
    const crumbs = breadcrumb(String.raw`update\update.rpf\dlc_patch\weapons.rpf`);
    expect(crumbs.map((c) => c.label)).toEqual([
      "update",
      "update.rpf",
      "dlc_patch",
      "weapons.rpf",
    ]);
    expect(crumbs[3]?.path).toBe(String.raw`update\update.rpf\dlc_patch\weapons.rpf`);
  });
});

describe("parentOf", () => {
  test("walks one level up", () => {
    expect(parentOf(String.raw`x64a.rpf\levels\gta5`)).toBe(String.raw`x64a.rpf\levels`);
  });

  test("returns the top level from a root archive", () => {
    expect(parentOf("x64a.rpf")).toBe("");
  });

  test("stays at the top level", () => {
    expect(parentOf("")).toBe("");
  });
});
