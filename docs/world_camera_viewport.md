# Deterministic world and camera viewport

`src/stdlib/world_camera.stasis` presents a two-dimensional simulated world
through a logical display rectangle. It is deliberately a presentation helper,
not a simulation system. Gameplay positions, collision, scoring, snapshots,
replay, and networking stay in world space. A renderer supplies a completed
world position to `world_camera_follow`, converts coordinates, draws inside a
balanced `world_camera_clip_begin` / `world_camera_clip_end` pair, and discards
the derived camera state.

## Camera contract

`WorldCamera` stores explicit world minimum and maximum bounds, logical
viewport position and dimensions, world-to-view scale, and the derived camera
origin. Non-positive scale becomes `1`; negative viewport dimensions become
zero; reversed world maxima collapse to their minima. These choices make bad
inputs deterministic without expanding the world.

The visible world span is `viewport / scale`. When the world is larger than
that span, the desired origin is `followed_point - span / 2`, inclusively
clamped to `[world_min, world_max - span]`. Equality stays exactly at the
boundary. The immediately adjacent interior value scrolls by the same amount;
the immediately adjacent exterior value remains clamped. When a world axis is
smaller than or equal to the visible span, it is centered and following cannot
move it. `world_to_view_*` and `view_to_world_*` are inverse operations over
the same supplied camera.

Hosts and guests must call the shared helper with the same completed state.
They then get identical projection and clip geometry without synchronizing a
camera. Camera origin, interpolation alpha, and resident tiles never enter an
authoritative payload.

## Presentation history and realtime controls

`WorldCameraHistory` retains only the previous and current completed render
positions. `interpolated_*` clamps alpha to `[0, 1]`; alpha zero selects the
previous endpoint, one selects the latest endpoint, and intermediate alpha is
linear. Reset and a caller-declared teleport collapse both endpoints so a
render frame cannot sweep across a discontinuity.

Interpolation never manufactures a simulation tick. With
`realtime_controls.stasis`, control transitions arrive independently and latch
persistent intent. Each 60 Hz simulation tick still calls `realtime_advance`,
reads that intent, advances authoritative world state, and updates history.
Presentation can run at another rate and interpolate the two completed
endpoints. The current desktop host invokes one render immediately after each
tick; it does not yet consume `presentation_hz`, so the live sample uses alpha
`1.0` and displays the completed endpoint without adding midpoint lag. The
sample's deterministic cadence probe separately models a future 120 Hz host by
sampling alpha `0.5` then `1.0` for each of 60 simulation ticks. It proves 120
presentation samples alongside only three accepted 20 Hz control transitions.

## Bounded tile residency

`WorldTileRange` computes a half-open visible tile rectangle. Padding is
clamped to two tiles, each axis is capped at 16 tiles, and the total is capped
by construction at 256. If the requested tile size cannot cover the padded
visible span within those bounds, calculation doubles the effective tile world
size until the complete span fits. The result exposes that effective size and
coarsening count; renderers must position and size tiles with it. Invalid tile
size produces status `-1`, while exhausting the bounded 64-step coarsening
search produces status `-2` and an empty range rather than partial coverage.
This makes procedural or chunked rendering proportional to the viewport rather
than the map dimensions; a full-map raster is neither required nor appropriate.

The density helper requests `logical_tile_pixels * density`, then applies a
resident-count tier so RGBA residency remains at or below 64 MiB:

| Resident tiles | Maximum raster side | Maximum RGBA bytes |
| ---: | ---: | ---: |
| 1-4 | 2048 | 64 MiB |
| 5-16 | 1024 | 64 MiB |
| 17-64 | 512 | 64 MiB |
| 65-256 | 256 | 64 MiB |

Measured deterministic representative: a 256 logical-pixel tile requested at
4x density with 256 resident tiles resolves to 256x256 RGBA and exactly
67,108,864 bytes. A central 640x320 viewport over 64-unit tiles with one tile
of padding submits 12x7 tiles: 84 tile rectangles and 1.68x geometric overdraw.
Density changes raster detail and byte residency, not the bounded tile count.
A 4096x2048 visible span requested at 64-unit tiles coarsens three times to
512-unit tiles and returns an 11x7 range whose edges cover the complete span.

## Evidence

- `tests/stasis/world_camera.test.stasis` covers equality and adjacent clamp
  values, centering, conversions, alpha endpoints and midpoint, teleport,
  supplied-state parity, clipping, tiling, and the high-density budget.
- `world_camera_viewport_seam` runs the projection and ordered clip result
  through both JIT and a linked AOT executable.
- `samples/world_camera_viewport` demonstrates 60 Hz simulation and delayed
  20 Hz controls with an anchored HUD. Its cadence probe models the 120 Hz
  presentation phases that a future host cadence consumer can supply.
