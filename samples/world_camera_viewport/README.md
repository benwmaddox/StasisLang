# World camera viewport sample

This sample keeps a 4096x4096 authoritative world while presenting a 640x320
logical viewport below a screen-anchored HUD. Controls transition at 20 Hz with
a two-tick delay and simulation advances independently at 60 Hz. The current
host renders once immediately after each tick, so live rendering selects the
completed endpoint with alpha `1.0`. The test-only cadence probe models future
120 Hz presentation by sampling alpha `0.5` and `1.0` per simulation tick.

```powershell
stasis --workspace samples/world_camera_viewport prepare
stasis --workspace samples/world_camera_viewport test --json
stasis --workspace samples/world_camera_viewport record --output artifacts/world_camera_viewport --width 640 --height 360 --fps 60 --frames 120
```

The blue HUD bar advances with simulation ticks. The orange bar counts the
much less frequent control transitions. Procedural tiles are selected only
from the bounded visible range. Oversized spans select a coarser effective tile
size so the complete viewport remains covered; the whole map is never
rasterized or walked.
