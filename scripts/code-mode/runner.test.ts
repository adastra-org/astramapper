/* oxlint-disable no-await-in-loop -- steps and polls run one after another by design */
import { describe, expect, test } from "bun:test";

import { encodePng, FakeApp, solidPng, VirtualClock, type FakeOptions } from "./fake-app";
import { PLACES, resolvePlace } from "./places";
import { renderMarkdown } from "./report";
import { assertionLabel, describeStep, runScenario, type Report } from "./runner";
import { scenario, type Scenario } from "./scenario";
import { SCENARIOS } from "./scenarios";

async function run(
  def: Scenario,
  options: Partial<Omit<FakeOptions, "clock">> = {},
): Promise<{ report: Report; app: FakeApp; files: Map<string, Uint8Array> }> {
  const clock = new VirtualClock();
  const app = new FakeApp({ clock, ...options });
  const files = new Map<string, Uint8Array>();
  const report = await runScenario(def, app, {
    outDir: "out",
    clock,
    write: async (path, data) => {
      files.set(path, data);
    },
  });
  return { report, app, files };
}

describe("openWorld", () => {
  test("goes to Explorer, waits for the game to open, then clicks World", async () => {
    const { report, app } = await run(
      scenario({ name: "t", description: "", steps: [{ do: "openWorld" }] }),
      { startIn: "explorer", gameOpensAfterMs: 12_000 },
    );
    expect(report.ok).toBe(true);
    expect(app.view).toBe("world");
    const clicks = app.sent.filter((c) => c.body.type === "click").map((c) => c.body.text);
    expect(clicks).toEqual(["explorer", "world"]);
    // World was not asked for before the scan was done.
    const worldClickAt = app.sent.findIndex((c) => c.body.text === "world");
    expect(worldClickAt).toBeGreaterThan(0);
  });

  test("when World is already open it does nothing", async () => {
    const { report, app } = await run(
      scenario({ name: "t", description: "", steps: [{ do: "openWorld" }] }),
      { startIn: "world" },
    );
    expect(report.ok).toBe(true);
    expect(app.sent).toEqual([]);
  });

  test("a game folder that never opens fails with what to do about it", async () => {
    const { report } = await run(
      scenario({ name: "t", description: "", steps: [{ do: "openWorld", timeoutMs: 40_000 }] }),
      { startIn: "explorer", gameOpensAfterMs: Number.POSITIVE_INFINITY },
    );
    expect(report.ok).toBe(false);
    expect(report.steps[0]!.error).toMatch(/saved game folder/);
  });
});

describe("steps", () => {
  const open = { do: "openWorld" } as const;

  test("go sends a camera command with resolved places and look-at", async () => {
    const { app, report } = await run(
      scenario({
        name: "t",
        description: "",
        steps: [open, { do: "go", to: "legion-square", lookAt: [1, 2, 3], pitchDeg: -5 }],
      }),
    );
    expect(report.ok).toBe(true);
    const camera = app.sent.find((c) => c.body.type === "camera")!;
    expect(camera.body).toMatchObject({ gta: PLACES["legion-square"], lookAtGta: [1, 2, 3], pitchDeg: -5 });
  });

  test("a command that fails in the app fails its step with the app's message, and the rest is skipped", async () => {
    const { report } = await run(
      scenario({
        name: "t",
        description: "",
        steps: [{ do: "go", to: "legion-square" }, { do: "wait", ms: 1 }],
      }),
      { startIn: "explorer" },
    );
    expect(report.ok).toBe(false);
    expect(report.steps[0]!.error).toBe("bridge command 1 (camera) failed: World is not open");
    expect(report.steps[1]!.error).toMatch(/skipped/);
  });

  test("fly waits for the flight to end, by its length and speed", async () => {
    const clock = new VirtualClock();
    const app = new FakeApp({ clock, startIn: "world" });
    const before = clock.t;
    const report = await runScenario(
      scenario({
        name: "t",
        description: "",
        steps: [{ do: "fly", path: [[0, 0, 0], [0, 6000, 0]], speed: 100 }],
      }),
      app,
      { outDir: "out", clock, write: async () => {} },
    );
    expect(report.ok).toBe(true);
    // 60 s of flight.
    expect(clock.t - before).toBeGreaterThanOrEqual(60_000);
    expect(clock.t - before).toBeLessThan(75_000);
  });

  test("waitFor times out with the condition in the message", async () => {
    const { report } = await run(
      scenario({
        name: "t",
        description: "",
        steps: [open, { do: "waitFor", gauge: "cells held", op: ">", value: 1000, timeoutMs: 5000 }],
      }),
    );
    expect(report.steps[1]!.error).toMatch(/timed out after 5000 ms waiting for cells held > 1000/);
  });

  test("an unknown place is a failed step, not a crash", async () => {
    const { report } = await run(
      scenario({ name: "t", description: "", steps: [open, { do: "go", to: "atlantis" }] }),
    );
    expect(report.steps[1]!.error).toMatch(/unknown place "atlantis"/);
  });

  test("shots are written under the output folder", async () => {
    const { report, files } = await run(
      scenario({ name: "t", description: "", steps: [open, { do: "shot", name: "one" }] }),
    );
    expect(report.screenshots).toEqual(["one.png"]);
    expect(files.has("out/one.png")).toBe(true);
  });

  test("resetPerf clears the span counts", async () => {
    const { report } = await run(
      scenario({
        name: "t",
        description: "",
        steps: [
          open,
          { do: "wait", ms: 20_000 },
          { do: "resetPerf" },
          { do: "assert", kind: "span", span: "frame", stat: "calls", op: "<", value: 400 },
        ],
      }),
    );
    expect(report.assertions[0]!.ok).toBe(true);
  });
});

describe("sample and assertions", () => {
  test("a flight sampled until it ends passes bounded-working-set assertions", async () => {
    const { report } = await run(
      scenario({
        name: "t",
        description: "d",
        steps: [
          { do: "openWorld" },
          { do: "fly", path: [[0, 0, 0], [0, 6000, 0]], speed: 100, wait: false },
          { do: "sample", name: "flight", forMs: 300_000, untilIdle: true, gauges: ["cells held"] },
          { do: "assert", kind: "series", sample: "flight", key: "cells held", stat: "min", op: ">", value: 0 },
          { do: "assert", kind: "series", sample: "flight", key: "cells held", stat: "max", op: "<=", value: 50 },
          { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
        ],
      }),
      { cellsHeld: (s) => 30 + (s % 10) },
    );
    expect(report.ok).toBe(true);
    const cells = report.series.find((s) => s.key === "cells held")!;
    // One point per fresh snapshot, about 60 s of flight.
    expect(cells.points).toBeGreaterThan(50);
    expect(cells.points).toBeLessThan(75);
    expect(cells.max).toBeLessThanOrEqual(39);
  });

  test("a heap that keeps climbing fails the growth assertion; a flat one passes", async () => {
    const steps: Scenario["steps"] = [
      { do: "openWorld" },
      { do: "sample", name: "tour", forMs: 60_000, gauges: ["JS heap used (MB)"] },
      { do: "assert", kind: "series", sample: "tour", key: "JS heap used (MB)", stat: "halfRatio", op: "<=", value: 1.2 },
      { do: "assert", kind: "series", sample: "tour", key: "JS heap used (MB)", stat: "slopePerMin", op: "<", value: 100 },
    ];
    const leaking = await run(scenario({ name: "t", description: "", steps }), { heapMb: (s) => 1000 + s * 40 });
    expect(leaking.report.ok).toBe(false);
    expect(leaking.report.assertions.map((a) => a.ok)).toEqual([false, false]);
    expect(leaking.report.assertions[1]!.actual).toBeCloseTo(2400, -1);

    const flat = await run(scenario({ name: "t", description: "", steps }), { heapMb: (s) => 1500 + (s % 3) });
    expect(flat.report.ok).toBe(true);
  });

  test("a series can be limited by another series of the same sample", async () => {
    const steps: Scenario["steps"] = [
      { do: "openWorld" },
      { do: "sample", name: "s", forMs: 5_000, gauges: ["model cache (MB)", "model cache budget (MB)", "nope"] },
      { do: "assert", kind: "series", sample: "s", key: "model cache (MB)", stat: "max", op: "<=", value: { key: "model cache budget (MB)" } },
      { do: "assert", kind: "series", sample: "s", key: "model cache (MB)", stat: "max", op: "<=", value: { key: "model cache budget (MB)", factor: 0.5 } },
      { do: "assert", kind: "series", sample: "s", key: "model cache (MB)", stat: "max", op: "<=", value: { key: "missing" } },
    ];
    const { report } = await run(scenario({ name: "t", description: "", steps }));
    expect(report.assertions.map((x) => [x.ok, x.expected])).toEqual([
      [true, 1000],
      [false, 500],
      [false, Number.NaN],
    ]);
    expect(report.assertions[1]!.detail).toBe("limit is 0.5 x model cache budget (MB) max");
  });

  test("a gauge or series that does not exist fails with a reason, never passes", async () => {
    const { report } = await run(
      scenario({
        name: "t",
        description: "",
        steps: [
          { do: "openWorld" },
          { do: "assert", kind: "gauge", gauge: "nope", op: "<", value: 1 },
          { do: "assert", kind: "series", sample: "none", key: "x", stat: "max", op: "<", value: 1 },
          { do: "assert", kind: "span", span: "nope", stat: "p95", op: "<", value: 1 },
          { do: "assert", kind: "image", shot: "none", region: [0, 0, 1, 1], op: ">", value: 0 },
        ],
      }),
    );
    expect(report.assertions.map((a) => a.ok)).toEqual([false, false, false, false]);
    expect(report.assertions.every((a) => a.actual === null && a.detail)).toBe(true);
  });

  test("span assertions read the WebView's spans, then the backend's", async () => {
    const { report } = await run(
      scenario({
        name: "t",
        description: "",
        steps: [
          { do: "openWorld" },
          { do: "assert", kind: "span", span: "frame", stat: "p95", op: "<", value: 30 },
          { do: "assert", kind: "span", span: "frame", stat: "p50", op: "<", value: 5 },
        ],
      }),
    );
    expect(report.assertions.map((a) => [a.ok, a.actual])).toEqual([
      [true, 20],
      [false, 8],
    ]);
  });

  test("console errors count, and ignore patterns exclude lines", async () => {
    const clock = new VirtualClock();
    const app = new FakeApp({ clock, startIn: "world" });
    app.log("error", "uncaught: boom");
    app.log("error", "texture 12 missing");
    app.log("warn", "slow");
    const report = await runScenario(
      scenario({
        name: "t",
        description: "",
        steps: [
          { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
          { do: "assert", kind: "logs", level: "error", op: "<=", value: 0, ignore: ["boom", "texture \\d+"] },
        ],
      }),
      app,
      { outDir: "out", clock, write: async () => {} },
    );
    // Lines printed before the run began are not the run's.
    expect(report.assertions.map((a) => a.actual)).toEqual([0, 0]);

    // One printed during the run is.
    const live = new FakeApp({ clock, startIn: "world" });
    const during = await runScenario(
      scenario({
        name: "t",
        description: "",
        steps: [
          { do: "cmd", command: { type: "camera", gta: [0, 0, 0] } },
          { do: "assert", kind: "logs", level: "error", op: "<=", value: 0 },
        ],
      }),
      Object.assign(live, {
        async cmd(body: { type: string } & Record<string, unknown>) {
          live.log("error", "uncaught: boom");
          return FakeApp.prototype.cmd.call(live, body);
        },
      }),
      { outDir: "out", clock, write: async () => {} },
    );
    expect(during.assertions[0]).toMatchObject({ ok: false, actual: 1 });
    expect(during.console.errors).toBe(1);
    expect(during.console.sample[0]).toContain("boom");
  });

  test("an image assertion measures how much of a region is not the clear colour", async () => {
    const w = 8;
    const h = 8;
    // Top half the scene's clear colour (sky), bottom half ground.
    const pixels = Array.from({ length: w * h }, (_, i): [number, number, number] =>
      Math.floor(i / w) < 4 ? [16, 20, 28] : [90, 120, 60],
    );
    const { report } = await run(
      scenario({
        name: "t",
        description: "",
        steps: [
          { do: "openWorld" },
          { do: "shot", name: "view" },
          { do: "assert", kind: "image", shot: "view", region: [0, 0.5, 1, 1], op: ">=", value: 0.9, label: "ground" },
          { do: "assert", kind: "image", shot: "view", region: [0, 0, 1, 0.5], op: "<=", value: 0.1, label: "sky" },
          { do: "assert", kind: "image", shot: "view", region: [0, 0, 1, 1], op: ">=", value: 0.9, label: "all" },
        ],
      }),
      { png: encodePng(w, h, pixels) },
    );
    expect(report.assertions.map((a) => [a.label, a.ok])).toEqual([
      ["ground", true],
      ["sky", true],
      ["all", false],
    ]);
  });

  test("a tolerance keeps a slightly lighter grid from counting as ground", async () => {
    const steps: Scenario["steps"] = [
      { do: "openWorld" },
      { do: "shot", name: "grid" },
      { do: "assert", kind: "image", shot: "grid", region: [0, 0, 1, 1], op: ">=", value: 0.9 },
      { do: "assert", kind: "image", shot: "grid", region: [0, 0, 1, 1], op: "<=", value: 0.1, tolerance: 40 },
    ];
    const { report } = await run(scenario({ name: "t", description: "", steps }), {
      png: solidPng(4, 4, [42, 48, 58]),
    });
    expect(report.assertions.map((a) => a.ok)).toEqual([true, true]);
  });

  test("the corner pixel can stand for the background", async () => {
    const { report } = await run(
      scenario({
        name: "t",
        description: "",
        steps: [
          { do: "openWorld" },
          { do: "shot", name: "flat" },
          { do: "assert", kind: "image", shot: "flat", region: [0, 0, 1, 1], op: "==", value: 0, background: "corner" },
        ],
      }),
      { png: solidPng(4, 4, [77, 88, 99]) },
    );
    expect(report.ok).toBe(true);
  });
});

describe("report", () => {
  test("markdown has a verdict, the table of assertions and the samples", async () => {
    const { report } = await run(
      scenario({
        name: "demo",
        description: "A demo | with a pipe",
        steps: [
          { do: "openWorld" },
          { do: "sample", name: "s", forMs: 5_000, gauges: ["cells held"] },
          { do: "assert", kind: "series", sample: "s", key: "cells held", stat: "max", op: "<=", value: 10 },
        ],
      }),
    );
    const md = renderMarkdown(report);
    expect(md).toContain("# demo: FAIL");
    expect(md).toContain("| FAIL | cells held max (s) <= 10 | 40 | <= 10 |");
    expect(md).toContain("| s | cells held |");
    expect(md).toContain("0 errors, 0 warnings");
  });

  test("a failed step is listed with its message", async () => {
    const { report } = await run(
      scenario({ name: "x", description: "", steps: [{ do: "go", to: "legion-square" }] }),
      { startIn: "explorer" },
    );
    expect(renderMarkdown(report)).toMatch(/## Failed steps\n\n- 1\. go to legion-square: bridge command 1/);
  });

  test("the JSON report round-trips", async () => {
    const { report } = await run(scenario({ name: "x", description: "", steps: [{ do: "openWorld" }] }));
    expect(JSON.parse(JSON.stringify(report))).toEqual(report);
  });
});

describe("descriptions and shipped scenarios", () => {
  test("steps and assertions describe themselves", () => {
    expect(describeStep({ do: "fly", path: ["a", [1, 2, 3]], speed: 50 })).toBe("fly a > [1,2,3] at 50 m/s");
    expect(assertionLabel({ kind: "gauge", gauge: "g", op: "<", value: 3 })).toBe("g < 3");
    expect(assertionLabel({ kind: "gauge", gauge: "g", op: "<", value: 3, label: "mine" })).toBe("mine");
  });

  test("places resolve by name, loosely spelled", () => {
    expect(resolvePlace("Legion Square")).toEqual(PLACES["legion-square"]!);
    expect(resolvePlace([1, 2, 3])).toEqual([1, 2, 3]);
    expect(() => resolvePlace("nowhere")).toThrow(/known:/);
  });

  test("every shipped scenario only uses known places and has assertions", () => {
    for (const s of Object.values(SCENARIOS)) {
      expect(s.steps.some((step) => step.do === "assert")).toBe(true);
      for (const step of s.steps) {
        if (step.do === "go") resolvePlace(step.to);
        if (step.do === "go" && step.lookAt) resolvePlace(step.lookAt);
        if (step.do === "fly") step.path.forEach(resolvePlace);
      }
    }
  });

  test("the shipped scenarios run to completion against a healthy fake app", async () => {
    for (const name of ["smoke", "streaming-bounded", "heap-flight"]) {
      const { report } = await run(SCENARIOS[name]!, { heapMb: () => 1500, cellsHeld: () => 50 });
      expect(report.steps.filter((s) => !s.ok)).toEqual([]);
      expect(report.assertions.filter((a) => !a.ok)).toEqual([]);
      expect(report.ok).toBe(true);
    }
  });

  test("heap-flight fails on a leaking heap", async () => {
    const { report } = await run(SCENARIOS["heap-flight"]!, { heapMb: (s) => 1000 + s * 20 });
    expect(report.ok).toBe(false);
  });
});
