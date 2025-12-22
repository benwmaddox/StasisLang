# SVG Migration Plan

Replace the custom `.stv` vector format with standard SVG, adding a smart PNG cache layer.

## Current State

```
.stv file → bake_stv_to_rgba() → RGBA pixels → GPU atlas/SDL texture
                    │
                    └── Hot reload via mtime checking
```

**Existing .stv files** (7 total):
- `assets_src/flappy-birds/bird.stv` (16x12)
- `assets_src/flappy-birds/pipe.stv` (24x64)
- `assets_src/brickout-revenge/ball.stv` (32x32)
- `assets_src/brickout-revenge/paddle.stv` (128x24)
- `assets_src/brickout-revenge/brick_basic.stv`
- `assets_src/brickout-revenge/brick_armored.stv`
- `assets_src/brickout-revenge/brick_reflector.stv`

---

## Target Architecture

```
                                ┌─────────────────────────┐
                                │   .cache/sprites/       │
                                │   ├── bird.png          │
                                │   ├── pipe.png          │
                                │   └── ...               │
                                └───────────┬─────────────┘
                                            │ (if cache valid)
                                            ▼
.svg file ──► SVG mtime check ──► Load cached PNG ──► RGBA pixels ──► GPU atlas
                    │
                    │ (if cache stale/missing)
                    ▼
            nanosvg rasterize ──► Write PNG cache ──► RGBA pixels ──► GPU atlas
```

**Key properties:**
- SVG rasterization only happens when source changes
- PNG cache provides instant loads after first rasterization
- Hot reload still works (checks SVG mtime vs cache mtime)
- Cache is per-project, gitignored

---

## SVG Library Options

### Option A: nanosvg (Recommended)

**What**: Single-header C library (nanosvg.h + nanosvgrast.h)

**Pros:**
- Zero dependencies (just add two .h files)
- Public domain / MIT license
- ~2500 lines total, easy to audit
- Supports: paths, basic shapes, gradients, transforms
- Already used by Dear ImGui, raylib, SDL_svg

**Cons:**
- No text support (must convert text to paths)
- No CSS styling
- No filters/effects

**Integration:**
```c
#define NANOSVG_IMPLEMENTATION
#include "nanosvg.h"
#define NANOSVGRAST_IMPLEMENTATION
#include "nanosvgrast.h"
```

**Verdict:** Best fit for game sprites. Simple shapes, no text needed.

### Option B: lunasvg

**What**: C++ library with C API

**Pros:**
- Better SVG 1.1 compliance
- Text support
- MIT license

**Cons:**
- C++ dependency
- ~8000 lines
- Build complexity

**Verdict:** Overkill for simple game sprites.

### Option C: resvg

**What**: Rust library with C bindings

**Pros:**
- Excellent SVG 2 support
- High-quality rendering
- Production-tested (used by many tools)

**Cons:**
- Rust toolchain required for builds
- Large binary size (~2MB)
- Complex integration

**Verdict:** Too heavy for this use case.

### Recommendation: **nanosvg**

---

## PNG Cache Strategy

### Cache Location
```
project/
├── assets/
│   ├── bird.svg          # Source (committed)
│   └── pipe.svg
└── .cache/
    └── sprites/
        ├── bird.png      # Generated (gitignored)
        └── pipe.png
```

### Cache Invalidation Logic

```c
bool is_cache_valid(const char* svg_path, const char* png_path) {
    uint64_t svg_mtime = get_file_mtime(svg_path);
    uint64_t png_mtime = get_file_mtime(png_path);

    // Cache valid if PNG exists and is newer than SVG
    return png_mtime > 0 && png_mtime >= svg_mtime;
}
```

### Cache Path Generation

```c
// "assets/sprites/bird.svg" → ".cache/sprites/bird.png"
void get_cache_path(const char* svg_path, char* out_path, size_t out_size) {
    // Extract filename without extension
    const char* filename = strrchr(svg_path, '/');
    if (!filename) filename = strrchr(svg_path, '\\');
    if (!filename) filename = svg_path;
    else filename++;

    // Build cache path
    snprintf(out_path, out_size, ".cache/sprites/%.*s.png",
             (int)(strrchr(filename, '.') - filename), filename);
}
```

### PNG Reading/Writing

Use `stb_image.h` (read) and `stb_image_write.h` (write) - both single-header, public domain.

Already have stb_truetype in the project, so stb pattern is established.

---

## Implementation Plan

### Step 1: Add Header Libraries

Add to `runtime/`:
- `nanosvg.h` - SVG parsing
- `nanosvgrast.h` - SVG rasterization
- `stb_image.h` - PNG reading
- `stb_image_write.h` - PNG writing

### Step 2: Implement Core Functions

```c
// New functions in stasis_graphics.c

// Rasterize SVG to RGBA (with 2x supersampling like .stv)
static unsigned char* bake_svg_to_rgba(const char* path, int* out_w, int* out_h);

// PNG cache operations
static bool cache_is_valid(const char* svg_path, const char* cache_path);
static unsigned char* cache_load_png(const char* path, int* w, int* h);
static bool cache_save_png(const char* path, const unsigned char* rgba, int w, int h);
static void cache_ensure_dir(void);
```

### Step 3: Update Sprite Loading

Modify `sprite_build_into_entry()`:

```c
static int sprite_build_into_entry(SpriteEntry* e, const char* path) {
    int w, h;
    unsigned char* rgba = NULL;

    // Determine if SVG or legacy .stv
    bool is_svg = str_ends_with(path, ".svg");

    if (is_svg) {
        char cache_path[512];
        get_cache_path(path, cache_path, sizeof(cache_path));

        if (cache_is_valid(path, cache_path)) {
            // Fast path: load from PNG cache
            rgba = cache_load_png(cache_path, &w, &h);
        } else {
            // Slow path: rasterize SVG, save to cache
            rgba = bake_svg_to_rgba(path, &w, &h);
            if (rgba) {
                cache_ensure_dir();
                cache_save_png(cache_path, rgba, w, h);
            }
        }
    } else {
        // Legacy .stv support (can remove later)
        rgba = bake_stv_to_rgba(path, &w, &h);
    }

    if (!rgba) return 0;

    // ... rest of upload to atlas unchanged ...
}
```

### Step 4: Update Hot Reload

Modify `stasis_gfx_poll_reload()` to check SVG mtime (not cache mtime):

```c
STASIS_EXPORT int stasis_gfx_poll_reload(int handle) {
    SpriteEntry* e = get_sprite_entry(handle);
    if (!e) return 0;

    uint64_t current_mtime = get_file_mtime(e->path);  // SVG file
    if (current_mtime != e->mtime) {
        e->mtime = current_mtime;

        // Invalidate cache by deleting it (next load will regenerate)
        if (str_ends_with(e->path, ".svg")) {
            char cache_path[512];
            get_cache_path(e->path, cache_path, sizeof(cache_path));
            remove(cache_path);  // Delete stale cache
        }

        sprite_build_into_entry(e, e->path);
        return 1;
    }
    return 0;
}
```

### Step 5: Convert Assets

Convert each .stv to equivalent SVG:

**bird.stv → bird.svg:**
```svg
<svg xmlns="http://www.w3.org/2000/svg" width="16" height="12">
  <rect x="2" y="3" width="12" height="6" fill="rgba(255,230,51,1)"/>
  <circle cx="12" cy="6" r="2" fill="rgba(242,179,38,1)"/>
  <rect x="10" y="4" width="2" height="2" fill="black"/>
</svg>
```

### Step 6: Remove .stv Code

Once all assets converted and tested, remove:
- `bake_stv_to_rgba()` function (~100 lines)
- .stv parsing logic
- Legacy format documentation references

---

## File Changes Summary

| File | Change |
|------|--------|
| `runtime/nanosvg.h` | Add (new) |
| `runtime/nanosvgrast.h` | Add (new) |
| `runtime/stb_image.h` | Add (new) |
| `runtime/stb_image_write.h` | Add (new) |
| `runtime/stasis_graphics.c` | Add SVG functions, update sprite loading |
| `runtime/CMakeLists.txt` | No change (headers only) |
| `assets_src/**/*.stv` | Convert to .svg |
| `.gitignore` | Add `.cache/` |

---

## API Changes

**None.** The public API remains unchanged:
- `stasis_gfx_load_sprite(path)` - now accepts `.svg` files
- `stasis_gfx_poll_reload(handle)` - works the same
- `stasis_gfx_draw_sprite(...)` - unchanged

---

## Testing Plan

1. **Unit test**: `stasis_gfx_debug_bake_hash()` produces consistent results for same SVG
2. **Visual test**: Render each converted sprite, compare to .stv version
3. **Hot reload test**: Modify SVG while running, verify sprite updates
4. **Cache test**:
   - First load: SVG rasterized, PNG created
   - Second load: PNG loaded (faster)
   - Modify SVG: Cache invalidated, re-rasterized

---

## Migration Path

1. **Phase 1**: Add SVG support alongside .stv (both work)
2. **Phase 2**: Convert all .stv files to .svg
3. **Phase 3**: Update documentation and tutorials
4. **Phase 4**: Remove .stv support (breaking change, major version bump)

For now, recommend keeping .stv support as fallback during transition.

---

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| nanosvg doesn't support needed SVG feature | Keep sprites simple; test conversions early |
| PNG cache gets corrupted | Regenerate on any load failure |
| Cache directory missing | Create on first write |
| Performance regression | Cache eliminates repeat rasterization |
| SVG size different from .stv | Explicit viewBox in SVG |

---

## Decision: Proceed with nanosvg + PNG cache

This approach:
- Minimal dependencies (4 header files)
- No build system changes
- Backward compatible (supports both formats)
- Fast iteration (PNG cache)
- Hot reload preserved
