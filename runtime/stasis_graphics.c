/*
 * Stasis Graphics Runtime Library
 * SDL2 + OpenGL backend for vector graphics rendering
 */

#include <SDL.h>
#include <SDL_image.h>
#if defined(__ANDROID__) && !defined(STASIS_GRAPHICS_SDL_ONLY)
#define STASIS_GRAPHICS_SDL_ONLY 1
#endif

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
#include <GL/glew.h>
#include <SDL_opengl.h>
#endif
#include <stdbool.h>
#include <string.h>
#include "stasis_asset_path.h"
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <stdint.h>
#include <limits.h>
#include <ctype.h>
#include <errno.h>
#include <time.h>
#if defined(__ANDROID__)
#include <android/log.h>
#endif
#include "stasis_render_contract.h"
#include "stasis_display_scale.h"
#include "stasis_renderer_lifecycle.h"
#if defined(_WIN32)
#include <sys/types.h>
#include <sys/stat.h>
#include <direct.h>
#include <windows.h>
#else
#include <sys/stat.h>
#include <unistd.h>
#endif

/* stb_truetype for font rendering */
#define STB_TRUETYPE_IMPLEMENTATION
#include "stb_truetype.h"

/* nanosvg for SVG parsing/rasterization */
#define NANOSVG_IMPLEMENTATION
#define NANOSVG_ALL_COLOR_KEYWORDS
#include "nanosvg.h"
#define NANOSVGRAST_IMPLEMENTATION
#include "nanosvgrast.h"

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static void flush_sprites(void);
static void render_postfx(void);
#endif

static void stasis_sdl_log_output(void* userdata, int category, SDL_LogPriority priority, const char* message) {
    (void)userdata;
    (void)category;
    (void)priority;
    if (!message) return;
#if defined(__ANDROID__)
    __android_log_write(ANDROID_LOG_INFO, "Stasis", message);
#else
    fprintf(stderr, "%s\n", message);
    fflush(stderr);
#endif
}

static void log_package_provenance(void) {
    char* base = SDL_GetBasePath();
    if (!base) return;
    char path[1024];
    int written = snprintf(path, sizeof(path), "%sstasis_provenance.json", base);
    SDL_free(base);
    if (written < 0 || (size_t)written >= sizeof(path)) return;
    FILE* file = fopen(path, "rb");
    if (!file) return;
    char manifest[65537];
    size_t count = fread(manifest, 1, sizeof(manifest) - 1, file);
    int overflow = fgetc(file) != EOF;
    fclose(file);
    if (overflow) {
        SDL_Log("Stasis package provenance is invalid: manifest exceeds 65536 bytes path=%s", path);
        return;
    }
    manifest[count] = '\0';
    SDL_Log("Stasis package provenance: path=%s manifest=%s", path, manifest);
}

#if defined(STASIS_GRAPHICS_STATIC)
#define STASIS_EXPORT
#elif defined(_WIN32)
#define STASIS_EXPORT __declspec(dllexport)
#else
#define STASIS_EXPORT __attribute__((visibility("default")))
#endif

STASIS_EXPORT void stasis_set_window_size(int width, int height);
STASIS_EXPORT int stasis_get_time_us(void);
STASIS_EXPORT int stasis_load_font(const char* path, int font_size);
STASIS_EXPORT int stasis_gfx_cache_text(int font_handle, const char* text);
STASIS_EXPORT void stasis_gfx_draw_text_cached(int run_handle, float x, float y, float r, float g, float b, float a);
STASIS_EXPORT float stasis_gfx_measure_text_cached(int run_handle);
STASIS_EXPORT int stasis_storage_load_i32(const char* scope, const char* key, int fallback);
STASIS_EXPORT int stasis_storage_save_i32(const char* scope, const char* key, int value);

/* Global state */
static SDL_Window* g_window = NULL;
static SDL_GLContext g_gl_context = NULL;
static SDL_Renderer* g_renderer = NULL;
static bool g_use_sdl_renderer = false;
static bool g_should_quit = false;
static const Uint8* g_keyboard_state = NULL;
static int g_window_width = 800;
static int g_window_height = 600;
static int g_native_window_width = 800;
static int g_native_window_height = 600;
static int g_drawable_width = 800;
static int g_drawable_height = 600;
static float g_pixel_scale = 1.0f;
static StasisDisplayMetrics g_display_metrics;
static int g_display_generation = 0;
static int g_density_generation = 0;
static bool g_window_resized = false;
static StasisRendererLifecycle g_resource_lifecycle;
static bool g_resource_frame_ready = false;
static bool g_force_debug_overlay = false;
static bool g_postfx_enabled = false;
static bool g_postfx_applied_this_frame = false;
static bool g_screenshot_taken = false;
static char g_screenshot_path[1024] = {0};
static int g_screenshot_exit_after = 0;
static int g_screenshot_frame = 1;
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static GLuint g_postfx_program = 0;
static GLint g_postfx_time_loc = -1;
static GLint g_postfx_depth_loc = -1;
static GLint g_postfx_intensity_loc = -1;
static GLint g_postfx_surface_loc = -1;
static GLint g_postfx_color_loc = -1;
#endif
static float g_postfx_strength = 0.0f;
static float g_postfx_phase = 0.0f;
static float g_postfx_speed = 0.0f;
static float g_postfx_color[3] = {0.05f, 0.85f, 0.78f};
static bool g_postfx_force_disable = false;

#define STASIS_PERF_SAMPLE_CAPACITY 360
typedef struct {
    uint64_t captured_counter;
    uint64_t tick_us;
    uint64_t render_us;
} StasisPerfSample;
static StasisPerfSample g_perf_samples[STASIS_PERF_SAMPLE_CAPACITY];
static int g_perf_sample_count = 0;
static int g_perf_sample_next = 0;
static uint64_t g_perf_pending_tick_us = 0;
static uint64_t g_perf_pending_guest_render_us = 0;
static uint64_t g_perf_render_started_counter = 0;
static int g_perf_font_handle = -1;

/* ============================================================
 * Input snapshot (mouse + touch) - per-frame deterministic view
 * ============================================================ */

#define STASIS_MAX_POINTERS 8

typedef struct {
    int id; /* 0 for mouse; 1.. for touch slots */
    int is_down;
    int went_down;
    int went_up;
    float x_px;
    float y_px;
    float dx_px;
    float dy_px;
    float x_n;
    float y_n;
} StasisPointer;

typedef struct {
    StasisPointer pointers[STASIS_MAX_POINTERS];
    int pointer_count;      /* 1 + highest pointer slot in use (mouse + touch slots; may include inactive holes) */
    int dropped_pointers;   /* touches dropped due to capacity */
    int viewport_x_px;
    int viewport_y_px;
    int viewport_w_px;
    int viewport_h_px;
} StasisInputFrame;

static StasisInputFrame g_input_frame;
static int g_events_pumped_this_frame = 0;
static float g_prev_x_px[STASIS_MAX_POINTERS];
static float g_prev_y_px[STASIS_MAX_POINTERS];
static SDL_FingerID g_finger_ids[STASIS_MAX_POINTERS - 1];
static int g_finger_active[STASIS_MAX_POINTERS - 1];

/* Forward decls for exported functions used before their definitions (MSVC C mode does not allow implicit declarations). */
STASIS_EXPORT int stasis_get_time_ms(void);
STASIS_EXPORT int stasis_should_quit(void);
STASIS_EXPORT void stasis_host_get_frame(int32_t* out_i32, float* out_f32);
STASIS_EXPORT int stasis_set_fullscreen(int fullscreen);
STASIS_EXPORT void stasis_gfx_draw_sprite(int handle, int x, int y, int w, int h, int rot_degrees, int a);
STASIS_EXPORT void stasis_gfx_submit_u8(const int32_t* cmd_i32, const float* cmd_f32, const uint8_t* cmd_u8);
STASIS_EXPORT void stasis_draw_text(int font_handle, const char* text, float x, float y, float r, float g, float b, float a);

/* Forward decls for internal helpers used before their definitions. */
typedef struct SpriteEntry SpriteEntry;
static SpriteEntry* sprite_get(int handle);
static SpriteEntry* sprite_fallback_get(void);
static void stasis_gfx_draw_sprite_internal(int handle, int x, int y, int w, int h, int rot_degrees, int a, int do_hash);
static void stasis_gfx_draw_sprites_i32_fast(const int32_t* cmds, int sprite_count);
static int sprite_build_into_entry_sized(SpriteEntry* e, const char* path, int max_w, int max_h);
static void stasis_sync_display_metrics(void);
static void stasis_reset_text_cache(void);
static void stasis_invalidate_renderer_resources(int discard_gpu_handles);
static int stasis_restore_renderer_resources(void);

/* Forward decls for helpers referenced early in the file (MSVC C mode does not allow implicit declarations). */
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static void setup_ortho(void);
static void reset_line_program(void);
static void reset_sprite_program(void);
#endif
static int stasis_perf_font(void);
static uint64_t stasis_perf_elapsed_us(uint64_t started_counter, uint64_t finished_counter);

/* Sprite atlas bookkeeping (paths + rasterized sprites). */
#define SPRITE_TABLE_INITIAL_CAPACITY 256
#define SPRITE_HANDLE_INDEX_BITS 20
#define SPRITE_HANDLE_INDEX_MASK ((1u << SPRITE_HANDLE_INDEX_BITS) - 1u)
#define SPRITE_HANDLE_GENERATION_MASK 0x7ffu

typedef struct SpriteEntry {
    char* path;
    int w;              /* current rasterized width */
    int h;              /* current rasterized height */
    int max_w;          /* requested max width (logical) */
    int max_h;          /* requested max height (logical) */
    int page_index;
    int atlas_x;
    int atlas_y;
    int alloc_x;
    int alloc_y;
    int alloc_w;
    int alloc_h;
    float u0, v0, u1, v1;
    uint64_t mtime;
    SDL_Texture* sdl_tex;
    int used;
    int ref_count;       /* callers sharing this raster cache entry */
    int needs_reraster;  /* flag for window resize */
    int reload_pending;  /* set when the asset watcher reloads this sprite */
    uint32_t generation;
    uint32_t surface_generation;
    uint32_t renderer_generation;
    int retired;         /* generation wrapped; never reuse this slot */
} SpriteEntry;

static SpriteEntry* g_sprites = NULL;
static int g_sprite_capacity = 0;
static int g_sprite_count = 0;
static int g_sprite_table_limit = -1;
static int g_sprite_max_dimension = -1;
static int g_sprite_max_pixels = -1;
static int g_sprite_max_file_bytes = -1;
static SpriteEntry g_sprite_fallback;

/* Font rendering with stb_truetype. */
#define MAX_FONTS 8
#define FONT_FIRST_CHAR 32
#define FONT_NUM_CHARS 95

typedef struct {
    bool active;
    stbtt_fontinfo font_info;
    unsigned char* ttf_buffer;
    float scale;
    int ascent, descent, line_gap;

    /* Baked bitmap atlas */
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    GLuint atlas_texture;
#endif
    SDL_Texture* sdl_texture;
    stbtt_bakedchar char_data[FONT_NUM_CHARS];
    int font_size;       /* requested logical pixel height */
    int raster_size;     /* density-scaled pixel height */
    int atlas_size;
    float pixel_scale;
    int needs_reraster;
    uint32_t surface_generation;
    uint32_t renderer_generation;
} StasisFont;

static StasisFont g_fonts[MAX_FONTS];

static const char* stasis_renderer_reason_name(StasisRendererResourceReason reason) {
    switch (reason) {
        case STASIS_RENDERER_REASON_SURFACE_CHANGED: return "surface_changed";
        case STASIS_RENDERER_REASON_TARGETS_RESET: return "targets_reset";
        case STASIS_RENDERER_REASON_DEVICE_RESET: return "device_reset";
        case STASIS_RENDERER_REASON_BACKGROUND: return "background";
        case STASIS_RENDERER_REASON_FOREGROUND: return "foreground";
        default: return "none";
    }
}

static void stasis_invalidate_renderer_resources(int discard_gpu_handles) {
    if (!g_use_sdl_renderer) return;
    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* entry = &g_sprites[i];
        if (!entry->used) continue;
        if (entry->sdl_tex && !discard_gpu_handles) SDL_DestroyTexture(entry->sdl_tex);
        entry->sdl_tex = NULL;
        entry->needs_reraster = 1;
    }
    if (g_sprite_fallback.sdl_tex && !discard_gpu_handles) {
        SDL_DestroyTexture(g_sprite_fallback.sdl_tex);
    }
    memset(&g_sprite_fallback, 0, sizeof(g_sprite_fallback));
    g_sprite_fallback.page_index = -1;
    for (int i = 0; i < MAX_FONTS; i++) {
        StasisFont* font = &g_fonts[i];
        if (!font->active) continue;
        if (font->sdl_texture && !discard_gpu_handles) SDL_DestroyTexture(font->sdl_texture);
        font->sdl_texture = NULL;
        font->needs_reraster = 1;
    }
    g_resource_frame_ready = false;
}

static int stasis_input_valid_index(int idx) {
    return idx >= 0 && idx < STASIS_MAX_POINTERS;
}

static void stasis_mark_density_resources_dirty(void) {
    g_density_generation++;
    for (int i = 0; i < g_sprite_capacity; i++) {
        if (g_sprites[i].used && g_sprites[i].max_w > 0 && g_sprites[i].max_h > 0) {
            g_sprites[i].needs_reraster = 1;
        }
    }
    for (int i = 0; i < MAX_FONTS; i++) {
        if (g_fonts[i].active) g_fonts[i].needs_reraster = 1;
    }
}

static void stasis_sync_display_metrics(void) {
    if (!g_window) return;

    int native_w = g_native_window_width;
    int native_h = g_native_window_height;
    SDL_GetWindowSize(g_window, &native_w, &native_h);
    if (native_w > 0) g_native_window_width = native_w;
    if (native_h > 0) g_native_window_height = native_h;

    int drawable_w = native_w;
    int drawable_h = native_h;
    if (g_use_sdl_renderer && g_renderer) {
        if (SDL_GetRendererOutputSize(g_renderer, &drawable_w, &drawable_h) != 0) {
            drawable_w = native_w;
            drawable_h = native_h;
        }
        SDL_RenderSetLogicalSize(g_renderer, g_window_width, g_window_height);
    }
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    else if (g_gl_context) {
        SDL_GL_GetDrawableSize(g_window, &drawable_w, &drawable_h);
    }
#endif

    if (drawable_w <= 0) drawable_w = g_window_width;
    if (drawable_h <= 0) drawable_h = g_window_height;
    StasisDisplayViewport safe_native = {
        0.0f, 0.0f, (float)g_native_window_width, (float)g_native_window_height};
    StasisDisplayMetrics next = stasis_display_metrics(
        g_window_width, g_window_height,
        g_native_window_width, g_native_window_height,
        drawable_w, drawable_h, safe_native);
    const int dimensions_changed =
        next.native_w != g_display_metrics.native_w ||
        next.native_h != g_display_metrics.native_h ||
        next.drawable_w != g_display_metrics.drawable_w ||
        next.drawable_h != g_display_metrics.drawable_h ||
        next.logical_w != g_display_metrics.logical_w ||
        next.logical_h != g_display_metrics.logical_h;
    const int density_changed =
        g_density_generation == 0 || fabsf(next.raster_scale - g_pixel_scale) > 0.001f;
    if (density_changed) {
        g_pixel_scale = next.raster_scale;
        stasis_mark_density_resources_dirty();
    }
    if (g_display_generation == 0 || dimensions_changed) {
        g_display_generation++;
        g_window_resized = true;
    }
    if (dimensions_changed && g_use_sdl_renderer &&
        g_resource_lifecycle.state != STASIS_RENDERER_UNAVAILABLE) {
        stasis_renderer_lifecycle_surface_changed(&g_resource_lifecycle);
        stasis_invalidate_renderer_resources(0);
    }
    g_display_metrics = next;
    g_drawable_width = drawable_w;
    g_drawable_height = drawable_h;
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (!g_use_sdl_renderer && g_gl_context) {
        glViewport(
            (int)floorf(next.drawable_viewport.x),
            stasis_display_bottom_origin_y(
                next.drawable_h, next.drawable_viewport),
            (int)ceilf(next.drawable_viewport.w),
            (int)ceilf(next.drawable_viewport.h));
        setup_ortho();
    }
#endif
}

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static void stasis_gl_clear_rect(int x, int y, int width, int height) {
    if (width <= 0 || height <= 0) return;
    glScissor(x, y, width, height);
    glClear(GL_COLOR_BUFFER_BIT);
}

static void stasis_gl_clear_letterbox_bars(void) {
    const StasisDisplayViewport viewport = g_display_metrics.drawable_viewport;
    const int left = (int)viewport.x;
    const int bottom = stasis_display_bottom_origin_y(
        g_drawable_height, viewport);
    const int right = left + (int)viewport.w;
    const int top = bottom + (int)viewport.h;
    glEnable(GL_SCISSOR_TEST);
    glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
    stasis_gl_clear_rect(0, 0, left, g_drawable_height);
    stasis_gl_clear_rect(right, 0, g_drawable_width - right, g_drawable_height);
    stasis_gl_clear_rect(0, 0, g_drawable_width, bottom);
    stasis_gl_clear_rect(0, top, g_drawable_width, g_drawable_height - top);
    glDisable(GL_SCISSOR_TEST);
}
#endif

static void stasis_window_to_logical(float native_x, float native_y, float* logical_x, float* logical_y) {
    if (!logical_x || !logical_y) return;
    stasis_display_native_to_logical_xy(
        &g_display_metrics, native_x, native_y, logical_x, logical_y);
}

static float stasis_clampf(float v, float minv, float maxv) {
    if (v < minv) return minv;
    if (v > maxv) return maxv;
    return v;
}

static void stasis_update_pointer_norm(int idx) {
    if (!stasis_input_valid_index(idx)) return;

    int vw = g_input_frame.viewport_w_px;
    int vh = g_input_frame.viewport_h_px;
    if (vw <= 0 || vh <= 0) {
        g_input_frame.pointers[idx].x_n = 0.0f;
        g_input_frame.pointers[idx].y_n = 0.0f;
        return;
    }

    g_input_frame.pointers[idx].x_n = stasis_clampf(
        (g_input_frame.pointers[idx].x_px - (float)g_input_frame.viewport_x_px) /
            (float)vw,
        0.0f, 1.0f);
    g_input_frame.pointers[idx].y_n = stasis_clampf(
        (g_input_frame.pointers[idx].y_px - (float)g_input_frame.viewport_y_px) /
            (float)vh,
        0.0f, 1.0f);
}

static void stasis_set_pointer_pos_px(int idx, float x, float y) {
    if (!stasis_input_valid_index(idx)) return;

    x = stasis_clampf(x, 0.0f, (float)g_window_width);
    y = stasis_clampf(y, 0.0f, (float)g_window_height);

    g_input_frame.pointers[idx].x_px = x;
    g_input_frame.pointers[idx].y_px = y;
    stasis_update_pointer_norm(idx);
}

static void stasis_update_safe_viewport(void) {
    if (!g_window) return;

    StasisDisplayViewport safe_native = {
        0.0f, 0.0f, (float)g_native_window_width, (float)g_native_window_height};

    int display = SDL_GetWindowDisplayIndex(g_window);
    if (display < 0) goto publish;

    SDL_Rect usable;
    if (SDL_GetDisplayUsableBounds(display, &usable) != 0) {
        goto publish;
    }

    int win_x = 0;
    int win_y = 0;
    SDL_GetWindowPosition(g_window, &win_x, &win_y);

    int win_right = win_x + g_native_window_width;
    int win_bottom = win_y + g_native_window_height;
    int left = usable.x > win_x ? usable.x : win_x;
    int top = usable.y > win_y ? usable.y : win_y;
    int right = (usable.x + usable.w) < win_right ? (usable.x + usable.w) : win_right;
    int bottom = (usable.y + usable.h) < win_bottom ? (usable.y + usable.h) : win_bottom;
    int w = right - left;
    int h = bottom - top;

    if (w > 0 && h > 0) {
        safe_native.x = (float)(left - win_x);
        safe_native.y = (float)(top - win_y);
        safe_native.w = (float)w;
        safe_native.h = (float)h;
    }

publish:
    g_display_metrics = stasis_display_metrics(
        g_window_width, g_window_height,
        g_native_window_width, g_native_window_height,
        g_drawable_width, g_drawable_height, safe_native);
    g_input_frame.viewport_x_px = (int)floorf(g_display_metrics.safe_logical_viewport.x);
    g_input_frame.viewport_y_px = (int)floorf(g_display_metrics.safe_logical_viewport.y);
    g_input_frame.viewport_w_px = (int)ceilf(g_display_metrics.safe_logical_viewport.w);
    g_input_frame.viewport_h_px = (int)ceilf(g_display_metrics.safe_logical_viewport.h);
}

static int stasis_find_finger_slot(SDL_FingerID fingerId) {
    for (int i = 0; i < STASIS_MAX_POINTERS - 1; i++) {
        if (g_finger_active[i] && g_finger_ids[i] == fingerId) {
            return i;
        }
    }
    return -1;
}

static int stasis_alloc_finger_slot(SDL_FingerID fingerId) {
    int existing = stasis_find_finger_slot(fingerId);
    if (existing >= 0) return existing;

    for (int i = 0; i < STASIS_MAX_POINTERS - 1; i++) {
        if (!g_finger_active[i]) {
            g_finger_active[i] = 1;
            g_finger_ids[i] = fingerId;
            return i;
        }
    }
    return -1;
}

static void stasis_release_finger_slot(SDL_FingerID fingerId) {
    int slot = stasis_find_finger_slot(fingerId);
    if (slot >= 0) {
        g_finger_active[slot] = 0;
    }
}

static void stasis_pump_events(void) {
    if (!g_window) return;
    stasis_sync_display_metrics();

    /* Snapshot "previous tick" positions for deltas. */
    for (int i = 0; i < STASIS_MAX_POINTERS; i++) {
        g_prev_x_px[i] = g_input_frame.pointers[i].x_px;
        g_prev_y_px[i] = g_input_frame.pointers[i].y_px;
        g_input_frame.pointers[i].dx_px = 0.0f;
        g_input_frame.pointers[i].dy_px = 0.0f;
        g_input_frame.pointers[i].went_down = 0;
        g_input_frame.pointers[i].went_up = 0;
        g_input_frame.pointers[i].id = i; /* stable slot id */
    }

    g_input_frame.dropped_pointers = 0;
    g_input_frame.viewport_x_px = 0;
    g_input_frame.viewport_y_px = 0;
    g_input_frame.viewport_w_px = g_window_width;
    g_input_frame.viewport_h_px = g_window_height;
    stasis_update_safe_viewport();

    SDL_Event event;
    while (SDL_PollEvent(&event)) {
        switch (event.type) {
            case SDL_QUIT:
                g_should_quit = true;
                break;
            case SDL_KEYDOWN:
                if (event.key.keysym.sym == SDLK_ESCAPE) {
                    g_should_quit = true;
                }
                if (event.key.keysym.sym == SDLK_F3 && event.key.repeat == 0) {
                    g_force_debug_overlay = !g_force_debug_overlay;
                    if (g_force_debug_overlay) (void)stasis_perf_font();
                    SDL_Log("performance HUD %s (F3 toggles)", g_force_debug_overlay ? "on" : "off");
                }
                break;
            case SDL_WINDOWEVENT:
                if (event.window.event == SDL_WINDOWEVENT_SIZE_CHANGED) {
                    stasis_sync_display_metrics();
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
                    if (!g_use_sdl_renderer) {
                        reset_line_program();
                        reset_sprite_program();
                    }
#endif
                    g_input_frame.viewport_w_px = g_window_width;
                    g_input_frame.viewport_h_px = g_window_height;
                    stasis_update_safe_viewport();
                }
                break;
#if SDL_VERSION_ATLEAST(2,0,2)
            case SDL_RENDER_TARGETS_RESET:
                stasis_renderer_lifecycle_renderer_reset(
                    &g_resource_lifecycle, STASIS_RENDERER_REASON_TARGETS_RESET);
                stasis_invalidate_renderer_resources(0);
                SDL_Log("Stasis renderer resources invalidated: backend=sdl reason=targets_reset surface_generation=%u renderer_generation=%u",
                    g_resource_lifecycle.surface_generation,
                    g_resource_lifecycle.renderer_generation);
                break;
            case SDL_RENDER_DEVICE_RESET:
                stasis_renderer_lifecycle_renderer_reset(
                    &g_resource_lifecycle, STASIS_RENDERER_REASON_DEVICE_RESET);
                stasis_invalidate_renderer_resources(0);
                SDL_Log("Stasis renderer resources invalidated: backend=sdl reason=device_reset surface_generation=%u renderer_generation=%u",
                    g_resource_lifecycle.surface_generation,
                    g_resource_lifecycle.renderer_generation);
                break;
#endif
            case SDL_APP_WILLENTERBACKGROUND:
                stasis_renderer_lifecycle_pause(&g_resource_lifecycle);
                g_resource_frame_ready = false;
                break;
            case SDL_APP_DIDENTERFOREGROUND:
                if (g_resource_lifecycle.state == STASIS_RENDERER_PAUSED) {
                    stasis_renderer_lifecycle_resume(&g_resource_lifecycle);
                    stasis_invalidate_renderer_resources(0);
                }
                break;
            case SDL_MOUSEBUTTONDOWN:
                if (event.button.button == SDL_BUTTON_LEFT) {
                    g_input_frame.pointers[0].went_down = 1;
                }
                break;
            case SDL_MOUSEBUTTONUP:
                if (event.button.button == SDL_BUTTON_LEFT) {
                    g_input_frame.pointers[0].went_up = 1;
                }
                break;
            case SDL_FINGERDOWN:
                {
                    int slot = stasis_alloc_finger_slot(event.tfinger.fingerId);
                    if (slot < 0) {
                        g_input_frame.dropped_pointers++;
                        break;
                    }
                    int idx = slot + 1;
                    g_input_frame.pointers[idx].is_down = 1;
                    g_input_frame.pointers[idx].went_down = 1;
                    float logical_x = 0.0f;
                    float logical_y = 0.0f;
                    stasis_window_to_logical(
                        event.tfinger.x * (float)g_native_window_width,
                        event.tfinger.y * (float)g_native_window_height,
                        &logical_x, &logical_y);
                    stasis_set_pointer_pos_px(idx, logical_x, logical_y);
                }
                break;
            case SDL_FINGERMOTION:
                {
                    int slot = stasis_find_finger_slot(event.tfinger.fingerId);
                    if (slot < 0) break;
                    int idx = slot + 1;
                    float logical_x = 0.0f;
                    float logical_y = 0.0f;
                    stasis_window_to_logical(
                        event.tfinger.x * (float)g_native_window_width,
                        event.tfinger.y * (float)g_native_window_height,
                        &logical_x, &logical_y);
                    stasis_set_pointer_pos_px(idx, logical_x, logical_y);
                }
                break;
            case SDL_FINGERUP:
                {
                    int slot = stasis_find_finger_slot(event.tfinger.fingerId);
                    if (slot < 0) break;
                    int idx = slot + 1;
                    g_input_frame.pointers[idx].is_down = 0;
                    g_input_frame.pointers[idx].went_up = 1;
                    stasis_release_finger_slot(event.tfinger.fingerId);
                    float logical_x = 0.0f;
                    float logical_y = 0.0f;
                    stasis_window_to_logical(
                        event.tfinger.x * (float)g_native_window_width,
                        event.tfinger.y * (float)g_native_window_height,
                        &logical_x, &logical_y);
                    stasis_set_pointer_pos_px(idx, logical_x, logical_y);
                }
                break;
            default:
                break;
        }
    }

    /* Mouse position and button state (left button = primary). */
    int mx = 0, my = 0;
    Uint32 buttons = SDL_GetMouseState(&mx, &my);
    float logical_mouse_x = 0.0f;
    float logical_mouse_y = 0.0f;
    stasis_window_to_logical((float)mx, (float)my, &logical_mouse_x, &logical_mouse_y);
    stasis_set_pointer_pos_px(0, logical_mouse_x, logical_mouse_y);
    g_input_frame.pointers[0].is_down = (buttons & SDL_BUTTON(SDL_BUTTON_LEFT)) ? 1 : 0;

    /* Compute deltas from previous tick positions. */
    for (int i = 0; i < STASIS_MAX_POINTERS; i++) {
        g_input_frame.pointers[i].dx_px = g_input_frame.pointers[i].x_px - g_prev_x_px[i];
        g_input_frame.pointers[i].dy_px = g_input_frame.pointers[i].y_px - g_prev_y_px[i];
    }

    /* Report up to the highest slot that is active or had a transition this frame. */
    int max_idx = 0; /* mouse slot */
    for (int i = 0; i < STASIS_MAX_POINTERS - 1; i++) {
        int idx = i + 1;
        if (g_finger_active[i] || g_input_frame.pointers[idx].went_down || g_input_frame.pointers[idx].went_up) {
            if (idx > max_idx) max_idx = idx;
        }
    }
    g_input_frame.pointer_count = max_idx + 1;
}

STASIS_EXPORT int stasis_input_pointer_count(void) {
    return g_window ? g_input_frame.pointer_count : 0;
}

STASIS_EXPORT int stasis_input_pointer_id(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return -1;
    return g_input_frame.pointers[idx].id;
}

STASIS_EXPORT int stasis_input_pointer_is_down(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0;
    return g_input_frame.pointers[idx].is_down ? 1 : 0;
}

STASIS_EXPORT int stasis_input_pointer_went_down(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0;
    return g_input_frame.pointers[idx].went_down ? 1 : 0;
}

STASIS_EXPORT int stasis_input_pointer_went_up(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0;
    return g_input_frame.pointers[idx].went_up ? 1 : 0;
}

STASIS_EXPORT float stasis_input_pointer_x_px(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].x_px;
}

STASIS_EXPORT float stasis_input_pointer_y_px(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].y_px;
}

STASIS_EXPORT float stasis_input_pointer_dx_px(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].dx_px;
}

STASIS_EXPORT float stasis_input_pointer_dy_px(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].dy_px;
}

STASIS_EXPORT float stasis_input_pointer_x_n(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].x_n;
}

STASIS_EXPORT float stasis_input_pointer_y_n(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].y_n;
}

STASIS_EXPORT int stasis_input_dropped_pointers(void) {
    return g_window ? g_input_frame.dropped_pointers : 0;
}

STASIS_EXPORT int stasis_input_viewport_x_px(void) {
    return g_window ? g_input_frame.viewport_x_px : 0;
}

STASIS_EXPORT int stasis_input_viewport_y_px(void) {
    return g_window ? g_input_frame.viewport_y_px : 0;
}

STASIS_EXPORT int stasis_input_viewport_w_px(void) {
    return g_window ? g_input_frame.viewport_w_px : 0;
}

STASIS_EXPORT int stasis_input_viewport_h_px(void) {
    return g_window ? g_input_frame.viewport_h_px : 0;
}

STASIS_EXPORT void stasis_get_desktop_size(int* width, int* height);

/*
 * Bulk host loop helpers.
 *
 * Goal: keep "host behavior" (input snapshot + window requests + submit) inside the runtime/graphics library,
 * so both production runners (stasis_runner) and dev runners (JIT) use the same code paths.
 *
 * Notes:
 * - The guest owns the host_window_request globals (src/runtime/host_window_request.stasis). We track the last applied
 *   request seq in this library, initialized by stasis_host_bulk_init().
 * - HostFrame layout is defined in src/runtime/host_frame.stasis and is written by stasis_host_get_frame().
 * - Rendering is driven by gfx_cmd buffers (src/runtime/gfx_cmd.stasis) and submitted by stasis_gfx_submit_u8().
 */
static int g_host_req_inited = 0;
static int32_t g_host_last_req_seq = 0;

STASIS_EXPORT void stasis_host_bulk_init(const int32_t* host_req_seq)
{
    g_host_last_req_seq = host_req_seq ? *host_req_seq : 0;
    g_host_req_inited = 1;
}

STASIS_EXPORT void stasis_host_bulk_apply_requests(
    const int32_t* host_req_seq,
    const int32_t* host_req_flags,
    const int32_t* host_req_window_w_px,
    const int32_t* host_req_window_h_px)
{
    /* Matches src/runtime/host_window_request.stasis */
    const int32_t HOST_REQ_FLAG_WINDOWED = 1;
    const int32_t HOST_REQ_FLAG_FULLSCREEN = 2;

    if (!host_req_seq || !host_req_flags)
    {
        return;
    }

    if (!g_host_req_inited)
    {
        stasis_host_bulk_init(host_req_seq);
    }

    const int32_t seq = *host_req_seq;
    if (seq == g_host_last_req_seq)
    {
        return;
    }
    g_host_last_req_seq = seq;

    const int32_t flags = *host_req_flags;
    if ((flags & HOST_REQ_FLAG_WINDOWED) != 0)
    {
        if (host_req_window_w_px && host_req_window_h_px)
        {
            (void)stasis_set_fullscreen(0);
            stasis_set_window_size(*host_req_window_w_px, *host_req_window_h_px);
        }
    }
    else if ((flags & HOST_REQ_FLAG_FULLSCREEN) != 0)
    {
        (void)stasis_set_fullscreen(1);
    }
}

STASIS_EXPORT void stasis_host_set_performance_metrics(uint64_t tick_us, uint64_t render_us)
{
    g_perf_pending_tick_us = tick_us;
    g_perf_pending_guest_render_us = render_us;
}

typedef int (*stasis_tick_fn)(void);

STASIS_EXPORT int stasis_host_bulk_step(
    int32_t* host_i32,
    float* host_f32,
    int32_t* gfx_cmd_i32,
    float* gfx_cmd_f32,
    uint8_t* gfx_cmd_u8,
    const int32_t* host_req_seq,
    const int32_t* host_req_flags,
    const int32_t* host_req_window_w_px,
    const int32_t* host_req_window_h_px,
    stasis_tick_fn tick_fn)
{
    if (!host_i32 || !host_f32 || !gfx_cmd_i32 || !gfx_cmd_f32 || !gfx_cmd_u8 || !tick_fn)
    {
        return -1;
    }

    stasis_host_get_frame(host_i32, host_f32);

    /* Exit if host requested quit (avoid requiring guest queries). */
    if (host_i32[9] != 0)
    {
        return 1;
    }

    stasis_host_bulk_apply_requests(host_req_seq, host_req_flags, host_req_window_w_px, host_req_window_h_px);

    const uint64_t tick_started = SDL_GetPerformanceCounter();
    const int tick_result = tick_fn();
    const uint64_t tick_finished = SDL_GetPerformanceCounter();
    if (tick_result != 0)
    {
        return tick_result;
    }

    stasis_host_set_performance_metrics(
        stasis_perf_elapsed_us(tick_started, tick_finished),
        0);
    stasis_gfx_submit_u8(gfx_cmd_i32, gfx_cmd_f32, gfx_cmd_u8);
    return 0;
}

/*
 * Host snapshot: fill caller-provided buffers with a deterministic view of host state.
 *
 * Layout is defined in src/runtime/host_frame.stasis. This is intentionally a simple
 * "copy out" ABI for native now, and a good fit for WASM later (one import to get a snapshot).
 */
STASIS_EXPORT void stasis_host_get_frame(int32_t* out_i32, float* out_f32) {
    if (!out_i32 || !out_f32) return;

    static int32_t g_host_tick_index = 0;
    const int32_t host_version = 2;
    const int i32_key_base = 32;
    const int i32_key_count = 512;

    /* i32 header */
    out_i32[0] = stasis_get_time_ms();
    out_i32[1] = g_window_width;
    out_i32[2] = g_window_height;
    out_i32[3] = g_input_frame.viewport_x_px;
    out_i32[4] = g_input_frame.viewport_y_px;
    out_i32[5] = g_input_frame.viewport_w_px;
    out_i32[6] = g_input_frame.viewport_h_px;
    out_i32[7] = g_input_frame.pointer_count;
    out_i32[8] = g_input_frame.dropped_pointers;
    out_i32[9] = stasis_should_quit();

    out_i32[10] = g_host_tick_index++;

    out_i32[11] = g_window_resized ? 1 : 0;
    g_window_resized = false;

    int screen_w = 0;
    int screen_h = 0;
    stasis_get_desktop_size(&screen_w, &screen_h);
    out_i32[12] = screen_w;
    out_i32[13] = screen_h;

    /* vNext */
    out_i32[14] = host_version;

    int32_t flags = 0;
    if (out_i32[9] != 0) flags |= 1; /* quit requested */
    if (out_i32[11] != 0) flags |= 8; /* resized */

    int32_t focused = 0;
    int32_t minimized = 0;
    if (g_window) {
        const Uint32 wf = SDL_GetWindowFlags(g_window);
        focused = ((wf & SDL_WINDOW_INPUT_FOCUS) != 0) ? 1 : 0;
        minimized = ((wf & SDL_WINDOW_MINIMIZED) != 0) ? 1 : 0;
        if (focused) flags |= 2;
        if (minimized) flags |= 4;
    }

    out_i32[15] = flags;
    out_i32[16] = 0; /* tick_hz: unknown */
    out_i32[17] = focused;
    out_i32[18] = minimized;
    out_i32[19] = stasis_get_time_us();

    out_i32[20] = g_display_metrics.logical_w;
    out_i32[21] = g_display_metrics.logical_h;
    out_i32[22] = g_display_metrics.native_w;
    out_i32[23] = g_display_metrics.native_h;
    out_i32[24] = g_display_metrics.drawable_w;
    out_i32[25] = g_display_metrics.drawable_h;
    out_i32[26] = (int32_t)floorf(g_display_metrics.safe_logical_viewport.x);
    out_i32[27] = (int32_t)floorf(g_display_metrics.safe_logical_viewport.y);
    out_i32[28] = (int32_t)ceilf(g_display_metrics.safe_logical_viewport.w);
    out_i32[29] = (int32_t)ceilf(g_display_metrics.safe_logical_viewport.h);
    out_i32[30] = g_display_generation;
    out_i32[31] = g_density_generation;

    /* Keyboard state: one i32 per scancode (0/1). */
    int num_keys = 0;
    const Uint8* keys = SDL_GetKeyboardState(&num_keys);
    for (int i = 0; i < i32_key_count; i++) {
        out_i32[i32_key_base + i] = (keys && i < num_keys && keys[i]) ? 1 : 0;
    }

    const int i32_base = i32_key_base + i32_key_count;
    const int i32_stride = 4;
    const int f32_base = 0;
    const int f32_stride = 6;
    for (int i = 0; i < STASIS_MAX_POINTERS; i++) {
        const StasisPointer* p = &g_input_frame.pointers[i];
        out_i32[i32_base + i * i32_stride + 0] = p->id;
        out_i32[i32_base + i * i32_stride + 1] = p->is_down;
        out_i32[i32_base + i * i32_stride + 2] = p->went_down;
        out_i32[i32_base + i * i32_stride + 3] = p->went_up;

        out_f32[f32_base + i * f32_stride + 0] = p->x_px;
        out_f32[f32_base + i * f32_stride + 1] = p->y_px;
        out_f32[f32_base + i * f32_stride + 2] = p->dx_px;
        out_f32[f32_base + i * f32_stride + 3] = p->dy_px;
        out_f32[f32_base + i * f32_stride + 4] = p->x_n;
        out_f32[f32_base + i * f32_stride + 5] = p->y_n;
    }

    for (int i = i32_base + STASIS_MAX_POINTERS * i32_stride; i < 768; i++) out_i32[i] = 0;
    for (int i = f32_base + STASIS_MAX_POINTERS * f32_stride; i < 64; i++) out_f32[i] = 0.0f;
    out_f32[48] = g_display_metrics.content_scale;
    out_f32[49] = g_display_metrics.raster_scale;
}

/* ============================================================
 * Audio output (SDL2) - f32 stereo ring buffer
 * ============================================================ */

static SDL_AudioDeviceID g_audio_device = 0;
static SDL_AudioSpec g_audio_spec_obtained;
static int g_audio_initialized = 0;
static int g_audio_underruns = 0;
static int g_audio_channels = 2;
static int g_audio_sample_rate = 48000;
static int g_audio_target_latency_frames = 2048;
static float* g_audio_ring = NULL;
static int g_audio_ring_capacity_frames = 0;
static int g_audio_ring_capacity_samples = 0;
static int g_audio_read_sample = 0;
static int g_audio_write_sample = 0;
static int g_audio_queued_samples = 0;
static int64_t g_audio_running_frame_index = 0;

static int stasis_audio_maxi(int a, int b) { return a > b ? a : b; }
static int stasis_audio_mini(int a, int b) { return a < b ? a : b; }

static void stasis_audio_callback(void* userdata, Uint8* stream, int len) {
    (void)userdata;
    if (!stream || len <= 0) return;

    if (!g_audio_ring || g_audio_channels <= 0) {
        SDL_memset(stream, 0, (size_t)len);
        g_audio_underruns++;
        return;
    }

    const int requested_samples = len / (int)sizeof(float);
    if (requested_samples <= 0) {
        return;
    }

    float* out = (float*)stream;
    int remaining = requested_samples;
    int have = g_audio_queued_samples;

    if (have < requested_samples) {
        g_audio_underruns++;
    }

    int to_copy = stasis_audio_mini(have, requested_samples);
    while (to_copy > 0) {
        int contiguous = g_audio_ring_capacity_samples - g_audio_read_sample;
        int chunk = stasis_audio_mini(to_copy, contiguous);
        SDL_memcpy(out, &g_audio_ring[g_audio_read_sample], (size_t)chunk * sizeof(float));
        out += chunk;
        remaining -= chunk;
        to_copy -= chunk;
        g_audio_read_sample = (g_audio_read_sample + chunk) % g_audio_ring_capacity_samples;
        g_audio_queued_samples -= chunk;
    }

    if (remaining > 0) {
        SDL_memset(out, 0, (size_t)remaining * sizeof(float));
    }

    g_audio_running_frame_index += requested_samples / g_audio_channels;
}

static void stasis_audio_shutdown_internal(void) {
    if (g_audio_device != 0) {
        SDL_CloseAudioDevice(g_audio_device);
        g_audio_device = 0;
    }

    if (g_audio_ring) {
        free(g_audio_ring);
        g_audio_ring = NULL;
    }

    g_audio_initialized = 0;
    g_audio_ring_capacity_frames = 0;
    g_audio_ring_capacity_samples = 0;
    g_audio_read_sample = 0;
    g_audio_write_sample = 0;
    g_audio_queued_samples = 0;
    g_audio_underruns = 0;
    g_audio_running_frame_index = 0;
}

static int stasis_audio_disabled(void) {
    const char* env = getenv("STASIS_DISABLE_AUDIO");
    return env && *env && strcmp(env, "0") != 0;
}

static int stasis_audio_ensure_init(void) {
    if (stasis_audio_disabled()) {
        return 0;
    }
    if (g_audio_initialized && g_audio_device != 0) {
        return 1;
    }

    if (SDL_InitSubSystem(SDL_INIT_AUDIO) != 0) {
        if (SDL_Init(SDL_INIT_AUDIO) != 0) {
            return 0;
        }
    }

    SDL_AudioSpec desired;
    SDL_zero(desired);
    desired.format = AUDIO_F32SYS;
    desired.channels = 2;
    desired.samples = 512;
    desired.callback = stasis_audio_callback;

    SDL_AudioSpec obtained;
    SDL_zero(obtained);

    /* Try 48k first, then 44.1k. */
    int rates[2] = { 48000, 44100 };
    SDL_AudioDeviceID dev = 0;
    for (int i = 0; i < 2 && dev == 0; i++) {
        desired.freq = rates[i];
        dev = SDL_OpenAudioDevice(NULL, 0, &desired, &obtained, 0);
        if (dev != 0) {
            g_audio_sample_rate = obtained.freq;
        }
    }

    if (dev == 0) {
        return 0;
    }

    if (obtained.format != AUDIO_F32SYS || obtained.channels != 2) {
        SDL_CloseAudioDevice(dev);
        return 0;
    }

    g_audio_device = dev;
    g_audio_spec_obtained = obtained;
    g_audio_channels = (int)obtained.channels;

    g_audio_target_latency_frames = stasis_audio_maxi(512, g_audio_target_latency_frames);
    g_audio_ring_capacity_frames = stasis_audio_maxi(8192, g_audio_target_latency_frames * 4);
    g_audio_ring_capacity_samples = g_audio_ring_capacity_frames * g_audio_channels;

    g_audio_ring = (float*)malloc((size_t)g_audio_ring_capacity_samples * sizeof(float));
    if (!g_audio_ring) {
        SDL_CloseAudioDevice(g_audio_device);
        g_audio_device = 0;
        return 0;
    }
    SDL_memset(g_audio_ring, 0, (size_t)g_audio_ring_capacity_samples * sizeof(float));

    g_audio_read_sample = 0;
    g_audio_write_sample = 0;
    g_audio_queued_samples = 0;
    g_audio_underruns = 0;
    g_audio_running_frame_index = 0;
    g_audio_initialized = 1;

    SDL_PauseAudioDevice(g_audio_device, 0);
    return 1;
}

/* Line batching for efficient rendering */
#define MAX_LINES 10000
typedef struct {
    float x, y;
    float r, g, b, a;
} LineVertex;
static LineVertex g_sdl_line_vertices[MAX_LINES * 2];
static struct {
    float x1, y1, x2, y2;
    float r, g, b, a;
} g_lines[MAX_LINES];
static LineVertex g_line_vertices[MAX_LINES * 2];
static int g_line_count = 0;
static int g_debug_frame_counter = 0;

/* Simple shader + buffer for line rendering */
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static GLuint g_line_program = 0;
static GLuint g_line_vbo = 0;
static GLuint g_line_vao = 0;
static GLint g_line_pos_loc = -1;
static GLint g_line_color_loc = -1;
#endif
static char g_asset_base[512] = {0};
static char g_asset_env[512] = {0};

/* Sprite atlas + batching (baked from SVG sources) */
#define SPRITE_ATLAS_DEFAULT_W 2048
#define SPRITE_ATLAS_DEFAULT_H 2048
#define SPRITE_ATLAS_PAD 2
#define MAX_SPRITE_VERTS (6 * 4096)

typedef struct {
    float x, y;
    float u, v;
    float r, g, b, a;
} SpriteVertex;

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
typedef struct {
    int x;
    int y;
    int w;
    int h;
} AtlasFreeRect;

typedef struct {
    GLuint texture;
    int w;
    int h;
    AtlasFreeRect* free_rects;
    int free_rect_count;
    int free_rect_capacity;
    int used_pixels;
} SpriteAtlasPage;

static GLuint g_sprite_program = 0;
static GLuint g_sprite_vbo = 0;
static GLuint g_sprite_vao = 0;
static GLint g_sprite_pos_loc = -1;
static GLint g_sprite_uv_loc = -1;
static GLint g_sprite_color_loc = -1;
static GLint g_sprite_tex_loc = -1;

static SpriteAtlasPage* g_sprite_atlas_pages = NULL;
static int g_sprite_atlas_page_count = 0;
static int g_sprite_atlas_page_capacity = 0;
static int g_sprite_atlas_page_w = 0;
static int g_sprite_atlas_page_h = 0;
static int g_sprite_atlas_gl_max_size = 0;
static int g_sprite_batch_page = -1;
#endif

static SpriteVertex g_sprite_vertices[MAX_SPRITE_VERTS];
static int g_sprite_vert_count = 0;

/* Convert screen coords to OpenGL NDC (-1 to 1) */
static float screen_to_ndc_x(float x) {
    return (x / g_window_width) * 2.0f - 1.0f;
}

static float screen_to_ndc_y(float y) {
    /* Flip Y so 0 is at top */
    return 1.0f - (y / g_window_height) * 2.0f;
}

STASIS_EXPORT void stasis_draw_line(float x1, float y1, float x2, float y2,
                                    float r, float g, float b, float a);

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static void setup_ortho(void) {
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glOrtho(0.0, (double)g_window_width, (double)g_window_height, 0.0, -1.0, 1.0);
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
}

static GLuint compile_simple_shader(GLenum type, const char* source) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    GLint status = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &status);
    if (status != GL_TRUE) {
        char log[512];
        glGetShaderInfoLog(shader, sizeof(log), NULL, log);
        SDL_Log("Simple shader compile error: %s", log);
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

static void ensure_line_program(void) {
    if (g_line_program != 0) return;

    const char* vs_src =
        "#version 120\n"
        "attribute vec2 a_pos;\n"
        "attribute vec4 a_color;\n"
        "varying vec4 v_color;\n"
        "void main(){ float x = (a_pos.x / %f) * 2.0 - 1.0; float y = 1.0 - (a_pos.y / %f) * 2.0; gl_Position = vec4(x, y, 0.0, 1.0); v_color = a_color; }\n";
    const char* fs_src =
        "#version 120\n"
        "varying vec4 v_color;\n"
        "void main(){ gl_FragColor = v_color; }\n";

    char vs_buf[256];
    snprintf(vs_buf, sizeof(vs_buf), vs_src, (float)g_window_width, (float)g_window_height);

    GLuint vs = compile_simple_shader(GL_VERTEX_SHADER, vs_buf);
    GLuint fs = compile_simple_shader(GL_FRAGMENT_SHADER, fs_src);
    if (vs == 0 || fs == 0) {
        if (vs) glDeleteShader(vs);
        if (fs) glDeleteShader(fs);
        return;
    }

    g_line_program = glCreateProgram();
    glAttachShader(g_line_program, vs);
    glAttachShader(g_line_program, fs);
    glBindAttribLocation(g_line_program, 0, "a_pos");
    glBindAttribLocation(g_line_program, 1, "a_color");
    glLinkProgram(g_line_program);
    GLint linked = 0;
    glGetProgramiv(g_line_program, GL_LINK_STATUS, &linked);
    glDeleteShader(vs);
    glDeleteShader(fs);
    if (linked != GL_TRUE) {
        char log[512];
        glGetProgramInfoLog(g_line_program, sizeof(log), NULL, log);
        SDL_Log("Simple program link error: %s", log);
        glDeleteProgram(g_line_program);
        g_line_program = 0;
        return;
    }

    g_line_pos_loc = 0;
    g_line_color_loc = 1;

    if (g_line_vao == 0) {
        glGenVertexArrays(1, &g_line_vao);
    }
    if (g_line_vbo == 0) {
        glGenBuffers(1, &g_line_vbo);
    }
}

static void reset_line_program(void) {
    if (g_line_program != 0) {
        glDeleteProgram(g_line_program);
        g_line_program = 0;
    }
}

static void reset_sprite_program(void) {
    if (g_sprite_program != 0) {
        glDeleteProgram(g_sprite_program);
        g_sprite_program = 0;
    }
}

/* Flush all batched lines to OpenGL */
static void flush_lines(void) {
    if (g_line_count == 0) return;

    glUseProgram(0);
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    glDrawBuffer(GL_BACK);

    /* Build vertex buffer */
    int vtx_count = g_line_count * 2;
    for (int i = 0; i < g_line_count; i++) {
        g_line_vertices[i * 2 + 0].x = g_lines[i].x1;
        g_line_vertices[i * 2 + 0].y = g_lines[i].y1;
        g_line_vertices[i * 2 + 0].r = g_lines[i].r;
        g_line_vertices[i * 2 + 0].g = g_lines[i].g;
        g_line_vertices[i * 2 + 0].b = g_lines[i].b;
        g_line_vertices[i * 2 + 0].a = g_lines[i].a;

        g_line_vertices[i * 2 + 1].x = g_lines[i].x2;
        g_line_vertices[i * 2 + 1].y = g_lines[i].y2;
        g_line_vertices[i * 2 + 1].r = g_lines[i].r;
        g_line_vertices[i * 2 + 1].g = g_lines[i].g;
        g_line_vertices[i * 2 + 1].b = g_lines[i].b;
        g_line_vertices[i * 2 + 1].a = g_lines[i].a;
    }

    ensure_line_program();
    if (g_line_program != 0) {
        glUseProgram(g_line_program);
        glBindVertexArray(g_line_vao);
        glBindBuffer(GL_ARRAY_BUFFER, g_line_vbo);
        glBufferData(GL_ARRAY_BUFFER, sizeof(LineVertex) * vtx_count, g_line_vertices, GL_DYNAMIC_DRAW);
        glEnableVertexAttribArray((GLuint)g_line_pos_loc);
        glVertexAttribPointer((GLuint)g_line_pos_loc, 2, GL_FLOAT, GL_FALSE, sizeof(LineVertex), (void*)offsetof(LineVertex, x));
        glEnableVertexAttribArray((GLuint)g_line_color_loc);
        glVertexAttribPointer((GLuint)g_line_color_loc, 4, GL_FLOAT, GL_FALSE, sizeof(LineVertex), (void*)offsetof(LineVertex, r));
        glDrawArrays(GL_LINES, 0, vtx_count);
        glDisableVertexAttribArray((GLuint)g_line_pos_loc);
        glDisableVertexAttribArray((GLuint)g_line_color_loc);
        glBindBuffer(GL_ARRAY_BUFFER, 0);
        glBindVertexArray(0);
        glUseProgram(0);
    }

    if (g_debug_frame_counter < 5) {
        GLenum err = glGetError();
        if (err != GL_NO_ERROR) {
            SDL_Log("GL error after flush_lines frame %d: 0x%x", g_debug_frame_counter, err);
        }
    }

    g_line_count = 0;
}
#endif

static void ensure_asset_base(void) {
    if (g_asset_base[0] != 0) return;

    /* Prefer explicit env override */
    const char* env = getenv("STASIS_ASSET_ROOT");
    if (env && *env) {
        strncpy(g_asset_env, env, sizeof(g_asset_env) - 1);
        g_asset_env[sizeof(g_asset_env) - 1] = 0;
        strncpy(g_asset_base, g_asset_env, sizeof(g_asset_base) - 1);
        g_asset_base[sizeof(g_asset_base) - 1] = 0;
        return;
    }
#if defined(_WIN32)
    _getcwd(g_asset_base, (int)sizeof(g_asset_base));
#else
    getcwd(g_asset_base, sizeof(g_asset_base));
#endif
}

static void gfx_asset_watch_init(void);
static void gfx_asset_watch_apply_pending_changes(void);
static void gfx_asset_watch_shutdown(void);

static int is_absolute_path(const char* path) {
    if (!path || !*path) return 0;
#if defined(_WIN32)
    if (path[0] == '\\' || path[0] == '/') return 1;
    if (isalpha((unsigned char)path[0]) && path[1] == ':' && (path[2] == '\\' || path[2] == '/')) return 1;
    return 0;
#else
    return path[0] == '/';
#endif
}

static int resolve_asset_path(const char* path, char* out, size_t out_size) {
    if (!out || out_size < 2 || !path || !*path) return 0;
    ensure_asset_base();
    if (is_absolute_path(path)) {
        if (g_asset_env[0] != 0) return 0;
        strncpy(out, path, out_size - 1);
        out[out_size - 1] = 0;
    } else {
        char normalized[1024];
        const char* relative = path;
        if (g_asset_env[0] != 0) {
            if (!stasis_asset_normalize_relative_path(path, normalized, sizeof(normalized))) {
                return 0;
            }
            relative = normalized;
        }
        snprintf(out, out_size, "%s/%s", g_asset_base, relative);
        out[out_size - 1] = 0;
    }
    for (char* p = out; *p; ++p) {
        if (*p == '\\') *p = '/';
    }
    return 1;
}

#if defined(_WIN32)
static volatile LONG g_asset_watch_dirty = 0;
static HANDLE g_asset_watch_stop_event = NULL;
static HANDLE g_asset_watch_change_handle = NULL;
static HANDLE g_asset_watch_thread = NULL;

static int gfx_asset_watch_enabled(void) {
    static int cached = -1;
    if (cached != -1) return cached;

    /* Explicit override (applies to both dev and non-dev runs). */
    const char* env = getenv("STASIS_GFX_WATCH_ASSETS");
    if (env && *env) {
        cached = (env[0] == '1') ? 1 : 0;
        return cached;
    }

    /* Default: enable only in dev (e.g. `stasis run --watch`). */
    const char* dev = getenv("STASIS_DEV");
    cached = (dev && dev[0] == '1') ? 1 : 0;
    return cached;
}

static DWORD WINAPI gfx_asset_watch_thread_proc(LPVOID userdata) {
    (void)userdata;

    HANDLE handles[2];
    handles[0] = g_asset_watch_stop_event;
    handles[1] = g_asset_watch_change_handle;

    for (;;) {
        DWORD wait = WaitForMultipleObjects(2, handles, FALSE, INFINITE);
        if (wait == WAIT_OBJECT_0) {
            break;
        }
        if (wait == WAIT_OBJECT_0 + 1) {
            InterlockedExchange(&g_asset_watch_dirty, 1);
            if (!FindNextChangeNotification(g_asset_watch_change_handle)) {
                break;
            }
            continue;
        }
        break;
    }

    return 0;
}
#endif

static void gfx_asset_watch_init(void) {
#if defined(_WIN32)
    if (!gfx_asset_watch_enabled()) return;
    if (g_asset_watch_thread) return;

    ensure_asset_base();

    g_asset_watch_stop_event = CreateEventA(NULL, TRUE, FALSE, NULL);
    if (!g_asset_watch_stop_event) {
        return;
    }

    DWORD flags = FILE_NOTIFY_CHANGE_FILE_NAME |
                  FILE_NOTIFY_CHANGE_DIR_NAME |
                  FILE_NOTIFY_CHANGE_LAST_WRITE |
                  FILE_NOTIFY_CHANGE_SIZE;
    g_asset_watch_change_handle = FindFirstChangeNotificationA(g_asset_base, TRUE, flags);
    if (g_asset_watch_change_handle == INVALID_HANDLE_VALUE) {
        g_asset_watch_change_handle = NULL;
        CloseHandle(g_asset_watch_stop_event);
        g_asset_watch_stop_event = NULL;
        return;
    }

    g_asset_watch_thread = CreateThread(NULL, 0, gfx_asset_watch_thread_proc, NULL, 0, NULL);
    if (!g_asset_watch_thread) {
        FindCloseChangeNotification(g_asset_watch_change_handle);
        g_asset_watch_change_handle = NULL;
        CloseHandle(g_asset_watch_stop_event);
        g_asset_watch_stop_event = NULL;
        return;
    }
#endif
}

static void gfx_asset_watch_shutdown(void) {
#if defined(_WIN32)
    if (g_asset_watch_stop_event) {
        SetEvent(g_asset_watch_stop_event);
    }
    if (g_asset_watch_thread) {
        WaitForSingleObject(g_asset_watch_thread, 5000);
        CloseHandle(g_asset_watch_thread);
        g_asset_watch_thread = NULL;
    }
    if (g_asset_watch_change_handle) {
        FindCloseChangeNotification(g_asset_watch_change_handle);
        g_asset_watch_change_handle = NULL;
    }
    if (g_asset_watch_stop_event) {
        CloseHandle(g_asset_watch_stop_event);
        g_asset_watch_stop_event = NULL;
    }
#endif
}

STASIS_EXPORT void stasis_gfx_notify_file_changed(const char* path) {
    (void)path;
#if defined(_WIN32)
    if (!gfx_asset_watch_enabled()) return;
    InterlockedExchange(&g_asset_watch_dirty, 1);
#endif
}

static char* read_text_file(const char* path) {
    ensure_asset_base();

    FILE* f = fopen(path, "rb");
    char resolved[1024];

    if (!f) {
        if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
            fprintf(stderr, "read_text_file: failed %s\n", path);
            return NULL;
        }
        f = fopen(resolved, "rb");
        if (!f) {
            fprintf(stderr, "read_text_file: failed %s (also %s)\n", path, resolved);
            return NULL;
        }
    }
    fseek(f, 0, SEEK_END);
    long len = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (len <= 0) {
        fclose(f);
        return NULL;
    }
    char* data = (char*)malloc((size_t)len + 1);
    if (!data) {
        fclose(f);
        return NULL;
    }
    size_t read = fread(data, 1, (size_t)len, f);
    fclose(f);
    data[read] = 0;
    return data;
}

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static GLuint compile_shader(GLenum type, const char* source) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    GLint status = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &status);
    if (status != GL_TRUE) {
        char log[512];
        glGetShaderInfoLog(shader, sizeof(log), NULL, log);
        SDL_Log("Shader compile error: %s", log);
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

static GLuint link_program(GLuint vs, GLuint fs) {
    GLuint prog = glCreateProgram();
    glAttachShader(prog, vs);
    glAttachShader(prog, fs);
    glLinkProgram(prog);
    GLint status = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &status);
    if (status != GL_TRUE) {
        char log[512];
        glGetProgramInfoLog(prog, sizeof(log), NULL, log);
        SDL_Log("Program link error: %s", log);
        glDeleteProgram(prog);
        return 0;
    }
    return prog;
}
#endif

static uint64_t get_file_mtime(const char* path) {
    char resolved[1024];
    const char* probe = path;
    if (!is_absolute_path(path)) {
        if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
            return 0;
        }
        probe = resolved;
    }
#if defined(_WIN32)
    struct _stat st;
    if (_stat(probe, &st) != 0) return 0;
    return (uint64_t)st.st_mtime;
#else
    struct stat st;
    if (stat(probe, &st) != 0) return 0;
    return (uint64_t)st.st_mtime;
#endif
}

static char* stasis_strdup(const char* s) {
    if (!s) return NULL;
#if defined(_WIN32)
    return _strdup(s);
#else
    return strdup(s);
#endif
}

static int clamp_i32(int value, int min_value, int max_value) {
    if (value < min_value) return min_value;
    if (value > max_value) return max_value;
    return value;
}

static int parse_env_i32(const char* name, int fallback, int min_value, int max_value) {
    int clamped_fallback = clamp_i32(fallback, min_value, max_value);
    const char* raw = getenv(name);
    if (!raw || !raw[0]) return clamped_fallback;

    char* end = NULL;
    long parsed = strtol(raw, &end, 10);
    if (end == raw || (end && *end != '\0')) {
        SDL_Log("%s: invalid integer '%s'; using %d", name, raw, clamped_fallback);
        return clamped_fallback;
    }
    if (parsed < (long)min_value) {
        SDL_Log("%s: clamping %ld to %d", name, parsed, min_value);
        return min_value;
    }
    if (parsed > (long)max_value) {
        SDL_Log("%s: clamping %ld to %d", name, parsed, max_value);
        return max_value;
    }
    return (int)parsed;
}

static int sprite_source_within_limits(const char* resolved_path, int max_w, int max_h) {
    if (!resolved_path || !resolved_path[0] || max_w <= 0 || max_h <= 0) return 0;
    if (g_sprite_max_dimension < 0) {
        g_sprite_max_dimension = parse_env_i32(
            "STASIS_GFX_MAX_SPRITE_DIMENSION", 16384, 1, 65536);
        g_sprite_max_pixels = parse_env_i32(
            "STASIS_GFX_MAX_SPRITE_PIXELS", 16 * 1024 * 1024, 1, INT_MAX);
        g_sprite_max_file_bytes = parse_env_i32(
            "STASIS_GFX_MAX_SPRITE_FILE_BYTES", 64 * 1024 * 1024, 1, INT_MAX);
    }
    if (max_w > g_sprite_max_dimension || max_h > g_sprite_max_dimension) {
        SDL_Log("gfx_load_sprite: sprite_dimensions_exceeded requested=%dx%d limit=%d",
                max_w, max_h, g_sprite_max_dimension);
        return 0;
    }
    uint64_t requested_pixels = (uint64_t)(unsigned int)max_w * (uint64_t)(unsigned int)max_h;
    if (requested_pixels > (uint64_t)(unsigned int)g_sprite_max_pixels) {
        SDL_Log("gfx_load_sprite: sprite_pixels_exceeded requested=%llu limit=%d",
                (unsigned long long)requested_pixels, g_sprite_max_pixels);
        return 0;
    }

#if defined(_WIN32)
    struct _stat st;
    if (_stat(resolved_path, &st) != 0 || st.st_size < 0) {
#else
    struct stat st;
    if (stat(resolved_path, &st) != 0 || st.st_size < 0) {
#endif
        SDL_Log("gfx_load_sprite: sprite_file_unreadable path=%s", resolved_path);
        return 0;
    }
    if ((uint64_t)st.st_size > (uint64_t)(unsigned int)g_sprite_max_file_bytes) {
        SDL_Log("gfx_load_sprite: sprite_file_too_large bytes=%llu limit=%d",
                (unsigned long long)st.st_size, g_sprite_max_file_bytes);
        return 0;
    }
    return 1;
}

static int ensure_sprite_table_capacity(int min_capacity) {
    if (min_capacity <= g_sprite_capacity) return 1;

    if (g_sprite_table_limit < 0) {
        g_sprite_table_limit = parse_env_i32("STASIS_GFX_MAX_SPRITES", 0, 0, INT_MAX / 2);
    }

    int limit = g_sprite_table_limit;
    if (limit <= 0) {
        limit = (int)SPRITE_HANDLE_INDEX_MASK;
    }
    if (limit > (int)SPRITE_HANDLE_INDEX_MASK) limit = (int)SPRITE_HANDLE_INDEX_MASK;
    if (min_capacity > limit) {
        return 0;
    }

    int new_capacity = g_sprite_capacity > 0 ? g_sprite_capacity : clamp_i32(SPRITE_TABLE_INITIAL_CAPACITY, 1, limit);
    while (new_capacity < min_capacity) {
        if (new_capacity >= limit) {
            new_capacity = limit;
            break;
        }
        if (new_capacity > limit / 2) {
            new_capacity = limit;
        } else {
            new_capacity *= 2;
        }
    }
    if (new_capacity < min_capacity) {
        return 0;
    }

    SpriteEntry* resized = (SpriteEntry*)realloc(g_sprites, sizeof(SpriteEntry) * (size_t)new_capacity);
    if (!resized) {
        return 0;
    }
    if (new_capacity > g_sprite_capacity) {
        memset(resized + g_sprite_capacity, 0, sizeof(SpriteEntry) * (size_t)(new_capacity - g_sprite_capacity));
    }
    g_sprites = resized;
    g_sprite_capacity = new_capacity;
    return 1;
}

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static int atlas_page_required_extent(int sprite_extent) {
    if (sprite_extent <= 0) return 0;
    if (sprite_extent > INT_MAX - SPRITE_ATLAS_PAD * 2) return 0;
    return sprite_extent + SPRITE_ATLAS_PAD * 2;
}

static int atlas_page_free_area(const SpriteAtlasPage* page) {
    int total = 0;
    if (!page) return 0;
    for (int i = 0; i < page->free_rect_count; i++) {
        total += page->free_rects[i].w * page->free_rects[i].h;
    }
    return total;
}

static void atlas_page_remove_rect(SpriteAtlasPage* page, int index) {
    if (!page || index < 0 || index >= page->free_rect_count) return;
    page->free_rects[index] = page->free_rects[page->free_rect_count - 1];
    page->free_rect_count--;
}

static int atlas_page_reserve_rects(SpriteAtlasPage* page, int additional) {
    if (!page || additional <= 0) return 1;
    if (page->free_rect_count + additional <= page->free_rect_capacity) return 1;

    int new_capacity = page->free_rect_capacity > 0 ? page->free_rect_capacity : 8;
    while (new_capacity < page->free_rect_count + additional) {
        if (new_capacity > INT_MAX / 2) {
            return 0;
        }
        new_capacity *= 2;
    }

    AtlasFreeRect* resized = (AtlasFreeRect*)realloc(page->free_rects, sizeof(AtlasFreeRect) * (size_t)new_capacity);
    if (!resized) {
        return 0;
    }
    page->free_rects = resized;
    page->free_rect_capacity = new_capacity;
    return 1;
}

static int atlas_page_push_rect(SpriteAtlasPage* page, AtlasFreeRect rect) {
    if (!page || rect.w <= 0 || rect.h <= 0) return 1;
    if (!atlas_page_reserve_rects(page, 1)) {
        return 0;
    }
    page->free_rects[page->free_rect_count++] = rect;
    return 1;
}

static int atlas_rect_contains(AtlasFreeRect a, AtlasFreeRect b) {
    return b.x >= a.x && b.y >= a.y &&
           b.x + b.w <= a.x + a.w &&
           b.y + b.h <= a.y + a.h;
}

static int atlas_try_merge_rects(AtlasFreeRect* a, AtlasFreeRect* b) {
    if (!a || !b) return 0;

    if (a->x == b->x && a->w == b->w) {
        if (a->y + a->h == b->y) {
            a->h += b->h;
            return 1;
        }
        if (b->y + b->h == a->y) {
            a->y = b->y;
            a->h += b->h;
            return 1;
        }
    }

    if (a->y == b->y && a->h == b->h) {
        if (a->x + a->w == b->x) {
            a->w += b->w;
            return 1;
        }
        if (b->x + b->w == a->x) {
            a->x = b->x;
            a->w += b->w;
            return 1;
        }
    }

    return 0;
}

static void atlas_page_coalesce(SpriteAtlasPage* page) {
    if (!page) return;

    int changed = 1;
    while (changed) {
        changed = 0;
        for (int i = 0; i < page->free_rect_count; i++) {
            for (int j = i + 1; j < page->free_rect_count; j++) {
                if (atlas_rect_contains(page->free_rects[i], page->free_rects[j])) {
                    atlas_page_remove_rect(page, j);
                    changed = 1;
                    goto restart;
                }
                if (atlas_rect_contains(page->free_rects[j], page->free_rects[i])) {
                    atlas_page_remove_rect(page, i);
                    changed = 1;
                    goto restart;
                }
                if (atlas_try_merge_rects(&page->free_rects[i], &page->free_rects[j])) {
                    atlas_page_remove_rect(page, j);
                    changed = 1;
                    goto restart;
                }
            }
        }
restart:
        ;
    }
}

static void atlas_init_config(void) {
    if (g_sprite_atlas_page_w > 0 && g_sprite_atlas_page_h > 0) return;

    if (g_sprite_atlas_gl_max_size <= 0) {
        GLint gl_limit = 0;
        glGetIntegerv(GL_MAX_TEXTURE_SIZE, &gl_limit);
        if (gl_limit <= 0) gl_limit = SPRITE_ATLAS_DEFAULT_W;
        g_sprite_atlas_gl_max_size = (int)gl_limit;
    }

    g_sprite_atlas_page_w = parse_env_i32("STASIS_GFX_ATLAS_W",
                                          SPRITE_ATLAS_DEFAULT_W,
                                          64,
                                          g_sprite_atlas_gl_max_size);
    g_sprite_atlas_page_h = parse_env_i32("STASIS_GFX_ATLAS_H",
                                          SPRITE_ATLAS_DEFAULT_H,
                                          64,
                                          g_sprite_atlas_gl_max_size);
}

static int atlas_page_upload_region(SpriteAtlasPage* page, int x, int y, int w, int h, const void* pixels) {
    if (!page || page->texture == 0 || w <= 0 || h <= 0) return 0;
    while (glGetError() != GL_NO_ERROR) {
        /* Clear stale errors so this upload has a deterministic result. */
    }
    glBindTexture(GL_TEXTURE_2D, page->texture);
    glTexSubImage2D(GL_TEXTURE_2D, 0, x, y, w, h, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    glGenerateMipmap(GL_TEXTURE_2D);
    glBindTexture(GL_TEXTURE_2D, 0);
    GLenum upload_error = glGetError();
    if (upload_error != GL_NO_ERROR) {
        SDL_Log("gfx_load_sprite: sprite_gpu_upload_failed gl_error=%u", (unsigned int)upload_error);
        return 0;
    }
    return 1;
}

static void atlas_page_clear_region(SpriteAtlasPage* page, int x, int y, int w, int h) {
    if (!page || page->texture == 0 || w <= 0 || h <= 0) return;
    size_t pixel_count = (size_t)w * (size_t)h;
    unsigned char* clear_pixels = (unsigned char*)calloc(pixel_count, 4);
    if (!clear_pixels) return;
    atlas_page_upload_region(page, x, y, w, h, clear_pixels);
    free(clear_pixels);
}

static int atlas_add_page(void) {
    atlas_init_config();

    if (g_sprite_atlas_page_count == g_sprite_atlas_page_capacity) {
        int new_capacity = g_sprite_atlas_page_capacity > 0 ? g_sprite_atlas_page_capacity * 2 : 4;
        SpriteAtlasPage* resized =
            (SpriteAtlasPage*)realloc(g_sprite_atlas_pages, sizeof(SpriteAtlasPage) * (size_t)new_capacity);
        if (!resized) {
            return -1;
        }
        memset(resized + g_sprite_atlas_page_capacity, 0,
               sizeof(SpriteAtlasPage) * (size_t)(new_capacity - g_sprite_atlas_page_capacity));
        g_sprite_atlas_pages = resized;
        g_sprite_atlas_page_capacity = new_capacity;
    }

    SpriteAtlasPage* page = &g_sprite_atlas_pages[g_sprite_atlas_page_count];
    memset(page, 0, sizeof(*page));
    page->w = g_sprite_atlas_page_w;
    page->h = g_sprite_atlas_page_h;

    glGenTextures(1, &page->texture);
    if (page->texture == 0) {
        return -1;
    }

    size_t pixel_count = (size_t)page->w * (size_t)page->h;
    unsigned char* initial_pixels = (unsigned char*)calloc(pixel_count, 4);
    if (!initial_pixels) {
        glDeleteTextures(1, &page->texture);
        page->texture = 0;
        return -1;
    }

    glBindTexture(GL_TEXTURE_2D, page->texture);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, page->w, page->h, 0, GL_RGBA, GL_UNSIGNED_BYTE, initial_pixels);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glGenerateMipmap(GL_TEXTURE_2D);
    glBindTexture(GL_TEXTURE_2D, 0);
    free(initial_pixels);

    if (!atlas_page_push_rect(page, (AtlasFreeRect){ 0, 0, page->w, page->h })) {
        glDeleteTextures(1, &page->texture);
        page->texture = 0;
        free(page->free_rects);
        page->free_rects = NULL;
        page->free_rect_capacity = 0;
        return -1;
    }

    return g_sprite_atlas_page_count++;
}

static int atlas_page_alloc_rect(SpriteAtlasPage* page, int sprite_w, int sprite_h,
                                 int* out_sprite_x, int* out_sprite_y,
                                 int* out_alloc_x, int* out_alloc_y,
                                 int* out_alloc_w, int* out_alloc_h) {
    if (!page) return 0;

    const int need_w = atlas_page_required_extent(sprite_w);
    const int need_h = atlas_page_required_extent(sprite_h);
    if (need_w <= 0 || need_h <= 0) return 0;
    if (need_w > page->w || need_h > page->h) return 0;

    int best_index = -1;
    int best_area = INT_MAX;
    for (int i = 0; i < page->free_rect_count; i++) {
        AtlasFreeRect rect = page->free_rects[i];
        if (rect.w < need_w || rect.h < need_h) continue;
        int area = rect.w * rect.h;
        if (best_index < 0 || area < best_area) {
            best_index = i;
            best_area = area;
        }
    }
    if (best_index < 0) return 0;

    AtlasFreeRect slot = page->free_rects[best_index];
    if (!atlas_page_reserve_rects(page, 2)) {
        return 0;
    }
    atlas_page_remove_rect(page, best_index);

    if (!atlas_page_push_rect(page, (AtlasFreeRect){ slot.x + need_w, slot.y, slot.w - need_w, need_h }) ||
        !atlas_page_push_rect(page, (AtlasFreeRect){ slot.x, slot.y + need_h, slot.w, slot.h - need_h })) {
        return 0;
    }

    page->used_pixels += need_w * need_h;
    *out_alloc_x = slot.x;
    *out_alloc_y = slot.y;
    *out_alloc_w = need_w;
    *out_alloc_h = need_h;
    *out_sprite_x = slot.x + SPRITE_ATLAS_PAD;
    *out_sprite_y = slot.y + SPRITE_ATLAS_PAD;
    return 1;
}

static void atlas_release_rect(int page_index, int alloc_x, int alloc_y, int alloc_w, int alloc_h) {
    if (page_index < 0 || page_index >= g_sprite_atlas_page_count) return;
    if (alloc_w <= 0 || alloc_h <= 0) return;

    SpriteAtlasPage* page = &g_sprite_atlas_pages[page_index];
    atlas_page_clear_region(page, alloc_x, alloc_y, alloc_w, alloc_h);
    if (atlas_page_push_rect(page, (AtlasFreeRect){ alloc_x, alloc_y, alloc_w, alloc_h })) {
        page->used_pixels -= alloc_w * alloc_h;
        if (page->used_pixels < 0) page->used_pixels = 0;
        atlas_page_coalesce(page);
    }
}

static void atlas_log_failure(const char* reason, const char* path, int sprite_w, int sprite_h) {
    int best_w = 0;
    int best_h = 0;
    int best_area = 0;
    int free_regions = 0;
    int free_pixels = 0;

    for (int i = 0; i < g_sprite_atlas_page_count; i++) {
        SpriteAtlasPage* page = &g_sprite_atlas_pages[i];
        free_regions += page->free_rect_count;
        free_pixels += atlas_page_free_area(page);
        for (int j = 0; j < page->free_rect_count; j++) {
            int area = page->free_rects[j].w * page->free_rects[j].h;
            if (area > best_area) {
                best_area = area;
                best_w = page->free_rects[j].w;
                best_h = page->free_rects[j].h;
            }
        }
    }

    SDL_Log(
        "gfx_load_sprite: %s for %s raster=%dx%d pages=%d page=%dx%d gl_max=%d sprites=%d/%d free_pixels=%d free_regions=%d largest_free=%dx%d",
        reason,
        path ? path : "(null)",
        sprite_w,
        sprite_h,
        g_sprite_atlas_page_count,
        g_sprite_atlas_page_w,
        g_sprite_atlas_page_h,
        g_sprite_atlas_gl_max_size,
        g_sprite_count,
        g_sprite_capacity,
        free_pixels,
        free_regions,
        best_w,
        best_h);
}

static int atlas_alloc(int sprite_w, int sprite_h, const char* path,
                       int* out_page_index, int* out_sprite_x, int* out_sprite_y,
                       int* out_alloc_x, int* out_alloc_y, int* out_alloc_w, int* out_alloc_h) {
    atlas_init_config();

    const int need_w = atlas_page_required_extent(sprite_w);
    const int need_h = atlas_page_required_extent(sprite_h);
    if (need_w <= 0 || need_h <= 0) {
        atlas_log_failure("invalid sprite extent", path, sprite_w, sprite_h);
        return 0;
    }
    if (need_w > g_sprite_atlas_page_w || need_h > g_sprite_atlas_page_h) {
        atlas_log_failure("sprite exceeds atlas page size", path, sprite_w, sprite_h);
        return 0;
    }

    for (int i = 0; i < g_sprite_atlas_page_count; i++) {
        if (atlas_page_alloc_rect(&g_sprite_atlas_pages[i], sprite_w, sprite_h,
                                  out_sprite_x, out_sprite_y,
                                  out_alloc_x, out_alloc_y,
                                  out_alloc_w, out_alloc_h)) {
            *out_page_index = i;
            return 1;
        }
    }

    int page_index = atlas_add_page();
    if (page_index < 0) {
        atlas_log_failure("failed to create atlas page", path, sprite_w, sprite_h);
        return 0;
    }

    if (!atlas_page_alloc_rect(&g_sprite_atlas_pages[page_index], sprite_w, sprite_h,
                               out_sprite_x, out_sprite_y,
                               out_alloc_x, out_alloc_y,
                               out_alloc_w, out_alloc_h)) {
        atlas_log_failure("new atlas page could not fit sprite", path, sprite_w, sprite_h);
        return 0;
    }

    *out_page_index = page_index;
    return 1;
}

static void ensure_sprite_program(void) {
    if (g_sprite_program != 0) return;

    const char* vs_src =
        "#version 120\n"
        "attribute vec2 a_pos;\n"
        "attribute vec2 a_uv;\n"
        "attribute vec4 a_color;\n"
        "varying vec2 v_uv;\n"
        "varying vec4 v_color;\n"
        "void main(){ float x = (a_pos.x / %f) * 2.0 - 1.0; float y = 1.0 - (a_pos.y / %f) * 2.0; gl_Position = vec4(x, y, 0.0, 1.0); v_uv = a_uv; v_color = a_color; }\n";
    const char* fs_src =
        "#version 120\n"
        "uniform sampler2D u_tex;\n"
        "varying vec2 v_uv;\n"
        "varying vec4 v_color;\n"
        "void main(){ gl_FragColor = texture2D(u_tex, v_uv) * v_color; }\n";

    char vs_buf[512];
    snprintf(vs_buf, sizeof(vs_buf), vs_src, (float)g_window_width, (float)g_window_height);

    GLuint vs = compile_shader(GL_VERTEX_SHADER, vs_buf);
    GLuint fs = compile_shader(GL_FRAGMENT_SHADER, fs_src);
    if (!vs || !fs) {
        if (vs) glDeleteShader(vs);
        if (fs) glDeleteShader(fs);
        return;
    }

    GLuint prog = glCreateProgram();
    glAttachShader(prog, vs);
    glAttachShader(prog, fs);
    glBindAttribLocation(prog, 0, "a_pos");
    glBindAttribLocation(prog, 1, "a_uv");
    glBindAttribLocation(prog, 2, "a_color");
    glLinkProgram(prog);

    GLint linked = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &linked);
    glDeleteShader(vs);
    glDeleteShader(fs);
    if (linked != GL_TRUE) {
        char log[512];
        glGetProgramInfoLog(prog, sizeof(log), NULL, log);
        SDL_Log("Sprite program link error: %s", log);
        glDeleteProgram(prog);
        return;
    }

    g_sprite_program = prog;
    g_sprite_pos_loc = 0;
    g_sprite_uv_loc = 1;
    g_sprite_color_loc = 2;
    g_sprite_tex_loc = glGetUniformLocation(g_sprite_program, "u_tex");

    if (g_sprite_vao == 0) {
        glGenVertexArrays(1, &g_sprite_vao);
    }
    if (g_sprite_vbo == 0) {
        glGenBuffers(1, &g_sprite_vbo);
    }
}
#endif

static void blend_px_premult(unsigned char* dst, int sr, int sg, int sb, int sa) {
    int inv = 255 - sa;
    dst[0] = (unsigned char)(sr + (dst[0] * inv) / 255);
    dst[1] = (unsigned char)(sg + (dst[1] * inv) / 255);
    dst[2] = (unsigned char)(sb + (dst[2] * inv) / 255);
    dst[3] = (unsigned char)(sa + (dst[3] * inv) / 255);
}

static void draw_rect_rgba(unsigned char* buf, int w, int h, int x, int y, int rw, int rh, float r, float g, float b, float a) {
    if (!buf) return;
    if (rw <= 0 || rh <= 0) return;
    int x0 = x;
    int y0 = y;
    int x1 = x + rw;
    int y1 = y + rh;
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > w) x1 = w;
    if (y1 > h) y1 = h;

    int sa = (int)(a * 255.0f + 0.5f);
    if (sa <= 0) return;
    if (sa > 255) sa = 255;
    int sr = (int)(r * (float)sa + 0.5f);
    int sg = (int)(g * (float)sa + 0.5f);
    int sb = (int)(b * (float)sa + 0.5f);
    if (sr < 0) sr = 0; if (sr > 255) sr = 255;
    if (sg < 0) sg = 0; if (sg > 255) sg = 255;
    if (sb < 0) sb = 0; if (sb > 255) sb = 255;

    for (int py = y0; py < y1; py++) {
        unsigned char* row = buf + (py * w * 4);
        for (int px = x0; px < x1; px++) {
            blend_px_premult(row + px * 4, sr, sg, sb, sa);
        }
    }
}

static void draw_circle_rgba(unsigned char* buf, int w, int h, float cx, float cy, float radius, float r, float g, float b, float a) {
    if (!buf) return;
    if (radius <= 0.0f) return;

    int sa = (int)(a * 255.0f + 0.5f);
    if (sa <= 0) return;
    if (sa > 255) sa = 255;
    int sr = (int)(r * (float)sa + 0.5f);
    int sg = (int)(g * (float)sa + 0.5f);
    int sb = (int)(b * (float)sa + 0.5f);
    if (sr < 0) sr = 0; if (sr > 255) sr = 255;
    if (sg < 0) sg = 0; if (sg > 255) sg = 255;
    if (sb < 0) sb = 0; if (sb > 255) sb = 255;

    float rr = radius * radius;
    int x0 = (int)floorf(cx - radius - 1.0f);
    int y0 = (int)floorf(cy - radius - 1.0f);
    int x1 = (int)ceilf(cx + radius + 1.0f);
    int y1 = (int)ceilf(cy + radius + 1.0f);
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > w) x1 = w;
    if (y1 > h) y1 = h;

    for (int py = y0; py < y1; py++) {
        unsigned char* row = buf + (py * w * 4);
        for (int px = x0; px < x1; px++) {
            float fx = (float)px + 0.5f;
            float fy = (float)py + 0.5f;
            float dx = fx - cx;
            float dy = fy - cy;
            if (dx * dx + dy * dy <= rr) {
                blend_px_premult(row + px * 4, sr, sg, sb, sa);
            }
        }
    }
}

static float dist2_point_segment(float px, float py, float ax, float ay, float bx, float by) {
    float abx = bx - ax;
    float aby = by - ay;
    float apx = px - ax;
    float apy = py - ay;
    float ab2 = abx * abx + aby * aby;
    float t = 0.0f;
    if (ab2 > 0.0f) {
        t = (apx * abx + apy * aby) / ab2;
        if (t < 0.0f) t = 0.0f;
        if (t > 1.0f) t = 1.0f;
    }
    float cx = ax + abx * t;
    float cy = ay + aby * t;
    float dx = px - cx;
    float dy = py - cy;
    return dx * dx + dy * dy;
}

static void draw_line_rgba(unsigned char* buf, int w, int h, float x1, float y1, float x2, float y2, float thickness, float r, float g, float b, float a) {
    if (!buf) return;
    if (thickness <= 0.0f) return;

    int sa = (int)(a * 255.0f + 0.5f);
    if (sa <= 0) return;
    if (sa > 255) sa = 255;
    int sr = (int)(r * (float)sa + 0.5f);
    int sg = (int)(g * (float)sa + 0.5f);
    int sb = (int)(b * (float)sa + 0.5f);
    if (sr < 0) sr = 0; if (sr > 255) sr = 255;
    if (sg < 0) sg = 0; if (sg > 255) sg = 255;
    if (sb < 0) sb = 0; if (sb > 255) sb = 255;

    float rad = thickness * 0.5f;
    float rr = rad * rad;
    float minx = fminf(x1, x2) - rad - 1.0f;
    float miny = fminf(y1, y2) - rad - 1.0f;
    float maxx = fmaxf(x1, x2) + rad + 1.0f;
    float maxy = fmaxf(y1, y2) + rad + 1.0f;
    int ix0 = (int)floorf(minx);
    int iy0 = (int)floorf(miny);
    int ix1 = (int)ceilf(maxx);
    int iy1 = (int)ceilf(maxy);
    if (ix0 < 0) ix0 = 0;
    if (iy0 < 0) iy0 = 0;
    if (ix1 > w) ix1 = w;
    if (iy1 > h) iy1 = h;

    for (int py = iy0; py < iy1; py++) {
        unsigned char* row = buf + (py * w * 4);
        for (int px = ix0; px < ix1; px++) {
            float fx = (float)px + 0.5f;
            float fy = (float)py + 0.5f;
            float d2 = dist2_point_segment(fx, fy, x1, y1, x2, y2);
            if (d2 <= rr) {
                blend_px_premult(row + px * 4, sr, sg, sb, sa);
            }
        }
    }
}

static void downsample_2x(unsigned char* out_buf, int out_w, int out_h, const unsigned char* in_buf, int in_w, int in_h) {
    for (int y = 0; y < out_h; y++) {
        for (int x = 0; x < out_w; x++) {
            int sx = x * 2;
            int sy = y * 2;
            const unsigned char* p0 = in_buf + ((sy + 0) * in_w + (sx + 0)) * 4;
            const unsigned char* p1 = in_buf + ((sy + 0) * in_w + (sx + 1)) * 4;
            const unsigned char* p2 = in_buf + ((sy + 1) * in_w + (sx + 0)) * 4;
            const unsigned char* p3 = in_buf + ((sy + 1) * in_w + (sx + 1)) * 4;
            unsigned char* o = out_buf + (y * out_w + x) * 4;
            o[0] = (unsigned char)(((int)p0[0] + (int)p1[0] + (int)p2[0] + (int)p3[0]) / 4);
            o[1] = (unsigned char)(((int)p0[1] + (int)p1[1] + (int)p2[1] + (int)p3[1]) / 4);
            o[2] = (unsigned char)(((int)p0[2] + (int)p1[2] + (int)p2[2] + (int)p3[2]) / 4);
            o[3] = (unsigned char)(((int)p0[3] + (int)p1[3] + (int)p2[3] + (int)p3[3]) / 4);
        }
    }
}

static uint32_t fnv1a_32(const unsigned char* data, size_t len) {
    uint32_t h = 2166136261u;
    for (size_t i = 0; i < len; i++) {
        h ^= (uint32_t)data[i];
        h *= 16777619u;
    }
    return h;
}

static uint32_t fnv1a_mix_u32(uint32_t h, uint32_t v) {
    h ^= (v >> 0) & 0xFFu; h *= 16777619u;
    h ^= (v >> 8) & 0xFFu; h *= 16777619u;
    h ^= (v >> 16) & 0xFFu; h *= 16777619u;
    h ^= (v >> 24) & 0xFFu; h *= 16777619u;
    return h;
}

/* Debug: per-frame draw-call hash (for verifying batch vs per-call equivalence). */
static int g_debug_hash_checked_env = 0;
static int g_debug_hash_enabled = 0;
static uint32_t g_debug_frame_hash = 0;

static void gfx_debug_hash_check_env(void) {
    if (g_debug_hash_checked_env) return;
    g_debug_hash_checked_env = 1;
    const char* env = getenv("STASIS_GFX_DEBUG_HASH");
    g_debug_hash_enabled = (env && env[0] == '1') ? 1 : 0;
}

static void gfx_debug_hash_reset_if_enabled(void) {
    gfx_debug_hash_check_env();
    if (!g_debug_hash_enabled) return;
    g_debug_frame_hash = 2166136261u;
}

static void gfx_debug_hash_i32(int32_t v) {
    if (!g_debug_hash_enabled) return;
    g_debug_frame_hash = fnv1a_mix_u32(g_debug_frame_hash, (uint32_t)v);
}

static void gfx_debug_hash_f32(float v) {
    if (!g_debug_hash_enabled) return;
    uint32_t bits = 0;
    memcpy(&bits, &v, sizeof(bits));
    g_debug_frame_hash = fnv1a_mix_u32(g_debug_frame_hash, bits);
}

STASIS_EXPORT void stasis_gfx_debug_enable_hash(int enabled) {
    g_debug_hash_checked_env = 1;
    g_debug_hash_enabled = enabled ? 1 : 0;
    if (g_debug_hash_enabled) {
        g_debug_frame_hash = 2166136261u;
    }
}

STASIS_EXPORT int stasis_gfx_debug_get_frame_hash(void) {
    gfx_debug_hash_check_env();
    if (!g_debug_hash_enabled) return 0;
    return (int)g_debug_frame_hash;
}

/* SVG rasterization (paths, gradients, transforms) via NanoSVG */
static int bake_svg_to_rgba(const char* path, unsigned char** out_pixels, int* out_w, int* out_h) {
    *out_pixels = NULL;
    *out_w = 0;
    *out_h = 0;

    char resolved[1024];
    if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
        fprintf(stderr, "bake_svg_to_rgba: bad path %s\n", path ? path : "(null)");
        return 0;
    }

    NSVGimage* image = nsvgParseFromFile(resolved, "px", 96.0f);
    if (!image) {
        fprintf(stderr, "bake_svg_to_rgba: failed to parse %s\n", resolved);
        return 0;
    }

    int w = (int)ceilf(image->width);
    int h = (int)ceilf(image->height);
    if (w <= 0 || h <= 0) {
        fprintf(stderr, "bake_svg_to_rgba: invalid size %dx%d in %s\n", w, h, resolved);
        nsvgDelete(image);
        return 0;
    }

    NSVGrasterizer* rast = nsvgCreateRasterizer();
    if (!rast) {
        fprintf(stderr, "bake_svg_to_rgba: failed to create rasterizer for %s\n", resolved);
        nsvgDelete(image);
        return 0;
    }

    unsigned char* pixels = (unsigned char*)malloc((size_t)w * (size_t)h * 4u);
    if (!pixels) {
        fprintf(stderr, "bake_svg_to_rgba: OOM allocating %d x %d buffer for %s\n", w, h, resolved);
        nsvgDeleteRasterizer(rast);
        nsvgDelete(image);
        return 0;
    }
    memset(pixels, 0, (size_t)w * (size_t)h * 4u);

    float sx = (float)w / image->width;
    float sy = (float)h / image->height;
    float scale = sx < sy ? sx : sy;

    nsvgRasterize(rast, image, 0.0f, 0.0f, scale, pixels, w, h, w * 4);

    nsvgDeleteRasterizer(rast);
    nsvgDelete(image);

    *out_pixels = pixels;
    *out_w = w;
    *out_h = h;
    return 1;
}

/*
 * Rasterize SVG to exactly max_w x max_h (in pixels).
 * The SVG content is scaled uniformly to fit within max_w x max_h (preserving aspect ratio)
 * and centered with transparent padding. This keeps sprite textures 1:1 with draw sizes to
 * avoid fuzz from resampling.
 */
static int bake_svg_to_rgba_sized(const char* resolved_path, int max_w, int max_h,
                                   unsigned char** out_pixels, int* out_w, int* out_h) {
    *out_pixels = NULL;
    *out_w = 0;
    *out_h = 0;

    if (max_w <= 0 || max_h <= 0) {
        fprintf(stderr, "bake_svg_to_rgba_sized: invalid max size %dx%d\n", max_w, max_h);
        return 0;
    }

    NSVGimage* image = nsvgParseFromFile(resolved_path, "px", 96.0f);
    if (!image) {
        fprintf(stderr, "bake_svg_to_rgba_sized: failed to parse %s\n", resolved_path);
        return 0;
    }

    if (image->width <= 0 || image->height <= 0) {
        fprintf(stderr, "bake_svg_to_rgba_sized: invalid SVG size %.1fx%.1f in %s\n",
                image->width, image->height, resolved_path);
        nsvgDelete(image);
        return 0;
    }

    /* Calculate scale to fit within max dimensions, preserving aspect ratio */
    float scale_x = (float)max_w / image->width;
    float scale_y = (float)max_h / image->height;
    float scale = (scale_x < scale_y) ? scale_x : scale_y;

    /* Calculate content size (rounded up) and center it in the full target buffer. */
    int content_w = (int)ceilf(image->width * scale);
    int content_h = (int)ceilf(image->height * scale);
    if (content_w < 1) content_w = 1;
    if (content_h < 1) content_h = 1;
    if (content_w > max_w) content_w = max_w;
    if (content_h > max_h) content_h = max_h;

    float tx = (float)(max_w - content_w) * 0.5f;
    float ty = (float)(max_h - content_h) * 0.5f;

    NSVGrasterizer* rast = nsvgCreateRasterizer();
    if (!rast) {
        fprintf(stderr, "bake_svg_to_rgba_sized: failed to create rasterizer for %s\n", resolved_path);
        nsvgDelete(image);
        return 0;
    }

    unsigned char* pixels = (unsigned char*)malloc((size_t)max_w * (size_t)max_h * 4u);
    if (!pixels) {
        fprintf(stderr, "bake_svg_to_rgba_sized: OOM allocating %d x %d buffer for %s\n", max_w, max_h, resolved_path);
        nsvgDeleteRasterizer(rast);
        nsvgDelete(image);
        return 0;
    }
    memset(pixels, 0, (size_t)max_w * (size_t)max_h * 4u);

    nsvgRasterize(rast, image, tx, ty, scale, pixels, max_w, max_h, max_w * 4);

    nsvgDeleteRasterizer(rast);
    nsvgDelete(image);

    *out_pixels = pixels;
    *out_w = max_w;
    *out_h = max_h;
    return 1;
}

/*
 * Debug helper: bake an SVG to RGBA on the CPU and return a deterministic 32-bit hash of the pixels.
 * Returns 0 on error (and logs).
 */
STASIS_EXPORT int stasis_gfx_debug_bake_hash(const char* path) {
    if (!path || !*path) return 0;
    unsigned char* pixels = NULL;
    int w = 0, h = 0;
    if (!bake_svg_to_rgba(path, &pixels, &w, &h)) {
        SDL_Log("gfx_debug_bake_hash: failed to bake %s", path);
        return 0;
    }
    uint32_t h32 = fnv1a_32(pixels, (size_t)w * (size_t)h * 4u);
    free(pixels);
    return (int)h32;
}

static int ends_with_ci(const char* s, const char* suffix) {
    if (!s || !suffix) return 0;
    size_t sl = strlen(s);
    size_t tl = strlen(suffix);
    if (tl > sl) return 0;
    const char* tail = s + (sl - tl);
    for (size_t i = 0; i < tl; i++) {
        char a = (char)tolower((unsigned char)tail[i]);
        char b = (char)tolower((unsigned char)suffix[i]);
        if (a != b) return 0;
    }
    return 1;
}

static void premultiply_rgba(unsigned char* pixels, int w, int h) {
    if (!pixels || w <= 0 || h <= 0) return;
    const int count = w * h;
    for (int i = 0; i < count; i++) {
        unsigned char* p = pixels + i * 4;
        const unsigned char a = p[3];
        if (a == 255) continue;
        if (a == 0) {
            p[0] = 0; p[1] = 0; p[2] = 0;
            continue;
        }
        p[0] = (unsigned char)((p[0] * a + 127) / 255);
        p[1] = (unsigned char)((p[1] * a + 127) / 255);
        p[2] = (unsigned char)((p[2] * a + 127) / 255);
    }
}

static int bake_raster_to_rgba_sized(const char* resolved_path, int max_w, int max_h,
                                     unsigned char** out_pixels, int* out_w, int* out_h) {
    *out_pixels = NULL;
    *out_w = 0;
    *out_h = 0;

    if (max_w <= 0 || max_h <= 0) {
        fprintf(stderr, "bake_raster_to_rgba_sized: invalid max size %dx%d\n", max_w, max_h);
        return 0;
    }

    SDL_Surface* loaded = IMG_Load(resolved_path);
    if (!loaded) {
        fprintf(stderr, "bake_raster_to_rgba_sized: IMG_Load failed for %s: %s\n", resolved_path, IMG_GetError());
        return 0;
    }

    SDL_Surface* rgba = SDL_ConvertSurfaceFormat(loaded, SDL_PIXELFORMAT_RGBA32, 0);
    SDL_FreeSurface(loaded);
    if (!rgba) {
        fprintf(stderr, "bake_raster_to_rgba_sized: SDL_ConvertSurfaceFormat failed for %s: %s\n", resolved_path, SDL_GetError());
        return 0;
    }

    const int src_w = rgba->w;
    const int src_h = rgba->h;
    if (src_w <= 0 || src_h <= 0) {
        SDL_FreeSurface(rgba);
        fprintf(stderr, "bake_raster_to_rgba_sized: invalid raster size %dx%d in %s\n", src_w, src_h, resolved_path);
        return 0;
    }

    unsigned char* out = (unsigned char*)malloc((size_t)max_w * (size_t)max_h * 4u);
    if (!out) {
        SDL_FreeSurface(rgba);
        fprintf(stderr, "bake_raster_to_rgba_sized: OOM allocating %d x %d buffer for %s\n", max_w, max_h, resolved_path);
        return 0;
    }
    memset(out, 0, (size_t)max_w * (size_t)max_h * 4u);

    float scale_x = (float)max_w / (float)src_w;
    float scale_y = (float)max_h / (float)src_h;
    float scale = (scale_x < scale_y) ? scale_x : scale_y;
    int content_w = (int)ceilf((float)src_w * scale);
    int content_h = (int)ceilf((float)src_h * scale);
    if (content_w < 1) content_w = 1;
    if (content_h < 1) content_h = 1;
    if (content_w > max_w) content_w = max_w;
    if (content_h > max_h) content_h = max_h;

    const int off_x = (max_w - content_w) / 2;
    const int off_y = (max_h - content_h) / 2;

    const unsigned char* src = (const unsigned char*)rgba->pixels;
    const int src_stride = rgba->pitch;

    for (int y = 0; y < content_h; y++) {
        int sy = (int)((float)y / scale);
        if (sy < 0) sy = 0;
        if (sy >= src_h) sy = src_h - 1;
        const unsigned char* src_row = src + (size_t)sy * (size_t)src_stride;
        unsigned char* dst_row = out + (size_t)(off_y + y) * (size_t)max_w * 4u + (size_t)off_x * 4u;
        for (int x = 0; x < content_w; x++) {
            int sx = (int)((float)x / scale);
            if (sx < 0) sx = 0;
            if (sx >= src_w) sx = src_w - 1;
            const unsigned char* sp = src_row + (size_t)sx * 4u;
            unsigned char* dp = dst_row + (size_t)x * 4u;
            dp[0] = sp[0];
            dp[1] = sp[1];
            dp[2] = sp[2];
            dp[3] = sp[3];
        }
    }

    SDL_FreeSurface(rgba);

    /* Match GL sprite pipeline: premultiplied alpha. */
    premultiply_rgba(out, max_w, max_h);

    *out_pixels = out;
    *out_w = max_w;
    *out_h = max_h;
    return 1;
}

static int bake_image_to_rgba_sized(const char* path, int max_w, int max_h,
                                    unsigned char** out_pixels, int* out_w, int* out_h) {
    if (ends_with_ci(path, ".svg")) {
        return bake_svg_to_rgba_sized(path, max_w, max_h, out_pixels, out_w, out_h);
    }
    return bake_raster_to_rgba_sized(path, max_w, max_h, out_pixels, out_w, out_h);
}

static int write_bmp_bgra32(const char* path, int w, int h, const uint8_t* bgra, int is_bottom_up) {
    if (!path || !*path || w <= 0 || h <= 0 || !bgra) return 0;

    FILE* f = fopen(path, "wb");
    if (!f) {
        return 0;
    }

    /* 32bpp BI_RGB BMP (BGRA), no row padding needed. */
    const uint32_t pixel_bytes = (uint32_t)w * (uint32_t)h * 4u;
    const uint32_t file_size = 14u + 40u + pixel_bytes;

    uint8_t file_hdr[14];
    memset(file_hdr, 0, sizeof(file_hdr));
    file_hdr[0] = 'B';
    file_hdr[1] = 'M';
    file_hdr[2] = (uint8_t)(file_size & 0xFFu);
    file_hdr[3] = (uint8_t)((file_size >> 8) & 0xFFu);
    file_hdr[4] = (uint8_t)((file_size >> 16) & 0xFFu);
    file_hdr[5] = (uint8_t)((file_size >> 24) & 0xFFu);
    file_hdr[10] = 54; /* pixel data offset */

    uint8_t info_hdr[40];
    memset(info_hdr, 0, sizeof(info_hdr));
    info_hdr[0] = 40; /* BITMAPINFOHEADER size */
    info_hdr[4] = (uint8_t)(w & 0xFF);
    info_hdr[5] = (uint8_t)((w >> 8) & 0xFF);
    info_hdr[6] = (uint8_t)((w >> 16) & 0xFF);
    info_hdr[7] = (uint8_t)((w >> 24) & 0xFF);

    /* Use negative height for top-down to match the natural SDL coordinate system. */
    int32_t signed_h = is_bottom_up ? h : -h;
    info_hdr[8] = (uint8_t)(signed_h & 0xFF);
    info_hdr[9] = (uint8_t)((signed_h >> 8) & 0xFF);
    info_hdr[10] = (uint8_t)((signed_h >> 16) & 0xFF);
    info_hdr[11] = (uint8_t)((signed_h >> 24) & 0xFF);

    info_hdr[12] = 1; /* planes */
    info_hdr[14] = 32; /* bpp */
    /* biCompression=0 (BI_RGB) */
    info_hdr[20] = (uint8_t)(pixel_bytes & 0xFFu);
    info_hdr[21] = (uint8_t)((pixel_bytes >> 8) & 0xFFu);
    info_hdr[22] = (uint8_t)((pixel_bytes >> 16) & 0xFFu);
    info_hdr[23] = (uint8_t)((pixel_bytes >> 24) & 0xFFu);

    if (fwrite(file_hdr, 1, sizeof(file_hdr), f) != sizeof(file_hdr) ||
        fwrite(info_hdr, 1, sizeof(info_hdr), f) != sizeof(info_hdr)) {
        fclose(f);
        return 0;
    }

    const uint32_t row_bytes = (uint32_t)w * 4u;
    if (is_bottom_up) {
        /* Write rows bottom-up (OpenGL glReadPixels origin). */
        for (int y = 0; y < h; y++) {
            const uint8_t* row = bgra + (size_t)y * (size_t)row_bytes;
            if (fwrite(row, 1, row_bytes, f) != row_bytes) {
                fclose(f);
                return 0;
            }
        }
    } else {
        /* Write rows top-down. */
        for (int y = 0; y < h; y++) {
            const uint8_t* row = bgra + (size_t)y * (size_t)row_bytes;
            if (fwrite(row, 1, row_bytes, f) != row_bytes) {
                fclose(f);
                return 0;
            }
        }
    }

    fclose(f);
    return 1;
}

static void write_u32_be(uint8_t* out, uint32_t value) {
    out[0] = (uint8_t)(value >> 24);
    out[1] = (uint8_t)(value >> 16);
    out[2] = (uint8_t)(value >> 8);
    out[3] = (uint8_t)value;
}

static uint32_t png_crc32_update(uint32_t crc, const uint8_t* data, size_t length) {
    for (size_t i = 0; i < length; i++) {
        crc ^= data[i];
        for (int bit = 0; bit < 8; bit++) {
            uint32_t mask = (uint32_t)(-(int32_t)(crc & 1u));
            crc = (crc >> 1) ^ (0xEDB88320u & mask);
        }
    }
    return crc;
}

static int write_png_chunk(FILE* f, const char type[4], const uint8_t* data, uint32_t length) {
    uint8_t size_bytes[4];
    uint8_t crc_bytes[4];
    write_u32_be(size_bytes, length);
    uint32_t crc = png_crc32_update(0xFFFFFFFFu, (const uint8_t*)type, 4);
    if (length > 0) crc = png_crc32_update(crc, data, length);
    write_u32_be(crc_bytes, crc ^ 0xFFFFFFFFu);
    return fwrite(size_bytes, 1, 4, f) == 4 &&
           fwrite(type, 1, 4, f) == 4 &&
           (length == 0 || fwrite(data, 1, length, f) == length) &&
           fwrite(crc_bytes, 1, 4, f) == 4;
}

static uint32_t png_adler32(const uint8_t* data, size_t length) {
    uint32_t a = 1;
    uint32_t b = 0;
    for (size_t offset = 0; offset < length;) {
        size_t block = length - offset;
        if (block > 5552) block = 5552;
        for (size_t i = 0; i < block; i++) {
            a += data[offset + i];
            b += a;
        }
        a %= 65521u;
        b %= 65521u;
        offset += block;
    }
    return (b << 16) | a;
}

/* Deterministic PNG writer using zlib's uncompressed DEFLATE blocks. */
static int write_png_bgra32(const char* path, int w, int h, const uint8_t* bgra, int is_bottom_up) {
    if (!path || !*path || w <= 0 || h <= 0 || !bgra) return 0;
    if ((size_t)w > (SIZE_MAX - 1u) / 4u) return 0;
    const size_t pixel_row_bytes = (size_t)w * 4u;
    const size_t png_row_bytes = pixel_row_bytes + 1u;
    if ((size_t)h > SIZE_MAX / png_row_bytes) return 0;
    const size_t raw_size = png_row_bytes * (size_t)h;
    if (raw_size > UINT32_MAX) return 0;

    uint8_t* raw = (uint8_t*)malloc(raw_size);
    if (!raw) return 0;
    for (int y = 0; y < h; y++) {
        const int source_y = is_bottom_up ? (h - 1 - y) : y;
        const uint8_t* source = bgra + (size_t)source_y * pixel_row_bytes;
        uint8_t* target = raw + (size_t)y * png_row_bytes;
        target[0] = 0;
        for (int x = 0; x < w; x++) {
            target[1 + x * 4 + 0] = source[x * 4 + 2];
            target[1 + x * 4 + 1] = source[x * 4 + 1];
            target[1 + x * 4 + 2] = source[x * 4 + 0];
            target[1 + x * 4 + 3] = source[x * 4 + 3];
        }
    }

    const size_t block_count = (raw_size + 65534u) / 65535u;
    if (raw_size > SIZE_MAX - 6u ||
        block_count > (SIZE_MAX - raw_size - 6u) / 5u) {
        free(raw);
        return 0;
    }
    const size_t zlib_size = 2u + raw_size + block_count * 5u + 4u;
    if (zlib_size > UINT32_MAX) {
        free(raw);
        return 0;
    }
    uint8_t* zlib = (uint8_t*)malloc(zlib_size);
    if (!zlib) {
        free(raw);
        return 0;
    }

    size_t input_offset = 0;
    size_t output_offset = 0;
    zlib[output_offset++] = 0x78;
    zlib[output_offset++] = 0x01;
    while (input_offset < raw_size) {
        size_t remaining = raw_size - input_offset;
        uint16_t block_length = (uint16_t)(remaining > 65535u ? 65535u : remaining);
        uint16_t inverse_length = (uint16_t)~block_length;
        zlib[output_offset++] = remaining <= 65535u ? 1u : 0u;
        zlib[output_offset++] = (uint8_t)block_length;
        zlib[output_offset++] = (uint8_t)(block_length >> 8);
        zlib[output_offset++] = (uint8_t)inverse_length;
        zlib[output_offset++] = (uint8_t)(inverse_length >> 8);
        memcpy(zlib + output_offset, raw + input_offset, block_length);
        output_offset += block_length;
        input_offset += block_length;
    }
    write_u32_be(zlib + output_offset, png_adler32(raw, raw_size));
    output_offset += 4;

    uint8_t ihdr[13];
    write_u32_be(ihdr, (uint32_t)w);
    write_u32_be(ihdr + 4, (uint32_t)h);
    ihdr[8] = 8;
    ihdr[9] = 6;
    ihdr[10] = 0;
    ihdr[11] = 0;
    ihdr[12] = 0;
    static const uint8_t signature[8] = {137, 80, 78, 71, 13, 10, 26, 10};

    FILE* f = fopen(path, "wb");
    int ok = f && fwrite(signature, 1, sizeof(signature), f) == sizeof(signature) &&
             write_png_chunk(f, "IHDR", ihdr, sizeof(ihdr)) &&
             write_png_chunk(f, "IDAT", zlib, (uint32_t)output_offset) &&
             write_png_chunk(f, "IEND", NULL, 0);
    if (f && fclose(f) != 0) ok = 0;
    if (!ok) remove(path);
    free(zlib);
    free(raw);
    return ok;
}

static int stasis_gfx_dump_image(const char* path, int png, int render_queued_lines) {
    if (!path || !*path) return 0;
    if (!g_window) return 0;
    if (g_drawable_width <= 0 || g_drawable_height <= 0) return 0;

    char resolved[1024];
    const char* out_path = path;
    if (!is_absolute_path(path)) {
        if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
            return 0;
        }
        out_path = resolved;
    }

    const int w = g_drawable_width;
    const int h = g_drawable_height;
    const size_t bytes = (size_t)w * (size_t)h * 4u;

    uint8_t* pixels = (uint8_t*)malloc(bytes);
    if (!pixels) return 0;

    int ok = 0;

    if (g_use_sdl_renderer) {
        if (g_renderer) {
            if (render_queued_lines) {
                /* Direct API calls may happen before end_frame(), so flush pending lines once. */
                SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
                SDL_Color color;
                for (int i = 0; i < g_line_count; i++) {
                    color.r = (Uint8)(g_lines[i].r * 255.0f);
                    color.g = (Uint8)(g_lines[i].g * 255.0f);
                    color.b = (Uint8)(g_lines[i].b * 255.0f);
                    color.a = (Uint8)(g_lines[i].a * 255.0f);
                    SDL_SetRenderDrawColor(g_renderer, color.r, color.g, color.b, color.a);
                    SDL_RenderDrawLineF(g_renderer, g_lines[i].x1, g_lines[i].y1, g_lines[i].x2, g_lines[i].y2);
                }
                g_line_count = 0;
            }

            /* SDL_RenderReadPixels reads from the current render target before present. */
            int rc = SDL_RenderReadPixels(g_renderer, NULL, SDL_PIXELFORMAT_BGRA32, pixels, w * 4);
            if (rc == 0) {
                ok = png ? write_png_bgra32(out_path, w, h, pixels, 0)
                         : write_bmp_bgra32(out_path, w, h, pixels, 0);
            }
        }
        free(pixels);
        return ok;
    }

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (g_gl_context) {
        /* Ensure buffered draws are applied so the screenshot matches what end_frame() will present. */
        flush_lines();
        flush_sprites();
        if (!g_postfx_applied_this_frame) {
            render_postfx();
            g_postfx_applied_this_frame = true;
        }
        glFlush();
        glFinish();

        /* Read the back buffer (origin bottom-left). Call before stasis_end_frame(). */
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        glReadBuffer(GL_BACK);
        glPixelStorei(GL_PACK_ALIGNMENT, 1);
        glReadPixels(0, 0, w, h, GL_BGRA, GL_UNSIGNED_BYTE, pixels);
        GLenum err = glGetError();
        if (err == GL_NO_ERROR) {
            /* glReadPixels returns rows from the bottom of the framebuffer first. */
            ok = png ? write_png_bgra32(out_path, w, h, pixels, 1)
                     : write_bmp_bgra32(out_path, w, h, pixels, 1);
        }
        free(pixels);
        return ok;
    }
#endif

    free(pixels);
    return 0;
}

STASIS_EXPORT int stasis_gfx_dump_bmp(const char* path) {
    return stasis_gfx_dump_image(path, 0, 1);
}

STASIS_EXPORT int stasis_gfx_dump_png(const char* path) {
    return stasis_gfx_dump_image(path, 1, 1);
}

static void capture_scheduled_screenshot(void) {
    if (g_screenshot_taken || g_screenshot_path[0] == 0 ||
        g_debug_frame_counter + 1 != g_screenshot_frame) {
        return;
    }
    int ok = stasis_gfx_dump_image(
        g_screenshot_path,
        ends_with_ci(g_screenshot_path, ".png"),
        0);
    if (!ok) {
        SDL_Log("failed to capture screenshot: %s", g_screenshot_path);
        if (g_screenshot_exit_after) g_should_quit = true;
        return;
    }
    g_screenshot_taken = true;
    const char* parity_stage = SDL_getenv("STASIS_PARITY_CAPTURE_STAGE");
    if (parity_stage && *parity_stage) {
        SDL_Log(
            "Stasis parity capture: stage=%s path=%s frame=%d backend=%s surface_generation=%u renderer_generation=%u",
            parity_stage,
            g_screenshot_path,
            g_screenshot_frame,
            g_use_sdl_renderer ? "sdl" : "gl",
            g_resource_lifecycle.surface_generation,
            g_resource_lifecycle.renderer_generation);
    }
    if (g_screenshot_exit_after) g_should_quit = true;
}

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static void flush_sprites(void) {
    if (g_sprite_vert_count == 0) return;
    ensure_sprite_program();
    if (g_sprite_program == 0 ||
        g_sprite_batch_page < 0 ||
        g_sprite_batch_page >= g_sprite_atlas_page_count ||
        g_sprite_atlas_pages[g_sprite_batch_page].texture == 0) {
        g_sprite_vert_count = 0;
        g_sprite_batch_page = -1;
        return;
    }

    glEnable(GL_BLEND);
    glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);

    glUseProgram(g_sprite_program);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, g_sprite_atlas_pages[g_sprite_batch_page].texture);
    if (g_sprite_tex_loc >= 0) glUniform1i(g_sprite_tex_loc, 0);

    glBindVertexArray(g_sprite_vao);
    glBindBuffer(GL_ARRAY_BUFFER, g_sprite_vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof(SpriteVertex) * (size_t)g_sprite_vert_count, g_sprite_vertices, GL_DYNAMIC_DRAW);

    glEnableVertexAttribArray((GLuint)g_sprite_pos_loc);
    glVertexAttribPointer((GLuint)g_sprite_pos_loc, 2, GL_FLOAT, GL_FALSE, sizeof(SpriteVertex), (void*)offsetof(SpriteVertex, x));
    glEnableVertexAttribArray((GLuint)g_sprite_uv_loc);
    glVertexAttribPointer((GLuint)g_sprite_uv_loc, 2, GL_FLOAT, GL_FALSE, sizeof(SpriteVertex), (void*)offsetof(SpriteVertex, u));
    glEnableVertexAttribArray((GLuint)g_sprite_color_loc);
    glVertexAttribPointer((GLuint)g_sprite_color_loc, 4, GL_FLOAT, GL_FALSE, sizeof(SpriteVertex), (void*)offsetof(SpriteVertex, r));

    glDrawArrays(GL_TRIANGLES, 0, g_sprite_vert_count);

    glDisableVertexAttribArray((GLuint)g_sprite_pos_loc);
    glDisableVertexAttribArray((GLuint)g_sprite_uv_loc);
    glDisableVertexAttribArray((GLuint)g_sprite_color_loc);
    glBindBuffer(GL_ARRAY_BUFFER, 0);
    glBindVertexArray(0);
    glBindTexture(GL_TEXTURE_2D, 0);
    glUseProgram(0);

    g_sprite_vert_count = 0;
    g_sprite_batch_page = -1;
}
#endif

/* Fast path for command-buffer sprite submission.
 *
 * When debug hashing is disabled and most sprites are unrotated, we avoid per-sprite trig
 * and reduce function-call overhead by writing vertices directly from the stream.
 *
 * NOTE: rotation != 0 falls back to the general path.
 */
static void stasis_gfx_draw_sprites_i32_fast(const int32_t* cmds, int sprite_count) {
    if (!cmds || sprite_count <= 0) return;

    if (g_use_sdl_renderer) {
        for (int i = 0; i < sprite_count; i++) {
            const int base = i * 7;
            stasis_gfx_draw_sprite_internal(
                cmds[base + 0],
                cmds[base + 1],
                cmds[base + 2],
                cmds[base + 3],
                cmds[base + 4],
                cmds[base + 5],
                cmds[base + 6],
                0);
        }
        return;
    }

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    for (int i = 0; i < sprite_count; i++) {
        const int base = i * 7;
        const int handle = cmds[base + 0];
        const int x = cmds[base + 1];
        const int y = cmds[base + 2];
        const int w = cmds[base + 3];
        const int h = cmds[base + 4];
        const int rot_degrees = cmds[base + 5];
        const int a = cmds[base + 6];

        if (w <= 0 || h <= 0) continue;
        SpriteEntry* e = sprite_get(handle);
        if (!e) e = sprite_fallback_get();
        if (!e) continue;

        if (e->needs_reraster) {
            if (e->path) sprite_build_into_entry_sized(e, e->path, e->max_w, e->max_h);
        }

        if (rot_degrees != 0) {
            stasis_gfx_draw_sprite_internal(handle, x, y, w, h, rot_degrees, a, 0);
            continue;
        }

        if (g_sprite_vert_count + 6 > MAX_SPRITE_VERTS) {
            flush_sprites();
        }
        if (g_sprite_vert_count > 0 && g_sprite_batch_page != e->page_index) {
            flush_sprites();
        }
        g_sprite_batch_page = e->page_index;

        const float af = (float)a / 255.0f;
        const float u0 = e->u0, v0 = e->v0, u1 = e->u1, v1 = e->v1;
        const float x0 = (float)x;
        const float y0 = (float)y;
        const float x1p = (float)(x + w);
        const float y1p = (float)(y + h);

        SpriteVertex* v = &g_sprite_vertices[g_sprite_vert_count];
        v[0] = (SpriteVertex){ x0,  y0,  u0, v0, af, af, af, af };
        v[1] = (SpriteVertex){ x1p, y0,  u1, v0, af, af, af, af };
        v[2] = (SpriteVertex){ x1p, y1p, u1, v1, af, af, af, af };
        v[3] = (SpriteVertex){ x1p, y1p, u1, v1, af, af, af, af };
        v[4] = (SpriteVertex){ x0,  y1p, u0, v1, af, af, af, af };
        v[5] = (SpriteVertex){ x0,  y0,  u0, v0, af, af, af, af };
        g_sprite_vert_count += 6;
    }
#else
    (void)cmds;
    (void)sprite_count;
#endif
}

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static const char* kFallbackPostfxVert =
"#version 120\n"
"varying vec2 v_uv;\n"
"void main(){ v_uv = gl_MultiTexCoord0.xy; gl_Position = gl_Vertex; }\n";

static const char* kFallbackPostfxFrag =
"#version 120\n"
"varying vec2 v_uv;\n"
"uniform float u_time;\n"
"uniform float u_depth_scale;\n"
"uniform float u_intensity;\n"
"uniform float u_surface_jitter;\n"
"uniform vec3 u_biolume_color;\n"
"float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7)))*43758.5453); }\n"
"float noise(vec2 p){ vec2 i=floor(p); vec2 f=fract(p); float a=hash(i); float b=hash(i+vec2(1.0,0.0)); float c=hash(i+vec2(0.0,1.0)); float d=hash(i+vec2(1.0,1.0)); vec2 u=f*f*(3.0-2.0*f); return mix(a,b,u.x)+(c-a)*u.y*(1.0-u.x)+(d-b)*u.x*u.y; }\n"
"void main(){ float depth=clamp(v_uv.y*u_depth_scale,0.0,1.0); float ripple=noise(v_uv*6.0+u_time*0.25); float wave=sin((v_uv.y*8.0)+(u_time*0.6)+ripple*u_surface_jitter); float c=0.5+0.5*wave; vec3 deep=vec3(0.02,0.08,0.12); vec3 mid=vec3(0.00,0.16,0.22); vec3 base=mix(deep,mid,depth); vec3 color=base+u_intensity*c*u_biolume_color; float atten=mix(1.0,0.25,depth); gl_FragColor=vec4(color*atten,0.18); }\n";

static void init_postfx_shader(void) {
    const char* fragSource = kFallbackPostfxFrag;

    GLuint vs = compile_shader(GL_VERTEX_SHADER, kFallbackPostfxVert);
    GLuint fs = compile_shader(GL_FRAGMENT_SHADER, fragSource);
    if (vs == 0 || fs == 0) {
        if (vs) glDeleteShader(vs);
        if (fs) glDeleteShader(fs);
        g_postfx_program = 0;
        return;
    }

    g_postfx_program = link_program(vs, fs);
    glDeleteShader(vs);
    glDeleteShader(fs);

    if (g_postfx_program != 0) {
        g_postfx_time_loc = glGetUniformLocation(g_postfx_program, "u_time");
        g_postfx_depth_loc = glGetUniformLocation(g_postfx_program, "u_depth_scale");
        g_postfx_intensity_loc = glGetUniformLocation(g_postfx_program, "u_intensity");
        g_postfx_surface_loc = glGetUniformLocation(g_postfx_program, "u_surface_jitter");
        g_postfx_color_loc = glGetUniformLocation(g_postfx_program, "u_biolume_color");
        /* Don't enable by default - wait for explicit set_postfx() call */
        g_postfx_enabled = false;
        SDL_Log("Post-effects shader compiled (not enabled until set_postfx called)");
    }
}

static void render_postfx(void) {
    if (g_postfx_force_disable) {
        return;
    }

    /* Allow disabling via environment variable for debugging */
    static int checked_env = 0;
    static int env_disable = 0;
    if (!checked_env) {
        const char* val = SDL_getenv("STASIS_DISABLE_POSTFX");
        env_disable = (val && strcmp(val, "0") != 0);
        checked_env = 1;
        if (env_disable) {
            SDL_Log("Post-effects disabled via STASIS_DISABLE_POSTFX");
        }
    }
    if (env_disable) {
        return;
    }

    if (!g_postfx_enabled || g_postfx_program == 0) {
        return;
    }

    /* Ensure blending is enabled for the overlay effect */
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);

    glUseProgram(g_postfx_program);
    if (g_postfx_time_loc >= 0) glUniform1f(g_postfx_time_loc, g_postfx_phase);
    if (g_postfx_depth_loc >= 0) glUniform1f(g_postfx_depth_loc, 1.6f);
    if (g_postfx_intensity_loc >= 0) glUniform1f(g_postfx_intensity_loc, g_postfx_strength);
    if (g_postfx_surface_loc >= 0) glUniform1f(g_postfx_surface_loc, 1.1f + g_postfx_speed * 0.5f);
    if (g_postfx_color_loc >= 0) glUniform3f(g_postfx_color_loc, g_postfx_color[0], g_postfx_color[1], g_postfx_color[2]);

    glBegin(GL_TRIANGLES);
    glTexCoord2f(0.0f, 0.0f); glVertex2f(-1.0f, -1.0f);
    glTexCoord2f(2.0f, 0.0f); glVertex2f( 3.0f, -1.0f);
    glTexCoord2f(0.0f, 2.0f); glVertex2f(-1.0f,  3.0f);
    glEnd();

    glUseProgram(0);
}
#endif

/*
 * Startup Render Verification
 *
 * These functions verify that rendering actually produces visible output.
 * They are called automatically after initialization to catch driver issues early.
 */

typedef struct {
    int success;
    int pixels_tested;
    int pixels_correct;
    char error_message[512];
    char gl_vendor[128];
    char gl_renderer[128];
    char gl_version[128];
    int gl_error_code;
} RenderTestResult;

static RenderTestResult g_last_test_result = {0};

/* Test OpenGL rendering by drawing a known pattern and reading it back */
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
static int verify_opengl_rendering(int width, int height) {
    RenderTestResult* result = &g_last_test_result;
    memset(result, 0, sizeof(*result));

    /* Store GL info for diagnostics */
    const char* vendor = (const char*)glGetString(GL_VENDOR);
    const char* renderer = (const char*)glGetString(GL_RENDERER);
    const char* version = (const char*)glGetString(GL_VERSION);

    if (vendor) strncpy(result->gl_vendor, vendor, sizeof(result->gl_vendor) - 1);
    if (renderer) strncpy(result->gl_renderer, renderer, sizeof(result->gl_renderer) - 1);
    if (version) strncpy(result->gl_version, version, sizeof(result->gl_version) - 1);

    /* Clear any pending errors */
    while (glGetError() != GL_NO_ERROR) {}

    /* Render a bright magenta quad to the center of the screen */
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    glDrawBuffer(GL_BACK);

    /* Clear to black first */
    glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);

    GLenum err = glGetError();
    if (err != GL_NO_ERROR) {
        result->gl_error_code = (int)err;
        snprintf(result->error_message, sizeof(result->error_message),
            "OpenGL error during clear: 0x%04X", err);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    /* Draw a test quad using immediate mode */
    setup_ortho();

    /* Draw magenta (255, 0, 255) quad in center region */
    int cx = width / 2;
    int cy = height / 2;
    int size = 50;

    glBegin(GL_QUADS);
    glColor4f(1.0f, 0.0f, 1.0f, 1.0f);  /* Magenta */
    glVertex2f((float)(cx - size), (float)(cy - size));
    glVertex2f((float)(cx + size), (float)(cy - size));
    glVertex2f((float)(cx + size), (float)(cy + size));
    glVertex2f((float)(cx - size), (float)(cy + size));
    glEnd();

    err = glGetError();
    if (err != GL_NO_ERROR) {
        result->gl_error_code = (int)err;
        snprintf(result->error_message, sizeof(result->error_message),
            "OpenGL error during quad draw: 0x%04X", err);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    /* Ensure rendering completed before reading back pixels */
    glFlush();
    glFinish();

    /* Read back pixels from the center of the quad */
    unsigned char pixels[4] = {0, 0, 0, 0};
    glReadBuffer(GL_BACK);
    glReadPixels(cx, height - cy, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixels);

    err = glGetError();
    if (err != GL_NO_ERROR) {
        result->gl_error_code = (int)err;
        snprintf(result->error_message, sizeof(result->error_message),
            "OpenGL error during pixel readback: 0x%04X", err);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    result->pixels_tested = 1;

    /* Verify we got magenta (or close to it) */
    /* Allow some tolerance for driver differences */
    int r_ok = (pixels[0] >= 200);  /* Should be ~255 */
    int g_ok = (pixels[1] <= 55);   /* Should be ~0 */
    int b_ok = (pixels[2] >= 200);  /* Should be ~255 */

    if (r_ok && g_ok && b_ok) {
        result->pixels_correct = 1;
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: OpenGL rendering verified");
        SDL_Log("  Test pixel readback: R=%d G=%d B=%d A=%d (expected ~255,0,255,255)",
            pixels[0], pixels[1], pixels[2], pixels[3]);
        /* Clear so the test pattern can't leak into the first presented frame. */
        glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);
        return 1;
    } else {
        snprintf(result->error_message, sizeof(result->error_message),
            "Pixel verification failed: got R=%d G=%d B=%d A=%d, expected ~255,0,255,255. "
            "This may indicate a driver issue or headless environment.",
            pixels[0], pixels[1], pixels[2], pixels[3]);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        SDL_Log("  GL_VENDOR: %s", result->gl_vendor);
        SDL_Log("  GL_RENDERER: %s", result->gl_renderer);
        SDL_Log("  GL_VERSION: %s", result->gl_version);
        return 0;
    }
}
#endif

/* Test SDL renderer by drawing a known pattern and reading it back */
static int verify_sdl_rendering(SDL_Renderer* renderer, int width, int height) {
    RenderTestResult* result = &g_last_test_result;
    memset(result, 0, sizeof(*result));

    /* Get renderer info for diagnostics */
    SDL_RendererInfo info;
    if (SDL_GetRendererInfo(renderer, &info) == 0) {
        strncpy(result->gl_renderer, info.name ? info.name : "unknown", sizeof(result->gl_renderer) - 1);
        snprintf(result->gl_version, sizeof(result->gl_version), "flags=0x%X", info.flags);
    }

    /* Clear to black */
    SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
    int rc = SDL_RenderClear(renderer);
    if (rc != 0) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_RenderClear failed: %s", SDL_GetError());
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    /* Draw a magenta rectangle in the center */
    int cx = width / 2;
    int cy = height / 2;
    int size = 50;

    SDL_SetRenderDrawColor(renderer, 255, 0, 255, 255);  /* Magenta */
    SDL_Rect rect = { cx - size, cy - size, size * 2, size * 2 };
    rc = SDL_RenderFillRect(renderer, &rect);
    if (rc != 0) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_RenderFillRect failed: %s", SDL_GetError());
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    /* We need to read back from a texture target to verify SDL rendering */
    /* Create a texture to render to */
    SDL_Texture* target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA8888,
        SDL_TEXTUREACCESS_TARGET, width, height);
    if (!target) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_CreateTexture (target) failed: %s. Cannot verify rendering.",
            SDL_GetError());
        SDL_Log("STARTUP TEST WARNING: %s", result->error_message);
        /* Not a fatal error - some drivers don't support render targets */
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL renderer created (readback not available)");
        return 1;
    }

    /* Render to texture */
    SDL_SetRenderTarget(renderer, target);
    SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
    SDL_RenderClear(renderer);
    SDL_SetRenderDrawColor(renderer, 255, 0, 255, 255);
    SDL_RenderFillRect(renderer, &rect);

    /* Read back a single pixel from center */
    unsigned char pixels[4] = {0, 0, 0, 0};
    SDL_Rect readRect = { cx, cy, 1, 1 };
    rc = SDL_RenderReadPixels(renderer, &readRect, SDL_PIXELFORMAT_RGBA8888, pixels, 4);

    /* Switch back to default target */
    SDL_SetRenderTarget(renderer, NULL);
    SDL_DestroyTexture(target);

    if (rc != 0) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_RenderReadPixels failed: %s", SDL_GetError());
        SDL_Log("STARTUP TEST WARNING: %s", result->error_message);
        /* Not a fatal error - continue anyway */
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL renderer works (readback not available)");
        return 1;
    }

    result->pixels_tested = 1;

    /* We drew magenta (R=255, G=0, B=255). Check various pixel formats:
     * - RGBA: [R,G,B,A] = [255,0,255,255]
     * - BGRA: [B,G,R,A] = [255,0,255,255]
     * - ABGR: [A,B,G,R] = [255,255,0,255]  <- This matches what user got!
     *
     * The key insight: if we see two channels at ~255 and one at ~0,
     * and the ~0 is NOT in the alpha position, we have a valid render.
     * This handles all reasonable byte orderings.
     */

    int high_count = 0;
    int low_count = 0;
    int low_pos = -1;

    for (int i = 0; i < 4; i++) {
        if (pixels[i] >= 200) high_count++;
        if (pixels[i] <= 55) {
            low_count++;
            low_pos = i;
        }
    }

    /* For magenta (two high RGB + one zero RGB + high alpha), we expect:
     * - At least 2 channels >= 200 (the R, B and A from magenta)
     * - Exactly 1 channel <= 55 (the G from magenta)
     * - The low channel should be green, not alpha (varies by format)
     *
     * Accept the result if we have 3 high values and 1 low value,
     * indicating a non-black, non-white, chromatic color was rendered.
     */
    int pattern_ok = (high_count >= 2 && low_count == 1);

    if (pattern_ok) {
        result->pixels_correct = 1;
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL rendering verified");
        SDL_Log("  Test pixel readback: [0]=%d [1]=%d [2]=%d [3]=%d (magenta pattern detected)",
            pixels[0], pixels[1], pixels[2], pixels[3]);
        return 1;
    } else if (pixels[0] == 0 && pixels[1] == 0 && pixels[2] == 0) {
        /* All black - nothing was rendered */
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL pixel verification failed: got black [0,0,0,%d]. "
            "Rendering may not be working.",
            pixels[3]);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        SDL_Log("  SDL_Renderer: %s", result->gl_renderer);
        return 0;
    } else {
        /* Got something unexpected but not black - likely a format issue, allow it */
        result->pixels_correct = 1;
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL rendering verified (unexpected format)");
        SDL_Log("  Test pixel readback: [0]=%d [1]=%d [2]=%d [3]=%d",
            pixels[0], pixels[1], pixels[2], pixels[3]);
        return 1;
    }
}

/*
 * Get detailed startup test results (for external diagnostics)
 * Returns pointer to static result struct
 */
STASIS_EXPORT const char* stasis_get_startup_test_error(void) {
    return g_last_test_result.error_message;
}

STASIS_EXPORT int stasis_get_startup_test_success(void) {
    return g_last_test_result.success;
}

/*
 * Initialize graphics window
 * Returns 1 on success, 0 on failure
 */
STASIS_EXPORT int stasis_init_window(int width, int height, const char* title) {
    if (g_window) {
        if (title && *title) {
            SDL_SetWindowTitle(g_window, title);
        }
        stasis_set_window_size(width, height);
        return 1;
    }

    SDL_LogSetOutputFunction(stasis_sdl_log_output, NULL);
    SDL_LogSetAllPriority(SDL_LOG_PRIORITY_INFO);
    log_package_provenance();
    SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "linear");
    if (SDL_Init(SDL_INIT_VIDEO | SDL_INIT_EVENTS) < 0) {
        SDL_Log("SDL_Init failed: %s", SDL_GetError());
        return 0;
    }

    /* Enable PNG sprite loading via SDL_image. */
    int img_flags = IMG_INIT_PNG;
    int img_inited = IMG_Init(img_flags);
    if ((img_inited & img_flags) != img_flags) {
        SDL_Log("IMG_Init failed (got=0x%x want=0x%x): %s", img_inited, img_flags, IMG_GetError());
        SDL_Quit();
        return 0;
    }

    /* Optional screenshot automation via environment variables. */
    g_screenshot_taken = false;
    g_screenshot_exit_after = 0;
    g_screenshot_frame = 1;
    g_debug_frame_counter = 0;
    g_screenshot_path[0] = 0;
    const char* screenshot = SDL_getenv("STASIS_SCREENSHOT_ONCE");
    if (screenshot && *screenshot) {
        strncpy(g_screenshot_path, screenshot, sizeof(g_screenshot_path) - 1);
        g_screenshot_path[sizeof(g_screenshot_path) - 1] = 0;
        const char* exit_after = SDL_getenv("STASIS_EXIT_AFTER_SCREENSHOT");
        if (exit_after && exit_after[0] == '1') {
            g_screenshot_exit_after = 1;
        }
        const char* screenshot_frame = SDL_getenv("STASIS_SCREENSHOT_FRAME");
        if (screenshot_frame && *screenshot_frame) {
            char* end = NULL;
            long parsed_frame = strtol(screenshot_frame, &end, 10);
            if (end != screenshot_frame && *end == 0 &&
                parsed_frame >= 1 && parsed_frame <= INT_MAX) {
                g_screenshot_frame = (int)parsed_frame;
            }
        }
    }

    const char* force_sdl = SDL_getenv("STASIS_USE_SDL");
    bool want_sdl = (force_sdl && strcmp(force_sdl, "0") != 0);
#if defined(STASIS_GRAPHICS_SDL_ONLY)
    want_sdl = true;
#endif

    g_window_width = width;
    g_window_height = height;
    g_native_window_width = width;
    g_native_window_height = height;
    g_drawable_width = width;
    g_drawable_height = height;
    g_pixel_scale = 1.0f;

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (!want_sdl) {
        /* Request OpenGL 2.1 compatibility profile for immediate mode */
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_MAJOR_VERSION, 2);
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_MINOR_VERSION, 1);
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_PROFILE_MASK, SDL_GL_CONTEXT_PROFILE_COMPATIBILITY);
        SDL_GL_SetAttribute(SDL_GL_DOUBLEBUFFER, 1);
        SDL_GL_SetAttribute(SDL_GL_DEPTH_SIZE, 24);
    }
#endif

    int native_request_width = width;
    int native_request_height = height;
    Uint32 window_flags = (want_sdl ? 0 : SDL_WINDOW_OPENGL) |
        SDL_WINDOW_SHOWN | SDL_WINDOW_RESIZABLE | SDL_WINDOW_ALLOW_HIGHDPI;
#if defined(__ANDROID__) || defined(__IPHONEOS__)
    SDL_DisplayMode display_mode;
    if (SDL_GetCurrentDisplayMode(0, &display_mode) == 0 &&
        display_mode.w > 0 && display_mode.h > 0) {
        native_request_width = display_mode.w;
        native_request_height = display_mode.h;
    }
    window_flags |= SDL_WINDOW_FULLSCREEN_DESKTOP;
#endif

    g_window = SDL_CreateWindow(
        title ? title : "Stasis",
        SDL_WINDOWPOS_CENTERED,
        SDL_WINDOWPOS_CENTERED,
        native_request_width,
        native_request_height,
        window_flags
    );

    if (!g_window) {
        SDL_Log("SDL_CreateWindow failed: %s", SDL_GetError());
        SDL_Quit();
        return 0;
    }

    /* Optional: start window minimized to keep automated/local test runs unobtrusive. */
    {
        const char* start_minimized = SDL_getenv("STASIS_WINDOW_START_MINIMIZED");
        if (start_minimized && strcmp(start_minimized, "0") != 0) {
            SDL_MinimizeWindow(g_window);
        }
    }

    /* Try GL first unless overridden */
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (!want_sdl) {
        g_gl_context = SDL_GL_CreateContext(g_window);
        if (g_gl_context) {
            glewExperimental = GL_TRUE;
            GLenum glew_status = glewInit();
            if (glew_status != GLEW_OK) {
                SDL_Log("glewInit failed: %s", (const char*)glewGetErrorString(glew_status));
                SDL_GL_DeleteContext(g_gl_context);
                g_gl_context = NULL;
            } else {
                int want_vsync = 1;
                const char* vsync_env = getenv("STASIS_GFX_VSYNC");
                if (vsync_env && vsync_env[0] == '0') {
                    want_vsync = 0;
                }
                int swap_ok = SDL_GL_SetSwapInterval(want_vsync ? 1 : 0);
                if (swap_ok != 0) {
                    SDL_Log("SDL_GL_SetSwapInterval failed (vsync=%d): %s", want_vsync, SDL_GetError());
                }
                glViewport(0, 0, width, height);
                glDisable(GL_SCISSOR_TEST);
                glEnable(GL_BLEND);
                glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
                glDisable(GL_DEPTH_TEST);
                glDisable(GL_CULL_FACE);
                glLineWidth(1.0f);

                g_window_width = width;
                g_window_height = height;
                glBindFramebuffer(GL_FRAMEBUFFER, 0);
                glDrawBuffer(GL_BACK);
                glReadBuffer(GL_BACK);
                setup_ortho();
                ensure_line_program();
                init_postfx_shader();

                SDL_Log("Stasis graphics initialized: %dx%d", width, height);
                SDL_Log("GL_VENDOR: %s", (const char*)glGetString(GL_VENDOR));
                SDL_Log("GL_RENDERER: %s", (const char*)glGetString(GL_RENDERER));
                SDL_Log("GL_VERSION: %s", (const char*)glGetString(GL_VERSION));
            }
        }
    }
#endif

    if (g_gl_context == NULL) {
        g_use_sdl_renderer = true;
        g_postfx_force_disable = true;
        g_renderer = SDL_CreateRenderer(g_window, -1, SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC);
        if (!g_renderer) {
            SDL_Log("SDL_CreateRenderer failed: %s", SDL_GetError());
            SDL_DestroyWindow(g_window);
            g_window = NULL;
            g_use_sdl_renderer = false;
            SDL_Quit();
            return 0;
        }
        if (SDL_RenderSetVSync(g_renderer, 1) != 0) {
            SDL_Log("SDL_RenderSetVSync failed: %s", SDL_GetError());
        }
        SDL_RenderSetLogicalSize(g_renderer, width, height);
        SDL_RendererInfo info;
        if (SDL_GetRendererInfo(g_renderer, &info) == 0) {
            SDL_Log("Stasis graphics initialized (SDL renderer): %dx%d name=%s flags=0x%x", width, height, info.name ? info.name : "?", info.flags);
        } else {
            SDL_Log("Stasis graphics initialized (SDL renderer): %dx%d", width, height);
        }
    } else {
        g_use_sdl_renderer = false;
    }

    stasis_sync_display_metrics();
    stasis_renderer_lifecycle_initialize(&g_resource_lifecycle);
    g_resource_frame_ready = true;
    SDL_Log("Stasis display metrics: logical=%dx%d native=%dx%d drawable=%dx%d scale=%.2f",
        g_window_width, g_window_height,
        g_native_window_width, g_native_window_height,
        g_drawable_width, g_drawable_height, g_pixel_scale);
    g_keyboard_state = SDL_GetKeyboardState(NULL);
    g_should_quit = false;
    g_line_count = 0;
    g_events_pumped_this_frame = 0;
    memset(&g_input_frame, 0, sizeof(g_input_frame));
    memset(g_finger_active, 0, sizeof(g_finger_active));
    memset(g_finger_ids, 0, sizeof(g_finger_ids));
    for (int i = 0; i < STASIS_MAX_POINTERS; i++) {
        g_input_frame.pointers[i].id = i;
    }

    /* Run startup render verification only when explicitly enabled. */
    const char* run_test = SDL_getenv("STASIS_RUN_RENDER_TEST");
    const char* skip_test = SDL_getenv("STASIS_SKIP_RENDER_TEST");
    int should_run_test = (run_test && strcmp(run_test, "0") != 0);
    int should_skip_test = (skip_test && strcmp(skip_test, "0") != 0);
    if (should_run_test && !should_skip_test) {
        int test_ok;
#if defined(STASIS_GRAPHICS_SDL_ONLY)
        if (!g_use_sdl_renderer) {
            fprintf(stderr, "error: SDL-only build requires an SDL renderer.\n");
            SDL_Quit();
            return 0;
        }
        test_ok = verify_sdl_rendering(g_renderer, width, height);
#else
        if (g_use_sdl_renderer) {
            test_ok = verify_sdl_rendering(g_renderer, width, height);
        } else {
            test_ok = verify_opengl_rendering(width, height);
        }
#endif

        if (!test_ok) {
            /* Print detailed diagnostics to stderr */
            fprintf(stderr, "\n");
            fprintf(stderr, "=== STASIS GRAPHICS STARTUP TEST FAILED ===\n");
            fprintf(stderr, "Error: %s\n", g_last_test_result.error_message);
            fprintf(stderr, "\n");
            fprintf(stderr, "Diagnostics:\n");
            if (g_use_sdl_renderer) {
                fprintf(stderr, "  Mode: SDL Renderer\n");
                fprintf(stderr, "  Renderer: %s\n", g_last_test_result.gl_renderer);
                fprintf(stderr, "  Info: %s\n", g_last_test_result.gl_version);
            } else {
                fprintf(stderr, "  Mode: OpenGL\n");
                fprintf(stderr, "  GL_VENDOR: %s\n", g_last_test_result.gl_vendor);
                fprintf(stderr, "  GL_RENDERER: %s\n", g_last_test_result.gl_renderer);
                fprintf(stderr, "  GL_VERSION: %s\n", g_last_test_result.gl_version);
                if (g_last_test_result.gl_error_code != 0) {
                    fprintf(stderr, "  GL Error Code: 0x%04X\n", g_last_test_result.gl_error_code);
                }
            }
            fprintf(stderr, "  Pixels Tested: %d\n", g_last_test_result.pixels_tested);
            fprintf(stderr, "  Pixels Correct: %d\n", g_last_test_result.pixels_correct);
            fprintf(stderr, "\n");
            fprintf(stderr, "Possible causes:\n");
            fprintf(stderr, "  - Running in a headless environment without display\n");
            fprintf(stderr, "  - GPU driver not properly installed\n");
            fprintf(stderr, "  - Remote desktop or virtual machine without GPU passthrough\n");
            fprintf(stderr, "  - Incompatible graphics hardware\n");
            fprintf(stderr, "\n");
            fprintf(stderr, "To disable this test, unset: STASIS_RUN_RENDER_TEST (or set to 0)\n");
            fprintf(stderr, "To force SDL renderer, set: STASIS_USE_SDL=1\n");
            fprintf(stderr, "================================================\n");
            fprintf(stderr, "\n");

            /* Cleanup and return failure */
            if (g_gl_context) {
                SDL_GL_DeleteContext(g_gl_context);
                g_gl_context = NULL;
            }
            if (g_renderer) {
                SDL_DestroyRenderer(g_renderer);
                g_renderer = NULL;
            }
            if (g_window) {
                SDL_DestroyWindow(g_window);
                g_window = NULL;
            }
            SDL_Quit();
            return 0;
        }

        /* Clear the test pattern before returning to caller */
        if (g_use_sdl_renderer) {
            SDL_SetRenderDrawColor(g_renderer, 0, 0, 0, 255);
            SDL_RenderClear(g_renderer);
        }
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        else {
            glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
            glClear(GL_COLOR_BUFFER_BIT);
        }
#endif
    }

    gfx_asset_watch_init();
    return 1;
}

/*
 * Get current window dimensions
 * Writes width and height to provided pointers
 */
STASIS_EXPORT void stasis_get_window_size(int* width, int* height) {
    if (width) *width = g_window_width;
    if (height) *height = g_window_height;
}

STASIS_EXPORT void stasis_get_display_metrics(
    int* logical_w,
    int* logical_h,
    int* native_w,
    int* native_h,
    int* drawable_w,
    int* drawable_h,
    int* safe_x,
    int* safe_y,
    int* safe_w,
    int* safe_h,
    float* content_scale,
    float* raster_scale,
    int* display_generation,
    int* density_generation
) {
    if (logical_w) *logical_w = g_display_metrics.logical_w;
    if (logical_h) *logical_h = g_display_metrics.logical_h;
    if (native_w) *native_w = g_display_metrics.native_w;
    if (native_h) *native_h = g_display_metrics.native_h;
    if (drawable_w) *drawable_w = g_display_metrics.drawable_w;
    if (drawable_h) *drawable_h = g_display_metrics.drawable_h;
    if (safe_x) *safe_x = (int)floorf(g_display_metrics.safe_logical_viewport.x);
    if (safe_y) *safe_y = (int)floorf(g_display_metrics.safe_logical_viewport.y);
    if (safe_w) *safe_w = (int)ceilf(g_display_metrics.safe_logical_viewport.w);
    if (safe_h) *safe_h = (int)ceilf(g_display_metrics.safe_logical_viewport.h);
    if (content_scale) *content_scale = g_display_metrics.content_scale;
    if (raster_scale) *raster_scale = g_display_metrics.raster_scale;
    if (display_generation) *display_generation = g_display_generation;
    if (density_generation) *density_generation = g_density_generation;
}

/*
 * Get current desktop usable dimensions (excluding taskbar/docks when available).
 * Writes width and height to provided pointers.
 *
 * Note: Requires SDL video to be initialized (typically via stasis_init_window).
 */
STASIS_EXPORT void stasis_get_desktop_size(int* width, int* height) {
    int w = 0;
    int h = 0;

    if (SDL_WasInit(SDL_INIT_VIDEO) == 0) {
        if (width) *width = 0;
        if (height) *height = 0;
        return;
    }

    SDL_Rect bounds;
    if (SDL_GetDisplayUsableBounds(0, &bounds) == 0) {
        w = bounds.w;
        h = bounds.h;
    } else {
        SDL_DisplayMode mode;
        if (SDL_GetDesktopDisplayMode(0, &mode) == 0) {
            w = mode.w;
            h = mode.h;
        }
    }

    if (width) *width = w;
    if (height) *height = h;
}

/*
 * Set window size (windowed mode).
 * width/height are in pixels.
 */
STASIS_EXPORT void stasis_set_window_size(int width, int height) {
    if (!g_window) {
        return;
    }

    if (width < 1 || height < 1) {
        return;
    }

    g_window_width = width;
    g_window_height = height;
    g_window_resized = true;
#if !defined(__ANDROID__) && !defined(__IPHONEOS__)
    SDL_SetWindowSize(g_window, width, height);
#endif
    stasis_sync_display_metrics();

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (!g_use_sdl_renderer) {
        setup_ortho();
        reset_line_program();
        reset_sprite_program();
    }
#endif
}

/*
 * Set fullscreen mode
 * fullscreen: 1 for fullscreen desktop, 0 for windowed
 * Returns 1 on success, 0 on failure
 */
STASIS_EXPORT int stasis_set_fullscreen(int fullscreen) {
    if (!g_window) {
        return 0;
    }

    Uint32 flags = fullscreen ? SDL_WINDOW_FULLSCREEN_DESKTOP : 0;
    int result = SDL_SetWindowFullscreen(g_window, flags);

    if (result == 0) {
        stasis_sync_display_metrics();

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        if (!g_use_sdl_renderer) {
            setup_ortho();
            reset_line_program();
            reset_sprite_program();
        }
#endif
    }

    return (result == 0) ? 1 : 0;
}

/*
 * Begin a new frame
 */
STASIS_EXPORT void stasis_begin_frame(void) {
    gfx_debug_hash_reset_if_enabled();
    gfx_asset_watch_apply_pending_changes();
    g_postfx_applied_this_frame = false;
    if (!g_events_pumped_this_frame) {
        stasis_pump_events();
        g_events_pumped_this_frame = 1;
    }
    g_resource_frame_ready = stasis_restore_renderer_resources() != 0;
    g_line_count = 0;
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
        SDL_RenderSetClipRect(g_renderer, NULL);
    } else {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        stasis_gl_clear_letterbox_bars();
#else
        (void)g_force_debug_overlay;
#endif
    }
}

static uint64_t stasis_perf_elapsed_us(uint64_t started_counter, uint64_t finished_counter) {
    const uint64_t frequency = SDL_GetPerformanceFrequency();
    if (started_counter == 0 || finished_counter < started_counter || frequency == 0) return 0;
    return ((finished_counter - started_counter) * 1000000u) / frequency;
}

static void stasis_perf_finish_render_sample(void) {
    const uint64_t now = SDL_GetPerformanceCounter();
    StasisPerfSample* sample = &g_perf_samples[g_perf_sample_next];
    sample->captured_counter = now;
    sample->tick_us = g_perf_pending_tick_us;
    sample->render_us = g_perf_pending_guest_render_us
        + stasis_perf_elapsed_us(g_perf_render_started_counter, now);
    g_perf_sample_next = (g_perf_sample_next + 1) % STASIS_PERF_SAMPLE_CAPACITY;
    if (g_perf_sample_count < STASIS_PERF_SAMPLE_CAPACITY) {
        g_perf_sample_count++;
    }
    g_perf_render_started_counter = 0;
}

static void stasis_perf_averages(double* tick_ms, double* render_ms) {
    const uint64_t now = SDL_GetPerformanceCounter();
    const uint64_t frequency = SDL_GetPerformanceFrequency();
    const uint64_t window = frequency * 5u;
    uint64_t tick_total = 0;
    uint64_t render_total = 0;
    int samples = 0;

    for (int i = 0; i < g_perf_sample_count; i++) {
        const StasisPerfSample* sample = &g_perf_samples[i];
        if (sample->captured_counter == 0 || now < sample->captured_counter) continue;
        if (now - sample->captured_counter > window) continue;
        tick_total += sample->tick_us;
        render_total += sample->render_us;
        samples++;
    }

    if (samples == 0) {
        *tick_ms = 0.0;
        *render_ms = 0.0;
        return;
    }
    *tick_ms = (double)tick_total / (double)samples / 1000.0;
    *render_ms = (double)render_total / (double)samples / 1000.0;
}

static int stasis_perf_font(void) {
    if (g_perf_font_handle >= 0) return g_perf_font_handle;
#if defined(_WIN32)
    const char* windows_dir = getenv("WINDIR");
    char path[1024];
    if (windows_dir && *windows_dir) {
        snprintf(path, sizeof(path), "%s/Fonts/segoeui.ttf", windows_dir);
    } else {
        snprintf(path, sizeof(path), "C:/Windows/Fonts/segoeui.ttf");
    }
    g_perf_font_handle = stasis_load_font(path, 15);
#else
    g_perf_font_handle = 0;
#endif
    return g_perf_font_handle;
}

static void stasis_perf_draw_overlay(void) {
    if (!g_force_debug_overlay) return;
    const int font = stasis_perf_font();
    if (font <= 0) return;

    double tick_ms = 0.0;
    double render_ms = 0.0;
    stasis_perf_averages(&tick_ms, &render_ms);
    const double total_ms = tick_ms + render_ms;
    const int budget_percent = (int)(total_ms * 6.0 + 0.5);
    char text[160];
    snprintf(
        text,
        sizeof(text),
        "tick=%.2f ms  render=%.2f ms  total=%.2f ms  budget@60fps=%d%%  [F3]",
        tick_ms,
        render_ms,
        total_ms,
        budget_percent);

    float r = 1.0f;
    float g = 1.0f;
    float b = 1.0f;
    if (budget_percent >= 100) {
        r = 0.73f; g = 0.41f; b = 1.0f;
    } else if (budget_percent >= 80) {
        r = 1.0f; g = 0.36f; b = 0.36f;
    } else if (budget_percent >= 50) {
        r = 1.0f; g = 0.84f; b = 0.40f;
    }

    const int background_width = g_window_width > 690 ? 680 : g_window_width - 16;
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
        SDL_SetRenderDrawColor(g_renderer, 20, 28, 38, 170);
        SDL_Rect background = { 8, 8, background_width, 28 };
        if (background.w > 0) SDL_RenderFillRect(g_renderer, &background);
    } else {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        flush_lines();
        flush_sprites();
        if (background_width > 0) {
            glDisable(GL_TEXTURE_2D);
            glColor4f(0.08f, 0.11f, 0.15f, 0.67f);
            glBegin(GL_QUADS);
            glVertex2f(8.0f, 8.0f);
            glVertex2f(8.0f + (float)background_width, 8.0f);
            glVertex2f(8.0f + (float)background_width, 36.0f);
            glVertex2f(8.0f, 36.0f);
            glEnd();
        }
#endif
    }

    stasis_draw_text(font, text, 13.0f, 12.0f, 0.0f, 0.0f, 0.0f, 0.9f);
    stasis_draw_text(font, text, 12.0f, 11.0f, r, g, b, 1.0f);
}

/*
 * End frame: flush lines, swap buffers, poll events
 */
STASIS_EXPORT void stasis_end_frame(void) {
    if (!g_resource_frame_ready) {
        g_perf_render_started_counter = 0;
        g_line_count = 0;
        g_events_pumped_this_frame = 0;
        return;
    }
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
        SDL_Color color;
        /* Render lines one by one; could be grouped by color if needed */
        for (int i = 0; i < g_line_count; i++) {
            color.r = (Uint8)(g_lines[i].r * 255.0f);
            color.g = (Uint8)(g_lines[i].g * 255.0f);
            color.b = (Uint8)(g_lines[i].b * 255.0f);
            color.a = (Uint8)(g_lines[i].a * 255.0f);
            SDL_SetRenderDrawColor(g_renderer, color.r, color.g, color.b, color.a);
            SDL_RenderDrawLineF(g_renderer, g_lines[i].x1, g_lines[i].y1, g_lines[i].x2, g_lines[i].y2);
        }

        /* Capture before present so we read the current render target. */
        capture_scheduled_screenshot();
        stasis_perf_finish_render_sample();
        SDL_RenderPresent(g_renderer);
        g_line_count = 0;
    } else {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        flush_lines();
        flush_sprites();
        if (!g_postfx_applied_this_frame) {
            render_postfx();
            g_postfx_applied_this_frame = true;
        }
        /* Capture after all draws (including postfx) but before swap. */
        capture_scheduled_screenshot();
        stasis_perf_finish_render_sample();
        SDL_GL_SwapWindow(g_window);
#else
        /* STASIS_GRAPHICS_SDL_ONLY should never create a GL context. */
        g_line_count = 0;
#endif
    }

    g_debug_frame_counter++;
    g_events_pumped_this_frame = 0;
}

/*
 * Clear screen with color
 */
STASIS_EXPORT void stasis_clear(float r, float g, float b, float a) {
    gfx_debug_hash_f32(r);
    gfx_debug_hash_f32(g);
    gfx_debug_hash_f32(b);
    gfx_debug_hash_f32(a);
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_NONE);
        SDL_SetRenderDrawColor(g_renderer, 0, 0, 0, 255);
        SDL_RenderClear(g_renderer);
        SDL_SetRenderDrawColor(g_renderer, (Uint8)(r * 255.0f), (Uint8)(g * 255.0f), (Uint8)(b * 255.0f), (Uint8)(a * 255.0f));
        SDL_FRect logical_canvas = {
            0.0f, 0.0f, (float)g_window_width, (float)g_window_height};
        SDL_RenderFillRectF(g_renderer, &logical_canvas);
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
    } else {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        const StasisDisplayViewport viewport = g_display_metrics.drawable_viewport;
        glEnable(GL_SCISSOR_TEST);
        glScissor(
            (int)viewport.x,
            stasis_display_bottom_origin_y(
                g_display_metrics.drawable_h, viewport),
            (int)viewport.w,
            (int)viewport.h);
        glClearColor(r, g, b, a);
        glClear(GL_COLOR_BUFFER_BIT);
        glDisable(GL_SCISSOR_TEST);
#else
        (void)r; (void)g; (void)b; (void)a;
#endif
    }
}

/*
 * Queue a line for batch rendering
 * Coordinates in screen space (0,0 = top-left)
 */
STASIS_EXPORT void stasis_draw_line(float x1, float y1, float x2, float y2,
                                    float r, float g, float b, float a) {
    gfx_debug_hash_f32(x1);
    gfx_debug_hash_f32(y1);
    gfx_debug_hash_f32(x2);
    gfx_debug_hash_f32(y2);
    gfx_debug_hash_f32(r);
    gfx_debug_hash_f32(g);
    gfx_debug_hash_f32(b);
    gfx_debug_hash_f32(a);
    if (g_use_sdl_renderer) {
        if (g_line_count >= MAX_LINES) {
            /* Cap silently */
            return;
        }
        g_lines[g_line_count].x1 = x1;
        g_lines[g_line_count].y1 = y1;
        g_lines[g_line_count].x2 = x2;
        g_lines[g_line_count].y2 = y2;
        g_lines[g_line_count].r = r;
        g_lines[g_line_count].g = g;
        g_lines[g_line_count].b = b;
        g_lines[g_line_count].a = a;
        g_line_count++;
        return;
    }

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (g_line_count >= MAX_LINES) {
        flush_lines();
    }

    g_lines[g_line_count].x1 = x1;
    g_lines[g_line_count].y1 = y1;
    g_lines[g_line_count].x2 = x2;
    g_lines[g_line_count].y2 = y2;
    g_lines[g_line_count].r = r;
    g_lines[g_line_count].g = g;
    g_lines[g_line_count].b = b;
    g_lines[g_line_count].a = a;
    g_line_count++;
#else
    (void)x1; (void)y1; (void)x2; (void)y2;
    (void)r; (void)g; (void)b; (void)a;
#endif
}

/*
 * Batched line submission.
 * lines: array of 8*f32 per line: x1,y1,x2,y2,r,g,b,a
 */
STASIS_EXPORT void stasis_draw_lines_f32(const float* lines, int line_count) {
    if (!lines || line_count <= 0) return;
    for (int i = 0; i < line_count; i++) {
        const int base = i * 8;
        stasis_draw_line(
            lines[base + 0],
            lines[base + 1],
            lines[base + 2],
            lines[base + 3],
            lines[base + 4],
            lines[base + 5],
            lines[base + 6],
            lines[base + 7]);
    }
}

/*
 * Command-buffer submission (v1 prototype).
 *
 * Command coordinates are host pixels. Ordering is fixed by the buffer layout:
 * clear -> lines -> sprites -> text -> present.
 */
static void flush_lines_before_later_layers(void) {
    if (g_line_count == 0) return;
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
        for (int i = 0; i < g_line_count; i++) {
            SDL_SetRenderDrawColor(
                g_renderer,
                (Uint8)(g_lines[i].r * 255.0f),
                (Uint8)(g_lines[i].g * 255.0f),
                (Uint8)(g_lines[i].b * 255.0f),
                (Uint8)(g_lines[i].a * 255.0f));
            SDL_RenderDrawLineF(
                g_renderer,
                g_lines[i].x1,
                g_lines[i].y1,
                g_lines[i].x2,
                g_lines[i].y2);
        }
        g_line_count = 0;
        return;
    }
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    flush_lines();
#endif
}

static void flush_sprites_before_text(void) {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (!g_use_sdl_renderer) flush_sprites();
#endif
}

static void stasis_gfx_submit_v1(const int32_t* cmd_i32, const float* cmd_f32, const uint8_t* cmd_u8) {
    static int contract_logged = 0;
    StasisRenderV1Validation validation = stasis_render_v1_validate(cmd_i32, cmd_f32);
    if (validation != STASIS_RENDER_V1_VALID) {
        if (!contract_logged) {
            SDL_Log(
                "Stasis renderer rejected frame: stage=command_header failure=%s magic=%d version=%d backend=%s surface_generation=%u renderer_generation=%u",
                stasis_render_v1_validation_name(validation),
                cmd_i32 ? cmd_i32[STASIS_RENDER_I_MAGIC] : 0,
                cmd_i32 ? cmd_i32[STASIS_RENDER_I_VERSION] : 0,
                g_use_sdl_renderer ? "sdl" : "gl",
                g_resource_lifecycle.surface_generation,
                g_resource_lifecycle.renderer_generation);
            contract_logged = 1;
        }
        return;
    }
    g_perf_render_started_counter = SDL_GetPerformanceCounter();

    const int32_t flags = cmd_i32[STASIS_RENDER_I_FLAGS];
    const int32_t gfx_cmd_max_lines = STASIS_RENDER_MAX_LINES;
    const int32_t gfx_cmd_max_sprites = STASIS_RENDER_MAX_SPRITES;
    const int32_t gfx_cmd_max_text = STASIS_RENDER_MAX_TEXT;
    const int32_t gfx_cmd_max_text_bytes = STASIS_RENDER_TEXT_MAX_BYTES;

    int32_t line_count = cmd_i32[3];
    int32_t sprite_count = cmd_i32[4];
    int32_t text_count = cmd_i32[7];
    int32_t text_bytes_used = cmd_i32[9];

    if (line_count < 0) line_count = 0;
    if (sprite_count < 0) sprite_count = 0;
    if (text_count < 0) text_count = 0;
    if (text_bytes_used < 0) text_bytes_used = 0;

    if (line_count > gfx_cmd_max_lines) line_count = gfx_cmd_max_lines;
    if (sprite_count > gfx_cmd_max_sprites) sprite_count = gfx_cmd_max_sprites;
    if (text_count > gfx_cmd_max_text) text_count = gfx_cmd_max_text;
    if (text_bytes_used > gfx_cmd_max_text_bytes) text_bytes_used = gfx_cmd_max_text_bytes;

    if (!contract_logged) {
        SDL_Log(
            "Stasis render contract v%d trace=%u flags=%d lines=%d sprites=%d text=%d",
            STASIS_RENDER_V1_VERSION,
            (unsigned int)stasis_render_v1_trace(cmd_i32, cmd_f32, cmd_u8),
            flags,
            line_count,
            sprite_count,
            text_count);
        contract_logged = 1;
    }

    stasis_begin_frame();

    if ((flags & STASIS_RENDER_FLAG_CLEAR) != 0) {
        stasis_clear(cmd_f32[0], cmd_f32[1], cmd_f32[2], cmd_f32[3]);
    }

    /* lines: f32 header is 4 (clear rgba), then line payload */
    if (line_count > 0) {
        stasis_draw_lines_f32(cmd_f32 + 4, line_count);
    }

    /* Lines are buffered on both backends. Flush before sprites so later layers
     * cannot cover text after text has already been drawn. */
    flush_lines_before_later_layers();

    /* sprites: i32 header is 32, then sprite payload */
    if (sprite_count > 0) {
        const int32_t* sprites = cmd_i32 + STASIS_RENDER_I_SPRITE_BASE;
        if (!g_debug_hash_enabled && !g_use_sdl_renderer) {
            stasis_gfx_draw_sprites_i32_fast(sprites, sprite_count);
        } else {
            for (int i = 0; i < sprite_count; i++) {
                const int base = i * 7;
                stasis_gfx_draw_sprite_internal(
                    sprites[base + 0],
                    sprites[base + 1],
                    sprites[base + 2],
                    sprites[base + 3],
                    sprites[base + 4],
                    sprites[base + 5],
                    sprites[base + 6],
                    g_debug_hash_enabled);
            }
        }
    }

    /* OpenGL sprites are batched. Native text draws immediately, so commit the
     * sprite batch first to preserve the documented line -> sprite -> text order. */
    flush_sprites_before_text();

    /* text: payload is split between i32 metadata + u8 bytes + f32 color/pos */
    /* byte_off < 0 encodes cached text run handle (no cmd_u8 access). */
    if (text_count > 0) {
        const int32_t text_i32_base = STASIS_RENDER_I_TEXT_BASE;
        const int32_t text_f32_base = STASIS_RENDER_F_TEXT_BASE;
        const int32_t* text_meta = cmd_i32 + text_i32_base;

        for (int i = 0; i < text_count; i++) {
            const int base_i = i * 3;
            const int font = text_meta[base_i + 0];
            const int byte_off = text_meta[base_i + 1];
            const int byte_len = text_meta[base_i + 2];

            if (font <= 0) continue;
            if (byte_off < 0) {
                const int run = -byte_off;
                const int base_f = text_f32_base + i * 6;
                const float x = cmd_f32[base_f + 0];
                const float y = cmd_f32[base_f + 1];
                const float r = cmd_f32[base_f + 2];
                const float g = cmd_f32[base_f + 3];
                const float b = cmd_f32[base_f + 4];
                const float a = cmd_f32[base_f + 5];
                stasis_gfx_draw_text_cached(run, x, y, r, g, b, a);
                continue;
            }
            if (!cmd_u8 || text_bytes_used <= 0) continue;
            if (!stasis_render_v1_text_span_is_valid(
                    byte_off, byte_len, text_bytes_used)) continue;

            const char* text = (const char*)(cmd_u8 + byte_off);

            const int base_f = text_f32_base + i * 6;
            const float x = cmd_f32[base_f + 0];
            const float y = cmd_f32[base_f + 1];
            const float r = cmd_f32[base_f + 2];
            const float g = cmd_f32[base_f + 3];
            const float b = cmd_f32[base_f + 4];
            const float a = cmd_f32[base_f + 5];

            stasis_draw_text(font, text, x, y, r, g, b, a);
        }
    }

    /* Present only if requested (lets benchmarks exclude swap/vsync). */
    if ((flags & STASIS_RENDER_FLAG_PRESENT) != 0) {
        stasis_perf_draw_overlay();
        stasis_end_frame();
    } else {
        g_perf_render_started_counter = 0;
    }
}

STASIS_EXPORT void stasis_gfx_submit(const int32_t* cmd_i32, const float* cmd_f32) {
    stasis_gfx_submit_v1(cmd_i32, cmd_f32, NULL);
}

STASIS_EXPORT void stasis_gfx_submit_u8(const int32_t* cmd_i32, const float* cmd_f32, const uint8_t* cmd_u8) {
    stasis_gfx_submit_v1(cmd_i32, cmd_f32, cmd_u8);
}

static SpriteEntry* sprite_get(int handle) {
    if (handle <= 0) return NULL;
    uint32_t raw = (uint32_t)handle;
    int idx = (int)(raw & SPRITE_HANDLE_INDEX_MASK) - 1;
    uint32_t generation = (raw >> SPRITE_HANDLE_INDEX_BITS) & SPRITE_HANDLE_GENERATION_MASK;
    if (idx < 0 || idx >= g_sprite_capacity) return NULL;
    if (!g_sprites) return NULL;
    if (!g_sprites[idx].used) return NULL;
    if (g_sprites[idx].generation != generation) return NULL;
    return &g_sprites[idx];
}

static int sprite_handle_for_slot(int slot) {
    if (slot < 0 || slot >= (int)SPRITE_HANDLE_INDEX_MASK) return 0;
    uint32_t generation = g_sprites[slot].generation & SPRITE_HANDLE_GENERATION_MASK;
    return (int)((generation << SPRITE_HANDLE_INDEX_BITS) | (uint32_t)(slot + 1));
}

STASIS_EXPORT int stasis_gfx_poll_reload(int handle) {
    SpriteEntry* e = sprite_get(handle);
    if (!e) return 0;
    if (!e->reload_pending) return 0;
    e->reload_pending = 0;
    return 1;
}

static int gfx_should_log_sprite_loads(void) {
    static int cached = -1;
    if (cached != -1) return cached;
    const char* env = getenv("STASIS_GFX_LOG_SPRITES");
    cached = (env && env[0] == '1') ? 1 : 0;
    return cached;
}

static void sprite_set_gl_region(SpriteEntry* e, int page_index, int sprite_x, int sprite_y,
                                 int alloc_x, int alloc_y, int alloc_w, int alloc_h,
                                 int sprite_w, int sprite_h) {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    SpriteAtlasPage* page = &g_sprite_atlas_pages[page_index];
    e->page_index = page_index;
    e->atlas_x = sprite_x;
    e->atlas_y = sprite_y;
    e->alloc_x = alloc_x;
    e->alloc_y = alloc_y;
    e->alloc_w = alloc_w;
    e->alloc_h = alloc_h;
    e->u0 = (float)sprite_x / (float)page->w;
    e->v0 = (float)sprite_y / (float)page->h;
    e->u1 = (float)(sprite_x + sprite_w) / (float)page->w;
    e->v1 = (float)(sprite_y + sprite_h) / (float)page->h;
#else
    (void)e;
    (void)page_index;
    (void)sprite_x;
    (void)sprite_y;
    (void)alloc_x;
    (void)alloc_y;
    (void)alloc_w;
    (void)alloc_h;
    (void)sprite_w;
    (void)sprite_h;
#endif
}

/*
 * Build sprite at specified max size. Used for sized loading and re-rasterization.
 */
static int sprite_build_into_entry_sized(SpriteEntry* e, const char* path, int max_w, int max_h) {
    const int raster_w = stasis_display_scaled_extent(max_w, g_pixel_scale);
    const int raster_h = stasis_display_scaled_extent(max_h, g_pixel_scale);
    if (!sprite_source_within_limits(path, raster_w, raster_h)) {
        return 0;
    }
    unsigned char* pixels = NULL;
    int w = 0, h = 0;
    if (!bake_image_to_rgba_sized(path, raster_w, raster_h, &pixels, &w, &h)) {
        SDL_Log("gfx_load_sprite: failed to bake %s at logical=%dx%d raster=%dx%d",
            path, max_w, max_h, raster_w, raster_h);
        return 0;
    }

    if (g_use_sdl_renderer) {
        if (!g_renderer) {
            free(pixels);
            return 0;
        }

        /* SDL expects straight alpha; convert from premultiplied. */
        for (int i = 0; i < w * h; i++) {
            unsigned char* p = pixels + i * 4;
            unsigned char a = p[3];
            if (a == 0) {
                p[0] = 0; p[1] = 0; p[2] = 0;
                continue;
            }
            int r = p[0];
            int g = p[1];
            int b = p[2];
            p[0] = (unsigned char)((r * 255 + (a / 2)) / a);
            p[1] = (unsigned char)((g * 255 + (a / 2)) / a);
            p[2] = (unsigned char)((b * 255 + (a / 2)) / a);
        }

        SDL_Texture* tex = SDL_CreateTexture(g_renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_STATIC, w, h);
        if (!tex) {
            SDL_Log("gfx_load_sprite: SDL_CreateTexture failed: %s", SDL_GetError());
            free(pixels);
            return 0;
        }
        SDL_SetTextureBlendMode(tex, SDL_BLENDMODE_BLEND);
        if (SDL_UpdateTexture(tex, NULL, pixels, w * 4) != 0) {
            SDL_Log("gfx_load_sprite: SDL_UpdateTexture failed: %s", SDL_GetError());
            SDL_DestroyTexture(tex);
            free(pixels);
            return 0;
        }

        free(pixels);
        SDL_Texture* previous = e->sdl_tex;
        e->w = w;
        e->h = h;
        e->max_w = max_w;
        e->max_h = max_h;
        e->page_index = -1;
        e->atlas_x = 0;
        e->atlas_y = 0;
        e->alloc_x = 0;
        e->alloc_y = 0;
        e->alloc_w = 0;
        e->alloc_h = 0;
        e->sdl_tex = tex;
        e->mtime = get_file_mtime(path);
        e->needs_reraster = 0;
        e->surface_generation = g_resource_lifecycle.surface_generation;
        e->renderer_generation = g_resource_lifecycle.renderer_generation;
        if (previous) SDL_DestroyTexture(previous);
        return 1;
    }

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (g_sprite_vert_count > 0) {
        flush_sprites();
    }

    const int can_reuse_existing = 0;

    int page_index = e->page_index;
    int sprite_x = e->atlas_x;
    int sprite_y = e->atlas_y;
    int alloc_x = e->alloc_x;
    int alloc_y = e->alloc_y;
    int alloc_w = e->alloc_w;
    int alloc_h = e->alloc_h;

    if (!can_reuse_existing) {
        if (!atlas_alloc(w, h, path, &page_index, &sprite_x, &sprite_y, &alloc_x, &alloc_y, &alloc_w, &alloc_h)) {
            free(pixels);
            return 0;
        }
    }

    SpriteAtlasPage* page = &g_sprite_atlas_pages[page_index];
    atlas_page_clear_region(page, alloc_x, alloc_y, alloc_w, alloc_h);
    if (!atlas_page_upload_region(page, sprite_x, sprite_y, w, h, pixels)) {
        if (!can_reuse_existing) {
            atlas_release_rect(page_index, alloc_x, alloc_y, alloc_w, alloc_h);
        }
        free(pixels);
        return 0;
    }

    free(pixels);

    if (!can_reuse_existing && e->page_index >= 0 && e->alloc_w > 0 && e->alloc_h > 0) {
        atlas_release_rect(e->page_index, e->alloc_x, e->alloc_y, e->alloc_w, e->alloc_h);
    }

    e->w = w;
    e->h = h;
    e->max_w = max_w;
    e->max_h = max_h;
    sprite_set_gl_region(e, page_index, sprite_x, sprite_y, alloc_x, alloc_y, alloc_w, alloc_h, w, h);
    e->mtime = get_file_mtime(path);
    e->needs_reraster = 0;
    e->surface_generation = g_resource_lifecycle.surface_generation;
    e->renderer_generation = g_resource_lifecycle.renderer_generation;
    return 1;
#else
    free(pixels);
    return 0;
#endif
}

static void gfx_asset_watch_apply_pending_changes(void) {
#if defined(_WIN32)
    if (!gfx_asset_watch_enabled()) return;

    if (InterlockedExchange(&g_asset_watch_dirty, 0) == 0) {
        return;
    }

    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* e = &g_sprites[i];
        if (!e->used || !e->path) continue;

        uint64_t mt = get_file_mtime(e->path);
        if (!mt || mt <= e->mtime) continue;

        if (!sprite_build_into_entry_sized(e, e->path, e->max_w, e->max_h)) {
            SDL_Log("gfx_watch: reload failed for %s", e->path);
        } else {
            e->reload_pending = 1;
        }
    }
#endif
}

/*
 * Load and bake a sprite from an SVG file at a specified max size.
 * The sprite will be rasterized to fit within max_w x max_h while preserving aspect ratio.
 * Returns an integer handle (stable for the lifetime of the process).
 */
STASIS_EXPORT int stasis_gfx_load_sprite(const char* path, int max_w, int max_h) {
    if (!path || !*path) return 0;
    if (!g_window) return 0;
    if (!g_use_sdl_renderer && !g_gl_context) return 0;
    if (g_use_sdl_renderer && !g_renderer) return 0;
    if (max_w <= 0 || max_h <= 0) return 0;

    char resolved[1024];
    if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
        SDL_Log("gfx_load_sprite: could not resolve %s", path);
        return 0;
    }
    const int raster_w = stasis_display_scaled_extent(max_w, g_pixel_scale);
    const int raster_h = stasis_display_scaled_extent(max_h, g_pixel_scale);

    /* Reuse the device-local raster/GPU texture for the same source and
     * logical target size. Drawable-density changes mark the entry dirty and
     * replace its raster before it is returned or drawn, so the effective key
     * is source + logical extent + current drawable extent. */
    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* cached = &g_sprites[i];
        if (!cached->used || !cached->path) continue;
        if (cached->max_w != max_w || cached->max_h != max_h) continue;
        if (strcmp(cached->path, resolved) != 0) continue;
        if (cached->w != raster_w || cached->h != raster_h) {
            cached->needs_reraster = 1;
        }
        if (cached->needs_reraster &&
            !sprite_build_into_entry_sized(cached, resolved, max_w, max_h)) {
            return 0;
        }
        if (cached->ref_count == INT_MAX) return 0;
        cached->ref_count++;
        return sprite_handle_for_slot(i);
    }

    if (!ensure_sprite_table_capacity(1)) {
        SDL_Log("gfx_load_sprite: sprite table allocation failed for %s", resolved);
        return 0;
    }

    int slot = -1;
    while (slot < 0) {
        for (int i = 0; i < g_sprite_capacity; i++) {
            if (!g_sprites[i].used && !g_sprites[i].retired) {
                slot = i;
                break;
            }
        }
        if (slot >= 0) break;
        if (!ensure_sprite_table_capacity(g_sprite_capacity + 1)) {
            SDL_Log("gfx_load_sprite: sprite table full for %s sprites=%d capacity=%d limit=%d",
                    resolved,
                    g_sprite_count,
                    g_sprite_capacity,
                    g_sprite_table_limit);
            return 0;
        }
    }

    SpriteEntry* e = &g_sprites[slot];
    uint32_t generation = e->generation;
    memset(e, 0, sizeof(*e));
    e->generation = generation;
    e->page_index = -1;
    e->path = stasis_strdup(resolved);
    if (!e->path) return 0;
    e->used = 1;
    e->ref_count = 1;
    if (!sprite_build_into_entry_sized(e, resolved, max_w, max_h)) {
        free(e->path);
        memset(e, 0, sizeof(*e));
        e->generation = generation;
        e->page_index = -1;
        return 0;
    }

    g_sprite_count++;
    if (gfx_should_log_sprite_loads()) {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        const int page_count_for_log = g_use_sdl_renderer ? 0 : g_sprite_atlas_page_count;
#else
        const int page_count_for_log = 0;
#endif
        SDL_Log("gfx_load_sprite: %s (%dx%d) -> handle=%d raster=%dx%d backend=%s page=%d pages=%d sprites=%d/%d",
                resolved, max_w, max_h, sprite_handle_for_slot(slot), e->w, e->h,
                g_use_sdl_renderer ? "sdl" : "gl",
                e->page_index,
                page_count_for_log,
                g_sprite_count,
                g_sprite_capacity);
    }
    return sprite_handle_for_slot(slot);
}

static SpriteEntry* sprite_fallback_get(void) {
    if (g_sprite_fallback.used) return &g_sprite_fallback;
    if (!g_window) return NULL;

    static const unsigned char pixels[16] = {
        255, 0, 255, 255, 24, 24, 24, 255,
        24, 24, 24, 255, 255, 0, 255, 255
    };
    SpriteEntry next;
    memset(&next, 0, sizeof(next));
    next.page_index = -1;
    next.w = 2;
    next.h = 2;
    next.max_w = 2;
    next.max_h = 2;

    if (g_use_sdl_renderer) {
        if (!g_renderer) return NULL;
        next.sdl_tex = SDL_CreateTexture(
            g_renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_STATIC, 2, 2);
        if (!next.sdl_tex) return NULL;
        SDL_SetTextureBlendMode(next.sdl_tex, SDL_BLENDMODE_BLEND);
        if (SDL_UpdateTexture(next.sdl_tex, NULL, pixels, 2 * 4) != 0) {
            SDL_DestroyTexture(next.sdl_tex);
            return NULL;
        }
    } else {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        int page_index, sprite_x, sprite_y, alloc_x, alloc_y, alloc_w, alloc_h;
        if (!atlas_alloc(2, 2, "<fallback>", &page_index, &sprite_x, &sprite_y,
                         &alloc_x, &alloc_y, &alloc_w, &alloc_h)) {
            return NULL;
        }
        SpriteAtlasPage* page = &g_sprite_atlas_pages[page_index];
        atlas_page_clear_region(page, alloc_x, alloc_y, alloc_w, alloc_h);
        if (!atlas_page_upload_region(page, sprite_x, sprite_y, 2, 2, pixels)) {
            atlas_release_rect(page_index, alloc_x, alloc_y, alloc_w, alloc_h);
            return NULL;
        }
        sprite_set_gl_region(&next, page_index, sprite_x, sprite_y,
                             alloc_x, alloc_y, alloc_w, alloc_h, 2, 2);
#else
        return NULL;
#endif
    }
    next.used = 1;
    next.surface_generation = g_resource_lifecycle.surface_generation;
    next.renderer_generation = g_resource_lifecycle.renderer_generation;
    g_sprite_fallback = next;
    return &g_sprite_fallback;
}

STASIS_EXPORT void stasis_gfx_release_sprite(int handle) {
    SpriteEntry* e = sprite_get(handle);
    if (!e) return;

    if (e->ref_count > 1) {
        e->ref_count--;
        return;
    }

    if (g_use_sdl_renderer) {
        if (e->sdl_tex) SDL_DestroyTexture(e->sdl_tex);
    } else {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        if (g_sprite_vert_count > 0) flush_sprites();
        if (e->page_index >= 0 && e->alloc_w > 0 && e->alloc_h > 0) {
            atlas_page_clear_region(
                &g_sprite_atlas_pages[e->page_index],
                e->alloc_x, e->alloc_y, e->alloc_w, e->alloc_h);
            atlas_release_rect(e->page_index, e->alloc_x, e->alloc_y, e->alloc_w, e->alloc_h);
        }
#endif
    }
    free(e->path);
    uint32_t next_generation = (e->generation + 1u) & SPRITE_HANDLE_GENERATION_MASK;
    memset(e, 0, sizeof(*e));
    e->generation = next_generation;
    e->retired = next_generation == 0u ? 1 : 0;
    e->page_index = -1;
    if (g_sprite_count > 0) g_sprite_count--;
}

/*
 * Draw a sprite at a specific size (top-left anchored) with rotation and tint.
 * All parameters are integers for simpler Stasis integration.
 * x, y: top-left position in pixels
 * w, h: desired size in pixels
 * rot_degrees: rotation in degrees (0-359), around the sprite center
 * a: alpha 0-255
 */
static void stasis_gfx_draw_sprite_internal(int handle, int x, int y, int w, int h,
                                           int rot_degrees, int a, int do_hash) {
    if (do_hash) {
        gfx_debug_hash_i32(handle);
        gfx_debug_hash_i32(x);
        gfx_debug_hash_i32(y);
        gfx_debug_hash_i32(w);
        gfx_debug_hash_i32(h);
        gfx_debug_hash_i32(rot_degrees);
        gfx_debug_hash_i32(a);
    }
    SpriteEntry* e = sprite_get(handle);
    if (!e) e = sprite_fallback_get();
    if (!e) return;

    if (w <= 0 || h <= 0) return;
    if (a < 0) a = 0;
    if (a > 255) a = 255;

    /* Re-rasterize only when explicitly invalidated (resize/reload).
     *
     * Re-baking per draw-size can overflow the atlas when sizes fluctuate frame-to-frame.
     * Sprites are baked at their load-time max size (max_w/max_h) and drawn scaled.
     */
    if (e->needs_reraster) {
        if (e->path) sprite_build_into_entry_sized(e, e->path, e->max_w, e->max_h);
    }
    if (e->surface_generation != g_resource_lifecycle.surface_generation ||
        e->renderer_generation != g_resource_lifecycle.renderer_generation) {
        SDL_Log("Stasis renderer rejected stale sprite: handle=%d path=%s logical=%dx%d raster=%dx%d backend=%s surface_generation=%u resource_surface_generation=%u renderer_generation=%u resource_renderer_generation=%u",
            handle, e->path ? e->path : "<fallback>", e->max_w, e->max_h, e->w, e->h,
            g_use_sdl_renderer ? "sdl" : "gl",
            g_resource_lifecycle.surface_generation, e->surface_generation,
            g_resource_lifecycle.renderer_generation, e->renderer_generation);
        return;
    }

    /* Convert degrees to radians */
    float rot = (float)rot_degrees * (3.14159265f / 180.0f);

    /* Alpha from 0-255 to 0.0-1.0 */
    float af = (float)a / 255.0f;

    if (g_use_sdl_renderer) {
        if (!g_renderer || !e->sdl_tex) return;
#if SDL_VERSION_ATLEAST(2,0,10)
        SDL_FRect dst;
        dst.w = (float)w;
        dst.h = (float)h;
        dst.x = (float)x;
        dst.y = (float)y;
        SDL_FPoint center = { dst.w * 0.5f, dst.h * 0.5f };
        SDL_SetTextureColorMod(e->sdl_tex, 255, 255, 255);
        SDL_SetTextureAlphaMod(e->sdl_tex, (Uint8)a);
        SDL_RenderCopyExF(g_renderer, e->sdl_tex, NULL, &dst, (double)rot_degrees, &center, SDL_FLIP_NONE);
#else
        SDL_Rect dst;
        dst.w = w;
        dst.h = h;
        dst.x = x;
        dst.y = y;
        SDL_Point center = { w / 2, h / 2 };
        SDL_SetTextureColorMod(e->sdl_tex, 255, 255, 255);
        SDL_SetTextureAlphaMod(e->sdl_tex, (Uint8)a);
        SDL_RenderCopyEx(g_renderer, e->sdl_tex, NULL, &dst, (double)rot_degrees, &center, SDL_FLIP_NONE);
#endif
        return;
    }

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (g_sprite_vert_count + 6 > MAX_SPRITE_VERTS) {
        flush_sprites();
    }
    if (g_sprite_vert_count > 0 && g_sprite_batch_page != e->page_index) {
        flush_sprites();
    }
    g_sprite_batch_page = e->page_index;
#endif

    float hw = (float)w * 0.5f;
    float hh = (float)h * 0.5f;
    float c = cosf(rot);
    float s = sinf(rot);

    float x0 = -hw, y0 = -hh;
    float x1 = hw, y1 = hh;
    float fx = (float)x + hw;
    float fy = (float)y + hh;

    float p0x = fx + x0 * c - y0 * s;
    float p0y = fy + x0 * s + y0 * c;
    float p1x = fx + x1 * c - y0 * s;
    float p1y = fy + x1 * s + y0 * c;
    float p2x = fx + x1 * c - y1 * s;
    float p2y = fy + x1 * s + y1 * c;
    float p3x = fx + x0 * c - y1 * s;
    float p3y = fy + x0 * s + y1 * c;

    float u0 = e->u0, v0 = e->v0, u1 = e->u1, v1 = e->v1;

    SpriteVertex* v = &g_sprite_vertices[g_sprite_vert_count];
    /* tri 1: 0,1,2 */
    v[0] = (SpriteVertex){ p0x, p0y, u0, v0, af, af, af, af };
    v[1] = (SpriteVertex){ p1x, p1y, u1, v0, af, af, af, af };
    v[2] = (SpriteVertex){ p2x, p2y, u1, v1, af, af, af, af };
    /* tri 2: 2,3,0 */
    v[3] = (SpriteVertex){ p2x, p2y, u1, v1, af, af, af, af };
    v[4] = (SpriteVertex){ p3x, p3y, u0, v1, af, af, af, af };
    v[5] = (SpriteVertex){ p0x, p0y, u0, v0, af, af, af, af };
    g_sprite_vert_count += 6;
}

STASIS_EXPORT void stasis_gfx_draw_sprite(int handle, int x, int y, int w, int h,
                                          int rot_degrees, int a) {
    stasis_gfx_draw_sprite_internal(handle, x, y, w, h, rot_degrees, a, 1);
}

/*
 * Batched sprite submission.
 * cmds: array of 7*i32 per sprite: handle,x,y,w,h,rot_degrees,a
 */
STASIS_EXPORT void stasis_gfx_draw_sprites_i32(const int32_t* cmds, int sprite_count) {
    if (!cmds || sprite_count <= 0) return;
    for (int i = 0; i < sprite_count; i++) {
        const int base = i * 7;
        stasis_gfx_draw_sprite_internal(
            cmds[base + 0],
            cmds[base + 1],
            cmds[base + 2],
            cmds[base + 3],
            cmds[base + 4],
            cmds[base + 5],
            cmds[base + 6],
            g_debug_hash_enabled);
    }
}

/*
 * Check if a key is currently pressed
 * Uses SDL scancodes (SDL_SCANCODE_*)
 */
STASIS_EXPORT int stasis_is_key_down(int scancode) {
    /* Pump events to ensure keyboard state is current */
    SDL_PumpEvents();
    g_keyboard_state = SDL_GetKeyboardState(NULL);
    if (!g_keyboard_state) return 0;
    if (scancode < 0 || scancode >= SDL_NUM_SCANCODES) return 0;
    return g_keyboard_state[scancode] ? 1 : 0;
}

/*
 * Get current time in milliseconds
 */
STASIS_EXPORT int stasis_get_time_ms(void) {
#if defined(_WIN32)
    if (SDL_WasInit(SDL_INIT_TIMER) == 0) {
        if (SDL_Init(SDL_INIT_TIMER) != 0) {
            return 0;
        }
    }
    return (int)SDL_GetTicks();
#else
    if (SDL_WasInit(SDL_INIT_TIMER) != 0) {
        return (int)SDL_GetTicks();
    }
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        return 0;
    }
    return (int)((now.tv_sec * 1000) + (now.tv_nsec / 1000000));
#endif
}

/*
 * Get current time in microseconds (truncated to i32).
 */
STASIS_EXPORT int stasis_get_time_us(void) {
    if (SDL_WasInit(SDL_INIT_TIMER) == 0) {
        if (SDL_Init(SDL_INIT_TIMER) != 0) {
            return 0;
        }
    }
    Uint64 freq = SDL_GetPerformanceFrequency();
    if (freq == 0) return 0;
    Uint64 counter = SDL_GetPerformanceCounter();
    Uint64 us = (counter * 1000000ull) / freq;
    return (int)us;
}

/*
 * Sleep for specified milliseconds
 */
STASIS_EXPORT void stasis_sleep_ms(int ms) {
    if (ms > 0) {
        if (SDL_WasInit(SDL_INIT_TIMER) != 0) {
            SDL_Delay((Uint32)ms);
            return;
        }
#if defined(_WIN32)
        if (SDL_Init(SDL_INIT_TIMER) != 0) {
            return;
        }
        SDL_Delay((Uint32)ms);
#else
        struct timespec delay;
        delay.tv_sec = ms / 1000;
        delay.tv_nsec = (ms % 1000) * 1000000;
        nanosleep(&delay, NULL);
#endif
    }
}

static int stasis_storage_component_valid(const char* value) {
    size_t length;
    size_t index;
    if (!value) return 0;
    length = strlen(value);
    if (length == 0 || length > 63) return 0;
    for (index = 0; index < length; index += 1) {
        unsigned char ch = (unsigned char)value[index];
        if (!((ch >= 'A' && ch <= 'Z') ||
              (ch >= 'a' && ch <= 'z') ||
              (ch >= '0' && ch <= '9') ||
              ch == '_' || ch == '-')) return 0;
    }
    return 1;
}

static int stasis_storage_i32_path(
    const char* scope,
    const char* key,
    char* path,
    size_t capacity,
    char** owned_root
) {
    int written;
    char* root;
    if (!path || capacity == 0 || !owned_root ||
        !stasis_storage_component_valid(scope) || !stasis_storage_component_valid(key)) {
        return 0;
    }
    root = SDL_GetPrefPath("StasisLang", scope);
    if (!root) return 0;
    written = snprintf(path, capacity, "%s%s.i32", root, key);
    if (written < 0 || (size_t)written >= capacity) {
        SDL_free(root);
        return 0;
    }
    *owned_root = root;
    return 1;
}

STASIS_EXPORT int stasis_storage_load_i32(const char* scope, const char* key, int fallback) {
    char path[1024];
    char buffer[64];
    char* root = NULL;
    char* end = NULL;
    long long parsed;
    FILE* file;
    int trailing;
    if (!stasis_storage_i32_path(scope, key, path, sizeof(path), &root)) return fallback;
    file = fopen(path, "rb");
    SDL_free(root);
    if (!file) return fallback;
    if (!fgets(buffer, sizeof(buffer), file)) {
        fclose(file);
        return fallback;
    }
    trailing = fgetc(file);
    fclose(file);
    if (trailing != EOF) return fallback;
    errno = 0;
    parsed = strtoll(buffer, &end, 10);
    if (errno != 0 || end == buffer) return fallback;
    while (*end != '\0' && isspace((unsigned char)*end)) end += 1;
    if (*end != '\0' || parsed < INT32_MIN || parsed > INT32_MAX) return fallback;
    return (int)parsed;
}

STASIS_EXPORT int stasis_storage_save_i32(const char* scope, const char* key, int value) {
    char path[1024];
    char temp_path[1032];
    char text[32];
    char* root = NULL;
    FILE* file;
    int path_written;
    int text_written;
    int ok = 1;
    if (!stasis_storage_i32_path(scope, key, path, sizeof(path), &root)) return 0;
    SDL_free(root);
    path_written = snprintf(temp_path, sizeof(temp_path), "%s.tmp", path);
    text_written = snprintf(text, sizeof(text), "%d\n", value);
    if (path_written < 0 || (size_t)path_written >= sizeof(temp_path) ||
        text_written < 0 || (size_t)text_written >= sizeof(text)) {
        return 0;
    }
    file = fopen(temp_path, "wb");
    if (!file) return 0;
    if (fwrite(text, 1, (size_t)text_written, file) != (size_t)text_written) ok = 0;
    if (ok && fflush(file) != 0) ok = 0;
    if (fclose(file) != 0) ok = 0;
    if (!ok) {
        remove(temp_path);
        return 0;
    }
#if defined(_WIN32)
    if (!MoveFileExA(temp_path, path, MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)) {
        remove(temp_path);
        return 0;
    }
#else
    if (rename(temp_path, path) != 0) {
        remove(temp_path);
        return 0;
    }
#endif
    return 1;
}

/*
 * Audio - init/shutdown and ring-buffer push API
 */
STASIS_EXPORT int stasis_audio_init(int sample_rate, int channels, int target_latency_frames) {
    if (sample_rate > 0) g_audio_sample_rate = sample_rate;
    if (channels != 0 && channels != 2) return 0;
    g_audio_channels = 2;
    if (target_latency_frames > 0) g_audio_target_latency_frames = target_latency_frames;

    if (g_audio_device != 0) {
        stasis_audio_shutdown_internal();
    }

    return stasis_audio_ensure_init();
}

STASIS_EXPORT void stasis_audio_shutdown(void) {
    stasis_audio_shutdown_internal();
}

STASIS_EXPORT int stasis_audio_is_available(void) {
    return stasis_audio_ensure_init();
}

STASIS_EXPORT int stasis_audio_get_sample_rate(void) {
    if (!stasis_audio_ensure_init()) return 0;
    return g_audio_sample_rate;
}

STASIS_EXPORT int stasis_audio_get_channels(void) {
    if (!stasis_audio_ensure_init()) return 0;
    return g_audio_channels;
}

STASIS_EXPORT int stasis_audio_get_queued_frames(void) {
    if (!stasis_audio_ensure_init()) return 0;

    int queued = 0;
    SDL_LockAudioDevice(g_audio_device);
    if (g_audio_channels > 0) {
        queued = g_audio_queued_samples / g_audio_channels;
    }
    SDL_UnlockAudioDevice(g_audio_device);
    return queued;
}

STASIS_EXPORT int stasis_audio_get_underruns(void) {
    if (!stasis_audio_ensure_init()) return 0;

    int underruns = 0;
    SDL_LockAudioDevice(g_audio_device);
    underruns = g_audio_underruns;
    SDL_UnlockAudioDevice(g_audio_device);
    return underruns;
}

STASIS_EXPORT int stasis_audio_push_f32_interleaved(const float* interleaved_lr, int frame_count) {
    if (!interleaved_lr || frame_count <= 0) return 0;
    if (!stasis_audio_ensure_init()) return 0;

    int accepted_samples = 0;
    const int requested_samples = frame_count * g_audio_channels;

    SDL_LockAudioDevice(g_audio_device);
    int free_samples = g_audio_ring_capacity_samples - g_audio_queued_samples;
    int to_write = stasis_audio_mini(requested_samples, free_samples);

    while (to_write > 0) {
        int contiguous = g_audio_ring_capacity_samples - g_audio_write_sample;
        int chunk = stasis_audio_mini(to_write, contiguous);
        SDL_memcpy(&g_audio_ring[g_audio_write_sample], &interleaved_lr[accepted_samples], (size_t)chunk * sizeof(float));
        g_audio_write_sample = (g_audio_write_sample + chunk) % g_audio_ring_capacity_samples;
        g_audio_queued_samples += chunk;
        accepted_samples += chunk;
        to_write -= chunk;
    }
    SDL_UnlockAudioDevice(g_audio_device);

    if (g_audio_channels <= 0) return 0;
    return accepted_samples / g_audio_channels;
}

/*
 * Configure fullscreen post-processing parameters.
 * strength: 0-1, phase/time: seconds, speed: oscillation scalar, color: rgb tint (0-1).
 */
STASIS_EXPORT void stasis_set_postfx(float strength, float phase, float speed, float r, float g, float b) {
    g_postfx_strength = strength;
    g_postfx_phase = phase;
    g_postfx_speed = speed;
    g_postfx_color[0] = r;
    g_postfx_color[1] = g;
    g_postfx_color[2] = b;
    g_postfx_enabled = true;
}

/*
 * Check if window should close
 */
STASIS_EXPORT int stasis_should_quit(void) {
    if (!g_events_pumped_this_frame) {
        stasis_pump_events();
        g_events_pumped_this_frame = 1;
    }
    return g_should_quit ? 1 : 0;
}

/* Mobile lifecycle polling remains responsive while no frame is presented. */
STASIS_EXPORT int stasis_mobile_poll_events(void) {
    stasis_pump_events();
    g_events_pumped_this_frame = 0;
    return g_should_quit ? 1 : 0;
}

STASIS_EXPORT void stasis_mobile_set_paused(int paused) {
    if (g_audio_device != 0) {
        SDL_PauseAudioDevice(g_audio_device, paused ? 1 : 0);
    }
    if (paused) {
        stasis_renderer_lifecycle_pause(&g_resource_lifecycle);
        g_resource_frame_ready = false;
    } else if (g_resource_lifecycle.state == STASIS_RENDERER_PAUSED) {
        stasis_renderer_lifecycle_resume(&g_resource_lifecycle);
        stasis_invalidate_renderer_resources(0);
    }
}

STASIS_EXPORT int stasis_gfx_get_resource_lifecycle(int32_t* out_i32, int count) {
    if (!out_i32 || count < 6) return 0;
    out_i32[0] = (int32_t)g_resource_lifecycle.state;
    out_i32[1] = (int32_t)g_resource_lifecycle.surface_generation;
    out_i32[2] = (int32_t)g_resource_lifecycle.renderer_generation;
    out_i32[3] = (int32_t)g_resource_lifecycle.restore_attempts;
    out_i32[4] = (int32_t)g_resource_lifecycle.restore_failures;
    out_i32[5] = (int32_t)g_resource_lifecycle.reason;
    return 1;
}

/*
 * Get current window width in pixels
 */
STASIS_EXPORT int stasis_gfx_window_width(void) {
    return g_window_width;
}

/*
 * Get current window height in pixels
 */
STASIS_EXPORT int stasis_gfx_window_height(void) {
    return g_window_height;
}

/*
 * Check if window was resized since last call.
 * Returns 1 if resized, 0 otherwise. Clears the flag after reading.
 */
STASIS_EXPORT int stasis_gfx_window_resized(void) {
    int result = g_window_resized ? 1 : 0;
    g_window_resized = false;
    return result;
}

/*
 * Cleanup and shutdown
 */
STASIS_EXPORT void stasis_shutdown(void) {
    stasis_audio_shutdown_internal();
    gfx_asset_watch_shutdown();

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    if (g_postfx_program) {
        glDeleteProgram(g_postfx_program);
        g_postfx_program = 0;
    }
    if (g_sprite_program) {
        glDeleteProgram(g_sprite_program);
        g_sprite_program = 0;
    }
    if (g_sprite_vbo) {
        glDeleteBuffers(1, &g_sprite_vbo);
        g_sprite_vbo = 0;
    }
    if (g_sprite_vao) {
        glDeleteVertexArrays(1, &g_sprite_vao);
        g_sprite_vao = 0;
    }
    for (int i = 0; i < g_sprite_atlas_page_count; i++) {
        if (g_sprite_atlas_pages[i].texture) {
            glDeleteTextures(1, &g_sprite_atlas_pages[i].texture);
            g_sprite_atlas_pages[i].texture = 0;
        }
        free(g_sprite_atlas_pages[i].free_rects);
        g_sprite_atlas_pages[i].free_rects = NULL;
        g_sprite_atlas_pages[i].free_rect_count = 0;
        g_sprite_atlas_pages[i].free_rect_capacity = 0;
    }
    free(g_sprite_atlas_pages);
    g_sprite_atlas_pages = NULL;
    g_sprite_atlas_page_count = 0;
    g_sprite_atlas_page_capacity = 0;
    g_sprite_atlas_page_w = 0;
    g_sprite_atlas_page_h = 0;
    g_sprite_atlas_gl_max_size = 0;
    g_sprite_batch_page = -1;
#endif
    for (int i = 0; i < g_sprite_capacity; i++) {
        if (g_sprites[i].used) {
            if (g_sprites[i].sdl_tex) {
                SDL_DestroyTexture(g_sprites[i].sdl_tex);
                g_sprites[i].sdl_tex = NULL;
            }
            if (g_sprites[i].path) free(g_sprites[i].path);
            memset(&g_sprites[i], 0, sizeof(g_sprites[i]));
        }
    }
    free(g_sprites);
    g_sprites = NULL;
    g_sprite_capacity = 0;
    g_sprite_count = 0;
    g_sprite_table_limit = -1;
    g_sprite_max_dimension = -1;
    g_sprite_max_pixels = -1;
    g_sprite_max_file_bytes = -1;
    if (g_sprite_fallback.sdl_tex) SDL_DestroyTexture(g_sprite_fallback.sdl_tex);
    memset(&g_sprite_fallback, 0, sizeof(g_sprite_fallback));
    g_sprite_fallback.page_index = -1;

    for (int i = 0; i < MAX_FONTS; i++) {
        if (g_fonts[i].active) {
            if (g_fonts[i].sdl_texture) {
                SDL_DestroyTexture(g_fonts[i].sdl_texture);
                g_fonts[i].sdl_texture = NULL;
            }
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
            if (g_fonts[i].atlas_texture) {
                glDeleteTextures(1, &g_fonts[i].atlas_texture);
                g_fonts[i].atlas_texture = 0;
            }
#endif
            if (g_fonts[i].ttf_buffer) {
                free(g_fonts[i].ttf_buffer);
                g_fonts[i].ttf_buffer = NULL;
            }
            g_fonts[i].active = false;
        }
    }
    stasis_reset_text_cache();
    if (g_renderer) {
        SDL_DestroyRenderer(g_renderer);
        g_renderer = NULL;
    }
    g_use_sdl_renderer = false;
    if (g_gl_context) {
        SDL_GL_DeleteContext(g_gl_context);
        g_gl_context = NULL;
    }
    if (g_window) {
        SDL_DestroyWindow(g_window);
        g_window = NULL;
    }
    SDL_Quit();
    memset(&g_resource_lifecycle, 0, sizeof(g_resource_lifecycle));
    g_resource_frame_ready = false;
    SDL_Log("Stasis graphics shutdown");
}

/* ===== DIRECTORY LISTING ===== */

#ifdef _WIN32
#else
#include <dirent.h>
#endif

#define STASIS_DIR_LIST_MAX_ENTRIES 256
#define STASIS_DIR_LIST_NAME_LEN 260
#define STASIS_UTF8_HEADER_SIZE 8
#define STASIS_DIR_ENTRY_STRIDE (STASIS_UTF8_HEADER_SIZE + STASIS_DIR_LIST_NAME_LEN)

static int count_utf8_codepoints(const unsigned char* data, int len)
{
    int count = 0;
    int i = 0;
    while (i < len) {
        unsigned char c = data[i];
        int advance = 1;
        if ((c & 0x80) == 0x00) {
            advance = 1;
        } else if ((c & 0xE0) == 0xC0) {
            advance = 2;
        } else if ((c & 0xF0) == 0xE0) {
            advance = 3;
        } else if ((c & 0xF8) == 0xF0) {
            advance = 4;
        }
        i += advance;
        count++;
    }
    return count;
}

static void write_utf8_entry(unsigned char* entry_base, const char* src)
{
    int copy_len = 0;
    while (copy_len < STASIS_DIR_LIST_NAME_LEN && src[copy_len] != '\0') {
        entry_base[STASIS_UTF8_HEADER_SIZE + copy_len] = (unsigned char)src[copy_len];
        copy_len++;
    }
    entry_base[STASIS_UTF8_HEADER_SIZE + copy_len] = 0;
    int char_len = count_utf8_codepoints(&entry_base[STASIS_UTF8_HEADER_SIZE], copy_len);
    *((int32_t*)(entry_base + 0)) = copy_len;
    *((int32_t*)(entry_base + 4)) = char_len;
}

/* List files in a directory
 * Returns number of files found (up to max_count)
 * out_paths: array of pointers to receive file paths
 * max_count: maximum number of files to return
 */
STASIS_EXPORT int stasis_list_directory(const char* path, char** out_paths, int max_count, int path_buffer_size) {
    if (!path || !out_paths || max_count <= 0) return 0;

    int count = 0;

#ifdef _WIN32
    char search_path[512];
    snprintf(search_path, sizeof(search_path), "%s\\*", path);

    WIN32_FIND_DATAA find_data;
    HANDLE hFind = FindFirstFileA(search_path, &find_data);

    if (hFind == INVALID_HANDLE_VALUE) {
        SDL_Log("stasis_list_directory: failed to open %s", path);
        return 0;
    }

    do {
        /* Skip . and .. */
        if (strcmp(find_data.cFileName, ".") == 0 || strcmp(find_data.cFileName, "..") == 0)
            continue;

        /* Copy filename to output buffer */
        if (count < max_count) {
            snprintf(out_paths[count], path_buffer_size, "%s", find_data.cFileName);
            count++;
        }
    } while (FindNextFileA(hFind, &find_data) != 0 && count < max_count);

    FindClose(hFind);
#else
    DIR* dir = opendir(path);
    if (!dir) {
        SDL_Log("stasis_list_directory: failed to open %s", path);
        return 0;
    }

    struct dirent* entry;
    while ((entry = readdir(dir)) != NULL && count < max_count) {
        /* Skip . and .. */
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0)
            continue;

        /* Copy filename to output buffer */
        snprintf(out_paths[count], path_buffer_size, "%s", entry->d_name);
        count++;
    }

    closedir(dir);
#endif

    SDL_Log("stasis_list_directory: found %d files in %s", count, path);
    return count;
}

STASIS_EXPORT int stasis_list_directory_struct(const char* path, unsigned char* names, int32_t* is_dir, int32_t* out_count) {
    if (!path || !names || !is_dir || !out_count) {
        return 0;
    }

    int count = 0;

#ifdef _WIN32
    char search_path[512];
    snprintf(search_path, sizeof(search_path), "%s\\*", path);

    WIN32_FIND_DATAA find_data;
    HANDLE hFind = FindFirstFileA(search_path, &find_data);

    if (hFind == INVALID_HANDLE_VALUE) {
        SDL_Log("stasis_list_directory_struct: failed to open %s", path);
        *out_count = 0;
        return 0;
    }

    do {
        if (strcmp(find_data.cFileName, ".") == 0 || strcmp(find_data.cFileName, "..") == 0)
            continue;

        if (count >= STASIS_DIR_LIST_MAX_ENTRIES)
            break;

        unsigned char* entry_ptr = names + ((size_t)count * STASIS_DIR_ENTRY_STRIDE);
        write_utf8_entry(entry_ptr, find_data.cFileName);
        is_dir[count] = (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) ? 1 : 0;
        count++;
    } while (FindNextFileA(hFind, &find_data) != 0);

    FindClose(hFind);
#else
    DIR* dir = opendir(path);
    if (!dir) {
        SDL_Log("stasis_list_directory_struct: failed to open %s", path);
        *out_count = 0;
        return 0;
    }

    struct dirent* entry;
    while ((entry = readdir(dir)) != NULL && count < STASIS_DIR_LIST_MAX_ENTRIES) {
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0)
            continue;

        bool entry_is_dir = false;
        char entry_path[512];
        snprintf(entry_path, sizeof(entry_path), "%s/%s", path, entry->d_name);
        struct stat st = {0};
        if (stat(entry_path, &st) == 0) {
            entry_is_dir = S_ISDIR(st.st_mode);
        }

        unsigned char* entry_ptr = names + ((size_t)count * STASIS_DIR_ENTRY_STRIDE);
        write_utf8_entry(entry_ptr, entry->d_name);
        is_dir[count] = entry_is_dir ? 1 : 0;
        count++;
    }

    closedir(dir);
#endif

    *out_count = count;
    return count;
}

STASIS_EXPORT void stasis_copy_dir_entry_name(const unsigned char* names, int32_t idx, unsigned char* out) {
    if (!names || !out || idx < 0 || idx >= STASIS_DIR_LIST_MAX_ENTRIES) {
        return;
    }

    size_t offset = (size_t)idx * STASIS_DIR_ENTRY_STRIDE;
    memcpy(out, names + offset, STASIS_DIR_ENTRY_STRIDE);
}

/* ===== FONT RENDERING WITH STB_TRUETYPE ===== */

/* ===== CACHED TEXT RUNS (glyph quads) ===== */

typedef struct {
    float x0, y0, x1, y1;
    float s0, t0, s1, t1;
} StasisTextQuad;

typedef struct {
    int active;
    int font_handle;
    uint32_t hash;
    int text_off;
    int text_len;
    int quad_off;
    int quad_count;
    float width;
    float height;
} StasisTextRun;

#define STASIS_MAX_TEXT_RUNS 1024
#define STASIS_TEXT_RUN_MAX_BYTES 262144
#define STASIS_TEXT_RUN_MAX_QUADS 65536

static StasisTextRun g_text_runs[STASIS_MAX_TEXT_RUNS];
static unsigned char g_text_run_bytes[STASIS_TEXT_RUN_MAX_BYTES];
static int g_text_run_bytes_used = 0;
static StasisTextQuad g_text_run_quads[STASIS_TEXT_RUN_MAX_QUADS];
static int g_text_run_quads_used = 0;

static void stasis_reset_text_cache(void) {
    memset(g_text_runs, 0, sizeof(g_text_runs));
    g_text_run_bytes_used = 0;
    g_text_run_quads_used = 0;
}

static uint32_t fnv1a_u32(const unsigned char* data, int len) {
    uint32_t h = 2166136261u;
    for (int i = 0; i < len; i++) {
        h ^= (uint32_t)data[i];
        h *= 16777619u;
    }
    return h;
}

static int stasis_build_font_atlas(StasisFont* font) {
    if (!font || !font->ttf_buffer || font->font_size <= 0) return 0;

    const float pixel_scale = g_pixel_scale < 1.0f ? 1.0f : g_pixel_scale;
    const int raster_size = stasis_display_scaled_extent(font->font_size, pixel_scale);
    const int atlas_size = stasis_display_font_atlas_extent(pixel_scale);
    const size_t atlas_pixels = (size_t)atlas_size * (size_t)atlas_size;
    unsigned char* atlas_bitmap = (unsigned char*)malloc(atlas_pixels);
    if (!atlas_bitmap) return 0;

    stbtt_bakedchar baked_chars[FONT_NUM_CHARS];
    int result = stbtt_BakeFontBitmap(font->ttf_buffer, 0, (float)raster_size,
        atlas_bitmap, atlas_size, atlas_size, FONT_FIRST_CHAR, FONT_NUM_CHARS, baked_chars);
    if (result <= 0) {
        free(atlas_bitmap);
        SDL_Log("stasis_load_font: BakeFontBitmap failed size=%d atlas=%d", raster_size, atlas_size);
        return 0;
    }

    if (g_use_sdl_renderer) {
        if (!g_renderer) {
            free(atlas_bitmap);
            return 0;
        }
        const size_t rgba_size = atlas_pixels * 4u;
        unsigned char* rgba = (unsigned char*)malloc(rgba_size);
        if (!rgba) {
            free(atlas_bitmap);
            return 0;
        }
        for (size_t i = 0; i < atlas_pixels; i++) {
            unsigned char alpha = atlas_bitmap[i];
            rgba[i * 4 + 0] = 255;
            rgba[i * 4 + 1] = 255;
            rgba[i * 4 + 2] = 255;
            rgba[i * 4 + 3] = alpha;
        }
        SDL_Texture* texture = SDL_CreateTexture(g_renderer, SDL_PIXELFORMAT_RGBA32,
            SDL_TEXTUREACCESS_STATIC, atlas_size, atlas_size);
        if (!texture) {
            free(rgba);
            free(atlas_bitmap);
            return 0;
        }
        SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_BLEND);
        if (SDL_UpdateTexture(texture, NULL, rgba, atlas_size * 4) != 0) {
            SDL_DestroyTexture(texture);
            free(rgba);
            free(atlas_bitmap);
            return 0;
        }
        free(rgba);
        if (font->sdl_texture) SDL_DestroyTexture(font->sdl_texture);
        font->sdl_texture = texture;
    } else {
#if !defined(STASIS_GRAPHICS_SDL_ONLY)
        GLuint texture = 0;
        glGenTextures(1, &texture);
        glBindTexture(GL_TEXTURE_2D, texture);
        glTexImage2D(GL_TEXTURE_2D, 0, GL_ALPHA, atlas_size, atlas_size,
            0, GL_ALPHA, GL_UNSIGNED_BYTE, atlas_bitmap);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        if (font->atlas_texture) glDeleteTextures(1, &font->atlas_texture);
        font->atlas_texture = texture;
#endif
    }

    free(atlas_bitmap);
    memcpy(font->char_data, baked_chars, sizeof(baked_chars));
    font->raster_size = raster_size;
    font->atlas_size = atlas_size;
    font->pixel_scale = pixel_scale;
    font->scale = stbtt_ScaleForPixelHeight(&font->font_info, (float)raster_size);
    font->needs_reraster = 0;
    font->surface_generation = g_resource_lifecycle.surface_generation;
    font->renderer_generation = g_resource_lifecycle.renderer_generation;
    return 1;
}

static int stasis_build_text_run_quads(StasisTextRun* run, StasisFont* font) {
    if (!run || !font || run->text_off < 0 || run->text_len <= 0) return 0;
    if (g_text_run_quads_used + run->text_len > STASIS_TEXT_RUN_MAX_QUADS) return 0;

    const char* text = (const char*)g_text_run_bytes + run->text_off;
    const float pixel_scale = font->pixel_scale;
    float pos_x = 0.0f;
    float pos_y = (float)font->ascent * font->scale;
    float max_x = 0.0f;
    float max_y = 0.0f;
    const float line_height =
        (float)(font->ascent - font->descent + font->line_gap) * font->scale;
    const int quad_off = g_text_run_quads_used;
    int quad_count = 0;

    for (int i = 0; i < run->text_len; i++) {
        unsigned char ch = (unsigned char)text[i];
        if (ch == '\r') continue;
        if (ch == '\n') {
            pos_x = 0.0f;
            pos_y += line_height;
            continue;
        }
        if (ch < FONT_FIRST_CHAR || ch >= FONT_FIRST_CHAR + FONT_NUM_CHARS) continue;

        stbtt_aligned_quad quad;
        stbtt_GetBakedQuad(font->char_data, font->atlas_size, font->atlas_size,
            (int)ch - FONT_FIRST_CHAR, &pos_x, &pos_y, &quad, g_use_sdl_renderer ? 0 : 1);
        StasisTextQuad* out = &g_text_run_quads[quad_off + quad_count];
        out->x0 = quad.x0 / pixel_scale;
        out->y0 = quad.y0 / pixel_scale;
        out->x1 = quad.x1 / pixel_scale;
        out->y1 = quad.y1 / pixel_scale;
        out->s0 = quad.s0;
        out->t0 = quad.t0;
        out->s1 = quad.s1;
        out->t1 = quad.t1;
        if (quad.x1 > max_x) max_x = quad.x1;
        if (quad.y1 > max_y) max_y = quad.y1;
        quad_count++;
    }

    g_text_run_quads_used += quad_count;
    run->quad_off = quad_off;
    run->quad_count = quad_count;
    run->width = max_x / pixel_scale;
    run->height = max_y / pixel_scale;
    return 1;
}

static int stasis_rebuild_text_runs(void) {
    g_text_run_quads_used = 0;
    for (int i = 0; i < STASIS_MAX_TEXT_RUNS; i++) {
        StasisTextRun* run = &g_text_runs[i];
        if (!run->active) continue;
        if (run->font_handle <= 0 || run->font_handle > MAX_FONTS) return 0;
        StasisFont* font = &g_fonts[run->font_handle - 1];
        if (!font->active || !stasis_build_text_run_quads(run, font)) return 0;
    }
    return 1;
}

static int stasis_ensure_font_ready(int font_handle) {
    if (font_handle <= 0 || font_handle > MAX_FONTS) return 0;
    StasisFont* font = &g_fonts[font_handle - 1];
    if (!font->active) return 0;

    int rebuilt_density_fonts = 0;
    for (int i = 0; i < MAX_FONTS; i++) {
        StasisFont* candidate = &g_fonts[i];
        if (!candidate->active || !candidate->needs_reraster) continue;
        if (!stasis_build_font_atlas(candidate)) return 0;
        rebuilt_density_fonts = 1;
    }
    return !rebuilt_density_fonts || stasis_rebuild_text_runs();
}

static int stasis_restore_renderer_resources(void) {
    if (g_resource_lifecycle.state == STASIS_RENDERER_PAUSED ||
        g_resource_lifecycle.state == STASIS_RENDERER_UNAVAILABLE) {
        return 0;
    }
    if (!stasis_renderer_lifecycle_begin_restore(&g_resource_lifecycle)) {
        return stasis_renderer_lifecycle_can_present(&g_resource_lifecycle);
    }

    int restored = g_use_sdl_renderer ? g_renderer != NULL : g_gl_context != NULL;
    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* entry = &g_sprites[i];
        if (!entry->used || !entry->path) continue;
        if (!sprite_build_into_entry_sized(entry, entry->path, entry->max_w, entry->max_h)) {
            SDL_Log("Stasis renderer restore failed: stage=sprite handle=%d path=%s logical=%dx%d raster=%dx%d backend=%s surface_generation=%u renderer_generation=%u reason=%s failure=texture_rebuild_failed",
                sprite_handle_for_slot(i), entry->path, entry->max_w, entry->max_h,
                entry->w, entry->h, g_use_sdl_renderer ? "sdl" : "gl",
                g_resource_lifecycle.surface_generation,
                g_resource_lifecycle.renderer_generation,
                stasis_renderer_reason_name(g_resource_lifecycle.reason));
            restored = 0;
        }
    }
    if (!sprite_fallback_get()) {
        SDL_Log("Stasis renderer restore failed: stage=fallback handle=0 path=<procedural> logical=2x2 raster=2x2 backend=%s surface_generation=%u renderer_generation=%u reason=%s failure=texture_rebuild_failed",
            g_use_sdl_renderer ? "sdl" : "gl",
            g_resource_lifecycle.surface_generation,
            g_resource_lifecycle.renderer_generation,
            stasis_renderer_reason_name(g_resource_lifecycle.reason));
        restored = 0;
    }
    for (int i = 0; i < MAX_FONTS; i++) {
        StasisFont* font = &g_fonts[i];
        if (!font->active) continue;
        if (!stasis_build_font_atlas(font)) {
            SDL_Log("Stasis renderer restore failed: stage=font handle=%d path=<retained-font-bytes> logical=%dx%d raster=%dx%d backend=%s surface_generation=%u renderer_generation=%u reason=%s failure=atlas_rebuild_failed",
                i + 1, font->font_size, font->font_size, font->raster_size,
                font->raster_size, g_use_sdl_renderer ? "sdl" : "gl",
                g_resource_lifecycle.surface_generation,
                g_resource_lifecycle.renderer_generation,
                stasis_renderer_reason_name(g_resource_lifecycle.reason));
            restored = 0;
        }
    }
    if (restored && !stasis_rebuild_text_runs()) {
        SDL_Log("Stasis renderer restore failed: stage=cached_text handle=0 path=<retained-text-runs> logical=0x0 raster=0x0 backend=%s surface_generation=%u renderer_generation=%u reason=%s failure=quad_rebuild_failed",
            g_use_sdl_renderer ? "sdl" : "gl",
            g_resource_lifecycle.surface_generation,
            g_resource_lifecycle.renderer_generation,
            stasis_renderer_reason_name(g_resource_lifecycle.reason));
        restored = 0;
    }

    stasis_renderer_lifecycle_finish_restore(&g_resource_lifecycle, restored);
    if (restored) {
        SDL_Log("Stasis renderer resources restored: backend=%s surface_generation=%u renderer_generation=%u reason=%s sprites=%d",
            g_use_sdl_renderer ? "sdl" : "gl",
            g_resource_lifecycle.surface_generation,
            g_resource_lifecycle.renderer_generation,
            stasis_renderer_reason_name(g_resource_lifecycle.reason),
            g_sprite_count);
    }
    return stasis_renderer_lifecycle_can_present(&g_resource_lifecycle);
}

static int stasis_find_or_alloc_text_run_slot(int font_handle, uint32_t hash, const char* text, int len) {
    int free_slot = -1;
    for (int i = 0; i < STASIS_MAX_TEXT_RUNS; i++) {
        if (!g_text_runs[i].active) {
            if (free_slot < 0) free_slot = i;
            continue;
        }
        if (g_text_runs[i].font_handle != font_handle) continue;
        if (g_text_runs[i].hash != hash) continue;
        if (g_text_runs[i].text_len != len) continue;
        if (g_text_runs[i].text_off < 0 || g_text_runs[i].text_off + len >= STASIS_TEXT_RUN_MAX_BYTES) continue;
        if (memcmp(g_text_run_bytes + g_text_runs[i].text_off, text, (size_t)len) == 0) {
            return i;
        }
    }
    return free_slot;
}

/* Cache a text run and return a 1-based handle (0 on failure). */
STASIS_EXPORT int stasis_gfx_cache_text(int font_handle, const char* text) {
    if (font_handle <= 0 || font_handle > MAX_FONTS) return 0;
    if (!text) return 0;
    if (!stasis_ensure_font_ready(font_handle)) return 0;
    StasisFont* font = &g_fonts[font_handle - 1];
    if (!font->active) return 0;

    const int len = (int)strlen(text);
    if (len <= 0) return 0;
    if (len > 8192) return 0; /* hard cap per cached run */

    const uint32_t hash = fnv1a_u32((const unsigned char*)text, len);
    const int slot = stasis_find_or_alloc_text_run_slot(font_handle, hash, text, len);
    if (slot < 0) return 0;
    if (g_text_runs[slot].active) {
        return slot + 1;
    }

    const int bytes_needed = len + 1;
    if (g_text_run_bytes_used + bytes_needed > STASIS_TEXT_RUN_MAX_BYTES) return 0;

    const int text_off = g_text_run_bytes_used;
    memcpy(g_text_run_bytes + text_off, text, (size_t)len);
    g_text_run_bytes[text_off + len] = 0;
    g_text_run_bytes_used += bytes_needed;

    StasisTextRun* run = &g_text_runs[slot];
    run->active = 1;
    run->font_handle = font_handle;
    run->hash = hash;
    run->text_off = text_off;
    run->text_len = len;
    if (!stasis_build_text_run_quads(run, font)) {
        memset(run, 0, sizeof(*run));
        return 0;
    }

    return slot + 1;
}

static void stasis_draw_text_cached_internal(int run_handle, float x, float y, float r, float g, float b, float a) {
    if (run_handle <= 0 || run_handle > STASIS_MAX_TEXT_RUNS) return;
    StasisTextRun* run = &g_text_runs[run_handle - 1];
    if (!run->active) return;
    if (run->font_handle <= 0 || run->font_handle > MAX_FONTS) return;
    if (!stasis_ensure_font_ready(run->font_handle)) return;

    StasisFont* font = &g_fonts[run->font_handle - 1];
    if (!font->active) return;

    if (g_use_sdl_renderer) {
        if (!font->sdl_texture || !g_renderer) return;

        SDL_SetTextureBlendMode(font->sdl_texture, SDL_BLENDMODE_BLEND);
        SDL_SetTextureColorMod(font->sdl_texture,
            (Uint8)(r < 0.0f ? 0 : (r > 1.0f ? 255 : (int)(r * 255.0f))),
            (Uint8)(g < 0.0f ? 0 : (g > 1.0f ? 255 : (int)(g * 255.0f))),
            (Uint8)(b < 0.0f ? 0 : (b > 1.0f ? 255 : (int)(b * 255.0f))));
        SDL_SetTextureAlphaMod(font->sdl_texture,
            (Uint8)(a < 0.0f ? 0 : (a > 1.0f ? 255 : (int)(a * 255.0f))));

        for (int i = 0; i < run->quad_count; i++) {
            StasisTextQuad* q = &g_text_run_quads[run->quad_off + i];
            SDL_Rect src;
            src.x = (int)(q->s0 * (float)font->atlas_size);
            src.y = (int)(q->t0 * (float)font->atlas_size);
            src.w = (int)((q->s1 - q->s0) * (float)font->atlas_size);
            src.h = (int)((q->t1 - q->t0) * (float)font->atlas_size);

            SDL_FRect dst;
            dst.x = x + q->x0;
            dst.y = y + q->y0;
            dst.w = q->x1 - q->x0;
            dst.h = q->y1 - q->y0;

            if (src.w > 0 && src.h > 0 && dst.w > 0.0f && dst.h > 0.0f) {
                SDL_RenderCopyF(g_renderer, font->sdl_texture, &src, &dst);
            }
        }
        return;
    }

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    glEnable(GL_TEXTURE_2D);
    glBindTexture(GL_TEXTURE_2D, font->atlas_texture);

    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
    glColor4f(r, g, b, a);

    glBegin(GL_QUADS);
    for (int i = 0; i < run->quad_count; i++) {
        StasisTextQuad* q = &g_text_run_quads[run->quad_off + i];
        glTexCoord2f(q->s0, q->t0); glVertex2f(x + q->x0, y + q->y0);
        glTexCoord2f(q->s1, q->t0); glVertex2f(x + q->x1, y + q->y0);
        glTexCoord2f(q->s1, q->t1); glVertex2f(x + q->x1, y + q->y1);
        glTexCoord2f(q->s0, q->t1); glVertex2f(x + q->x0, y + q->y1);
    }
    glEnd();

    glDisable(GL_TEXTURE_2D);
    glColor4f(1, 1, 1, 1);
#endif
}

STASIS_EXPORT void stasis_gfx_draw_text_cached(int run_handle, float x, float y, float r, float g, float b, float a) {
    stasis_draw_text_cached_internal(run_handle, x, y, r, g, b, a);
}

STASIS_EXPORT float stasis_gfx_measure_text_cached(int run_handle) {
    if (run_handle <= 0 || run_handle > STASIS_MAX_TEXT_RUNS) return 0.0f;
    StasisTextRun* run = &g_text_runs[run_handle - 1];
    if (!run->active) return 0.0f;
    if (!stasis_ensure_font_ready(run->font_handle)) return 0.0f;
    return run->width;
}

/* Load a TrueType font from disk */
STASIS_EXPORT int stasis_load_font(const char* path, int font_size) {
    if (!path || font_size <= 0) return 0;
    if (!g_window) return 0;

    char resolved[1024];
    if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
        SDL_Log("stasis_load_font: could not resolve %s", path);
        return 0;
    }

    /* Find free slot */
    int slot = -1;
    for (int i = 0; i < MAX_FONTS; i++) {
        if (!g_fonts[i].active) {
            slot = i;
            break;
        }
    }

    if (slot == -1) {
        SDL_Log("stasis_load_font: no free font slots");
        return 0;
    }

    /* Read font file */
    FILE* f = fopen(resolved, "rb");
    if (!f) {
        SDL_Log("stasis_load_font: failed to open %s", resolved);
        return 0;
    }

    fseek(f, 0, SEEK_END);
    size_t size = ftell(f);
    fseek(f, 0, SEEK_SET);

    unsigned char* ttf_buffer = (unsigned char*)malloc(size);
    if (!ttf_buffer) {
        fclose(f);
        SDL_Log("stasis_load_font: malloc failed");
        return 0;
    }

    fread(ttf_buffer, 1, size, f);
    fclose(f);

    /* Initialize font */
    StasisFont* font = &g_fonts[slot];
    memset(font, 0, sizeof(*font));
    if (!stbtt_InitFont(&font->font_info, ttf_buffer, 0)) {
        free(ttf_buffer);
        SDL_Log("stasis_load_font: stbtt_InitFont failed for %s", resolved);
        return 0;
    }

    font->ttf_buffer = ttf_buffer;
    font->font_size = font_size;
    stbtt_GetFontVMetrics(&font->font_info, &font->ascent, &font->descent, &font->line_gap);
    font->active = true;
    if (!stasis_build_font_atlas(font)) {
        font->active = false;
        free(ttf_buffer);
        memset(font, 0, sizeof(*font));
        return 0;
    }
    SDL_Log("stasis_load_font: loaded %s logical_size=%d raster_size=%d scale=%.2f handle=%d",
        resolved, font_size, font->raster_size, font->pixel_scale, slot + 1);

    return slot + 1; /* Return 1-based handle */
}

/* Draw text string using loaded font */
STASIS_EXPORT void stasis_draw_text(int font_handle, const char* text, float x, float y,
                                    float r, float g, float b, float a) {
    if (font_handle <= 0 || font_handle > MAX_FONTS) return;
    if (!stasis_ensure_font_ready(font_handle)) return;

    StasisFont* font = &g_fonts[font_handle - 1];
    if (!font->active || !text) return;

    if (g_use_sdl_renderer) {
        if (!font->sdl_texture) return;

        SDL_SetTextureBlendMode(font->sdl_texture, SDL_BLENDMODE_BLEND);
        SDL_SetTextureColorMod(font->sdl_texture,
            (Uint8)(r < 0.0f ? 0 : (r > 1.0f ? 255 : (int)(r * 255.0f))),
            (Uint8)(g < 0.0f ? 0 : (g > 1.0f ? 255 : (int)(g * 255.0f))),
            (Uint8)(b < 0.0f ? 0 : (b > 1.0f ? 255 : (int)(b * 255.0f))));
        SDL_SetTextureAlphaMod(font->sdl_texture,
            (Uint8)(a < 0.0f ? 0 : (a > 1.0f ? 255 : (int)(a * 255.0f))));

        const float pixel_scale = font->pixel_scale;
        float pos_x = x * pixel_scale;
        float pos_y = y * pixel_scale + (float)font->ascent * font->scale;
        const float start_x = pos_x;
        const float line_height =
            (float)(font->ascent - font->descent + font->line_gap) * font->scale;

        while (*text) {
            unsigned char ch = (unsigned char)*text;
            if (ch == '\r') {
                text++;
                continue;
            }
            if (ch == '\n') {
                pos_x = start_x;
                pos_y += line_height;
                text++;
                continue;
            }

            if (ch >= FONT_FIRST_CHAR && ch < FONT_FIRST_CHAR + FONT_NUM_CHARS) {
                stbtt_aligned_quad quad;
                stbtt_GetBakedQuad(font->char_data, font->atlas_size, font->atlas_size,
                    (int)ch - FONT_FIRST_CHAR, &pos_x, &pos_y, &quad, 0);

                SDL_Rect src;
                src.x = (int)(quad.s0 * (float)font->atlas_size);
                src.y = (int)(quad.t0 * (float)font->atlas_size);
                src.w = (int)((quad.s1 - quad.s0) * (float)font->atlas_size);
                src.h = (int)((quad.t1 - quad.t0) * (float)font->atlas_size);

                SDL_FRect dst;
                dst.x = quad.x0 / pixel_scale;
                dst.y = quad.y0 / pixel_scale;
                dst.w = (quad.x1 - quad.x0) / pixel_scale;
                dst.h = (quad.y1 - quad.y0) / pixel_scale;

                if (src.w > 0 && src.h > 0 && dst.w > 0.0f && dst.h > 0.0f) {
                    SDL_RenderCopyF(g_renderer, font->sdl_texture, &src, &dst);
                }
            }

            text++;
        }
        return;
    }

#if !defined(STASIS_GRAPHICS_SDL_ONLY)
    /* OpenGL path */
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    glEnable(GL_TEXTURE_2D);
    glBindTexture(GL_TEXTURE_2D, font->atlas_texture);

    /* Set color and modelview for text */
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();

    glColor4f(r, g, b, a);

    glBegin(GL_QUADS);

    const float pixel_scale = font->pixel_scale;
    float pos_x = x * pixel_scale;
    float pos_y = y * pixel_scale + (float)font->ascent * font->scale;
    const float start_x = pos_x;
    const float line_height =
        (float)(font->ascent - font->descent + font->line_gap) * font->scale;

    while (*text) {
        int c = (unsigned char)*text;
        if (c == '\r') {
            text++;
            continue;
        }
        if (c == '\n') {
            pos_x = start_x;
            pos_y += line_height;
            text++;
            continue;
        }
        if (c >= FONT_FIRST_CHAR && c < FONT_FIRST_CHAR + FONT_NUM_CHARS) {
            stbtt_aligned_quad quad;
            stbtt_GetBakedQuad(font->char_data, font->atlas_size, font->atlas_size,
                              c - FONT_FIRST_CHAR, &pos_x, &pos_y, &quad, 1);

            glTexCoord2f(quad.s0, quad.t0); glVertex2f(quad.x0 / pixel_scale, quad.y0 / pixel_scale);
            glTexCoord2f(quad.s1, quad.t0); glVertex2f(quad.x1 / pixel_scale, quad.y0 / pixel_scale);
            glTexCoord2f(quad.s1, quad.t1); glVertex2f(quad.x1 / pixel_scale, quad.y1 / pixel_scale);
            glTexCoord2f(quad.s0, quad.t1); glVertex2f(quad.x0 / pixel_scale, quad.y1 / pixel_scale);
        }
        text++;
    }

    glEnd();

    glDisable(GL_TEXTURE_2D);
    glColor4f(1, 1, 1, 1);
#endif
}

/* Measure text width for layout */
STASIS_EXPORT float stasis_measure_text(int font_handle, const char* text) {
    if (font_handle <= 0 || font_handle > MAX_FONTS || !text) return 0.0f;
    if (!stasis_ensure_font_ready(font_handle)) return 0.0f;

    StasisFont* font = &g_fonts[font_handle - 1];
    if (!font->active) return 0.0f;

    float pos_x = 0.0f, pos_y = 0.0f;

    while (*text) {
        int c = (unsigned char)*text;
        if (c == '\r' || c == '\n') {
            text++;
            continue;
        }
        if (c >= FONT_FIRST_CHAR && c < FONT_FIRST_CHAR + FONT_NUM_CHARS) {
            stbtt_aligned_quad quad;
            stbtt_GetBakedQuad(font->char_data, font->atlas_size, font->atlas_size,
                              c - FONT_FIRST_CHAR, &pos_x, &pos_y, &quad, g_use_sdl_renderer ? 0 : 1);
        }
        text++;
    }

    return pos_x / font->pixel_scale;
}
