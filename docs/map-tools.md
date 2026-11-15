# Map tools

Audit loose map streaming assets (`.ymap` / `.ytyp`) of a FiveM server, and measure how many of them load at the same spot. The audit reads only; `map-hex.ts` reads only. Nothing here writes to the server's files.

## 1. Audit: `map_audit`

```bash
cargo build --release -p cw-core --example map_audit
./target/release/examples/map_audit.exe "E:/FiveM/Astra/resources" --out map-audit.json
./target/release/examples/map_audit.exe "E:/FiveM/Astra/resources" --rules map-rules.json --gta "E:/SteamLibrary/steamapps/common/Grand Theft Auto V" --out map-audit.json
```

Arguments: `<resources dir> [--rules map-rules.json] [--gta <GTA V folder>] [--out map-audit.json]`.

`--gta` (or `ASTRAMAPPER_GTA_PATH`) reads the game archives for the vanilla YMAP names and archetype names. It enables:

- vanilla-override: tells a YMAP that replaces a game one from a new map.
- orphan-parent: checks each `parent` link against the game and the loose YMAPs.

Without `--gta` those checks are skipped, never guessed.

Only files under a `stream` folder get flagged. The owning resource and the first folder under `stream` (the "map") are taken from the path.

| Issue | Meaning |
|---|---|
| `duplicate-name` | Same YMAP (or ytyp) name in more than one file |
| `duplicate-archetype` | A ytyp file defines archetypes that another file also defines |
| `huge-extents` | Only HD entities and streaming span above `hugeExtentM`. Not checked for vanilla overrides |
| `mixed-lod` | HD and LOD entities in the same YMAP. They belong in separate YMAPs linked by `parent` |
| `orphan-parent` | `parent` names a YMAP that is neither vanilla nor a loose YMAP. Needs `--gta` |
| `tiny-ymap` | Fewer than `tinyEntities` entities and no MLO instance |
| `extents-stale` | An entity sits outside the stored entity or streaming box (beyond `extentsToleranceM`) |
| `mlo-room-flags` | An MLO room (except room 0, limbo) lacks the bits in `roomFlags` |
| `mlo-portal-flags` | A door portal lacks `doorPortalFlags`, or a window portal lacks `windowPortalFlags` |
| `mlo-timecycle` | A room timecycle is not in `roomTimecycles` |

Portal kind comes from the names of the objects attached to it: `door` in the name makes a door; `window`, `glass` or `_win` makes a window; anything else is `other` and is not checked.

Timecycles without a readable name are written as `hash_XXXXXXXX`.

## 2. Rules: `map-rules.json`

camelCase, every field optional. A file only needs the fields it changes.

```json
{
  "roomFlags": 96,
  "doorPortalFlags": 64,
  "windowPortalFlags": 8,
  "roomTimecycles": ["mp_studio_lo"],
  "hugeExtentM": 1000,
  "tinyEntities": 5,
  "extentsToleranceM": 5
}
```

| Field | Default | Basis |
|---|---|---|
| `roomFlags` | 96 | Room bits 32 and 64. Their meaning is not confirmed: [altv docs](https://altv.stuyk.com/docs/mapping/mlo-room-flags.html) mark them unknown; [dekurwinator flags](https://dekurwinator-mods.bitrix24.site/flags/) list them |
| `doorPortalFlags` | 64 | With this bit set, the MLO stops rendering while the door is closed, see [forum.cfx.re](https://forum.cfx.re/t/mlo-renders-when-entering-room/5208215) |
| `windowPortalFlags` | 8 | No source. Team choice |
| `roomTimecycles` | `["mp_studio_lo"]` | Team choice. Pending validation in game |
| `hugeExtentM` | 1000 | Streaming span above this, for HD-only YMAPs, is `huge-extents` |
| `tinyEntities` | 5 | Below this, a YMAP with no MLO is `tiny-ymap` |
| `extentsToleranceM` | 5 | Margin before extents count as stale |

`roomFlags` and `doorPortalFlags` are subset checks: the room or portal must have every listed bit set.

## 3. Hex load: `map-hex.ts`

```bash
bun scripts/map-hex.ts map-audit.json [--size 250] [--at x,y ...] [--out map-hex.json]
```

Writes `map-hex.json` and `map-hex.md` next to `--out`. Read-only on the server.

Context: the client crashes with `MapDataLoadedNode Pool Full, Size == 1100` when too many YMAPs are loaded at one spot. The pool size is fixed; `increase_pool_size` does not raise it (see [server commands](https://docs.fivem.net/docs/server-manual/server-commands/)). A YMAP loads while the camera is inside its streaming box, so the script counts, for each hex cell centre, the server YMAPs whose streaming box contains it.

- `--size` is the hex size in metres (default 250).
- The cell grid covers the playable area. The known crash spots (Rockford, Sandy Shores, Grapeseed) are always reported as spots.
- `--at x,y` adds one more spot to report. Repeat for several points.
- Vanilla overrides are counted apart: they replace a game YMAP and do not add one.
- Entities of the game itself are not in the report. The count is what the server adds, not the total the client loads.

Merge proposals: YMAPs of the same map, the same LOD class (detail or coarse), whose entity centres fall in the same cell, when the merged streaming box stays under `hugeExtentM`. Excluded: vanilla overrides, MLOs, and mixed-LOD files. Groups are sorted by YMAPs saved.

## Not done yet

- Absolute counts including the game's own YMAPs. Today the count is only what the server adds.
- `ymap_merge` with `--apply`, backup and rollback. Needs a RAGE writer like `tools/yft-writer`.
- Automatic fix of MLO flags (`mlo-room-flags`, `mlo-portal-flags`, `mlo-timecycle`). Report only for now.
