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
| `image.rs` | `Utils/DDSIO.cs` | **partial** | `GetPixels` and `GetDDSFile`. BC1–BC7 decode via `bcdec_rs`; DDS bytes byte-identical. Reading a `.dds` back in, and the D3DX legacy-format conversions, are not ported. |
| math | SharpDX.Mathematics | todo | `glam` aliases. `f32` only in serialized layouts. |
| `utils/names.rs` | `MetaTypes/MetaNames.cs` (870 KB) | **done** | The 20292-name hash table, generated into `assets/metanames.bin` by `scripts/metanames.ps1` and binary-searched. Not hand-written. |
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
| `meta/mod.rs` | `YmtFile.Load` | **partial** | Picks the container: resource ⇒ RSC, else `RBF0`, else `PSIN`. Reads only. |
| `meta/rsc.rs` | `Meta.cs`, `MetaTypes.cs` | **partial** | Structure, enum and block tables, packed pointers. The `MetaBuilder` half is not ported. |
| `meta/pso.rs` | `Pso.cs` | **partial** | Section loop, data map, schema (structures and enums), string tables. Big-endian, as PC stores it. |
| `meta/rbf.rs` | `Rbf.cs` | **partial** | The token stream, descriptor interning, attribute counts. Writer not ported. |
| `meta/tree.rs` | `MetaXml.cs` (`MetaXml`, `PsoXml`, `RbfXml`) | **partial** | Walks any of the three into a tree of named values, resolved against the schema the file carries. Values are formatted as the reference formats them. |
| `meta/typed.rs` | `MetaTypes.Get*` helpers | **partial** | The typed layer `YtypFile` and `YmapFile` build on: `GetTypedData`, `GetPointerArray`, `GetTypedPointerArray`, `ConvertDataArray`, `GetHashArray`/`GetUintArray`/`GetFloatArray`/`GetByteArray`, `GetString(s)`, and the packed records (`CMapData`, `CEntityDef`, `CBaseArchetypeDef`, `CMloArchetypeDef`, `CMloRoomDef`, `CMloPortalDef`, `CMloEntitySet`, `CMloTimeCycleModifier`, `CMloInstanceDef`, `CMapTypes`, `CCompositeEntityType`, `CCarGen`, `CTimeCycleModifier`, `BoxOccluder`, `OccludeModel`, `CLODLight`, `CDistantLODLight`, `CBlockDesc`, the grass records). Read only. |
| `PsoTypes.cs` (1.5 MB) | — | not ported | The fallback definitions for structures a PSO file does not declare. Four nodes in two `.ymt` in the install need it; everything else is self-describing. |

### Notes on the metadata layer

**Three encodings, one shape.** A `.ymt` is RSC, PSO or RBF, and which one is
not a property of the bytes alone: RSC is decided by the *archive entry* being a
resource, and only then do the magic numbers get a look. All three carry their
own schema, which is why 4713 files walk without a table of type definitions.

**`MetaTypes.SwapBytes(ulong)` is not a byte swap.** It swaps the bytes within
each 32-bit half and leaves the halves where they are, under the reference's own
comment `[not swapping 32bit blocks! careful!]`. Every PSO pointer goes through
it. Reading it as `u64::swap_bytes` makes every array in every PSO file resolve
to block 0, which looks like a broken block table rather than a broken swap.

**Floats are printed the way `FloatUtil.ToString` prints them**: seven
significant digits if that reads back as the same `f32`, nine otherwise. That is
not Rust's shortest round trip — `1086.60546875` is `1086.6055` at eight digits,
which the reference never tries, so it writes `1086.60547`. Ties round away from
zero, which is .NET's rule and not Rust's, so the rounding is done here rather
than by the formatter.

**Strings decode as UTF-8, and the reference decodes them as ASCII.** A byte
above `0x7F` becomes `?` in CodeWalker; here it becomes the character it stands
for, so `credits.ymt` reads `Łukasz Bogaj` rather than `??ukasz Bogaj`. This is
the one deliberate divergence in the layer; `meta_gate.rs` collapses non-ASCII
runs on both sides so every ASCII character is still compared exactly.

**An array of an undefined type is empty, not an error.** The reference resolves
an array's element structure only inside `if (count > 0)`, so a zero-length
array of a type the file never declares is a self-closing tag. Porting the
resolution to the top of the branch turned 3028 nodes across 1877 files into
errors that CodeWalker does not report.

**Both signed and unsigned PSO int arrays print unsigned.** The reference reads
either through `GetUintArray`, so a hash-like value in a signed array comes out
as `3431763157` and not as `-863204139`.

**The tree keeps what XML puts in attributes.** A map entry's key and an RBF
element's attributes are attributes in the reference's output and children here,
because a tree has nowhere else to put them. `meta_gate.rs` flattens both sides
by the oracle's own rule so the comparison stays about what was read.

**A typed record may straddle a block's last byte.** `MetaTypes` decodes records
with `Marshal.PtrToStructure`, which checks only that the record *starts* inside
the block and reads the tail regardless of where it lands — real files rely on
it, e.g. the second archetype of a two-archetype block runs past the block's end
in stock `.ytyp`. `meta/typed.rs` reproduces the bounds rule: a record that
starts past the end is absent, a record that starts inside but runs past it
decodes with the missing tail zero-filled, matching this crate's short-read rule
elsewhere. The fields before the end are real; the fields after it are whatever
the reference read out of adjacent memory, so the gate compares only in-bounds
fields.

**The typed layer decodes by field offset, not by schema.** `GetTypedData<T>`
finds the first block whose structure name matches and reads the raw bytes at
the offsets `MetaTypes.cs` documents. The schema the file carries (the
`MetaStructure` tables) is not consulted for these records — the C# struct layout
is the layout. `meta/typed.rs` keeps a `Decode` impl per record with the
reference's offsets verbatim, and the `.ytyp`/`.ymap` gates compare against the
reference structure for structure.

## Resources

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `resources/reader.rs` | `ResourceData.cs` | **partial** | `ResourceDataReader` only. The writer lands with the first format that serializes. |
| `resources/vertex.rs` | `VertexType.cs`, `VertexDeclaration` | **partial** | Declaration decode and component extraction. Gen9 declarations not ported. |
| `resources/drawable.rs` | `Drawable.cs` (282 KB) | **partial** | Geometry path only: models, geometries, vertex/index buffers, shader name hashes, and the shader group — `ShaderFX` parameters (`ShaderParametersBlock`) with each texture slot resolved to the `TextureBase`'s `NameHash`, plus the embedded `TextureDictionary` when the group carries one. Reads, does not write. |
| `resources/frag.rs` | `Frag.cs` (190 KB) | **partial** | `FragType`, `FragDrawable` (a `DrawableBase` plus a 168-byte fragment tail), `FragBoneTransforms`, `FragGlassWindow`, `FragVehicleGlassWindows`, and the physics LOD tree: `FragPhysicsLODGroup` → `FragPhysicsLOD` → `FragPhysTypeChild`/`FragPhysTypeGroup`/`FragPhysArchetype`. The joint types, articulated body, evt sets and transforms are read through for the pointer chain but not kept. Reads, does not write. |
| `resources/texture.rs` | `Texture.cs` (61 KB) | **partial** | `TextureDictionary`, `TextureBase`, `Texture`, `TextureData` and the mip pitch table. Legacy layout only — the Gen9 branch and `ShaderResourceViewG9` go with the Enhanced edition. Reads, does not write. |
| `resources/clip.rs` | `Clip.cs` (164 KB) | **partial** | `ClipDictionary`, both hash maps, `Clip` (animation and animation-list) and `Animation` headers. The channel codecs, tags and properties are not read: tags and properties are counted. |
| `resources/bounds.rs` | `Bounds.cs` (5488 lines) + `YbnFile.cs` | **partial** | `BoundsDictionary` not ported; a `.ybn` reads one `Bounds` root per `YbnFile.Load`. The type-byte dispatch, the shared 112-byte base, the primitive tails, `BoundGeometry` (quantised vertices, polygons, materials, octants), `BoundBVH` and `BoundComposite` with their transform/bounds/flag arrays and `BVH` blocks. Reads, does not write. |
| `bounds_mesh` (in `resources/bounds.rs`) | `GetVertexPos` + the world-space walk | n/a | Not a port. Flattens the triangle polygons of the whole tree into uploadable buffers, pushing each vertex through the composite transform chain. |

`Particle` (264 KB), `Clothes` (111 KB), `Expression`, `Nav`, `Node`,
`Filter`, `VehicleRecord`, `WaypointRecord`, `ResourceBuilder`,
`ResourceAnalyzer` — all **todo**. (`ShaderParams` is a name table, see below.)

Not read by `resources/drawable.rs`, though their pointers are kept: the
skeleton, joints, light attributes and collision bounds. Each needs a sibling
file that is not ported.

### Notes on shader parameters and textures on models

**A shader parameter names the texture that fills it, by hash.** `ShaderFX`
keeps a `ShaderParametersBlock`: 16-byte records of `(DataType, Unknown_1h,
Unknown_2h, Unknown_4h, DataPointer)` followed by the vector4 data those
pointers reference and a parallel `u32` list of parameter-name hashes. `DataType
0` is a texture slot; the reference (and this port) resolves it to the
`TextureBase` it points at and takes that base's `NameHash` — the Jenkins hash
of the lowercased texture name. The sampler constants that matter to a viewer
come from `ShaderParams.cs`: `DiffuseSampler = 4059966321`, `BumpSampler =
1186448975`, `SpecSampler = 1619499462` (plus `DiffuseHfSampler`,
`TintPaletteSampler`, `DetailSampler`). `shader_gate.rs` pins the whole
parameter list against the oracle, hash for hash.

**Most stock drawables have no embedded dictionary.** The `ShaderGroup` carries
an optional `TextureDictionary`; in practice the pixels live in `.ytd` files the
archetype loads at runtime. The viewer resolves a texture hash by walking the
same chain the reference's renderer does, cheap-first: the drawable's embedded
dictionary, then a `.ytd` sharing the model's basename, then every `.ytd` in
the model's folder. A hash that resolves to an undecodable texture is skipped
and that part renders untextured rather than failing the model.

**Model textures are reoriented for WebGL on the way in, not in the decoder.**
`image.rs` decodes to file order and `texture_gate.rs` pins that against the
reference's `GetPixels`, and `RenderableTexture.Load` uploads the raw data to
D3D with no flip — so file row 0 is the top of the texture in-game, and the
model's `v = 0` samples it. three.js, however, reads UVs in the OpenGL
convention where `v = 0` is the *bottom* of the texture. The three.js viewer
sets `flipY = false` so the top row of the decoded PNG lands on `v = 0`, which
is what lines a model's texture up with its UVs; the default `flipY = true`
would render every part vertically mirrored. Bump maps additionally carry
left-handed (DirectX) tangent space — `NormalMap` in the reference pushes red
along the tangent and green along the bitangent — so the viewer inverts the
green channel, which is the standard DirectX-to-OpenGL normal-map conversion.
Neither is a format change, which is why they live in the viewer and not in
`cw-core`.

**Terrain blends four layers by vertex data** (v0.3.7, read from
`CodeWalker.Shaders/TerrainPS.hlsl` and `TerrainShader.cs`). The ground shaders
(`terrain_cb_w_4lyr*`) bind their colour in `TextureSampler_layer0` to `3`
(`3576369631`, `606121937`, `831736502`, `2025281789`), not in `DiffuseSampler`,
and an optional `lookupSampler` (`2295086480`) the `_cm` variants use as a blend
mask. All four layers are sampled with `TEXCOORD0`; the mask with `TEXCOORD1`.
The weight is `w = mask ? mix(mask, COLOUR1, COLOUR0.a) : COLOUR1`, then
`mix(mix(l0, l1, w.b), mix(l2, l3, w.b), w.g)`. The vertex layouts the
reference lists for them (`PNCCT`, `PNCCTT`, `PNCCTX`, `PNCCTTX`, `PNCTTX`...)
all carry `COLOUR0` and `COLOUR1`, which is why the decode needed both, not
only `COLOUR0`. `MeshPart` carries them for a part that binds any layer
(`layers`, `mask`, `colours` as 8 bytes a vertex, `uvs1`); other parts are
unchanged. Not read: the layer normal maps (`BumpSampler_layer0..3`), specular,
the `pxm` parallax and the `tnt` tint of the golf variants. Open: the reference
comments `vc1 = m; //needs work!` for the `_cm` variants and "perhaps another
sampling of the mask is needed" for the others; this port follows the formula
as written.

**Open question: layer textures outside the model's own chain.** Of the layers
`ch1_05_land5` binds, about half (`cs_rsn_tf_mntscrubs002`,
`cs_rsn_sl_lushgrassrock3`, `dh_ch02_rockface_03/04`, `cs_rsd_ah_roadcut_01a`...)
are not in the drawable's embedded dictionary, nor in the archetype's
`textureDictionary`, nor in the model's folder; they sit in other maps' `.ytd`s
(214 dictionaries hold the five hashes checked). The game gets them through the
parent TXD chain (`CMapParentTxds` in the maps' `_manifest.ymf`, child ->
parent dictionary), which `cw-core` does not read (only the generic tree). Until
it does, a layer that does not resolve is blended as flat grey, which is the
pale patches left on lowland ground. Tracked in issue #32. Next step: a typed `CMapParentTxds` reader
and the parent walk in `resolve_textures_with`.

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

### Notes on textures

**The payload length and the mip layout are two different calculations.**
`TextureData.Read` sizes its read as `stride * height`, quartered per level.
`DDSIO.GetMipmapImages` finds each mip with `ComputePitch`, which clamps a
block-compressed level to one whole 4x4 block. The two disagree for the tail
mips of every compressed texture — a 1x1 DXT1 mip costs 8 bytes, not the 0 the
first arithmetic reaches. Both are ported and both are needed: `Texture::data`
is read the first way, `Texture::mips` addresses it the second. A port that used
one for both would read the right bytes and index them wrongly, or the reverse.

**BC decoding is not a port, and the output differs by a rounding unit.**
`DecompressDxt1Block` expands the two endpoint colours to 8 bits and *then*
interpolates with truncating integer division. The D3D spec, and every GPU,
interpolates in the packed 5/6-bit domain and rounds. `image.rs` uses
`bcdec_rs`, which does the latter. The endpoints agree exactly — both use the
same 565→888 expansion — so only block indices 2 and 3 can move, and the worst
deviation measured over the oracle sample is **1/255**. `texture_gate.rs`
asserts a bound of 4 and prints the worst it saw, so a real decode error cannot
hide under it: a swapped channel or a shifted mip moves a value by tens.

**CodeWalker cannot decode BC7.** `GetPixels` has an empty
`case DXGI_FORMAT_BC7_UNORM: //BC7 TODO!!` and returns null, so a BC7 texture
shows as nothing in the reference. This port decodes it. There is no oracle for
that format; the gate checks it against its own DDS round trip instead.

**Two divergences in the channel mapping, both deliberate.** `D3DFMT_X8R8G8B8`
has no alpha channel — the `X` byte is undefined — and the reference passes the
stored byte through, so a texture with zeroes there renders invisible. This port
forces it opaque, and the gate compares RGB only for that format.
`ConvertBGR5A1ToRGBA8` in the reference computes `v1 * 255 + v0` where the byte
pair means `v1 * 256 + v0`, and advances its output index by `i * 4` while
stepping `i` by 2. Both look like slips rather than format rules; `A1R5G5B5` is
decoded correctly here. It is a rare format and no oracle file uses it.

**The single-channel formats *do* follow the reference, including where that is
lossy.** `D3DFMT_L8` and `D3DFMT_ATI1` carry one channel and land in red alone
rather than spread across RGB as a grey. Matching what CodeWalker draws matters
more than second-guessing what the texture meant, and the gate is exact there.

**Six stock `.ytd` files do not read, in either implementation.** Every one is a
resource whose payload does not inflate to the size its page flags declare — two
extract to zero bytes, the rest to about two thirds:

```text
x64a.rpf\textures\parachute_decals.ytd                 794624 declared,  73349 extracted
x64f.rpf\...\des_setpiece.rpf\v_des_truck_txd.ytd      188416 declared, 123256
x64f.rpf\...\des_fib_ceiling2.rpf\v_des_ceil2_txd.ytd   49152 declared,  28123
x64f.rpf\...\des_hosp_ceil.rpf\des_hosp_ceil.ytd         8192 declared,      0
x64f.rpf\...\des_hosp_ceil.rpf\des_hosp_ceil_txd.ytd   139264 declared,  84675
x64f.rpf\...\des_hosp_ceil2.rpf\des_hosp_ceil2_txd.ytd   8192 declared,      0
```

The reference throws an `ArgumentException` on all six:
`ResourceDataReader`'s constructor slices two `MemoryStream`s out of a buffer
too short for them, and the clamping that would avoid it is commented out. This
port clamps instead, so the header is read out of whatever bytes are there and
comes back as a plausible-looking pile of hashes. `TextureDictionary::read`
catches that with the structural invariant — the name and texture lists are
parallel, so counts that disagree are not a header — and refuses with the reason
attached. The remaining 88,868 dictionaries and 335,729 textures parse.

### Notes on clips**The clip map is what names an animation.** An animation block does not carry
its own hash: the hash table entry that points at it does, and `Animation.Hash`
is written by the reader from there. A clip points straight at the block. The
reference gets the link for free from `ReadBlockAt`, which caches by position;
this port reads the animation map first and keys the animations on their block
address so a clip can find the one it plays.

**Two shapes behind one 112-byte block.** `ClipType` 0 is a single animation with
a rate and a time range; type 1 is a list of animations with weights. Both share
the first `0x50` bytes, and the count of tags is the *second* `u16` of
`ClipPropertyMap` — the first is the bucket capacity, which is larger.

**Four `.ycd` do not read, in either implementation**: the destruction archives
`des_bridge`, `des_setpiece`, `des_fib_ceiling2` and `des_hosp_ceil`, whose
payloads are shorter than their page flags declare. The same six-file failure
mode as the textures above, and the reference throws on them too. The remaining
24840 dictionaries, 315964 clips and 250205 animations resolve.

### Notes on bounds

**The type byte rides inside a shared base.** `BoundsType` lives at the *first*
byte of the 112-byte base struct, and `IResourceXXSystemBlock.GetType` peeks it
by jumping sixteen bytes in — past the `ResourceFileBase` header — and back.
The whole `BoundSphere`/`BoundBox`/`BoundGeometry`/`BoundBVH`/`BoundComposite`
hierarchy is one struct plus a per-type tail, which is how `bounds.rs` models
it: `Bounds` with a `BoundsData` enum. Reading the type from anywhere else —
an extension table, a heuristics guess — mislabels a `Composite` as a `Box`.

**A polygon record keeps its type in the low bits of its first byte.**
`ReadPolygons` masks byte 0 of each 16-byte record with `0xF8` before parsing,
so the area field is read with three bits cleared, and the type is `b0 & 7`.
Triangle vertex indices carry an `0x8000` flag bit that is not an index; the
reference masks it off whenever it addresses a vertex, and the flatten does too.

**Materials are padded to four on the way in and truncated back.** The reader
reads `max(4, MaterialsCount)` entries and then, when the count is below four,
copies out only the real ones. The array handed to callers is always the short
one.

**The octant item pointers are a sibling block, not a tail.** The 32 bytes of
counts are read at `OctantsPointer`; the eight `u64` item pointers live at
`OctantItemsPointer`, which the caller passes as a parameter. Both pointers come
from the geometry header, and neither is implied by the other's position.

**BVH lists come in two widths.** `BVH.Nodes` is a `ResourceSimpleList64b_s` —
32-bit count and capacity, no pad, read by *capacity* — while `BVH.Trees` is a
`ResourceSimpleList64_s` — 16-bit count and capacity plus a pad word, read by
*count*. The two are the same block shape only on paper; reading both as the
`_s` variant makes the node list the wrong length in every geometry that has a
BVH.

**Vertices are stored quantised to `i16` and relative to their geometry's
centre.** `BoundVertex_s` is three shorts; the decoded vertex is `vector *
Quantum`, and its world position adds `CenterGeom` and then pushes through the
composite's child transform — `Transform * (v + CenterGeom)` in the reference's
`GetVertexPos`. The flatten composes the child transforms down the tree so every
triangle lands in the same space.

### Notes on drawable dictionaries

**A `.ydd` names its drawables by hash, and the names are in the drawables.**
`YddFile` builds its lookup from the dictionary's hash list, and the drawable's
own `Name` field is only trusted when it hashes back to the entry it sits under.
Where it does not, the entry shows as `hash_XXXXXXXX`, which is what the
reference displays. A `.ydr` opens through the same path as a dictionary of one.

### Notes on fragments

**A `.yft` is a shell around drawables, not a drawable itself.** `FragType`
holds one main `FragDrawable`, one cloth, an alternative array and a physics
LOD tree whose children each carry up to two more drawables (intact/damaged).
The reference's `YftFile.Load` reads the root, then `AssignChildrenShaders`
walks the physics tree giving each child the main drawable's shader group; the
viewer enumerates the same set via `all_drawables`, named by where they sit
(`drawable`, `cloth`, `drawable0`, `lod1.child3.drawable2`).

**`FragDrawable` is a `DrawableBase` plus a fixed 168-byte tail.** The base is
the same 168 bytes a `.ydr`'s drawable opens with — shader group, bounds, LOD
model pointers — so the geometry reader is reused verbatim and the tail adds
the fragment matrix, a bounds pointer and optional transform arrays. The whole
thing is 336 bytes; a port that treated a `FragDrawable` as a full `Drawable`
(208 bytes) would misread the tail as geometry.

**The physics chain is read through even when only the drawables matter.** The
joint types dispatch on a byte at offset 0x15 of the block (0 → 1-DOF, 1 →
3-DOF, anything else → null), and `FragPhysGroupNamesBlock` re-reads each
group's embedded name struct from its own pointers. Vanilla files embed the
names in the groups, so the sibling block is skipped; the group name struct is
ten little-endian uints packing forty ASCII characters, which `ToString` emits
LSB-first and trims at the first NUL.

**Not ported:** the articulated body and joint structures beyond the type
dispatch (they are read through, not kept), `EnvironmentCloth`, `LightAttributes`,
`FragPhysTransforms` (the joint matrices), and writing.

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

All **todo** as files, though `YdrFile.cs` and `YddFile.cs` are covered by
`resources::read_named_drawables`, `YftFile.cs` by `resources::read_fragments`,
`YtdFile.cs` by `resources::read_textures`, `YcdFile.cs` by
`resources::read_clips`, `YmtFile.cs` by `meta::read_meta`, `YbnFile.cs` by
`resources::read_bounds`, `Gxt2File.cs` by `gxt2::Gxt2File`, and
`AwcFile.cs` by `audio::AwcFile`.

| Unit | C# reference | Status | Notes |
| --- | --- | --- | --- |
| `filetypes/ytyp.rs` | `YtypFile.cs` | **partial** | The typed model over the meta tree: `CMapTypes`, the archetype pointer array dispatched by target structure (`CBaseArchetypeDef`/`CTimeArchetypeDef`/`CMloArchetypeDef`), MLO children (rooms, portals, entity sets, timecycle modifiers), `GetExtensions` results and `CCompositeEntityType`. 2759 `.ytyp` across the install read. Reads, does not write. |
| `filetypes/ymap.rs` | `YmapFile.cs` | **partial** | `CMapData`, the entity pointer array split into `CEntityDef`/`CMloInstanceDef` and the `EnsureEntities` hierarchy, car gens, timecycle modifiers, box occluders, occlude models, LOD and distant-LOD lights, grass batches, physics dictionaries and strings. 19,383 `.ymap` across the install read, 3.07M entities. Reads, does not write. |

Planned order: `YndFile`, then the rest. `YmapFile` and `YtypFile` are metadata
containers, so they already open in the Explorer as trees; what these readers
add is the typed model above the tree.

### Notes on GXT2

**GXT2 is the first file type to round-trip, so it is `done`.** Unlike the
resource formats, the reference has a full `Save`, and `gxt2::Gxt2File::write`
mirrors it: the entry offsets are filled as the strings are laid out after the
second `GXT2` header, so parse → serialize with no edits reproduces the input
byte for byte. Every one of the 19,477 `.gxt2` in the install round-trips, and
the sampled entries agree with the reference hash for hash, text for text.

**The reference's early returns are errors here.** `Gxt2File.Load` returns
leaving the fields defaulted when either `GXT2` magic is missing, so a wrong
file quietly reads as empty. This port refuses instead, because a file the
extension calls GXT2 that does not start with `GXT2` is not a GXT2 file and a
viewer that shows nothing would look like a bug.

**Offsets are absolute within the payload.** The entry table's `Offset` field
points at the string's NUL-terminated bytes directly; the second `GXT2` magic
and the `end` length after it are checked but the strings are not relative to
them. Entries are stored in the file's own order and only `FromText` sorts
them, by hash.

`AwcFile` · `Builders` · `CacheDatFile` · `CarColsFile` · `CarModColsFile` ·
`CarVariationsFile` · `CutFile` · `DistantLightsFile` · `DlcContentFile` ·
`DlcSetupFile` · `FxcFile` · `GtxdFile` · `Gxt2File` · `HeightmapFile` ·
`JPsoFile` · `MrfFile` · `PedFile` · `PedsFile` · `RelFile` (1.2 MB) · `Stats` ·
`VehicleLayoutsFile` · `VehiclesFile` · `WatermapFile` · `YcdFile` ·
`YddFile` · `YdrFile` · `YedFile` · `YfdFile` · `YftFile` · `YldFile` ·
`YmfFile` · `YmtFile` · `YndFile` · `YnvFile` · `YpdbFile` ·
`YptFile` · `YtdFile` · `YvrFile` · `YwrFile`
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

Filled in during phase 4, when `CodeWalker/Shaders/*.hlsl` is translated to
**GLSL** for three.js. WGSL was the plan until the phase 3.0 spike chose
three.js in the WebView over a native wgpu surface; there is no wgpu renderer
and no WGSL. Any shader that does not translate cleanly gets a row here naming
the approximation used. Unlit flat materials are not an acceptable default: an
unrecognised shader still resolves to a profile built from what the material
binds plus its render bucket.

Approximations already in the tree, before the shader work proper:

| Unit | Approximation |
| --- | --- |
| BC5/ATI2 normal maps | Blue is left at zero rather than reconstructed as `sqrt(1 - x² - y²)`. The PNG path did the same, so this is not a regression, but it is wrong and belongs in the uber-shader. None appeared in a 510-texture sample of placed city geometry (93.3% BC1, 6.5% BC3). |
| BC4/ATI1 | Decoded into red with green and blue at zero, following the reference. |
| Normal map handedness | Corrected with a negative `normalScale.y` in the shader rather than by inverting the green channel on the CPU, which is not possible without decoding the blocks. |
| Terrain | Four layers blended by `COLOUR0` / `COLOUR1` / `TEXCOORD1` as in `TerrainPS.hlsl`, lit by the standard material. No layer normal maps, specular, parallax or tint. |

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
