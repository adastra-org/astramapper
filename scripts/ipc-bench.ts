/* oxlint-disable no-await-in-loop -- probes must run one after another, that is the measurement */
// Raw IPC cost of the release app, measured from inside its WebView.
//
//   bun run build && cargo build --release --features tauri/custom-protocol
//   bun scripts/ipc-bench.ts <path to astramapper.exe> <GTA V folder>
//
// Launches the exe with WebView2 remote debugging on, drives it over CDP and
// prints p50 / p95 / max in ms per probe. It exists to answer #8: is a slow
// `invoke` the transport, the backend, or the JS main thread? With nothing else
// running, a call is ~1 ms; what the F3 panel shows is queueing behind bytes
// (see "IPC cost" in docs/streaming.md). Windows only (WebView2).
//
// `--features tauri/custom-protocol` matters: a plain `cargo build --release`
// binary points at the dev server instead of the embedded frontend.

const [exe, game] = process.argv.slice(2);
if (!exe || !game) {
  console.error("usage: bun scripts/ipc-bench.ts <astramapper.exe> <GTA V folder>");
  process.exit(1);
}

const PORT = 9333;
const proc = Bun.spawn([exe], {
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
    // Its own profile, so a running copy of the app does not collide with it.
    WEBVIEW2_USER_DATA_FOLDER: `${process.env.TEMP ?? "."}\\astramapper-ipc-bench`,
  },
  stdout: "ignore",
  stderr: "ignore",
});

async function pageTarget(): Promise<{ webSocketDebuggerUrl: string }> {
  for (let attempt = 0; attempt < 60; attempt++) {
    try {
      const targets = (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()) as {
        type: string;
        webSocketDebuggerUrl: string;
      }[];
      const page = targets.find((t) => t.type === "page");
      if (page) return page;
    } catch {
      // not listening yet
    }
    await Bun.sleep(500);
  }
  throw new Error("the WebView never exposed a CDP page");
}

const socket = new WebSocket((await pageTarget()).webSocketDebuggerUrl);
await new Promise((resolve) => {
  socket.addEventListener("open", resolve);
});
let nextId = 0;
type CdpMessage = {
  id: number;
  result?: { exceptionDetails?: unknown; result?: { value?: unknown } };
};
const waiting = new Map<number, (message: CdpMessage) => void>();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(String(event.data)) as CdpMessage;
  waiting.get(message.id)?.(message);
});

function evaluate(expression: string): Promise<unknown> {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    waiting.set(id, (message) => {
      const result = message.result;
      if (result?.exceptionDetails) reject(new Error(JSON.stringify(result.exceptionDetails)));
      else resolve(result?.result?.value);
    });
    socket.send(
      JSON.stringify({
        id,
        method: "Runtime.evaluate",
        params: { expression, awaitPromise: true, returnByValue: true },
      }),
    );
  });
}

// Helpers every probe shares, evaluated in the page.
const prelude = `
const pct = (a, f) => { const s = [...a].sort((x, y) => x - y); return +s[Math.min(s.length - 1, Math.ceil(s.length * f) - 1)].toFixed(2); };
const sum = (a) => ({ n: a.length, p50: pct(a, .5), p95: pct(a, .95), max: +Math.max(...a).toFixed(2) });
const inv = (c, a) => window.__TAURI_INTERNALS__.invoke(c, a);
async function seq(n, f) { const o = []; for (let i = 0; i < n; i++) { const t = performance.now(); await f(i); o.push(performance.now() - t); } return o; }
`;

async function probe(name: string, body: string): Promise<void> {
  const value = await evaluate(`(async () => { ${prelude} ${body} })()`);
  console.log(name.padEnd(52), JSON.stringify(value));
}

await Bun.sleep(3000);
await evaluate(
  `window.__TAURI_INTERNALS__.invoke('open_game_folder', { folder: ${JSON.stringify(game)} })`,
);

await probe("setTimeout(0), the JS floor", `return sum(await seq(100, () => new Promise(r => setTimeout(r, 0))));`);
await probe("app_info (sync, trivial)", `return sum(await seq(100, () => inv('app_info')));`);
await probe("world_status (sync)", `return sum(await seq(100, () => inv('world_status')));`);
await probe("world_asset (async, tiny reply)", `return sum(await seq(100, () => inv('world_asset', { name: 'x' })));`);

const model = (await evaluate(
  `(async () => { for (const n of ['prop_tree_cedar_02', 'prop_bin_01a', 'prop_cs_bin_01']) { const p = await window.__TAURI_INTERNALS__.invoke('world_asset', { name: n }); if (p) return p; } return null; })()`,
)) as string | null;

if (model) {
  const path = JSON.stringify(model);
  await evaluate(
    `window.__TAURI_INTERNALS__.invoke('set_gpu_capabilities', { caps: { s3tc: true, rgtc: true, bptc: true } })`,
  );
  await evaluate(`window.__TAURI_INTERNALS__.invoke('world_model', { path: ${path} })`); // warm the cache
  await probe(
    "world_model (cached) sequential",
    `const b = await inv('world_model', { path: ${path} }); const r = sum(await seq(50, () => inv('world_model', { path: ${path} }))); r.bytes = b.byteLength; return r;`,
  );
  for (const n of [1, 2, 4, 8, 12]) {
    await probe(
      `throughput, ${n} in flight (MB/s)`,
      `let bytes = 0, stop = false; const t0 = performance.now(); const w = Array.from({ length: ${n} }, async () => { while (!stop) { bytes += (await inv('world_model', { path: ${path} })).byteLength; } }); await new Promise(r => setTimeout(r, 3000)); stop = true; const el = performance.now() - t0; await Promise.all(w); return +(bytes / 1048576 / (el / 1000)).toFixed(0);`,
    );
  }
  for (const n of [0, 4, 12]) {
    await probe(
      `world_status while ${n} model transfers are in flight`,
      `let stop = false; const w = Array.from({ length: ${n} }, async () => { while (!stop) await inv('world_model', { path: ${path} }); }); await new Promise(r => setTimeout(r, 500)); const r = sum(await seq(40, () => inv('world_status'))); stop = true; await Promise.all(w); return r;`,
    );
  }
}

socket.close();
proc.kill();
process.exit(0);
