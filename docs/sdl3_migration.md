# Native SDL3 migration and rollback

Stasis uses SDL3 3.4.10 and SDL3_image 3.4.4 on Windows, Linux, macOS, Android,
and iOS. The official release archives and SHA-256 values are declared once in
`runtime/CMakeLists.txt`; release provenance must report
`sdl3=3.4.10-static` and `sdl3_image=3.4.4-static`.

## Compatibility boundary

`runtime/stasis_graphics.c` remains the shared command interpreter and owns
logical coordinates, renderer resources, CPU SVG/font/raster preparation,
event normalization, and the bounded audio ring. SDL3 is the native platform
layer beneath that contract:

- window/display adapters use SDL3 display IDs, high-pixel-density windows,
  pixel-size events, and logical presentation;
- drawing uses SDL3 float-coordinate render operations and surface readback;
- input uses SDL3 top-level events and normalized touch identities;
- audio uses one SDL3 playback stream whose callback drains the existing
  bounded Stasis ring and emits silence on underrun;
- SDL3_image loads package raster assets without a separate global init path;
- lifecycle, hot-swap, and Stasis-facing ABI entry points do not change.

There is no SDL2 or `sdl2-compat` fallback. Android packages load `libSDL3.so`
and `libSDL3_image.so`; iOS packages embed and sign `SDL3.framework` and
`SDL3_image.framework`; desktop release runtimes link the pinned static family.

## Validation matrix

| Target | Build/package evidence | Runtime acceptance |
| --- | --- | --- |
| Windows x64 | pinned CMake build, signed executable/DLL checks, provenance audit | clear, line, sprite, cached text, resize/density, audio, hot reload, present capture |
| Linux x64 | pinned CMake build and release archive audit | same render fixture under Xvfb plus audio-disabled startup |
| macOS x64/arm64 | pinned CMake bundle, dylib/rpath and signing checks | Retina resize/density fixture and packaged runner launch |
| Android arm64 | exact release source checkouts, Gradle/NDK package audit | API-35 emulator/device render, touch, audio, background/foreground restoration |
| iOS arm64/simulator | generated Xcode shell with matching XCFrameworks | simulator/device render, touch, audio, background/foreground restoration |

Portable pull-request checks enforce source/API contracts, native runtime
compilation, package contents, and render parity. Release publication must also
capture the platform-specific runtime evidence above; a missing target remains
a release blocker rather than silently selecting SDL2.

## Repinning and rollback

A repin changes both versions, archive hashes, immutable Android checkout
commits, framework inputs, provenance expectations, tests, and this matrix in
one reviewed change. Rollback means reverting the complete Stasis change to the
last native SDL3 family known to pass all targets. It must never reintroduce
SDL2 or `sdl2-compat`; if the selected SDL3 family is broken on one target,
publication stops until a native SDL3 repin or runtime correction restores the
matrix.
