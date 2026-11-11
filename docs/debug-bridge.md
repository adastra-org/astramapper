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

## Next

A Code Mode on top of the same channel (scripted scenarios, assertions on the
gauges) is not built; the commands above are the pieces it would call.
