# Format port status

Reference: `adastra-org/CodeWalker` @ `d11e5951`, read from
`E:\Ad Astra Tools\CodeWalkerAstra`.

Status values: **done** (round-trip byte-equality passes on a real fixture) ·
**partial** (reads, does not yet round-trip) · **todo** · **deferred**.

A format is only **done** when parse → serialize with no edits reproduces the
input bytes exactly. Reading correctly is not enough; the editor writes these
files back.

## Utils and crypto

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `utils/jenk.rs` | `Utils/Jenk.cs` | **done** | Vectors from the reference DLL. Three overloads kept distinct, see `porting-guide.md`. |
| `utils/keys.rs` | `Utils/GTAKeys.cs` | todo | SHA-1 hash search over `GTA5.exe`; needs `magic.dat` embedded. |
| `utils/crypto.rs` | `Utils/GTACrypto.cs` | todo | AES + NG decrypt/encrypt and the NG tables. |
| `utils/data.rs` | `Utils/Data.cs` | todo | LE reader/writer. |
| `utils/dds.rs` | `Utils/DDSIO.cs` | todo | 124 KB; BC1–BC7. |
| math | SharpDX.Mathematics | todo | `glam` aliases. `f32` only in serialized layouts. |

## Archive layer

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `rpf/file.rs` | `RpfFile.cs` (105 KB) | todo | Header, entry table, nesting, RSC7. |
| `rpf/entry.rs` | `RpfFile.cs` | todo | Directory / binary / resource discrimination. |
| `rpf/manager.rs` | `RpfManager.cs` | todo | |
| `game_file.rs` | `GameFile.cs` | todo | |
| `cache.rs` | `GameFileCache.cs` (240 KB) | todo | |

## Meta types

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `meta/` | `MetaTypes/Meta.cs`, `MetaTypes.cs`, `MetaXml.cs` | todo | |
| `meta/pso.rs` | `Pso.cs`, `PsoTypes.cs` (1.5 MB) | todo | Generated table — emit Rust from the `.cs` with a script, do not hand-port. |
| `meta/names.rs` | `MetaNames.cs` (870 KB) | todo | Same: generated. |
| `meta/rbf.rs` | `Rbf.cs` | todo | |

## Resources

`Drawable` (282 KB), `Particle` (264 KB), `Bounds` (212 KB), `Frag` (190 KB),
`Clip` (164 KB), `Clothes` (111 KB), `ShaderParams`, `Expression`, `Texture`,
`Nav`, `Node`, `Filter`, `VertexType`, `VehicleRecord`, `WaypointRecord`,
`ResourceBaseTypes`, `ResourceData`, `ResourceBuilder`, `ResourceFile`,
`ResourceAnalyzer` — all **todo**. These are the shared building blocks the file
types compose, so they land before the parsers that use them.

## File types

All 41 **todo**. Planned order: `YtdFile`, `YtypFile`, `YmapFile`, `YdrFile`,
`YddFile`, `YbnFile`, `YftFile`, `YndFile`, `YmtFile`, `Gxt2File`, then the rest.

`AwcFile` · `Builders` · `CacheDatFile` · `CarColsFile` · `CarModColsFile` ·
`CarVariationsFile` · `CutFile` · `DistantLightsFile` · `DlcContentFile` ·
`DlcSetupFile` · `FxcFile` · `GtxdFile` · `Gxt2File` · `HeightmapFile` ·
`JPsoFile` · `MrfFile` · `PedFile` · `PedsFile` · `RelFile` (1.2 MB) · `Stats` ·
`VehicleLayoutsFile` · `VehiclesFile` · `WatermapFile` · `YbnFile` · `YcdFile` ·
`YddFile` · `YdrFile` · `YedFile` · `YfdFile` · `YftFile` · `YldFile` ·
`YmapFile` · `YmfFile` · `YmtFile` · `YndFile` · `YnvFile` · `YpdbFile` ·
`YptFile` · `YtdFile` · `YtypFile` · `YvrFile` · `YwrFile`

## World models

`Entity`, `Space`, `Camera`, `Ped`, `Vehicle`, `Water`, `Weather`, `Timecycle`,
`PopZones`, `Scenarios`, `Trains`, `AudioZones`, `Heightmaps`, `Clouds`,
`Weapon`, `Watermaps`, `TimecycleMods` — all **todo**.

## Deferred

| Unit | Reason |
| --- | --- |
| `Fbx`, `FbxConverter` | Needs a Rust FBX writer. Interface stubbed, port later. |
| `Gen9Converter` | Anti-goal. |
| Enhanced-edition RPF | Oodle compression, `rpf.cache`, Gen9 resources. After Legacy works end to end. |

## Shader fidelity

Filled in during phase 3, when `CodeWalker/Shaders/*.hlsl` is translated to WGSL.
Any shader that does not translate cleanly gets a row here naming the
approximation used. Unlit flat materials are not an acceptable default.

## Open questions

None recorded yet. Anything marked `TODO(port):` in the Rust source belongs here
with the file it came from.
