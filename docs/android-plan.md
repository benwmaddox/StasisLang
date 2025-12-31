# Android Support Plan

This plan focuses on getting Stasis running on Android in small, verifiable steps.

## Goals (initial)

- Produce an Android-compatible build of the native runtime (`libstasis_graphics.so`) for `arm64-v8a`.
- Keep the Stasis-facing API stable while swapping platform implementations underneath.
- Start with a "works on device" loop: open a window, draw, handle input, play audio.

## Constraints

- Android does not support the current OpenGL 2.1 + GLEW code path.
- The runtime should avoid implicit allocation and keep frame-to-frame behavior explicit.
- The host layer will eventually own platform services (assets, input, audio, lifecycle).

## Phase 1: Build the runtime for Android (SDL_Renderer-only)

1. Add an SDL_Renderer-only build mode in `runtime/` (no OpenGL/GLEW).
2. Cross-compile with the Android NDK using vcpkg triplets:
   - Triplet: `arm64-android` (maps to `arm64-v8a`)
   - Dependency: `sdl2`
3. Output: a shared library suitable for loading from Java/Kotlin via `System.loadLibrary`.

Build helper:
- `runtime/build_android.ps1` (uses `VCPKG_ROOT` or `C:\vcpkg` + `ANDROID_NDK_HOME`)

Related (Brickout Revenge debug workflow):
- See `docs/brickout-android-debug-plan.md` and `android/build_brickout_android_debug.ps1` / `android/install_brickout_android_debug.ps1` for a "build APK + install + push assets" path.

## Phase 2: Minimal Android host app (JNI bridge)

Create a small Android app that:

- Loads native libs: `libSDL2.so` + `libstasis_graphics.so`.
- Calls a tiny C entry point to initialize the runtime and run a loop.
- Surfaces lifecycle events (pause/resume) into the runtime as explicit calls.

Notes:
- Prefer SDL's Android integration for window + event pump.
- Add a minimal JNI bridge only for platform features SDL does not cover (permissions, file picker, etc.).

## Phase 3: Input + Audio alignment

- Input: map Android touch to the same pointer snapshot model as desktop.
- Audio: use SDL audio backend; keep ring-buffer API identical across platforms.

## Milestones / Acceptance

- Runtime builds for `arm64-android` without OpenGL/GLEW dependencies.
- A minimal Android app launches on device/emulator and renders a frame.
- Pointer/tap events reach Stasis code with correct coordinates.
- Audio playback works with underrun diagnostics visible.
