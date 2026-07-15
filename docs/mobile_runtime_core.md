# Shared Mobile Runtime Core

Android and iOS release shells link `stasis_mobile_runtime` together with the
compiler-produced AOT game objects. The core is a fixed-entry lifecycle layer;
it never loads game code dynamically and does not include the desktop runner.

Platform shells provide `main`, `tick`, and `render` entry pointers plus the
existing HostFrame, window-request, and render-command buffers in a
`StasisMobileRuntimeConfig`. The core then:

1. starts the shared SDL-only graphics runtime
2. calls game `main` once
3. snapshots input and applies window requests
4. calls `tick` and `render` in order
5. submits the shared render-command buffers

Pause and focus state are explicit and do not replace or reload game code.
`on_code_swap` remains ABI-compatible metadata but is never called on mobile.

Mobile CMake builds default to `STASIS_GRAPHICS_SDL_ONLY=ON`, build the static
`stasis_mobile_runtime` target, and leave the desktop dynamic runner and sys
runtime disabled. Both platform shells consume the same public header and
static library; platform SDK code remains a thin adapter around this API.

Host-side link and lifecycle coverage lives under `runtime/tests`:

```powershell
cmake -S runtime/tests -B target/mobile-runtime-tests
cmake --build target/mobile-runtime-tests --config Release
ctest --test-dir target/mobile-runtime-tests -C Release --output-on-failure
```
