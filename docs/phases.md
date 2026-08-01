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
| 1 — Core port (`cw-core`) | **in progress** — 1.1 crypto and 1.2 RPF done, both gates cleared |
| 2 — Explorer Mode | **in progress** — folder picker and `list_dir` over the real archives |
| 3 — World View | not started |

Branch: work happens on `dev`. Current tests: **43 unit + 13 gate + 10 frontend**.
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

Two of them compare against oracles whose data is gitignored. Regenerate before
running, or they skip with a message:

```bash
pwsh -File scripts/rpf-oracle.ps1
```

```bash
bash scripts/rpftool-oracle.sh
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

### 1.3 MetaTypes

`meta/` ← `MetaTypes/`. `PsoTypes.cs` (1.5 MB) and `MetaNames.cs` (870 KB) are
generated tables — write a script that reads the `.cs` and emits Rust. Do not
hand-port them.

### 1.4 Resources, then FileTypes

`resources/` first (Drawable, Bounds, Frag, Texture, Clip, Particle,
ShaderParams, VertexType, ResourceBaseTypes/Data/Builder) — the shared building
blocks. Then the 41 file types in dependency order: `YtdFile`, `YtypFile`,
`YmapFile`, `YdrFile`, `YddFile`, `YbnFile`, `YftFile`, `YndFile`, `YmtFile`,
`Gxt2File`, then the rest.

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

The scaffold: `src-tauri/src/explorer.rs` (`open_game_folder`, `list_dir`,
`game_is_open`), `src/lib/explorer.ts`, `src/components/Explorer.tsx`. Picking a
folder recovers the keys, scans every archive off the main thread, and the whole
install browses as one tree — nested archives included, with no visible seam at
an `.rpf` boundary. Gate at `src-tauri/tests/explorer_gate.rs`.

### Next

- **Extraction to disk.** `RpfManager::extract` already works and is covered by
  the 1.2 gate; what is missing is the IPC command and the save dialog. The keys
  are already held in `explorer::Loaded` for exactly this.
- **Lazy pagination.** `list_dir` currently returns a whole directory. Fine at
  the sizes measured, but the phase 2 brief asks for paging.
- **Search** by name and Jenkins hash across the 403,642-entry index.
- **Per-type viewers**, which need 1.3 and 1.4 first — there is nothing to view
  until the resource and file-type parsers land.
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
