# Brickout Defense / Brickout Revenge

This sample is a faithful Stasis port of the published 2016 Flash game. The
published `TowerDefense.swf`, its recovered ActionScript, and the original
`Graphics/TD` artwork are authoritative. See `original_reference.md` for the
recovery contract and exact source-derived statistics.

## Core loop

The player buys and places tower blocks across a 32 x 32 collision grid. Enemy
balls enter in timed groups and try to reach the bottom wall. A computer-driven
paddle at the top keeps balls in play while towers acquire targets and launch
homing projectiles. The player wins when every scheduled group and living ball
is gone, and loses after twenty balls breach the bottom.

The simulation runs at a deterministic 60 Hz, matching the Flash build. Desktop
and Android use the same fixed-tick state and the same 660 x 550 virtual canvas.

## Content recovered from the original

- Towers: wall, standard, rapid, healer, sniper, rocket, and gold miner.
- Balls: basic, fast, slow, curve, swarm, splitter, fire, and mover.
- Campaign levels: Marathon and Split & Swarm.
- Challenge level: the published Test Level with gold miners.
- Former challenge: Earthquake, recovered from the SWF but not linked by the
  final Flash campaign menu.
- Original background, paddle, tower frames, shop art, buttons, selector, ready
  LED, embedded fonts, and source-derived vector HUD assets.

## Controls

Desktop:

- `1`-`6`: select a tower from the two shop rows
- Pointer click: select a tower or place the selected shop tower
- `U`: upgrade the selected tower
- `S`: sell the selected tower
- `N`: release the next enemy group immediately
- `P`: pause
- `Esc`: cancel selection or return to the previous menu

Android:

- Tap a menu entry to open it.
- Tap a shop tile to select a tower, then tap the playfield to place it.
- Tap an existing tower to inspect it.
- Tap the original upgrade, sell, cancel, pause, and next-wave buttons.

## Run

```powershell
cargo run -p stasis -- play samples\brickout_revenge\brickout_revenge_v1.stasis
```

`brickout_revenge.stasis` remains a friendly alias. The prior speculative Stasis
recreation is retained as `brickout_revenge_modern_attempt.stasis` for reference
but is not part of the canonical sample path.
