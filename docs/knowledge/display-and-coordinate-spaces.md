# Display and coordinate spaces

Stasis presents one logical canvas to a game. Treat its coordinates as the
common language for layout, drawing, pointer hit tests, and UI collision.
The host owns the conversion to window and render-target pixels. The
[compiled example](examples/src/display_spaces.stasis) resolves a button once
and uses the same f32 rectangle in its public presentation command and pointer hit test;
[tests](examples/tests/display_spaces.test.stasis) exercise density changes
and a fractional safe-area fit.

The example imports `/vendor/stasis/stdlib/graphics.stasis` for drawing and
host types, and imports `/vendor/stasis/stdlib/ui_pointer.stasis` directly for
pointer helpers. Keep this helper module an explicit import wherever its API is
used.

## Space and transform owners

| Space | Units and purpose | Transform owner |
| --- | --- | --- |
| Native window/surface | Platform input and window units; not necessarily physical pixels | OS and host |
| Drawable | Physical render-target pixels, possibly denser than the native surface | Host renderer |
| Logical canvas | Stable top-left game units selected by `init_window` or a later canvas request | Game chooses extent; host fits and presents it |
| Safe logical viewport | Platform-usable portion of the logical canvas after insets and fitting | Host maps platform safe area into logical coordinates |
| Authored reference (optional) | Design-page units such as 360 x 720 | Game may contain or crop it inside the safe logical viewport |
| World/camera | Simulation positions, cells, and camera projection | Game |

The host fits the logical canvas into the native surface, accounting for
letterboxing or pillarboxing, and maps that presentation to the drawable
backing. It maps native pointer events through the inverse fit and publishes
`x_logical` and `y_logical`. The game does not repeat either conversion.
Within the logical canvas, a game may map an authored page into the safe
viewport. A camera may then project world coordinates into logical drawing
coordinates. Those are separate, game-owned transforms:

```text
world/cells --game camera projection--> logical canvas
authored page --optional game contain--> safe logical viewport in logical canvas
logical canvas --host fit/presentation--> native surface --host backing--> drawable
native pointer --host inverse fit--> logical pointer --game hit test--> resolved Rect
```

`init_window(360, 720, title)` requests a 360 x 720 logical canvas. On a
desktop it also initially requests a 360 x 720 window client size in native
window units. A high-density drawable can have more physical pixels, and a
maximized or fullscreen window can have a different native extent. Neither
changes the logical canvas. Desktop maximization, mobile fullscreen, resize,
and orientation are host presentation decisions. A game that wants a responsive
canvas can call `set_canvas_size(width, height)` after observing a new surface
extent. This changes only the logical canvas; the host keeps the user's native
window size and window mode, and reports the accepted logical and safe viewport
on a following frame. The repository's
`docs/display_metrics.md` describes host implementation details; that source
file is outside the offline knowledge package.

## Read HostFrame fields by ownership

Refresh a `HostFrame` once before reading it for a tick. These fields are
host observations, not instructions to multiply game coordinates.

| `HostDisplayFrame` field | Consumer and ownership |
| --- | --- |
| `logical_width`, `logical_height` | Game layout, clipping, and logical bounds use these logical extents. The game requests the canvas; host reports the current accepted value. |
| `safe_x`, `safe_y`, `safe_width`, `safe_height` | Host supplies a safe rectangle in logical units. UI layout may place controls here or contain an authored page inside it. |
| `available_width`, `available_height` | Host reports safe-area-adjusted presentation space in platform units (CSS pixels on Web, native usable units on desktop, surface units on Android). Use when choosing a presentation mode or canvas request; do not mix directly with logical hit geometry. |
| `native_width_px`, `native_height_px` | Host diagnostics for native window/surface extent; despite the ABI suffix, units follow the platform surface. Never substitute for logical layout size. |
| `drawable_width_px`, `drawable_height_px` | Renderer backing size in physical pixels. Host/resource preparation owns it. |
| `screen_width_px`, `screen_height_px` | Compatibility-named host slots currently carry available presentation extents, not a guaranteed physical monitor resolution. Prefer `available_width` and `available_height` when choosing a canvas and do not use these slots for geometry. |
| `content_scale` | Host presentation ratio of fitted drawable content pixels per logical unit. Renderer/resource backing only. |
| `raster_scale` | Host bounded SVG/font resource density. Asset preparation and cache identity only. |
| `generation`, `resized` | Host change notifications. Recompute responsive layout when logical or safe viewport inputs change. A notification does not itself supply a geometry multiplier. |
| `density_generation` | Host resource invalidation generation when effective raster density changes. Rebuild density-sensitive resources, not layout or collisions. |

| `HostPointerFrame` field | Consumer and ownership |
| --- | --- |
| `id` | Host-stable identity for distinguishing active pointers during a contact. The game may retain it for capture. |
| `is_down`, `went_down`, `went_up` | Host-normalized held/edge state. Game input rules choose how to respond. |
| `x_logical`, `y_logical` | Host-transformed point in the logical canvas. Compare directly against logical rectangles. |
| `dx_logical`, `dy_logical` | Host-transformed motion in logical units; apply to logical gestures without density multiplication. |
| `x_normalized`, `y_normalized` | Host-normalized position relative to the safe logical viewport; useful for proportional controls. Convert to a chosen logical rectangle only if that mapping is deliberate. |

`pointer_count` bounds valid entries in `pointers[8]`; the host reports
overflow separately in `dropped_pointer_count`, and the count may include
empty slots below the highest active slot. The example chooses its primary
pointer with `primary_pointer_index()`: the first newly pressed slot wins,
then the first held or just released slot, then slot 0 when the host reported
any slots; an empty frame returns -1. Check for -1 before indexing.

## Pointer slots and touch gestures

On native SDL hosts, slot 0 carries mouse input and touch events use slots 1
through 7. SDL's defaults can mirror mobile touch as mouse input, and can turn
mouse input into touch events on Android and iOS. A nonzero slot therefore
means an SDL touch event, not proof of a physical finger. In pinned SDL 3.4.10,
`SDL_HINT_TRACKPAD_IS_TOUCH_ONLY` defaults to mouse events on macOS; setting it
to `1` makes trackpad touches use touch events. See SDL's documentation for
[`SDL_HINT_TOUCH_MOUSE_EVENTS`](https://wiki.libsdl.org/SDL3/SDL_HINT_TOUCH_MOUSE_EVENTS),
[`SDL_HINT_MOUSE_TOUCH_EVENTS`](https://wiki.libsdl.org/SDL3/SDL_HINT_MOUSE_TOUCH_EVENTS),
[`SDL_HINT_TRACKPAD_IS_TOUCH_ONLY`](https://wiki.libsdl.org/SDL3/SDL_HINT_TRACKPAD_IS_TOUCH_ONLY),
and [iOS input](https://wiki.libsdl.org/SDL3/README-ios). On iOS, indirect
pointing input can be delivered as touch depending on the app's
`UIApplicationSupportsIndirectInputEvents` property. If the key is absent,
Apple documents that iOS 17 and later default to supporting indirect input;
earlier iOS versions default to the compatibility behavior that can synthesize
touches. See Apple's [property documentation](https://developer.apple.com/documentation/BundleResources/Information-Property-List/UIApplicationSupportsIndirectInputEvents).
The current Web host publishes only slot 0, which can represent mouse or touch
input; it does not publish multi-touch slots.

Use `touch_pair(result)` to fill caller-owned storage with the first two down
slots in the reported range from 1 through 7. It skips holes, copies each
slot's current `id`, and marks the pair invalid if fewer than two are down.
The helper clears all result fields when there is no pair. Store and compare
both IDs across frames before applying a gesture delta, since slot membership
can change as contacts end. Reset gesture baselines whenever the pair or pinch
sample is invalid, as well as when either ID changes.

Use `pinch(minimum_native_surface_px, pair_result, sample_result)` to measure
that pair. It clears both outputs, then refreshes `pair_result` through
`touch_pair` and measures that same pair, so slot selection has one policy. A
spread equal to the minimum is accepted; coincident contacts are invalid. A
valid `sample_result` contains the logical midpoint and spread plus spread in
native surface units; pair indices and IDs remain in `pair_result`. An invalid
pair clears both outputs. Invalid metrics or a negative minimum clear the
geometry sample while `pair_result` still describes the current pair, including
when its spread is below threshold.
The generated example and regression tests include a phone-mirror frame with
the same contact in slots 0 and 1: one touch-event slot must not form a pair.

`native_width_px` is a legacy field name: it reports native surface units,
which are CSS pixels on Web and are not guaranteed to be physical panel
pixels. `drawable_width_px` is the renderer backing width in physical pixels.
For a distance measured in native surface units, convert it to logical units
with `drawable_width_px / (content_scale * native_width_px)`; the standard
library's `native_surface_px_to_logical()` applies this ratio and returns
zero for invalid metrics. Use `drawable_px_to_logical()` for a distance
already measured in drawable pixels. Keep the two units explicit in names.
For handset controls, choose a touch target or gesture slop in native surface
units and convert it once; 48 surface pixels is a useful starting value, not
a promise that the platform reports physical panel pixels.

## Resolve geometry once

For ordinary UI, place logical rectangles directly within the safe logical
viewport. A contain fit is useful when an authored layout must keep its aspect
ratio. For reference extent `(Rw, Rh)` and safe logical rectangle `S`:

```text
reference_fit = min(S.w / Rw, S.h / Rh)
page_origin = (S.x + (S.w - Rw * reference_fit) / 2,
               S.y + (S.h - Rh * reference_fit) / 2)
logical_rect = page_origin + reference_rect * reference_fit
```

Compute that mapping once and keep the resulting exact f32 destination
`Rect(x, y, w, h)`. Submit its fields to drawing and use those same fields
for pointer hit testing. Fractional safe fits can place edges between integer
coordinates; rounding only one side changes which pixels are clickable.
Keep inclusive/exclusive edge policy explicit, as in the example's half-open
hit rectangle. Do not derive hit geometry from a rasterized asset's width.

A camera is another game-owned mapping: `world_to_logical` may use
`camera_zoom`, `cell_size_logical`, and a camera origin. Use those names,
`reference_fit`, and `resource_raster_scale` rather than a generic
`scale`. World collisions remain in world/cell units; project only for draw
and pointer selection. An inverse camera projection can map a logical pointer
to world coordinates. Display density never belongs in that mapping.

Multiplying a logical button rectangle or `x_logical` by
`content_scale`, `raster_scale`, device pixel ratio, or drawable/native
ratio applies the host's display fit twice. It breaks layout and hit tests
when only backing density changes. The deterministic example tests this by
changing drawable dimensions and density while holding the safe logical
viewport fixed; its resolved rectangle and pointer hit stay identical.
