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
- available presentation extent: the platform's safe-area-adjusted space in
  which a guest may choose a presentation size; it is independent of the
  current logical canvas, native window/canvas, and drawable backing
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

HostFrame version 4 keeps the canonical physical fields in `host_i32` and the
logical/presentation fields in `host_f32`:

| Indices | Meaning |
| --- | --- |
| `22, 23` | native width, height |
| `24, 25` | drawable width, height |
| `30` | display generation |
| `31` | density generation |

| f32 indices | Meaning |
| --- | --- |
| `48, 49` | content scale, raster scale |
| `50, 51` | logical width, height |
| `52..55` | safe logical x, y, width, height |
| `56, 57` | available presentation width, height |

The Stasis stdlib exposes these through `gfx_logical_*`, `gfx_native_*`,
`gfx_drawable_*`, `gfx_safe_viewport_*`, `gfx_available_width`,
`gfx_available_height`, `gfx_content_scale`, `gfx_raster_scale`, and the two
generation accessors. Available presentation values are scalar platform units:
CSS pixels after safe-area accounting on Web, desktop usable-area units on
the native window's current display (with the primary display only as a
fallback), and platform surface units on Android. They are populated
before guest `main()` and every `tick()`. Native hosts can call
`stasis_get_display_metrics` for the pre-existing display geometry.

The display generation advances once when any logical, native, drawable, or
available-presentation extent changes. This includes safe visible viewport
changes on Web even when the fitted CSS canvas remains pinned at the same size.
The density generation advances only when the effective raster scale changes.
Callers can therefore rebuild responsive layout or surface state on display
generation while invalidating density-dependent resources exactly once per
cache-key change.

For SDL renderer hosts, drawable size comes from `SDL_GetRenderOutputSize` and
always names the complete physical render target. It is not the current fitted
logical-presentation output. The latter can retain the previous canvas's
letterboxed dimensions during a portrait/landscape transition. Stasis first
samples the full backing, derives the fitted drawable viewport separately, and
then applies the requested logical presentation. Consequently a logical canvas
change can alter content scale and density without ever replacing the backing
receipt with a stale fitted-content size.

## Web display boundary

The browser host keeps four extents separate: the available safe visible
viewport, the guest logical canvas, the CSS rectangle selected by the shell
fitter, and the physical canvas backing. The shell publishes the available
extent before its viewport-change event. The HostFrame never derives slots
`56/57` from the fitted CSS rectangle, logical size, or backing allocation.
`data-logical-width` and `data-logical-height` on the canvas are the shell's
layout metadata; the shell never derives its aspect ratio from the physical
`canvas.width` or `canvas.height`. The host allocates the physical backing as
the CSS extent times the effective `devicePixelRatio`, rounded to whole pixels
and reduced as one scale when the 8192-axis or 64 MiB backing cap would be
exceeded. `data-*` receipts expose logical, CSS, backing, DPR, scale, cap, and
generation values for inspection.

Canvas2D receives one logical-unit transform per frame. Private WebGL2 targets
are allocated in backing pixels and receive logical dimensions in their shader
uniform; their completed image is composited into the main backing under an
identity transform. This keeps rectangles, lines, text, sprites, clips, and
intermediate targets from applying density twice. Pointer coordinates use the
CSS bounding rectangle and map back to the logical canvas, including after a
resize, orientation change, or DPR change.

Web sprite preparation uses bounded density tiers (`1`, `1.25`, `1.5`, `2`,
`3`, `4`, `6`, and `8`). A cache key includes canonical source identity,
logical target extent, the selected physical output dimensions and tier, and
fixed raster options; raw DPR and intermediate scale values are not key
components. PNG sources are never enlarged into a derived tier; an
under-provisioned source is retained with an explicit `data-asset-fallback`
receipt. The runtime publishes source, prepared, decoded, atlas, and cache
pixel/byte receipts (`data-asset-source-*`, `data-asset-prepared-*`,
`data-asset-decoded-*`, `data-asset-atlas-*`, and `data-asset-cache-*`) where
the browser can know them. Density generation invalidates each live atlas
entry once, and stale async preparations are rejected by resource generation
and tier key before becoming drawable.

## Resource cache policy

SVG entries are keyed by canonical source identity, logical target extent, the
current raster scale, and the fixed raster options used by the shared runtime.
Font and cached-text entries use logical font size plus the same raster scale.
Raster dimensions use checked, bounded `ceil(logical_extent * raster_scale)`;
the logical draw size remains unchanged.

Desktop resource extent calculation uses the exact integer logical/full-backing
ratio rather than applying `ceil` to a rounded floating-point scale. For
example, an 18-pixel font at a `1920 / 720` backing ratio prepares exactly 48
pixels, not 49 due to binary float drift. With `STASIS_GFX_LOG_SPRITES=1`, each
successful initial or replacement preparation emits a current-resource receipt
containing its handle, source bytes, logical and raster extents, and density
generation; font receipts also include the live atlas extent.
Window presentation receipts name both the display generation and density
generation so acceptance evidence can reject resource receipts from an older
backing.

Desktop and packaged mobile builds perform this policy in
`runtime/stasis_graphics.c`. Workshop and generated release apps receive
the same logical/native/drawable metadata in reserved current gfx_cmd header slots,
use the same aspect-fit viewport, and replace SVG/font/text textures when the
density generation changes. Those metadata slots are host-populated and are
not part of the command trace, so JIT/AOT command parity is unchanged.

Desktop presentation is explicit and independent from the logical canvas:

- `init_window(width, height, title)` and `set_window_size(width, height)` select
  a restored, resizable window whose client size uses the requested logical
  dimensions.
- `set_maximized(1)` fills the window-manager usable work area while preserving
  taskbars, docks, panels, and normal window chrome. It does not change the
  logical size last requested by `init_window` or `set_window_size`.
- `set_maximized_canvas(width, height)` changes the logical canvas and keeps the
  native presentation maximized. SDL aspect-fits that canvas with letterbox or
  pillarbox buffers when its aspect ratio differs from the native surface.
- `set_maximized(0)` restores a windowed presentation using the retained logical
  dimensions.
- `set_fullscreen(1)` remains borderless desktop fullscreen and is not an alias
  for maximized presentation. `set_fullscreen(0)` returns to the retained
  windowed dimensions.
- Android and iOS continue to own a fullscreen native surface; maximized
  requests preserve logical dimensions but do not alter that platform policy.

Requests published during `main()` are applied immediately after startup, and
requests published during `tick()` are applied at the next between-tick host
boundary in both JIT and packaged AOT runners. A common portrait setup is:

```stasis
init_window(360, 720, "Portrait Game");
set_maximized(1);
```

A responsive game can switch between purpose-built logical canvases without
restoring or resizing the desktop window:

```stasis
set_maximized_canvas(720, 360);
```

On Android and iOS the native surface remains platform-owned fullscreen while
the requested logical canvas changes through the same between-tick mailbox.

The title argument remains a compatibility parameter for guest source. The
native title is owned by the project/CLI launch configuration because the
bounded window mailbox intentionally carries only presentation mode and logical
dimensions; changing presentation does not replace that configured title.

On Windows, `stasis_runner.exe` declares per-monitor-v2 DPI awareness and the
graphics runtime enables SDL's DPI-scaled point coordinate mode before video
initialization. A requested `800 x 600` logical window on a 150% display is
therefore an `800 x 600` SDL window with a `1200 x 900` drawable. Windows does
not bitmap-stretch a lower-resolution frame, and the resulting `1.5` raster
scale rebuilds SVG and font resources at the drawable density.

Regular SDL framebuffer captures remain fitted-content captures. Their extent
is derived from the full backing and logical canvas, while framebuffer memory
and density preparation continue to use the complete backing. Recording targets
retain their explicitly requested physical extent.

On Linux, X11 tests may set `SDL_VIDEO_X11_SCALING_FACTOR` before SDL video
initialization to exercise deterministic 1x, fractional, and 2x backing tiers.
This is an SDL/X11 acceptance control, not a game-owned scale. Wayland remains
compositor-owned. Both backends feed the same full-backing metrics, fitted
content, pointer transform, density generation, and bounded preparation path.
X11 itself uses pixel window coordinates, so Stasis queries SDL's window/display
content scale and applies it when creating or resizing a windowed presentation.
`SDL_EVENT_WINDOW_DISPLAY_SCALE_CHANGED` reapplies that physical extent without
changing the logical canvas. Maximized/fullscreen backing remains the actual
bounded screen surface rather than multiplying beyond the display.

On macOS, the release toolchain ships `stasis_runner.app`, and generated
desktop packages preserve the same app-bundle contract with a game-specific
`Info.plist`. Both enable `NSHighResolutionCapable`. Together with SDL's
`SDL_WINDOW_ALLOW_HIGHDPI` window flag, an `800 x 600` logical window on a
2x Retina display receives a `1600 x 1200` drawable instead of a
resolution-doubled low-density surface. The runtime applies the same drawable
scale and density-sensitive resource rebuild policy used on Windows.
