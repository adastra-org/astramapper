/* oxlint-disable no-await-in-loop -- steps and polls run one after another by design */
/**
 * Runs a scenario against a bridge. It knows nothing about HTTP: it talks to a
 * `Bridge` and a `Clock`, so the unit tests drive it with a fake app and virtual
 * time.
 */
import { coverage, decodePng, difference, pixel, type Image } from "./png";
import { resolvePlace } from "./places";
import { CLEAR_COLOR, type Assertion, type Scenario, type Step } from "./scenario";
import { compare, stat, type Point } from "./stats";
import { realClock, type AppSnapshot, type Bridge, type Clock, type LogLine } from "./types";

export type StepResult = { index: number; label: string; ok: boolean; ms: number; error?: string };

export type AssertionResult = {
  label: string;
  ok: boolean;
  /** The measured number; null when it could not be measured. */
  actual: number | null;
  op: string;
  expected: number;
  /** Why it could not be measured, or what was looked at. */
  detail?: string;
};

export type SeriesReport = {
  sample: string;
  key: string;
  points: number;
  first: number;
  last: number;
  min: number;
  max: number;
  mean: number;
  slopePerMin: number;
  halfRatio: number | null;
  /** The raw points, `[ms since the run began, value]`. */
  data: Array<[number, number]>;
};

export type Report = {
  scenario: string;
  description: string;
  startedAt: string;
  durationMs: number;
  /** True when every step ran and every assertion held. */
  ok: boolean;
  steps: StepResult[];
  assertions: AssertionResult[];
  series: SeriesReport[];
  screenshots: string[];
  console: { errors: number; warnings: number; sample: string[] };
};

export type RunOptions = {
  /** Where screenshots go; their paths in the report are relative to it. */
  outDir: string;
  clock?: Clock;
  /** Writes a file, creating its folder. Injected so tests need no disk. */
  write: (path: string, data: Uint8Array) => Promise<void>;
  /** Progress lines. */
  log?: (line: string) => void;
  /** How often conditions are polled. */
  pollMs?: number;
};

function gaugeValue(s: AppSnapshot, name: string): number | undefined {
  return s.gauges.find((g) => g.name === name)?.value;
}

const DEFAULT_TIMEOUT = 60_000;
const COMMAND_TIMEOUT = 15_000;

class StepError extends Error {}

export function describeStep(step: Step): string {
  switch (step.do) {
    case "openWorld":
      return "open World";
    case "go":
      return `go to ${typeof step.to === "string" ? step.to : `[${step.to.join(", ")}]`}`;
    case "fly":
      return `fly ${step.path.map((p) => (typeof p === "string" ? p : `[${p.join(",")}]`)).join(" > ")}${
        step.speed ? ` at ${step.speed} m/s` : ""
      }`;
    case "stop":
      return "stop";
    case "wait":
      return `wait ${step.ms} ms`;
    case "waitFor":
      return `wait for ${step.gauge} ${step.op} ${step.value}`;
    case "resetPerf":
      return "reset perf";
    case "sample":
      return `sample "${step.name}" for ${step.forMs} ms${step.untilIdle ? " (or until the flight ends)" : ""}`;
    case "shot":
      return `screenshot ${step.name}`;
    case "cmd":
      return `command ${step.command.type}`;
    case "assert":
      return `assert ${assertionLabel(step)}`;
  }
}

export function assertionLabel(a: Assertion): string {
  if (a.label) return a.label;
  switch (a.kind) {
    case "gauge":
      return `${a.gauge} ${a.op} ${a.value}`;
    case "series":
      return `${a.key} ${a.stat} (${a.sample}) ${a.op} ${typeof a.value === "number" ? a.value : `${a.value.factor ?? 1} x ${a.value.key}`}`;
    case "span":
      return `${a.span} ${a.stat} ${a.op} ${a.value}`;
    case "logs":
      return `console ${a.level}s ${a.op} ${a.value}`;
    case "image":
      return `${a.shot} coverage [${a.region.join(", ")}] ${a.op} ${a.value}`;
    case "imageDiff":
      return `${a.shot} vs ${a.against} difference [${a.region.join(", ")}] ${a.op} ${a.value}`;
  }
}

const SPAN_FIELD = {
  p50: "p50Ms",
  p95: "p95Ms",
  max: "maxMs",
  mean: "meanMs",
  calls: "calls",
} as const;

export async function runScenario(
  scenarioDef: Scenario,
  bridge: Bridge,
  options: RunOptions,
): Promise<Report> {
  const clock = options.clock ?? realClock;
  const pollMs = options.pollMs ?? 500;
  const log = options.log ?? (() => {});
  const began = clock.now();
  const startedAt = new Date(began).toISOString();

  const lines: LogLine[] = [];
  let cursor = 0;
  const series = new Map<string, Map<string, Point[]>>();
  const shots = new Map<string, Image>();
  const screenshots: string[] = [];
  let lastBackend: AppSnapshot["spans"] = [];

  async function pullLogs(): Promise<void> {
    for (;;) {
      const page = await bridge.logs(cursor, 1000);
      lines.push(...page.logs);
      cursor = page.next;
      if (page.logs.length < 1000) return;
    }
  }

  async function snap(): Promise<AppSnapshot | null> {
    const state = await bridge.state();
    lastBackend = state.backend;
    return state.state;
  }

  async function waitUntil<T>(
    what: string,
    timeoutMs: number,
    check: () => Promise<T | null | undefined | false>,
  ): Promise<T> {
    const deadline = clock.now() + timeoutMs;
    for (;;) {
      const got = await check();
      if (got) return got;
      if (clock.now() >= deadline) throw new StepError(`timed out after ${timeoutMs} ms waiting for ${what}`);
      await clock.sleep(pollMs);
    }
  }

  /** Queue a command and wait until the WebView has run it; throws if it failed there. */
  async function send(command: { type: string } & Record<string, unknown>): Promise<AppSnapshot> {
    const id = await bridge.cmd(command);
    const snapshot = await waitUntil(`command ${id} (${command.type}) to run`, COMMAND_TIMEOUT, async () => {
      const s = await snap();
      return s && s.lastCommand >= id ? s : null;
    });
    await pullLogs();
    const failed = lines.find((l) => l.seq > runStartSeq && l.text.startsWith(`bridge command ${id} (`) && l.text.includes("failed"));
    if (failed) throw new StepError(failed.text);
    return snapshot;
  }

  async function clickExact(label: string): Promise<void> {
    const s = await snap();
    if (!s) throw new StepError("the WebView has not reported yet");
    const wanted = label.toLowerCase();
    const matching = s.clickable.filter((c) => c.includes(wanted));
    const nth = matching.findIndex((c) => c === wanted);
    if (matching.length === 0) throw new StepError(`nothing to click that matches "${label}"`);
    await send({ type: "click", text: wanted, nth: Math.max(nth, 0) });
  }

  function findSpan(s: AppSnapshot | null, name: string): AppSnapshot["spans"][number] | undefined {
    return s?.spans.find((r) => r.name === name) ?? lastBackend.find((r) => r.name === name);
  }

  function gameOpened(): boolean {
    return (
      lastBackend.some((r) => r.name === "open.scan_archives" && r.calls > 0) ||
      lines.some((l) => l.text.includes("open.scan_archives"))
    );
  }

  async function openWorld(timeoutMs: number): Promise<void> {
    const deadline = clock.now() + timeoutMs;
    const first = await waitUntil("the WebView to report (is the app window up?)", timeoutMs, snap);
    if (first.worldOpen) return;
    // The Explorer restores the last game folder when it mounts. World needs that
    // scan done, so go to Explorer, wait for the backend to report it, then World.
    await pullLogs();
    if (first.clickable.includes("explorer")) await clickExact("explorer");
    const explorerAt = clock.now();
    let triedEarly = false;
    for (;;) {
      await snap();
      await pullLogs();
      const now = clock.now();
      if (gameOpened()) break;
      if (!triedEarly && now - explorerAt > 20_000) {
        // The span may have been cleared by an earlier reset: try World anyway.
        triedEarly = true;
        try {
          await clickExact("world");
          await waitUntil("World to open", 10_000, async () => (await snap())?.worldOpen);
          return;
        } catch {
          if (first.clickable.includes("explorer")) await clickExact("explorer");
        }
      }
      if (now >= deadline) {
        throw new StepError(
          "no backend game activity (open.scan_archives) after opening Explorer: does the app have a saved game folder? Pick one by hand once, then rerun",
        );
      }
      await clock.sleep(pollMs);
    }
    await clickExact("world");
    await waitUntil("World to open", Math.max(deadline - clock.now(), 10_000), async () => (await snap())?.worldOpen);
  }

  function seriesFor(sample: string): Map<string, Point[]> {
    let bySample = series.get(sample);
    if (!bySample) {
      bySample = new Map();
      series.set(sample, bySample);
    }
    return bySample;
  }

  async function runSample(step: Extract<Step, { do: "sample" }>): Promise<void> {
    const data = seriesFor(step.name);
    const every = Math.max(step.everyMs ?? 1000, 250);
    const start = clock.now();
    let lastAt = -1;
    for (;;) {
      const s = await snap();
      if (s && s.at !== lastAt) {
        lastAt = s.at;
        const t = clock.now() - began;
        const push = (key: string, v: number | null | undefined): void => {
          if (v === null || v === undefined || Number.isNaN(v)) return;
          let points = data.get(key);
          if (!points) {
            points = [];
            data.set(key, points);
          }
          points.push({ t, v });
        };
        push("fps", s.fps);
        for (const name of step.gauges ?? []) push(name, gaugeValue(s, name));
        for (const spec of step.spans ?? []) {
          const row = findSpan(s, spec.name);
          for (const which of spec.stats ?? ["p50", "p95", "max"]) {
            push(`span:${spec.name}.${which}`, row ? (row[SPAN_FIELD[which]] as number) : undefined);
          }
        }
        if (step.untilIdle && !s.flying && clock.now() - start >= every) return;
      }
      if (clock.now() - start >= step.forMs) return;
      await clock.sleep(every);
    }
  }

  async function runAssertion(a: Assertion): Promise<AssertionResult> {
    const label = assertionLabel(a);
    let expected = 0;
    let limitDetail: string | undefined;
    if (a.kind === "series" && typeof a.value === "object") {
      const other = series.get(a.sample)?.get(a.value.key);
      const which = a.value.stat ?? "max";
      expected = other && other.length > 0 ? stat(other, which) * (a.value.factor ?? 1) : Number.NaN;
      limitDetail = `limit is ${a.value.factor ?? 1} x ${a.value.key} ${which}`;
    } else {
      expected = (a as { value: number }).value;
    }
    const result = (actual: number | null, detail?: string): AssertionResult => ({
      label,
      ok: actual !== null && compare(actual, a.op, expected),
      actual,
      op: a.op,
      expected,
      ...(detail || limitDetail ? { detail: detail ?? limitDetail } : {}),
    });
    switch (a.kind) {
      case "gauge": {
        const s = await snap();
        const v = s ? gaugeValue(s, a.gauge) : undefined;
        return v === undefined ? result(null, `gauge "${a.gauge}" is not reported`) : result(v);
      }
      case "series": {
        const points = series.get(a.sample)?.get(a.key);
        if (!points || points.length === 0) {
          return result(null, `no samples for "${a.key}" in "${a.sample}"`);
        }
        const v = stat(points, a.stat);
        return Number.isNaN(v) ? result(null, `${a.stat} needs more samples (${points.length})`) : result(v);
      }
      case "span": {
        const row = findSpan(await snap(), a.span);
        return row ? result(row[SPAN_FIELD[a.stat]] as number) : result(null, `span "${a.span}" is not reported`);
      }
      case "logs": {
        await pullLogs();
        const ignore = (a.ignore ?? []).map((source) => new RegExp(source));
        const count = lines.filter(
          (l) => l.seq > runStartSeq && l.level === a.level && !ignore.some((re) => re.test(l.text)),
        ).length;
        return result(count);
      }
      case "image": {
        const image = shots.get(a.shot);
        if (!image) return result(null, `no screenshot named "${a.shot}" was taken`);
        const bg = a.background === "corner" ? pixel(image, 0, 0) : (a.background ?? CLEAR_COLOR);
        return result(coverage(image, a.region, bg, a.tolerance));
      }
      case "imageDiff": {
        const first = shots.get(a.shot);
        const second = shots.get(a.against);
        if (!first) return result(null, `no screenshot named "${a.shot}" was taken`);
        if (!second) return result(null, `no screenshot named "${a.against}" was taken`);
        const measured = difference(first, second, a.region, a.tolerance);
        return result(
          measured,
          Number.isNaN(measured) ? "the screenshots differ in size" : undefined,
        );
      }
    }
  }

  const steps: StepResult[] = [];
  const assertions: AssertionResult[] = [];
  let aborted = false;

  await pullLogs();
  /** Console lines at or before this belong to earlier runs. */
  const runStartSeq = cursor;

  for (const [index, step] of scenarioDef.steps.entries()) {
    const label = describeStep(step);
    if (aborted) {
      steps.push({ index, label, ok: false, ms: 0, error: "skipped: an earlier step failed" });
      continue;
    }
    log(`[${index + 1}/${scenarioDef.steps.length}] ${label}`);
    const t0 = clock.now();
    try {
      await runStep(step);
      steps.push({ index, label, ok: true, ms: clock.now() - t0 });
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      steps.push({ index, label, ok: false, ms: clock.now() - t0, error: message });
      log(`  failed: ${message}`);
      aborted = true;
    }
  }

  async function runStep(step: Step): Promise<void> {
    switch (step.do) {
      case "openWorld":
        return openWorld(step.timeoutMs ?? 180_000);
      case "go": {
        const command: Record<string, unknown> = { type: "camera", gta: resolvePlace(step.to) };
        if (step.lookAt !== undefined) command.lookAtGta = resolvePlace(step.lookAt);
        if (step.yawDeg !== undefined) command.yawDeg = step.yawDeg;
        if (step.pitchDeg !== undefined) command.pitchDeg = step.pitchDeg;
        await send(command as { type: string });
        return;
      }
      case "fly": {
        const path = step.path.map(resolvePlace);
        const speed = step.speed ?? 300;
        await send({ type: "fly", path, speed });
        if (step.wait === false) return;
        let length = 0;
        for (let i = 1; i < path.length; i++) {
          const a = path[i - 1]!;
          const b = path[i]!;
          length += Math.hypot(b[0] - a[0], b[1] - a[1], b[2] - a[2]);
        }
        const budget = step.timeoutMs ?? (length / speed) * 1000 * 2 + 30_000;
        await waitUntil("the flight to end", budget, async () => {
          const s = await snap();
          return s && !s.flying;
        });
        return;
      }
      case "stop":
        await send({ type: "stop" });
        return;
      case "wait":
        await clock.sleep(step.ms);
        return;
      case "waitFor":
        await waitUntil(`${step.gauge} ${step.op} ${step.value}`, step.timeoutMs ?? DEFAULT_TIMEOUT, async () => {
          const s = await snap();
          const v = s ? gaugeValue(s, step.gauge) : undefined;
          return v !== undefined && compare(v, step.op, step.value);
        });
        return;
      case "resetPerf":
        await send({ type: "resetPerf" });
        return;
      case "sample":
        return runSample(step);
      case "shot": {
        const bytes = await bridge.screenshot();
        const file = `${step.name}.png`;
        await options.write(`${options.outDir}/${file}`, bytes);
        screenshots.push(file);
        try {
          shots.set(step.name, decodePng(bytes));
        } catch {
          // Not decodable: image assertions on it report "no screenshot".
        }
        return;
      }
      case "cmd":
        await send(step.command);
        return;
      case "assert": {
        const { do: _do, ...assertion } = step;
        assertions.push(await runAssertion(assertion as Assertion));
        return;
      }
    }
  }

  await pullLogs();
  const mine = lines.filter((l) => l.seq > runStartSeq);
  const errors = mine.filter((l) => l.level === "error");
  const warnings = mine.filter((l) => l.level === "warn");
  const reports: SeriesReport[] = [];
  for (const [sample, keys] of series) {
    for (const [key, points] of keys) {
      const ratio = stat(points, "halfRatio");
      reports.push({
        sample,
        key,
        points: points.length,
        first: stat(points, "first"),
        last: stat(points, "last"),
        min: stat(points, "min"),
        max: stat(points, "max"),
        mean: stat(points, "mean"),
        slopePerMin: stat(points, "slopePerMin"),
        halfRatio: Number.isFinite(ratio) ? ratio : null,
        data: points.map((p): [number, number] => [p.t, p.v]),
      });
    }
  }
  return {
    scenario: scenarioDef.name,
    description: scenarioDef.description,
    startedAt,
    durationMs: clock.now() - began,
    ok: steps.every((s) => s.ok) && assertions.every((a) => a.ok),
    steps,
    assertions,
    series: reports,
    screenshots,
    console: {
      errors: errors.length,
      warnings: warnings.length,
      sample: [...errors, ...warnings].slice(0, 10).map((l) => `${l.level}: ${l.text.slice(0, 200)}`),
    },
  };
}

