# Porting guide

How to move one piece of `CodeWalker.Core` from C# to Rust without changing its
behaviour by accident.

## The reference source

```
E:\Ad Astra Tools\CodeWalkerAstra
```

`adastra-org/CodeWalker`, pinned at commit **`d11e5951`** ("Merge branch
'pr-318'"), upstream `dexyfex/CodeWalker`. It is read in place and **never
modified**. When you re-pin, update this commit and note what moved in
`formats.md`.

Two other things on disk are worth knowing about:

- `E:\Ad Astra Tools\CodeWalker` — CodeWalker built 2025-03-17. Both a GUI to
  compare against and, more usefully, a **loadable oracle** (below).
- `E:\Ad Astra Tools\RPFTool` — a working TypeScript RPF reader with real test
  fixtures. It only handles unencrypted (`OPEN`) archives, but for those it is a
  second independent implementation to diff against.

## The loop

For each unit, in this order. Do not skip step 1; do not reorder 2 and 3.

1. **Read the C#.** Open the actual `.cs`. Not your memory of the format, not a
   wiki page. If the reference is ambiguous, mark it `TODO(port): <question>` in
   the Rust source and record the question in `formats.md`. Never guess quietly.
2. **Write the failing test.** For a parser this is the round trip: read a real
   fixture, serialize it back, assert the bytes are identical. Run it. Watch it
   fail for the right reason.
3. **Implement.** Little-endian, packed, `f32` — never widen a serialized field
   to `f64`.
4. **Watch it pass.**
5. **Commit.** `feat(core): port YmapFile` / `test(core): round-trip ymap`.

## Getting expected values: load the reference DLL

The most valuable trick in this port. `CodeWalker.Core.dll` targets
netstandard2.0, so Windows PowerShell 5.1 loads it directly and you can call the
real implementation to generate expected values:

```powershell
Add-Type -Path "E:\Ad Astra Tools\CodeWalker\CodeWalker.Core.dll"
[CodeWalker.GameFiles.JenkHash]::GenHash("x64a.rpf")
```

Expected values produced this way come from the implementation you are porting.
Values you derive by hand come from your understanding of it, which is the thing
under test. Prefer the former, and say in the test comment where they came from.

## Worked example: `JenkHash`

Jenkins one-at-a-time. Every name in the game files is identified by it.

### The C#

`CodeWalker.Core/GameFiles/Utils/Jenk.cs`:

```csharp
public static uint GenHash(string text)
{
    if (text == null) return 0;
    uint h = 0;
    for (int i = 0; i < text.Length; i++)
    {
        h += (byte)text[i];
        h += (h << 10);
        h ^= (h >> 6);
    }
    h += (h << 3);
    h ^= (h >> 11);
    h += (h << 15);
    return h;
}
```

### The two traps

**Overflow.** C# arithmetic is unchecked by default, so every `+` above wraps
silently. Rust panics on overflow in debug builds. A direct transcription
compiles and then aborts on the first long name. Every addition becomes
`wrapping_add`. The shifts are all below 32, so `<<` and `>>` carry over as-is.

**`(byte)text[i]`.** A C# `char` is a UTF-16 code unit, and the cast truncates it
to its low 8 bits. That is not UTF-8 encoding, and it is not what
`GenHash(text, UTF8)` does. The sibling overloads genuinely disagree — asking the
reference DLL for `"étest"` gives three different answers:

| Overload | Result |
| --- | --- |
| `GenHash(s)` — low-byte truncation | `0xC709AB9C` |
| `GenHash(s, UTF8)` | `0xA91B0629` |
| `GenHash(s, ASCII)` — `?` substitution | `0x84E0894A` |

They agree on ASCII, which is every real game file name, so a wrong choice here
would pass every realistic test and stay wrong. Keep the overloads distinct and
pin the divergence with a test.

### The Rust

`crates/cw-core/src/utils/jenk.rs`:

```rust
pub fn gen_hash(text: &str) -> u32 {
    let mut h: u32 = 0;
    for unit in text.encode_utf16() {
        h = h.wrapping_add((unit as u8) as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}
```

`encode_utf16().map(|u| u as u8)` is the exact analogue of iterating C# `char`s
and casting each to `byte`.

### The tests

Three of them, each pinning something different:

- `matches_reference_gen_hash` — 14 vectors from the reference DLL.
- `encodings_diverge_on_non_ascii` — the table above, so a future "cleanup" that
  collapses the overloads fails loudly.
- `arithmetic_wraps_instead_of_panicking` — a 4096-character input, which
  overflows `u32` many times and would abort on a plain `+`.

## Naming

Keep the C# names, translated to Rust idiom. `YmapFile.cs` becomes
`ymap_file.rs`; `GenHash` becomes `gen_hash`; domain words (`drawable`,
`archetype`, `lod`, `entity_def`) are kept verbatim. Someone holding both files
side by side should not have to translate vocabulary as well as syntax.
