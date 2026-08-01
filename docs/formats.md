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
| `utils/data.rs` | `Utils/Data.cs` | **done** | Reader only, LE only. Writer lands with the first format that serializes. |
| `utils/net_random.rs` | .NET `System.Random` | **done** | Not a game format; the key loader needs it. Pinned against .NET Framework 4.8. |
| `utils/keys.rs` | `Utils/GTAKeys.cs` | **done** | `LoadFromPath` path only. `magic.dat` embedded from `assets/`. |
| `utils/crypto.rs` | `Utils/GTACrypto.cs` | **done** | AES-256 ECB, NG decrypt, `GTA5Hash`. NG *encrypt* still todo. |
| `utils/dds.rs` | `Utils/DDSIO.cs` | todo | 124 KB; BC1–BC7. |
| math | SharpDX.Mathematics | todo | `glam` aliases. `f32` only in serialized layouts. |
| `text.rs` | — | n/a | Not a port. Encoding sniffing for the Explorer's text viewer. |
| `filetype.rs` | `GameFileType` + `GameFileCache.cs` switches | n/a | Extension-to-family table, collected rather than ported. |
| `mesh.rs` | `CodeWalker/Rendering` buffer upload | n/a | Not a port. Flattens a `Drawable` into buffers for the WebView. |

### Notes on the crypto

**Which key path.** The reference has two. `Generate` hash-searches `GTA5.exe`
for the AES key, all 101 NG keys and the decrypt tables; `LoadFromPath` →
`GenerateV2` + `UseMagicData` scans out only the AES key and unmasks everything
else from `magic.dat`. Every real caller uses the second, so that is what is
ported. Consequence: `RandomGauss` and `LookUpTableGenerator` — the largest and
most intimidating part of `GTAKeys.cs` — are not on the critical path, because
they solve for *encryption* tables only. They will be needed to write
NG-encrypted archives back out, not to read them.

**`GTA5Hash` is Jenkins plus a lookup table.** The reference writes its mixing
steps as multiplications, which disguises it: `1025 * x == x + (x << 10)`,
`9 * x == x + (x << 3)`, and `32769 * x == x + (x << 15)` are precisely Jenkins'
three steps. So `gta5_hash` is `gen_hash` with a byte substitution applied to
each input character. The real LUT remaps only **27 of 256** values, meaning the
two hashes agree on the large majority of inputs — a port that dropped the table
would pass casual testing and select the wrong NG key for names containing one
of the 27.

**The LUT is not a permutation.** It has 229 distinct values with a maximum
multiplicity of 2. (An earlier test asserted a permutation; that was assumed,
not taken from the reference, and the real keys disproved it.)

**.NET `System.Random` is load-bearing.** `UseMagicData` unmasks `magic.dat` by
subtracting four `Random` byte streams seeded with `(int)JenkHash.GenHash(aesKey)`.
The seed is normally negative, and `Math.Abs` makes seed and `-seed` equivalent
while `int.MinValue` is special-cased to `int.MaxValue`. All three behaviours are
pinned by tests. Substituting any other PRNG produces silent garbage.

## Archive layer

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `rpf/file.rs` | `RpfFile.cs` (105 KB) | **partial** | Reads: header, entry table, tree, nesting, RSC7 + deflate extraction. Writing is not ported, so not **done** by this doc's rule. |
| `rpf/entry.rs` | `RpfFile.cs` | **partial** | Directory / binary / resource discrimination, page flags, oversized-resource escape. `Write` not ported. |
| `rpf/manager.rs` | `RpfManager.cs` | **partial** | Folder scan, flattening, path index, transparent nested listing. Mod/DLC precedence deliberately omitted. |
| `game_file.rs` | `GameFile.cs` | todo | |
| `cache.rs` | `GameFileCache.cs` (240 KB) | todo | |

### Notes on the archive layer

**Three entry layouts, not one field in three widths.** A directory reads its
name offset as a `u32`, a resource as a `u16`, a binary entry as the low 16 bits
of a packed `u64`. They agree only because name tables are under 64 KB. The
discriminator is the second `u32`: `0x7FFFFF00` is a directory, a clear top bit
is a binary file, anything else is a resource.

**Unknown encryption values are NG, not errors.** `RpfEncryption` has four named
values and the reference's `default:` case decrypts as NG anyway, with the
comment "never seems to get here". Ported as-is.

**`GetSizeFromFlags` overflows, and that is load-bearing as a test.** Its mixed
`int`/`uint` arithmetic widens to `long` and casts back to `int`, so saturated
flag words come out negative — the reference returns `-1627389952` for
`0x0FEFFFFF`. Computing in `u64` and truncating reproduces the same 32 bits.
Pinned, because any misplaced shift in the nine page-count fields would land
somewhere else entirely.

**Oversized resources.** A resource whose 24-bit length field reads `0xFFFFFF`
is really `>= 0xFFFFFF`; the true length is unpacked from bytes 7, 14, 5 and 2 of
its own payload header, in that order. Verified against the reference on a real
file: `b2a.ytd` is 20921589 bytes. RPFTool does **not** implement this and
reports the raw sentinel, so it is the one place the two oracles disagree and
the C# reference arbitrates.

**A nested archive is one thing seen from two sides.** The `.rpf` binary entry in
the parent and the root directory of the archive it opens share a path. The index
keeps the parent's entry, because that is the one that can be extracted as a
file; listing it steps through into the archive transparently.

## Meta types

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `meta/` | `MetaTypes/Meta.cs`, `MetaTypes.cs`, `MetaXml.cs` | todo | |
| `meta/pso.rs` | `Pso.cs`, `PsoTypes.cs` (1.5 MB) | todo | Generated table — emit Rust from the `.cs` with a script, do not hand-port. |
| `meta/names.rs` | `MetaNames.cs` (870 KB) | todo | Same: generated. |
| `meta/rbf.rs` | `Rbf.cs` | todo | |

## Resources

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `resources/reader.rs` | `ResourceData.cs` | **partial** | `ResourceDataReader` only. The writer lands with the first format that serializes. |
| `resources/vertex.rs` | `VertexType.cs`, `VertexDeclaration` | **partial** | Declaration decode and component extraction. Gen9 declarations not ported. |
| `resources/drawable.rs` | `Drawable.cs` (282 KB) | **partial** | Geometry path only: models, geometries, vertex/index buffers, shader name hashes. Reads, does not write. |

`Particle` (264 KB), `Bounds` (212 KB), `Frag` (190 KB), `Clip` (164 KB),
`Clothes` (111 KB), `ShaderParams`, `Expression`, `Texture`, `Nav`, `Node`,
`Filter`, `VehicleRecord`, `WaypointRecord`, `ResourceBuilder`,
`ResourceAnalyzer` — all **todo**.

Not read by `resources/drawable.rs`, though their pointers are kept: the
embedded texture dictionary, the skeleton, joints, light attributes, collision
bounds and shader *parameters*. Each needs a sibling file that is not ported.

### Notes on the resource layer

**Two segments, no relocation pass.** A resource payload is a system half and a
graphics half, mapped at `0x50000000` and `0x60000000`. Every pointer in the file
is an address in that space, so resolving one is masking off the base and
picking a segment. The reference's segment test is a bitmask, not a range check,
and it tries system first; both are reproduced, because the masks overlap and the
order decides who wins.

**Short reads are zeros, not errors.** `ResourceDataReader` reads through
`MemoryStream`, which short-reads rather than throwing. A payload smaller than
its page flags claim therefore yields zeros, and the reference relies on it. The
consequence for the Explorer: a junk `.ydr` *parses*, into a drawable with no
models. "It parsed" is not the question worth asking, which is why
`Drawable::has_geometry` exists.

**Vertex declarations are indexed by semantic, not by packing order.** `flags` is
a bitmask over the 16 semantics; `types` is 16 nibbles, one per semantic, present
or not. A component's offset is the sum of the sizes of the *present* components
below it. Reading the nibbles in sequence works on `0x7755555555996996` — the
declaration nearly every YDR uses — and silently misparses the rest.

**A resource length of `0xFFFFFF` is unrelated to `FloatUnk`.** Two component
nibbles (`FloatUnk`, and 11-15) are sized zero in the reference and never seen.
They are kept sized zero rather than rejected, so an unknown declaration decodes
to a shorter stride instead of failing.

**The drawable's bounding box is not a superset of its vertices.** Measured over
716k vertices: a fifth fall outside it, two thirds for skinned models. The
per-geometry AABB from `DrawableModel.BoundsPointer` *is* exact — not one vertex
outside — and is what `drawable_gate.rs` validates against.

## Audio

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `audio/awc.rs` | `AwcFile.cs` (113 KB) | **partial** | Header, stream and chunk tables, `format`/`streamformat`/`data`/`mid`, multi-channel de-interleave, RSXXTEA. Reads only; PC (little-endian) only. |
| `audio/adpcm.rs` | `ADPCMCodec` in `AwcFile.cs` | **partial** | Decode. Encode not ported. |
| `audio/wav.rs` | `AwcStream.GetWavStream` | **done** | 16-bit mono PCM, which is the only shape the reference writes. |
| `rel.rs` | `RelFile.cs` (1.2 MB) | todo | The metadata that names and sequences the waves. Without it a stream is a hash. |

### Notes on the audio layer

**Three encryption modes, and one of them has no flag.** A container can be
encrypted whole, per multi-channel block, or not at all, under a modified XXTEA
(`^ 0x7B3A207F` folded into the key lookup). The per-block and single-channel
cases are flags. The whole-file case is detected by the magic failing to read as
`ADAT` — there is nothing else to go on.

**The decrypt looks asymmetric and is not.** It seeds its running value from
`blocks[0]` where the textbook XXTEA decrypt seeds from the last block, and the
reference labels its own code as undefined behaviour it has not cleaned up. At
`p = n-1` the textbook value *is* `blocks[0]`, so the two directions agree. A
test pins the round trip so a future tidy-up cannot quietly change which is true.
XXTEA is undefined for a single 4-byte block and the two directions genuinely
disagree there; no real payload is that short.

**A stream is a channel, not a sound.** A stereo song is two streams paired by a
`_left`/`_right` naming convention that lives in the name tables, not in the
file. In a multi-channel container the audio is in a single carrier stream with
id 0, and the others index into it; a channel's audio is its slice of every
block, concatenated.

**The declared sample count is authoritative, in both directions.** Blocks hold
4088 samples, so an odd declared count is routinely one more than the blocks can
produce — `01_0_gtaiv_legacy_support_small.awc` declares 12161 and carries 12160.
This port pads to the declared count. It also *trims* to it, which the reference
does not: `DecodeADPCM` returns a `data.len() * 4` buffer however little it
wrote, so CodeWalker's exported WAVs carry four bytes of silence per block
header. That trim is the one deliberate divergence; the samples agree exactly.
Verified over 132.7 million samples against `GetPcmData`.

## File types

All 41 **todo** as files, though `YdrFile.cs`, `YddFile.cs` and `AwcFile.cs` are
covered by `resources::read_drawables` and `audio::AwcFile` respectively.
Planned order: `YtdFile`, `YtypFile`, `YmapFile`, `YdrFile`, `YddFile`,
`YbnFile`, `YftFile`, `YndFile`, `YmtFile`, `Gxt2File`, then the rest.

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

- `RpfResourceFileEntry.IsEncrypted` is set from `name.EndsWith(".ysc")` with the
  reference's own comment "any other way to know..?". Carried across with the
  same doubt: there is no flag for it in the entry, so a differently named
  encrypted script would be missed here exactly as it is there.
- NG *encryption* (writing archives back) needs `RandomGauss.Solve` and
  `LookUpTableGenerator.BuildLUTs2`. Not required for reading; deferred until
  archive writing is in scope.
- `ExtractFileResource` mutates `entry.FileSize -= 0x10` on the path where
  inflation fails and the raw bytes are returned, which changes what a second
  extraction of the same entry reads. Not carried across — it looks like a bug
  rather than a format rule. Marked `TODO(port):` in `rpf/file.rs`.
- The reference's directory-tree walk has no revisit guard, so an archive whose
  directory points at an ancestor loops forever. This port keeps the same stack
  walk but refuses to enter a directory twice. No real archive needs this; it
  matters because the tool opens files it did not create.

Anything marked `TODO(port):` in the Rust source belongs here with the file it
came from.
