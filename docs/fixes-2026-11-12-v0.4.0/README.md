# Fixes 2026-11-12 (v0.4.0): vehicles

`[ ]` pending · `[~]` partial, with note · `[x]` done and verified. Findings in [../vehicle-flags-and-glass.md](../vehicle-flags-and-glass.md).

## Server vehicles (reported by Faku)

- [~] **Armor the windows.** Two parts: the flag (`FLAG_HAS_BULLET_RESISTANT_GLASS` vs `FLAG_HAS_BULLETPROOF_GLASS`) and the collision material of the glass. The material types 120 to 123 are measured; their names are not (need `materials.dat`). No tool yet.
- [ ] **Apply / remove tint.** Lights get tinted when UVs are wrong. Not investigated; needs a vehicle that shows it.
- [~] **Missing LODs.** `yft_optimize --lods` generates them, with backup, rollback and report. Verified on a scratch copy; not yet in game.
- [~] **Misconfigured flags.** 86 uses of 11 flags the official list does not define, in 45 files (table in the findings doc). Reported; no `vehicle_flags` tool yet.

## References

- [x] All game references must come from <https://docs.fivem.net/docs/game-references/>. The flag audit was redone against the official vehicle-flags page (204 flags).
- [ ] Collision materials have no page in that section: names must come from the game's `materials.dat`.

## Open decisions

- [ ] Per vehicle: `BULLET_RESISTANT` (glass resists) or `BULLETPROOF` (glass never breaks)? The two are not interchangeable.
- [ ] Ambiguous flags (`FLAG_HAS_EXTRA`, `FLAG_HAS_EXTRAS`, `FLAG_RESPRAY`, `FLAG_NO_DOORS`, `FLAG_EXTRAS_SUNROOF`): remove or map?
