# Windows high-DPI rendering research

Date: 2026-08-04

## Finding

The Windows desktop package is not explicitly DPI aware. The current release
runner manifest contains only `requestedExecutionLevel`; it has neither
`dpiAware` nor `dpiAwareness`. `runtime/stasis_graphics.c` also initializes SDL
video without first setting either `SDL_HINT_WINDOWS_DPI_AWARENESS` or
`SDL_HINT_WINDOWS_DPI_SCALING`.

This can leave the process DPI unaware. On a display configured above 100%,
Windows may give the application a virtualized 96-DPI render target and then
bitmap-stretch that result to the monitor. Stasis consequently observes a
drawable no larger than the logical/window size, keeps `g_pixel_scale` at 1,
rasterizes SVG and text at 1x, and cannot recover the physical pixels that the
Desktop Window Manager adds later. The result is the expected physical window
size but a softer image.

ChessTD/Gambit Guard reproduces the vulnerable configuration. It requests a
`360 x 720` logical window and runs the pinned 2026-07-30 nightly runner. The
embedded manifest extracted from that exact `stasis_runner.exe` has no DPI
declaration. Most of the game's UI is SVG and its text uses Nunito, while its
painted PNG masters are substantially larger than their logical draw sizes, so
the source art is not the limiting factor. The old Windows host is preventing
the renderer from requesting the higher-density drawable those assets can use.

`SDL_WINDOW_ALLOW_HIGHDPI` alone does not establish Windows process DPI
awareness or opt SDL into DPI-scaled coordinates. SDL documents its Windows
DPI scaling hint as the switch that makes requested window sizes use scaled
points, requests per-monitor awareness, and maintains window size across
monitors with different scale factors.

## Existing code that is already correct

Once SDL exposes the real drawable, the Stasis path is coherent:

- `SDL_GetWindowSize` supplies the SDL/window coordinate extent.
- `SDL_GetRendererOutputSize` (or `SDL_GL_GetDrawableSize` on the legacy path)
  supplies render-target pixels.
- `stasis_display_metrics` derives `content_scale` from drawable pixels per
  logical pixel.
- SVG and font raster extents are multiplied by that scale.
- A density change marks cached sprites and fonts for re-rasterization.
- Display metrics are synchronized on every event pump, not only on a window
  size event, so moving between monitors can be detected.

SDL 2.32.10 also converts Windows client coordinates and mouse coordinates to
the same DPI-scaled point space when `SDL_HINT_WINDOWS_DPI_SCALING` is enabled,
and emits a size-changed event when the framebuffer changes at a DPI boundary.
That matches the existing Stasis native-to-logical input transform.

## Recommended implementation

1. Before `SDL_Init(SDL_INIT_VIDEO | SDL_INIT_EVENTS)` on Windows, set
   `SDL_HINT_WINDOWS_DPI_SCALING` to `"1"`. Do not set only
   `SDL_HINT_WINDOWS_DPI_AWARENESS=permonitorv2`: awareness alone gives crisp
   pixels but makes an `800 x 600` request an 800-physical-pixel client area,
   which appears smaller at 150% or 200%. DPI scaling gives the intended
   combination: 800 x 600 SDL points, a 1200 x 900 drawable at 150%, and a
   Stasis raster scale of 1.5.
2. Embed a Windows application manifest in `stasis_runner.exe` with
   `dpiAwareness` preferring `PerMonitorV2` and a legacy `dpiAware` fallback.
   Microsoft recommends the manifest as the process default. The SDL hint is
   still needed because it also selects SDL's scaled-point coordinate policy.
3. Keep `SDL_WINDOW_ALLOW_HIGHDPI`, `SDL_GetRendererOutputSize`, logical
   presentation, and the existing density cache policy. Do not multiply game
   coordinates by Windows DPI in Stasis code; SDL and the display-metrics layer
   already own those transforms.
4. Clarify the `stasis_set_window_size` comment: the parameters are logical
   canvas/window points, not necessarily physical pixels.

The manifest belongs on the executable, not the graphics DLL. The canonical
Windows process is `stasis_runner.exe`; a DLL manifest does not set the host
process default.

## Implemented in this worktree

- The graphics runtime enables `SDL_HINT_WINDOWS_DPI_SCALING` before SDL video
  initialization.
- The Windows runner embeds a manifest that prefers per-monitor-v2 awareness
  and retains the legacy per-monitor fallback.
- Release/bootstrap source bundles include the manifest.
- Windows PR CI extracts the built executable manifest and checks both DPI
  declarations.
- A Rust source-contract test enforces hint ordering and manifest wiring.
- The canonical display metrics document now records the Windows behavior.
- The macOS runner is now a minimal `.app` bundle with
  `NSHighResolutionCapable=true`, and release/bootstrap archives include that
  bundle. The toolchain resolves its inner executable transparently.
- Generated macOS desktop packages stage the runner, game library, graphics
  runtime, launch metadata, and assets inside a game-specific `.app` bundle,
  retaining the Retina opt-in through the final distribution boundary.
- macOS CI builds the bundle and reads the generated plist with `plutil` so a
  packaging regression cannot silently disable Retina drawables.

## Other platforms

macOS needed a packaging change as well. SDL's high-DPI window flag only
requests a high-density drawable when the application bundle declares
`NSHighResolutionCapable`; the previous release archive had no runner app
bundle or plist at all. The new bundle supplies that missing host-level opt-in.

iOS, Android, and Wayland do not need equivalent changes. The iOS host already
uses SDL's high-DPI window flag and derives its drawable from the native screen
scale. Android owns a native pixel surface and reports density through its
existing display-metrics path. SDL's Wayland backend negotiates the compositor
buffer scale for high-DPI windows. X11 remains dependent on the desktop/SDL
environment's scaling support, but it does not have an executable manifest or
bundle key analogous to Windows and macOS.

## Verification plan

Automated checks:

- Assert the Windows DPI scaling hint occurs before SDL video initialization.
- Extract the built runner manifest with `mt.exe` in Windows CI and require the
  expected `dpiAwareness` and legacy fallback entries.
- Preserve the current display-scale unit tests for 1.0, 1.25, 1.5, and 2.0
  drawable/logical ratios.
- Extend the Windows launch probe to record/assert logical, native, drawable,
  content-scale, raster-scale, display-generation, and density-generation
  values. On a 100% CI desktop this remains a useful 1:1 regression check.

Hardware or VM acceptance:

- At 150%, request 800 x 600 and verify logical/native are 800 x 600,
  drawable is 1200 x 900, and raster scale is 1.5.
- Capture the framebuffer and verify the image itself is 1200 x 900, rather
  than an 800 x 600 image enlarged by Windows.
- Move the running window between 100% and 150%/200% monitors. Verify physical
  window size remains approximately constant, drawable/raster scale changes,
  assets re-rasterize once, input still round-trips, and the frame remains
  sharp.
- Repeat windowed, maximized, and fullscreen-desktop transitions.

## Evidence and references

- The 2026-07-30 Windows nightly runner pinned by ChessTD was extracted locally
  with the Windows SDK manifest tool. It contained only
  `requestedExecutionLevel` and no DPI declaration.
- SDL 2.32.10 source installed by the repository's vcpkg setup leaves DPI
  awareness unchanged when no hint is present. Its DPI scaling hint requests
  per-monitor-v2 awareness, uses scaled points, transforms mouse/client
  coordinates, and handles `WM_DPICHANGED`.
- SDL documentation:
  https://wiki.libsdl.org/SDL2/SDL_HINT_WINDOWS_DPI_AWARENESS
- SDL scaled-point policy:
  https://wiki.libsdl.org/SDL2/SDL_HINT_WINDOWS_DPI_SCALING
- SDL window versus drawable sizing:
  https://wiki.libsdl.org/SDL2/SDL_GetWindowSize
- SDL high-DPI window flag and macOS bundle requirement:
  https://wiki.libsdl.org/SDL2/SDL_WindowFlags
- Apple `NSHighResolutionCapable` bundle key:
  https://developer.apple.com/documentation/bundleresources/information-property-list/nshighresolutioncapable
- Microsoft process-default DPI guidance:
  https://learn.microsoft.com/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process
- Microsoft per-monitor-v2 behavior:
  https://learn.microsoft.com/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows

## Theory gained

Windows can add physical presentation pixels after a DPI-unaware application
has rendered, so drawable-versus-logical scaling is only trustworthy after the
host process opts out of DPI virtualization. Once SDL owns a per-monitor,
scaled-point window, Stasis's existing drawable metrics predict the correct
asset density. An adjacent prediction is that fixing process/SDL DPI policy
will improve SVG and text sharpness without changing game layout or input code.

## Reflection

- Good: tracing one frame from process manifest through SDL window coordinates,
  drawable size, density cache, and presentation isolated the missing boundary.
- Bad: the existing high-DPI flag and display contract can look complete while
  Windows still performs an unobservable final bitmap stretch.
- Adjustment: Windows display work should verify the executable awareness
  context and captured framebuffer dimensions, not only renderer flags and
  logical metrics.
