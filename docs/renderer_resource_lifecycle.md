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

Surface resize and orientation advance `surface_generation` without invalidating
device-local resources. Renderer/context creation, `SDL_RENDER_TARGETS_RESET`, and
`SDL_RENDER_DEVICE_RESET` advance both `surface_generation` and
`renderer_generation`. Generations skip zero. A sprite, fallback texture, font
atlas, or text texture can be submitted only when its renderer generation matches.
Android pause/resume is a visibility transition: the Workshop asks GLSurfaceView to
preserve its EGL context and retains textures when that context survives. A later
`onSurfaceCreated` callback is the authoritative signal that the context was lost.

The native SDL runtime retains sprite paths, logical raster requests, decoded font
bytes, font metrics, and cached text bytes/quads. Android Workshop and Published
previews retain their project or packaged manifest, asset identities, content
hashes, and font sources. Lost-context handles are discarded without calling a
destructor in the invalid context. Resize-only invalidation deletes still-valid
handles before rebuilding them.

## Restore transaction

Before the first post-context-loss game frame is presented, the native renderer
rebuilds all active sprites, the procedural fallback, every active font atlas, and
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

## Verification

- `ctest --test-dir <runtime-build> -C Release --output-on-failure` exercises the
  bounded lifecycle transition/retry contract and existing render/mobile contracts.
- `gradle testWorkshopDebugUnitTest` covers Android lifecycle generations, schema,
  and provider behavior.
- `mobile/android/test_emulator.ps1` installs Workshop, launches it, rotates it,
  backgrounds/resumes it, forces Activity/process recreation, requires multiple
  successful restoration markers with no restore failure, and force-stops the app.
