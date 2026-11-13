# Asset index (`asset_index`)

Offline index of the loose streaming models of a FiveM server: triangles per LOD, vertices, LOD distances and weight in bytes of every `.yft`, `.ydr` and `.ydd` drawable, joined with its `.ytd`. The in-game debug resource (`astra_debug`) reads it to show what the targeted entity costs, because no native reports polygons or weight.

```bash
cargo build --release -p cw-core --example asset_index
./target/release/examples/asset_index.exe "E:/FiveM/Astra/resources" --out E:/Proyectos/_scratch/asset-index.json
```

Read-only: nothing under the resources folder is touched. The generated JSON is not committed to this repo.

| Flag | Meaning |
|---|---|
| `--out <path>` | Output file (default `asset-index.json`). Parent folders are created |
| `--pretty` | Indented output (one field per line), for reading or diffs |
| `--min` | Compact output, no whitespace. This is already the default; the flag exists so a script can say it and it wins over `--pretty` |
| `--only-resources a,b` | Scan only resources whose path starts with one of the prefixes, case-insensitive, `/` separators (`"[astra]/[cochesfacciones]"`). The scan is the slow part, so this also makes the run faster |
| `--generated-at <ISO>` | Fixed `generatedAt`, for reproducible output (tests, diffs). Default: now, UTC |

Run time on the Astra server (20,479 loose files, 11k `.yft`, 3k `.ydr`, 1.7k `.ydd`, 9.5k `.ytd`): about 2 minutes.

## Contract (`version: 1`)

```json
{
  "version": 1,
  "generatedAt": "2026-11-13T23:12:38Z",
  "source": "resources",
  "models": {
    "aleutianems": {
      "hash": 220272040,
      "resource": "[astra]/[cochesfacciones]/Facciones",
      "kind": "yft",
      "tris":      { "high": 103023, "med": 15971, "low": 8383, "vlow": 1707 },
      "verts":     { "high": 98380 },
      "bytes":     { "model": 5037612, "ytd": 4119121, "total": 9156733 },
      "textures":  { "count": 42, "maxSize": 4096 },
      "lodDist":   { "high": 9998.0, "med": 9998.0, "low": 9998.0, "vlow": 9998.0 },
      "flags":     ["heavy-tris"]
    }
  },
  "duplicates": {
    "9k120": [
      { "resource": "[astra]/[cochespruebas]/Pruebas",   "path": "[astra]/[cochespruebas]/Pruebas/stream/[nuketruck]/9k120.ydr", "kind": "ydr", "trisHigh": 2060, "bytesTotal": 135739 },
      { "resource": "[astra]/[cochespruebas]/Vehiculos", "path": "[astra]/[cochespruebas]/Vehiculos/stream/[ah6m]/9k120.ydr",   "kind": "ydr", "trisHigh": 2060, "bytesTotal": 135739 }
    ]
  }
}
```

`duplicates` is the one addition to the agreed shape (an object, empty when there are none); the rest is as agreed.

- **Order.** `models` and `duplicates` are sorted by name; the fields of an entry are always in the order above. The same input gives the same bytes except for `generatedAt`.
- **Keys.** The lowercase model name: the file name without extension and without a `_hi` / `+hi` suffix for `.yft` / `.ydr`; the drawable's own name for `.ydd` (below).
- **`source`.** Last component of the scanned folder.
- **Numbers** are integers, except `lodDist` (floats, rounded to 2 decimals).

### Hash and sign

`hash` is `joaat(lowercase name)` as an **unsigned 32-bit integer**, the value of `GetHashKey(name)`. `GetEntityModel` returns the same 32 bits, but FiveM Lua hands native `Hash` results over as a **signed** integer, so `GetEntityModel(e)` for `adder` is `-1216765807`, while the index stores `3078201489`. Compare after normalizing:

```lua
local function u32(h) return h & 0xFFFFFFFF end   -- Lua 5.4 (FiveM)
byHash[u32(entry.hash)] = entry                   -- hash is already unsigned
local entry = byHash[u32(GetEntityModel(entity))]
```

In JS/TS, `hash >>> 0`. Build the `hash -> entry` map once when the index loads.

### Per field

| Field | Rule |
|---|---|
| `tris` | Triangles per LOD of the model (`high`, `med`, `low`, `vlow`), summed over its drawables (below) |
| `verts.high` | Vertices of the high LOD, summed the same way |
| `bytes.model` | Size **on disk** of the model file(s): compressed RSC7, what a player downloads. Not GPU memory |
| `bytes.ytd` | Size on disk of the texture dictionaries of the model (rule below) |
| `bytes.total` | `model + ytd` |
| `textures` | Texture count and the largest side in pixels over those dictionaries |
| `lodDist` | `lodDistances` of the drawable (the main one for a fragment). `9998` is the placeholder almost every vehicle carries, i.e. no real distance configured |
| `flags` | The `vehicle_audit` flags with the same thresholds (shared code, `cw_core::loose`): `heavy-tris` (high above 100k), `no-lods` (high above 20k and no medium or low), `missing-lods` (high above 5k and a LOD missing), `ytd-over-16mb`, `low-res` (largest texture 512 or less), `no-textures`, `empty-mesh`, `damaged`, `unsupported-format`. See [vehicle-tools.md](vehicle-tools.md) |

### Which `.ytd` belongs to a model

The `.ytd` files **in the same resource** whose name, after dropping `_hi` / `+hi`, is the model's name: `adder.ytd` and `adder+hi.ytd`. Both are added up (bytes, texture count; the largest side is the maximum). Anything else is not counted: a dictionary shared by name (`vehshare.ytd`), a texture parent set in a `.ytyp`/`.meta`, or a `.ytd` in another resource. For those the index under-reports the weight; `bytes.ytd` of 0 means "no same-name dictionary", not "no textures". On the Astra server 1,339 of 17,699 models have one.

For a `.ydd` entry the lookup uses the **file's** name (`mp_m_freemode_01^uppr_000_u.ydd` looks for `...uppr_000_u.ytd`), which almost never matches the variant dictionaries ped clothing uses, so clothing reports no textures.

### Fragments (`.yft`)

One entry per file, named after it. Its triangles are the sum of the main drawable and the drawables of its parts (wheels, doors, ...). Each part's `drawable2` is the **damaged variant** of that part and is not counted, so the totals are lower than `vehicle_audit`'s (which sums everything, every physics LOD included, and adds the `_hi` file to the base file).

When both `x.yft` and `x_hi.yft` exist, the `_hi` file holds only a bigger high LOD that streams in up close: `tris.high` and `verts.high` are the larger of the two files, the other LODs come from the base file, and `bytes.model` is the sum of both files.

### Dictionaries (`.ydd`)

One entry per drawable, keyed by the drawable's name from the dictionary (`hash_XXXXXXXX`, lowercase, when the file carries no name for it) and with the dictionary's own hash for it. `bytes.model` is the size of the **whole dictionary file**, because it streams as a unit; it is not split per drawable. A dictionary that does not decode contributes no entries (there are no names to key them by); it is counted in the run summary.

### Duplicates

Several resources can provide the same name (the same prop in two vehicle packs, `9k120` above). Which one FiveM serves depends on resource start order in the server config (`ensure`/`start`, category folders, dependencies), which this tool does not read, so **it cannot be told offline**. Rule used:

1. The candidate whose `resource`, then `path`, sorts first is the one in `models`.
2. Every candidate, that one included and listed first, goes to `duplicates[name]` with its resource, path, high triangles and total bytes.

If the candidates differ in cost, that name's numbers in `models` may not be what the server actually loads. The debug resource should show a warning for a name present in `duplicates`. On the Astra server: 410 names. A `.yft` and a `.ydr` of the same name count as duplicates too.

## Limits

- **Gen9 and unreadable files.** A `.yft`/`.ydr` that is not RSC7 (Gen9, unknown) or does not decode is still listed, with zero triangles and the `unsupported-format` or `damaged` flag, so a model is never silently missing. Those entries carry no useful numbers. On the Astra server: 29 model files and 13 `.ytd` failed to decode.
- **Skinned meshes (peds).** Triangles are counted from the drawable's geometry like any other model; skinned peds are untested and the numbers are unverified there.
- **Cloth and extra physics LODs.** The cloth drawable of a fragment is counted with the rest; only the first physics LOD's parts are counted, the others repeat the same parts.
- **`bytes` is on-disk size**, not the memory the model takes once streamed (RSC7 virtual + physical). Good for ranking and for the 16 MB class; not for VRAM.
- **Not covered:** models that exist only inside RPFs, models from `.ytyp`-only props without a loose file, `.ybn` collisions, and anything outside the scanned folder.
- **Size.** 17,699 models give about 5.9 MiB compact (0.5 MiB gzipped). `lodDist` is the biggest part (about 16%), then `duplicates` (5%). A consumer that only needs vehicles can pass `--only-resources`.
