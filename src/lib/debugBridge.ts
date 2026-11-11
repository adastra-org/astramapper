/**
 * The WebView half of the debug bridge (`src-tauri/src/debug_bridge.rs`).
 *
 * Four times a second it sends the console lines and, once a second, a state
 * snapshot to the backend, and runs the commands that come back. The backend
 * serves them over loopback HTTP, so a shell can read the console and F3's
 * numbers as they happen and move the camera. Nothing here runs unless the
 * backend says the bridge is on (debug builds, or `ASTRAMAPPER_DEBUG_BRIDGE=1`),
 * so a shipped app never patches `console`.
 *
 * # Commands
 *
 * Positions are game coordinates (Z up), the ones the rest of the app prints.
 *
 * | `type` | fields |
 * | --- | --- |
 * | `camera` | `gta?: [x,y,z]`, `lookAtGta?: [x,y,z]`, `yawDeg?`, `pitchDeg?` |
 * | `fly` | `path: [[x,y,z], ...]`, `speed?` (m/s, default 300): fly through the points |
 * | `hold` | `key` (`forward`, `back`, `left`, `right`, `up`, `down`, `sprint`), `ms` |
 * | `stop` | cancel a flight and release held keys |
 * | `screenshot` | the next sync carries the canvas as a PNG |
 * | `resetPerf` | clear the F3 spans |
 * | `click` | `text`, `nth?`: click the visible button, link or checkbox whose label contains `text` |
 * | `key` | `key`: send a keydown (and keyup) to the window, e.g. `F3` |
 *
 * Camera commands do nothing until World is open; `/state` says whether it is.
 */
import { invoke } from "@tauri-apps/api/core";

import * as perf from "./perf";
import { gtaToWorld, worldToGta } from "./world";

type Vec3 = readonly [number, number, number];

/** What World exposes so the bridge can read and move its fly camera. */
export type CameraApi = {
  /** Position in three.js world space, and yaw/pitch in radians. */
  pose(): { position: Vec3; yaw: number; pitch: number };
  set(position: Vec3, yaw: number, pitch: number): void;
  setKey(key: string, down: boolean): void;
  /** The canvas as a PNG data URL, or null when there is none. */
  capture(): string | null;
};

let camera: CameraApi | null = null;

/** World registers its camera while it is mounted, and clears it on unmount. */
export function registerCamera(api: CameraApi | null): void {
  camera = api;
}

const DEG = Math.PI / 180;

/** Yaw and pitch (radians) that look from `from` toward `to`, both in world space. */
export function lookAngles(from: Vec3, to: Vec3): { yaw: number; pitch: number } {
  const dx = to[0] - from[0];
  const dy = to[1] - from[1];
  const dz = to[2] - from[2];
  const length = Math.hypot(dx, dy, dz);
  if (length < 1e-9) return { yaw: 0, pitch: 0 };
  // Inverse of the camera's `look = (-sin yaw cos pitch, sin pitch, -cos yaw cos pitch)`.
  return { yaw: Math.atan2(-dx, -dz), pitch: Math.asin(dy / length) };
}

/** A flight through waypoints in world space, advanced by `stepFlight`. */
export type Flight = {
  readonly points: readonly Vec3[];
  readonly speed: number;
  /** Index of the waypoint being flown toward. */
  next: number;
  position: Vec3;
};

/**
 * Advance `flight` by `dt` seconds and return the new position, the look angles
 * toward the waypoint being flown to, and whether the last waypoint was reached.
 * Pure apart from mutating `flight`.
 */
export function stepFlight(
  flight: Flight,
  dt: number,
): { position: Vec3; yaw: number; pitch: number; done: boolean } {
  let budget = Math.max(0, flight.speed * dt);
  let position = flight.position;
  while (flight.next < flight.points.length) {
    const target = flight.points[flight.next]!;
    const dx = target[0] - position[0];
    const dy = target[1] - position[1];
    const dz = target[2] - position[2];
    const distance = Math.hypot(dx, dy, dz);
    if (distance <= budget) {
      position = target;
      budget -= distance;
      flight.next += 1;
      continue;
    }
    const k = budget / distance;
    position = [position[0] + dx * k, position[1] + dy * k, position[2] + dz * k];
    break;
  }
  flight.position = position;
  const done = flight.next >= flight.points.length;
  const aim = flight.points[Math.min(flight.next, flight.points.length - 1)] ?? position;
  const { yaw, pitch } = lookAngles(position, aim);
  return { position, yaw, pitch, done };
}

let flight: Flight | null = null;
let flightFrame = 0;
const held = new Map<string, ReturnType<typeof setTimeout>>();

function stopFlight(): void {
  flight = null;
  // Absent outside a browser (the unit tests).
  globalThis.cancelAnimationFrame?.(flightFrame);
}

function flyFrame(last: number): void {
  flightFrame = requestAnimationFrame((now) => {
    const active = flight;
    if (!active || !camera) return;
    const step = stepFlight(active, Math.min((now - last) / 1000, 0.1));
    const pose = camera.pose();
    // Keep the heading when the aim is the point just reached.
    const aimed = step.done ? { yaw: pose.yaw, pitch: pose.pitch } : step;
    camera.set(step.position, aimed.yaw, aimed.pitch);
    if (step.done) flight = null;
    else flyFrame(now);
  });
}

function vec3(value: unknown): Vec3 | null {
  if (!Array.isArray(value) || value.length !== 3) return null;
  const [x, y, z] = value as unknown[];
  return [x, y, z].every((n) => typeof n === "number" && Number.isFinite(n))
    ? [x as number, y as number, z as number]
    : null;
}

let wantScreenshot = false;
let lastCommand = 0;

/** Run one command from the backend. Throws on a command that cannot run. */
export function runCommand(command: { id?: number; type: string } & Record<string, unknown>): void {
  switch (command.type) {
    case "camera": {
      if (!camera) throw new Error("World is not open");
      const pose = camera.pose();
      const position = command.gta ? gtaToWorld(requireVec(command.gta, "gta")) : pose.position;
      let yaw = typeof command.yawDeg === "number" ? command.yawDeg * DEG : pose.yaw;
      let pitch = typeof command.pitchDeg === "number" ? command.pitchDeg * DEG : pose.pitch;
      if (command.lookAtGta) {
        ({ yaw, pitch } = lookAngles(position, gtaToWorld(requireVec(command.lookAtGta, "lookAtGta"))));
      }
      stopFlight();
      camera.set(position, yaw, pitch);
      return;
    }
    case "fly": {
      if (!camera) throw new Error("World is not open");
      const raw = command.path;
      if (!Array.isArray(raw) || raw.length === 0) throw new Error("fly needs a non-empty path");
      const points = raw.map((p, i) => gtaToWorld(requireVec(p, `path[${i}]`)));
      const speed = typeof command.speed === "number" && command.speed > 0 ? command.speed : 300;
      stopFlight();
      flight = { points, speed, next: 0, position: camera.pose().position };
      flyFrame(performance.now());
      return;
    }
    case "hold": {
      if (!camera) throw new Error("World is not open");
      const key = String(command.key);
      const ms = typeof command.ms === "number" ? Math.min(Math.max(command.ms, 0), 60_000) : 1000;
      camera.setKey(key, true);
      clearTimeout(held.get(key));
      held.set(
        key,
        setTimeout(() => {
          camera?.setKey(key, false);
          held.delete(key);
        }, ms),
      );
      return;
    }
    case "stop": {
      stopFlight();
      for (const [key, timer] of held) {
        clearTimeout(timer);
        camera?.setKey(key, false);
      }
      held.clear();
      return;
    }
    case "screenshot":
      wantScreenshot = true;
      return;
    case "resetPerf":
      perf.reset();
      void perf.backendReset();
      return;
    case "click": {
      const text = String(command.text ?? "");
      if (!text) throw new Error("click needs a text");
      const nth = typeof command.nth === "number" ? command.nth : 0;
      const target = clickables().filter((c) => c.label.includes(text.toLowerCase()))[nth];
      if (!target) throw new Error(`nothing visible to click matches "${text}"`);
      target.element.click();
      return;
    }
    case "key": {
      const key = String(command.key ?? "");
      if (!key) throw new Error("key needs a key");
      for (const type of ["keydown", "keyup"]) {
        window.dispatchEvent(new KeyboardEvent(type, { key, code: key, bubbles: true }));
      }
      return;
    }
    default:
      throw new Error(`unknown command type "${command.type}"`);
  }
}

const CLICKABLE = "button, a[href], [role=button], [role=tab], label, input[type=checkbox]";

/** The visible things a `click` command can hit, in document order. */
function clickables(): Array<{ element: HTMLElement; label: string }> {
  const out: Array<{ element: HTMLElement; label: string }> = [];
  for (const element of document.querySelectorAll<HTMLElement>(CLICKABLE)) {
    const box = element.getBoundingClientRect();
    if (box.width === 0 || box.height === 0) continue;
    const label = (
      element.getAttribute("aria-label") ??
      (element.textContent || (element as HTMLInputElement).value || "")
    )
      .replace(/\s+/g, " ")
      .trim()
      .toLowerCase();
    if (label) out.push({ element, label });
  }
  return out;
}

function requireVec(value: unknown, name: string): Vec3 {
  const parsed = vec3(value);
  if (!parsed) throw new Error(`${name} must be [x, y, z] numbers`);
  return parsed;
}

// ------------------------------------------------------------ console capture

const LINE_MAX = 2000;

type OutLog = { level: string; text: string };
const pending: OutLog[] = [];
/** Lines kept while the backend is unreachable, so a stall cannot grow memory. */
const PENDING_MAX = 2000;

/** One console argument as text: strings as they are, errors with their stack. */
export function formatArg(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack ?? `${value.name}: ${value.message}`;
  if (value === undefined) return "undefined";
  try {
    const text = JSON.stringify(value);
    return text === undefined ? String(value) : text;
  } catch {
    return String(value);
  }
}

/** The line a call to `console.<level>(...args)` becomes. */
export function formatLine(args: readonly unknown[]): string {
  const text = args.map(formatArg).join(" ");
  return text.length > LINE_MAX ? `${text.slice(0, LINE_MAX)}... [${text.length - LINE_MAX} chars cut]` : text;
}

function enqueue(level: string, text: string): void {
  if (pending.length >= PENDING_MAX) pending.shift();
  pending.push({ level, text });
}

function captureConsole(): void {
  for (const level of ["log", "info", "warn", "error", "debug"] as const) {
    const original = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      original(...args);
      enqueue(level, formatLine(args));
    };
  }
  window.addEventListener("error", (event) => {
    enqueue("error", `uncaught: ${event.message} (${event.filename}:${event.lineno}:${event.colno})`);
  });
  window.addEventListener("unhandledrejection", (event) => {
    enqueue("error", `unhandled rejection: ${formatArg(event.reason)}`);
  });
}

// ------------------------------------------------------------------- state

const STATE_EVERY_MS = 1000;
const SYNC_EVERY_MS = 250;

function snapshot(): Record<string, unknown> {
  let cameraState: Record<string, unknown> | null = null;
  if (camera) {
    const pose = camera.pose();
    cameraState = {
      gta: worldToGta([pose.position[0], pose.position[1], pose.position[2]]),
      world: pose.position,
      yawDeg: pose.yaw / DEG,
      pitchDeg: pose.pitch / DEG,
    };
  }
  return {
    at: Date.now(),
    worldOpen: camera !== null,
    camera: cameraState,
    flying: flight !== null,
    lastCommand,
    fps: perf.fps(),
    gauges: perf.gauges(),
    spans: perf.report(),
    viewport: { width: window.innerWidth, height: window.innerHeight },
    // What a `click` can hit right now, so a command can be written from this.
    clickable: clickables()
      .slice(0, 80)
      .map((c) => c.label.slice(0, 60)),
  };
}

let started = false;

/** Start the bridge if the backend has it on. Safe to call more than once. */
export async function startDebugBridge(): Promise<void> {
  if (started) return;
  started = true;
  let info: { port: number } | null = null;
  try {
    info = await invoke<{ port: number } | null>("debug_bridge_info");
  } catch {
    // Not running inside Tauri (a browser harness) or an old backend: no bridge.
    return;
  }
  if (!info) return;
  captureConsole();
  console.info(`debug bridge on, port ${info.port}`);

  let lastState = 0;
  let busy = false;
  const sync = async (): Promise<void> => {
    if (busy) return;
    busy = true;
    const logs = pending.splice(0, pending.length);
    const now = Date.now();
    const state = now - lastState >= STATE_EVERY_MS ? snapshot() : null;
    if (state) lastState = now;
    let screenshot: string | null = null;
    if (wantScreenshot) {
      wantScreenshot = false;
      // Empty tells the waiting HTTP request that there was no canvas.
      screenshot = camera?.capture() ?? "";
    }
    try {
      const commands = await invoke<Array<{ id: number; type: string } & Record<string, unknown>>>(
        "debug_bridge_sync",
        { logs, state, screenshot },
      );
      for (const command of commands) {
        lastCommand = command.id;
        try {
          runCommand(command);
        } catch (cause) {
          console.warn(`bridge command ${command.id} (${command.type}) failed: ${String(cause)}`);
        }
      }
    } catch {
      // The backend is gone or busy; put the lines back for the next round.
      pending.unshift(...logs);
      if (pending.length > PENDING_MAX) pending.splice(0, pending.length - PENDING_MAX);
    } finally {
      busy = false;
    }
  };
  window.setInterval(() => void sync(), SYNC_EVERY_MS);
}
