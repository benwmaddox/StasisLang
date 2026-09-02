# Stasis Graphics Runtime

Native SDL3 graphics library for Stasis programs. Shipping desktop, Android,
and iOS builds compile the same `stasis_graphics.c` command interpreter and SDL
resource lifecycle. See `docs/shared_renderer_process.md` for the contract.

## Framebuffer capture

`stasis_gfx_dump_png(path)` writes the current framebuffer as an RGBA PNG and
returns `1` on success or `0` on failure. `stasis_gfx_dump_bmp(path)` provides
the existing BMP equivalent. Relative runtime paths resolve through the asset
root; callers that need a specific output location should pass an absolute path.

For automated captures, set `STASIS_SCREENSHOT_ONCE` to an output path and
optionally set the 1-based `STASIS_SCREENSHOT_FRAME` and
`STASIS_EXIT_AFTER_SCREENSHOT=1`. Scheduled capture occurs after queued drawing
and post-effects and before the frame is presented. A `.png` suffix selects PNG;
other suffixes use BMP. PNG bytes are deterministic for identical framebuffer
pixels, though pixels can vary across backends, drivers, and platforms. Ordinary
captures use the actual fitted readback surface dimensions returned by SDL;
fixed recording targets continue to require their configured physical extent.

## High-density displays

`init_window(width, height, title)` defines the game's logical coordinate space.
Stasis keeps that space stable when a mobile or high-DPI desktop surface has a
larger drawable framebuffer. SDL maps logical drawing commands to the drawable,
while pointer positions and safe viewports are converted back to logical pixels.

SVG sources remain packaged as SVG files. On the device, sized SVG and raster
assets are baked at the drawable-to-logical pixel scale. Rasterized GPU entries
are shared in memory by source path and logical target size; a density change
replaces their device raster while preserving the game-facing sprite handle.
TrueType atlases use the same scale, but text measurement and glyph placement
remain in logical pixels. A drawable-density change invalidates the affected
sprite and font caches so they are rebuilt before their next draw. Framebuffer
captures use the actual SDL readback resolution, which can be a fitted subset of
the complete drawable backing.

SVG parsing and CPU rasterization use the vendored ThorVG 1.2.0 CPU/SVG build
pinned in `third_party/thorvg/STASIS_PROVENANCE.md`. The bridge initializes four
ThorVG workers once per process and serializes each asset bake while ThorVG uses
those workers internally. SVG content is clipped to its declared viewport;
geometry outside the root viewport no longer bleeds into transparent contain-fit
padding. GPU texture upload and mipmap generation remain separate publication
steps after the CPU bake.

## Prerequisites

- CMake 3.16+
- A C compiler (MSVC, Clang, or GCC)
- Network access for the first configure, or a pre-populated CMake FetchContent cache

## Setup (Windows)

1. Build the library. CMake fetches and verifies SDL3 3.4.10 and SDL3_image
   3.4.4 from their official release archives:
   ```cmd
   cd runtime
   build.bat
   ```

2. Run an Asteroids demo:
   ```cmd
   cd ..
   cargo run -p stasis --release -- play samples\asteroids.stasis
   ```

Press `F3` in a Windows play window to toggle the performance HUD. It follows
the shared ordered performance contract, showing the phases and workload
details that the active backend can measure, plus a five-second rolling worst
frame-work value. Unsupported fields are omitted from the rendered HUD. See
`docs/performance_hud.md` for the contract.

## Android (NDK)

Android uses the same canonical SDL renderer process as desktop and iOS. Use
the NDK toolchain through direct CMake.

Build helper:
- `runtime/build_android.ps1` (requires `ANDROID_NDK_HOME`, CMake, and Ninja)

## Shared mobile core

Android and iOS release shells link the `stasis_mobile_runtime` static target.
It excludes the desktop runner and SDL main shim. See
`docs/mobile_runtime_core.md` for the lifecycle ABI and CMake setup.

Brickout Revenge debug APK workflow:
- See `docs/brickout-android-debug-plan.md` and use `android/build_brickout_android_debug.ps1` + `android/install_brickout_android_debug.ps1`.

## Manual Build

The same pinned dependency path is used on every host:

1. Configure with bundled SDL enabled.
2. Build the requested runtime targets.

   ```cmd
   mkdir build && cd build
   cmake .. -DSTASIS_GRAPHICS_BUNDLE_SDL=ON
   cmake --build . --config Release
   ```

Do not provide SDL2, `sdl2-compat`, or an unversioned system SDL package to a
shipping build. See `docs/sdl3_migration.md` for the compatibility boundary.

## Native quad replay

Sprites are placed in renderer-private, bounded SDL texture pages using the
compiler-provided logical grouping policy. Pages reserve padded opaque-white
and missing-image regions. Ordinary sprites that fit share bounded `512 x 512`
group-0 cold pages, matching the Web atlas's initial page extent; this keeps
direct and pre-policy loads bounded without mixing them into compiler-eligible
groups. Larger standalone images receive a dedicated SDL texture domain with
the same reserved regions. Padding is edge-extruded at load, density-change, or
renderer-generation rebuild time.

The v7 order stream remains declarative. Adjacent sprite runs and solid
rectangles are lowered in exact painter order to fixed reusable
`SDL_RenderGeometry` storage. A solid uses the active page or bounded lookahead
to the next sprite page, so it does not create an avoidable texture transition.
Replay splits only at real page/domain, clip, primitive, state, or fixed-capacity
boundaries. Applications do not need to layer or reorder translucent content.

## API

The library exports these functions for Stasis programs:

| Function | Description |
|----------|-------------|
| `stasis_init_window(w, h, title)` | Create the SDL window and renderer |
| `stasis_begin_frame()` | Start a new frame |
| `stasis_end_frame()` | Render queued lines, swap buffers |
| `stasis_clear(r, g, b, a)` | Clear screen with color |
| `stasis_draw_line(x1, y1, x2, y2, r, g, b, a)` | Queue a line for rendering |
| `stasis_draw_lines_f32(lines, count)` | Batch: queue `count` lines from an `f32` array (8 floats per line) |
| `stasis_gfx_load_sprite(path, max_w, max_h)` | Load and bake an image into the sprite atlas system; returns handle |
| `stasis_gfx_draw_sprite(handle, x, y, sx, sy, rot, r, g, b, a)` | Draw baked sprite (centered) with scale/rotation/tint |
| `stasis_gfx_draw_sprites(cmd_i32, cmd_f32, count)` | Batch: draw sprites from typed state and logical-geometry arrays |
| `stasis_gfx_debug_bake_hash(path)` | Debug: bake SVG on CPU and return a pixel hash |
| `stasis_gfx_debug_enable_hash(enabled)` | Debug: enable per-frame draw-call hash (for verifying batch equivalence) |
| `stasis_gfx_debug_get_frame_hash()` | Debug: get current frame hash (0 if disabled) |
| `stasis_is_key_down(scancode)` | Check if key is pressed |
| `stasis_get_time_ms()` | Get time in milliseconds |
| `stasis_get_time_us()` | Get time in microseconds (truncated to i32) |
| `stasis_sleep_ms(ms)` | Sleep for milliseconds |
| `stasis_should_quit()` | Pump input/events (once per frame) and report quit state |
| `stasis_input_pointer_count()` | Number of pointers tracked this frame (mouse + active touches) |
| `stasis_input_pointer_*` | Pointer snapshot queries (pos, deltas, edge flags) |
| `stasis_audio_is_available()` | Initialize audio if needed; returns 1 on success |
| `stasis_audio_get_sample_rate()` | Current audio sample rate (Hz) |
| `stasis_audio_get_channels()` | Current audio channels (v1: 2) |
| `stasis_audio_get_queued_frames()` | Frames currently queued in the ring buffer |
| `stasis_audio_get_underruns()` | Underrun counter (device starved -> outputs silence) |
| `stasis_audio_push_f32_interleaved(ptr, frames)` | Push `f32` interleaved frames (LRLR...); returns frames accepted |
| `stasis_audio_load_wav(path)` | Decode a bounded mono/stereo PCM16 WAV asset; returns an opaque handle |
| `stasis_audio_play(handle, loop, volume, pan)` | Start an overlapping asset voice; returns an opaque voice handle |
| `stasis_audio_stop(voice)` | Stop one asset voice |
| `stasis_audio_voice_set_paused(voice, paused)` | Pause or resume one voice without changing its cursor |
| `stasis_audio_voice_set_volume_pan(voice, volume, pan)` | Update one active voice (`volume` 0..1, `pan` -1..1) |
| `stasis_audio_load_music/effect(path)` | Category loaders for bounded WAV or MP3 assets |
| `stasis_audio_play_music(handle, loop, volume)` | Start one exclusive music voice for an asset |
| `stasis_audio_pause_music(handle, paused)` | Pause or resume every active voice for a music asset |
| `stasis_audio_set_music_volume(handle, volume)` | Update every active voice for a music asset |
| `stasis_audio_stop_music(handle)` | Stop every active voice for a music asset |
| `stasis_audio_play_effect(handle, volume)` | Start an overlapping centered one-shot |
| `stasis_asset_request_sprite(path, width, height)` | Queue sprite I/O and rasterization; returns a task handle immediately |
| `stasis_asset_request_audio(path)` | Queue bounded WAV/MP3 I/O and decoding; returns a task handle immediately |
| `stasis_asset_task_poll(task)` | Poll `pending`, `loading`, `loaded`, `failed`, or `cancelled`; publishes completed host resources on the caller thread |
| `stasis_asset_task_take_handle(task)` | Transfer a loaded sprite/audio handle to the caller and retire the task |
| `stasis_asset_task_cancel(task)` | Cancel or retire a task and release an untaken resource |

WAV asset decoding accepts little-endian PCM16 at 8–384 kHz, one or two channels. Category loaders
also accept mono or stereo MP3 in that sample-rate range. Each source file is capped at 16 MiB and
each decoded asset at 64 MiB. Compressed bytes remain compressed in game packages and decode into
bounded host memory when loaded. The callback linearly resamples into the active stereo device rate
and clamps the combined raw-stream and asset-voice mix. Asset and voice tables are fixed at 64 and
32 entries so a game cannot create unbounded native audio state. All handles and decoded buffers
remain host-owned; deterministic Stasis snapshots retain only the opaque integers chosen by game
code.

Asynchronous asset tasks use one bounded 64-entry queue and one host worker. File access, image
rasterization, and audio decoding happen off the frame thread. `asset_task_poll` performs only the
required main-thread publication step (texture upload or mixer-table insertion). `ImageAsset` and
`AudioAsset` expose this as `load_*()`, `ready()`, `failed()`, `play()`/`publish()`, and `release()`;
their `AssetState` and opaque handles are driven by the host task. Games should release abandoned
or superseded assets so their bounded task slots can be reused. The web host maps the same states
onto browser image and audio promises.

`LoadingProgress.advance(total_count, loaded_count, failed_count)` derives the real completed and
in-progress counts and percentage for a loading screen. Failed work counts as finished so callers
can decide whether to continue or retry. `displayed_percent` advances by at most one percentage
point per tick, while `complete()` always uses the real counts. An empty batch reports 0% and is
already complete.

`play` and the native runner use HostFrame bulk snapshots for per-tick input/state now.
Application code should read keyboard/pointer/quit state through the public wrappers in
`src/stdlib/graphics.stasis`. The fixed HostFrame layout is
private to `src/stdlib/internal/host_frame_raw.stasis`; integration tests may import it directly,
while ordinary tests should use `src/stdlib/testing/input_testkit.stasis`.

## SDL Scancodes

Common key scancodes for input:
- W = 26, A = 4, S = 22, D = 7
- Space = 44, Escape = 41
- Arrow keys: Up = 82, Down = 81, Left = 80, Right = 79
