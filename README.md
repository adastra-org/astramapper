# AstraMapper

A port of [CodeWalker](https://github.com/dexyfex/CodeWalker), the GTA V modding
tool, from C# / WinForms / DirectX 11 to Tauri v2 — Rust backend, React 19
frontend.

The goal is the same three things the original does: browse the game's `.rpf`
archives, view and extract what is inside them, and render and edit the world in
3D. Binary formats are ported bit-exactly; the UI and renderer are rebuilt.

## Status: phase 0 complete

**Works**

- Cargo workspace with `cw-core` (the port of `CodeWalker.Core`, no Tauri
  dependency) and the Tauri app crate.
- Jenkins hash ported and verified against the reference implementation.
- App shell with the three modes stubbed, calling into `cw-core` over IPC.
- Tauri dialog / fs / store plugins wired.
- `cargo test`, `bun test`, typecheck, lint and build all green.

**Stubbed**

- Explorer, World and Project are placeholder panels.
- `crates/cw-render` does not exist yet.

**Next: phase 1** — port `CodeWalker.Core` into `cw-core`, starting with the
crypto that gates everything else (`Jenk` → `GTAKeys` → `GTACrypto` → `RpfFile`).
No stock GTA V archive opens until that works. See
[docs/formats.md](docs/formats.md) for per-format status.

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

- [architecture.md](docs/architecture.md) — crate layout, IPC rules, editions,
  why crypto comes first, CI and fixtures.
- [porting-guide.md](docs/porting-guide.md) — how to port one C# unit to Rust,
  with a full worked example.
- [formats.md](docs/formats.md) — per-format port status and open questions.

## Credits

CodeWalker is by [dexyfex](https://github.com/dexyfex) and contributors. This
port tracks the [adastra-org fork](https://github.com/adastra-org/CodeWalker).
