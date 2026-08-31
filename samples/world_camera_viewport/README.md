# World camera viewport sample

This sample keeps a 4096x4096 authoritative world while presenting a 640x320
logical viewport below a screen-anchored HUD. Controls transition at 20 Hz with
a two-tick delay, simulation advances independently at 60 Hz, and presentation
uses bounded interpolation between the latest two completed states.

```powershell
stasis --workspace samples/world_camera_viewport prepare
stasis --workspace samples/world_camera_viewport test --json
stasis --workspace samples/world_camera_viewport record --output artifacts/world_camera_viewport --width 640 --height 360 --fps 60 --frames 120
```

The blue HUD bar advances with simulation ticks. The orange bar counts the
much less frequent control transitions. Procedural tiles are selected only
from the bounded visible range; the whole map is never rasterized or walked.
