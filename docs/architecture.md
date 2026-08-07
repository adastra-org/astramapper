# Architecture

AstraMapper is a port of [CodeWalker](https://github.com/dexyfex/CodeWalker) from
C# / WinForms / DirectX 11 to Tauri v2: a Rust backend with a React 19 frontend
in the system WebView.

## Crate layout

```
Cargo.toml            workspace root, shared dependency versions
crates/cw-core/       parsers, crypto, RPF reader, meta, world models, wire format
crates/cw-cache/      pack-file disk cache for compiled assets
src-tauri/            Tauri app: commands, state, IPC, windows
src/                  React frontend
```

There is no `cw-render`. Early drafts of this document listed one, holding a
wgpu renderer; the phase 3 spike decided against a native surface (see
[Rendering](#rendering-phase-3)) and the crate was never created.

`cw-core` **must not depend on `tauri`**. It stays testable with plain
`cargo test` and reusable from a CLI or another frontend. `src-tauri` is the
only crate that knows an app exists; it owns state and IPC and delegates every
parse downward.

`cw-cache` depends on **neither** `tauri` nor `cw-core`. It stores opaque bytes
under an opaque key; what the bytes mean is the caller's business. That is what
makes its round trip testable with no app and no game install, and it is why
`src-tauri` resolves the cache directory from `app_cache_dir()` and hands it in
rather than the crate reaching for one. See [phase 4.5-4.7](phases.md) for the
three decisions behind it.

The layout maps onto the original like this:

| CodeWalker | AstraMapper |
| --- | --- |
| `CodeWalker.Core` (~178k lines) | `crates/cw-core` |
| `CodeWalker/Rendering/DirectX` + HLSL | `src/` (three.js in the WebView) |
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
- A vertex declaration, an ADPCM block and a text encoding are all resolved in
  Rust. The WebView receives `f32` arrays, a `.wav` and a `String`.

Buffers that are genuinely bytes — mesh attributes, hex pages, decoded audio —
cross as base64 rather than as JSON number arrays. A 60k-vertex drawable is about
2 MB of JSON digits versus 700 KB of base64 that decodes straight into a
`Float32Array`. Same channel, very different parse on the far side.

That reasoning was right and stopped one step short: base64 still copies every
buffer three times before it reaches the GPU. The world view's streaming path
uses a **binary envelope** instead (`crates/cw-core/src/wire.rs`): a fixed
header, UTF-8 JSON metadata, then raw sections on 16-byte boundaries, returned
through `tauri::ipc::Response` and read as an `ArrayBuffer`. The alignment is
what lets the frontend take `Float32Array` and `Uint8Array` *views* over the
response rather than copies. The metadata stays JSON on purpose: it is the part
that churns, and the bytes that matter are already zero-copy. The Explorer's
commands still use base64 — one file at a time is not where the copies hurt.

The envelope carries more than buffers. A `.ymap`'s placements go through it as
one section per field — position, rotation, scale, LOD, and integer columns into
two string tables for the archetype name and the LOD label. That payload is
thirteen small fields per entity rather than one big buffer, so the saving is a
different one: JSON repeats every field *name* per entity, about 120 bytes of
keys against 60 bytes of values, and columns delete them. Over the base game's
763,716 placements that is 204.1 MB down to 51.4 MB, and 745ms of `JSON.parse`
down to an 88ms decode. See `src-tauri/src/placements.rs`.

**Textures on the streaming path cross as GPU blocks, not as images.** This is a
deliberate change to the rule above, so here is the defence. The rule exists
because the WebView has no business *parsing* archive formats, and it still does
not: the vertex declaration, the mip layout, the dictionary chain and the format
identification all resolve in Rust. BC1 through BC7 are not an archive format,
they are the GPU's own storage format, and the WebView's entire involvement is
one `compressedTexImage2D` call per mip level. The alternative measured at 12.62s
against 2.87s over 58 placed city models, for a *larger* payload and four times
the VRAM, because it decoded blocks to RGBA and re-encoded them as PNG so the
browser could decode them again and the driver could re-compress nothing. The
frontend declares which formats its context accepts through
`set_gpu_capabilities`; anything it cannot take is decoded to RGBA in Rust. See
`src-tauri/src/textures.rs`.

One file is open at a time, and its extracted payload is held in Rust state
(`viewer::ViewerState`). Extraction is a seek, a decrypt and an inflate; paging
through a 20 MB texture dictionary would otherwise redo all three per scroll.

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

**Decision: three.js in the WebView.** The phase 3 spike resolved the wgpu-versus-
three question in favour of the documented fallback. The native route meant a wgpu
surface obtained via `raw-window-handle` with the WebView composited over it as a
transparent overlay. In Tauri v2 there is no first-class native-only window: every
`tauri::Window` hosts a WebView, so the surface would live in a separate `tao`
window running alongside Tauri's own event loop, with a transparent, frameless,
always-on-top WebView tracking its position and size. That is two windowing
worlds sharing one process, and each half has its own sharp edge: mixing `tao` and
`tauri` event loops is unsupported, and a transparent WebView over a GPU surface
has known compositing and input-routing problems on Windows (the target platform
for a GTA V tool). The upside — exact HLSL→WGSL shader parity — does not earn a
modding tool's debug view, where what matters is placing things and seeing why.

three.js wins on the phase's own criteria: it is already proven in this repo
(Explorer's model and collision previews upload Rust-decoded buffers at 60 fps),
the overlay awkwardness is zero because there is no overlay, and shader fidelity
is its PBR built-ins, which are enough to show placement, materials and lighting.
Consequences of the decision:

- `crates/cw-render` is not created as a wgpu crate. World rendering lives in the
  WebView; Rust prepares geometry and placements, exactly as in Explorer mode.
- There is no HLSL→WGSL translation table. Model materials use three.js
  `MeshStandardMaterial`, and the per-format fidelity questions that remain are
  data questions (skeleton transforms, shader parameter resolution), not shader
  language questions.
- "LOD streaming" becomes loading maps and models on demand from the Rust index
  as the camera approaches, rather than a GPU residency loop.

Explorer Mode's model preview shares this stack and continues to load on demand
so it stays out of the startup bundle.

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
