# Vehicle flags, glass and tint: findings

What was measured on the Ad Astra server (`E:\FiveM\Astra\resources`, 2026-11-12) and what is still open. Companion to [vehicle-tools.md](vehicle-tools.md).

## Source of truth

Every game reference comes from the official Cfx docs, not from memory or from CodeWalker's name registry:

- Index: <https://docs.fivem.net/docs/game-references/>
- Vehicle flags: <https://docs.fivem.net/docs/game-references/vehicle-references/vehicle-flags/> (204 flags, numbered 0 to 203, with a description and the build that introduced each)

CodeWalker's `MetaNames.cs` is a hash-name registry. It lists names the game never defined, so it is not a validator. The first pass of this audit used it and marked `FLAG_HAS_BULLETPROOF_GLASS` and similar names as valid by accident; the numbers below use the official list.

Not in that section: collision materials. There is no page for them, so their names come from the game's own `materials.dat` (see "Glass" below).

## 1. Flags in `vehicles.meta`

Scan: every `vehicles.meta` under `resources` (1,072 models) checked against the 204 official flags.

**86 uses of flags the game does not define, in 45 files.**

| Flag in the server | Uses | Official name |
|---|---|---|
| `FLAG_HAS_EXTRA` | 29 | none |
| `FLAG_HAS_INTERIOR_EXTRA` | 29 | `FLAG_HAS_INTERIOR_EXTRAS` |
| `FLAG_HAS_EXTRA_ALL` | 7 | probably `FLAG_EXTRAS_ALL` |
| `FLAG_RESPRAY` | 7 | none (the default already allows respray; the opposite is `FLAG_NO_RESPRAY`) |
| `FLAG_HAS_EXTRAS` | 7 | none |
| `FLAG_INCREASED_RAMMING_FORCE` | 2 | `FLAG_HAS_INCREASED_RAMMING_FORCE` |
| `FLAG_CAPPED_EXPLOSION_DAMAGE` | 1 | `FLAG_HAS_CAPPED_EXPLOSION_DAMAGE` |
| `FLAG_HAS_GULLWING_DOORS` | 1 | `FLAG_HAS_GULL_WING_DOORS` |
| `FLAG_NO_ROOF` | 1 | `FLAG_HAS_NO_ROOF` |
| `FLAG_NO_DOORS` | 1 | none |
| `FLAG_EXTRAS_SUNROOF` | 1 | none |

Flags the docs mark as "Does nothing" (safe to leave, noise to remove): `FLAG_NO_BOOT` (171 uses), `FLAG_BOOT_IN_FRONT` (13 uses; the docs say to use `FLAG_FRONT_BOOT`).

Planned tool: `vehicle_flags`. Reports unknown and no-op flags, applies only the renames above that have an official equivalent, leaves ambiguous ones listed, with backup and rollback like `yft_optimize`.

## 2. Glass: flag and material

Two things decide how a window behaves, and the flag is the smaller one.

**Flags** (official descriptions):

| Flag | # | Effect |
|---|---|---|
| `FLAG_HAS_BULLETPROOF_GLASS` | 76 | Prevents glass from breaking |
| `FLAG_HAS_BULLET_RESISTANT_GLASS` | 120 | Vehicle's glass is resistant to bullets |

They are different behaviours, not synonyms. The server uses `BULLET_RESISTANT` on 22 entries and `BULLETPROOF` on 8.

**Material.** Each collision polygon in the `.yft` has a material type, and that decides how the surface reacts to bullets. Faku's own example: `rrfaku` has resistant but not unbreakable glass and its `vehicles.meta` carries neither flag. So the material alone gives the resistance.

`yft_glass` (read-only example in `crates/cw-core/examples/`) dumps the material types of a `.yft` and its glass-window table:

```bash
cargo build --release -p cw-core --example yft_glass
./target/release/examples/yft_glass.exe "E:/FiveM/Astra/resources/.../car.yft"
```

Measured on 7 vehicles (polygon counts left out; types are the raw `material_type`):

| Vehicle | Flag in meta | Material types |
|---|---|---|
| `rrfaku` | none | 116, 122, 142 |
| `s500w223` | BULLETPROOF | 116, 121, 123, 142 |
| `nuketruck` | BULLETPROOF | 116, 122, 142 |
| `caliburn` | BULLETPROOF | 116, 121, 142 |
| `aleutianems` | RESISTANT | 116, 117, 120, 121, 142 |
| `hx58` | RESISTANT | 0, 70, 116, 117, 118, 122, 142 |
| `bfdsuv` | none | 116, 117, 120, 121, 122, 142 |

Names for those numbers, from the game's own `materials.dat` (full table in [collision-materials.md](collision-materials.md)):

| Type | Name |
|---|---|
| 116 | `CAR_METAL` |
| 117 | `CAR_PLASTIC` |
| 118 | `CAR_SOFTTOP` |
| 120 | `CAR_GLASS_WEAK` |
| 121 | `CAR_GLASS_MEDIUM` |
| 122 | `CAR_GLASS_STRONG` |
| 123 | `CAR_GLASS_BULLETPROOF` |
| 142 | `PHYS_CAR_VOID` |
| 70 | `WOOD_SOLID_MEDIUM` |
| 0 | `DEFAULT` |

Reading the samples with those names:

- `rrfaku` uses `CAR_GLASS_STRONG`: resistant, not unbreakable. That matches Faku's description and confirms the offset of the table.
- Only 123 `CAR_GLASS_BULLETPROOF` is really unbreakable (penetration resistance 10000, no shoot-through flags). Of the three BULLETPROOF-flagged samples only `s500w223` has it, on 8 polygons; `nuketruck` and `caliburn` carry the flag but their glass is `STRONG` / `MEDIUM`.
- `bfdsuv` has no glass flag and uses `WEAK`, `MEDIUM` and `STRONG` mixed, per window.
- `aleutianems` and `hx58` carry `BULLET_RESISTANT` with `WEAK`, `MEDIUM`, `STRONG` only.
- So the flag and the material are independent: the material decides how the glass takes a bullet, the flag adds its own behaviour on top. A glass audit has to read both.
- The `.yft` also carries a vehicle glass-window table (`yft_glass` prints it). What its fields mean for armor is not worked out yet.

## 3. Tint on lights

Not reproduced yet. Hypothesis (details in [vehicle-shaders.md](vehicle-shaders.md)): the lights use a paint shader, a `[PAINT:X]` material name or `vehicle_vehglass` instead of `vehicle_lightsemissive`, or share UVs with a painted region. Detecting it from the `.yft` is possible by listing the shader of every geometry. Fixing it means editing materials or geometry through the C# writer, which is itself unverified in game.

Needs a concrete vehicle where the lights get tinted to reproduce it.

## 4. LODs

Done: `yft_optimize --lods`. See [vehicle-tools.md](vehicle-tools.md). Still unverified in game.
