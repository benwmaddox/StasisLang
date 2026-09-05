# Building the pinned SDL3 runtime

Stasis ships one native SDL3 family: SDL3 3.4.10 and SDL3_image 3.4.4. The
runtime requires SDL video, events, timers, the 2D renderer, audio streams, and
filesystem helpers. Those subsystems must not be disabled in a size-optimized
build because they are part of the cross-platform runtime contract.

`runtime/CMakeLists.txt` disables SDL test programs and optional SDL3_image
codecs that Stasis does not consume, then statically links the pinned family
into `stasis_graphics`. Platform adapters may still omit unrelated joystick,
haptic, sensor, or GPU APIs when the upstream CMake configuration supports it,
but only after the Windows, Linux, macOS, Android, and iOS acceptance matrix
continues to pass.

Configure the supported build with:

```text
cmake -S runtime -B runtime/build \
  -DSTASIS_GRAPHICS_BUNDLE_SDL=ON

cmake --build runtime/build --config Release
```

Replacing SDL with a platform-only windowing layer or `sdl2-compat` is outside
the supported architecture. It would break the single renderer, input, audio,
surface-lifecycle, and release-provenance contract.
