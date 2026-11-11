import { describe, expect, test } from "bun:test";

import { BridgeError } from "./client";
import { flag, main, parseCommand } from "./cli";

describe("parseCommand", () => {
  test("key=value pairs, JSON when it parses", () => {
    expect(parseCommand(["click", "text=world", "nth=1"])).toEqual({ type: "click", text: "world", nth: 1 });
    expect(parseCommand(["fly", "path=[[1,2,3],[4,5,6]]", "speed=120"])).toEqual({
      type: "fly",
      path: [[1, 2, 3], [4, 5, 6]],
      speed: 120,
    });
    expect(parseCommand(["key", "key=F3"])).toEqual({ type: "key", key: "F3" });
  });

  test("a whole JSON command", () => {
    expect(parseCommand(['{"type":"stop"}'])).toEqual({ type: "stop" });
    expect(() => parseCommand(['{"nope":1}'])).toThrow(BridgeError);
  });

  test("bad input is a BridgeError", () => {
    expect(() => parseCommand([])).toThrow(BridgeError);
    expect(() => parseCommand(["click", "oops"])).toThrow(/key=value/);
  });
});

test("flag reads the value after a name", () => {
  expect(flag(["--since", "5"], "--since")).toBe("5");
  expect(flag(["a"], "--since")).toBeUndefined();
});

describe("main", () => {
  test("with no app running it exits 2 with a clear message, and starts nothing", async () => {
    const previous = process.env.ASTRAMAPPER_BRIDGE_INFO;
    process.env.ASTRAMAPPER_BRIDGE_INFO = "/definitely/not/here.json";
    const errors: string[] = [];
    const original = console.error;
    console.error = (...args: unknown[]) => void errors.push(args.join(" "));
    try {
      expect(await main(["state"])).toBe(2);
    } finally {
      console.error = original;
      if (previous === undefined) delete process.env.ASTRAMAPPER_BRIDGE_INFO;
      else process.env.ASTRAMAPPER_BRIDGE_INFO = previous;
    }
    expect(errors.join("\n")).toMatch(/AstraMapper is not running/);
  });

  test("an unknown scenario is a usage error before anything connects", async () => {
    // `run` needs the app, so this reaches discovery first; the point is the exit code.
    const previous = process.env.ASTRAMAPPER_BRIDGE_INFO;
    process.env.ASTRAMAPPER_BRIDGE_INFO = "/definitely/not/here.json";
    const original = console.error;
    console.error = () => {};
    try {
      expect(await main(["run", "nope"])).toBe(2);
    } finally {
      console.error = original;
      if (previous === undefined) delete process.env.ASTRAMAPPER_BRIDGE_INFO;
      else process.env.ASTRAMAPPER_BRIDGE_INFO = previous;
    }
  });
});
