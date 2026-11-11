/**
 * The scenario format: a name and a list of steps. A scenario is a plain object
 * (a TypeScript module's default export, or a JSON file), so it can be written
 * by hand, generated, or read back from a report.
 */
import type { Place } from "./places";
import type { Region } from "./png";
import type { Op, Stat } from "./stats";

export type SpanStat = "p50" | "p95" | "max" | "mean" | "calls";

/** A series asserted on after a `sample` step. */
export type SeriesKey = string;

export type Assertion = { label?: string } & (
  | {
      /** A gauge's value right now. */
      kind: "gauge";
      gauge: string;
      op: Op;
      value: number;
    }
  | {
      /** A statistic of a series a `sample` step recorded. */
      kind: "series";
      sample: string;
      /** A gauge name, or `span:<name>.<p50|p95|max|mean>`, or `fps`. */
      key: SeriesKey;
      stat: Stat;
      op: Op;
      /** A number, or another series of the same sample to compare against. */
      value: number | { key: SeriesKey; stat?: Stat; factor?: number };
    }
  | {
      /** One statistic of a span (the WebView's, or else the backend's), as of now. */
      kind: "span";
      span: string;
      stat: SpanStat;
      op: Op;
      value: number;
    }
  | {
      /** How many console lines of `level` appeared since the run began. */
      kind: "logs";
      level: "error" | "warn";
      op: Op;
      value: number;
      /** Regex sources; matching lines do not count. */
      ignore?: string[];
    }
  | {
      /**
       * The fraction of a screenshot's `region` (x0, y0, x1, y1 as fractions of
       * the image) that differs from the background colour.
       */
      kind: "image";
      shot: string;
      region: Region;
      op: Op;
      value: number;
      /** `[r, g, b]`, or "corner" to take the top-left pixel. Defaults to the scene's clear colour. */
      background?: readonly [number, number, number] | "corner";
      /** How far (per channel) a pixel may be from the background and still count as background. Default 6. */
      tolerance?: number;
    }
);

export type Step =
  | {
      /** Get to World from wherever the app is (see `docs/code-mode.md` for the Explorer quirk). */
      do: "openWorld";
      timeoutMs?: number;
    }
  | {
      /** Teleport the camera. */
      do: "go";
      to: Place;
      lookAt?: Place;
      yawDeg?: number;
      pitchDeg?: number;
    }
  | {
      /** Fly through places at `speed` m/s; waits for the end unless `wait` is false. */
      do: "fly";
      path: Place[];
      speed?: number;
      wait?: boolean;
      timeoutMs?: number;
    }
  | { do: "stop" }
  | { do: "wait"; ms: number }
  | {
      /** Wait until a gauge satisfies a condition. */
      do: "waitFor";
      gauge: string;
      op: Op;
      value: number;
      timeoutMs?: number;
    }
  | { do: "resetPerf" }
  | {
      /**
       * Read gauges and span statistics every `everyMs` for `forMs`, or until
       * the flight ends with `untilIdle`. One point per fresh snapshot (the app
       * sends one a second).
       */
      do: "sample";
      name: string;
      forMs: number;
      everyMs?: number;
      untilIdle?: boolean;
      gauges?: string[];
      spans?: Array<{ name: string; stats?: Array<"p50" | "p95" | "max" | "mean"> }>;
    }
  | { do: "shot"; name: string }
  | {
      /** Any bridge command, as `docs/debug-bridge.md` lists them. */
      do: "cmd";
      command: { type: string } & Record<string, unknown>;
    }
  | ({ do: "assert" } & Assertion);

export type Scenario = {
  name: string;
  description: string;
  steps: Step[];
};

/** Identity, for type checking a scenario written as a module. */
export function scenario(value: Scenario): Scenario {
  return value;
}

/** The default scene clear colour (`World.tsx`: `0x10141c`). */
export const CLEAR_COLOR = [16, 20, 28] as const;
