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
| 1 — Core port (`cw-core`) | **in progress** — 1.1 crypto, 1.2 RPF, 1.3 MetaTypes and the drawable, texture, audio and clip slices of 1.4 done, all gates cleared |
| 2 — Explorer Mode | **in progress** — browsing, per-type viewers, extraction |
| 3 — World View | not started |

Branch: work happens on `dev`. Current tests: **167 unit + 25 gate + 27 frontend**.
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
is not geometry — texture dictionary, skeleton, joints, light attributes,
bounds, shader parameters.

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
the tree (`YmapFile`, `YtypFile` and friends still open as trees, not as
entities).

### 1.4 Resources, then FileTypes

The rest of `resources/` (Bounds, Frag, Clip, Particle, ShaderParams,
ResourceBaseTypes/Builder) — the shared building blocks. Then the remaining file
types in dependency order: `YtypFile`, `YmapFile`, `YbnFile`, `YftFile`,
`YndFile`, `YmtFile`, `Gxt2File`, then the rest.

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

Five viewers, and the backend picks which one opens because that decision needs
the payload:

- **3D** for `.ydr` and `.ydd`, via three.js over the mesh buffers from
  `cw_core::mesh`. Orbit, wireframe, LOD and per-drawable selection for a
  dictionary. Positions, normals and texcoords are decoded in Rust; the WebView
  uploads buffers and nothing else.
- **Texture** for `.ytd`. The dictionary is listed on open and one texture is
  decoded at a time, to PNG — a 2048² surface is 16 MB of RGBA and 22 MB once
  base64'd, so PNG is faster to ship than raw pixels and the WebView decodes it
  natively. Mip picker, an alpha checkerboard, and export as `.dds` (the stored
  mip chain, lossless) or `.png` (the mip on screen).
- **Audio** for `.awc`. Streams are listed on open and decoded one at a time —
  a container runs to hundreds of them — then played as WAV in an `<audio>`
  element, or saved to disk. ADPCM is decoded in Rust; no browser plays
  Rockstar's framing.
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
- **Textures on models.** The 3D viewer draws untextured. A `.ydr` names its
  materials by hash and points at an embedded texture dictionary the drawable
  reader steps past; both halves now exist, so wiring them together is reading
  `ShaderGroup.TextureDictionary` and `ShaderParams.cs` for which texture fills
  which slot.
- **Fragments and particles.** `.yft` and `.ypt` embed drawables the 3D viewer
  could already draw; reaching them needs `Frag.cs` and `Particle.cs`.
- **Meta viewers.** `.ymap`/`.ytyp`/`.ymt` need 1.3, and would then show as XML
  the way the reference does.
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
