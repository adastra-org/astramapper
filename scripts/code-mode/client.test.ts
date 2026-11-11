import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { BridgeError, defaultInfoPath, discover, HttpBridge, parseInfo, processAlive } from "./client";

const dir = mkdtempSync(join(tmpdir(), "astramapper-client-test-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));

describe("discover", () => {
  test("no info file means the app is not running, and says how to start it", () => {
    expect(() => discover(join(dir, "nope.json"))).toThrow(/not running.*tauri dev/s);
  });

  test("a file whose process is gone is stale", () => {
    const path = join(dir, "stale.json");
    writeFileSync(path, JSON.stringify({ port: 1, token: "t", pid: 2_000_000_000 }));
    expect(() => discover(path)).toThrow(/stale/);
  });

  test("a live process gives its port and token", () => {
    const path = join(dir, "live.json");
    writeFileSync(path, JSON.stringify({ port: 17420, token: "abc", pid: process.pid }));
    expect(discover(path)).toEqual({ port: 17420, token: "abc", pid: process.pid });
  });

  test("garbage is a BridgeError, not a crash", () => {
    expect(() => parseInfo("not json")).toThrow(BridgeError);
    expect(() => parseInfo("{}")).toThrow(BridgeError);
  });

  test("the path follows the temp directory, or the override", () => {
    expect(defaultInfoPath({ TEMP: "C:\\T" })).toContain("astramapper-debug-bridge.json");
    expect(defaultInfoPath({ ASTRAMAPPER_BRIDGE_INFO: "/x/y.json" })).toBe("/x/y.json");
  });

  test("processAlive", () => {
    expect(processAlive(process.pid)).toBe(true);
    expect(processAlive(2_000_000_000)).toBe(false);
  });
});

describe("HttpBridge against a fake server", () => {
  const seen: Array<{ method: string; path: string; auth: string | null; body: string }> = [];
  let server: ReturnType<typeof Bun.serve>;
  let bridge: HttpBridge;
  const PNG = new Uint8Array([137, 80, 78, 71, 1, 2, 3]);

  beforeAll(() => {
    server = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      async fetch(request) {
        const url = new URL(request.url);
        seen.push({
          method: request.method,
          path: url.pathname + url.search,
          auth: request.headers.get("authorization"),
          body: await request.text(),
        });
        if (request.headers.get("authorization") !== "Bearer secret") {
          return Response.json({ error: "missing or wrong token" }, { status: 401 });
        }
        switch (url.pathname) {
          case "/health":
            return Response.json({ ok: true });
          case "/state":
            return Response.json({ state: null, ageMs: null, backend: [] });
          case "/logs":
            return Response.json({ next: 7, logs: [{ seq: 7, t: 0, source: "console", level: "log", text: "hi" }] });
          case "/cmd":
            return Response.json({ id: 42 });
          case "/screenshot":
            return new Response(PNG, { headers: { "Content-Type": "image/png" } });
          default:
            return Response.json({ error: "unknown route" }, { status: 404 });
        }
      },
    });
    bridge = new HttpBridge({ port: server.port!, token: "secret", pid: process.pid });
  });
  afterAll(() => server.stop(true));

  test("every route, with the bearer token", async () => {
    await bridge.health();
    expect(await bridge.state()).toEqual({ state: null, ageMs: null, backend: [] });
    expect((await bridge.logs(3, 10)).next).toBe(7);
    expect(seen[seen.length - 1]).toMatchObject({ path: "/logs?since=3&limit=10", auth: "Bearer secret" });
    expect(await bridge.cmd({ type: "stop" })).toBe(42);
    expect(seen[seen.length - 1]).toMatchObject({ method: "POST", path: "/cmd", body: '{"type":"stop"}' });
    expect(await bridge.screenshot()).toEqual(PNG);
  });

  test("a wrong token is a clear error carrying the server's message", async () => {
    const wrong = new HttpBridge({ port: server.port!, token: "nope", pid: process.pid });
    await expect(wrong.health()).rejects.toThrow(/401.*wrong token/);
  });

  test("a closed port says the app may have exited", async () => {
    const closed = new HttpBridge({ port: 1, token: "x", pid: process.pid }, 1000);
    await expect(closed.health()).rejects.toThrow(/cannot reach the bridge/);
  });
});
