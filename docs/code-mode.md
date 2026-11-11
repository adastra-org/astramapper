# Code Mode

Check the real app without clicking: a scenario runner on top of the
[debug bridge](debug-bridge.md). A coding agent, or a person, runs one command
and gets a pass/fail report with the numbers behind it. Added in v0.3.4.

```bash
bun run tauri dev                       # the app, in its own terminal (the tool starts nothing)
bun scripts/bridge.ts list              # shipped scenarios and named places
bun scripts/bridge.ts run smoke         # exit 0 pass, 1 an assertion or step failed, 2 cannot run
```

It reads `astramapper-debug-bridge.json` in the temp directory for the port and
token (`ASTRAMAPPER_BRIDGE_INFO` overrides the path). With no app running it
says so and exits 2; a stale file (the pid is gone) is reported as stale.

**One app at a time.** The bridge info file and the dev ports are single
instance, and a scenario moves the camera and resets the F3 spans. Do not run
one while someone else is measuring in the same app.

## Commands

| Command | |
|---|---|
| `run <scenario> [--out DIR]` | a shipped scenario by name, or a `.ts` / `.json` file; writes `report.json`, `report.md` and the screenshots to `DIR` (default `out/code-mode/<name>-<time>/`) and prints the markdown |
| `state [--gauges\|--spans\|--clickable]` | F3's numbers as JSON; the flags narrow it |
| `logs [--since N] [--limit N] [--follow]` | the console and backend lines |
| `shot [file.png]` | a screenshot of the World canvas |
| `cmd <type> [key=value ...]` | one bridge command, e.g. `cmd camera gta=[195,-935,60] yawDeg=90`, `cmd click text=world`; a value that parses as JSON is JSON. `cmd '{"type":"stop"}'` also works |
| `list` | shipped scenarios and places |

## A scenario

A plain object (the default export of a TypeScript module, or a JSON file): a
name, a description and `steps`. Types in
[`scripts/code-mode/scenario.ts`](../scripts/code-mode/scenario.ts).

```ts
import { scenario } from "../scenario";

export default scenario({
  name: "my-check",
  description: "What this proves.",
  steps: [
    { do: "openWorld" },
    { do: "go", to: "legion-square", lookAt: "ls-airport" },
    { do: "waitFor", gauge: "cells held", op: ">", value: 0 },
    { do: "resetPerf" },
    { do: "fly", path: ["ls-airport", "vinewood-sign"], speed: 120, wait: false },
    { do: "sample", name: "flight", forMs: 120_000, untilIdle: true, gauges: ["cells held"] },
    { do: "assert", kind: "series", sample: "flight", key: "cells held", stat: "max", op: "<=", value: 200 },
  ],
});
```

### Steps

| `do` | |
|---|---|
| `openWorld` | get to World from wherever the app is (see below) |
| `go` | teleport: `to`, optional `lookAt`, `yawDeg`, `pitchDeg`. Places are names from `list` or `[x, y, z]` in game coordinates |
| `fly` | fly through `path` at `speed` m/s. Waits for the end unless `wait: false` (then pair it with `sample ... untilIdle`) |
| `stop` | cancel a flight, release keys |
| `wait` | `ms` |
| `waitFor` | wait until a gauge satisfies `op value` (`timeoutMs`, default 60 s) |
| `resetPerf` | clear the spans on both sides, so a window starts clean |
| `sample` | read gauges and span statistics for `forMs` (or until the flight ends with `untilIdle`); one point per fresh snapshot, which the app sends once a second |
| `shot` | save the canvas as `<name>.png` in the output folder |
| `cmd` | any bridge command, raw |
| `assert` | see below |

A step that fails (a command the app could not run, a timeout) stops the run;
the later steps are reported as skipped. A failed assertion does not stop it.

### Assertions

`{ do: "assert", kind, op, value, label? }` with `op` one of `<`, `<=`, `>`,
`>=`, `==`, `!=`. A number that cannot be measured (gauge not reported, no
samples) fails, with the reason in the report; it never passes by default.

| `kind` | Measures |
|---|---|
| `gauge` | a gauge's value right now (`gauge`) |
| `series` | a statistic of a series a `sample` recorded: `sample`, `key` (a gauge name, `fps`, or `span:<name>.<p50\|p95\|max\|mean>`), `stat`. `value` may be `{ key, stat?, factor? }` to compare with another series, e.g. the model cache against its budget |
| `span` | `span`, `stat` (`p50`, `p95`, `max`, `mean`, `calls`), from the WebView's spans, else the backend's |
| `logs` | the number of console lines of `level` (`error`, `warn`) since the run began, minus any matching the `ignore` regexes |
| `imageDiff` | the fraction of a screenshot's `region` that differs from another screenshot (`against`) of the same size by more than `tolerance` per channel (default 30). The same pose twice: 0 is the same picture |
| `image` | the fraction of a screenshot's `region` (`[x0, y0, x1, y1]` as fractions) that differs from the background colour (the scene's clear colour by default, or `"corner"`), with a per-channel `tolerance` (default 6). The scene draws a grid a little lighter than the clear colour, so ground checks need a larger tolerance |

Series statistics: `last`, `first`, `min`, `max`, `mean`, `slopePerMin` (least
squares, units per minute) and `halfRatio` (the second half's maximum over the
first half's: near 1 means it levelled off, well above 1 means it is still
climbing, which is how "does not grow monotonically" is checked without a magic
ceiling).

## Opening World

World needs the game folder scanned, and Explorer is what restores it (from the
saved last folder) when it mounts, so clicking World straight away opens a World
with nothing to load. `openWorld` clicks Explorer, waits for the backend to
report the scan (`open.scan_archives` in its spans or the console), then clicks
World and waits for `worldOpen`. If no game folder is saved it fails after the
timeout and says to pick one by hand once. If the scan's span was cleared by an
earlier `resetPerf` it tries World after 20 s anyway.

## Report

`report.json` has every step with its time and error, every assertion with the
measured number and the limit, every series with its statistics and raw points,
the screenshot names and a console summary. `report.md` is the short version for
a PR:

```
# streaming-bounded: PASS
| | Assertion | Measured | Limit | Note |
| pass | cells held max (flight) <= 200 | 81 | <= 200 | |
```

## Shipped scenarios

| Name | Checks |
|---|---|
| `smoke` | World opens, frames render, cells are held, the console has no errors |
| `streaming-bounded` | flying a loop over the city at 120 m/s, cells held and placements held stay under a ceiling, the model cache stays within 10% of its budget, no console errors |
| `terrain-distance` | looking level toward Sandy Shores and Paleto Bay from the Maze Bank Tower, terrain is drawn far beyond the streamed cells (the band from above the horizon to just under it has terrain-coloured pixels, not only sky and the grid helper); screenshots kept |
| `terrain-revisit` | the ground over Sandy Shores at a fixed pose, then a flight to the LS airport and back at 120 m/s: the distant ground band is the same picture on return (`imageDiff` under 8%) and still drawn (coverage >= 50% at both visits); the view from the far end looking back is kept (v0.3.9) |
| `heap-flight` | touring the map at 150 m/s, the JS heap stays under a ceiling and its second half is within 20% of the first (#23) |

The ceilings come from what the first flight measured (`debug-bridge.md`) and
are meant to be tightened; they are numbers in the scenario file.

## How it is built

`scripts/code-mode/`, pure TypeScript, no dependency beyond Bun:

- `client.ts`: finds the bridge and speaks HTTP (`Bridge` interface in `types.ts`).
- `runner.ts`: the step logic. It sees only a `Bridge` and a `Clock`.
- `stats.ts`, `png.ts`: series statistics and a small PNG decoder for `image`.
- `scenario.ts`, `scenarios/`: the format and the shipped scenarios.
- `report.ts`, `cli.ts`: markdown output and the command line.
- `fake-app.ts`: a fake app with virtual time that the tests drive the runner
  against (Explorer restoring the game folder, snapshots once a second, commands
  applied on the next sync, `bridge command N failed` lines). `bun test` needs no
  app.

The bridge itself needed nothing new: waiting is polling `/state`, a screenshot
already comes back as PNG bytes, and a command's success is read from
`lastCommand` and the `bridge command N (type) failed` console line.
