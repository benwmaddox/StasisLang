# Building Minimal SDL2 for Stasis

To reduce exe size, build SDL2 from source with only required features:

## SDL2 CMake Options to Disable

```bash
cmake ../SDL \
  -DSDL_AUDIO=OFF \           # No audio system (~30% savings)
  -DSDL_HAPTIC=OFF \          # No force feedback
  -DSDL_JOYSTICK=OFF \        # No gamepad support
  -DSDL_SENSOR=OFF \          # No sensor APIs
  -DSDL_RENDER=OFF \          # Don't need SDL_Renderer (using OpenGL directly)
  -DSDL_POWER=OFF \           # Battery status API
  -DSDL_FILESYSTEM=OFF \      # Filesystem helpers
  -DSDL_TIMERS=ON \           # Keep timers (needed)
  -DSDL_VIDEO=ON \            # Keep video (needed)
  -DSDL_EVENTS=ON \           # Keep events (needed)
  -DSDL_RENDER_D3D=OFF \      # No Direct3D backend
  -DSDL_RENDER_METAL=OFF \    # No Metal backend
  -DSDL_HIDAPI=OFF \          # No HID device support
  -DBUILD_SHARED_LIBS=OFF     # Static library
```

## Expected Savings

- Audio subsystem: ~500KB final exe
- Joystick/Haptic: ~200KB
- Unused render backends: ~150KB
- **Total reduction: ~800KB-1MB (final exe would be ~800KB-1MB)**

## Alternative: Windows-only minimal build

For Windows-only distribution, could replace SDL2 entirely with:
- Win32 API for windowing (~50KB)
- WGL for OpenGL context (~20KB)
- Win32 input handling (~10KB)

**Savings: Exe drops to ~400-500KB** but loses Linux/Mac support.
