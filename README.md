# AstraMapper

A port of [CodeWalker](https://github.com/dexyfex/CodeWalker), the GTA V modding
tool, from C# / WinForms / DirectX 11 to Tauri v2 — Rust backend, React 19
frontend.

The goal is the same three things the original does: browse the game's `.rpf`
archives, view and extract what is inside them, and render and edit the world in
3D. Binary formats are ported bit-exactly; the UI and renderer are rebuilt.

## Status: phase 1 in progress — archive crypto working

**Works**

- Cargo workspace with `cw-core` (the port of `CodeWalker.Core`, no Tauri
  dependency) and the Tauri app crate.
- App shell with the three modes stubbed, calling into `cw-core` over IPC.
- Tauri dialog / fs / store plugins wired.
- **Archive decryption against a real retail install.** Keys are recovered from
  the local `GTA5.exe` and the NG cipher opens stock archives:

  ```
  x64a.rpf    656 entries,  655 printable names
  common.rpf 1421 entries, 1420 printable names
  ```

  Ported: `Jenk`, `Data` (reader), .NET `System.Random`, `GTAKeys`, `GTACrypto`.
- 27 unit tests plus 3 install-dependent gate tests; typecheck, lint and build
  all green.

**Stubbed**

- No format parsers yet — the RPF reader is next.
- Explorer, World and Project are placeholder panels.
- `crates/cw-render` does not exist yet.

**Next: phase 1.2** — the RPF archive layer (`RpfFile`, `RpfManager`). See
[docs/phases.md](docs/phases.md) for the handoff: what is done, what to do next,
and the traps found so far.

## Development

```bash
bun install
```

```bash
bun run tauri dev
```

Checks, all of which CI runs:

```bash
cargo test --workspace && bun run typecheck && bunx oxlint . && bun test && bun run build
```

Tests that need a real GTA V install are marked `#[ignore]`:

```bash
cargo test --workspace -- --ignored
```

## Docs

- [phases.md](docs/phases.md) — **start here.** Roadmap, current position, and
  the exact next step, written for a session with no prior context.
- [architecture.md](docs/architecture.md) — crate layout, IPC rules, editions,
  why crypto comes first, CI and fixtures.
- [porting-guide.md](docs/porting-guide.md) — how to port one C# unit to Rust,
  with a full worked example.
- [formats.md](docs/formats.md) — per-format port status and open questions.

## Credits

CodeWalker is by [dexyfex](https://github.com/dexyfex) and contributors. This
port tracks the [adastra-org fork](https://github.com/adastra-org/CodeWalker).
