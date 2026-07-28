# Display metrics contract

Stasis games author rendering and input in one logical top-left coordinate
space. `init_window(360, 720, ...)` therefore selects a `360 x 720` logical
canvas even when the platform owns a `1080 x 2400` native window and drawable.
Native resize, orientation, density, and surface changes never silently replace
the requested logical dimensions.

The runtime tracks these values separately:

- logical size: stable game coordinates requested by `init_window` or
  `set_window_size`
- native size: platform window or surface coordinates used by pointer events
- drawable size: actual render-target pixels
- content viewport: the centered aspect-fit rectangle in the native/drawable
  surface; unused pixels are letterbox or pillarbox space
- safe viewport: the platform-usable area intersected with the content viewport
  and reported in logical coordinates
- content scale: drawable content pixels per logical pixel, including values
  below `1.0` when the canvas is downscaled
- raster scale: the effective SVG/font cache density, clamped to `1.0..8.0` so
  low-resolution surfaces do not destroy source detail and extreme surfaces do
  not create unbounded rasters

Pointer positions are transformed from native coordinates through the content
viewport, then clamped to the logical canvas. Normalized pointer coordinates
are relative to the safe logical viewport. The forward and inverse transform
use the same aspect-fit values, including letterbox offsets.

## HostFrame API

HostFrame version 2 keeps indices `1/2` as logical-width/logical-height
compatibility aliases and adds explicit display fields at `host_i32[20..31]`:

| Indices | Meaning |
| --- | --- |
| `20, 21` | logical width, height |
| `22, 23` | native width, height |
| `24, 25` | drawable width, height |
| `26..29` | safe logical x, y, width, height |
| `30` | display generation |
| `31` | density generation |

`host_f32[48]` is content scale and `host_f32[49]` is raster scale. The Stasis
stdlib exposes these through `gfx_logical_*`, `gfx_native_*`, `gfx_drawable_*`,
`gfx_safe_viewport_*`, `gfx_content_scale`, `gfx_raster_scale`, and the two
generation accessors. Native hosts can call `stasis_get_display_metrics`.

The display generation advances once when any logical, native, or drawable
extent changes. The density generation advances only when the effective raster
scale changes. Callers can therefore rebuild layout or surface state on display
generation while invalidating density-dependent resources exactly once per
cache-key change.

## Resource cache policy

SVG entries are keyed by canonical source identity, logical target extent, the
current raster scale, and the fixed raster options used by the shared runtime.
Font and cached-text entries use logical font size plus the same raster scale.
Raster dimensions use checked, bounded `ceil(logical_extent * raster_scale)`;
the logical draw size remains unchanged.

Desktop and packaged mobile builds perform this policy in
`runtime/stasis_graphics.c`. Workshop and bundled Published previews receive
the same logical/native/drawable metadata in reserved gfx_cmd v2 header slots,
use the same aspect-fit viewport, and replace SVG/font/text textures when the
density generation changes. Those metadata slots are host-populated and are
not part of the command trace, so JIT/AOT command parity is unchanged.
