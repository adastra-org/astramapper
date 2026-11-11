/** Shapes shared by the client, the runner and the scenarios. */

export type Vec3 = readonly [number, number, number];

export type SpanReport = {
  name: string;
  calls: number;
  totalMs: number;
  meanMs: number;
  minMs: number;
  p50Ms: number;
  p95Ms: number;
  maxMs: number;
  bytes: number;
};

/** The WebView's snapshot, as `src/lib/debugBridge.ts` builds it. */
export type AppSnapshot = {
  at: number;
  worldOpen: boolean;
  camera: { gta: Vec3; world: Vec3; yawDeg: number; pitchDeg: number } | null;
  flying: boolean;
  lastCommand: number;
  fps: number | null;
  gauges: Array<{ name: string; value: number }>;
  spans: SpanReport[];
  viewport: { width: number; height: number };
  clickable: string[];
};

/** `GET /state`. `state` is null until the WebView has synced once. */
export type BridgeState = {
  state: AppSnapshot | null;
  ageMs: number | null;
  backend: SpanReport[];
};

export type LogLine = {
  seq: number;
  t: number;
  source: string;
  level: string;
  text: string;
};

export type LogPage = { next: number; logs: LogLine[] };

/** The bridge as the runner sees it. The HTTP client and the test fakes implement it. */
export interface Bridge {
  health(): Promise<void>;
  state(): Promise<BridgeState>;
  logs(since: number, limit?: number): Promise<LogPage>;
  /** Queue a command; resolves to its id. */
  cmd(body: { type: string } & Record<string, unknown>): Promise<number>;
  /** The canvas as PNG bytes. Rejects when World is not open. */
  screenshot(): Promise<Uint8Array>;
}

/** Time, injectable so tests need no real waiting. */
export interface Clock {
  now(): number;
  sleep(ms: number): Promise<void>;
}

export const realClock: Clock = {
  now: () => Date.now(),
  sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
};
