# AstraMapper

A port of [CodeWalker](https://github.com/dexyfex/CodeWalker), the GTA V modding
tool, from C# / WinForms / DirectX 11 to Tauri v2 — Rust backend, React 19
frontend.

The goal is the same three things the original does: browse the game's `.rpf`
archives, view and extract what is inside them, and render and edit the world in
3D. Binary formats are ported bit-exactly; the UI and renderer are rebuilt.

## Status: phase 2 in progress — the whole install browses, and six viewers work

**Works**

- Cargo workspace with `cw-core` (the port of `CodeWalker.Core`, no Tauri
  dependency) and the Tauri app crate.
- **Archive decryption against a real retail install.** Keys are recovered from
  the local `GTA5.exe` and the NG cipher opens stock archives.
- **The whole game folder as one tree.** 179 top-level archives, 5,199 nested,
  403,642 entries, scanned in 0.2 s — nested `.rpf`s browse with no visible seam.
- **Explorer viewers**, with the backend choosing which one opens because that
  decision needs the payload:
  - **3D** for `.ydr` and `.ydd`, in three.js over mesh buffers decoded in Rust.
  - **Collision** for `.ybn`, in three.js over the whole composite tree
    flattened to world space — vertices quantised in Rust, child transforms
    applied, triangles uploaded as buffers.
  - **Texture** for `.ytd` — BC1–BC7 decoded in Rust, mip picker, alpha
    checkerboard, export as `.dds` or `.png`.
  - **Audio** for `.awc` — every stream listed, decoded on demand, played or
    saved as WAV.
  - **Text**, editable, for anything that sniffs as text whatever its extension.
  - **Hex**, paged, for everything else.
- **Extraction to disk** for any entry, decrypted and decompressed.
- 171 unit tests, 28 install-dependent gate tests, 27 frontend tests. Format
  work is cross-checked against `CodeWalker.Core.dll` loaded as an oracle:

  ```
  drawables  213 geometries, 80,350 vertices identical; 716,436 vertices
             inside their own AABB across 300 files
  textures   198 textures identical field for field, 198 .dds files byte for
             byte; 335,729 textures parse across the whole install
  audio      261 streams, 132,741,415 samples decode identically;
             400 containers open, 104,263 seconds of audio
  bounds     85 geometries, 127,345 polygons identical, 126,942 triangles
             flattened one for one; 15,139 .ybn files parse, 104.6M triangles
  ```

**Not yet**

- Writing. Reading only, everywhere: `RpfFile.Write`, `CompressBytes` and NG
  *encryption* are all still open, so a text edit saves a copy to disk rather
  than going back into the archive.
- Metadata (`.ymap`, `.ytyp`), fragments (`.yft`), particles (`.ypt`) — each
  opens as hex or a tree, with a note saying which parser is missing.
- Textures on models: the 3D viewer draws untextured. Both halves exist now;
  matching a material to a texture needs `ShaderParams.cs`.
- Stream names in `.awc`: `RelFile.cs` is not ported, so streams show as hashes.
- World and Project are placeholder panels; `crates/cw-render` does not exist.

See [docs/phases.md](docs/phases.md) for the handoff: what is done, what to do
next, and the traps found so far.

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

Four of those gates compare against data dumped from the reference DLL. The
dumps are gitignored; regenerate them first or the gates skip with a message:

```bash
pwsh -File scripts/rpf-oracle.ps1; pwsh -File scripts/drawable-oracle.ps1; pwsh -File scripts/awc-oracle.ps1; pwsh -File scripts/texture-oracle.ps1
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
