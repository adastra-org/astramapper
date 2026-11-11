# Debug bridge

Lets a developer, or a coding agent, watch the running app and drive its
camera from a shell. Added in v0.3.1.

## What it gives

- The console (`log`, `info`, `warn`, `error`, `debug`, uncaught errors, rejected
  promises) and the backend's `[perf]` lines, as they happen.
- F3's numbers as JSON: gauges, every span's calls, p50 and p95, fps, the
  camera, and which buttons are on screen.
- Commands: teleport or fly the camera, hold a movement key, click a button,
  send a key, take a screenshot, reset the spans.

## Turning it on

It runs in debug builds (`bun run tauri dev`). A release build runs it only with
`ASTRAMAPPER_DEBUG_BRIDGE=1`; `ASTRAMAPPER_DEBUG_BRIDGE=0` turns it off in a
debug build. A shipped app has no listening port and does not patch `console`.

On start the app writes `astramapper-debug-bridge.json` in the temp directory
(`%TEMP%` on Windows) with `port` (17420 when free), `token` and `pid`. Every
request needs the token as `Authorization: Bearer <token>` or `?token=`. The
server binds `127.0.0.1` only and rejects a `Host` that is not a loopback name,
so a web page in the user's browser cannot drive the app. The file is stale once
the app exits; check `pid`.

## Routes

| Route | |
|---|---|
| `GET /health` | `{ "ok": true }` |
| `GET /state` | `{ state, ageMs, backend }`: the WebView's last snapshot (refreshed every second) and the backend's span report |
| `GET /logs?since=N&limit=M` | lines with `seq > N`; keep `next` for the following call. The last 4,000 lines are kept |
| `POST /cmd` | queue a command (a JSON object with a string `type`); returns `{ "id": n }`; `state.lastCommand` shows the last one run |
| `GET /screenshot` | queues a capture and returns the PNG (`504` if World is not open) |

## Commands

Positions are game coordinates (Z up), the ones the app prints.

| `type` | fields |
|---|---|
| `camera` | `gta?: [x,y,z]`, `lookAtGta?: [x,y,z]`, `yawDeg?`, `pitchDeg?` |
| `fly` | `path: [[x,y,z], ...]`, `speed?` in m/s (default 300) |
| `hold` | `key` (`forward`, `back`, `left`, `right`, `up`, `down`, `sprint`), `ms` |
| `stop` | cancel a flight and release held keys |
| `screenshot` | capture the canvas on the next sync |
| `resetPerf` | clear the F3 spans, both sides |
| `click` | `text`, `nth?`: the visible button, link or checkbox whose label contains `text` (`state.clickable` lists them) |
| `key` | `key`: keydown and keyup on the window, e.g. `F3` |

Camera commands need World open; open it with `{"type":"click","text":"world"}`.
A command that cannot run logs `bridge command N (type) failed: ...` to the
console, visible in `/logs`.

## Example

```bash
F="$TEMP/astramapper-debug-bridge.json"
PORT=$(sed 's/.*"port":\([0-9]*\).*/\1/' "$F"); TOKEN=$(sed 's/.*"token":"\([^"]*\)".*/\1/' "$F")
H="Authorization: Bearer $TOKEN"
curl -s -H "$H" -X POST -d '{"type":"click","text":"world"}' http://127.0.0.1:$PORT/cmd
curl -s -H "$H" -X POST -d '{"type":"fly","path":[[195,-935,80],[-300,-300,80]],"speed":120}' http://127.0.0.1:$PORT/cmd
curl -s -H "$H" http://127.0.0.1:$PORT/state
curl -s -H "$H" http://127.0.0.1:$PORT/screenshot -o shot.png
```

## How it works

`src-tauri/src/debug_bridge.rs` holds the queues and the HTTP listener (a small
hand-written HTTP/1.0 subset, no new dependency). `src/lib/debugBridge.ts` calls
the `debug_bridge_sync` command four times a second: console lines and, once a
second, a snapshot go up, queued commands come down. World registers its fly
camera with `registerCamera` so commands move the same state WASD does.

## First run (v0.3.1, base game, release-like dev build)

Flight over Los Santos at 120 m/s through five waypoints, F3 read through
`/state`:

- `frame` p95 30 to 85 ms, `long task` p95 100 to 150 ms, fps 50 to 56.
- `JS heap used` went from 1,068 MB to 5,376 MB and stayed at 5.5 GB while idle,
  with the model cache at its 1,238 MB budget. `resident models` kept growing
  (418, 1,236, 1,855) and `geometries` with it (2,202 to 5,489) while the cells
  held went back down to about 40. Tracked in #23.

## After the texture fix (v0.3.2, #23)

The heap growth was `textureCache` in `World.tsx`: a bare `Map` that never
released anything, whose textures kept their mip chains, which are views over
the IPC response each model arrived in. One texture kept a whole response
(geometry included) alive for the session, so the cache budget of #6 did not
bound the heap. Textures are now reference-counted with the shared model parts
that use them (`src/lib/world/refCache.ts`), freed with the last user, and drop
their CPU mips once uploaded. F3 gets `texture cache` and `shared models`.

Same flight, same machine:

| | before | after |
|---|---|---|
| `JS heap used` at 8 s intervals (MB) | 2,439 / 2,789 / 3,305 / 3,722 / 4,012 / 4,463, then 5,376 | 2,704 / 3,133 / 3,640 / 3,804 / 4,168 / 2,870 / 3,218 |
| heap idle after the flight | 5,500 and not coming down | 3,150 to 3,200, stable |
| `textures` / `geometries` | 482 to 2,531 / 2,202 to 5,489, only growing | 1,448 to 2,983 / 3,743 to 5,865, follow what is near |

The heap is no longer monotonic and the collector brings it back. It is not
under the 300 MB target of #2: the model cache is allowed up to 3 GB (#6) and
the models in use stay in it. Screenshots after the flight show the textures
intact.

## Reading a whole run

F3's `p95` is over the last 256 samples. Each span in `/state` also carries
`runP50Ms` and `runP95Ms` over every call since the last `resetPerf` (0.5 ms
bins, so an upper edge), which is what to compare between two flights.
`bun run tauri dev` with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222 --remote-allow-origins=*"`
also opens the WebView to the DevTools protocol for a CPU profile or a trace
while the bridge flies a route; see "Frame hitches" in streaming.md.

## Next

A Code Mode on top of the same channel (scripted scenarios, assertions on the
gauges) is not built; the commands above are the pieces it would call.
