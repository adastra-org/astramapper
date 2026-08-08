# Phase status and handoff

The roadmap, where the port currently stands, and exactly what to do next. Written
so a session with no prior context can pick the work up from here.

Read alongside [architecture.md](architecture.md) (why things are shaped this way),
[porting-guide.md](porting-guide.md) (how to port one unit), and
[formats.md](formats.md) (per-format status).

---

## Where things stand

| Phase | State |
| --- | --- |
| 0 — Scaffolding | **complete** |
| 1 — Core port (`cw-core`) | **in progress** — 1.1 crypto, 1.2 RPF, 1.3 MetaTypes and the drawable, texture, audio, clip, bounds and fragment slices of 1.4 done, plus `YtypFile` and `YmapFile`, all gates cleared |
| 2 — Explorer Mode | **in progress** — browsing, per-type viewers, extraction |
| 3 — World View | **complete** — renderer decided (three.js), world mode shell, WASD camera, raycast selection, single-map placement, DLC/patchday content groups, real-geometry LOD streaming with textures, move/rotate/scale gizmos + undo/redo, `.cwproj` save/load, per-map load-region streaming, MLO entity-set expansion |
| 4 — World performance, shaders and tooling | **in progress** — mapped archives, world commands off the event loop, archetype-driven texture resolution, binary IPC envelope carrying GPU texture blocks and geometry, cache storage layer, the renderer's per-pass sweeps made proportional to what changed, placements off JSON and into the envelope, a view-ordered streaming pass |

Branch: work happens on `dev`. Current tests: **284 unit + 52 gate + 137 frontend**.
The gates are `#[ignore]`d and need a local install:

```bash
cargo test --workspace -- --ignored --nocapture
```

---

## Machine setup (already true, do not redo)

| Thing | Where |
| --- | --- |
| C# reference | `E:\Ad Astra Tools\CodeWalkerAstra` — `adastra-org/CodeWalker` @ `d11e5951`. **Read-only, in place. Never modify.** |
| Reference binaries | `E:\Ad Astra Tools\CodeWalker` — built 2025-03-17. GUI to compare against, and a loadable oracle. |
| Prior TS RPF reader | `E:\Ad Astra Tools\RPFTool` — handles `OPEN` archives; second oracle + real fixtures in `incoming/`. |
| GTA V Legacy (target) | `E:\SteamLibrary\steamapps\common\Grand Theft Auto V` |
| GTA V Enhanced (out of scope) | `...\Grand Theft Auto V Enhanced` — Oodle, `rpf.cache`, Gen9 |

### The oracle trick

`CodeWalker.Core.dll` is netstandard2.0, so PowerShell 5.1 loads it and you can
call the real implementation for expected values:

```powershell
Add-Type -Path "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll"
[CodeWalker.GameFiles.JenkHash]::GenHash("x64a.rpf")
```

Use this for every test vector. Values derived by hand are derived from your
understanding of the code, which is the thing under test.

---

## Commands

```bash
cargo test --workspace && bun run typecheck && bunx oxlint . && bun test && bun run build
```

The gates (need the game; override the path with `ASTRAMAPPER_GTA_PATH`):

```bash
cargo test --workspace -- --ignored --nocapture
```

Five of them compare against oracles whose data is gitignored. Regenerate before
running, or they skip with a message:

```bash
pwsh -File scripts/rpf-oracle.ps1
```

```bash
bash scripts/rpftool-oracle.sh
```

```bash
pwsh -File scripts/drawable-oracle.ps1
```

```bash
pwsh -File scripts/awc-oracle.ps1
```

```bash
pwsh -File scripts/texture-oracle.ps1
```

```bash
pwsh -File scripts/clip-oracle.ps1
```

```bash
pwsh -File scripts/meta-oracle.ps1
```

```bash
pwsh -File scripts/bounds-oracle.ps1
```

```bash
pwsh -File scripts/frag-oracle.ps1
```

```bash
pwsh -File scripts/gxt2-oracle.ps1
```

```bash
pwsh -File scripts/ytyp-oracle.ps1
```

```bash
pwsh -File scripts/ymap-oracle.ps1
```

---

## Phase 0 — complete

Cargo workspace (`crates/cw-core` + `src-tauri`), React 19 + Tailwind v4 + oxlint
frontend, three-mode app shell calling `app_info` over IPC, dialog/fs/store
plugins wired, CI defined in `.github/workflows/ci.yml` (committed, **not**
runner-bound — no remote exists yet).

---

## Phase 1 — core port

### 1.1 Utils and crypto — **DONE, gate cleared**

Files: `crates/cw-core/src/utils/{jenk,data,net_random,crypto,keys}.rs`,
gate at `crates/cw-core/tests/crypto_gate.rs`.

Proven against the real install:

```
x64a.rpf    656 entries,  655 printable names, 11888 byte name table
common.rpf 1421 entries, 1420 printable names, 28112 byte name table
```

**Four things worth knowing before touching this code:**

1. **Only one key path matters.** `GTAKeys.cs` has two. Every real caller uses
   `LoadFromPath` → `GenerateV2` + `UseMagicData`: scan the exe for the AES key
   only, unmask everything else from `magic.dat`. `RandomGauss` and
   `LookUpTableGenerator` — the bulk of that 91 KB file — build *encryption*
   tables and are not needed to read anything.
2. **`GTA5Hash` is Jenkins plus a lookup table.** `1025*x == x+(x<<10)`,
   `9*x == x+(x<<3)`, `32769*x == x+(x<<15)` are Jenkins' three mixing steps.
   The real LUT remaps only 27 of 256 values, so the two hashes agree on most
   inputs — dropping the table looks correct until it isn't.
3. **The LUT is not a permutation** (229 distinct values, max multiplicity 2).
4. **.NET `System.Random` is reimplemented bit-exactly** in `net_random.rs`
   because `UseMagicData` seeds it with the Jenkins hash of the AES key. Do not
   substitute another PRNG.

### 1.2 RPF archive layer — **DONE, gate cleared**

Files: `crates/cw-core/src/rpf/{mod,entry,file,manager}.rs`, gate at
`crates/cw-core/tests/rpf_gate.rs`.

Proven against the real install:

```
179 top-level archives, 5199 nested, 403,642 entries, scanned in 0.2s
x64a.rpf    all 656 entries and all 624 extracted payloads match CodeWalker
common.rpf all 1421 entries and all 1378 extracted payloads match CodeWalker
7 OPEN archives, 249 entries agree with RPFTool
```

Both oracles were used, and they disagree in exactly one place — see below.
Regenerate their data with `scripts/rpf-oracle.ps1` and
`scripts/rpftool-oracle.sh`.

**Five things worth knowing before touching this code:**

1. **The three entry layouts are genuinely different**, not one field at three
   widths. A directory reads its name offset as a `u32`, a resource as a `u16`,
   a binary entry as the low 16 bits of a packed `u64`. They agree only because
   name tables are under 64 KB. Unifying them will work until a name table grows.
2. **Unknown encryption values are NG, not errors.** The reference's `default:`
   case decrypts as NG with the comment "never seems to get here".
3. **`GetSizeFromFlags` overflows into negative `int`** — the reference really
   returns `-1627389952` for `0x0FEFFFFF`. `size_from_flags` computes in `u64`
   and truncates, reproducing the same 32 bits. This is pinned as a test because
   any misplaced shift among the nine page-count fields would land elsewhere.
4. **A resource length of `0xFFFFFF` is an escape**, not a length: the real value
   is unpacked from bytes 7, 14, 5 and 2 of the payload's own header. **RPFTool
   does not implement this** and reports the raw sentinel, so it is the single
   point where the two oracles disagree; CodeWalker arbitrates and agrees with
   this port (`b2a.ytd` is 20921589 bytes).
5. **A nested archive is one thing seen from two sides.** The `.rpf` binary entry
   in the parent and the root directory of the archive it opens share a path. The
   index keeps the parent's entry — it is the one that can be extracted — and
   listing it steps through transparently.

Not ported: writing. `Write` on every entry type, `CompressBytes`, and NG
*encryption* are all still open, so `formats.md` marks this **partial** rather
than **done** under its round-trip rule.

### 1.4a Resources — drawables — **DONE, gate cleared**

Taken out of order, ahead of 1.3, because Explorer Mode needs *something* to
view and a drawable is the shortest path from an archive entry to a picture.

Files: `crates/cw-core/src/resources/{reader,vertex,drawable}.rs` and
`crates/cw-core/src/mesh.rs`, gate at
`crates/cw-core/tests/drawable_gate.rs`, oracle at
`scripts/drawable-oracle.ps1`.

Proven against the real install:

```
40 files, 213 geometries, 80,350 vertices agree with CodeWalker field for field
300 files, 1,163 geometries, 716,436 vertices all inside their own AABB
150 files, 349 renderable bundles, 391,852 triangles
```

**Four things worth knowing before touching this code:**

1. **A junk payload parses.** `ResourceDataReader` reads through `MemoryStream`,
   which short-reads instead of throwing, so a truncated or wrong file decodes
   to zeros — which look like null pointers, which look like absent blocks. The
   result is a valid `Drawable` with nothing in it. Ask `has_geometry`, not
   whether `read` returned `Ok`.
2. **Vertex declarations are indexed by semantic, not by packing order.** See
   `formats.md`. Reading the nibbles in sequence works on the one declaration
   nearly every YDR uses and misparses the rest.
3. **The drawable's bounding box is not a superset of its vertices** — a fifth
   fall outside, two thirds for skinned models. The *per-geometry* AABB is
   exact, and is what the gate validates positions against.
4. **`DrawableModelsBlock` has no header of its own.** It is read at whichever
   LOD pointer is non-zero and then ignores that position, re-seeking to each of
   the four pointers held by its owner. The four lists only look nested.

Not ported: writing, Gen9 declarations, and everything a drawable points at that
is not geometry — skeleton, joints, light attributes, collision bounds. The
shader group *is* ported: `ShaderFX` parameters with each texture slot resolved
to a texture hash, plus the embedded `ShaderGroup.TextureDictionary`, pinned by
`shader_gate.rs` and put to work by the textured model viewer.

### 1.4b Resources — textures — **DONE, gate cleared**

Out of order for the same reason as the drawables: a `.ytd` is self-contained,
and a texture is the second-shortest path from an archive entry to a picture.

Files: `crates/cw-core/src/resources/texture.rs` and
`crates/cw-core/src/image.rs`, gate at
`crates/cw-core/tests/texture_gate.rs`, oracle at
`scripts/texture-oracle.ps1`.

Proven against the real install:

```
36 files, 198 textures agree with CodeWalker field for field
198 DDS files byte-identical, 198 decodes within 1/255 per channel
200 files, 1,000 textures, 368,119,504 pixels decode
88,868 dictionaries, 335,729 textures parse across the whole install
```

**Five things worth knowing before touching this code:**

1. **The payload length and the mip layout are two different calculations.**
   `TextureData.Read` sizes its read as `stride * height` quartered per level;
   `GetMipmapImages` finds each mip with `ComputePitch`, which clamps a
   compressed level to one whole 4x4 block. They disagree for the tail mips of
   every compressed texture. Both are needed — one reads the right bytes, the
   other indexes them.
2. **BC decoding is `bcdec_rs`, not a port, and the output differs by a
   rounding unit.** The reference interpolates block colours in 8-bit space with
   truncating division; the D3D spec interpolates in the packed 5/6-bit domain
   and rounds. The endpoints agree exactly, so only indices 2 and 3 move, by at
   most 1/255 over the whole oracle sample. The gate pins the bound.
3. **CodeWalker cannot decode BC7** — `GetPixels` has an empty case for it. This
   port can, so there is no oracle for that format and the gate checks it
   against its own DDS round trip.
4. **Six stock `.ytd` files do not read, in either implementation.** They are
   resources whose payload does not inflate to the size their page flags
   declare. The reference throws; this port refuses on the structural invariant
   that the name and texture lists are parallel. See `formats.md` for the list.
5. **`TextureBase` and `Texture` are one struct read in two halves.** The
   reference declares width, height, format and the data pointer on the *base*
   class for Gen9's sake, but the Legacy reader only fills them in the subclass
   at offset 0x50.

Not ported: writing, the Gen9 layout, reading a `.dds` back in, and the D3DX
legacy-format conversions in `DDSIO`.

### 1.4c Audio — AWC — **DONE, gate cleared**

Also out of order, for the same reason: `.awc` is self-contained, so a viewer
can play it without any of `1.3` landing first.

Files: `crates/cw-core/src/audio/{awc,adpcm,wav}.rs`, gate at
`crates/cw-core/tests/awc_gate.rs`, oracle at `scripts/awc-oracle.ps1`.

Proven against the real install:

```
31 files (12 multi-channel), 261 streams, 132,741,415 samples decode
  identically to CodeWalker
400 containers open, 12,230 playable streams (11,103 ADPCM, 1,127 PCM),
  104,263 seconds of audio
```

**Three things worth knowing before touching this code:**

1. **Whole-file encryption has no flag.** It is detected by the magic failing to
   read as `ADAT`. The per-block and single-channel modes *are* flags.
2. **The RSXXTEA decrypt looks asymmetric and is not**, and the reference
   describes its own code as undefined behaviour. See `formats.md`; there is a
   test pinning the round trip so a cleanup cannot silently change it.
3. **A stream is one channel.** Multi-channel containers put all the audio in a
   carrier stream with id 0 and have the others index into it.

Not ported: writing, encoding, big-endian console files, the `peak`/`markers`/
`seektable`/granular chunks, and `RelFile.cs` — so streams are identified by
hash, with no names and no `_left`/`_right` pairing.

### 1.4d Resources — clips — **DONE, gate cleared**

A `.ycd` is self-contained like the three before it, and it is what `YddFile`
and the scenario metadata point at.

Files: `crates/cw-core/src/resources/clip.rs`, gate at
`crates/cw-core/tests/clip_gate.rs`, oracle at `scripts/clip-oracle.ps1`.

Proven against the real install:

```
24 dictionaries, 180 clips and 117 animations agree with CodeWalker
24,840 dictionaries, 315,964 clips, 250,205 animations resolve across the
  whole install
```

**Two things worth knowing before touching this code:**

1. **The map entry names the animation, not the block.** An animation block has
   no hash of its own; the hash table entry pointing at it does, and a clip
   points straight at the block. The reference gets the link from a position
   cache inside `ReadBlockAt`; this reads the animation map first and keys the
   animations on their block address.
2. **The tag count is the second `u16` of `ClipPropertyMap`.** The first is the
   bucket capacity and is larger.

Not ported: the animation channel codecs, the tags and the properties, so there
is no playback and will not be until the skeleton lands too. Four destruction
`.ycd` do not read, for the same truncated-payload reason as the six `.ytd`.

### 1.4e Resources — bounds — **DONE, gate cleared**

Collision geometry. `.ybn` is self-contained like the four before it, and it is
what the drawable's `BoundPointer` and the fragments point at.

Files: `crates/cw-core/src/resources/bounds.rs`, gate at
`crates/cw-core/tests/bounds_gate.rs`, oracle at `scripts/bounds-oracle.ps1`.

Proven against the real install:

```
15 files, 85 geometries, 75,741 vertices and 127,345 polygons agree with
  CodeWalker field for field
126,942 flattened triangles match the oracle's Triangle polygons one for one
15,139 of 15,139 .ybn files parse across the whole install
  (56,773 composite children, 104,624,274 triangles)
```

**Four things worth knowing before touching this code:**

1. **The type byte rides inside a shared base.** `BoundsType` is the first byte
   of the 112-byte base struct; the reference dispatches on it by peeking
   sixteen bytes in. `Bounds` is one struct with a `BoundsData` tail, not a
   class hierarchy — every `Bound*` type is the base plus a tail.
2. **A polygon record keeps its type in the low bits of its first byte.**
   `ReadPolygons` masks byte 0 with `0xF8` before parsing, so the triangle's
   area field is read with three bits cleared. Triangle vertex indices carry an
   `0x8000` flag bit that must be masked off wherever a vertex is addressed.
3. **BVH lists come in two widths.** `Nodes` is the 32-bit `_b` list variant,
   read by capacity; `Trees` is the 16-bit `_s` variant with a pad word, read by
   count. Reading both as one shape silently mis-sizes the node list.
4. **Materials are padded to four on the way in and truncated back.** The array
   callers see is always the short one, never the padded one.

Not ported: writing, `BoundsDictionary`, the vertex *shrink* pass (editor-only),
and every `SphereIntersect`/`RayIntersect` collision routine in `World/`. The
viewer draws the triangles via `Bounds::triangle_mesh`, which pushes each vertex
through the composite transform chain the way `GetVertexPos` does.

### 1.4f Resources — fragments — **DONE, gate cleared**

`.yft` is self-contained like the five before it, and it is what the drawable's
`BoundPointer` and most of the game's prop, vehicle and ped models live in.

Files: `crates/cw-core/src/resources/frag.rs`, gate at
`crates/cw-core/tests/frag_gate.rs`, oracle at `scripts/frag-oracle.ps1`.

Proven against the real install:

```
8 files, 228 drawables, 13 renderable geometries agree with CodeWalker field
  for field across the whole physics LOD tree
61,430 of 61,430 .yft files parse across the whole install
  (223,359 embedded drawables, 459,734 geometries)
```

**Three things worth knowing before touching this code:**

1. **A `.yft` is a shell, not a drawable.** `FragType` holds a main
   `FragDrawable`, a cloth, an alternative array and a physics LOD tree whose
   children each carry up to two more drawables. `all_drawables` flattens that
   tree in the order the reference walks it, so the viewer draws a `.yft` the
   way `YftFile.Load` + `AssignChildrenShaders` do.
2. **`FragDrawable` is a `DrawableBase` plus a fixed 168-byte tail.** The base
   is the same 168 bytes a `.ydr`'s drawable opens with, so the geometry reader
   is reused verbatim; the tail adds the fragment matrix, a bounds pointer and
   optional transform arrays. Treating it as a full 208-byte `Drawable` would
   misread the tail as geometry.
3. **The physics chain is read through even when only the drawables matter.**
   The joint types dispatch on a byte at offset 0x15 of the block (0 → 1-DOF,
   1 → 3-DOF, anything else → null). Group names are ten little-endian uints
   packing forty ASCII characters.

Not ported: writing, the articulated body and joint structures beyond the type
dispatch, `EnvironmentCloth`, `LightAttributes`, and `FragPhysTransforms`.

### 1.3 MetaTypes — **DONE, gate cleared**

`meta/` ← `MetaTypes/`. All three container encodings and the walk that turns any
of them into named values.

Files: `crates/cw-core/src/meta/{mod,rsc,pso,rbf,tree}.rs` and
`crates/cw-core/src/utils/names.rs`, gate at
`crates/cw-core/tests/meta_gate.rs`, oracles at `scripts/meta-oracle.ps1` and
`scripts/metanames.ps1`.

Proven against the real install:

```
50 files across all three encodings and 30 root structures, 90,895 leaves
  agree with CodeWalker value for value
4,713 .ymt files, 14,217,919 nodes walk across the whole install
  (2,190 PSO, 2,326 RSC, 197 RBF, 42 distinct root structures)
```

**Four things worth knowing before touching this code:**

1. **`MetaTypes.SwapBytes(ulong)` swaps within each 32-bit half only**, under
   the reference's own `[not swapping 32bit blocks! careful!]`. Every PSO
   pointer goes through it, so getting it wrong sends every array to block 0.
2. **The files carry their own schema.** That is why `PsoTypes.cs` (1.5 MB) is
   not ported: only four nodes in two `.ymt` in the whole install need its
   fallback definitions. `MetaNames.cs` (870 KB) *is* needed, and is generated
   into `assets/metanames.bin` by `scripts/metanames.ps1` rather than
   hand-written.
3. **Floats follow `FloatUtil.ToString`**: seven significant digits if they read
   back, nine otherwise, with ties rounded away from zero. Rust's shortest
   round-trip prints eight and the reference never tries eight.
4. **Strings are decoded as UTF-8 here and as ASCII there.** The one deliberate
   divergence in the layer; see `formats.md`.

Not ported: writing (`MetaBuilder`, `PsoBuilder`), and the typed models above
the tree — those that have landed are below.

### 1.4g File types — Ytyp and Ymap — **DONE, gates cleared**

The typed models above the meta tree, taken next because Phase 3's world view
cannot place anything without them. Both build on a shared typed-reader layer
(`meta/typed.rs`), a port of the `MetaTypes.Get*` helpers that decode records
by the field offsets `MetaTypes.cs` documents rather than by the file's own
schema.

Files: `crates/cw-core/src/filetypes/{mod,ytyp,ymap}.rs`, gate at
`crates/cw-core/tests/ymap_gate.rs`, oracles at `scripts/ytyp-oracle.ps1` and
`scripts/ymap-oracle.ps1`.

Proven against the real install:

```
30 files, 118 archetypes agree with CodeWalker field for field
2,759 .ytyp files, 316,962 archetypes parse across the whole install
15 files, 3,681 entities and their hierarchy agree with CodeWalker
19,383 .ymap files, 3,074,696 entities parse across the whole install
```

**Four things worth knowing before touching this code:**

1. **The archetype array is a pointer array dispatched by target structure.**
   `YtypFile.Load` reads `CMapTypes.archetypes` as `MetaPOINTER[]`, then
   switches on each target block's `StructureNameHash` to build a `Base`, a
   `Time` or an `Mlo` archetype. An unknown target is skipped, the reference's
   `default: continue`.
2. **A `.ymap`'s entity array is two types in one list.** `CEntityDef` and
   `CMloInstanceDef` (interior instances) sit in the same pointer array, and
   `GetTypedPointerArray` filters them apart — order-preserving, null when
   empty. `parentIndex` indexes the *combined* list (CEntityDefs first), and
   the hierarchy pass treats an out-of-range parent as a root, exactly the
   reference's `EnsureEntities` rule.
3. **The typed layer decodes by offset, and a record can straddle a block.**
   `Marshal.PtrToStructure` checks only that a record starts inside its block;
   real `.ytyp` rely on it. `decode` reproduces the bounds rule — a short tail
   decodes zero-filled. See `formats.md`.
4. **Not all `.ytyp`/`.ymap` are RSC.** A handful of non-resource entries are
   RBF or PSO, which `YtypFile.Load`/`YmapFile.Load` handle through
   `NonMetaLoad` and yield no typed data; the gate skips them the same way.

Not ported: writing (the `MetaBuilder` side), so neither format round-trips yet.
The MLO *children* are read, but the entity-set → entity expansion that builds
world-space MLO entities (`MloInstanceData.CreateYmapEntities`) is editor logic
that belongs with Phase 3.

### 1.4 Resources, then FileTypes

The rest of `resources/` (Particle, ShaderParams, Clothes,
ResourceBaseTypes/Builder) — the shared building blocks. Then the remaining file
types in dependency order: `YndFile`, `YmtFile`, then the rest. `Gxt2File` is
done — see the Explorer viewer below.

Per format: a round-trip byte-equality test on a real fixture **plus** a JSON
`describe()` snapshot for the phase 2 viewers.

### 1.5 World domain models

`world/` ← `World/` (Entity, Space, Camera, Water, Weather, Timecycle, PopZones,
Scenarios, Trains, AudioZones, Heightmaps).

### Deferred

`Fbx`/`FbxConverter` (needs a Rust FBX writer), `Gen9Converter`, Enhanced-edition
RPF/Oodle, NG *encryption* (needs `RandomGauss.Solve`).

---

## Phase 2 — Explorer Mode

Game-folder picker → Rust-side virtual FS over `x64*.rpf` + DLC → lazy paginated
`list_dir` IPC → per-type viewers (texture via `bcdec` decode in Rust; model as
compact mesh buffers into three.js; meta/hex/text/GXT/audio) → extract to disk,
search by name and Jenkins hash.

Rule: heavy work in Rust threads, and **raw archive bytes never cross the IPC
boundary** for the frontend to parse.

### Done

**Browsing.** `src-tauri/src/explorer.rs` (`open_game_folder`, `list_dir`,
`game_is_open`), `src/lib/explorer.ts`, `src/components/Explorer.tsx`. Picking a
folder recovers the keys, scans every archive off the main thread, and the whole
install browses as one tree — nested archives included, with no visible seam at
an `.rpf` boundary. Gate at `src-tauri/tests/explorer_gate.rs`.

**Viewing.** `src-tauri/src/viewer.rs` (`open_entry`, `read_text`, `read_hex`,
`read_model`, `read_texture`, `read_clips`, `read_meta`, `extract_to`,
`save_text`), `src/lib/viewer.ts`,
`src/components/FileViewer.tsx` and `src/components/viewers/`. One file is open
at a time and its extracted payload is held on the Rust side, so paging a 20 MB
file does not re-decrypt and re-inflate it per scroll.

The backend picks which viewer opens, because that decision needs the payload:

- **3D** for `.ydr` and `.ydd`, via three.js over the mesh buffers from
  `cw_core::mesh`. Orbit, wireframe, LOD picker (High through VeryLow) and
  per-drawable selection for a dictionary. Positions, normals and texcoords are
  decoded in Rust; the WebView uploads buffers and nothing else. Parts bind the
  resolved diffuse/normal textures shipped as PNGs, with shadow-casting key
  light and a shadow camera fitted to the model's bounds. A part whose texture
  hash has no resolution falls back to the flat material.
- **Collision** for `.ybn`, via three.js over the triangle buffers from
  `cw_core::bounds::Bounds::triangle_mesh`. The whole composite tree is
  flattened to world space on the Rust side — every triangle, including the
  child transforms and each geometry's centre offset — and the toolbar shows the
  root type, the child count and the totals.
- **Map** for `.ymap` and `.ytyp`, the typed models above the generic meta tree.
  A `.ymap` shows the placement hierarchy `EnsureEntities` builds — every placed
  entity with its parent/child tree, resolved archetype names, LOD level,
  interior-instance badge and rig extensions, filterable by archetype — plus
  the car generators, occluders, LOD lights and grass batches that ride along.
  A `.ytyp` lists the archetypes discriminated by kind, MLO interiors broken
  down into their rooms, portals, entity sets and entities. Hashes are resolved
  to names in Rust via the metanames table; the generic XML tree stays one tab
  over for the byte-for-byte truth.
- **Texture** for `.ytd`. The dictionary is listed on open and one texture is
  decoded at a time, to PNG — a 2048² surface is 16 MB of RGBA and 22 MB once
  base64'd, so PNG is faster to ship than raw pixels and the WebView decodes it
  natively. Mip picker, an alpha checkerboard, and export as `.dds` (the stored
  mip chain, lossless) or `.png` (the mip on screen).
- **Audio** for `.awc`. Streams are listed on open and decoded one at a time —
  a container runs to hundreds of them — then played as WAV in an `<audio>`
  element, or saved to disk. ADPCM is decoded in Rust; no browser plays
  Rockstar's framing.
- **Strings** for `.gxt2`. Every hash → text pair is listed, filterable, with
  the hash shown so a string can be matched against a script or a mod. The
  format round-trips byte for byte, the first file type that does.
- **Text** for anything that sniffs as text, whatever its extension — which is
  most `.meta`, `.xml`, `.ini` and a fair number of `.dat`. Editable, with the
  encoding preserved on save.
- **Hex**, paged, always available for anything at all.

A file the extension says is a model but that will not parse falls back with the
reason attached, and the formats that have no parser yet each say so rather than
opening as unexplained bytes.

**Extraction.** Any entry to disk, decrypted and decompressed. Saving edited
text writes a copy to disk, *not* back into the archive: RPF writing is not
ported, and the UI says so rather than offering a save that would quietly do
something else.

### Next

- **Writing back into an archive.** The blocker for a real editor. Needs
  `RpfFile.Write`, `CompressBytes` and NG *encryption* (`RandomGauss.Solve`).
- **Textures on models — DONE.** The 3D viewer resolves each part's material
  slots (`DiffuseSampler`/`BumpSampler`/`SpecSampler` from `ShaderParams.cs`)
  through `read_shader_group`'s parameter list, then walks the reference's
  dictionary chain — embedded `ShaderGroup.TextureDictionary`, same-basename
  `.ytd`, then every `.ytd` in the model's folder — decodes the resolved texture
  in Rust, and ships it to the viewer as a PNG. `shader_gate.rs` pins the
  parameters and embedded dictionaries against the oracle. LOD switching (High
  through VeryLow) and shadowed key lighting came with it.
- **Fragments — DONE.** `.yft` embeds drawables the 3D viewer draws via
  `Frag.cs`, each named by where it sits in the physics LOD tree. **Particles**
  are the remaining embedder: `.ypt` needs `Particle.cs`.
- **Audio names.** `RelFile.cs` would give the streams in an `.awc` names
  instead of hashes, and pair `_left`/`_right` channels into stereo.
- **Lazy pagination.** `list_dir` still returns a whole directory. Fine at the
  sizes measured, but the phase 2 brief asks for paging.
- **Search** by name and Jenkins hash across the 403,642-entry index.
- **DLC/mod precedence.** `RpfManager` deliberately keeps one flat index and lets
  later archives win a path collision. The reference maintains six overlapping
  lists to resolve base/mod/DLC override order; that work belongs with
  `patchday*` selection in phase 3.

## Phase 3 — World View

wgpu into a native surface via `raw-window-handle`, WebView as transparent UI
overlay. Spike capped at two days; three.js is the documented fallback. Then Rust
LOD streaming (ORPHANHD → HD → LOD0-4), HLSL→WGSL translation with a fidelity
table, WASD camera, raycast selection, gizmos, undo/redo, `.cwproj`,
`patchday*` DLC selection.

### 3.0 Renderer spike — **decided: three.js in the WebView**

The wgpu-native-surface spike concluded against the native route: Tauri v2 has no
native-only window, so the surface would sit in a `tao` window beside Tauri's own
event loop with a transparent WebView tracking it — two windowing worlds in one
process, unsupported event-loop mixing, and known WebView-over-GPU compositing
problems on Windows. The fidelity ceiling does not earn a modding tool's debug
view. The decision and its consequences are recorded in `architecture.md` under
**Rendering**. `crates/cw-render` is not a wgpu crate; world rendering is three.js
in the WebView on the same Rust-prepared-buffers boundary as the Explorer.

### 3.1 World mode foundation — **DONE**

Files: `src-tauri/src/world.rs`, `src/components/World.tsx`,
`src/lib/world.ts`, gate at `src-tauri/tests/world_gate.rs`.

- **Content groups.** `world_status` groups every archive by which content it
  belongs to: `base` for the root `x64a..w.rpf` set, one group per DLC pack for
  `update\x64\dlcpacks\*` and `x64\dlcpacks\*` (the `patchday*`, `mp*` and
  `sport` packs), and the directory for anything else. Each group reports its
  archive and entry counts, so the UI can offer base/mod/DLC override selection
  without scanning anything heavy.
- **Map placement.** `world_map` reads any `.ymap` by archive path straight from
  the index — it does *not* clobber the Explorer's one-open-file viewer state —
  and returns every placed entity flattened to world space: position, the stored
  quaternion, per-axis scale, LOD level, parent link and child count. The
  frontend renders each map as instanced boxes in a Z-up→Y-up rotated world
  group, coloured by LOD level (MLO a different hue), and raycasts to select.
- **Camera and selection.** A WASD fly camera (drag to look, `Shift` to sprint,
  `Space`/`Ctrl` up/down, wheel to dolly) over a grid, with click-to-select
  showing the entity's archetype, position, LOD, priority and flags. Double-click
  frames the selected entity.
- **DLC/patchday selection.** The sidebar lists the content groups; ticking one
  lists its `.ymap` files (capped at 200, sorted) and ticking a map places it.
  Maps placed are listed with a remove control. When no game folder is open, the
  mode shows a synthetic demo street so the pipeline is visible and testable
  without the game.
- **Place-the-open-map.** If a `.ymap` is open in Explorer, `world_status`
  reports it and the world view offers to place it directly.

Not yet: whole-install streaming (loading maps as the camera moves), gizmos,
undo/redo, `.cwproj` projects, and drawing real geometry instead of placement
markers. The MLO entity-set expansion (`MloInstanceData.CreateYmapEntities`)
still belongs here rather than in the readers. The stored entity quaternion is
passed through as the file carries it; the gate should confirm orientation
matches the reference before real geometry is drawn.

### 3.2 Real geometry and LOD streaming — **DONE**

The next slice of the phase brief, after the camera and selection: place real
models instead of boxes, and stream them by camera distance the way the engine
does (`ORPHANHD` → HD → LOD0-4, here as the drawable's own High→VeryLow LODs).

- **Archetype → asset resolution.** `world_asset` finds the model file for a
  placed entity's resolved name — a `.ydr`, `.yft` or `.ydd` whose basename
  matches, preferring an `update\` copy over the base game (a first pass at
  the reference's content precedence) and `.ydr` over `.yft` over `.ydd`.
  Names the name table cannot resolve have no file and are skipped, not failed.
- **Geometry by path.** `world_model` reads any model file into mesh bundles
  (every LOD) straight from the index, again past the viewer's one-open-file
  state, so several entities can stream at once. Textures are not resolved yet —
  streamed models render flat, on a neutral grey.
- **Streaming.** A `Stream HD geometry` toggle loads the nearest unplaced
  entities within a radius of the camera, up to a batch per pass, and unloads
  them again beyond an outer radius. An asset cache and a model cache mean a
  street of identical props resolves and decodes once, not per instance. The
  selected entity gets a `Show model` / `Hide model` button whether or not the
  streamer is on.
- **LOD by distance.** Each streamed entity's drawable LODs are uploaded once
  and swapped per frame by distance to the camera, using the entity's own
  `lod_dist` as the HD boundary (`lodBandForDistance`). The marker box for an
  entity with geometry steps aside (its instance matrix is zeroed) and comes
  back when the entity is unloaded.
- **Raycast through geometry.** A loaded entity is selectable through its real
  triangles, not just its (hidden) box. Clicking still ignores a drag so
  rotating the camera never deselects.

The demo plaza has no assets to stream, so it stays boxes — the streaming
pipeline is exercised only against a real install (gate: `streams_an_entitys_
archetype_to_real_geometry`).

Not yet: model *textures* in the world (flat materials while streaming), the
full streaming scheduler with per-map load regions, gizmos, undo/redo, and
`.cwproj`.

### 3.3 Textures on streamed models — **DONE**

The flat-shaded caveat of 3.2, closed. A streamed model now brings its pixels.

- **Resolution, bounded.** `read_world_model` resolves each part's texture
  hashes against the drawables' embedded dictionaries, then the sibling `.ytd`
  (same folder and basename) found by one index lookup — deliberately skipping
  the Explorer's third step, a full-tree scan of every `.ytd` in the model's
  folder, which is too expensive per streamed model. Most stock pixels live in
  the sibling anyway.
- **Decode in Rust.** Each resolved texture is decoded to a PNG on the Rust
  side (the boundary rule), so the WebView only ever uploads image bytes, never
  BC blocks.
- **Materials.** Each part binds its diffuse and normal maps onto a
  `MeshStandardMaterial`, falling back to the flat grey when a hash resolves to
  nothing — the same rule as the Explorer's model viewer. A session-wide
  texture cache keeps repeated decodes off the main thread.

The gate now asserts a real invariant: a model whose parts reference a diffuse
texture and whose sibling `.ytd` exists must resolve at least one texture.

Not yet: the full streaming scheduler with per-map load regions, gizmos,
undo/redo, and `.cwproj`.

### 3.4 Placement editing: move gizmo and undo/redo — **DONE**

The first editor slice of the phase: drag a placed entity instead of just
looking at it.

- **Edits overlay.** Every drag writes a `TransformEdit` into a keyed map that
  the canvas lays onto the entity each frame — onto its geometry holder if the
  model is streamed, onto its box instance otherwise — so nothing is rebuilt
  while dragging and the placement source data stays untouched. The selection
  outline and the details panel follow the edit live.
- **Move gizmo.** Selecting an entity shows three coloured axis lines at its
  position (the game's own X/Y/Z, in the Z-up world). Axis picking is done in
  screen space, so a handle is grabbable at any zoom; dragging projects the
  pointer onto a camera-facing plane and moves the entity along that axis. Left
  drag grabs a handle, or falls back to rotating the camera.
- **Undo/redo.** Each completed drag is one history entry (`before`/`after`),
  popped by the buttons or Ctrl+Z / Ctrl+Y. The redo stack clears on a new drag,
  the way editors behave.
- **Testable core.** `applyEdit`/`editOf`, history push/pop and the gizmo axis
  math are pure functions in `src/lib/world.ts`, covered by unit tests; the demo
  plaza is editable, so the whole loop works without the game.

Not yet: rotate and scale gizmos (the phase's plural "gizmos" is a move gizmo
for now), the full streaming scheduler with per-map load regions, `.cwproj`
projects, and the MLO entity-set expansion.

### 3.5 Rotate and scale gizmos — **DONE**

The editing trio from the phase brief, completed: selecting an entity shows a
Move / Rotate / Scale choice, and each mode has its own handle set.

- **Move and scale** share the three axis handles. Scale drags grow or shrink
  `scaleXy` (the game's uniform ground-plane scale, for either the X or Y
  handle) or `scaleZ`, clamped above zero, by how far the pointer travels
  relative to the handle length.
- **Rotate** swaps the axis lines for three coloured rings, one per axis, in
  the game's Z-up space. Picking is by raycast against a fat invisible twin of
  each ring so a thin visual is still grabbable; dragging projects the pointer
  onto the ring's plane and turns the angle delta into a world-space rotation
  about that axis. The angular accumulation handles the seam where `atan2`
  wraps.
- **Everything else holds.** Each drag is one undo/redo step, edits overlay the
  placement live (box instance or geometry holder), the outline and details
  panel follow, and the pure `axisRotate` helper is unit-tested (identity, a
  quarter turn about Z, unit length).

Not yet: the full streaming scheduler with per-map load regions, `.cwproj`
projects, and the MLO entity-set expansion.

### 3.6 `.cwproj` projects — **DONE**

The world editor gets a saveable session: the placed maps and every edit,
written to a `.cwproj` file and loaded back in another session.

- **Format.** A JSON document `{ format: "astramapper", version: 1, maps:
  [archive paths], edits: { entity key: transform } }`. This app's own shape —
  CodeWalker's `.cwproj` is an editor session file with no stable on-disk
  format to copy. `serializeProject` / `parseProject` are pure and tested,
  including rejecting foreign formats, unsupported versions and malformed
  edits.
- **Save / Load / New.** The World sidebar's Project section writes the current
  placement + edits through a save dialog, loads one back (re-placing the maps
  in parallel and layering the edits on), or clears the world. Loading tolerates
  a map that no longer exists in the install — it reports how many failed
  rather than failing the whole project.
- **IPC.** Two thin Rust commands, `project_save` and `project_load`, write and
  read the file so the frontend never touches disk IO itself.

Not yet: the full streaming scheduler with per-map load regions, and the MLO
entity-set expansion.

### 3.7 Per-map load-region streaming — **DONE**

The ad-hoc nearest-entity loop becomes a real scheduler, still one pass every
`STREAM_INTERVAL_MS` but deciding from the maps' own data.

- **Load regions.** A map streams only while the camera is inside its declared
  streaming box (`streamingExtents`), with a margin, so content the camera has
  left is dropped wholesale instead of lingering. That is the "per-map load
  region" of the phase brief.
- **Entity tiers.** Each entity's geometry streams within a radius derived from
  its own `lodDist` and its placement LOD level — `ORPHAN_HD` nearest, then HD,
  LOD, and the SLOD fills farthest — the `ORPHANHD → HD → LOD0-4` idea mapped
  onto load *radius* rather than just the per-frame drawable-LOD swap from 3.2.
  Unloading uses hysteresis (1.5× the radius) so a camera parked on a boundary
  does not thrash.
- **Pure and tested.** `streamingDecision` is a pure function over
  `{ extents, placements }` regions taking the camera, the loaded and pending
  key sets and a batch limit, returning what to load and what to unload. It is
  unit-tested: nearest-first batching, hysteresis unload, out-of-region drop,
  and never re-queuing a loaded or pending entity.

Not yet: the MLO entity-set expansion. The pass itself was rewritten in 4.12 —
same rules, ordered by where the camera is looking, and no longer scanning every
placement in the world to decide.

### 3.8 MLO entity-set expansion — **DONE**

The last item the phase brief deferred here. An MLO interior instance in a
`.ymap` is a position, a rotation and an archetype hash; the interior's actual
contents live in the MLO archetype's entity list inside a `.ytyp`, in the
interior's local space.

- **Expansion.** `cw_core::filetypes::mlo` ports `MloInstanceData.CreateYmap
  Entities`: each interior entity is placed at `world = R * local + T` with the
  combined rotation `R_instance * R_local` — no instance scale, matching the
  reference. The math is pure and unit-tested (identity, translation, a quarter
  turn about Z mapping +X to +Y, and a unit-length combined rotation).
- **Archetype resolution.** `index_mlo_archetypes` scans the `.ytyp` files once
  and keys every MLO archetype by its lookup hash — the same value an
  instance's `archetypeName` carries. `Loaded` keeps the index, so placing
  several interior-bearing maps does not re-scan 2,759 `.ytyp` files. A map
  with no MLO instances never pays for the scan.
- **World placement.** `world_map` expands each MLO instance's interior into the
  flattened entity list as `fromMlo` entities — real world-space positions,
  rotations, scales and LOD distances — so they render, stream by LOD and edit
  exactly like any other placed entity, with an "interior entity" badge in the
  selection panel. The gate (`expands_an_mlo_instances_interior_into_the_world`)
  asserts a city map's interiors expand, name their archetypes and sit at
  finite positions.

Phase 3 is complete against the brief: the renderer is settled, the world
browses, streams, textures, edits with undo/redo, saves to `.cwproj`, and now
shows interiors. What remains in the roadmap is the Explorer's writing work
(`RpfFile.Write`, NG *encryption*) and the phase's anti-goals.

---

## Phase 4 — World performance, shaders and tooling

Phase 3 made the world view work. It does not scale: nothing is cached to disk,
so every launch re-scans 179 archives and 403,642 entries; every command is a
sync `#[tauri::command]` on the event loop; and there are no shaders, just one
`MeshStandardMaterial` per part per LOD.

The phase is measurement-led. Three findings so far each corrected the plan it
came from, and each is recorded where it landed rather than where it was
guessed.

### 4.1 I/O and threading — **DONE**

- **Archives are read through a memory mapping** (`rpf/mapped.rs`), keyed by
  physical path because 5,199 nested archives resolve to 179 files. Capped at 64
  live mappings with LRU unmap plus a "Release archives" action, because a
  mapping holds a Windows file lock and the user may want to patch the game.
- **Measured 1.13x**, not the several-times win the plan assumed. At 7.5 GB
  decompressed across 4,000 reads the run is dominated by deflate at ~380 MB/s,
  so `File::open` per extraction was never the bottleneck. **This is why the
  disk cache is the centrepiece of the phase and not an optimisation on top of
  one.**
- Every world command that touches an archive is `async` and runs its body in
  `spawn_blocking`. The invariant: no lock guard crosses an `.await`, which is
  enforced by `GameState` handing out an `Arc<Loaded>` rather than a guard.
- The `.ytyp` archetype scan is rayon, and now indexes every archetype rather
  than only MLO ones.

### 4.2 Textures resolve through the archetype — **DONE**

Supersedes 3.3's resolution rule. `read_world_model` used to extract, inflate
and fully parse **every** `.ytd` in a model's folder: 28.8 dictionaries for the
average model in a sample of 400, and 332 dictionaries, 1 GB and 3.2 seconds for
the worst, to draw one model.

An archetype names the dictionary it wants, and 158,574 of the install's 159,375
archetypes name one. The chain is now embedded → the archetype's
`textureDictionary` → the sibling `.ytd` → the folder scan as a last resort,
kept because a modded archetype may name nothing. Measured **1.8x** over 58
placed city models with identical texture coverage, and that is a lower bound:
the archetype path runs first and pays the cold cost.

**GTA ships base and HD texture dictionaries side by side**, `foo.ytd` next to
`foo+hi.ytd` / `foo+hidr.ytd`, same texture names at different resolutions,
swapped at `hdTextureDist`. The folder scan took whichever tier its ordering
reached first. The archetype names the base tier, so the choice is deterministic
now, and this is the mechanism LOD-aware texture selection should hang off
rather than anything invented.

### 4.3 Binary IPC: textures as GPU blocks — **DONE**

Supersedes 3.3's "the WebView only ever uploads image bytes, never BC blocks".

Textures crossed as base64 PNGs: decode the BC blocks to full-resolution RGBA,
deflate that into a PNG, base64 it, JSON-escape the base64, and on the far side
inflate it again so the driver could re-compress nothing. Over 600 real textures
that costs **12.92s and 443.9 MB of intermediate RGBA that is thrown away**;
copying the stored mip chain costs **19.58ms**.

So the streaming path ships the blocks, in a binary envelope
(`crates/cw-core/src/wire.rs`): JSON metadata in front of 16-byte-aligned raw
sections, returned through `tauri::ipc::Response`. `docs/architecture.md`
defends the boundary-rule change; the short version is that BC blocks are the
GPU's own format rather than an archive format, and the WebView parses nothing.
The frontend declares its context's extensions through `set_gpu_capabilities`
and anything unsupported is decoded to RGBA in Rust — still not a PNG.

End to end over 58 placed city models: **12.62s to 2.87s and 86.9 MB to
61.1 MB**, with the envelope carrying every mip against the PNG path's mip 0
alone. The blocks are 93.3% BC1 and 6.5% BC3. VRAM drops to roughly a quarter.

The gate (`the_block_path_resolves_the_same_textures_as_the_png_path`) checks
every mip's byte length against `TextureFormat::compute_pitch`, which is the
arithmetic the driver does. The frontend uploads on the metadata's word and has
no way to check it, so this is where it gets checked.

Three fidelity fixes fell out of writing the upload path: diffuse uploads as
sRGB and normal maps stay linear (both were linear, which washes the scene out);
`normalScale.y` is negative, the shader-side equivalent of the green-channel
inversion the Explorer's viewer does on the CPU for RAGE's left-handed bump
maps; and textures wrap rather than clamp. `CompressedTexture` also forces
`flipY` off, which the Explorer's viewer already documents as correct for GTA's
Direct3D UVs — the world view's PNG path had it on and was mirroring every
texture vertically.

### 4.4 Geometry in the envelope — **DONE**

Vertex and index buffers are envelope sections rather than base64 strings, and
indices narrow to `u16` when a part indexes fewer than 65,536 vertices. Every one
of 361 parts across 58 placed city models did, which halves the index payload.

Measuring first changed what this commit was for. Over 400 model files and 3,382
parts, base64 encoding costs **0.2ms per file** — so the Rust-side CPU argument
was weak, and the wire alone is only 1.33x smaller. The real cost was the
frontend, which copied every buffer three times on the main thread before three.js
saw it. It now takes typed-array views straight over the IPC response.

Like for like over those 58 models, geometry and textures together: **12.97s to
2.90s, 108.3 MB to 54.1 MB**. Exactly 2x the wire.

`cw_core::mesh::Buffer` is what lets both transports share one flattening: it
serializes as base64 for the Explorer's JSON commands and hands out its bytes for
the envelope.

### 4.5 The cache's storage layer — **DONE**

`crates/cw-cache` holds keys, pack files and the index journal. Three decisions,
each argued in its module header:

- **Keyed by source, not content.** Content-addressing means extracting to find
  out whether extraction was needed, which is the work being avoided. The cost is
  that every input which could change the output must be in the key, and the
  rule is: when in doubt, put it in.
- **Pack files with a sidecar index.** One file per asset is 400,000 files on
  NTFS for Defender to scan. A key-value store is a large dependency and a second
  failure mode for regenerable data. One flat file cannot evict.
- **Whole-pack eviction.** Append-only packs cannot reclaim a hole without a
  compactor, and a compactor is a background rewrite of gigabytes that can
  corrupt the store, to recover space that costs nothing to leave alone.

The crate depends on neither `tauri` nor `cw-core`: it stores opaque bytes under
an opaque key, which is what makes the round trip testable with no app and no
install. `src-tauri` resolves `app_cache_dir()` and passes it in.

Surviving a kill mid-bake is treated as the normal case. Nothing fsyncs, journal
replay stops at the first torn record, and any record whose bytes do not fit the
pack it names is dropped on open. Both halves have tests.

Archive stamping landed with it (`cw_core::rpf::stamp_file`,
`RpfManager::archive_stamps`): length and mtime, mixed, with a nested archive
inheriting its top-level ancestor's stamp. That inheritance is the point of
stamping per archive rather than per install — **patching `update.rpf` must
invalidate what came out of it and not the other 178.**

### 4.6 The `Cache` facade, and models served from it — **DONE**

`Cache::open` checks a manifest and **wipes on a format or install mismatch**.
There is no migration path and there should not be one: a migration is a bug
surface with a silent failure mode, guarding data that costs one bake to rebuild.

`world_model` goes through `get_or_insert_with`, so the on-demand and the
background paths are one line and cannot drift. **Measured over 58 placed city
models: 4.26s cold, 55ms warm, 90.6 MB stored.**

The rule the facade is built around: **a cache failure may never fail the
operation that asked for it.** `get` returns `Option`, write failures land in a
counter, and a poisoned lock is taken anyway rather than propagated — a panic in
one thread must not disable caching for the session.

Two things worth knowing about the implementation:

- **Every pack is mapped at open**, not on first read. A pack that is not mapped
  cannot be an eviction candidate, so lazy mapping would leave the budget
  enforced against the one pack being written.
- Reads and appends use separate handles, so a record written after a mapping was
  taken falls outside it. `get` notices, remaps once and retries. That is also
  exactly what a torn append looks like, and it resolves the same way: a miss.

**GPU capabilities are in the key.** The same texture compiles to different bytes
on a machine without BPTC, and two GPUs must not read each other's artifacts.

The cache is owned by `explorer::Loaded` rather than held in its own Tauri state,
because its contents are scoped to one install. That removes the state where an
open cache and an open game folder disagree about which folder that is.

`cache_gate.rs` pins the assertion that matters: **byte identity between what the
compiler produced and what a reopened cache serves.** A cache that serves nearly
the right bytes is worse than no cache, because the failure is silent and looks
like a parser bug. Invalidation is not gated there on purpose — proving it means
writing to the game folder, and the three unit tests it decomposes into already
pin it.

### 4.7 The background bake — **DONE**

**By reachability, never by enumeration.** A naive full bake is tens of gigabytes
of peds, vehicles, weapons and cutscenes; this one considers only models a
`.ymap` entity's archetype names, which falls out of walking the maps.

**Ordered by placement frequency, descending**, which is what makes a bake
cancelled or budget-stopped halfway still worth having. Measured over the base
game by `bake_gate.rs`:

| | |
| --- | --- |
| Distinct reachable models | 56,166 |
| Placements of them | 692,953 |
| Share of the install's 403,642 entries | 13.9% |
| Walk time | 2.1s |

| Models baked | Share of all placements covered |
| --- | --- |
| top 100 | 55.5% |
| top 500 | 75.9% |
| top 1,000 | 83.6% |
| top 5,000 | 92.5% |

At the budget the pass **stops enqueueing rather than evicting**: a bake that
evicted its own earlier work would run forever and finish holding whatever it did
last.

Its rayon pool is sized `(cpus - 1).max(1)`, so it cannot starve the model the
user is waiting on, and progress is coalesced to 4 Hz because one event per item
at this scale would cost more than the bake. `open_game_folder` cancels it but
does not join it — the pass holds an `Arc<Loaded>`, so the install and the cache
it writes to outlive the folder change.

The pass itself is not gated: driving it needs a `tauri::ipc::Channel`, which
needs a running app. Its state machine is unit tested and its reachability walk
is gated.

### 4.8 Preloading a placed map — **DONE**

The streamer answers "what should be resident given where the camera is". The
Preload panel answers "what would it take for all of it to be resident", which
is a different question with a different failure mode: the streamer is bounded
by a radius, and **a preload is bounded only by its cap**.

So the cap is the safety mechanism, and the order decides what survives it —
nearest to the camera first, so a cap that bites keeps what is in front of you.
It defaults to 5,000 and is shown against the real placed count, because with
the base game placed that count is about 693,000 and the renderer still builds
one `THREE.Group` of meshes per entity. Nothing near that will render until
archetypes are instanced.

Starting a preload **turns streaming off**, deliberately: the streamer unloads
anything past its radius, so leaving it on would tear out what the preload just
loaded as soon as the camera moved.

This forced stage 5.2 forward. Loaded geometry now lives in
`src/lib/world/geometryStore.ts` rather than a `useState` array, because
`loadEntity` used to scan the array to deduplicate, copy it to append, and list
it in its dependencies, and the streaming interval listed it too and was torn
down and rebuilt on every single load. **Loading n entities was O(n squared) in
three separate places.** Streaming hid it by never loading more than a couple of
hundred at once.

Store notifications are **coalesced to one per frame**, which is the point rather
than an optimisation: without it a preload rebuilds the renderer's scene
bookkeeping once per model, and the quadratic cost moves out of React into
three.js instead of going away.

### 4.9 The renderer side of the same quadratics — **DONE**

4.8 moved the loaded set out of React and coalesced its notifications to one per
frame. That fixed the React half and left the three.js half, because **one
notification per frame is only cheap if what the renderer does per notification
is proportional to what changed.** Three things were not, and a preload runs
hundreds of passes, so each of them was quadratic over the preload:

| Per pass, 5,000 loaded | Before | After |
| --- | --- | --- |
| Marker-box restore | 203ms | 0.12ms |
| Raycast list rebuild | 5.9ms | removed |
| LOD swap (per frame) | 0.38ms | 0.11ms |

Over a 5,000-entity preload at 12 per batch — about 417 passes — the box restore
alone was **84 seconds of pure bookkeeping**, and the raycast rebuild another
2.4. Measured on the shapes these passes actually walk, at the base game's
693,000 placements.

**The box restore swept every placement in the world to find the few that
changed.** It ran over every entity of every placed map, asking whether it was
loaded, and resolved each one with `instances.indexOf` — a linear scan inside a
linear scan. An entity's marker only needs restoring at the moment its geometry
unloads, and the sync pass already knows exactly which those are.

Two bugs fell out of rewriting it. The marker index was built from **the first
colour bucket only**, and boxes are bucketed by LOD colour, so every entity in a
later bucket kept its marker box when its model loaded and ignored its own
edits — `indexOf` returned -1 and the write was silently skipped. Keying every
bucket by entity index fixes both. And the raycast tested **every LOD bundle,
including the hidden ones**, so a click could select through a model to a LOD
that was not on screen; picking now walks the loaded set directly and tests only
the visible band, which is what let the flat list go entirely.

**`lodBandForDistance` is cheap and writing `visible` is not.** The swap
recomputed the band for every loaded entity every frame — correct, and worth
keeping — and then wrote `visible` on every mesh of every bundle whether or not
the band had moved. 14.4 million property writes over 120 frames became 129,000,
because a model almost never changes band between two frames.

Underneath all three is one lookup. `(mapKey, entityIndex)` → placement was
`entries.find(...)` then `placements.find(...)` at seven call sites, three of
which run per frame while a gizmo drags. `src/lib/world/placementIndex.ts`
builds it once per placement change: **17.8us to 0.058us, 309x**, for 55ms of
build. It is nested rather than keyed by `keyFor` on purpose — a flat map would
hold 693,000 concatenated strings. For the same reason the reverse map from key
to identity is gone; `parseKey` splits on the **last** separator, because a map
key is an archive path and carries colons of its own.

None of this raises `PRELOAD_CAP`. The cap stands where 4.8 put it, and for the
reason 4.8 gave: one `THREE.Group` of meshes per entity. What changed is that
the cost of a preload is now the models it builds, not the bookkeeping around
them.

### 4.10 Where launching actually goes — **MEASURED**

The phase opened by naming the cold launch as a headline cost: *"every launch
re-scans 179 archives and 403,642 entries"*. Measured against the real install,
in release, that whole path is **570ms**:

| Step | |
| --- | --- |
| Read `GTA5.exe` (45 MB) | 11ms |
| Recover the keys | 138ms |
| `RpfManager::scan` — 5,378 archives, 403,642 entries | 242ms |
| `archive_stamps` | 5ms |
| Archetype index — 159,375 archetypes | 136ms |

**So serializing the install-scoped indexes into the pinned pack is not worth
building.** It was carried as the next perf item on the assumption the scan
dominated; it does not, and the ceiling on the whole idea is under half a second
against a format, an invalidation rule and a new silent-corruption surface. It
comes off the list rather than staying on it as something to get to. (Caveat
that belongs with the number: this is a warm OS file cache. A first launch after
a reboot pays real disk reads for 179 archive headers on top.)

What entering World Mode costs is a different thing entirely, and it is much
larger:

| Step | |
| --- | --- |
| List the base group | 2.8ms for 4,601 maps |
| Read and flatten every one | **2.0s** for 763,716 placements |
| Serialize those placements to JSON | 313ms for **204.1 MB** |

`world_map` was a plain `#[tauri::command]`, so **every placement crossed the
IPC boundary as JSON text and was parsed by the WebView** — 204 MB of it. That
is the same shape of problem 4.3 and 4.4 found for textures and geometry, on the
one payload that was never moved into the envelope. Closed in 4.11 below.

The frontend's own dedup of that list was quadratic and is fixed here:
`loadBaseMaps` checked `next.some(m => m.path === ...)` per map, ten million
path comparisons to merge a list that is already distinct.

### 4.11 The placement envelope — **DONE**

Placements move into `cw_core::wire` as columns, one section per field, with the
two strings — the archetype name and the LOD label — deduplicated into tables in
the JSON metadata and pointed at by an integer column.

Over the base game's 4,597 readable maps and 763,716 placements:

| | JSON | Envelope | |
| --- | --- | --- | --- |
| On the wire | 204.1 MB | 51.4 MB | 3.97x |
| Rust-side write | 313ms | 82ms | 3.80x |
| Frontend decode | 745ms `JSON.parse` | 88ms | 8.45x |

**Most of the JSON was not data.** A placement is thirteen fields, and JSON
repeats every field *name* per entity — roughly 120 bytes of keys against 60
bytes of values. Columns delete the keys outright, which is where the 4x comes
from; the two string tables are what stops the archetype names from coming back
in through the side door.

The frontend decode still **materializes one object per placement**, because
`Placed` is an object with tuple fields and the whole renderer reads it that
way. That is the remaining 88ms and it is deliberate: the win here is deleting
204 MB of text and the parse over it, not the object graph, which only a
columnar `Placed` would remove and which would touch every reader in
`World.tsx`.

**`nameHash` is not derived from `name`, even though it nearly could be.** The
name table is keyed on the pair, so one name reached through two archetype
hashes gets two rows. Deriving it would save four bytes an entity and would make
an entity silently inherit a neighbour's archetype hash the day the assumption
broke, which is not a failure anyone finds by looking at the screen.

Both halves are written from the documented layout and each is unit-tested
against a fixture it built itself — which proves each half self-consistent and
nothing about the pair. `the_placement_envelope_round_trips_a_real_map` is what
pins them together: real maps through the encoder, decoded at the same column
offsets the frontend uses, asserted field for field.

### 4.12 The streaming pass, and what issue #1 actually asked for — **DONE**

[Issue #1](https://github.com/adastra-org/astramapper/issues/1) asked for three
things: sphere loading in an area around the camera, lower LOD quality with
distance, and visual culling of the FOV. **Two of the three were already
built.** The sphere is 3.7 (`entityStreamRadius` per LOD tier, nearest first,
hysteresis unload) and the distance-driven LOD is 3.2 (`lodBandForDistance`
swapping the drawable's own bundles per frame). Frustum *rendering* culling was
already there too, from three.js, which culls every object by its bounding
sphere without being asked.

What was genuinely missing is the third thing pointed at streaming rather than
rendering: **the streamer was direction-blind.** It filled its batch of 24 from
the nearest unloaded entities in every direction, including behind the camera.
Measured in the steady state — thousands resident, so the unloaded frontier is a
shell rather than a puddle at the camera's feet:

| Batch of 24 | On screen |
| --- | --- |
| Nearest-first | **5 of 24** |
| View-ordered | **24 of 24** |

Nineteen of every twenty-four slots were spent on geometry behind the camera.

**It orders, it does not cull, and that is the whole design.** The candidate set
stays the sphere and the unload rule is untouched. Culling the candidates to the
frustum would mean turning around to find nothing there, followed by a reload
storm as the same geometry streams back in — which is worse than the problem.
Ordering cannot pop, costs nothing measurable, and with a batch of 24 against a
sphere holding thousands, order *is* what a user experiences as load speed.

The cone is 60 degrees off axis rather than the six real frustum planes. It
decides order, not visibility, so being generous costs only that something just
off screen loads slightly sooner than it had to, while being exact would couple
a pure function to the projection matrix and the aspect ratio. Inside
`STREAM_VIEW_NEAR` everything counts as on screen: a cone has nothing at its
apex, and without that rule the models the camera is standing among score as
behind it.

**Measuring it first found something bigger.** With the base game placed, one
pass was **76ms, four times a second, on the main thread — 30% of a core, in a
`setInterval`, as 76ms of jank every 250ms.** Three causes, all of them the
function scanning what it did not need:

- it walked every placement of every *out-of-range* map, computing a distance
  and a key, only to discard them. An out-of-range map's unload list is exactly
  what it has loaded, so it never needs to look at a placement.
- it built a `keyFor` string per placement per pass — 763,102 of them — to ask a
  set membership question. The loaded and pending sets are now regrouped once
  per pass into `mapKey → Set<entityIndex>`, so the inner loop compares integers.
- it pushed every in-radius candidate into an array (about 46,000), sorted it,
  and took the first 24. The top of the batch is now kept by insertion as the
  scan runs, behind a squared-distance reject that avoids the square root too.

**76.3ms to 0.8ms cold, 85.5ms to 2.4ms in the steady state.** The view cone
rides along for free — 2.1ms against 2.4ms without it, which is noise.

### Still to come

Reading the maps themselves, which is now the whole cost of filling the world:
2.0s of the 2.1s that remains. It is 4,597 separate `world_map` calls, each one
extracting, inflating and walking a `.ymap` from scratch on every launch — and
unlike the launch scan, this one is worth putting in the cache, because the
measurement says so rather than the plan.

Archetype instancing, which is what `PRELOAD_CAP` is really waiting on: entities
sharing an archetype should share one draw call instead of a group each.

The `Canvas` setup effect, which rebuilds the renderer, the scene and every
instanced mesh whenever the placed maps change — and **re-frames the camera
while doing it**, so placing one more map after flying across the city throws
the view back to the overview. The base load batches into one `setPlaced` so it
pays this once, but every later placement pays it again.

Then the shader parameter values and extended vertex decode that phase 4 skipped
and that the shader work now needs; the `World.tsx` decomposition and spatial
streaming index; GLSL shader profiles; layers, search and multi-select; and the
collision and AI path layers.

Two gaps are visible in the app today and are expected rather than regressions.
**Terrain renders flat grey** because `terrain_cb_w_4lyr`'s four-layer blend is
driven by `COLOUR0` and `TEXCOORD1`, which `MeshPart` does not decode yet.
**Trees read as solid canopies** because there is no alpha test yet.

## Anti-goals

Gen9 conversion pipeline, full FBX round-trip, Mod Manager, standalone
vehicle/ped editors, server-side anything, mobile/web deployment.

---

## Working rules

- Phase by phase. Do not start one until the previous phase's acceptance passes.
- Read the `.cs` before implementing. Never guess a format detail — mark
  `TODO(port): <question>` and record it in `formats.md`.
- Write the failing round-trip test first, then implement.
- Never fabricate a fixture. Real files go in `fixtures/` (gitignored, not
  redistributable) with `fixtures/manifest.json` tracked.
- Commit per working milestone, conventional commits.
- `cw-core` must never depend on `tauri`.
