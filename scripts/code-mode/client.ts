/**
 * HTTP client for the debug bridge (`docs/debug-bridge.md`), and the discovery of
 * where it listens. Starts nothing: when the app is not running it says so.
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import type { Bridge, BridgeState, LogPage } from "./types";

export type BridgeInfo = { port: number; token: string; pid: number };

/** A failure the CLI prints without a stack: the app is down, the token is wrong... */
export class BridgeError extends Error {
  override name = "BridgeError";
}

export function defaultInfoPath(env: Record<string, string | undefined> = process.env): string {
  if (env.ASTRAMAPPER_BRIDGE_INFO) return env.ASTRAMAPPER_BRIDGE_INFO;
  const dir = env.TEMP ?? env.TMP ?? env.TMPDIR ?? "/tmp";
  return join(dir, "astramapper-debug-bridge.json");
}

export function parseInfo(text: string): BridgeInfo {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new BridgeError("the bridge info file is not valid JSON");
  }
  const info = raw as Partial<BridgeInfo> | null;
  if (!info || typeof info.port !== "number" || typeof info.token !== "string") {
    throw new BridgeError("the bridge info file has no port and token");
  }
  return { port: info.port, token: info.token, pid: typeof info.pid === "number" ? info.pid : 0 };
}

export function processAlive(pid: number): boolean {
  if (pid <= 0) return true; // unknown: let /health decide
  try {
    process.kill(pid, 0);
    return true;
  } catch (cause) {
    // EPERM means it exists but is not ours.
    return (cause as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** Read the info file; throws a clear `BridgeError` when the app is not running. */
export function discover(path: string = defaultInfoPath()): BridgeInfo {
  if (!existsSync(path)) {
    throw new BridgeError(
      `AstraMapper is not running: no bridge info at ${path}. Start it with \`bun run tauri dev\` (this tool starts nothing).`,
    );
  }
  const info = parseInfo(readFileSync(path, "utf8"));
  if (!processAlive(info.pid)) {
    throw new BridgeError(
      `AstraMapper is not running: ${path} is stale (pid ${info.pid} is gone). Start it with \`bun run tauri dev\`.`,
    );
  }
  return info;
}

export class HttpBridge implements Bridge {
  constructor(
    private readonly info: BridgeInfo,
    private readonly timeoutMs = 10_000,
  ) {}

  private async request(path: string, init: RequestInit = {}): Promise<Response> {
    const url = `http://127.0.0.1:${this.info.port}${path}`;
    let response: Response;
    try {
      response = await fetch(url, {
        ...init,
        headers: { ...init.headers, Authorization: `Bearer ${this.info.token}` },
        signal: AbortSignal.timeout(this.timeoutMs),
      });
    } catch (cause) {
      throw new BridgeError(
        `cannot reach the bridge on port ${this.info.port} (${String(cause)}). Is AstraMapper still running?`,
      );
    }
    if (!response.ok) {
      let message = response.statusText;
      try {
        message = ((await response.json()) as { error?: string }).error ?? message;
      } catch {
        // keep the status text
      }
      throw new BridgeError(`${path}: HTTP ${response.status} ${message}`);
    }
    return response;
  }

  async health(): Promise<void> {
    await this.request("/health");
  }

  async state(): Promise<BridgeState> {
    return (await (await this.request("/state")).json()) as BridgeState;
  }

  async logs(since: number, limit = 500): Promise<LogPage> {
    return (await (await this.request(`/logs?since=${since}&limit=${limit}`)).json()) as LogPage;
  }

  async cmd(body: { type: string } & Record<string, unknown>): Promise<number> {
    const response = await this.request("/cmd", { method: "POST", body: JSON.stringify(body) });
    return ((await response.json()) as { id: number }).id;
  }

  async screenshot(): Promise<Uint8Array> {
    // The backend waits up to 6 s for the WebView; give the HTTP call a little more.
    const response = await this.request("/screenshot");
    return new Uint8Array(await response.arrayBuffer());
  }
}

/** Discover the app and check it answers. */
export async function connect(path?: string): Promise<HttpBridge> {
  const bridge = new HttpBridge(discover(path));
  await bridge.health();
  return bridge;
}
