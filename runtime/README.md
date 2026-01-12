# Stasis Graphics Runtime

Native SDL2+OpenGL graphics library for Stasis programs.

## Prerequisites

- CMake 3.16+
- vcpkg (for SDL2)
- A C compiler (MSVC, Clang, or GCC)

## Setup (Windows)

1. Install vcpkg if not already installed:
   ```cmd
   git clone https://github.com/microsoft/vcpkg.git C:\vcpkg
   C:\vcpkg\bootstrap-vcpkg.bat
   ```

2. Set environment variable:
   ```cmd
   set VCPKG_ROOT=C:\vcpkg
   ```

3. Build the library:
   ```cmd
   cd runtime
   build.bat
   ```

4. Run an Asteroids demo:
   ```cmd
   cd ..
   dotnet run --project Stasis.Cli -- run samples/asteroids.stasis --graphics --graphics-lib runtime\build\Release\stasis_graphics.dll
   ```

## Android (NDK)

Android builds currently use the SDL_Renderer backend only (no OpenGL 2.1/GLEW path):

- Configure `runtime/CMakeLists.txt` with `-DSTASIS_GRAPHICS_SDL_ONLY=ON`
- Use an NDK toolchain (direct CMake toolchain or vcpkg Android triplets)

Build helper:
- `runtime/build_android.ps1` (requires `ANDROID_NDK_HOME` and vcpkg via `VCPKG_ROOT` or `C:\vcpkg`)

Brickout Revenge debug APK workflow:
- See `docs/brickout-android-debug-plan.md` and use `android/build_brickout_android_debug.ps1` + `android/install_brickout_android_debug.ps1`.

## Manual Build (Alternative)

If you prefer to build manually or vcpkg is unavailable:

1. Download SDL2 development libraries from https://github.com/libsdl-org/SDL/releases
2. Extract and note the path to SDL2 include and lib directories
3. Build with CMake:
   ```cmd
   mkdir build && cd build
   cmake .. -DSDL2_DIR=<path-to-sdl2>
   cmake --build . --config Release
   ```

## API

The library exports these functions for Stasis programs:

| Function | Description |
|----------|-------------|
| `stasis_init_window(w, h, title)` | Create window with OpenGL context |
| `stasis_begin_frame()` | Start a new frame |
| `stasis_end_frame()` | Render queued lines, swap buffers |
| `stasis_clear(r, g, b, a)` | Clear screen with color |
| `stasis_draw_line(x1, y1, x2, y2, r, g, b, a)` | Queue a line for rendering |
| `stasis_draw_lines_f32(lines, count)` | Batch: queue `count` lines from an `f32` array (8 floats per line) |
| `stasis_gfx_load_sprite(path)` | Load and bake an SVG sprite into an atlas; returns handle |
| `stasis_gfx_draw_sprite(handle, x, y, sx, sy, rot, r, g, b, a)` | Draw baked sprite (centered) with scale/rotation/tint |
| `stasis_gfx_draw_sprites_i32(cmds, count)` | Batch: draw `count` sprites from an `i32` array (7 ints per sprite) |
| `stasis_gfx_debug_bake_hash(path)` | Debug: bake SVG on CPU and return a pixel hash |
| `stasis_gfx_debug_enable_hash(enabled)` | Debug: enable per-frame draw-call hash (for verifying batch equivalence) |
| `stasis_gfx_debug_get_frame_hash()` | Debug: get current frame hash (0 if disabled) |
| `stasis_is_key_down(scancode)` | Check if key is pressed |
| `stasis_get_time_ms()` | Get time in milliseconds |
| `stasis_sleep_ms(ms)` | Sleep for milliseconds |
| `stasis_should_quit()` | Pump input/events (once per frame) and report quit state |
| `stasis_input_pointer_count()` | Number of pointers tracked this frame (mouse + active touches) |
| `stasis_input_pointer_*` | Pointer snapshot queries (pos, deltas, edge flags) |
| `stasis_input_viewport_*_px` | Viewport rectangle (currently full window) |
| `stasis_audio_is_available()` | Initialize audio if needed; returns 1 on success |
| `stasis_audio_get_sample_rate()` | Current audio sample rate (Hz) |
| `stasis_audio_get_channels()` | Current audio channels (v1: 2) |
| `stasis_audio_get_queued_frames()` | Frames currently queued in the ring buffer |
| `stasis_audio_get_underruns()` | Underrun counter (device starved -> outputs silence) |
| `stasis_audio_push_f32_interleaved(ptr, frames)` | Push `f32` interleaved frames (LRLR...); returns frames accepted |

## SDL Scancodes

Common key scancodes for input:
- W = 26, A = 4, S = 22, D = 7
- Space = 44, Escape = 41
- Arrow keys: Up = 82, Down = 81, Left = 80, Right = 79
