import { describe, expect, test } from "bun:test";

import {
  formatArg,
  formatLine,
  lookAngles,
  registerCamera,
  runCommand,
  stepFlight,
  type CameraApi,
  type Flight,
} from "./debugBridge";

describe("lookAngles", () => {
  test("looking down -Z is yaw 0 and level", () => {
    const { yaw, pitch } = lookAngles([0, 0, 0], [0, 0, -10]);
    expect(yaw).toBeCloseTo(0);
    expect(pitch).toBeCloseTo(0);
  });

  test("it inverts the camera's look vector", () => {
    const from = [10, 5, -3] as const;
    const to = [-40, 60, 80] as const;
    const { yaw, pitch } = lookAngles(from, to);
    const length = Math.hypot(to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    expect(-Math.sin(yaw) * Math.cos(pitch)).toBeCloseTo((to[0] - from[0]) / length);
    expect(Math.sin(pitch)).toBeCloseTo((to[1] - from[1]) / length);
    expect(-Math.cos(yaw) * Math.cos(pitch)).toBeCloseTo((to[2] - from[2]) / length);
  });

  test("the same point has no direction and does not produce NaN", () => {
    expect(lookAngles([1, 2, 3], [1, 2, 3])).toEqual({ yaw: 0, pitch: 0 });
  });
});

const flight = (): Flight => ({
  points: [
    [100, 0, 0],
    [100, 0, 100],
  ],
  speed: 50,
  next: 0,
  position: [0, 0, 0],
});

describe("stepFlight", () => {
  test("moves at the speed along the leg and is not done", () => {
    const f = flight();
    const step = stepFlight(f, 1);
    expect(step.position).toEqual([50, 0, 0]);
    expect(step.done).toBe(false);
  });

  test("carries leftover distance into the next leg", () => {
    const f = flight();
    const step = stepFlight(f, 3);
    // 150 m: 100 along the first leg, 50 along the second.
    expect(step.position[0]).toBeCloseTo(100);
    expect(step.position[2]).toBeCloseTo(50);
    expect(f.next).toBe(1);
  });

  test("finishes on the last waypoint and stays there", () => {
    const f = flight();
    const step = stepFlight(f, 100);
    expect(step.position).toEqual([100, 0, 100]);
    expect(step.done).toBe(true);
    expect(stepFlight(f, 1).position).toEqual([100, 0, 100]);
  });

  test("a zero step does not move", () => {
    const f = flight();
    expect(stepFlight(f, 0).position).toEqual([0, 0, 0]);
  });
});

describe("console formatting", () => {
  test("strings stay as they are and values are JSON", () => {
    expect(formatArg("hi")).toBe("hi");
    expect(formatArg({ a: 1 })).toBe('{"a":1}');
    expect(formatArg(undefined)).toBe("undefined");
    expect(formatArg(3)).toBe("3");
  });

  test("an error keeps its stack and a cycle does not throw", () => {
    expect(formatArg(new Error("boom"))).toContain("boom");
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(formatArg(cyclic)).toBe("[object Object]");
  });

  test("a line joins its arguments and cuts a very long one", () => {
    expect(formatLine(["a", 1, { b: 2 }])).toBe('a 1 {"b":2}');
    const long = formatLine(["x".repeat(5000)]);
    expect(long.length).toBeLessThan(2100);
    expect(long).toContain("chars cut");
  });
});

describe("runCommand", () => {
  const recorded: Array<{ position: readonly number[]; yaw: number; pitch: number }> = [];
  const keys: Array<[string, boolean]> = [];
  const api: CameraApi = {
    pose: () => ({ position: [0, 0, 0], yaw: 0.5, pitch: 0.1 }),
    set: (position, yaw, pitch) => recorded.push({ position, yaw, pitch }),
    setKey: (key, down) => keys.push([key, down]),
    capture: () => null,
  };

  test("camera commands fail clearly while World is not open", () => {
    registerCamera(null);
    expect(() => runCommand({ type: "camera", gta: [0, 0, 0] })).toThrow("World is not open");
  });

  test("a camera command converts game coordinates to world space", () => {
    registerCamera(api);
    recorded.length = 0;
    runCommand({ type: "camera", gta: [10, 20, 30], yawDeg: 90, pitchDeg: -45 });
    const set = recorded[recorded.length - 1]!;
    // Game (x, y, z) is world (x, z, -y).
    expect(set.position).toEqual([10, 30, -20]);
    expect(set.yaw).toBeCloseTo(Math.PI / 2);
    expect(set.pitch).toBeCloseTo(-Math.PI / 4);
  });

  test("lookAtGta overrides the angles", () => {
    registerCamera(api);
    recorded.length = 0;
    // From the origin toward game +Y, which is world -Z: yaw 0, level.
    runCommand({ type: "camera", gta: [0, 0, 0], lookAtGta: [0, 100, 0] });
    expect(recorded[recorded.length - 1]!.yaw).toBeCloseTo(0);
    expect(recorded[recorded.length - 1]!.pitch).toBeCloseTo(0);
  });

  test("bad input is rejected with the field name", () => {
    registerCamera(api);
    expect(() => runCommand({ type: "camera", gta: [1, 2] })).toThrow("gta");
    expect(() => runCommand({ type: "fly", path: [] })).toThrow("path");
    expect(() => runCommand({ type: "nope" })).toThrow("unknown command");
  });

  test("hold presses the key and stop releases it", () => {
    registerCamera(api);
    keys.length = 0;
    runCommand({ type: "hold", key: "forward", ms: 60_000 });
    expect(keys).toEqual([["forward", true]]);
    runCommand({ type: "stop" });
    expect(keys[keys.length - 1]).toEqual(["forward", false]);
  });
});
