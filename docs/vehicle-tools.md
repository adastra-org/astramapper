# Vehicle tools

Audit and optimize loose vehicle streaming assets (`.yft` / `.ytd`) of a FiveM server. Three CLIs under `crates/cw-core/examples/` plus one writer under `tools/yft-writer/`. None of them touches a file unless you pass `--apply`.

## 1. Audit: `vehicle_audit`

```bash
cargo build --release -p cw-core --example vehicle_audit
./target/release/examples/vehicle_audit.exe "E:/FiveM/Astra/resources" --out audit.json
```

Reads every `.yft`/`.ytd` (parallel), groups them by (resource, model) and writes one JSON: textures (size, format), triangles per LOD, YTD size, flags. Flags:

| Flag | Meaning |
|---|---|
| `damaged` | Claims to be RSC7 but does not decode: bad deflate, invalid resource pointers, parser panic |
| `unsupported-format` | Another container (Gen9, unknown magic). Not proof of damage |
| `empty-mesh` | Fragment parses but has no high-LOD triangles |
| `ytd-over-16mb` | Texture dictionaries add up to more than 16 MB. `ytd_class` also buckets every model: `none`, `<=4MB`, `4-8MB`, `8-16MB`, `>16MB` |
| `low-res` | Largest texture is 512 px or less |
| `heavy-tris` | More than 100k high-LOD triangles |
| `no-lods` | More than 20k high triangles and no medium or low LOD |
| `missing-lods` | More than 5k high triangles and at least one of medium/low/very-low missing (what `--lods` fills) |
| `no-textures` | `.ytd` decodes to zero textures |

The scan covers everything under the folder, not only cars: road and map props show up in the `>16MB` bucket too, so filter by resource.

## 2. Optimize: `yft_optimize`

```bash
cargo build --release -p cw-core --example yft_optimize
tools/yft-writer/build.cmd          # once; needs CodeWalker.Core built (net48 Release)

# try it: result and report go to --out-dir, the original is not touched
yft_optimize optimize car.yft --out-dir out --lods --high-ratio 0.75

# replace the original, with a backup next to the manifest
yft_optimize optimize car.yft --out-dir out --backup-dir backups --lods --high-ratio 0.75 --apply

# undo
yft_optimize rollback backups/car.<ts>.manifest.json
```

- `--high-ratio r` simplifies the high LOD to share `r` of its triangles (default 1: untouched). meshoptimizer edge collapse, attributes (normals, UV) weighted, vertex cache reordered. A per-part error cap (1% of the part's extent for the high LOD) is a limit, not a goal: when it binds, the result is bigger than the target and the report says so.
- `--lods` creates the medium, low and very-low LODs that are missing, at 50%, 25% and 10% of the (new) high LOD, with error caps of 2%, 5% and 15%. Existing LODs are never replaced.
- `--lock-border` keeps open borders fixed (windows, thin shells).
- Every drawable of the fragment is processed, in the same order `read_named_fragments` uses (main, cloth, array, physics-LOD children: doors, wheels).

Safety net:

1. The result is re-read with `cw-core` (a different reader from the one that wrote it): triangle counts per LOD must equal the plan, indices must stay inside the vertex buffer, the high LOD's bounding box must not move more than 1%.
2. `--apply` only runs if verification passed. It copies the original to `<name>.<sha256>.orig.yft`, re-reads the copy and checks its hash, writes a manifest (target, backup, sha256 before and after), then replaces the file through a temp file.
3. `rollback` refuses if the backup no longer matches its hash, or if the target changed since the optimization (`--force` overrides). It restores and re-checks the hash.
4. Comparison: `<name>.report.json` and `<name>.report.md` with triangles and vertices per LOD before/after, mean and max geometric error (share of each part's extent), file size change.

What is not verified: the optimized file loading in FiveM, and how it looks. Check a sample in game or in CodeWalker before running it over a folder. Adding LODs makes the file larger (about +75% on the sample) in exchange for a lighter draw at distance; the size change is in the report.

## 3. Writer: `tools/yft-writer`

`cw-core` has no RAGE writer. `YftWriter.exe` (C#, built with the `csc` that ships with .NET Framework, no SDK) loads a `.yft` with CodeWalker.Core, applies a plan of index buffers, compacts the vertex buffers and saves. Plan format is documented at the top of `YftWriter.cs`. `YftWriter info file.yft` prints triangles, vertices and LOD distances per drawable.

## 4. Experiment: `yft_simplify`

Decimation sweep (75/50/25/10%) on one file, with the error per ratio. Kept as the reference for the settings above.

## Not done yet

- Texture side: re-downloading originals for over-compressed textures, versioning, and recompression. The audit only finds them.
- Gen9 files (`unsupported-format`) are skipped.
- Skinned meshes (peds) are untested.
- Blender/Sollumz path: Blender 5.3 has Sollumz 2.9.1 installed, but Sollumz works on CodeWalker XML, not binary `.yft`, so it is a manual path for now.
