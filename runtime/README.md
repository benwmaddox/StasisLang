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
| `stasis_is_key_down(scancode)` | Check if key is pressed |
| `stasis_get_time_ms()` | Get time in milliseconds |
| `stasis_sleep_ms(ms)` | Sleep for milliseconds |
| `stasis_should_quit()` | Check if window should close |

## SDL Scancodes

Common key scancodes for input:
- W = 26, A = 4, S = 22, D = 7
- Space = 44, Escape = 41
- Arrow keys: Up = 82, Down = 81, Left = 80, Right = 79
