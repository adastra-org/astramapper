# AstraMapper

A port of [CodeWalker](https://github.com/dexyfex/CodeWalker), the GTA V modding
tool, from C# / WinForms / DirectX 11 to Tauri v2 — Rust backend, React 19
frontend.

The goal is the same three things the original does: browse the game's `.rpf`
archives, view and extract what is inside them, and render and edit the world in
3D. Binary formats are ported bit-exactly; the UI and renderer are rebuilt.

## Status: phase 3 in progress — Explorer done, World started

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
    Parts bind the diffuse and normal textures resolved from the model's
    `.ytd` dictionaries and drawn with shadow-casting lighting; a LOD picker
    switches between High and VeryLow.
  - **Fragments** for `.yft`, the physics-wrapped drawables that make up most of
    the game's props, vehicles and peds — the main drawable, the cloth, the
    alternative array and every physics child, each named by where it sits.
  - **Collision** for `.ybn`, in three.js over the whole composite tree
    flattened to world space — vertices quantised in Rust, child transforms
    applied, triangles uploaded as buffers.
  - **Map** for `.ymap` and `.ytyp`, the typed models above the generic meta
    tree. A `.ymap` shows the placement hierarchy — every placed entity with
    its parent/child tree, resolved archetype names and LOD levels, MLO
    interior instances and rig extensions — plus the car generators, occluders,
    LOD lights and grass batches. A `.ytyp` lists the archetypes by kind, MLO
    interiors broken down into rooms, portals and entity sets. Filterable, with
    the raw XML tree one tab over.
  - **Texture** for `.ytd` — BC1–BC7 decoded in Rust, mip picker, alpha
    checkerboard, export as `.dds` or `.png`.
  - **Audio** for `.awc` — every stream listed, decoded on demand, played or
    saved as WAV.
  - **Strings** for `.gxt2` — every hash → text pair listed, filterable, with
    the hash shown. GXT2 is the first file type that round-trips byte for byte.
  - **Text**, editable, for anything that sniffs as text whatever its extension.
  - **Hex**, paged, for everything else.
- **World view** — the phase 3 renderer, settled on three.js in the WebView
  (the wgpu-native-surface spike lost on the overlay problem; the decision and
  trade-offs are recorded in `docs/architecture.md`). The world is a fly-camera
  three.js scene over placements the Rust side already decoded. Content is
  grouped into `base` plus every DLC/patchday pack, each browsable down to its
  `.ymap` files; placing a map renders its entities as LOD-coloured instanced
  boxes, click-to-select with a details panel, double-click to focus, and the
  map open in Explorer can be placed with one button. A `Stream HD geometry`
  toggle loads the nearest placed entities' real models as you fly — resolving
  each archetype to its `.ydr`/`.yft`, streaming them in and out by distance,
  swapping the drawable's High→VeryLow LODs per frame and binding each part's
  resolved diffuse/normal textures — with the selected entity's model also
  loadable by hand. With no game folder open the same
  renderer shows a demo plaza so the pipeline works without the game.
- **Extraction to disk** for any entry, decrypted and decompressed.
- 194 unit tests, 40 install-dependent gate tests, 48 frontend tests. Format
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
  fragments  228 drawables across the physics LOD tree identical; 61,430 .yft
             files parse, 223,359 embedded drawables, 459,734 geometries
  shaders    173 drawables, 197 shaders, 3,805 parameters and 40 embedded
             textures identical; 109,758 .ydr/.ydd files read their shader
             parameters across the whole install
  gxt2       283 entries identical, and all 19,477 .gxt2 files round-trip
             byte for byte (8.8M strings)
  ```

**Not yet**

- Writing. Reading only, everywhere: `RpfFile.Write`, `CompressBytes` and NG
  *encryption* are all still open, so a text edit saves a copy to disk rather
  than going back into the archive.
- Particles (`.ypt`) — embeds drawables behind `Particle.cs`, which is not
  ported, so it opens as hex with a note saying which parser is missing.
- Stream names in `.awc`: `RelFile.cs` is not ported, so streams show as hashes.
- The remaining world features — the full streaming scheduler, gizmos, undo/redo
  and `.cwproj` projects — are still ahead. `Project` is still a placeholder.

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

Seven of those gates compare against data dumped from the reference DLL. The
dumps are gitignored; regenerate them first or the gates skip with a message:

```bash
pwsh -File scripts/rpf-oracle.ps1; pwsh -File scripts/drawable-oracle.ps1; pwsh -File scripts/awc-oracle.ps1; pwsh -File scripts/texture-oracle.ps1; pwsh -File scripts/frag-oracle.ps1; pwsh -File scripts/gxt2-oracle.ps1; pwsh -File scripts/shader-oracle.ps1
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
