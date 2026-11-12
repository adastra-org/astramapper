# Fixes 2026-11-12 (v0.4.0): vehicles

`[ ]` pending · `[~]` partial, with note · `[x]` done and verified. Findings in [../vehicle-flags-and-glass.md](../vehicle-flags-and-glass.md).

## Server vehicles (reported by Faku)

- [~] **Armor the windows.** Two parts: the flag (`FLAG_HAS_BULLET_RESISTANT_GLASS` vs `FLAG_HAS_BULLETPROOF_GLASS`) and the collision material of the glass (`CAR_GLASS_WEAK`/`MEDIUM`/`STRONG`/`BULLETPROOF`, types 120 to 123, named from `materials.dat`). Measured on 7 vehicles; no tool that sets them yet.
- [ ] **Apply / remove tint.** Lights get tinted when UVs are wrong. Hypothesis written down ([vehicle-shaders.md](../vehicle-shaders.md)); needs a vehicle that shows it.
- [~] **Missing LODs.** `yft_optimize --lods` generates them, with backup, rollback and report. Verified on a scratch copy; not yet in game.
- [~] **Misconfigured flags.** 86 uses of 11 flags the official list does not define, in 45 files (table in the findings doc). Reported; no `vehicle_flags` tool yet.

## References

- [x] All game references must come from <https://docs.fivem.net/docs/game-references/>. The flag audit was redone against the official vehicle-flags page (204 flags).
- [x] Collision materials have no page in that section: names taken from the game's `materials.dat` ([collision-materials.md](../collision-materials.md)).
- [ ] <https://gtaforums.com/topic/899267-entities-and-materials/> could not be read (Cloudflare challenge). Faku can paste the relevant part if it matters.

## Open decisions

- [ ] Per vehicle: `BULLET_RESISTANT` (glass resists) or `BULLETPROOF` (glass never breaks)? The two are not interchangeable.
- [ ] Ambiguous flags (`FLAG_HAS_EXTRA`, `FLAG_HAS_EXTRAS`, `FLAG_RESPRAY`, `FLAG_NO_DOORS`, `FLAG_EXTRAS_SUNROOF`): remove or map?
