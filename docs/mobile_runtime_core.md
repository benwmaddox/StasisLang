# Shared Mobile Runtime Core

Android and iOS release apps link the same `stasis_mobile_runtime` static C
library. The target compiles the existing Stasis graphics, input, audio, and
asset host APIs with the canonical SDL renderer; it never links the desktop SDL
entry shim, DLL runner, dynamic loader, JIT, watcher, or hot-swap code.

## Shell integration

A thin platform shell supplies the compiler-generated `main`, `tick`, and
`render` symbols through `StasisMobileGameEntries`, then calls:

```c
#include "stasis_mobile_runtime.h"
#include "published_aot_symbols.h"

StasisMobileGameEntries game = {
    STASIS_AOT_BIND_RUNTIME_GLOBALS,
    STASIS_AOT_MAIN,
    STASIS_AOT_TICK,
    STASIS_AOT_RENDER
};
StasisMobileRuntimeConfig config = {1280, 720, "My Stasis Game"};

int status = stasis_mobile_runtime_initialize(&config, &game);
StasisMobileFramePacer frame_pacer;
stasis_mobile_frame_pacer_reset(&frame_pacer, SDL_GetTicksNS());
while (status == STASIS_MOBILE_RUNTIME_OK) {
    status = stasis_mobile_runtime_step();
    if (status == STASIS_MOBILE_RUNTIME_OK) {
        uint64_t wait_ns = stasis_mobile_frame_pacer_wait_ns(
            &frame_pacer, SDL_GetTicksNS());
        if (wait_ns > 0) {
            SDL_DelayPrecise(wait_ns);
        }
    }
}
stasis_mobile_runtime_shutdown();
```

The shared shell targets 60 deterministic steps per second independently of
the display refresh rate. Vsync time counts toward the frame interval, so a
60 Hz display does not receive an unconditional extra delay while 90 Hz and
120 Hz displays cannot accelerate game time. If suspension or overload misses
a complete interval, the completed step owns the current interval: the pacer
returns a full interval of sleep before the following tick and stores the
deadline two intervals ahead. Subsequent steps resume normal absolute-deadline
pacing without a burst of catch-up ticks.

The generated AOT symbol header declares `main`, `tick`, and `render` as
`int32_t(void)` and is the source of the actual symbol names. The runtime turns
any non-zero entry result into a stop request and retains the exact result for
the platform shell to log or return. The generated `published_aot_bindings.c`
registers linked function pointers and string literals with the shared AOT
state/dispatch layer; shells compile it as an ordinary source file and do not
discover symbols dynamically.

Platform pause and resume callbacks call `stasis_mobile_runtime_set_paused`.
The platform adapter also maps the packaged `stasis_game` asset root to a path
or SDL-backed resource view that the unchanged Stasis asset APIs can consume.

## CMake

Mobile toolchains enable the target by default. A shell embedding the runtime
from a parent CMake project can also request it explicitly:

```cmake
set(STASIS_BUILD_MOBILE_RUNTIME ON CACHE BOOL "" FORCE)
set(STASIS_GRAPHICS_BUILD_SHARED OFF CACHE BOOL "" FORCE)
set(STASIS_GRAPHICS_BUILD_STATIC OFF CACHE BOOL "" FORCE)
set(STASIS_BUILD_RUNNER OFF CACHE BOOL "" FORCE)
set(STASIS_BUILD_SYS OFF CACHE BOOL "" FORCE)
add_subdirectory(path/to/stasis/runtime stasis-runtime)
target_link_libraries(my_mobile_shell PRIVATE stasis_mobile_runtime)
```

The parent project may provide `SDL3::SDL3` and `SDL3_image::SDL3_image`
targets directly. Otherwise the runtime resolves their CMake packages.

Android's generated `published_aot_objects.cmake` includes the bindings source.
An iOS target adds the package manifest's `bindings_source` alongside its AOT
objects before linking `stasis_mobile_runtime`.
