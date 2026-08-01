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
| 1 — Core port (`cw-core`) | **in progress** — 1.1 crypto done, gate cleared |
| 2 — Explorer Mode | not started |
| 3 — World View | not started |

Branch: work happens on `dev`. Current tests: **27 unit + 3 gate** (gate is
`#[ignore]`d, needs a local install).

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

The gate (needs the game; override the path with `ASTRAMAPPER_GTA_PATH`):

```bash
cargo test -p cw-core -- --ignored --nocapture
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

### 1.2 RPF archive layer — **NEXT**

Target: `crates/cw-core/src/rpf/{mod,file,entry,manager}.rs`.
Reference: `CodeWalker.Core/GameFiles/RpfFile.cs` (105 KB), `RpfManager.cs`.

Most of the header logic already exists, written by hand inside
`tests/crypto_gate.rs::opens_a_real_encrypted_archive`. Moving it into
`rpf/file.rs` and building an entry model around it is the concrete first step.

What the reference does, in order:

- Header: `Version` must be `0x52504637` (`RPF7`), then `EntryCount`,
  `NamesLength`, `Encryption`.
- `RpfEncryption`: `NONE = 0`, `OPEN = 0x4E45504F`, `AES = 0x0FFFFFF9`,
  `NG = 0x0FEFFFFF`. **Any unrecognised value is treated as NG**, not an error.
- Read `EntryCount * 16` bytes of entries and `NamesLength` bytes of names, then
  decrypt both according to `Encryption` (NG keys on the archive's *file name*
  and *size on disk* — both wrong-able silently).
- Entry discrimination, from the first two u32s (`y`, `x`) of each 16-byte entry:
  - `x == 0x7fffff00` → directory
  - `(x & 0x80000000) == 0` → binary file
  - otherwise → resource file
- Names come from `namesrdr.Position = e.NameOffset`, NUL-terminated. The
  reference truncates names longer than 256 chars (comment: long names freeze the
  RPF Explorer).
- `RpfResourceFileEntry.IsEncrypted = name.EndsWith(".ysc")`, with the
  reference's own comment "any other way to know..?". Carry the doubt across.
- RPF-in-RPF nesting, with the reference's cycle handling.
- RSC7 resource header + raw-deflate payloads; binary entries are deflate too.

Cross-check the `OPEN` path against RPFTool's `src/rpf/{header,entries,extract}.ts`
and `bun run src/cli.ts list <rpf> --json`. Two independent implementations
disagreeing is the cheapest bug signal available.

Acceptance: enumerate all 179 legacy RPFs; entry counts and extracted bytes match
`CodeWalker RPF Explorer.exe` on a sampled set.

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
