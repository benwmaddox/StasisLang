# Stasis game templates (headless)

These are small, deterministic templates meant to exercise Stasis' static global memory + SoA style.
They do not require the graphics runtime; they emit snapshots (SVG/CSV) into `build/`.

## Run

From the repo root:

- `.\stasis.bat run .\examples\templates\factorio_lite.stasis --backend llvm`
  - Writes `build/factorio_lite_<tick>.svg`
- `.\stasis.bat run .\examples\templates\breakout_defense.stasis --backend llvm`
  - Writes `build/breakout_defense_<tick>.svg`
- `.\stasis.bat run .\examples\templates\match3_overlay.stasis --backend llvm`
  - Writes `build/match3_combo_hist.csv`

If you want to validate compilation only (no execution), add `--emit-ir`:

- `.\stasis.bat run .\examples\templates\factorio_lite.stasis --emit-ir > $null`

## Files

- `examples/templates/template_io.stasis`: ASCII int formatting + file writing + snapshot naming helpers
- `examples/templates/template_svg.stasis`: tiny SVG builder used by the templates
- `examples/templates/factorio_lite.stasis`: belt simulation + one assembler + SVG snapshots
- `examples/templates/breakout_defense.stasis`: bouncing balls + one tower + projectiles + SVG snapshots
- `examples/templates/match3_overlay.stasis`: match detection + deterministic cascades + CSV histogram
