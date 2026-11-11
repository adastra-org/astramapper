/**
 * A fake AstraMapper for the tests: it implements `Bridge` and keeps its own
 * virtual time, so a scenario that flies for four minutes runs in milliseconds.
 * It mimics the parts of the app the runner depends on: the Explorer restoring
 * the game folder, World opening only after that, snapshots refreshed once a
 * second, commands applied on the next sync, and `bridge command N failed` lines.
 */
import { deflateSync } from "node:zlib";

import type { AppSnapshot, Bridge, BridgeState, Clock, LogLine, LogPage, SpanReport } from "./types";

export class VirtualClock implements Clock {
  t = 1_000_000;
  now(): number {
    return this.t;
  }
  async sleep(ms: number): Promise<void> {
    this.t += ms;
  }
}

export type FakeOptions = {
  clock: VirtualClock;
  startIn?: "explorer" | "world";
  /** Milliseconds after Explorer shows before the game folder is open; Infinity: never. */
  gameOpensAfterMs?: number;
  /** The game was already open before the run (the backend remembers it). */
  gameAlreadyOpen?: boolean;
  heapMb?: (secondsSinceStart: number) => number;
  cellsHeld?: (secondsSinceStart: number) => number;
  /** PNG bytes the screenshot route returns. */
  png?: Uint8Array;
};

const span = (name: string, calls: number, p50: number, p95: number): SpanReport => ({
  name,
  calls,
  totalMs: calls * p50,
  meanMs: p50,
  minMs: p50,
  p50Ms: p50,
  p95Ms: p95,
  maxMs: p95 * 2,
  bytes: 0,
});

export class FakeApp implements Bridge {
  readonly clock: VirtualClock;
  readonly sent: Array<{ id: number; body: { type: string } & Record<string, unknown> }> = [];
  view: "explorer" | "world";
  private nextId = 1;
  private pending: Array<{ id: number; at: number; body: { type: string } & Record<string, unknown> }> = [];
  private lastCommand = 0;
  private logLines: LogLine[] = [];
  private seq = 1;
  private explorerSince: number | null = null;
  private gameOpen: boolean;
  private flightEnd = 0;
  private resetAt: number;
  private readonly origin: number;
  private readonly opts: FakeOptions;

  constructor(opts: FakeOptions) {
    this.opts = opts;
    this.clock = opts.clock;
    this.origin = opts.clock.t;
    this.resetAt = opts.clock.t;
    this.view = opts.startIn ?? "explorer";
    this.gameOpen = opts.gameAlreadyOpen ?? false;
    if (this.view === "explorer") this.explorerSince = opts.clock.t;
  }

  /** Console lines the app itself printed, for a test to plant. */
  log(level: string, text: string): void {
    this.logLines.push({ seq: this.seq++, t: this.clock.t - this.origin, source: "console", level, text });
  }

  private fail(id: number, type: string, why: string): void {
    this.log("warn", `bridge command ${id} (${type}) failed: ${why}`);
  }

  private process(): void {
    const t = this.clock.t;
    if (
      this.view === "explorer" &&
      !this.gameOpen &&
      this.explorerSince !== null &&
      t - this.explorerSince >= (this.opts.gameOpensAfterMs ?? 3000)
    ) {
      this.gameOpen = true;
      this.log("info", "[perf] open.scan_archives 1200.00ms");
    }
    const ready = this.pending.filter((c) => c.at + 250 <= t);
    this.pending = this.pending.filter((c) => c.at + 250 > t);
    for (const { id, at, body } of ready) {
      this.lastCommand = id;
      switch (body.type) {
        case "click": {
          const text = String(body.text);
          if (text === "explorer") {
            if (this.view !== "explorer") this.explorerSince = at;
            this.view = "explorer";
          } else if (text === "world") {
            if (this.gameOpen) this.view = "world";
            // The real app shows World regardless, but without a game it has nothing to load.
          } else {
            this.fail(id, "click", `nothing visible to click matches "${text}"`);
          }
          break;
        }
        case "camera":
        case "hold":
          if (this.view !== "world") this.fail(id, body.type, "World is not open");
          break;
        case "fly": {
          if (this.view !== "world") {
            this.fail(id, "fly", "World is not open");
            break;
          }
          const path = body.path as number[][];
          let length = 0;
          for (let i = 1; i < path.length; i++) {
            length += Math.hypot(...path[i]!.map((v, k) => v - path[i - 1]![k]!));
          }
          this.flightEnd = at + (length / Number(body.speed ?? 300)) * 1000;
          break;
        }
        case "stop":
          this.flightEnd = 0;
          break;
        case "resetPerf":
          this.resetAt = at;
          break;
        default:
          break;
      }
    }
  }

  async health(): Promise<void> {}

  async state(): Promise<BridgeState> {
    this.process();
    const t = this.clock.t;
    const at = Math.floor(t / 1000) * 1000;
    const seconds = (at - this.origin) / 1000;
    const inWorld = this.view === "world";
    const snapshot: AppSnapshot = {
      at,
      worldOpen: inWorld,
      camera: inWorld
        ? { gta: [0, 0, 0], world: [0, 0, 0], yawDeg: 0, pitchDeg: 0 }
        : null,
      flying: inWorld && t < this.flightEnd,
      lastCommand: this.lastCommand,
      fps: inWorld ? 60 : null,
      gauges: inWorld
        ? [
            { name: "cells held", value: (this.opts.cellsHeld ?? (() => 40))(seconds) },
            { name: "JS heap used (MB)", value: (this.opts.heapMb ?? (() => 1000))(seconds) },
            { name: "triangles", value: 1_500_000 },
            { name: "cell placements held", value: 20_000 },
            { name: "model cache (MB)", value: 900 },
            { name: "model cache budget (MB)", value: 1000 },
          ]
        : [],
      spans: inWorld ? [span("frame", Math.floor((at - this.resetAt) / 16), 8, 20)] : [],
      viewport: { width: 1280, height: 720 },
      clickable: ["explorer", "world", "project", "diagnostics f3", "x64a.rpf"],
    };
    return {
      state: snapshot,
      ageMs: t - at,
      backend: this.gameOpen && this.resetAt === this.origin ? [span("open.scan_archives", 1, 1200, 1200)] : [],
    };
  }

  async logs(since: number, limit = 500): Promise<LogPage> {
    this.process();
    const logs = this.logLines.filter((l) => l.seq > since).slice(0, limit);
    return { next: logs[logs.length - 1]?.seq ?? since, logs };
  }

  async cmd(body: { type: string } & Record<string, unknown>): Promise<number> {
    const id = this.nextId++;
    this.sent.push({ id, body });
    this.pending.push({ id, at: this.clock.t, body });
    return id;
  }

  async screenshot(): Promise<Uint8Array> {
    if (this.view !== "world") throw new Error("/screenshot: HTTP 504 the WebView reported no canvas to capture");
    return this.opts.png ?? solidPng(4, 4, [10, 10, 10]);
  }
}

// ------------------------------------------------------------ PNG encoding

function crc32(bytes: Uint8Array): number {
  let c = ~0;
  for (const b of bytes) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ 0xedb88320 : c >>> 1;
  }
  return ~c >>> 0;
}

function chunk(type: string, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(12 + data.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, data.length);
  for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
  out.set(data, 8);
  view.setUint32(8 + data.length, crc32(out.subarray(4, 8 + data.length)));
  return out;
}

/**
 * An 8-bit RGB PNG of `pixels` (row-major `[r, g, b]`), each row written with
 * the PNG filter `filters[y % filters.length]` (0 none, 1 sub, 2 up, 3 average, 4 Paeth).
 */
export function encodePng(
  width: number,
  height: number,
  pixels: ReadonlyArray<readonly [number, number, number]>,
  filters: readonly number[] = [0],
): Uint8Array {
  const channels = 3;
  const stride = width * channels;
  const raw = new Uint8Array((stride + 1) * height);
  const at = (x: number, y: number, c: number): number =>
    x < 0 || y < 0 ? 0 : pixels[y * width + x]![c]!;
  for (let y = 0; y < height; y++) {
    const filter = filters[y % filters.length]!;
    raw[y * (stride + 1)] = filter;
    for (let x = 0; x < width; x++) {
      for (let c = 0; c < channels; c++) {
        const value = at(x, y, c);
        const left = at(x - 1, y, c);
        const up = at(x, y - 1, c);
        const upLeft = at(x - 1, y - 1, c);
        let predicted = 0;
        if (filter === 1) predicted = left;
        else if (filter === 2) predicted = up;
        else if (filter === 3) predicted = (left + up) >> 1;
        else if (filter === 4) {
          const p = left + up - upLeft;
          const pa = Math.abs(p - left);
          const pb = Math.abs(p - up);
          const pc = Math.abs(p - upLeft);
          predicted = pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
        }
        raw[y * (stride + 1) + 1 + x * channels + c] = (value - predicted) & 0xff;
      }
    }
  }
  const header = new Uint8Array(13);
  const hv = new DataView(header.buffer);
  hv.setUint32(0, width);
  hv.setUint32(4, height);
  header[8] = 8;
  header[9] = 2;
  const parts = [
    Uint8Array.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", new Uint8Array(0)),
  ];
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

export function solidPng(width: number, height: number, rgb: readonly [number, number, number]): Uint8Array {
  return encodePng(
    width,
    height,
    Array.from({ length: width * height }, () => rgb),
  );
}
