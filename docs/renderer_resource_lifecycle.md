# Renderer resource lifecycle

Stasis keeps game-visible renderer handles stable while device-local resources are
recreated. CPU source data survives the transition; GPU and SDL objects never do.

## State machine

Every renderer moves through the same states:

- `Unavailable`: no renderer exists.
- `Ready`: resource generations match the active surface and frames may present.
- `Paused`: the host is in the background; ticks may poll events but no frame presents.
- `RestorePending`: a surface or renderer generation changed.
- `Restoring`: the renderer is rebuilding every active device-local resource.
- `RestoreFailed`: the frame is withheld and the complete restore is retried on the
  next frame.

Surface resize, orientation, and display-scale changes advance the display metrics
`display_generation` and mark the host frame as resized. A preparation-scale change
also advances `density_generation` and marks sprite and font resources for
re-rasterization; these changes retain logical handles and source data. Renderer or
context creation, `SDL_RENDER_TARGETS_RESET`, and `SDL_RENDER_DEVICE_RESET` advance
the lifecycle `surface_generation` and `renderer_generation`. Generations skip zero.
Sprite atlas entries validate their lifecycle generations before submission. Font
atlases and cached text are rebuilt by the restore transaction, and the font-use
path rerasterizes density-dirty fonts before it draws or measures them.

Live font handles retain their source bytes and logical size across density,
surface, and renderer resets. Native font atlases rasterize at a bounded minimum
2x backing scale (or the higher current density up to 8x), then draw down with
linear filtering while all measurements and quads remain in logical units.
Each successful `load_font` acquires an ownership reference, including duplicate
loads that return a shared handle. Balance each acquisition with one
`release_font`; cached text does not retain the font independently. The final
release destroys the atlas and source bytes, generation-invalidates the handle,
and normally removes only cached text owned by that font. Earlier releases leave
the remaining owners and their cached text usable. If native cache compaction
cannot allocate its bounded scratch buffer, or detects corrupt cache state, it
fails closed by clearing the complete text cache. A replacement should acquire
and prepare its font and text before releasing the previous owned font.
On Web, each font handle owns its `FontFace` and starts in `Pending` or `Loading`
when acquired after `main`. `font_status(handle)` becomes `Loaded` only after the
face resolves and Canvas metrics have been calibrated. A rejected load settles as
`Failed` and clears pending text-run metrics; a release removes the handle and
ignores any later `FontFace` completion. Prepared Canvas and GPU text resources
are created only for loaded fonts and are evicted when calibration changes the
font generation, so a compatibility fallback cannot remain the drawable cache.
The dynload bridge reports `Failed` for a positive handle when it is paired with
an older native library that lacks the optional status export; this is terminal
for readiness consumers while legacy synchronous font loading remains usable.
Android pause/resume is a visibility transition: the Workshop asks GLSurfaceView to
preserve its EGL context and retains textures when that context survives. A later
`onSurfaceCreated` callback is the authoritative signal that the context was lost.

The native SDL runtime retains sprite paths, logical raster requests, decoded font
bytes, font metrics, and cached text bytes/quads. Android Workshop and release
previews retain their project or packaged manifest, asset identities, content
hashes, and font sources. Density changes rebuild device-local raster data under
the same logical handles. A lost context discards only device-local pointers while
retaining those handles and sources; restore recreates the resources in the new
lifecycle generations.

## Restore transaction

Before the first post-context-loss game frame is presented, the native renderer
rebuilds all active sprite atlas pages (including their white-solid and
missing-resource placeholder regions), every active font atlas, and
cached text geometry. A failure keeps the lifecycle retryable and withholds that
game frame. The Android GLES adapter first presents a context-local `STASIS LOADING`
marker drawn only with clears and scissor rectangles, before shaders, fonts,
textures, or game assets. Surface setup redraws rather than erases the marker, and
the marker remains visible for at least 250 ms before restoration starts. It then
restores resources referenced by the production command frame in bounded 8 ms
batches. Every incomplete batch keeps the loading marker presented; no partial game
frame is published. The normal game frame replaces the marker only after every
referenced sprite and text texture is ready and the GL checks succeed. A provider
or GL failure marks the restore failed and retries it on the next requested frame.

This path covers Android context loss and Activity recreation, plus SDL target and
device resets. Resize, orientation, and Android background/resume retain resources
when the graphics context remains valid. The legacy desktop GL
backend remains a conformance-only adapter; its supported resize path resets GL
program state, while shipping desktop and mobile packages use SDL.

## Diagnostics

Native restore messages contain `stage`, resource handle/path, logical and raster
dimensions, backend, surface generation, renderer generation, transition reason,
and failure. `stasis_gfx_get_resource_lifecycle` exposes state, both generations,
attempt/failure counters, and the last reason for build audits. Android preview
errors expose the same fields in the visible resource error and under the
`StasisRenderer` log tag.
Android also emits `resource_restore_timing` with wall time, sprite resolution,
decode, upload, text rasterization, restored counts, and the number of budget
deferrals. This makes asset-heavy games such as Chess TD diagnosable from logcat.
Generated SDL mobile packages present the same asset-free `STASIS LOADING` pixel
marker immediately after renderer creation and again on SDL target/device reset.
The marker remains in the presented framebuffer while the synchronous SDL resource
transaction rebuilds every sprite atlas page, placeholder region, font atlas, and cached text run. A
normal game frame is presented only after that transaction succeeds. Ordinary
Android foreground resume preserves resources unless SDL reports an actual reset.

## Verification

- `ctest --test-dir <runtime-build> -C Release --output-on-failure` exercises the
  bounded lifecycle transition/retry contract and existing render/mobile contracts.
- `gradle testWorkshopDebugUnitTest` covers Android lifecycle generations, schema,
  and provider behavior.
- `mobile/android/test_emulator.ps1` installs Workshop, launches it, rotates it,
  backgrounds/resumes it, forces Activity/process recreation, requires multiple
  successful restoration markers with no restore failure, and force-stops the app.
- The IT-020 generated release-shell fixture performs the same sequence with a
  packaged SVG sprite, the same renderer's missing-resource placeholder region,
  a font atlas, and cached text.
  Its `stasis.seam_test.v1` lifecycle markers include state, surface and
  renderer generations, restore attempts/failures, and the transition reason;
  each stage also retains a named-region capture and process identity.
  Resume is validated within the original process: presented/accepted counters
  must advance while a preserved renderer generation may remain unchanged. A
  forced Activity restart starts a new process epoch, so its positive
  generations are checked independently rather than treated as monotonic with
  the prior process. Same-process surface/device reset reasons must advance
  the renderer generation. The fixture's compositor oracle counts target-color
  pixels in inset resource bounds, including glyph-colored pixels for cached
  text; lane backgrounds and fill-rectangle edges cannot satisfy it. Accepted
  and presented counters only gate render progress; the captured Android
  framebuffer is the actual presentation oracle.
