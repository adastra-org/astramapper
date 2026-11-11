/* oxlint-disable no-await-in-loop -- steps and polls run one after another by design */
/**
 * Command line for the debug bridge and Code Mode. Parsing and output live here;
 * `scripts/bridge.ts` only calls `main`.
 */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { BridgeError, connect, defaultInfoPath } from "./client";
import { PLACES } from "./places";
import { renderMarkdown } from "./report";
import { runScenario } from "./runner";
import type { Scenario } from "./scenario";
import { SCENARIOS } from "./scenarios";
import type { Bridge } from "./types";

export const USAGE = `bun scripts/bridge.ts <command>

  state [--gauges|--spans|--clickable]   F3's numbers and the camera, as JSON
  logs [--since N] [--limit N] [--follow] the console and backend lines
  shot [file.png]                         a screenshot of the World canvas
  cmd <type> [key=value ...]              queue a command (values are JSON when they parse)
  cmd '<json>'                            ... or the whole command as JSON
  run <scenario> [--out DIR]              run a scenario, write report.json, report.md and shots
  list                                    shipped scenarios and known places

Needs the app running (bun run tauri dev); starts nothing. Bridge info:
${defaultInfoPath()} (override with ASTRAMAPPER_BRIDGE_INFO).`;

/** `key=value` pairs to a command; a value that parses as JSON is JSON, else a string. */
export function parseCommand(args: readonly string[]): { type: string } & Record<string, unknown> {
  const [first, ...rest] = args;
  if (!first) throw new BridgeError("cmd needs a type or a JSON command");
  if (first.trimStart().startsWith("{")) {
    const parsed = JSON.parse(first) as { type?: unknown };
    if (typeof parsed.type !== "string") throw new BridgeError('a command needs a string "type"');
    return parsed as { type: string };
  }
  const command: { type: string } & Record<string, unknown> = { type: first };
  for (const pair of rest) {
    const eq = pair.indexOf("=");
    if (eq <= 0) throw new BridgeError(`expected key=value, got "${pair}"`);
    const key = pair.slice(0, eq);
    const raw = pair.slice(eq + 1);
    try {
      command[key] = JSON.parse(raw);
    } catch {
      command[key] = raw;
    }
  }
  return command;
}

export function flag(args: readonly string[], name: string): string | undefined {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
}

async function loadScenario(ref: string): Promise<Scenario> {
  const shipped = SCENARIOS[ref];
  if (shipped) return shipped;
  if (!/\.(json|ts|js)$/.test(ref)) {
    throw new BridgeError(`unknown scenario "${ref}"; shipped: ${Object.keys(SCENARIOS).join(", ")}`);
  }
  const path = resolve(ref);
  if (path.endsWith(".json")) return JSON.parse(await readFile(path, "utf8")) as Scenario;
  const url = pathToFileURL(path).href;
  const module = (await import(url)) as { default?: Scenario };
  if (!module.default) throw new BridgeError(`${ref} has no default export`);
  return module.default;
}

async function writeFileDeep(path: string, data: Uint8Array | string): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, data);
}

function print(value: unknown): void {
  console.log(typeof value === "string" ? value : JSON.stringify(value, null, 2));
}

async function runCommand(command: string, args: string[], bridge: Bridge): Promise<number> {
  switch (command) {
    case "state": {
      const state = await bridge.state();
      if (args.includes("--gauges")) {
        print(Object.fromEntries((state.state?.gauges ?? []).map((g) => [g.name, g.value])));
      } else if (args.includes("--spans")) {
        print({ webview: state.state?.spans ?? [], backend: state.backend });
      } else if (args.includes("--clickable")) {
        print(state.state?.clickable ?? []);
      } else {
        print(state);
      }
      return 0;
    }
    case "logs": {
      let since = Number(flag(args, "--since") ?? 0);
      const limit = Number(flag(args, "--limit") ?? 500);
      do {
        const page = await bridge.logs(since, limit);
        for (const line of page.logs) console.log(`${line.seq} [${line.source}/${line.level}] ${line.text}`);
        since = page.next;
        if (args.includes("--follow")) await new Promise((r) => setTimeout(r, 1000));
      } while (args.includes("--follow"));
      return 0;
    }
    case "shot": {
      const file = resolve(args[0] ?? "shot.png");
      await writeFileDeep(file, await bridge.screenshot());
      console.log(file);
      return 0;
    }
    case "cmd": {
      const id = await bridge.cmd(parseCommand(args));
      print({ id });
      return 0;
    }
    case "run": {
      const ref = args[0];
      if (!ref) throw new BridgeError("run needs a scenario name or file");
      const scenario = await loadScenario(ref);
      const stamp = new Date().toISOString().replace(/[:.]/g, "-");
      const out = resolve(flag(args, "--out") ?? `out/code-mode/${scenario.name}-${stamp}`);
      const report = await runScenario(scenario, bridge, {
        outDir: out,
        write: writeFileDeep,
        log: (line) => console.error(line),
      });
      await writeFileDeep(`${out}/report.json`, JSON.stringify(report, null, 2));
      await writeFileDeep(`${out}/report.md`, renderMarkdown(report));
      console.log(renderMarkdown(report));
      console.error(`report: ${out}`);
      return report.ok ? 0 : 1;
    }
    default:
      throw new BridgeError(`unknown command "${command}"\n\n${USAGE}`);
  }
}

/** Returns the process exit code: 0 ok, 1 a scenario failed, 2 the tool could not run. */
export async function main(argv: string[]): Promise<number> {
  const [command, ...args] = argv;
  if (!command || command === "help" || command === "--help") {
    console.log(USAGE);
    return 0;
  }
  if (command === "list") {
    print({ scenarios: Object.fromEntries(Object.values(SCENARIOS).map((s) => [s.name, s.description])), places: PLACES });
    return 0;
  }
  try {
    const bridge = await connect();
    return await runCommand(command, args, bridge);
  } catch (cause) {
    if (cause instanceof BridgeError) {
      console.error(cause.message);
      return 2;
    }
    throw cause;
  }
}
