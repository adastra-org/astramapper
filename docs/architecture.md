# Architecture

AstraMapper is a port of [CodeWalker](https://github.com/dexyfex/CodeWalker) from
C# / WinForms / DirectX 11 to Tauri v2: a Rust backend with a React 19 frontend
in the system WebView.

## Crate layout

```
Cargo.toml            workspace root, shared dependency versions
crates/cw-core/       parsers, crypto, RPF reader, meta, world models
crates/cw-render/     wgpu renderer and world streaming (phase 3)
src-tauri/            Tauri app: commands, state, IPC, windows
src/                  React frontend
```

`cw-core` and `cw-render` **must not depend on `tauri`**. They stay testable
with plain `cargo test` and reusable from a CLI or another frontend. `src-tauri`
is the only crate that knows an app exists; it owns state and IPC and delegates
every parse downward.

The three map onto the original like this:

| CodeWalker | AstraMapper |
| --- | --- |
| `CodeWalker.Core` (~178k lines) | `crates/cw-core` |
| `CodeWalker/Rendering/DirectX` + HLSL | `crates/cw-render` (wgpu + WGSL) |
| `CodeWalker/Forms`, `Tools`, `CodeWalker.WinForms` | `src/` (React) |
| SharpDX.Mathematics | `glam` |
| XAudio2 | Web Audio, decoded in Rust |
| XInput | Gamepad API |
| DockPanelSuite | plain resizable panels in React |

## The IPC boundary

Thin, by rule. Heavy work happens in Rust; the WebView receives results that are
already computed — directory listings, decoded RGBA, mesh buffers, selection
state. **Raw archive bytes never cross the boundary for the frontend to parse.**

Consequences that shape the commands:

- Listings are lazy and paginated. Some archives hold tens of thousands of
  entries; a single `list_dir` returning everything would stall the channel.
- The hex viewer requests page ranges, never a whole file.
- Nothing is sent per frame in the render loop.

Concurrency is Rust threads (`std::thread`, `rayon`), replacing the C# `Task`
parallelism. Web Workers are not used.

## Editions

Legacy GTA V (`Grand Theft Auto V`) is the target. Enhanced
(`Grand Theft Auto V Enhanced`) uses Oodle compression, an `rpf.cache`, and Gen9
resource layouts; it is deliberately out of scope until Legacy works end to end.

## Encryption, and why it comes first

Every stock GTA V archive is NG-encrypted. `RpfFile.ReadHeader` decrypts the
entry table and the name table through `GTACrypto.DecryptNG` before it can read a
single filename. Those keys are not distributed: `GTA5Keys.Generate` finds them
inside `GTA5.exe` at runtime by SHA-1 hash search, guided by the ~150 KB of
hashes in `CodeWalker.Core/Resources/magic.dat`.

So the port order is `Jenk` → `GTAKeys` → `GTACrypto` → `RpfFile`. Until the
crypto works, nothing in a retail install opens at all. (RPFTool, the earlier
TypeScript reader, sidestepped this by supporting only unencrypted `OPEN`
archives — which is why it works on FiveM mod archives but not on `x64a.rpf`.)

## Rendering (phase 3)

Default is wgpu drawing into a native surface obtained via `raw-window-handle`,
with the WebView as a transparent UI overlay. A spike, capped at two days,
decides it; three.js in the WebView is the documented fallback. The criteria are
shader fidelity, 60 fps on mid-range hardware, and how awkward the overlay turns
out to be. The decision and its trade-offs get recorded here when made.

## Build and CI

`.github/workflows/ci.yml` defines the checks: `cargo fmt`, `cargo clippy`,
`cargo test`, then `bun run typecheck`, `bunx oxlint .`, `bun test`,
`bun run build`. No remote is wired yet, so nothing runs it; it is committed so
the checks live in one reviewable place.

**Tauri cannot cross-compile.** Each OS builds on a runner of that OS. Producing
a Windows installer from a Linux runner is not a thing; add matrix entries with
their own runner labels instead. A Linux runner additionally needs the WebKit
development packages:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf build-essential curl wget file libssl-dev libxdo-dev
```

Tests that need a real GTA V install are marked `#[ignore]` so CI stays green on
a machine without the game. Run them locally with `cargo test -- --ignored`.

## Fixtures

Real game files live in `fixtures/` and are **gitignored** — they are not
redistributable. `fixtures/manifest.json` is tracked and records source archive,
path, size and SHA-256 for each one, so the set can be rebuilt from a local
install. Fixtures are never fabricated: if a format has no real sample, that is
stated in `formats.md` and the tests use synthetic vectors derived from the C#
reader, marked `TODO(port):`.
