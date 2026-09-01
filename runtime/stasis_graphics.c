/*
 * Stasis Graphics Runtime Library
 * SDL3 renderer for vector graphics rendering
 */

#include <SDL3/SDL.h>
#include <SDL3_image/SDL_image.h>

#include <stdbool.h>
#include <string.h>
#include "stasis_asset_path.h"
#include "stasis_audio_assets.h"
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <stdint.h>
#include <limits.h>
#include <ctype.h>
#include <errno.h>
#include <stdarg.h>
#include <time.h>
#if defined(__ANDROID__)
#include <android/log.h>
#endif
#include "stasis_render_contract.h"
#include "stasis_display_scale.h"
#include "stasis_renderer_lifecycle.h"
#include "stasis_performance_metrics.h"
#include "stasis_image_writer.h"
#include "stasis_sprite_atlas_policy.h"
#include "stasis_mixed_quad_planner.h"
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

#include "stasis_svg.h"

static void stasis_render_reset_clip(void);
static void stasis_render_push_clip(float x, float y, float w, float h);
static void stasis_render_pop_clip(void);

#ifndef STASIS_RELEASE_ID
#define STASIS_RELEASE_ID "development"
#endif

/* Set by the supported toolchain build. An empty value is intentionally not a
 * valid installed-toolchain identity. */
#ifndef STASIS_BUILD_FINGERPRINT
#define STASIS_BUILD_FINGERPRINT ""
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
    const char* asset_root = SDL_getenv("STASIS_ASSET_ROOT");
    const char* base = NULL;
    if (!asset_root || !*asset_root) {
        base = SDL_GetBasePath();
        asset_root = base;
    }
    if (!asset_root) return;
    char path[1024];
    size_t root_len = strlen(asset_root);
    const char* separator = root_len > 0 &&
        (asset_root[root_len - 1] == '/' || asset_root[root_len - 1] == '\\') ? "" : "/";
    int written = snprintf(
        path, sizeof(path), "%s%sstasis_provenance.json", asset_root, separator);
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

STASIS_EXPORT void stasis_host_log_message(const char* message) {
    if (message && *message) SDL_Log("%s", message);
}

STASIS_EXPORT void stasis_set_window_size(int width, int height);
STASIS_EXPORT int stasis_set_maximized(int maximized);
STASIS_EXPORT int stasis_get_time_us(void);
STASIS_EXPORT int stasis_load_font(const char* path, int font_size);
STASIS_EXPORT int stasis_gfx_cache_text(int font_handle, const char* text);
STASIS_EXPORT void stasis_gfx_draw_text_cached(int run_handle, float x, float y, float r, float g, float b, float a);
STASIS_EXPORT float stasis_gfx_measure_text_cached(int run_handle);
STASIS_EXPORT float stasis_gfx_measure_text_cached_height(int run_handle);
STASIS_EXPORT int stasis_clipboard_load_ascii(char* out, int capacity);
STASIS_EXPORT int stasis_clipboard_save_ascii(const char* value, int length);
STASIS_EXPORT int stasis_storage_load_ascii(const char* scope, const char* key, char* out, int capacity);
STASIS_EXPORT int stasis_storage_load_i32(const char* scope, const char* key, int fallback);
STASIS_EXPORT int stasis_storage_save_ascii(const char* scope, const char* key, const char* value, int length);
STASIS_EXPORT int stasis_storage_save_i32(const char* scope, const char* key, int value);

STASIS_EXPORT int stasis_graphics_runtime_abi_version(void) {
    return STASIS_GRAPHICS_RUNTIME_ABI_VERSION;
}

STASIS_EXPORT const char* stasis_graphics_release_id(void) {
    return STASIS_RELEASE_ID;
}

STASIS_EXPORT const char* stasis_graphics_build_fingerprint(void) {
    return STASIS_BUILD_FINGERPRINT;
}

/* Global state */
static SDL_SpinLock g_runtime_error_lock;
static char g_runtime_error[512];
static SDL_Window* g_window = NULL;
static SDL_Renderer* g_renderer = NULL;
static bool g_should_quit = false;
static const bool* g_keyboard_state = NULL;
static int g_window_width = 800;
static int g_window_height = 600;
static int g_native_window_width = 800;
static int g_native_window_height = 600;
static int g_drawable_width = 800;
static int g_drawable_height = 600;
static float g_pixel_scale = 1.0f;
static bool g_recording_presentation = false;
static bool g_x11_scale_controlled_window = false;
static int g_recording_width = 0;
static int g_recording_height = 0;
static uint32_t g_recording_fps = 0;
static bool g_recording_config_pending = false;
static StasisDisplayMetrics g_display_metrics;
static int g_display_generation = 0;
static int g_density_generation = 0;
static StasisDisplayPreparationScale g_density_preparation_scale = {0, 0};
static int g_available_width = 0;
static int g_available_height = 0;
static bool g_window_resized = false;
static bool g_window_minimized = false;
static uint32_t g_render_accepted_frames = 0;
static uint32_t g_render_rejected_frames = 0;
static uint32_t g_render_presented_frames = 0;
static uint32_t g_render_last_trace = 0;
static StasisRenderValidation g_render_last_validation = STASIS_RENDER_VALID;
static int32_t g_render_last_display_generation = 0;
static int32_t g_render_last_density_generation = 0;
static uint32_t g_render_logged_validation_mask = 0;
static bool g_render_contract_logged = false;
static int g_render_trace_enabled = -1;
static uint32_t g_native_draw_submissions = 0;
static uint32_t g_native_page_transitions = 0;
static uint32_t g_native_mixed_runs = 0;
static uint64_t g_native_submitted_bytes = 0;
typedef struct {
    int active;
    int logical_w;
    int logical_h;
    int native_w;
    int native_h;
    int drawable_w;
    int drawable_h;
    int available_w;
    int available_h;
    StasisDisplayViewport safe_native;
} StasisTestDisplayOverride;
static StasisTestDisplayOverride g_test_display_override;
static StasisRendererLifecycle g_resource_lifecycle;
static bool g_resource_frame_ready = false;
static bool g_force_debug_overlay = false;
static SDL_AtomicInt g_performance_metrics_requested;
static bool g_screenshot_taken = false;
static char g_screenshot_path[1024] = {0};
static int g_screenshot_exit_after = 0;
static int g_screenshot_frame = 1;

#define STASIS_PERF_SAMPLE_CAPACITY 1200
typedef struct {
    uint64_t captured_counter;
    StasisPerformanceMetrics metrics;
} StasisPerfSample;
static StasisPerfSample g_perf_samples[STASIS_PERF_SAMPLE_CAPACITY];
static int g_perf_sample_count = 0;
static int g_perf_sample_next = 0;
static uint64_t g_perf_pending_tick_us = 0;
static uint64_t g_perf_pending_guest_render_us = 0;
static uint64_t g_perf_render_started_counter = 0;
static SDL_SpinLock g_perf_metrics_lock;
static StasisPerformanceMetrics g_perf_latest_metrics;
static uint32_t g_perf_pending_lines = STASIS_PERF_UNAVAILABLE;
static uint32_t g_perf_pending_rectangles = STASIS_PERF_UNAVAILABLE;
static uint32_t g_perf_pending_sprites = STASIS_PERF_UNAVAILABLE;
static uint32_t g_perf_pending_text = STASIS_PERF_UNAVAILABLE;
static uint32_t g_perf_pending_commands = STASIS_PERF_UNAVAILABLE;
static const char* g_restore_label[] = {
    "   01110 11111 01110 01110 11111 01110   ",
    "   10001 00100 10001 10001 00100 10001   ",
    "   10000 00100 10001 10000 00100 10000   ",
    "   01110 00100 11111 01110 00100 01110   ",
    "   00001 00100 10001 00001 00100 00001   ",
    "   10001 00100 10001 10001 00100 10001   ",
    "   01110 00100 10001 01110 11111 01110   ",
    "                                         ",
    "10000 01110 01110 11110 11111 10001 01110",
    "10000 10001 10001 10001 00100 11001 10001",
    "10000 10001 10001 10001 00100 10101 10000",
    "10000 10001 11111 10001 00100 10011 10111",
    "10000 10001 10001 10001 00100 10001 10001",
    "10000 10001 10001 10001 00100 10001 10001",
    "11111 01110 10001 11110 11111 10001 01110"
};

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
static int8_t g_keyboard_event_state[SDL_SCANCODE_COUNT];
static float g_prev_x_px[STASIS_MAX_POINTERS];
static float g_prev_y_px[STASIS_MAX_POINTERS];
static SDL_FingerID g_finger_ids[STASIS_MAX_POINTERS - 1];
static int g_finger_active[STASIS_MAX_POINTERS - 1];
#if defined(__IPHONEOS__)
static bool g_ios_three_finger_latched = false;
#endif

/* Forward decls for exported functions used before their definitions (MSVC C mode does not allow implicit declarations). */
STASIS_EXPORT int stasis_get_time_ms(void);
STASIS_EXPORT int stasis_should_quit(void);
STASIS_EXPORT void stasis_host_get_frame(int32_t* out_i32, float* out_f32);
STASIS_EXPORT int stasis_host_performance_metrics_enabled(void);
STASIS_EXPORT void stasis_host_set_performance_metrics_enabled(int enabled);
STASIS_EXPORT int stasis_set_fullscreen(int fullscreen);
STASIS_EXPORT void stasis_gfx_draw_sprite(int handle, float x, float y, float w, float h, int rot_degrees, int a);
STASIS_EXPORT void stasis_gfx_release_sprite(int handle);
STASIS_EXPORT void stasis_audio_release(int asset_handle);
STASIS_EXPORT void stasis_gfx_submit_u8(int32_t* cmd_i32, const float* cmd_f32, const uint8_t* cmd_u8);
STASIS_EXPORT int stasis_test_get_render_submission_state(int32_t* out_i32, int32_t capacity);
STASIS_EXPORT int stasis_test_get_sprite_state(int32_t handle, int32_t* out_i32, int32_t capacity);
STASIS_EXPORT void stasis_draw_text(int font_handle, const char* text, float x, float y, float r, float g, float b, float a);

/* Forward decls for internal helpers used before their definitions. */
typedef struct SpriteEntry SpriteEntry;
static SpriteEntry* sprite_get(int handle);
static SpriteEntry* sprite_fallback_get(void);
static void stasis_gfx_draw_sprite_internal(int handle, float x, float y, float w, float h,
    float rot_degrees, uint32_t tint_rgba, float src_x, float src_y, float src_w, float src_h,
    float pivot_x, float pivot_y, float scale_x, float scale_y, int do_hash);
static int sprite_build_into_entry_sized(SpriteEntry* e, const char* path, int max_w, int max_h);
static void stasis_sync_display_metrics(void);
static void stasis_set_logical_size(int width, int height);
static void stasis_reset_text_cache(void);
static void stasis_invalidate_renderer_resources(int discard_gpu_handles);
static int stasis_restore_renderer_resources(void);
static void stasis_present_gpu_loading(void);
static void stasis_asset_tasks_shutdown(void);
static void stasis_sprite_atlas_reset(int destroy_textures);
static int stasis_draw_mixed_order_span(
    const int32_t* cmd_i32, const float* cmd_f32, int start, int order_count,
    int rect_count, int sprite_run_count);

/* Forward decls for helpers referenced early in the file (MSVC C mode does not allow implicit declarations). */
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
    StasisSpriteAtlasPolicyV3 atlas_policy;
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

#define STASIS_SDL_ATLAS_PAGE_SIZE 2048
#define STASIS_SDL_ATLAS_MAX_PAGES 256
#define STASIS_SDL_ATLAS_PADDING 1
#define STASIS_SDL_ATLAS_WHITE_SIZE 2
typedef struct {
    SDL_Texture* texture;
    int width;
    int height;
    int cursor_x;
    int cursor_y;
    int row_h;
    int white_x;
    int white_y;
    int placeholder_x;
    int placeholder_y;
    uint64_t group_id;
    int dedicated;
} StasisSdlAtlasPage;
static StasisSdlAtlasPage g_sprite_atlas_pages[STASIS_SDL_ATLAS_MAX_PAGES];
static int g_sprite_atlas_page_count = 0;
#if defined(_MSC_VER)
__declspec(thread) static StasisSpriteAtlasPolicyV3 g_next_sprite_atlas_policy_v3;
#else
static _Thread_local StasisSpriteAtlasPolicyV3 g_next_sprite_atlas_policy_v3;
#endif

STASIS_EXPORT void stasis_gfx_set_next_sprite_atlas_eligible(int eligible) {
    (void)eligible;
    g_next_sprite_atlas_policy_v3 = stasis_sprite_atlas_policy_v3_standalone();
}

STASIS_EXPORT void stasis_gfx_set_next_sprite_atlas_policy_v3(
    int eligible,
    uint64_t group_id,
    uint32_t member_count,
    uint64_t logical_pixel_area,
    uint32_t max_logical_width,
    uint32_t max_logical_height) {
    g_next_sprite_atlas_policy_v3 = stasis_sprite_atlas_policy_v3_make(
        eligible,
        group_id,
        member_count,
        logical_pixel_area,
        max_logical_width,
        max_logical_height);
}

#define STASIS_ASSET_TASK_CAPACITY 64
#define STASIS_ASSET_TASK_NONE 0
#define STASIS_ASSET_TASK_PENDING 1
#define STASIS_ASSET_TASK_LOADING 2
#define STASIS_ASSET_TASK_LOADED 3
#define STASIS_ASSET_TASK_FAILED 4
#define STASIS_ASSET_TASK_CANCELLED 5
#define STASIS_ASSET_TASK_DECODED 6
#define STASIS_ASSET_TASK_PUBLISHING 7
#define STASIS_ASSET_KIND_SPRITE 1
#define STASIS_ASSET_KIND_AUDIO 2

typedef struct {
    int id;
    int kind;
    int state;
    int handle;
    int release_requested;
    char path[1024];
    int max_w;
    int max_h;
    StasisSpriteAtlasPolicyV3 atlas_policy;
    int raster_w;
    int raster_h;
    unsigned char* pixels;
    int pixel_w;
    int pixel_h;
    StasisDecodedAudio audio;
} StasisAssetTask;

static StasisAssetTask g_asset_tasks[STASIS_ASSET_TASK_CAPACITY];
static SDL_Mutex* g_asset_task_mutex;
static SDL_Condition* g_asset_task_condition;
static SDL_Thread* g_asset_task_thread;
static int g_asset_task_stop;
static int g_asset_task_next_id = 1;

/* Font rendering with stb_truetype. */
#define MAX_FONTS 32
#define FONT_FIRST_CHAR 32
#define FONT_NUM_CHARS 95

typedef struct {
    bool active;
    stbtt_fontinfo font_info;
    unsigned char* ttf_buffer;
    float scale;
    int ascent, descent, line_gap;

    /* Baked bitmap atlas */
    SDL_Texture* sdl_texture;
    stbtt_bakedchar char_data[FONT_NUM_CHARS];
    int font_size;       /* requested logical pixel height */
    int raster_size;     /* density-scaled pixel height */
    int atlas_size;
    float pixel_scale;
    int needs_reraster;
    uint32_t surface_generation;
    uint32_t renderer_generation;
    char source_path[1024];
    uint64_t source_size;
} StasisFont;

static StasisFont g_fonts[MAX_FONTS];

static void stasis_release_font(StasisFont* font) {
    if (!font) return;
    if (font->sdl_texture) {
        SDL_DestroyTexture(font->sdl_texture);
        font->sdl_texture = NULL;
    }
    if (font->ttf_buffer) {
        free(font->ttf_buffer);
        font->ttf_buffer = NULL;
    }
    memset(font, 0, sizeof(*font));
}

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
    stasis_sprite_atlas_reset(!discard_gpu_handles);
    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* entry = &g_sprites[i];
        if (!entry->used) continue;
        entry->sdl_tex = NULL;
        entry->needs_reraster = 1;
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

static void stasis_present_gpu_loading(void) {
    if (!g_window) return;
    const int rows = (int)(sizeof(g_restore_label) / sizeof(g_restore_label[0]));
    const int columns = (int)strlen(g_restore_label[0]);
    int cell_w = g_window_width / (columns + 8);
    int cell_h = g_window_height / 60;
    int cell = cell_w < cell_h ? cell_w : cell_h;
    if (cell < 2) cell = 2;
    const int origin_x = (g_window_width - columns * cell) / 2;
    const int origin_y = (g_window_height - rows * cell) / 2;

    if (g_renderer) {
        SDL_SetRenderTarget(g_renderer, NULL);
        SDL_SetRenderLogicalPresentation(
            g_renderer, g_window_width, g_window_height,
            SDL_LOGICAL_PRESENTATION_LETTERBOX);
        SDL_SetRenderClipRect(g_renderer, NULL);
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_NONE);
        SDL_SetRenderDrawColor(g_renderer, 15, 20, 28, 255);
        SDL_RenderClear(g_renderer);
        SDL_SetRenderDrawColor(g_renderer, 66, 153, 225, 255);
        for (int row = 0; row < rows; row++) {
            const char* pixels = g_restore_label[row];
            for (int column = 0; column < columns; column++) {
                if (pixels[column] != '1') continue;
                SDL_FRect pixel = {
                    (float)(origin_x + column * cell),
                    (float)(origin_y + row * cell),
                    (float)cell,
                    (float)cell
                };
                SDL_RenderFillRect(g_renderer, &pixel);
            }
        }
        SDL_RenderPresent(g_renderer);
    }
    else {
        return;
    }
    SDL_Log("Stasis renderer loading screen presented: backend=%s reason=%s surface_generation=%u renderer_generation=%u",
        "sdl",
        stasis_renderer_reason_name(g_resource_lifecycle.reason),
        g_resource_lifecycle.surface_generation,
        g_resource_lifecycle.renderer_generation);
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

static int stasis_current_scaled_extent(int logical_extent) {
    return stasis_display_scaled_extent_for_backing(
        logical_extent,
        g_window_width, g_window_height,
        g_drawable_width, g_drawable_height);
}

static SDL_DisplayID stasis_select_presentation_display(
    SDL_DisplayID window_display, SDL_DisplayID primary_display) {
    return window_display != 0 ? window_display : primary_display;
}

static float stasis_x11_window_scale(void) {
#if defined(__linux__) && !defined(__ANDROID__)
    const char* driver = SDL_GetCurrentVideoDriver();
    if (!driver || strcmp(driver, "x11") != 0) return 1.0f;
    float scale = g_window
        ? SDL_GetWindowDisplayScale(g_window)
        : SDL_GetDisplayContentScale(SDL_GetPrimaryDisplay());
    if (!isfinite(scale) || scale < 1.0f) return 1.0f;
    return scale > 8.0f ? 8.0f : scale;
#else
    return 1.0f;
#endif
}

static bool stasis_x11_scale_controlled_launch(void) {
#if defined(__linux__) && !defined(__ANDROID__)
    const char* driver = SDL_GetCurrentVideoDriver();
    return driver && strcmp(driver, "x11") == 0 &&
        stasis_display_scale_control_is_valid(
            SDL_getenv("SDL_VIDEO_X11_SCALING_FACTOR"));
#else
    return false;
#endif
}

static void stasis_apply_x11_window_scale(int explicit_window_request) {
    if (!g_window || g_recording_presentation) return;
    const SDL_WindowFlags flags = SDL_GetWindowFlags(g_window);
    if (!stasis_display_should_apply_windowed_extent(
            explicit_window_request,
            (flags & SDL_WINDOW_FULLSCREEN) != 0,
            (flags & SDL_WINDOW_MAXIMIZED) != 0,
            (flags & SDL_WINDOW_MINIMIZED) != 0)) {
        return;
    }
    const float scale = stasis_x11_window_scale();
    SDL_SetWindowSize(
        g_window,
        stasis_display_scaled_window_extent(g_window_width, scale),
        stasis_display_scaled_window_extent(g_window_height, scale));
    SDL_SyncWindow(g_window);
}

static void stasis_query_available_presentation(
    int fallback_w, int fallback_h, int* width, int* height) {
    int available_w = 0;
    int available_h = 0;
    if (SDL_WasInit(SDL_INIT_VIDEO) != 0) {
        const SDL_DisplayID window_display =
            g_window ? SDL_GetDisplayForWindow(g_window) : 0;
        const SDL_DisplayID display = stasis_select_presentation_display(
            window_display, SDL_GetPrimaryDisplay());
        SDL_Rect bounds;
        if (display != 0 && SDL_GetDisplayUsableBounds(display, &bounds)) {
            available_w = bounds.w;
            available_h = bounds.h;
        } else if (display != 0) {
            const SDL_DisplayMode* mode = SDL_GetDesktopDisplayMode(display);
            if (mode) {
                available_w = mode->w;
                available_h = mode->h;
            }
        }
    }
    if (available_w <= 0) available_w = fallback_w > 0 ? fallback_w : 1;
    if (available_h <= 0) available_h = fallback_h > 0 ? fallback_h : 1;
    if (width) *width = available_w;
    if (height) *height = available_h;
}

static void stasis_sync_display_metrics(void) {
    if (!g_window) return;

    int native_w = g_native_window_width;
    int native_h = g_native_window_height;
    if (g_test_display_override.active) {
        g_window_width = g_test_display_override.logical_w;
        g_window_height = g_test_display_override.logical_h;
        native_w = g_test_display_override.native_w;
        native_h = g_test_display_override.native_h;
    } else {
        SDL_GetWindowSize(g_window, &native_w, &native_h);
    }
    if (native_w > 0) g_native_window_width = native_w;
    if (native_h > 0) g_native_window_height = native_h;

    int drawable_w = native_w;
    int drawable_h = native_h;
    if (g_test_display_override.active) {
        drawable_w = g_test_display_override.drawable_w;
        drawable_h = g_test_display_override.drawable_h;
        if (g_renderer) {
            SDL_SetRenderLogicalPresentation(
                g_renderer, g_window_width, g_window_height,
                SDL_LOGICAL_PRESENTATION_LETTERBOX);
        }
    } else if (g_renderer) {
        /* The display contract owns the complete renderer backing here.  The
         * "current" output is adjusted by SDL logical presentation and can
         * still describe the previous fitted viewport while a logical canvas
         * change is being applied. */
        if (!SDL_GetRenderOutputSize(g_renderer, &drawable_w, &drawable_h)) {
            drawable_w = native_w;
            drawable_h = native_h;
        }
        SDL_SetRenderLogicalPresentation(
            g_renderer, g_window_width, g_window_height,
            SDL_LOGICAL_PRESENTATION_LETTERBOX);
    }

    if (drawable_w <= 0) drawable_w = g_window_width;
    if (drawable_h <= 0) drawable_h = g_window_height;
    int available_w = 0;
    int available_h = 0;
    if (g_test_display_override.active) {
        available_w = g_test_display_override.available_w;
        available_h = g_test_display_override.available_h;
    } else {
        stasis_query_available_presentation(
            g_native_window_width, g_native_window_height,
            &available_w, &available_h);
    }
    StasisDisplayViewport safe_native = {
        0.0f, 0.0f, (float)g_native_window_width, (float)g_native_window_height};
    StasisDisplayMetrics next = stasis_display_metrics(
        g_window_width, g_window_height,
        g_native_window_width, g_native_window_height,
        drawable_w, drawable_h, safe_native);
    const StasisDisplayPreparationScale next_preparation_scale =
        stasis_display_preparation_scale(
            next.logical_w, next.logical_h, next.drawable_w, next.drawable_h);
    const int dimensions_changed =
        next.native_w != g_display_metrics.native_w ||
        next.native_h != g_display_metrics.native_h ||
        next.drawable_w != g_display_metrics.drawable_w ||
        next.drawable_h != g_display_metrics.drawable_h ||
        next.logical_w != g_display_metrics.logical_w ||
        next.logical_h != g_display_metrics.logical_h ||
        available_w != g_available_width ||
        available_h != g_available_height;
    const int density_changed = g_density_generation == 0 ||
        stasis_display_preparation_scale_changed(
            g_density_preparation_scale, next_preparation_scale);
    if (density_changed) {
        g_pixel_scale = next.raster_scale;
        g_density_preparation_scale = next_preparation_scale;
        stasis_mark_density_resources_dirty();
    }
    if (g_display_generation == 0 || dimensions_changed) {
        g_display_generation++;
        g_window_resized = true;
    }
    g_display_metrics = next;
    g_available_width = available_w;
    g_available_height = available_h;
    g_drawable_width = drawable_w;
    g_drawable_height = drawable_h;
}


static void stasis_window_to_logical(float native_x, float native_y, float* logical_x, float* logical_y) {
    if (!logical_x || !logical_y) return;
    stasis_display_native_to_logical_xy(
        &g_display_metrics, native_x, native_y, logical_x, logical_y);
}

/*
 * Supply platform readings to the real SDL window-event path for native seam
 * tests. HostFrame and gfx_cmd buffers remain owned by their production writers.
 */
STASIS_EXPORT int stasis_test_push_display_event(
    int kind,
    int logical_w,
    int logical_h,
    int native_w,
    int native_h,
    int drawable_w,
    int drawable_h,
    int available_w,
    int available_h,
    int safe_x,
    int safe_y,
    int safe_w,
    int safe_h) {
    const char* enabled = SDL_getenv("STASIS_ENABLE_TEST_INPUT");
    if (!g_window || !enabled || enabled[0] != '1' || enabled[1] != '\0' ||
        logical_w <= 0 || logical_h <= 0 || native_w <= 0 || native_h <= 0 ||
        drawable_w <= 0 || drawable_h <= 0 || available_w <= 0 ||
        available_h <= 0 || safe_x < 0 || safe_y < 0 ||
        safe_w <= 0 || safe_h <= 0 || safe_x + safe_w > native_w ||
        safe_y + safe_h > native_h || kind < 1 || kind > 3) {
        return 0;
    }

    g_test_display_override.active = 1;
    g_test_display_override.logical_w = logical_w;
    g_test_display_override.logical_h = logical_h;
    g_test_display_override.native_w = native_w;
    g_test_display_override.native_h = native_h;
    g_test_display_override.drawable_w = drawable_w;
    g_test_display_override.drawable_h = drawable_h;
    g_test_display_override.available_w = available_w;
    g_test_display_override.available_h = available_h;
    g_test_display_override.safe_native = (StasisDisplayViewport){
        (float)safe_x, (float)safe_y, (float)safe_w, (float)safe_h};

    SDL_Event event;
    SDL_zero(event);
    event.type = kind == 1 ? SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED :
        (kind == 2 ? SDL_EVENT_WINDOW_MINIMIZED : SDL_EVENT_WINDOW_RESTORED);
    event.window.windowID = SDL_GetWindowID(g_window);
    return SDL_PushEvent(&event) ? 1 : 0;
}

STASIS_EXPORT uint32_t stasis_test_select_presentation_display(
    uint32_t window_display, uint32_t primary_display) {
    const char* enabled = SDL_getenv("STASIS_ENABLE_TEST_INPUT");
    if (!enabled || enabled[0] != '1' || enabled[1] != '\0') return 0;
    return (uint32_t)stasis_select_presentation_display(
        (SDL_DisplayID)window_display, (SDL_DisplayID)primary_display);
}

/*
 * Native integration-test input enters through SDL's event queue. The explicit
 * environment gate prevents shipped applications from synthesizing host input.
 */
STASIS_EXPORT int stasis_test_push_input_event(
    int kind, int code, float logical_x, float logical_y) {
    const char* enabled = SDL_getenv("STASIS_ENABLE_TEST_INPUT");
    if (!g_window || !enabled || enabled[0] != '1' || enabled[1] != '\0') return 0;

    SDL_Event event;
    SDL_zero(event);
    if (kind == 1 || kind == 2) {
        if (code < 0 || code >= SDL_SCANCODE_COUNT) return 0;
        event.type = kind == 1 ? SDL_EVENT_KEY_DOWN : SDL_EVENT_KEY_UP;
        event.key.scancode = (SDL_Scancode)code;
        event.key.repeat = false;
    } else if (kind >= 3 && kind <= 5) {
        StasisDisplayMetrics input_metrics = g_display_metrics;
        if (g_test_display_override.active) {
            input_metrics = stasis_display_metrics(
                g_test_display_override.logical_w,
                g_test_display_override.logical_h,
                g_test_display_override.native_w,
                g_test_display_override.native_h,
                g_test_display_override.drawable_w,
                g_test_display_override.drawable_h,
                g_test_display_override.safe_native);
        }
        if (code < 0 || input_metrics.native_w <= 0 || input_metrics.native_h <= 0) {
            return 0;
        }
        float native_x = 0.0f;
        float native_y = 0.0f;
        stasis_display_logical_to_native_xy(
            &input_metrics, logical_x, logical_y, &native_x, &native_y);
        event.type = kind == 3 ? SDL_EVENT_FINGER_DOWN :
            (kind == 4 ? SDL_EVENT_FINGER_MOTION : SDL_EVENT_FINGER_UP);
        event.tfinger.touchID = 1;
        event.tfinger.fingerID = (SDL_FingerID)code;
        event.tfinger.x = native_x / (float)input_metrics.native_w;
        event.tfinger.y = native_y / (float)input_metrics.native_h;
    } else {
        return 0;
    }
    return SDL_PushEvent(&event) ? 1 : 0;
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

    if (g_test_display_override.active) {
        safe_native = g_test_display_override.safe_native;
        goto publish;
    }

    SDL_DisplayID display = SDL_GetDisplayForWindow(g_window);
    if (display == 0) goto publish;

    SDL_Rect usable;
    if (!SDL_GetDisplayUsableBounds(display, &usable)) {
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

#if defined(__IPHONEOS__)
static int stasis_ios_active_finger_count(void) {
    int active_fingers = 0;
    for (int finger = 0; finger < STASIS_MAX_POINTERS - 1; finger++) {
        if (g_finger_active[finger]) active_fingers++;
    }
    return active_fingers;
}
#endif

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
    memset(g_keyboard_event_state, -1, sizeof(g_keyboard_event_state));
    g_input_frame.viewport_x_px = 0;
    g_input_frame.viewport_y_px = 0;
    g_input_frame.viewport_w_px = g_window_width;
    g_input_frame.viewport_h_px = g_window_height;
    stasis_update_safe_viewport();

    SDL_Event event;
    while (SDL_PollEvent(&event)) {
        switch (event.type) {
            case SDL_EVENT_QUIT:
                SDL_Log("Stasis quit requested: SDL_EVENT_QUIT");
                g_should_quit = true;
                break;
            case SDL_EVENT_KEY_DOWN:
                if (event.key.scancode >= 0 && event.key.scancode < SDL_SCANCODE_COUNT) {
                    g_keyboard_event_state[event.key.scancode] = 1;
                }
                if (event.key.key == SDLK_ESCAPE) {
                    SDL_Log("Stasis quit requested: Escape key");
                    g_should_quit = true;
                }
                if (event.key.key == SDLK_F3 && !event.key.repeat) {
                    g_force_debug_overlay = !g_force_debug_overlay;
                    g_perf_sample_count = 0;
                    g_perf_sample_next = 0;
                    g_perf_render_started_counter = 0;
                    stasis_host_set_performance_metrics_enabled(
                        g_force_debug_overlay ? 1 : 0);
                    SDL_Log("performance HUD %s (F3 toggles)", g_force_debug_overlay ? "on" : "off");
                }
                break;
            case SDL_EVENT_KEY_UP:
                if (event.key.scancode >= 0 && event.key.scancode < SDL_SCANCODE_COUNT) {
                    g_keyboard_event_state[event.key.scancode] = 0;
                }
                break;
            case SDL_EVENT_WINDOW_RESIZED:
            case SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED:
                stasis_sync_display_metrics();
                g_input_frame.viewport_w_px = g_window_width;
                g_input_frame.viewport_h_px = g_window_height;
                stasis_update_safe_viewport();
                break;
            case SDL_EVENT_WINDOW_DISPLAY_SCALE_CHANGED:
#if defined(__linux__) && !defined(__ANDROID__)
                stasis_apply_x11_window_scale(0);
#endif
                stasis_sync_display_metrics();
                g_input_frame.viewport_w_px = g_window_width;
                g_input_frame.viewport_h_px = g_window_height;
                stasis_update_safe_viewport();
                break;
            case SDL_EVENT_WINDOW_MINIMIZED:
                g_window_minimized = true;
                break;
            case SDL_EVENT_WINDOW_RESTORED:
                g_window_minimized = false;
                stasis_sync_display_metrics();
                stasis_update_safe_viewport();
                break;
            case SDL_EVENT_RENDER_TARGETS_RESET:
                stasis_renderer_lifecycle_renderer_reset(
                    &g_resource_lifecycle, STASIS_RENDERER_REASON_TARGETS_RESET);
                stasis_invalidate_renderer_resources(0);
                stasis_present_gpu_loading();
                SDL_Log("Stasis renderer resources invalidated: backend=sdl reason=targets_reset surface_generation=%u renderer_generation=%u",
                    g_resource_lifecycle.surface_generation,
                    g_resource_lifecycle.renderer_generation);
                break;
            case SDL_EVENT_RENDER_DEVICE_RESET:
                stasis_renderer_lifecycle_renderer_reset(
                    &g_resource_lifecycle, STASIS_RENDERER_REASON_DEVICE_RESET);
                stasis_invalidate_renderer_resources(0);
                stasis_present_gpu_loading();
                SDL_Log("Stasis renderer resources invalidated: backend=sdl reason=device_reset surface_generation=%u renderer_generation=%u",
                    g_resource_lifecycle.surface_generation,
                    g_resource_lifecycle.renderer_generation);
                break;
            case SDL_EVENT_WILL_ENTER_BACKGROUND:
                stasis_renderer_lifecycle_pause(&g_resource_lifecycle);
                g_resource_frame_ready = false;
                break;
            case SDL_EVENT_DID_ENTER_FOREGROUND:
                if (g_resource_lifecycle.state == STASIS_RENDERER_PAUSED) {
                    stasis_renderer_lifecycle_resume(&g_resource_lifecycle);
                }
                break;
            case SDL_EVENT_MOUSE_BUTTON_DOWN:
                if (event.button.button == SDL_BUTTON_LEFT) {
                    g_input_frame.pointers[0].went_down = 1;
                }
                break;
            case SDL_EVENT_MOUSE_BUTTON_UP:
                if (event.button.button == SDL_BUTTON_LEFT) {
                    g_input_frame.pointers[0].went_up = 1;
                }
                break;
            case SDL_EVENT_FINGER_DOWN:
                {
                    int slot = stasis_alloc_finger_slot(event.tfinger.fingerID);
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
#if defined(__IPHONEOS__)
                    if (stasis_ios_active_finger_count() >= 3 && !g_ios_three_finger_latched) {
                        g_ios_three_finger_latched = true;
                        g_force_debug_overlay = !g_force_debug_overlay;
                        g_perf_sample_count = 0;
                        g_perf_sample_next = 0;
                        g_perf_render_started_counter = 0;
                        stasis_host_set_performance_metrics_enabled(
                            g_force_debug_overlay ? 1 : 0);
                    }
#endif
                }
                break;
            case SDL_EVENT_FINGER_MOTION:
                {
                    int slot = stasis_find_finger_slot(event.tfinger.fingerID);
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
            case SDL_EVENT_FINGER_UP:
                {
                    int slot = stasis_find_finger_slot(event.tfinger.fingerID);
                    if (slot < 0) break;
                    int idx = slot + 1;
                    g_input_frame.pointers[idx].is_down = 0;
                    g_input_frame.pointers[idx].went_up = 1;
                    stasis_release_finger_slot(event.tfinger.fingerID);
#if defined(__IPHONEOS__)
                    if (stasis_ios_active_finger_count() < 3) g_ios_three_finger_latched = false;
#endif
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
    float mx = 0.0f, my = 0.0f;
    SDL_MouseButtonFlags buttons = SDL_GetMouseState(&mx, &my);
    float logical_mouse_x = 0.0f;
    float logical_mouse_y = 0.0f;
    stasis_window_to_logical(mx, my, &logical_mouse_x, &logical_mouse_y);
    stasis_set_pointer_pos_px(0, logical_mouse_x, logical_mouse_y);
    g_input_frame.pointers[0].is_down =
        (buttons & SDL_BUTTON_MASK(SDL_BUTTON_LEFT)) ? 1 : 0;

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

STASIS_EXPORT float stasis_input_pointer_x_logical(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].x_px;
}

STASIS_EXPORT float stasis_input_pointer_y_logical(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].y_px;
}

STASIS_EXPORT float stasis_input_pointer_dx_logical(int idx) {
    if (!g_window || !stasis_input_valid_index(idx)) return 0.0f;
    return g_input_frame.pointers[idx].dx_px;
}

STASIS_EXPORT float stasis_input_pointer_dy_logical(int idx) {
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
 * - HostFrame layout is defined in src/stdlib/internal/host_frame.stasis and is written by stasis_host_get_frame().
 * - Rendering is driven by gfx_cmd buffers (src/stdlib/internal/gfx_cmd.stasis) and submitted by stasis_gfx_submit_u8().
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
    const int32_t HOST_REQ_FLAG_MAXIMIZED = 4;

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
#if !defined(__ANDROID__) && !defined(__IPHONEOS__)
            (void)stasis_set_fullscreen(0);
#endif
            stasis_set_window_size(*host_req_window_w_px, *host_req_window_h_px);
        }
    }
    else if ((flags & HOST_REQ_FLAG_FULLSCREEN) != 0)
    {
        (void)stasis_set_fullscreen(1);
    }
    else if ((flags & HOST_REQ_FLAG_MAXIMIZED) != 0)
    {
        if (host_req_window_w_px && host_req_window_h_px)
        {
            stasis_set_logical_size(*host_req_window_w_px, *host_req_window_h_px);
        }
        (void)stasis_set_maximized(1);
    }
}

STASIS_EXPORT void stasis_host_set_performance_metrics(uint64_t tick_us, uint64_t render_us)
{
    if (!stasis_host_performance_metrics_enabled()) return;
    g_perf_pending_tick_us = tick_us;
    g_perf_pending_guest_render_us = render_us;
}

STASIS_EXPORT int stasis_host_performance_metrics_enabled(void)
{
    return (g_force_debug_overlay || SDL_GetAtomicInt(&g_performance_metrics_requested) != 0)
        ? 1
        : 0;
}

STASIS_EXPORT void stasis_host_set_performance_metrics_enabled(int enabled)
{
    SDL_SetAtomicInt(&g_performance_metrics_requested, enabled != 0);
    SDL_LockSpinlock(&g_perf_metrics_lock);
    memset(&g_perf_latest_metrics, 0, sizeof(g_perf_latest_metrics));
    g_perf_latest_metrics.version = STASIS_PERF_METRICS_VERSION;
    g_perf_latest_metrics.size = (uint32_t)sizeof(g_perf_latest_metrics);
    g_perf_latest_metrics.tick_us = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.guest_render_us = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.host_replay_us = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.render_prep_us = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.gpu_submit_us = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.gpu_execution_us = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.frame_work_us = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.present_wait_us = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.commands = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.lines = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.rectangles = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.sprites = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.text = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.instances = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.batches = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.draw_calls = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.texture_switches = STASIS_PERF_UNAVAILABLE;
    g_perf_latest_metrics.uploaded_bytes = STASIS_PERF_UNAVAILABLE;
    snprintf(g_perf_latest_metrics.backend, sizeof(g_perf_latest_metrics.backend), "%s",
        "SDL");
    SDL_UnlockSpinlock(&g_perf_metrics_lock);
}

STASIS_EXPORT void stasis_host_report_runtime_error(const char* message)
{
    if (!message || !*message) return;
    SDL_LockSpinlock(&g_runtime_error_lock);
    snprintf(g_runtime_error, sizeof(g_runtime_error), "%s", message);
    SDL_UnlockSpinlock(&g_runtime_error_lock);
}

STASIS_EXPORT int stasis_host_copy_runtime_error(char* output, size_t output_size)
{
    if (!output || output_size == 0) return 0;
    SDL_LockSpinlock(&g_runtime_error_lock);
    int has_error = g_runtime_error[0] != '\0';
    snprintf(output, output_size, "%s", g_runtime_error);
    SDL_UnlockSpinlock(&g_runtime_error_lock);
    return has_error;
}

static void stasis_report_runtime_errorf(const char* format, ...)
{
    char message[512];
    va_list args;
    va_start(args, format);
    vsnprintf(message, sizeof(message), format, args);
    va_end(args);
    stasis_host_report_runtime_error(message);
}

STASIS_EXPORT void stasis_host_get_latest_performance_metrics(
    uint32_t* tick_us,
    uint32_t* render_us)
{
    StasisPerformanceMetrics metrics;
    SDL_LockSpinlock(&g_perf_metrics_lock);
    metrics = g_perf_latest_metrics;
    SDL_UnlockSpinlock(&g_perf_metrics_lock);
    if (tick_us) {
        *tick_us = metrics.tick_us;
    }
    if (render_us) {
        *render_us = metrics.host_replay_us == STASIS_PERF_UNAVAILABLE
            ? metrics.guest_render_us
            : metrics.guest_render_us + metrics.host_replay_us;
    }
}

/* Additive, capacity-checked snapshot API. The original two-value function above
 * remains ABI-compatible for older shells and tools. */
STASIS_EXPORT int stasis_host_get_latest_performance_metrics_v1(
    StasisPerformanceMetrics* output, size_t capacity)
{
    if (!output || capacity < sizeof(StasisPerformanceMetrics)) return 0;
    SDL_LockSpinlock(&g_perf_metrics_lock);
    *output = g_perf_latest_metrics;
    SDL_UnlockSpinlock(&g_perf_metrics_lock);
    return 1;
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

    const int measure_frame = stasis_host_performance_metrics_enabled();
    const uint64_t tick_started = measure_frame ? SDL_GetPerformanceCounter() : 0;
    const int tick_result = tick_fn();
    const uint64_t tick_finished = measure_frame ? SDL_GetPerformanceCounter() : 0;
    if (tick_result != 0)
    {
        return tick_result;
    }

    if (measure_frame) {
        stasis_host_set_performance_metrics(
            stasis_perf_elapsed_us(tick_started, tick_finished),
            0);
    }
    stasis_gfx_submit_u8(gfx_cmd_i32, gfx_cmd_f32, gfx_cmd_u8);
    return 0;
}

/*
 * Host snapshot: fill caller-provided buffers with a deterministic view of host state.
 *
 * Layout is defined in src/stdlib/internal/host_frame.stasis. This is intentionally a simple
 * "copy out" ABI for native now, and a good fit for WASM later (one import to get a snapshot).
 */
STASIS_EXPORT void stasis_host_get_frame(int32_t* out_i32, float* out_f32) {
    if (!out_i32 || !out_f32) return;

    static int32_t g_host_tick_index = 0;
    const int32_t host_version = 4;
    const int i32_key_base = 32;
    const int i32_key_count = 512;
    const int should_quit = stasis_should_quit();

    /* i32 header */
    out_i32[0] = stasis_get_time_ms();
    for (int index = 1; index <= 6; index++) out_i32[index] = 0;
    out_i32[7] = g_input_frame.pointer_count;
    out_i32[8] = g_input_frame.dropped_pointers;
    out_i32[9] = should_quit;

    out_i32[10] = g_host_tick_index++;

    out_i32[11] = g_window_resized ? 1 : 0;
    g_window_resized = false;

    out_i32[12] = g_available_width;
    out_i32[13] = g_available_height;

    /* vNext */
    out_i32[14] = host_version;

    int32_t flags = 0;
    if (out_i32[9] != 0) flags |= 1; /* quit requested */
    if (out_i32[11] != 0) flags |= 8; /* resized */

    int32_t focused = 0;
    int32_t minimized = g_window_minimized ? 1 : 0;
    if (g_window) {
        const Uint32 wf = SDL_GetWindowFlags(g_window);
        focused = ((wf & SDL_WINDOW_INPUT_FOCUS) != 0) ? 1 : 0;
        if (focused) flags |= 2;
        if (minimized) flags |= 4;
    }

    out_i32[15] = flags;
    out_i32[16] = 0; /* tick_hz: unknown */
    out_i32[17] = focused;
    out_i32[18] = minimized;
    out_i32[19] = stasis_get_time_us();

    out_i32[20] = 0;
    out_i32[21] = 0;
    out_i32[22] = g_display_metrics.native_w;
    out_i32[23] = g_display_metrics.native_h;
    out_i32[24] = g_display_metrics.drawable_w;
    out_i32[25] = g_display_metrics.drawable_h;
    for (int index = 26; index <= 29; index++) out_i32[index] = 0;
    out_i32[30] = g_display_generation;
    out_i32[31] = g_density_generation;

    /* Keyboard state: one i32 per scancode (0/1). */
    int num_keys = 0;
    const bool* keys = SDL_GetKeyboardState(&num_keys);
    for (int i = 0; i < i32_key_count; i++) {
        out_i32[i32_key_base + i] = g_keyboard_event_state[i] >= 0
            ? g_keyboard_event_state[i]
            : ((keys && i < num_keys && keys[i]) ? 1 : 0);
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
    out_f32[50] = (float)g_display_metrics.logical_w;
    out_f32[51] = (float)g_display_metrics.logical_h;
    out_f32[52] = g_display_metrics.safe_logical_viewport.x;
    out_f32[53] = g_display_metrics.safe_logical_viewport.y;
    out_f32[54] = g_display_metrics.safe_logical_viewport.w;
    out_f32[55] = g_display_metrics.safe_logical_viewport.h;
    out_f32[56] = (float)g_available_width;
    out_f32[57] = (float)g_available_height;
}

/* ============================================================
 * Audio output (SDL3) - f32 stereo ring buffer feeding a device stream
 * ============================================================ */

static SDL_AudioStream* g_audio_stream = NULL;
static int g_audio_initialized = 0;
static int g_audio_underruns = 0;
static int g_audio_channels = 2;
static int g_audio_sample_rate = 48000;
static int g_audio_target_latency_frames = 2048;
#define STASIS_AUDIO_MAX_TARGET_LATENCY_FRAMES (1 << 20)
static float* g_audio_ring = NULL;
static int g_audio_ring_capacity_frames = 0;
static int g_audio_ring_capacity_samples = 0;
static int g_audio_read_sample = 0;
static int g_audio_write_sample = 0;
static int g_audio_queued_samples = 0;
static int64_t g_audio_running_frame_index = 0;
static int g_recording_audio_enabled = 0;

static StasisAudioAssetStore g_audio_assets;

static int resolve_asset_path(const char* path, char* out, size_t out_size);

static int stasis_audio_has_active_voice(void) {
    return stasis_audio_assets_has_active_voice(&g_audio_assets);
}

static int stasis_audio_maxi(int a, int b) { return a > b ? a : b; }
static int stasis_audio_mini(int a, int b) { return a < b ? a : b; }

static int stasis_audio_ensure_ring_init(void) {
    if (g_audio_assets.next_asset_handle <= 0 || g_audio_assets.next_voice_handle <= 0) {
        stasis_audio_assets_reset(&g_audio_assets);
    }
    if (g_audio_ring) return 1;

    g_audio_channels = 2;
    g_audio_target_latency_frames = stasis_audio_maxi(512, g_audio_target_latency_frames);
    g_audio_ring_capacity_frames = stasis_audio_maxi(8192, g_audio_target_latency_frames * 4);
    g_audio_ring_capacity_samples = g_audio_ring_capacity_frames * g_audio_channels;
    g_audio_ring = (float*)malloc((size_t)g_audio_ring_capacity_samples * sizeof(float));
    if (!g_audio_ring) {
        g_audio_ring_capacity_frames = 0;
        g_audio_ring_capacity_samples = 0;
        return 0;
    }
    SDL_memset(g_audio_ring, 0, (size_t)g_audio_ring_capacity_samples * sizeof(float));
    g_audio_read_sample = 0;
    g_audio_write_sample = 0;
    g_audio_queued_samples = 0;
    g_audio_underruns = 0;
    g_audio_running_frame_index = 0;
    return 1;
}

static int stasis_audio_mix_output(float* output, int frame_count) {
    if (!output || frame_count <= 0 || g_audio_channels <= 0 ||
        !stasis_audio_ensure_ring_init()) return 0;
    const int sample_count = frame_count * g_audio_channels;
    SDL_memset(output, 0, (size_t)sample_count * sizeof(float));
    const int available = stasis_audio_mini(sample_count, g_audio_queued_samples);
    int copied = 0;
    while (copied < available) {
        int contiguous = g_audio_ring_capacity_samples - g_audio_read_sample;
        int part = stasis_audio_mini(available - copied, contiguous);
        SDL_memcpy(
            &output[copied], &g_audio_ring[g_audio_read_sample],
            (size_t)part * sizeof(float));
        copied += part;
        g_audio_read_sample = (g_audio_read_sample + part) % g_audio_ring_capacity_samples;
        g_audio_queued_samples -= part;
    }
    stasis_audio_assets_mix(&g_audio_assets, output, frame_count, g_audio_sample_rate);
    g_audio_running_frame_index += frame_count;
    return frame_count;
}

static void SDLCALL stasis_audio_callback(
    void* userdata,
    SDL_AudioStream* stream,
    int additional_amount,
    int total_amount
) {
    (void)userdata;
    (void)total_amount;
    if (!stream || additional_amount <= 0 || g_audio_channels <= 0) return;

    int remaining_frames = additional_amount / (int)sizeof(float) / g_audio_channels;
    if (g_audio_queued_samples < remaining_frames * g_audio_channels &&
        !stasis_audio_has_active_voice()) {
        g_audio_underruns++;
    }

    float output[1024];
    const int max_frames = (int)(sizeof(output) / sizeof(output[0])) / g_audio_channels;
    while (remaining_frames > 0) {
        int chunk = stasis_audio_mini(remaining_frames, max_frames);
        if (stasis_audio_mix_output(output, chunk) != chunk) return;
        if (!SDL_PutAudioStreamData(
                stream, output, chunk * g_audio_channels * (int)sizeof(float))) return;
        remaining_frames -= chunk;
    }
}

static void stasis_audio_shutdown_internal(void) {
    if (g_audio_stream) {
        SDL_DestroyAudioStream(g_audio_stream);
        g_audio_stream = NULL;
    }

    if (g_audio_ring) {
        free(g_audio_ring);
        g_audio_ring = NULL;
    }

    stasis_audio_assets_reset(&g_audio_assets);

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
    if (g_recording_audio_enabled) return stasis_audio_ensure_ring_init();
    if (g_audio_initialized && g_audio_stream) {
        return 1;
    }

    if (!SDL_InitSubSystem(SDL_INIT_AUDIO)) {
        if (!SDL_Init(SDL_INIT_AUDIO)) {
            SDL_Log("stasis_audio_init: SDL audio subsystem unavailable: %s", SDL_GetError());
            return 0;
        }
    }

    SDL_AudioSpec desired;
    SDL_zero(desired);
    desired.format = SDL_AUDIO_F32;
    desired.channels = 2;

    /* Try 48k first, then 44.1k. */
    int rates[2] = { 48000, 44100 };
    SDL_AudioStream* stream = NULL;
    for (int i = 0; i < 2 && !stream; i++) {
        desired.freq = rates[i];
        stream = SDL_OpenAudioDeviceStream(
            SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK,
            &desired,
            stasis_audio_callback,
            NULL);
        if (stream) g_audio_sample_rate = rates[i];
    }

    if (!stream) {
        SDL_Log("stasis_audio_init: playback stream unavailable: %s", SDL_GetError());
        return 0;
    }

    g_audio_stream = stream;
    g_audio_channels = 2;

    if (!stasis_audio_ensure_ring_init()) {
        SDL_DestroyAudioStream(g_audio_stream);
        g_audio_stream = NULL;
        return 0;
    }
    g_audio_initialized = 1;

    if (!SDL_ResumeAudioStreamDevice(g_audio_stream)) {
        SDL_Log("stasis_audio_init: playback stream could not resume: %s", SDL_GetError());
        stasis_audio_shutdown_internal();
        return 0;
    }
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

typedef struct {
    float x;
    float y;
    float w;
    float h;
} StasisRenderClip;
static StasisRenderClip g_render_clip_stack[STASIS_RENDER_MAX_CLIPS];
static int g_render_clip_depth = 0;

/* Simple shader + buffer for line rendering */
static char g_asset_base[512] = {0};
static char g_asset_env[512] = {0};

STASIS_EXPORT int stasis_set_asset_root(const char* path) {
    if (!path || !*path || strlen(path) >= sizeof(g_asset_base)) return 0;
    strncpy(g_asset_base, path, sizeof(g_asset_base) - 1);
    g_asset_base[sizeof(g_asset_base) - 1] = 0;
    strncpy(g_asset_env, path, sizeof(g_asset_env) - 1);
    g_asset_env[sizeof(g_asset_env) - 1] = 0;
    return 1;
}

STASIS_EXPORT void stasis_draw_line(float x1, float y1, float x2, float y2,
                                    float r, float g, float b, float a);
STASIS_EXPORT void stasis_fill_rect(float x, float y, float w, float h,
                                    float r, float g, float b, float a);


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
    if (stasis_asset_path_is_virtual_root(path)) {
        char normalized[1024];
        if (!stasis_asset_normalize_relative_path(path, normalized, sizeof(normalized))) {
            return 0;
        }
        snprintf(out, out_size, "%s/%s", g_asset_base, normalized);
        out[out_size - 1] = 0;
    } else if (is_absolute_path(path)) {
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
#if defined(_WIN32)
    /* Sprite callers commonly use ../assets/... from a staged src directory. Keep the
     * cache key identical to the absolute path passed by the host watcher. */
    char canonical[1024];
    if (_fullpath(canonical, out, sizeof(canonical))) {
        strncpy(out, canonical, out_size - 1);
        out[out_size - 1] = 0;
        for (char* p = out; *p; ++p) {
            if (*p == '\\') *p = '/';
        }
    }
#endif
    return 1;
}

#if defined(_WIN32)
static volatile LONG g_asset_watch_dirty = 0;
static volatile LONG g_asset_watch_force_reload = 0;
#define GFX_ASSET_WATCH_MAX_PENDING_PATHS 64
#define GFX_ASSET_WATCH_PATH_SIZE 1024
static SRWLOCK g_asset_watch_path_lock = SRWLOCK_INIT;
static int g_asset_watch_pending_path_count = 0;
static char g_asset_watch_pending_paths[GFX_ASSET_WATCH_MAX_PENDING_PATHS][GFX_ASSET_WATCH_PATH_SIZE];
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

#if !defined(_WIN32)
static volatile int g_asset_watch_dirty = 0;
static volatile int g_asset_watch_force_reload = 0;
#define GFX_ASSET_WATCH_MAX_PENDING_PATHS 64
#define GFX_ASSET_WATCH_PATH_SIZE 1024
static int g_asset_watch_pending_path_count = 0;
static char g_asset_watch_pending_paths[GFX_ASSET_WATCH_MAX_PENDING_PATHS][GFX_ASSET_WATCH_PATH_SIZE];
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
    int force_reload = !path || !*path;
#if defined(_WIN32)
    if (!force_reload) {
        AcquireSRWLockExclusive(&g_asset_watch_path_lock);
        if (g_asset_watch_pending_path_count < GFX_ASSET_WATCH_MAX_PENDING_PATHS) {
            char* pending = g_asset_watch_pending_paths[g_asset_watch_pending_path_count++];
            strncpy(pending, path, GFX_ASSET_WATCH_PATH_SIZE - 1);
            pending[GFX_ASSET_WATCH_PATH_SIZE - 1] = 0;
            for (char* p = pending; *p; ++p) {
                if (*p == '\\') *p = '/';
            }
        } else {
            force_reload = 1;
        }
        ReleaseSRWLockExclusive(&g_asset_watch_path_lock);
    }
    InterlockedExchange(&g_asset_watch_dirty, 1);
    if (force_reload) InterlockedExchange(&g_asset_watch_force_reload, 1);
#else
    if (!force_reload) {
        if (g_asset_watch_pending_path_count < GFX_ASSET_WATCH_MAX_PENDING_PATHS) {
            char* pending = g_asset_watch_pending_paths[g_asset_watch_pending_path_count++];
            strncpy(pending, path, GFX_ASSET_WATCH_PATH_SIZE - 1);
            pending[GFX_ASSET_WATCH_PATH_SIZE - 1] = 0;
            for (char* p = pending; *p; ++p) {
                if (*p == '\\') *p = '/';
            }
        } else {
            force_reload = 1;
        }
    }
    g_asset_watch_dirty = 1;
    if (force_reload) g_asset_watch_force_reload = 1;
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


static uint64_t get_file_mtime(const char* path) {
    char resolved[1024];
    const char* probe = path;
    if (!is_absolute_path(path) || stasis_asset_path_is_virtual_root(path)) {
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

/* SVG rasterization (paths, gradients, transforms) via ThorVG. */
static int bake_svg_to_rgba(const char* path, unsigned char** out_pixels, int* out_w, int* out_h) {
    *out_pixels = NULL;
    *out_w = 0;
    *out_h = 0;

    char resolved[1024];
    if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
        fprintf(stderr, "bake_svg_to_rgba: bad path %s\n", path ? path : "(null)");
        return 0;
    }

    if (!stasis_svg_rasterize_file(resolved, 0, 0, out_pixels, out_w, out_h)) {
        fprintf(stderr, "bake_svg_to_rgba: failed to parse %s\n", resolved);
        return 0;
    }
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

    if (!stasis_svg_rasterize_file(
            resolved_path, max_w, max_h, out_pixels, out_w, out_h)) {
        fprintf(stderr, "bake_svg_to_rgba_sized: failed to parse %s\n", resolved_path);
        return 0;
    }
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
        fprintf(stderr, "bake_raster_to_rgba_sized: IMG_Load failed for %s: %s\n", resolved_path, SDL_GetError());
        return 0;
    }

    SDL_Surface* rgba = SDL_ConvertSurface(loaded, SDL_PIXELFORMAT_RGBA32);
    SDL_DestroySurface(loaded);
    if (!rgba) {
        fprintf(stderr, "bake_raster_to_rgba_sized: SDL_ConvertSurface failed for %s: %s\n", resolved_path, SDL_GetError());
        return 0;
    }

    const int src_w = rgba->w;
    const int src_h = rgba->h;
    if (src_w <= 0 || src_h <= 0) {
        SDL_DestroySurface(rgba);
        fprintf(stderr, "bake_raster_to_rgba_sized: invalid raster size %dx%d in %s\n", src_w, src_h, resolved_path);
        return 0;
    }

    unsigned char* out = (unsigned char*)malloc((size_t)max_w * (size_t)max_h * 4u);
    if (!out) {
        SDL_DestroySurface(rgba);
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

    SDL_DestroySurface(rgba);

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

static void stasis_asset_task_clear(StasisAssetTask* task) {
    if (!task) return;
    free(task->pixels);
    stasis_audio_decoded_free(&task->audio);
    memset(task, 0, sizeof(*task));
}

static int stasis_asset_task_worker(void* unused) {
    (void)unused;
    for (;;) {
        SDL_LockMutex(g_asset_task_mutex);
        int slot = -1;
        while (!g_asset_task_stop && slot < 0) {
            int decoded_backlog = 0;
            for (int i = 0; i < STASIS_ASSET_TASK_CAPACITY; i++) {
                if (g_asset_tasks[i].state == STASIS_ASSET_TASK_DECODED) {
                    decoded_backlog = 1;
                    break;
                }
            }
            if (!decoded_backlog) {
                for (int i = 0; i < STASIS_ASSET_TASK_CAPACITY; i++) {
                    if (g_asset_tasks[i].state == STASIS_ASSET_TASK_PENDING) {
                        slot = i;
                        g_asset_tasks[i].state = STASIS_ASSET_TASK_LOADING;
                        break;
                    }
                }
            }
            if (slot < 0) SDL_WaitCondition(g_asset_task_condition, g_asset_task_mutex);
        }
        if (g_asset_task_stop) {
            SDL_UnlockMutex(g_asset_task_mutex);
            return 0;
        }

        StasisAssetTask* task = &g_asset_tasks[slot];
        int task_id = task->id;
        int kind = task->kind;
        char path[1024];
        memcpy(path, task->path, sizeof(path));
        int raster_w = task->raster_w;
        int raster_h = task->raster_h;
        SDL_UnlockMutex(g_asset_task_mutex);

        unsigned char* pixels = NULL;
        int pixel_w = 0;
        int pixel_h = 0;
        StasisDecodedAudio audio;
        memset(&audio, 0, sizeof(audio));
        int ok = kind == STASIS_ASSET_KIND_SPRITE
            ? bake_image_to_rgba_sized(path, raster_w, raster_h, &pixels, &pixel_w, &pixel_h)
            : stasis_audio_decode(path, &audio);

        SDL_LockMutex(g_asset_task_mutex);
        task = &g_asset_tasks[slot];
        if (task->id != task_id || task->release_requested ||
            task->state == STASIS_ASSET_TASK_CANCELLED) {
            free(pixels);
            stasis_audio_decoded_free(&audio);
            stasis_asset_task_clear(task);
        } else if (!ok) {
            task->state = STASIS_ASSET_TASK_FAILED;
        } else {
            task->pixels = pixels;
            task->pixel_w = pixel_w;
            task->pixel_h = pixel_h;
            task->audio = audio;
            task->state = STASIS_ASSET_TASK_DECODED;
        }
        SDL_UnlockMutex(g_asset_task_mutex);
    }
}

static int stasis_asset_tasks_ensure_storage(void) {
    if (g_asset_task_mutex && g_asset_task_condition) return 1;
    g_asset_task_mutex = SDL_CreateMutex();
    g_asset_task_condition = SDL_CreateCondition();
    if (!g_asset_task_mutex || !g_asset_task_condition) goto fail;
    return 1;

fail:
    if (g_asset_task_condition) SDL_DestroyCondition(g_asset_task_condition);
    if (g_asset_task_mutex) SDL_DestroyMutex(g_asset_task_mutex);
    g_asset_task_condition = NULL;
    g_asset_task_mutex = NULL;
    return 0;
}

static int stasis_asset_tasks_ensure_started(void) {
    if (g_asset_task_thread) return 1;
    if (!stasis_asset_tasks_ensure_storage()) return 0;
    g_asset_task_stop = 0;
    g_asset_task_thread = SDL_CreateThread(stasis_asset_task_worker, "stasis-assets", NULL);
    if (!g_asset_task_thread) goto fail;
    return 1;

fail:
    if (g_asset_task_condition) SDL_DestroyCondition(g_asset_task_condition);
    if (g_asset_task_mutex) SDL_DestroyMutex(g_asset_task_mutex);
    g_asset_task_condition = NULL;
    g_asset_task_mutex = NULL;
    return 0;
}

static StasisAssetTask* stasis_asset_task_find_locked(int task_id);

static int stasis_asset_task_request(
    int kind,
    const char* path,
    int max_w,
    int max_h,
    StasisSpriteAtlasPolicyV3 atlas_policy
) {
    char resolved[1024];
    if (!path || !*path || !resolve_asset_path(path, resolved, sizeof(resolved))) return 0;
    if (kind == STASIS_ASSET_KIND_SPRITE) {
        if (!g_window || max_w <= 0 || max_h <= 0) return 0;
    } else if (kind == STASIS_ASSET_KIND_AUDIO && !stasis_audio_ensure_init()) {
        return 0;
    }
    const int synchronous_audio =
        kind == STASIS_ASSET_KIND_AUDIO && g_recording_audio_enabled;
    if (synchronous_audio) {
        if (!stasis_asset_tasks_ensure_storage()) return 0;
    } else if (!stasis_asset_tasks_ensure_started()) {
        return 0;
    }

    SDL_LockMutex(g_asset_task_mutex);
    int slot = -1;
    for (int i = 0; i < STASIS_ASSET_TASK_CAPACITY; i++) {
        if (g_asset_tasks[i].state == STASIS_ASSET_TASK_NONE) {
            slot = i;
            break;
        }
    }
    if (slot < 0) {
        SDL_UnlockMutex(g_asset_task_mutex);
        return 0;
    }
    StasisAssetTask* task = &g_asset_tasks[slot];
    memset(task, 0, sizeof(*task));
    task->id = g_asset_task_next_id++;
    if (g_asset_task_next_id <= 0) g_asset_task_next_id = 1;
    task->kind = kind;
    task->state = synchronous_audio ? STASIS_ASSET_TASK_LOADING : STASIS_ASSET_TASK_PENDING;
    task->max_w = max_w;
    task->max_h = max_h;
    task->atlas_policy = kind == STASIS_ASSET_KIND_SPRITE
        ? atlas_policy : stasis_sprite_atlas_policy_v3_standalone();
    task->raster_w = kind == STASIS_ASSET_KIND_SPRITE
        ? stasis_current_scaled_extent(max_w) : 0;
    task->raster_h = kind == STASIS_ASSET_KIND_SPRITE
        ? stasis_current_scaled_extent(max_h) : 0;
    memcpy(task->path, resolved, strlen(resolved) + 1);
    int id = task->id;
    SDL_UnlockMutex(g_asset_task_mutex);
    if (synchronous_audio) {
        StasisDecodedAudio audio;
        memset(&audio, 0, sizeof(audio));
        const int ok = stasis_audio_decode(resolved, &audio);
        SDL_LockMutex(g_asset_task_mutex);
        task = stasis_asset_task_find_locked(id);
        if (!task || task->release_requested || task->state == STASIS_ASSET_TASK_CANCELLED) {
            stasis_audio_decoded_free(&audio);
            if (task) stasis_asset_task_clear(task);
        } else if (!ok) {
            task->state = STASIS_ASSET_TASK_FAILED;
            stasis_audio_decoded_free(&audio);
        } else {
            task->audio = audio;
            task->state = STASIS_ASSET_TASK_DECODED;
        }
        SDL_UnlockMutex(g_asset_task_mutex);
    } else {
        SDL_LockMutex(g_asset_task_mutex);
        task = stasis_asset_task_find_locked(id);
        if (task) SDL_SignalCondition(g_asset_task_condition);
        SDL_UnlockMutex(g_asset_task_mutex);
    }
    return id;
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

    int w = g_recording_presentation ? g_recording_width : g_drawable_width;
    int h = g_recording_presentation ? g_recording_height : g_drawable_height;
    if (g_renderer && !g_recording_presentation) {
        /* A regular screenshot captures the fitted logical content returned by
         * SDL readback, not the complete renderer backing used by density and
         * framebuffer accounting. Derive that extent from the full backing;
         * SDL_GetCurrentRenderOutputSize can itself report stale presentation
         * state during the same logical-canvas transition. */
        w = stasis_current_scaled_extent(g_window_width);
        h = stasis_current_scaled_extent(g_window_height);
    }
    const size_t bytes = (size_t)w * (size_t)h * 4u;

    uint8_t* pixels = (uint8_t*)malloc(bytes);
    if (!pixels) return 0;

    int ok = 0;

    if (true) {
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
                    SDL_RenderLine(g_renderer, g_lines[i].x1, g_lines[i].y1, g_lines[i].x2, g_lines[i].y2);
                }
                g_line_count = 0;
            }

            /* Read the fixed physical recording target, not the logical viewport. */
            if (g_recording_presentation) {
                SDL_SetRenderLogicalPresentation(
                    g_renderer, 0, 0, SDL_LOGICAL_PRESENTATION_DISABLED);
            }
            /* SDL3 returns an owned surface for renderer readback. */
            SDL_Surface* readback = SDL_RenderReadPixels(g_renderer, NULL);
            SDL_Surface* bgra = readback
                ? SDL_ConvertSurface(readback, SDL_PIXELFORMAT_BGRA32)
                : NULL;
            if (bgra && bgra->w == w && bgra->h == h) {
                for (int row = 0; row < h; row++) {
                    SDL_memcpy(
                        pixels + (size_t)row * (size_t)w * 4u,
                        (const uint8_t*)bgra->pixels + (size_t)row * (size_t)bgra->pitch,
                        (size_t)w * 4u);
                }
                ok = png ? stasis_image_writer_write_png_bgra32(out_path, w, h, pixels, 0)
                         : stasis_image_writer_write_bmp_bgra32(out_path, w, h, pixels, 0);
            } else {
                SDL_Log("recording readback dimensions mismatch: got=%dx%d expected=%dx%d",
                    bgra ? bgra->w : 0, bgra ? bgra->h : 0, w, h);
            }
            SDL_DestroySurface(bgra);
            SDL_DestroySurface(readback);
            if (g_recording_presentation) {
                SDL_SetRenderLogicalPresentation(
                    g_renderer, g_window_width, g_window_height,
                    SDL_LOGICAL_PRESENTATION_LETTERBOX);
            }
        }
        free(pixels);
        return ok;
    }


    free(pixels);
    return 0;
}

STASIS_EXPORT int stasis_gfx_dump_bmp(const char* path) {
    return stasis_gfx_dump_image(path, 0, 1);
}

STASIS_EXPORT int stasis_gfx_dump_png(const char* path) {
    return stasis_gfx_dump_image(path, 1, 1);
}

STASIS_EXPORT int stasis_host_schedule_screenshot(const char* path) {
    if (!path || !*path || strlen(path) >= sizeof(g_screenshot_path)) return 0;
    strncpy(g_screenshot_path, path, sizeof(g_screenshot_path) - 1);
    g_screenshot_path[sizeof(g_screenshot_path) - 1] = 0;
    g_screenshot_frame = (int)(g_debug_frame_counter + 1);
    g_screenshot_exit_after = 0;
    g_screenshot_taken = false;
    return 1;
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
            "sdl",
            g_resource_lifecycle.surface_generation,
            g_resource_lifecycle.renderer_generation);
    }
    if (g_screenshot_exit_after) g_should_quit = true;
}



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
    char renderer[128];
    char version[128];
} RenderTestResult;

static RenderTestResult g_last_test_result = {0};

/* Test SDL renderer by drawing a known pattern and reading it back */
static int verify_sdl_rendering(SDL_Renderer* renderer, int width, int height) {
    RenderTestResult* result = &g_last_test_result;
    memset(result, 0, sizeof(*result));

    const char* renderer_name = SDL_GetRendererName(renderer);
    strncpy(result->renderer, renderer_name ? renderer_name : "unknown",
        sizeof(result->renderer) - 1);
    snprintf(result->version, sizeof(result->version), "SDL3 renderer");

    /* Clear to black */
    SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
    bool rc = SDL_RenderClear(renderer);
    if (!rc) {
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
    SDL_FRect rect = {
        (float)(cx - size), (float)(cy - size),
        (float)(size * 2), (float)(size * 2)};
    rc = SDL_RenderFillRect(renderer, &rect);
    if (!rc) {
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
    SDL_Surface* readback = SDL_RenderReadPixels(renderer, &readRect);
    rc = readback && SDL_ReadSurfacePixel(
        readback, 0, 0, &pixels[0], &pixels[1], &pixels[2], &pixels[3]);
    SDL_DestroySurface(readback);

    /* Switch back to default target */
    SDL_SetRenderTarget(renderer, NULL);
    SDL_DestroyTexture(target);

    if (!rc) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_RenderReadPixels failed: %s", SDL_GetError());
        SDL_Log("STARTUP TEST WARNING: %s", result->error_message);
        /* Not a fatal error - continue anyway */
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL renderer works (readback not available)");
        return 1;
    }

    result->pixels_tested = 1;

    /* SDL3 normalizes the surface readback into explicit RGBA channels. */

    int high_count = 0;
    int low_count = 0;
    for (int i = 0; i < 4; i++) {
        if (pixels[i] >= 200) high_count++;
        if (pixels[i] <= 55) low_count++;
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
        SDL_Log("  SDL_Renderer: %s", result->renderer);
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

/* Typed recording setup used by the JIT host before window initialization. */
STASIS_EXPORT int stasis_set_recording_config(int width, int height, uint32_t fps) {
    if (g_window || width < 1 || height < 1 || fps == 0) return 0;
    g_recording_width = width;
    g_recording_height = height;
    g_recording_fps = fps;
    g_recording_config_pending = true;
    return 1;
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

    SDL_SetLogOutputFunction(stasis_sdl_log_output, NULL);
    SDL_SetLogPriorities(SDL_LOG_PRIORITY_INFO);
    log_package_provenance();
    if (!SDL_Init(SDL_INIT_VIDEO | SDL_INIT_EVENTS)) {
        stasis_report_runtime_errorf("SDL initialization failed: %s", SDL_GetError());
        SDL_Log("SDL_Init failed: %s", SDL_GetError());
        return 0;
    }
    g_x11_scale_controlled_window = stasis_x11_scale_controlled_launch();

    /* Optional screenshot automation via environment variables. */
    g_screenshot_taken = false;
    g_screenshot_exit_after = 0;
    g_screenshot_frame = 1;
    g_debug_frame_counter = 0;
    g_screenshot_path[0] = 0;
    const bool typed_recording = g_recording_config_pending;
    const int typed_width = g_recording_width;
    const int typed_height = g_recording_height;
    const uint32_t typed_fps = g_recording_fps;
    g_recording_config_pending = false;
    g_recording_presentation = false;
    g_recording_width = 0;
    g_recording_height = 0;
    g_recording_fps = 0;
    if (typed_recording) {
        g_recording_presentation = true;
        g_recording_width = typed_width;
        g_recording_height = typed_height;
        g_recording_fps = typed_fps;
        width = typed_width;
        height = typed_height;
        SDL_Log("Stasis recording presentation: hidden=1 logical=%dx%d physical=%dx%d fps=%u",
            width, height, g_recording_width, g_recording_height, g_recording_fps);
    }
    const char* recording = SDL_getenv("STASIS_RECORDING_PRESENTATION");
    const char* recording_width = SDL_getenv("STASIS_RECORDING_WIDTH");
    const char* recording_height = SDL_getenv("STASIS_RECORDING_HEIGHT");
    const char* recording_fps = SDL_getenv("STASIS_RECORDING_FPS");
    if (!typed_recording && recording && strcmp(recording, "0") != 0 && recording_width && recording_height && recording_fps) {
        char* width_end = NULL;
        char* height_end = NULL;
        char* fps_end = NULL;
        long parsed_width = strtol(recording_width, &width_end, 10);
        long parsed_height = strtol(recording_height, &height_end, 10);
        long parsed_fps = strtol(recording_fps, &fps_end, 10);
        if (width_end != recording_width && *width_end == 0 &&
            height_end != recording_height && *height_end == 0 &&
            fps_end != recording_fps && *fps_end == 0 &&
            parsed_width > 0 && parsed_width <= INT_MAX &&
            parsed_height > 0 && parsed_height <= INT_MAX &&
            parsed_fps > 0 && parsed_fps <= UINT32_MAX) {
            g_recording_presentation = true;
            g_recording_width = (int)parsed_width;
            g_recording_height = (int)parsed_height;
            g_recording_fps = (uint32_t)parsed_fps;
            width = g_recording_width;
            height = g_recording_height;
            SDL_Log("Stasis recording presentation: hidden=1 logical=%dx%d physical=%dx%d fps=%u",
                width, height, g_recording_width, g_recording_height, g_recording_fps);
        }
    }
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

    g_window_width = width;
    g_window_height = height;
    g_native_window_width = width;
    g_native_window_height = height;
    g_drawable_width = width;
    g_drawable_height = height;
    g_pixel_scale = 1.0f;
    g_density_preparation_scale.numerator = 0;
    g_density_preparation_scale.denominator = 0;


    int native_request_width = width;
    int native_request_height = height;
    SDL_WindowFlags window_flags = SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY;
    if (g_recording_presentation) {
        window_flags &= ~SDL_WINDOW_RESIZABLE;
        window_flags |= SDL_WINDOW_HIDDEN;
    }
    const char* force_hidden = SDL_getenv("STASIS_WINDOW_HIDDEN");
    if (force_hidden && strcmp(force_hidden, "0") != 0) {
        window_flags |= SDL_WINDOW_HIDDEN;
    }
#if defined(__ANDROID__) || defined(__IPHONEOS__)
    const SDL_DisplayMode* display_mode =
        SDL_GetCurrentDisplayMode(SDL_GetPrimaryDisplay());
    if (display_mode && display_mode->w > 0 && display_mode->h > 0) {
        native_request_width = display_mode->w;
        native_request_height = display_mode->h;
    }
    window_flags |= SDL_WINDOW_FULLSCREEN;
#else
    if (!g_recording_presentation) {
        const float display_scale = stasis_x11_window_scale();
        native_request_width = stasis_display_scaled_window_extent(width, display_scale);
        native_request_height = stasis_display_scaled_window_extent(height, display_scale);
    }
    /* Let the desktop window manager fill its usable work area without
       covering taskbars, docks, or panels. */
    if (!g_recording_presentation && !g_x11_scale_controlled_window) {
        window_flags |= SDL_WINDOW_MAXIMIZED;
    }
#endif

    g_window = SDL_CreateWindow(
        title ? title : "Stasis",
        native_request_width,
        native_request_height,
        window_flags
    );

    if (!g_window) {
        stasis_report_runtime_errorf("Game window creation failed: %s", SDL_GetError());
        SDL_Log("SDL_CreateWindow failed: %s", SDL_GetError());
        SDL_Quit();
        return 0;
    }
    if (!g_recording_presentation) {
        SDL_SetWindowPosition(g_window, SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED);
    }

    /* Optional: start window minimized to keep automated/local test runs unobtrusive. */
    {
        const char* start_minimized = SDL_getenv("STASIS_WINDOW_START_MINIMIZED");
        if (start_minimized && strcmp(start_minimized, "0") != 0) {
            SDL_MinimizeWindow(g_window);
        }
    }

    g_renderer = SDL_CreateRenderer(g_window, NULL);
    if (!g_renderer) {
        stasis_report_runtime_errorf("Renderer creation failed: %s", SDL_GetError());
        SDL_Log("SDL_CreateRenderer failed: %s", SDL_GetError());
        SDL_DestroyWindow(g_window);
        g_window = NULL;
        SDL_Quit();
        return 0;
    }
    if (!SDL_SetRenderVSync(g_renderer, g_recording_presentation ? 0 : 1)) {
        SDL_Log("SDL_SetRenderVSync failed (recording=%d): %s",
            g_recording_presentation ? 1 : 0, SDL_GetError());
    }
    SDL_SetDefaultTextureScaleMode(g_renderer, SDL_SCALEMODE_LINEAR);
    SDL_SetRenderLogicalPresentation(
        g_renderer, width, height, SDL_LOGICAL_PRESENTATION_LETTERBOX);
    const char* renderer_name = SDL_GetRendererName(g_renderer);
    SDL_Log("Stasis graphics initialized (SDL renderer): %dx%d name=%s",
        width, height, renderer_name ? renderer_name : "?");

    stasis_sync_display_metrics();
    g_window_minimized = (SDL_GetWindowFlags(g_window) & SDL_WINDOW_MINIMIZED) != 0;
    stasis_renderer_lifecycle_initialize(&g_resource_lifecycle);
    g_resource_frame_ready = true;
    /* Pump once before presenting the asset-free loading frame. */
    SDL_PumpEvents();
    stasis_present_gpu_loading();
    SDL_Log("Stasis display metrics: logical=%dx%d native=%dx%d drawable=%dx%d scale=%.3f display_scale=%.3f display_generation=%d density_generation=%d",
        g_window_width, g_window_height,
        g_native_window_width, g_native_window_height,
        g_drawable_width, g_drawable_height, g_pixel_scale,
        stasis_x11_window_scale(), g_display_generation, g_density_generation);
    g_keyboard_state = SDL_GetKeyboardState(NULL);
    g_should_quit = false;
    g_line_count = 0;
    g_events_pumped_this_frame = 0;
    memset(&g_input_frame, 0, sizeof(g_input_frame));
    memset(g_keyboard_event_state, -1, sizeof(g_keyboard_event_state));
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
        test_ok = verify_sdl_rendering(g_renderer, width, height);

        if (!test_ok) {
            /* Print detailed diagnostics to stderr */
            fprintf(stderr, "\n");
            fprintf(stderr, "=== STASIS GRAPHICS STARTUP TEST FAILED ===\n");
            fprintf(stderr, "Error: %s\n", g_last_test_result.error_message);
            fprintf(stderr, "\n");
            fprintf(stderr, "Diagnostics:\n");
            fprintf(stderr, "  Mode: SDL Renderer\n");
            fprintf(stderr, "  Renderer: %s\n", g_last_test_result.renderer);
            fprintf(stderr, "  Info: %s\n", g_last_test_result.version);
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
            fprintf(stderr, "================================================\n");
            fprintf(stderr, "\n");

            /* Cleanup and return failure */
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
        SDL_SetRenderDrawColor(g_renderer, 0, 0, 0, 255);
        SDL_RenderClear(g_renderer);
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
    if (SDL_WasInit(SDL_INIT_VIDEO) == 0) {
        if (width) *width = 0;
        if (height) *height = 0;
        return;
    }
    stasis_query_available_presentation(
        g_native_window_width, g_native_window_height, width, height);
}

static void stasis_set_logical_size(int width, int height) {
    if (width < 1 || height < 1) {
        return;
    }

    g_window_width = width;
    g_window_height = height;
    g_window_resized = true;
}

/*
 * Set window size (windowed mode).
 * width/height are logical canvas/window points, not necessarily drawable pixels.
 */
STASIS_EXPORT void stasis_set_window_size(int width, int height) {
    if (!g_window || width < 1 || height < 1) {
        return;
    }

    stasis_set_logical_size(width, height);
#if !defined(__ANDROID__) && !defined(__IPHONEOS__)
    if (g_recording_presentation) {
        if (g_renderer) {
            SDL_SetRenderLogicalPresentation(
                g_renderer, width, height, SDL_LOGICAL_PRESENTATION_LETTERBOX);
        }
        stasis_sync_display_metrics();
        return;
    }
    const SDL_WindowFlags window_flags = SDL_GetWindowFlags(g_window);
    if ((window_flags & (SDL_WINDOW_MAXIMIZED | SDL_WINDOW_MINIMIZED)) != 0) {
        SDL_RestoreWindow(g_window);
        SDL_SyncWindow(g_window);
    }
    /* X11 window-manager state can remain maximized briefly after restore.
       The explicit request still owns the retained windowed backing extent. */
    stasis_apply_x11_window_scale(1);
#endif
    stasis_sync_display_metrics();

#if !defined(__ANDROID__) && !defined(__IPHONEOS__)
    SDL_Log(
        "Stasis window presentation: mode=windowed logical=%dx%d native=%dx%d drawable=%dx%d display_scale=%.3f display_generation=%d density_generation=%d",
        g_window_width, g_window_height,
        g_native_window_width, g_native_window_height,
        g_drawable_width, g_drawable_height,
        stasis_x11_window_scale(), g_display_generation, g_density_generation);
#endif

}

/*
 * Maximize or restore the desktop presentation without replacing the logical canvas.
 * Mobile owns its fullscreen surface and treats this as an accepted no-op.
 */
STASIS_EXPORT int stasis_set_maximized(int maximized) {
    if (!g_window) {
        return 0;
    }

    if (g_recording_presentation) {
        (void)maximized;
        if (g_renderer) {
            SDL_SetRenderLogicalPresentation(
                g_renderer, g_window_width, g_window_height,
                SDL_LOGICAL_PRESENTATION_LETTERBOX);
        }
        stasis_sync_display_metrics();
        return 1;
    }

#if defined(__ANDROID__) || defined(__IPHONEOS__)
    (void)maximized;
    stasis_sync_display_metrics();
    return 1;
#else
    if (g_x11_scale_controlled_window) {
        maximized = 0;
    }
    bool result = SDL_SetWindowFullscreen(g_window, false);
    if (result) {
        result = maximized ? SDL_MaximizeWindow(g_window) : SDL_RestoreWindow(g_window);
    }
    if (result) {
        SDL_SyncWindow(g_window);
        if (g_x11_scale_controlled_window) {
            stasis_apply_x11_window_scale(1);
        }
        stasis_sync_display_metrics();


        SDL_DisplayID display = SDL_GetDisplayForWindow(g_window);
        SDL_Rect usable = {0, 0, 0, 0};
        int native_w = 0;
        int native_h = 0;
        int border_top = 0;
        int border_left = 0;
        int border_bottom = 0;
        int border_right = 0;
        SDL_GetWindowSize(g_window, &native_w, &native_h);
        SDL_GetWindowBordersSize(
            g_window, &border_top, &border_left, &border_bottom, &border_right);
        if (display != 0 && SDL_GetDisplayUsableBounds(display, &usable)) {
            SDL_Log(
                "Stasis window presentation: mode=%s logical=%dx%d native=%dx%d drawable=%dx%d bounds=%dx%d usable=%dx%d display_scale=%.3f display_generation=%d density_generation=%d",
                maximized ? "maximized" : "windowed",
                g_window_width, g_window_height,
                native_w, native_h,
                g_drawable_width, g_drawable_height,
                native_w + border_left + border_right,
                native_h + border_top + border_bottom,
                usable.w, usable.h, stasis_x11_window_scale(),
                g_display_generation, g_density_generation);
        }
    }
    return result ? 1 : 0;
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

    if (g_recording_presentation) {
        (void)fullscreen;
        stasis_sync_display_metrics();
        return 1;
    }

    bool result = SDL_SetWindowFullscreen(g_window, fullscreen != 0);

    if (result) {
        stasis_sync_display_metrics();

#if !defined(__ANDROID__) && !defined(__IPHONEOS__)
        SDL_Log("Stasis window presentation: mode=%s", fullscreen ? "fullscreen" : "windowed");
#endif

    }

    return result ? 1 : 0;
}

/*
 * Begin a new frame
 */
STASIS_EXPORT void stasis_begin_frame(void) {
    gfx_debug_hash_reset_if_enabled();
    gfx_asset_watch_apply_pending_changes();
    if (!g_events_pumped_this_frame) {
        stasis_pump_events();
        g_events_pumped_this_frame = 1;
    }
    g_resource_frame_ready = stasis_restore_renderer_resources() != 0;
    g_line_count = 0;
    stasis_render_reset_clip();
    SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
    SDL_SetRenderClipRect(g_renderer, NULL);
}

static uint64_t stasis_perf_elapsed_us(uint64_t started_counter, uint64_t finished_counter) {
    if (started_counter == 0 || finished_counter < started_counter) return 0;
    const uint64_t frequency = SDL_GetPerformanceFrequency();
    if (frequency == 0) return 0;
    return ((finished_counter - started_counter) * 1000000u) / frequency;
}

STASIS_EXPORT uint64_t stasis_host_performance_counter(void) {
    if (!stasis_host_performance_metrics_enabled()) return 0;
    return SDL_GetPerformanceCounter();
}

STASIS_EXPORT uint64_t stasis_host_performance_elapsed_us(
    uint64_t started_counter,
    uint64_t finished_counter) {
    return stasis_perf_elapsed_us(started_counter, finished_counter);
}

static void stasis_perf_finish_render_sample(uint64_t host_replay_us,
                                             uint64_t present_wait_us) {
    if (!stasis_host_performance_metrics_enabled()) {
        g_perf_render_started_counter = 0;
        return;
    }
    const uint64_t now = SDL_GetPerformanceCounter();
    StasisPerfSample* sample = &g_perf_samples[g_perf_sample_next];
    sample->captured_counter = now;
    memset(&sample->metrics, 0, sizeof(sample->metrics));
    sample->metrics.version = STASIS_PERF_METRICS_VERSION;
    sample->metrics.size = (uint32_t)sizeof(sample->metrics);
    sample->metrics.tick_us = (uint32_t)g_perf_pending_tick_us;
    sample->metrics.guest_render_us = (uint32_t)g_perf_pending_guest_render_us;
    sample->metrics.host_replay_us = (uint32_t)host_replay_us;
    sample->metrics.render_prep_us = STASIS_PERF_UNAVAILABLE;
    sample->metrics.gpu_submit_us = STASIS_PERF_UNAVAILABLE;
    sample->metrics.gpu_execution_us = STASIS_PERF_UNAVAILABLE;
    sample->metrics.present_wait_us = (uint32_t)present_wait_us;
    sample->metrics.frame_work_us = sample->metrics.tick_us
        + sample->metrics.guest_render_us + sample->metrics.host_replay_us;
    sample->metrics.commands = g_perf_pending_commands;
    sample->metrics.lines = g_perf_pending_lines;
    sample->metrics.rectangles = g_perf_pending_rectangles;
    sample->metrics.sprites = g_perf_pending_sprites;
    sample->metrics.text = g_perf_pending_text;
    sample->metrics.instances = STASIS_PERF_UNAVAILABLE;
    sample->metrics.batches = STASIS_PERF_UNAVAILABLE;
    sample->metrics.draw_calls = STASIS_PERF_UNAVAILABLE;
    sample->metrics.texture_switches = STASIS_PERF_UNAVAILABLE;
    sample->metrics.uploaded_bytes = STASIS_PERF_UNAVAILABLE;
    snprintf(sample->metrics.backend, sizeof(sample->metrics.backend), "%s",
        "SDL");
    SDL_LockSpinlock(&g_perf_metrics_lock);
    g_perf_latest_metrics = sample->metrics;
    SDL_UnlockSpinlock(&g_perf_metrics_lock);
    g_perf_sample_next = (g_perf_sample_next + 1) % STASIS_PERF_SAMPLE_CAPACITY;
    if (g_perf_sample_count < STASIS_PERF_SAMPLE_CAPACITY) {
        g_perf_sample_count++;
    }
    g_perf_render_started_counter = 0;
}

static void stasis_perf_latest_snapshot(StasisPerformanceMetrics* output,
                                         uint32_t* worst_frame_work_us) {
    if (!output) return;
    SDL_LockSpinlock(&g_perf_metrics_lock);
    *output = g_perf_latest_metrics;
    SDL_UnlockSpinlock(&g_perf_metrics_lock);
    uint32_t worst = output->frame_work_us;
    const uint64_t now = SDL_GetPerformanceCounter();
    const uint64_t frequency = SDL_GetPerformanceFrequency();
    const uint64_t window = frequency * 5u;
    for (int i = 0; i < g_perf_sample_count; i++) {
        const StasisPerfSample* sample = &g_perf_samples[i];
        if (sample->captured_counter == 0 || now < sample->captured_counter
            || now - sample->captured_counter > window) continue;
        if (sample->metrics.frame_work_us != STASIS_PERF_UNAVAILABLE
            && sample->metrics.frame_work_us > worst) {
            worst = sample->metrics.frame_work_us;
        }
    }
    if (worst_frame_work_us) *worst_frame_work_us = worst;
}

static void stasis_perf_append_value(char* output, size_t capacity, uint32_t us) {
    if (us == STASIS_PERF_UNAVAILABLE) {
        output[0] = '\0';
    } else {
        snprintf(output, capacity, "%.2f ms", (double)us / 1000.0);
    }
}

static void stasis_perf_append_count(char* output, size_t capacity, uint32_t count) {
    if (count == STASIS_PERF_UNAVAILABLE) output[0] = '\0';
    else snprintf(output, capacity, "%u", (unsigned int)count);
}


static void stasis_perf_draw_overlay(void) {
    if (!g_force_debug_overlay) return;

    StasisPerformanceMetrics metrics;
    uint32_t worst_frame_work_us = STASIS_PERF_UNAVAILABLE;
    stasis_perf_latest_snapshot(&metrics, &worst_frame_work_us);
    const int under_budget = metrics.frame_work_us != STASIS_PERF_UNAVAILABLE
        && metrics.frame_work_us <= 16667;
    char tick[32], guest[32], host[32], frame[32], present[32];
    char commands[16], lines[16], rectangles[16], sprites[16], text_count[16];
    stasis_perf_append_value(tick, sizeof(tick), metrics.tick_us);
    stasis_perf_append_value(guest, sizeof(guest), metrics.guest_render_us);
    stasis_perf_append_value(host, sizeof(host), metrics.host_replay_us);
    stasis_perf_append_value(frame, sizeof(frame), metrics.frame_work_us);
    stasis_perf_append_value(present, sizeof(present), metrics.present_wait_us);
    stasis_perf_append_count(commands, sizeof(commands), metrics.commands);
    stasis_perf_append_count(lines, sizeof(lines), metrics.lines);
    stasis_perf_append_count(rectangles, sizeof(rectangles), metrics.rectangles);
    stasis_perf_append_count(sprites, sizeof(sprites), metrics.sprites);
    stasis_perf_append_count(text_count, sizeof(text_count), metrics.text);
    char text[5][220];
    snprintf(text[0], sizeof(text[0]), "%s  [F3]", metrics.backend[0] ? metrics.backend : "native");
    if (metrics.tick_us == STASIS_PERF_UNAVAILABLE && metrics.guest_render_us == STASIS_PERF_UNAVAILABLE) {
        text[1][0] = '\0';
    } else {
        snprintf(text[1], sizeof(text[1]), "tick %s  guest render %s", tick, guest);
    }
    if (metrics.host_replay_us == STASIS_PERF_UNAVAILABLE) text[2][0] = '\0';
    else snprintf(text[2], sizeof(text[2]), "host replay %s", host);
    if (metrics.frame_work_us == STASIS_PERF_UNAVAILABLE) {
        text[3][0] = '\0';
    } else {
        snprintf(text[3], sizeof(text[3]), "frame work %s (worst %.2f ms)  %s%s%s",
            frame, worst_frame_work_us == STASIS_PERF_UNAVAILABLE ? 0.0 : (double)worst_frame_work_us / 1000.0,
            under_budget ? "UNDER 16.67 ms" : "OVER 16.67 ms",
            metrics.present_wait_us == STASIS_PERF_UNAVAILABLE ? "" : "  present wait ",
            metrics.present_wait_us == STASIS_PERF_UNAVAILABLE ? "" : present);
    }
    if (metrics.commands == STASIS_PERF_UNAVAILABLE && metrics.lines == STASIS_PERF_UNAVAILABLE
        && metrics.rectangles == STASIS_PERF_UNAVAILABLE && metrics.sprites == STASIS_PERF_UNAVAILABLE
        && metrics.text == STASIS_PERF_UNAVAILABLE) {
        text[4][0] = '\0';
    } else {
        size_t offset = 0;
        text[4][0] = '\0';
        if (metrics.commands != STASIS_PERF_UNAVAILABLE) {
            offset += (size_t)snprintf(text[4] + offset, sizeof(text[4]) - offset, "commands %s", commands);
        }
        if (metrics.lines != STASIS_PERF_UNAVAILABLE && offset < sizeof(text[4])) {
            offset += (size_t)snprintf(text[4] + offset, sizeof(text[4]) - offset, "%slines %s", offset ? "  " : "", lines);
        }
        if (metrics.rectangles != STASIS_PERF_UNAVAILABLE && offset < sizeof(text[4])) {
            offset += (size_t)snprintf(text[4] + offset, sizeof(text[4]) - offset, "%srects %s", offset ? "  " : "", rectangles);
        }
        if (metrics.sprites != STASIS_PERF_UNAVAILABLE && offset < sizeof(text[4])) {
            offset += (size_t)snprintf(text[4] + offset, sizeof(text[4]) - offset, "%ssprites %s", offset ? "  " : "", sprites);
        }
        if (metrics.text != STASIS_PERF_UNAVAILABLE && offset < sizeof(text[4])) {
            (void)snprintf(text[4] + offset, sizeof(text[4]) - offset, "%stext %s", offset ? "  " : "", text_count);
        }
    }

    float r = 1.0f;
    float g = 1.0f;
    float b = 1.0f;
    const int budget_percent = metrics.frame_work_us == STASIS_PERF_UNAVAILABLE
        ? 0 : (int)(metrics.frame_work_us * 100u / 16667u);
    if (budget_percent >= 100) {
        r = 0.73f; g = 0.41f; b = 1.0f;
    } else if (budget_percent >= 80) {
        r = 1.0f; g = 0.36f; b = 0.36f;
    } else if (budget_percent >= 50) {
        r = 1.0f; g = 0.84f; b = 0.40f;
    }

    const int background_width = g_window_width > 760 ? 750 : g_window_width - 16;
    const float background_height = 18.0f + 18.0f * 5.0f;
    SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
    SDL_SetRenderDrawColor(g_renderer, 20, 28, 38, 170);
    SDL_FRect background = { 8.0f, 8.0f, (float)background_width, background_height };
    if (background.w > 0) SDL_RenderFillRect(g_renderer, &background);
    SDL_SetRenderDrawColor(g_renderer, (Uint8)(r * 255.0f), (Uint8)(g * 255.0f), (Uint8)(b * 255.0f), 255);
    for (int line = 0; line < 5; line++) {
        if (text[line][0]) SDL_RenderDebugText(g_renderer, 12.0f, 11.0f + (float)line * 18.0f, text[line]);
    }
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
    if (true) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
        SDL_Color color;
        /* Render lines one by one; could be grouped by color if needed */
        for (int i = 0; i < g_line_count; i++) {
            color.r = (Uint8)(g_lines[i].r * 255.0f);
            color.g = (Uint8)(g_lines[i].g * 255.0f);
            color.b = (Uint8)(g_lines[i].b * 255.0f);
            color.a = (Uint8)(g_lines[i].a * 255.0f);
            SDL_SetRenderDrawColor(g_renderer, color.r, color.g, color.b, color.a);
            SDL_RenderLine(g_renderer, g_lines[i].x1, g_lines[i].y1, g_lines[i].x2, g_lines[i].y2);
        }

        /* Capture before present so we read the current render target. */
        capture_scheduled_screenshot();
        const int measure_frame = stasis_host_performance_metrics_enabled();
        if (measure_frame) {
            const uint64_t host_finished = SDL_GetPerformanceCounter();
            const uint64_t present_started = SDL_GetPerformanceCounter();
            SDL_RenderPresent(g_renderer);
            const uint64_t present_finished = SDL_GetPerformanceCounter();
            stasis_perf_finish_render_sample(stasis_perf_elapsed_us(
                g_perf_render_started_counter, host_finished),
                stasis_perf_elapsed_us(present_started, present_finished));
        } else {
            SDL_RenderPresent(g_renderer);
        }
        g_line_count = 0;
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
    SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_NONE);
    SDL_SetRenderDrawColor(g_renderer, 0, 0, 0, 255);
    SDL_RenderClear(g_renderer);
    SDL_SetRenderDrawColor(g_renderer, (Uint8)(r * 255.0f), (Uint8)(g * 255.0f), (Uint8)(b * 255.0f), (Uint8)(a * 255.0f));
    SDL_FRect logical_canvas = {
        0.0f, 0.0f, (float)g_window_width, (float)g_window_height};
    SDL_RenderFillRect(g_renderer, &logical_canvas);
    SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
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

STASIS_EXPORT void stasis_fill_rect(float x, float y, float w, float h,
                                    float r, float g, float b, float a) {
    gfx_debug_hash_f32(x);
    gfx_debug_hash_f32(y);
    gfx_debug_hash_f32(w);
    gfx_debug_hash_f32(h);
    gfx_debug_hash_f32(r);
    gfx_debug_hash_f32(g);
    gfx_debug_hash_f32(b);
    gfx_debug_hash_f32(a);
    if (w <= 0.0f || h <= 0.0f) return;
    SDL_FRect rect = {x, y, w, h};
    SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
    SDL_SetRenderDrawColor(
        g_renderer,
        (Uint8)(r * 255.0f),
        (Uint8)(g * 255.0f),
        (Uint8)(b * 255.0f),
        (Uint8)(a * 255.0f));
    SDL_RenderFillRect(g_renderer, &rect);
}

static void stasis_render_set_clip(StasisRenderClip clip) {
    if (clip.w < 0.0f) clip.w = 0.0f;
    if (clip.h < 0.0f) clip.h = 0.0f;
    if (!g_renderer) return;
    SDL_Rect rect = {
        (int)floorf(clip.x),
        (int)floorf(clip.y),
        (int)ceilf(clip.w),
        (int)ceilf(clip.h)};
    SDL_SetRenderClipRect(g_renderer, &rect);
}

static void stasis_render_reset_clip(void) {
    g_render_clip_depth = 0;
    memset(g_render_clip_stack, 0, sizeof(g_render_clip_stack));
    if (g_renderer) SDL_SetRenderClipRect(g_renderer, NULL);
}

static void stasis_render_push_clip(float x, float y, float w, float h) {
    if (g_render_clip_depth >= STASIS_RENDER_MAX_CLIPS) return;
    StasisRenderClip clip = {x, y, w, h};
    const float logical_w = g_display_metrics.logical_w > 0
        ? (float)g_display_metrics.logical_w : (float)g_window_width;
    const float logical_h = g_display_metrics.logical_h > 0
        ? (float)g_display_metrics.logical_h : (float)g_window_height;
    const float left = clip.x > 0.0f ? clip.x : 0.0f;
    const float top = clip.y > 0.0f ? clip.y : 0.0f;
    const float right_limit = clip.x + clip.w < logical_w
        ? clip.x + clip.w : logical_w;
    const float bottom_limit = clip.y + clip.h < logical_h
        ? clip.y + clip.h : logical_h;
    clip.x = left;
    clip.y = top;
    clip.w = right_limit > left ? right_limit - left : 0.0f;
    clip.h = bottom_limit > top ? bottom_limit - top : 0.0f;
    if (g_render_clip_depth > 0) {
        const StasisRenderClip parent = g_render_clip_stack[g_render_clip_depth - 1];
        const float parent_right = parent.x + parent.w;
        const float parent_bottom = parent.y + parent.h;
        const float clipped_right = clip.x + clip.w < parent_right
            ? clip.x + clip.w : parent_right;
        const float clipped_bottom = clip.y + clip.h < parent_bottom
            ? clip.y + clip.h : parent_bottom;
        if (clip.x < parent.x) clip.x = parent.x;
        if (clip.y < parent.y) clip.y = parent.y;
        clip.w = clipped_right > clip.x ? clipped_right - clip.x : 0.0f;
        clip.h = clipped_bottom > clip.y ? clipped_bottom - clip.y : 0.0f;
    }
    g_render_clip_stack[g_render_clip_depth++] = clip;
    stasis_render_set_clip(clip);
}

static void stasis_render_pop_clip(void) {
    if (g_render_clip_depth <= 0) return;
    g_render_clip_depth--;
    if (g_render_clip_depth == 0) {
        if (g_renderer) SDL_SetRenderClipRect(g_renderer, NULL);
        return;
    }
    stasis_render_set_clip(g_render_clip_stack[g_render_clip_depth - 1]);
}

/*
 * Current command-buffer submission.
 *
 * Command coordinates are host pixels. Ordering is fixed by the buffer layout:
 * Flush category-local batches before a later command category or clip state
 * change draws.
 */
static void flush_ordered_lines(void) {
    if (g_line_count == 0) return;
    SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
    for (int i = 0; i < g_line_count; i++) {
        SDL_SetRenderDrawColor(
            g_renderer,
            (Uint8)(g_lines[i].r * 255.0f),
            (Uint8)(g_lines[i].g * 255.0f),
            (Uint8)(g_lines[i].b * 255.0f),
            (Uint8)(g_lines[i].a * 255.0f));
        SDL_RenderLine(
            g_renderer,
            g_lines[i].x1,
            g_lines[i].y1,
            g_lines[i].x2,
            g_lines[i].y2);
    }
    g_line_count = 0;
}

static void flush_ordered_sprites(void) {
}

static void stasis_draw_ordered_sprite(
    const int32_t* cmd_i32,
    const float* cmd_f32,
    int32_t index
) {
    const int32_t* sprite_i32 = cmd_i32 + STASIS_RENDER_I_SPRITE_BASE;
    const float* sprite_f32 = cmd_f32 + STASIS_RENDER_F_SPRITE_BASE;
    const int base_i = index * STASIS_RENDER_SPRITE_I32_STRIDE;
    const int base_f = index * STASIS_RENDER_SPRITE_F32_STRIDE;
    stasis_gfx_draw_sprite_internal(
        sprite_i32[base_i + 0],
        sprite_f32[base_f + 0],
        sprite_f32[base_f + 1],
        sprite_f32[base_f + 2],
        sprite_f32[base_f + 3],
        sprite_f32[base_f + 12],
        (uint32_t)sprite_i32[base_i + 1],
        sprite_f32[base_f + 4], sprite_f32[base_f + 5],
        sprite_f32[base_f + 6], sprite_f32[base_f + 7],
        sprite_f32[base_f + 8], sprite_f32[base_f + 9],
        sprite_f32[base_f + 10], sprite_f32[base_f + 11],
        g_debug_hash_enabled);
}
static void stasis_draw_ordered_text(
    const int32_t* cmd_i32,
    const float* cmd_f32,
    const uint8_t* cmd_u8,
    int32_t text_bytes_used,
    int32_t index
) {
    const int base_i = STASIS_RENDER_I_TEXT_BASE +
        index * STASIS_RENDER_TEXT_I32_STRIDE;
    const int font = cmd_i32[base_i + 0];
    const int byte_off = cmd_i32[base_i + 1];
    const int byte_len = cmd_i32[base_i + 2];
    if (font <= 0) return;

    const int base_f = STASIS_RENDER_F_TEXT_BASE +
        index * STASIS_RENDER_TEXT_F32_STRIDE;
    const float x = cmd_f32[base_f + 0];
    const float y = cmd_f32[base_f + 1];
    const float r = cmd_f32[base_f + 2];
    const float g = cmd_f32[base_f + 3];
    const float b = cmd_f32[base_f + 4];
    const float a = cmd_f32[base_f + 5];
    if (byte_off < 0) {
        if (byte_off != INT32_MIN) {
            stasis_gfx_draw_text_cached(-byte_off, x, y, r, g, b, a);
        }
        return;
    }
    if (!cmd_u8 || text_bytes_used <= 0 ||
        !stasis_render_text_span_is_valid(byte_off, byte_len, text_bytes_used)) {
        return;
    }
    stasis_draw_text(font, (const char*)(cmd_u8 + byte_off), x, y, r, g, b, a);
}

static void stasis_draw_ordered_rect(
    const float* cmd_f32,
    int32_t index
) {
    const int base = STASIS_RENDER_F_RECT_REVERSE_BASE -
        index * STASIS_RENDER_GEOMETRY_F32_STRIDE;
    stasis_fill_rect(
        cmd_f32[base + 0],
        cmd_f32[base + 1],
        cmd_f32[base + 2],
        cmd_f32[base + 3],
        cmd_f32[base + 4],
        cmd_f32[base + 5],
        cmd_f32[base + 6],
        cmd_f32[base + 7]);
}

static void stasis_stamp_display_metadata(int32_t* cmd_i32) {
    cmd_i32[STASIS_RENDER_I_LOGICAL_W] = g_display_metrics.logical_w;
    cmd_i32[STASIS_RENDER_I_LOGICAL_H] = g_display_metrics.logical_h;
    cmd_i32[STASIS_RENDER_I_NATIVE_W] = g_display_metrics.native_w;
    cmd_i32[STASIS_RENDER_I_NATIVE_H] = g_display_metrics.native_h;
    cmd_i32[STASIS_RENDER_I_DRAWABLE_W] = g_display_metrics.drawable_w;
    cmd_i32[STASIS_RENDER_I_DRAWABLE_H] = g_display_metrics.drawable_h;
    cmd_i32[STASIS_RENDER_I_SAFE_X] =
        (int32_t)floorf(g_display_metrics.safe_logical_viewport.x);
    cmd_i32[STASIS_RENDER_I_SAFE_Y] =
        (int32_t)floorf(g_display_metrics.safe_logical_viewport.y);
    cmd_i32[STASIS_RENDER_I_SAFE_W] =
        (int32_t)ceilf(g_display_metrics.safe_logical_viewport.w);
    cmd_i32[STASIS_RENDER_I_SAFE_H] =
        (int32_t)ceilf(g_display_metrics.safe_logical_viewport.h);
    cmd_i32[STASIS_RENDER_I_DISPLAY_GENERATION] = g_display_generation;
    cmd_i32[STASIS_RENDER_I_DENSITY_GENERATION] = g_density_generation;
}

static bool stasis_render_trace_is_enabled(void) {
    if (g_render_trace_enabled < 0) {
        const char* enabled = SDL_getenv("STASIS_ENABLE_TEST_INPUT");
        g_render_trace_enabled =
            enabled && enabled[0] == '1' && enabled[1] == '\0';
    }
    return g_render_trace_enabled != 0;
}

static void stasis_gfx_submit_frame(int32_t* cmd_i32, const float* cmd_f32, const uint8_t* cmd_u8) {
    /* Start before validation so a valid frame's host replay includes command
     * validation and decoding. Rejected frames return before publishing a
     * performance sample. */
    const uint64_t host_started_counter = stasis_host_performance_metrics_enabled()
        ? SDL_GetPerformanceCounter() : 0;
    StasisRenderValidation validation = stasis_render_validate(cmd_i32, cmd_f32);
    if (validation != STASIS_RENDER_VALID) {
        g_render_rejected_frames++;
        g_render_last_validation = validation;
        const uint32_t validation_mask = 1u << (uint32_t)validation;
        if ((g_render_logged_validation_mask & validation_mask) == 0) {
            SDL_Log(
                "Stasis renderer rejected frame: stage=%s failure=%s magic=%d version=%d backend=%s surface_generation=%u renderer_generation=%u",
                stasis_render_validation_stage(validation),
                stasis_render_validation_name(validation),
                cmd_i32 ? cmd_i32[STASIS_RENDER_I_MAGIC] : 0,
                cmd_i32 ? cmd_i32[STASIS_RENDER_I_VERSION] : 0,
                "sdl",
                g_resource_lifecycle.surface_generation,
                g_resource_lifecycle.renderer_generation);
            g_render_logged_validation_mask |= validation_mask;
        }
        return;
    }
    g_render_accepted_frames++;
    g_render_last_validation = STASIS_RENDER_VALID;
    if (!g_render_contract_logged || stasis_render_trace_is_enabled()) {
        g_render_last_trace = stasis_render_trace(cmd_i32, cmd_f32, cmd_u8);
    }
    stasis_stamp_display_metadata(cmd_i32);
    g_render_last_display_generation =
        cmd_i32[STASIS_RENDER_I_DISPLAY_GENERATION];
    g_render_last_density_generation =
        cmd_i32[STASIS_RENDER_I_DENSITY_GENERATION];
    g_perf_render_started_counter = host_started_counter;

    const int32_t flags = cmd_i32[STASIS_RENDER_I_FLAGS];
    const int32_t gfx_cmd_max_lines = STASIS_RENDER_MAX_LINES;
    const int32_t gfx_cmd_max_sprites = STASIS_RENDER_MAX_SPRITES;
    const int32_t gfx_cmd_max_text = STASIS_RENDER_MAX_TEXT;
    const int32_t gfx_cmd_max_text_bytes = STASIS_RENDER_TEXT_MAX_BYTES;
    const int32_t gfx_cmd_max_clips = STASIS_RENDER_MAX_CLIPS;

    int32_t line_count = cmd_i32[3];
    int32_t sprite_count = cmd_i32[4];
    int32_t text_count = cmd_i32[7];
    int32_t text_bytes_used = cmd_i32[9];

    if (line_count < 0) line_count = 0;
    if (sprite_count < 0) sprite_count = 0;
    if (text_count < 0) text_count = 0;
    if (text_bytes_used < 0) text_bytes_used = 0;

    if (line_count > gfx_cmd_max_lines) line_count = gfx_cmd_max_lines;
    const int32_t rect_count = stasis_render_rect_count(cmd_i32, line_count);
    if (sprite_count > gfx_cmd_max_sprites) sprite_count = gfx_cmd_max_sprites;
    if (text_count > gfx_cmd_max_text) text_count = gfx_cmd_max_text;
    if (text_bytes_used > gfx_cmd_max_text_bytes) text_bytes_used = gfx_cmd_max_text_bytes;

    g_perf_pending_lines = (uint32_t)line_count;
    g_perf_pending_rectangles = (uint32_t)rect_count;
    g_perf_pending_sprites = (uint32_t)sprite_count;
    g_perf_pending_text = (uint32_t)text_count;
    g_perf_pending_commands = (uint32_t)(line_count + rect_count + sprite_count + text_count);
    g_native_draw_submissions = 0;
    g_native_page_transitions = 0;
    g_native_mixed_runs = 0;
    g_native_submitted_bytes = 0;

    if (!g_render_contract_logged) {
        SDL_Log(
            "Stasis render contract v%d trace=%u flags=%d lines=%d rects=%d sprites=%d text=%d",
            cmd_i32[STASIS_RENDER_I_VERSION],
            (unsigned int)g_render_last_trace,
            flags,
            line_count,
            rect_count,
            sprite_count,
            text_count);
        g_render_contract_logged = true;
    }

    stasis_begin_frame();

    if ((flags & STASIS_RENDER_FLAG_CLEAR) != 0) {
        stasis_clear(cmd_f32[0], cmd_f32[1], cmd_f32[2], cmd_f32[3]);
    }

    const int32_t clip_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_CLIP_COUNT], gfx_cmd_max_clips);
    const int32_t order_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_ORDER_COUNT], STASIS_RENDER_MAX_ORDER);
    const int32_t sprite_run_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_SPRITE_RUN_COUNT], STASIS_RENDER_MAX_SPRITE_RUNS);
    if (order_count > 0) {
        int32_t pending_kind = 0;
        for (int32_t order_index = 0; order_index < order_count;) {
            const int32_t entry = cmd_i32[STASIS_RENDER_I_ORDER_BASE + order_index];
            if (entry < 0) { order_index++; continue; }
            const int32_t kind = entry / STASIS_RENDER_ORDER_KIND_SCALE;
            const int32_t index = entry % STASIS_RENDER_ORDER_KIND_SCALE;
            if (kind == STASIS_RENDER_ORDER_CLIP_PUSH && index < clip_count) {
                flush_ordered_lines();
                flush_ordered_sprites();
                const int32_t base = STASIS_RENDER_F_CLIP_BASE +
                    index * STASIS_RENDER_CLIP_F32_STRIDE;
                stasis_render_push_clip(
                    cmd_f32[base + 0], cmd_f32[base + 1],
                    cmd_f32[base + 2], cmd_f32[base + 3]);
                pending_kind = 0;
                order_index++;
            } else if (kind == STASIS_RENDER_ORDER_CLIP_POP && index == 0) {
                flush_ordered_lines();
                flush_ordered_sprites();
                stasis_render_pop_clip();
                pending_kind = 0;
                order_index++;
            } else if (kind == STASIS_RENDER_ORDER_LINE && index < line_count) {
                if (pending_kind == STASIS_RENDER_ORDER_SPRITE) flush_ordered_sprites();
                stasis_draw_lines_f32(
                    cmd_f32 + STASIS_RENDER_F_LINE_BASE +
                        index * STASIS_RENDER_LINE_F32_STRIDE,
                    1);
                pending_kind = kind;
                order_index++;
            } else if ((kind == STASIS_RENDER_ORDER_SPRITE && index < sprite_run_count) ||
                       (kind == STASIS_RENDER_ORDER_RECT && index < rect_count)) {
                if (pending_kind == STASIS_RENDER_ORDER_LINE) flush_ordered_lines();
                order_index = stasis_draw_mixed_order_span(
                    cmd_i32, cmd_f32, order_index, order_count, rect_count, sprite_run_count);
                pending_kind = 0;
            } else if (kind == STASIS_RENDER_ORDER_TEXT && index < text_count) {
                if (pending_kind == STASIS_RENDER_ORDER_LINE) flush_ordered_lines();
                if (pending_kind == STASIS_RENDER_ORDER_SPRITE) flush_ordered_sprites();
                stasis_draw_ordered_text(
                    cmd_i32, cmd_f32, cmd_u8, text_bytes_used, index);
                pending_kind = kind;
                order_index++;
            } else {
                order_index++;
            }
        }
        if (pending_kind == STASIS_RENDER_ORDER_LINE) flush_ordered_lines();
        if (pending_kind == STASIS_RENDER_ORDER_SPRITE) flush_ordered_sprites();
    } else {
        if (line_count > 0) {
            stasis_draw_lines_f32(cmd_f32 + STASIS_RENDER_F_LINE_BASE, line_count);
            flush_ordered_lines();
        }
        for (int32_t index = 0; index < rect_count; index++) {
            stasis_draw_ordered_rect(cmd_f32, index);
        }
        for (int32_t run = 0; run < sprite_run_count; run++) {
            const int32_t run_base = STASIS_RENDER_I_SPRITE_RUN_BASE +
                run * STASIS_RENDER_SPRITE_RUN_I32_STRIDE;
            const int32_t first = cmd_i32[run_base + 0];
            const int32_t count = cmd_i32[run_base + 1];
            for (int32_t offset = 0; offset < count; offset++) {
                stasis_draw_ordered_sprite(cmd_i32, cmd_f32, first + offset);
            }
        }
        if (sprite_count > 0) flush_ordered_sprites();
        for (int32_t index = 0; index < text_count; index++) {
            stasis_draw_ordered_text(cmd_i32, cmd_f32, cmd_u8, text_bytes_used, index);
        }
    }

    /* A valid v7 stream is balanced; reset defensively before postfx/present so
     * a malformed host-side sequence cannot leak clipping into the next frame. */
    stasis_render_reset_clip();

    /* Present only if requested (lets benchmarks exclude swap/vsync). */
    if ((flags & STASIS_RENDER_FLAG_PRESENT) != 0) {
        stasis_perf_draw_overlay();
        stasis_end_frame();
        g_render_presented_frames++;
    } else {
        g_perf_render_started_counter = 0;
    }
}

STASIS_EXPORT int stasis_test_get_render_submission_state(int32_t* out_i32, int32_t capacity) {
    const char* enabled = SDL_getenv("STASIS_ENABLE_TEST_INPUT");
    if (!out_i32 || capacity < 5 || !enabled || enabled[0] != '1' || enabled[1] != '\0') {
        return 0;
    }
    out_i32[0] = (int32_t)g_render_accepted_frames;
    out_i32[1] = (int32_t)g_render_rejected_frames;
    out_i32[2] = (int32_t)g_render_presented_frames;
    out_i32[3] = (int32_t)g_render_last_validation;
    out_i32[4] = (int32_t)g_render_last_trace;
    if (capacity >= 7) {
        out_i32[5] = g_render_last_display_generation;
        out_i32[6] = g_render_last_density_generation;
    }
    if (capacity >= 12) {
        out_i32[7] = (int32_t)g_native_draw_submissions;
        out_i32[8] = (int32_t)g_native_page_transitions;
        out_i32[9] = (int32_t)g_native_mixed_runs;
        out_i32[10] = (int32_t)(uint32_t)g_native_submitted_bytes;
        out_i32[11] = (int32_t)(uint32_t)(g_native_submitted_bytes >> 32);
    }
    return 1;
}

STASIS_EXPORT int stasis_test_get_sprite_state(int32_t handle, int32_t* out_i32, int32_t capacity) {
    const char* enabled = SDL_getenv("STASIS_ENABLE_TEST_INPUT");
    if (!out_i32 || capacity < 4 || !enabled || enabled[0] != '1' || enabled[1] != '\0') {
        return 0;
    }
    SpriteEntry* entry = sprite_get(handle);
    out_i32[0] = entry != NULL ? 1 : 0;
    out_i32[1] = entry != NULL ? entry->ref_count : 0;
    out_i32[2] = entry != NULL ? (int32_t)entry->generation : 0;
    out_i32[3] = g_sprite_count;
    if (capacity >= 5) out_i32[4] = entry != NULL ? entry->atlas_policy.eligible : 0;
    if (capacity >= 7) {
        const uint64_t group_id = entry != NULL ? entry->atlas_policy.group_id : 0;
        out_i32[5] = (int32_t)(uint32_t)group_id;
        out_i32[6] = (int32_t)(uint32_t)(group_id >> 32);
    }
    if (capacity >= 12) {
        out_i32[7] = entry != NULL ? entry->w : 0;
        out_i32[8] = entry != NULL ? entry->h : 0;
        out_i32[9] = entry != NULL ? entry->needs_reraster : 0;
        out_i32[10] = entry != NULL ? entry->max_w : 0;
        out_i32[11] = entry != NULL ? entry->max_h : 0;
    }
    return 1;
}

STASIS_EXPORT void stasis_gfx_submit(int32_t* cmd_i32, const float* cmd_f32) {
    stasis_gfx_submit_frame(cmd_i32, cmd_f32, NULL);
}

STASIS_EXPORT void stasis_gfx_submit_u8(int32_t* cmd_i32, const float* cmd_f32, const uint8_t* cmd_u8) {
    stasis_gfx_submit_frame(cmd_i32, cmd_f32, cmd_u8);
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

static uint64_t stasis_resource_source_bytes(const char* path) {
    if (!path || !*path) return 0;
    FILE* file = fopen(path, "rb");
    if (!file) return 0;
    if (fseek(file, 0, SEEK_END) != 0) {
        fclose(file);
        return 0;
    }
    const long size = ftell(file);
    fclose(file);
    return size > 0 ? (uint64_t)size : 0;
}

static void stasis_log_sprite_preparation(
    const SpriteEntry* entry, const char* path, int replaces_existing
) {
    if (!gfx_should_log_sprite_loads() || !entry) return;
    int handle = 0;
    for (int i = 0; i < g_sprite_capacity; i++) {
        if (&g_sprites[i] == entry) {
            handle = sprite_handle_for_slot(i);
            break;
        }
    }
    SDL_Log(
        "Stasis resource preparation: kind=sprite event=%s handle=%d path=%s logical=%dx%d raster=%dx%d source_bytes=%llu density_generation=%d",
        replaces_existing ? "replace" : "initial", handle,
        path ? path : "<unknown>", entry->max_w, entry->max_h,
        entry->w, entry->h,
        (unsigned long long)stasis_resource_source_bytes(path), g_density_generation);
}

static void stasis_log_font_preparation(const StasisFont* font, int replaces_existing) {
    if (!gfx_should_log_sprite_loads() || !font) return;
    int handle = 0;
    for (int i = 0; i < MAX_FONTS; i++) {
        if (&g_fonts[i] == font) {
            handle = i + 1;
            break;
        }
    }
    SDL_Log(
        "Stasis resource preparation: kind=font event=%s handle=%d path=%s logical_size=%d raster_size=%d atlas=%dx%d source_bytes=%llu density_generation=%d",
        replaces_existing ? "replace" : "initial", handle,
        font->source_path[0] ? font->source_path : "<unknown>",
        font->font_size, font->raster_size, font->atlas_size, font->atlas_size,
        (unsigned long long)font->source_size, g_density_generation);
}

static void stasis_sprite_atlas_reset(int destroy_textures) {
    for (int i = 0; i < g_sprite_atlas_page_count; i++) {
        if (destroy_textures && g_sprite_atlas_pages[i].texture) {
            SDL_DestroyTexture(g_sprite_atlas_pages[i].texture);
        }
    }
    memset(g_sprite_atlas_pages, 0, sizeof(g_sprite_atlas_pages));
    g_sprite_atlas_page_count = 0;
}

#define STASIS_MIXED_GEOMETRY_BATCH_QUADS 2048
static SDL_Vertex g_mixed_geometry_vertices[STASIS_MIXED_GEOMETRY_BATCH_QUADS * 4];
static int g_mixed_geometry_indices[STASIS_MIXED_GEOMETRY_BATCH_QUADS * 6];
static int g_mixed_geometry_indices_ready = 0;

static void stasis_prepare_mixed_geometry_indices(void) {
    if (g_mixed_geometry_indices_ready) return;
    for (int i = 0; i < STASIS_MIXED_GEOMETRY_BATCH_QUADS; i++) {
        const int v = i * 4;
        const int n = i * 6;
        g_mixed_geometry_indices[n + 0] = v + 0;
        g_mixed_geometry_indices[n + 1] = v + 1;
        g_mixed_geometry_indices[n + 2] = v + 2;
        g_mixed_geometry_indices[n + 3] = v + 0;
        g_mixed_geometry_indices[n + 4] = v + 2;
        g_mixed_geometry_indices[n + 5] = v + 3;
    }
    g_mixed_geometry_indices_ready = 1;
}

static void stasis_submit_mixed_geometry(SDL_Texture* texture, int quad_count) {
    if (!g_renderer || !texture || quad_count <= 0) return;
    SDL_SetTextureColorMod(texture, 255, 255, 255);
    SDL_SetTextureAlphaMod(texture, 255);
    SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_BLEND);
    SDL_RenderGeometry(g_renderer, texture, g_mixed_geometry_vertices,
        quad_count * 4, g_mixed_geometry_indices, quad_count * 6);
    g_native_draw_submissions++;
    g_native_submitted_bytes += (uint64_t)quad_count *
        (uint64_t)(sizeof(SDL_Vertex) * 4u + sizeof(int) * 6u);
}

static void stasis_mixed_set_quad(
    int quad, const SDL_FPoint points[4], SDL_FColor color,
    float u0, float v0, float u1, float v1
) {
    SDL_Vertex* v = &g_mixed_geometry_vertices[quad * 4];
    static const int uv_x[4] = {0, 1, 1, 0};
    static const int uv_y[4] = {0, 0, 1, 1};
    for (int i = 0; i < 4; i++) {
        v[i].position = points[i];
        v[i].color = color;
        v[i].tex_coord.x = uv_x[i] ? u1 : u0;
        v[i].tex_coord.y = uv_y[i] ? v1 : v0;
    }
}

static SpriteEntry* stasis_mixed_sprite_entry(const int32_t* cmd_i32, int sprite_index) {
    const int32_t* lane = cmd_i32 + STASIS_RENDER_I_SPRITE_BASE;
    SpriteEntry* entry = sprite_get(lane[sprite_index * STASIS_RENDER_SPRITE_I32_STRIDE]);
    if (!entry) entry = sprite_fallback_get();
    if (entry && entry->needs_reraster && entry->path) {
        sprite_build_into_entry_sized(entry, entry->path, entry->max_w, entry->max_h);
    }
    if (!entry || !entry->sdl_tex || entry->page_index < 0 ||
        entry->surface_generation != g_resource_lifecycle.surface_generation ||
        entry->renderer_generation != g_resource_lifecycle.renderer_generation) return NULL;
    return entry;
}

static int stasis_mixed_lookahead_page(
    const int32_t* cmd_i32, int order_index, int order_count, int sprite_run_count
) {
    const int limit = order_index + STASIS_MIXED_GEOMETRY_BATCH_QUADS < order_count
        ? order_index + STASIS_MIXED_GEOMETRY_BATCH_QUADS : order_count;
    for (int i = order_index; i < limit; i++) {
        const int32_t order = cmd_i32[STASIS_RENDER_I_ORDER_BASE + i];
        if (order < 0) break;
        const int kind = order / STASIS_RENDER_ORDER_KIND_SCALE;
        const int index = order % STASIS_RENDER_ORDER_KIND_SCALE;
        if (kind == STASIS_RENDER_ORDER_RECT) continue;
        if (kind != STASIS_RENDER_ORDER_SPRITE || index >= sprite_run_count) break;
        const int run_base = STASIS_RENDER_I_SPRITE_RUN_BASE +
            index * STASIS_RENDER_SPRITE_RUN_I32_STRIDE;
        if (cmd_i32[run_base + 1] <= 0) continue;
        SpriteEntry* entry = stasis_mixed_sprite_entry(cmd_i32, cmd_i32[run_base + 0]);
        return entry ? entry->page_index : -1;
    }
    SpriteEntry* fallback = sprite_fallback_get();
    return fallback ? fallback->page_index : -1;
}

static void stasis_mixed_add_rect(const float* cmd_f32, int index, int quad, int page_index) {
    const int base = STASIS_RENDER_F_RECT_REVERSE_BASE -
        index * STASIS_RENDER_GEOMETRY_F32_STRIDE;
    const float x = cmd_f32[base + 0];
    const float y = cmd_f32[base + 1];
    const float w = cmd_f32[base + 2];
    const float h = cmd_f32[base + 3];
    SDL_FPoint points[4] = {{x,y}, {x+w,y}, {x+w,y+h}, {x,y+h}};
    SDL_FColor color = {cmd_f32[base + 4], cmd_f32[base + 5],
        cmd_f32[base + 6], cmd_f32[base + 7]};
    const StasisSdlAtlasPage* page = &g_sprite_atlas_pages[page_index];
    const float u0 = (float)page->white_x / (float)page->width;
    const float v0 = (float)page->white_y / (float)page->height;
    const float u1 = (float)(page->white_x + STASIS_SDL_ATLAS_WHITE_SIZE) / (float)page->width;
    const float v1 = (float)(page->white_y + STASIS_SDL_ATLAS_WHITE_SIZE) / (float)page->height;
    stasis_mixed_set_quad(quad, points, color, u0, v0, u1, v1);
}

static int stasis_mixed_add_sprite(
    const int32_t* cmd_i32, const float* cmd_f32, int index, int quad, SpriteEntry* entry
) {
    const int base_i = STASIS_RENDER_I_SPRITE_BASE +
        index * STASIS_RENDER_SPRITE_I32_STRIDE;
    const int base_f = STASIS_RENDER_F_SPRITE_BASE +
        index * STASIS_RENDER_SPRITE_F32_STRIDE;
    const float x = cmd_f32[base_f + 0], y = cmd_f32[base_f + 1];
    const float w = cmd_f32[base_f + 2], h = cmd_f32[base_f + 3];
    float sx = cmd_f32[base_f + 4], sy = cmd_f32[base_f + 5];
    float sw = cmd_f32[base_f + 6], sh = cmd_f32[base_f + 7];
    const float px = cmd_f32[base_f + 8], py = cmd_f32[base_f + 9];
    const float scale_x = cmd_f32[base_f + 10], scale_y = cmd_f32[base_f + 11];
    if (sw == 0.0f && sh == 0.0f) { sx = 0; sy = 0; sw = (float)entry->w; sh = (float)entry->h; }
    if (w <= 0 || h <= 0 || scale_x == 0 || scale_y == 0 || sx < 0 || sy < 0 ||
        sw <= 0 || sh <= 0 || sx + sw > entry->w || sy + sh > entry->h) return 0;
    float transformed[8];
    stasis_mixed_quad_transform(x, y, w, h, px, py, scale_x, scale_y,
        cmd_f32[base_f + 12], transformed);
    SDL_FPoint points[4];
    for (int i = 0; i < 4; i++) {
        points[i].x = transformed[i * 2];
        points[i].y = transformed[i * 2 + 1];
    }
    const uint32_t tint = (uint32_t)cmd_i32[base_i + 1];
    SDL_FColor color = {(float)((tint >> 24) & 255u) / 255.0f,
        (float)((tint >> 16) & 255u) / 255.0f,
        (float)((tint >> 8) & 255u) / 255.0f, (float)(tint & 255u) / 255.0f};
    const StasisSdlAtlasPage* page = &g_sprite_atlas_pages[entry->page_index];
    stasis_mixed_set_quad(quad, points, color,
        (entry->atlas_x + sx) / page->width, (entry->atlas_y + sy) / page->height,
        (entry->atlas_x + sx + sw) / page->width, (entry->atlas_y + sy + sh) / page->height);
    return 1;
}

static int stasis_draw_mixed_order_span(
    const int32_t* cmd_i32, const float* cmd_f32, int start, int order_count,
    int rect_count, int sprite_run_count
) {
    stasis_prepare_mixed_geometry_indices();
    int order_index = start;
    int quad_count = 0;
    int page_index = -1;
    g_native_mixed_runs++;
    while (order_index < order_count) {
        const int32_t order = cmd_i32[STASIS_RENDER_I_ORDER_BASE + order_index];
        if (order < 0) break;
        const int kind = order / STASIS_RENDER_ORDER_KIND_SCALE;
        const int index = order % STASIS_RENDER_ORDER_KIND_SCALE;
        if (kind != STASIS_RENDER_ORDER_RECT && kind != STASIS_RENDER_ORDER_SPRITE) break;
        if (kind == STASIS_RENDER_ORDER_RECT) {
            if (index >= rect_count) { order_index++; continue; }
            int desired_page = page_index;
            if (desired_page < 0) desired_page = stasis_mixed_lookahead_page(
                cmd_i32, order_index + 1, order_count, sprite_run_count);
            if (desired_page < 0) { stasis_draw_ordered_rect(cmd_f32, index); order_index++; continue; }
            if (page_index < 0) page_index = desired_page;
            stasis_mixed_add_rect(cmd_f32, index, quad_count++, page_index);
        } else {
            if (index >= sprite_run_count) { order_index++; continue; }
            const int run_base = STASIS_RENDER_I_SPRITE_RUN_BASE +
                index * STASIS_RENDER_SPRITE_RUN_I32_STRIDE;
            const int first = cmd_i32[run_base + 0];
            const int count = cmd_i32[run_base + 1];
            for (int offset = 0; offset < count; offset++) {
                SpriteEntry* entry = stasis_mixed_sprite_entry(cmd_i32, first + offset);
                if (!entry) continue;
                if (page_index >= 0 && entry->page_index != page_index) {
                    stasis_submit_mixed_geometry(g_sprite_atlas_pages[page_index].texture, quad_count);
                    quad_count = 0;
                    g_native_page_transitions++;
                }
                page_index = entry->page_index;
                if (stasis_mixed_add_sprite(cmd_i32, cmd_f32, first + offset, quad_count, entry)) quad_count++;
                if (quad_count == STASIS_MIXED_GEOMETRY_BATCH_QUADS) {
                    stasis_submit_mixed_geometry(g_sprite_atlas_pages[page_index].texture, quad_count);
                    quad_count = 0;
                }
            }
        }
        order_index++;
        if (quad_count == STASIS_MIXED_GEOMETRY_BATCH_QUADS) {
            stasis_submit_mixed_geometry(g_sprite_atlas_pages[page_index].texture, quad_count);
            quad_count = 0;
        }
    }
    if (quad_count > 0 && page_index >= 0) {
        stasis_submit_mixed_geometry(g_sprite_atlas_pages[page_index].texture, quad_count);
    }
    return order_index;
}

static int stasis_sprite_atlas_create_page(int width, int height, uint64_t group_id, int dedicated) {
    if (!g_renderer || width <= 0 || height <= 0 ||
        g_sprite_atlas_page_count >= STASIS_SDL_ATLAS_MAX_PAGES) return -1;
    const size_t bytes = (size_t)width * (size_t)height * 4u;
    if (bytes / 4u != (size_t)width * (size_t)height) return -1;
    unsigned char* initial = (unsigned char*)calloc(bytes, 1u);
    if (!initial) return -1;
    StasisSdlAtlasPage* page = &g_sprite_atlas_pages[g_sprite_atlas_page_count];
    page->texture = SDL_CreateTexture(
        g_renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_STATIC, width, height);
    if (!page->texture || !SDL_UpdateTexture(page->texture, NULL, initial, width * 4)) {
        if (page->texture) SDL_DestroyTexture(page->texture);
        free(initial);
        memset(page, 0, sizeof(*page));
        return -1;
    }
    free(initial);
    SDL_SetTextureBlendMode(page->texture, SDL_BLENDMODE_BLEND);
    SDL_SetTextureScaleMode(page->texture, SDL_SCALEMODE_LINEAR);
    page->width = width;
    page->height = height;
    page->white_x = 1;
    page->white_y = 1;
    page->placeholder_x = 5;
    page->placeholder_y = 1;
    page->cursor_x = 1;
    page->cursor_y = 6;
    page->group_id = group_id;
    page->dedicated = dedicated;
    static const unsigned char white[16] = {
        255,255,255,255, 255,255,255,255,
        255,255,255,255, 255,255,255,255};
    static const unsigned char placeholder[16] = {
        255,0,255,255, 24,24,24,255,
        24,24,24,255, 255,0,255,255};
    SDL_Rect white_rect = {page->white_x, page->white_y, 2, 2};
    SDL_Rect placeholder_rect = {page->placeholder_x, page->placeholder_y, 2, 2};
    if (!SDL_UpdateTexture(page->texture, &white_rect, white, 8) ||
        !SDL_UpdateTexture(page->texture, &placeholder_rect, placeholder, 8)) {
        SDL_DestroyTexture(page->texture);
        memset(page, 0, sizeof(*page));
        return -1;
    }
    return g_sprite_atlas_page_count++;
}

static int stasis_sprite_atlas_reserve_on_page(
    StasisSdlAtlasPage* page, int w, int h, int* out_x, int* out_y
) {
    const int alloc_w = w + STASIS_SDL_ATLAS_PADDING * 2;
    const int alloc_h = h + STASIS_SDL_ATLAS_PADDING * 2;
    int x = page->cursor_x;
    int y = page->cursor_y;
    if (x + alloc_w > page->width) {
        x = 1;
        y += page->row_h;
        page->row_h = 0;
    }
    if (y + alloc_h > page->height) return 0;
    page->cursor_x = x + alloc_w;
    if (alloc_h > page->row_h) page->row_h = alloc_h;
    *out_x = x + STASIS_SDL_ATLAS_PADDING;
    *out_y = y + STASIS_SDL_ATLAS_PADDING;
    return 1;
}

static int stasis_sprite_atlas_allocate(
    const StasisSpriteAtlasPolicyV3* policy, int logical_w, int logical_h,
    int w, int h, int* out_x, int* out_y
) {
    const int eligible = policy && policy->eligible;
    const uint64_t group_id = eligible ? policy->group_id : 0;
    if (eligible && w + 2 <= STASIS_SDL_ATLAS_PAGE_SIZE &&
        h + 8 <= STASIS_SDL_ATLAS_PAGE_SIZE) {
        for (int i = 0; i < g_sprite_atlas_page_count; i++) {
            StasisSdlAtlasPage* page = &g_sprite_atlas_pages[i];
            if (page->dedicated || page->group_id != group_id) continue;
            if (stasis_sprite_atlas_reserve_on_page(page, w, h, out_x, out_y)) return i;
        }
        int page_w = STASIS_SDL_ATLAS_PAGE_SIZE;
        int page_h = STASIS_SDL_ATLAS_PAGE_SIZE;
        if (!stasis_sprite_atlas_page_size_v3(policy, logical_w, logical_h, w, h,
                STASIS_SDL_ATLAS_PAGE_SIZE, STASIS_SDL_ATLAS_PAGE_SIZE,
                STASIS_SDL_ATLAS_PAGE_SIZE, STASIS_SDL_ATLAS_PADDING, &page_w, &page_h)) {
            return -1;
        }
        if (page_w < 8) page_w = 8;
        if (page_h < h + 8) page_h = stasis_sprite_atlas_next_extent(
            h + 8, STASIS_SDL_ATLAS_PAGE_SIZE);
        const int page_index = stasis_sprite_atlas_create_page(page_w, page_h, group_id, 0);
        if (page_index < 0) return -1;
        return stasis_sprite_atlas_reserve_on_page(
            &g_sprite_atlas_pages[page_index], w, h, out_x, out_y) ? page_index : -1;
    }
    const int width = w + 10;
    const int height = h + 8;
    const int page_index = stasis_sprite_atlas_create_page(width, height, group_id, 1);
    if (page_index < 0) return -1;
    return stasis_sprite_atlas_reserve_on_page(
        &g_sprite_atlas_pages[page_index], w, h, out_x, out_y) ? page_index : -1;
}

static int stasis_sprite_atlas_upload(
    int page_index, int x, int y, const unsigned char* pixels, int w, int h
) {
    if (page_index < 0 || page_index >= g_sprite_atlas_page_count) return 0;
    const int padded_w = w + 2;
    const int padded_h = h + 2;
    unsigned char* padded = (unsigned char*)malloc((size_t)padded_w * (size_t)padded_h * 4u);
    if (!padded) return 0;
    for (int py = 0; py < padded_h; py++) {
        int sy = py - 1;
        if (sy < 0) sy = 0;
        if (sy >= h) sy = h - 1;
        for (int px = 0; px < padded_w; px++) {
            int sx = px - 1;
            if (sx < 0) sx = 0;
            if (sx >= w) sx = w - 1;
            memcpy(padded + ((size_t)py * padded_w + px) * 4u,
                pixels + ((size_t)sy * w + sx) * 4u, 4u);
        }
    }
    SDL_Rect rect = {x - 1, y - 1, padded_w, padded_h};
    const int ok = SDL_UpdateTexture(
        g_sprite_atlas_pages[page_index].texture, &rect, padded, padded_w * 4);
    free(padded);
    return ok;
}


/*
 * Build sprite at specified max size. Used for sized loading and re-rasterization.
 */
static int sprite_publish_pixels_into_entry(
    SpriteEntry* e,
    const char* path,
    int max_w,
    int max_h,
    unsigned char* pixels,
    int w,
    int h
) {
    const int replaces_existing = e->w > 0 && e->h > 0;
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

        int atlas_x = e->atlas_x;
        int atlas_y = e->atlas_y;
        int page_index = e->page_index;
        const int page_compatible = page_index >= 0 && page_index < g_sprite_atlas_page_count &&
            ((e->atlas_policy.eligible && !g_sprite_atlas_pages[page_index].dedicated &&
              g_sprite_atlas_pages[page_index].group_id == e->atlas_policy.group_id) ||
             (!e->atlas_policy.eligible && g_sprite_atlas_pages[page_index].dedicated));
        if (!page_compatible || e->alloc_w < w + 2 || e->alloc_h < h + 2) {
            page_index = stasis_sprite_atlas_allocate(
                &e->atlas_policy, max_w, max_h, w, h, &atlas_x, &atlas_y);
        }
        if (page_index < 0 ||
            !stasis_sprite_atlas_upload(page_index, atlas_x, atlas_y, pixels, w, h)) {
            SDL_Log("gfx_load_sprite: SDL atlas allocation/upload failed: %s", SDL_GetError());
            free(pixels);
            return 0;
        }

        free(pixels);
        StasisSdlAtlasPage* page = &g_sprite_atlas_pages[page_index];
        e->w = w;
        e->h = h;
        e->max_w = max_w;
        e->max_h = max_h;
        e->page_index = page_index;
        e->atlas_x = atlas_x;
        e->atlas_y = atlas_y;
        e->alloc_x = atlas_x - 1;
        e->alloc_y = atlas_y - 1;
        e->alloc_w = w + 2;
        e->alloc_h = h + 2;
        e->u0 = (float)atlas_x / (float)page->width;
        e->v0 = (float)atlas_y / (float)page->height;
        e->u1 = (float)(atlas_x + w) / (float)page->width;
        e->v1 = (float)(atlas_y + h) / (float)page->height;
        e->sdl_tex = page->texture;
        e->mtime = get_file_mtime(path);
        e->needs_reraster = 0;
        e->surface_generation = g_resource_lifecycle.surface_generation;
        e->renderer_generation = g_resource_lifecycle.renderer_generation;
        stasis_log_sprite_preparation(e, path, replaces_existing);
        return 1;
}

static int sprite_build_into_entry_sized(SpriteEntry* e, const char* path, int max_w, int max_h) {
    const int raster_w = stasis_current_scaled_extent(max_w);
    const int raster_h = stasis_current_scaled_extent(max_h);
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
    return sprite_publish_pixels_into_entry(e, path, max_w, max_h, pixels, w, h);
}

static void gfx_asset_watch_apply_pending_changes(void) {
    int dirty = 0;
    int force_reload = 0;
    int pending_path_count = 0;
    char pending_paths[GFX_ASSET_WATCH_MAX_PENDING_PATHS][GFX_ASSET_WATCH_PATH_SIZE];
#if defined(_WIN32)
    dirty = InterlockedExchange(&g_asset_watch_dirty, 0);
    force_reload = InterlockedExchange(&g_asset_watch_force_reload, 0);
    AcquireSRWLockExclusive(&g_asset_watch_path_lock);
    pending_path_count = g_asset_watch_pending_path_count;
    if (pending_path_count > 0) {
        memcpy(pending_paths, g_asset_watch_pending_paths,
               sizeof(pending_paths[0]) * (size_t)pending_path_count);
    }
    g_asset_watch_pending_path_count = 0;
    ReleaseSRWLockExclusive(&g_asset_watch_path_lock);
#else
    dirty = g_asset_watch_dirty;
    g_asset_watch_dirty = 0;
    force_reload = g_asset_watch_force_reload;
    g_asset_watch_force_reload = 0;
    pending_path_count = g_asset_watch_pending_path_count;
    if (pending_path_count > 0) {
        memcpy(pending_paths, g_asset_watch_pending_paths,
               sizeof(pending_paths[0]) * (size_t)pending_path_count);
    }
    g_asset_watch_pending_path_count = 0;
#endif
    if (!dirty) {
        return;
    }

    if (pending_path_count > 0 && !force_reload) {
        /* If any explicit notification cannot be matched to a retained cache entry,
         * conservatively invalidate all entries. This covers deleted files and any
         * path spelling that the platform could not canonicalize. */
        for (int path_index = 0; path_index < pending_path_count; path_index++) {
            int matched = 0;
            for (int i = 0; i < g_sprite_capacity; i++) {
                SpriteEntry* e = &g_sprites[i];
                if (!e->used || !e->path) continue;
#if defined(_WIN32)
                if (_stricmp(e->path, pending_paths[path_index]) == 0) {
#else
                if (strcmp(e->path, pending_paths[path_index]) == 0) {
#endif
                    matched = 1;
                    break;
                }
            }
            if (!matched) {
                force_reload = 1;
                break;
            }
        }
    }

    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* e = &g_sprites[i];
        if (!e->used || !e->path) continue;

        int path_matches = 0;
        for (int path_index = 0; path_index < pending_path_count; path_index++) {
#if defined(_WIN32)
            if (_stricmp(e->path, pending_paths[path_index]) == 0) {
#else
            if (strcmp(e->path, pending_paths[path_index]) == 0) {
#endif
                path_matches = 1;
                break;
            }
        }
        if (pending_path_count > 0 && !force_reload && !path_matches) continue;

        uint64_t mt = get_file_mtime(e->path);
        if (!force_reload && !path_matches && (!mt || mt <= e->mtime)) continue;

        if (!sprite_build_into_entry_sized(e, e->path, e->max_w, e->max_h)) {
            SDL_Log("gfx_watch: reload failed for %s", e->path);
        } else {
            e->reload_pending = 1;
        }
    }
}

/*
 * Load and bake a sprite from an SVG file at a specified max size.
 * The sprite will be rasterized to fit within max_w x max_h while preserving aspect ratio.
 * Returns an integer handle (stable for the lifetime of the process).
 */
STASIS_EXPORT int stasis_gfx_load_sprite(const char* path, int max_w, int max_h) {
    const StasisSpriteAtlasPolicyV3 atlas_policy = g_next_sprite_atlas_policy_v3;
    g_next_sprite_atlas_policy_v3 = stasis_sprite_atlas_policy_v3_standalone();
    if (!path || !*path) return 0;
    if (!g_window) return 0;
    if (!g_renderer) return 0;
    if (max_w <= 0 || max_h <= 0) return 0;

    char resolved[1024];
    if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
        stasis_report_runtime_errorf("Sprite path could not be resolved: %s", path);
        SDL_Log("gfx_load_sprite: could not resolve %s", path);
        return 0;
    }
    const int raster_w = stasis_current_scaled_extent(max_w);
    const int raster_h = stasis_current_scaled_extent(max_h);

    /* Reuse the device-local raster/GPU texture for the same source and
     * logical target size. Drawable-density changes mark the entry dirty and
     * replace its raster before it is returned or drawn, so the effective key
     * is source + logical extent + current drawable extent. */
    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* cached = &g_sprites[i];
        if (!cached->used || !cached->path) continue;
        if (cached->max_w != max_w || cached->max_h != max_h) continue;
        if (strcmp(cached->path, resolved) != 0) continue;
        const StasisSpriteAtlasPolicyV3 previous_atlas_policy = cached->atlas_policy;
        const int previous_needs_reraster = cached->needs_reraster;
        if (!stasis_sprite_atlas_policy_v3_equal(&cached->atlas_policy, &atlas_policy)) {
            cached->atlas_policy = atlas_policy;
            cached->needs_reraster = 1;
        }
        if (cached->w != raster_w || cached->h != raster_h) {
            cached->needs_reraster = 1;
        }
        if (cached->needs_reraster &&
            !sprite_build_into_entry_sized(cached, resolved, max_w, max_h)) {
            cached->atlas_policy = previous_atlas_policy;
            cached->needs_reraster = previous_needs_reraster;
            stasis_report_runtime_errorf("Sprite failed to reload: %s", path);
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
    e->atlas_policy = atlas_policy;
    e->path = stasis_strdup(resolved);
    if (!e->path) return 0;
    e->used = 1;
    e->ref_count = 1;
    if (!sprite_build_into_entry_sized(e, resolved, max_w, max_h)) {
        stasis_report_runtime_errorf("Sprite failed to load: %s", path);
        SDL_Log("gfx_load_sprite: failed path=%s resolved=%s", path, resolved);
        free(e->path);
        memset(e, 0, sizeof(*e));
        e->generation = generation;
        e->page_index = -1;
        return 0;
    }

    g_sprite_count++;
    if (gfx_should_log_sprite_loads()) {
        const int page_count_for_log = 0;
        SDL_Log("gfx_load_sprite: %s (%dx%d) -> handle=%d raster=%dx%d backend=%s page=%d pages=%d sprites=%d/%d",
                resolved, max_w, max_h, sprite_handle_for_slot(slot), e->w, e->h,
                "sdl",
                e->page_index,
                page_count_for_log,
                g_sprite_count,
                g_sprite_capacity);
    }
    return sprite_handle_for_slot(slot);
}

static int stasis_gfx_publish_sprite_task(StasisAssetTask* task) {
    if (!task || !task->pixels || task->pixel_w <= 0 || task->pixel_h <= 0) return 0;
    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* cached = &g_sprites[i];
        if (!cached->used || !cached->path) continue;
        if (cached->max_w != task->max_w || cached->max_h != task->max_h) continue;
        if (strcmp(cached->path, task->path) != 0) continue;
        const StasisSpriteAtlasPolicyV3 previous_atlas_policy = cached->atlas_policy;
        const int previous_needs_reraster = cached->needs_reraster;
        if (!stasis_sprite_atlas_policy_v3_equal(&cached->atlas_policy, &task->atlas_policy)) {
            cached->atlas_policy = task->atlas_policy;
            cached->needs_reraster = 1;
        }
        if (cached->needs_reraster) {
            unsigned char* pixels = task->pixels;
            task->pixels = NULL;
            if (!sprite_publish_pixels_into_entry(
                    cached,
                    task->path,
                    task->max_w,
                    task->max_h,
                    pixels,
                    task->pixel_w,
                    task->pixel_h)) {
                cached->atlas_policy = previous_atlas_policy;
                cached->needs_reraster = previous_needs_reraster;
                return 0;
            }
        }
        if (cached->ref_count == INT_MAX) return 0;
        cached->ref_count++;
        free(task->pixels);
        task->pixels = NULL;
        return sprite_handle_for_slot(i);
    }

    if (!ensure_sprite_table_capacity(1)) return 0;
    int slot = -1;
    while (slot < 0) {
        for (int i = 0; i < g_sprite_capacity; i++) {
            if (!g_sprites[i].used && !g_sprites[i].retired) {
                slot = i;
                break;
            }
        }
        if (slot < 0 && !ensure_sprite_table_capacity(g_sprite_capacity + 1)) return 0;
    }

    SpriteEntry* entry = &g_sprites[slot];
    uint32_t generation = entry->generation;
    memset(entry, 0, sizeof(*entry));
    entry->generation = generation;
    entry->page_index = -1;
    entry->atlas_policy = task->atlas_policy;
    entry->path = stasis_strdup(task->path);
    if (!entry->path) return 0;
    entry->used = 1;
    entry->ref_count = 1;
    unsigned char* pixels = task->pixels;
    task->pixels = NULL;
    if (!sprite_publish_pixels_into_entry(
            entry,
            task->path,
            task->max_w,
            task->max_h,
            pixels,
            task->pixel_w,
            task->pixel_h)) {
        free(entry->path);
        memset(entry, 0, sizeof(*entry));
        entry->generation = generation;
        entry->page_index = -1;
        return 0;
    }
    g_sprite_count++;
    return sprite_handle_for_slot(slot);
}

STASIS_EXPORT int stasis_asset_request_sprite(const char* path, int max_w, int max_h) {
    return stasis_asset_task_request(
        STASIS_ASSET_KIND_SPRITE, path, max_w, max_h,
        stasis_sprite_atlas_policy_v3_standalone());
}

STASIS_EXPORT int stasis_asset_request_sprite_with_policy(
    const char* path,
    int max_w,
    int max_h,
    int atlas_eligible
) {
    (void)atlas_eligible;
    return stasis_asset_task_request(
        STASIS_ASSET_KIND_SPRITE, path, max_w, max_h,
        stasis_sprite_atlas_policy_v3_standalone());
}

STASIS_EXPORT int stasis_asset_request_sprite_with_policy_v3(
    const char* path,
    int max_w,
    int max_h,
    int atlas_eligible,
    uint64_t group_id,
    uint32_t member_count,
    uint64_t logical_pixel_area,
    uint32_t max_logical_width,
    uint32_t max_logical_height
) {
    return stasis_asset_task_request(
        STASIS_ASSET_KIND_SPRITE,
        path,
        max_w,
        max_h,
        stasis_sprite_atlas_policy_v3_make(
            atlas_eligible,
            group_id,
            member_count,
            logical_pixel_area,
            max_logical_width,
            max_logical_height));
}

STASIS_EXPORT int stasis_asset_request_audio(const char* path) {
    return stasis_asset_task_request(
        STASIS_ASSET_KIND_AUDIO, path, 0, 0,
        stasis_sprite_atlas_policy_v3_standalone());
}

static StasisAssetTask* stasis_asset_task_find_locked(int task_id) {
    if (task_id <= 0) return NULL;
    for (int i = 0; i < STASIS_ASSET_TASK_CAPACITY; i++) {
        if (g_asset_tasks[i].id == task_id &&
            g_asset_tasks[i].state != STASIS_ASSET_TASK_NONE) return &g_asset_tasks[i];
    }
    return NULL;
}

STASIS_EXPORT int stasis_asset_task_poll(int task_id) {
    if (!g_asset_task_mutex) return STASIS_ASSET_TASK_NONE;
    SDL_LockMutex(g_asset_task_mutex);
    StasisAssetTask* task = stasis_asset_task_find_locked(task_id);
    if (!task) {
        SDL_UnlockMutex(g_asset_task_mutex);
        return STASIS_ASSET_TASK_NONE;
    }
    if (task->state != STASIS_ASSET_TASK_DECODED) {
        int state = task->state;
        SDL_UnlockMutex(g_asset_task_mutex);
        return state;
    }
    if (task->kind == STASIS_ASSET_KIND_SPRITE) {
        int current_raster_w = stasis_current_scaled_extent(task->max_w);
        int current_raster_h = stasis_current_scaled_extent(task->max_h);
        if (task->raster_w != current_raster_w || task->raster_h != current_raster_h) {
            free(task->pixels);
            task->pixels = NULL;
            task->pixel_w = 0;
            task->pixel_h = 0;
            task->raster_w = current_raster_w;
            task->raster_h = current_raster_h;
            task->state = STASIS_ASSET_TASK_PENDING;
            SDL_SignalCondition(g_asset_task_condition);
            SDL_UnlockMutex(g_asset_task_mutex);
            return STASIS_ASSET_TASK_PENDING;
        }
    }
    task->state = STASIS_ASSET_TASK_PUBLISHING;
    int kind = task->kind;
    SDL_SignalCondition(g_asset_task_condition);
    SDL_UnlockMutex(g_asset_task_mutex);

    int handle = 0;
    if (kind == STASIS_ASSET_KIND_SPRITE) {
        handle = stasis_gfx_publish_sprite_task(task);
    } else if (kind == STASIS_ASSET_KIND_AUDIO && (g_audio_stream || g_recording_audio_enabled)) {
        if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
        handle = stasis_audio_assets_store_decoded(&g_audio_assets, &task->audio);
        if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
    }

    SDL_LockMutex(g_asset_task_mutex);
    task = stasis_asset_task_find_locked(task_id);
    if (!task) {
        SDL_UnlockMutex(g_asset_task_mutex);
        return STASIS_ASSET_TASK_NONE;
    }
    task->handle = handle;
    task->state = handle > 0 ? STASIS_ASSET_TASK_LOADED : STASIS_ASSET_TASK_FAILED;
    int state = task->state;
    SDL_UnlockMutex(g_asset_task_mutex);
    return state;
}

STASIS_EXPORT int stasis_asset_task_take_handle(int task_id) {
    if (!g_asset_task_mutex) return 0;
    SDL_LockMutex(g_asset_task_mutex);
    StasisAssetTask* task = stasis_asset_task_find_locked(task_id);
    if (!task || task->state != STASIS_ASSET_TASK_LOADED) {
        SDL_UnlockMutex(g_asset_task_mutex);
        return 0;
    }
    int handle = task->handle;
    task->handle = 0;
    stasis_asset_task_clear(task);
    SDL_UnlockMutex(g_asset_task_mutex);
    return handle;
}

STASIS_EXPORT void stasis_asset_task_cancel(int task_id) {
    if (!g_asset_task_mutex) return;
    int kind = 0;
    int handle = 0;
    SDL_LockMutex(g_asset_task_mutex);
    StasisAssetTask* task = stasis_asset_task_find_locked(task_id);
    if (!task) {
        SDL_UnlockMutex(g_asset_task_mutex);
        return;
    }
    if (task->state == STASIS_ASSET_TASK_LOADING ||
        task->state == STASIS_ASSET_TASK_PUBLISHING) {
        task->release_requested = 1;
        task->state = STASIS_ASSET_TASK_CANCELLED;
        SDL_UnlockMutex(g_asset_task_mutex);
        return;
    }
    kind = task->kind;
    handle = task->handle;
    task->handle = 0;
    stasis_asset_task_clear(task);
    SDL_UnlockMutex(g_asset_task_mutex);
    if (handle > 0 && kind == STASIS_ASSET_KIND_SPRITE) stasis_gfx_release_sprite(handle);
    if (handle > 0 && kind == STASIS_ASSET_KIND_AUDIO) stasis_audio_release(handle);
}

static SpriteEntry* sprite_fallback_get(void) {
    if (g_sprite_fallback.used) return &g_sprite_fallback;
    if (!g_window) return NULL;

    SpriteEntry next;
    memset(&next, 0, sizeof(next));
    next.page_index = -1;
    next.w = 2;
    next.h = 2;
    next.max_w = 2;
    next.max_h = 2;

    if (!g_renderer) return NULL;
    int page_index = -1;
    for (int i = 0; i < g_sprite_atlas_page_count; i++) {
        if (!g_sprite_atlas_pages[i].dedicated) { page_index = i; break; }
    }
    if (page_index < 0) page_index = stasis_sprite_atlas_create_page(
        STASIS_SDL_ATLAS_PAGE_SIZE, STASIS_SDL_ATLAS_PAGE_SIZE, 0, 0);
    if (page_index < 0) return NULL;
    StasisSdlAtlasPage* page = &g_sprite_atlas_pages[page_index];
    next.page_index = page_index;
    next.atlas_x = page->placeholder_x;
    next.atlas_y = page->placeholder_y;
    next.u0 = (float)page->placeholder_x / (float)page->width;
    next.v0 = (float)page->placeholder_y / (float)page->height;
    next.u1 = (float)(page->placeholder_x + 2) / (float)page->width;
    next.v1 = (float)(page->placeholder_y + 2) / (float)page->height;
    next.sdl_tex = page->texture;
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

    e->sdl_tex = NULL; /* Atlas pages are renderer-owned. */
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
 * Geometry remains logical f32 through submission and rasterizes here.
 * rot_degrees: rotation in degrees (0-359), around the sprite center
 * a: alpha 0-255
 */
static void stasis_gfx_draw_sprite_internal(int handle, float x, float y, float w, float h,
                                           float rot_degrees, uint32_t tint_rgba,
                                           float src_x, float src_y, float src_w, float src_h,
                                           float pivot_x, float pivot_y,
                                           float scale_x, float scale_y, int do_hash) {
    if (do_hash) {
        gfx_debug_hash_i32(handle);
        gfx_debug_hash_f32(x);
        gfx_debug_hash_f32(y);
        gfx_debug_hash_f32(w);
        gfx_debug_hash_f32(h);
        gfx_debug_hash_f32(rot_degrees);
        gfx_debug_hash_i32((int32_t)tint_rgba);
    }
    SpriteEntry* e = sprite_get(handle);
    if (!e) e = sprite_fallback_get();
    if (!e) return;

    if (w <= 0 || h <= 0 || scale_x == 0.0f || scale_y == 0.0f) return;

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
            "sdl",
            g_resource_lifecycle.surface_generation, e->surface_generation,
            g_resource_lifecycle.renderer_generation, e->renderer_generation);
        return;
    }

    if (src_w == 0.0f && src_h == 0.0f) {
        src_x = 0.0f; src_y = 0.0f; src_w = (float)e->w; src_h = (float)e->h;
    }
    if (src_x < 0.0f || src_y < 0.0f || src_w <= 0.0f || src_h <= 0.0f ||
        src_x + src_w > (float)e->w || src_y + src_h > (float)e->h) return;
    const float src_u0 = src_x / (float)e->w;
    const float src_v0 = src_y / (float)e->h;
    const float src_u1 = (src_x + src_w) / (float)e->w;
    const float src_v1 = (src_y + src_h) / (float)e->h;
    const Uint8 tint_r = (Uint8)((tint_rgba >> 24) & 0xffu);
    const Uint8 tint_g = (Uint8)((tint_rgba >> 16) & 0xffu);
    const Uint8 tint_b = (Uint8)((tint_rgba >> 8) & 0xffu);
    const Uint8 tint_a = (Uint8)(tint_rgba & 0xffu);
    if (!g_renderer || !e->sdl_tex) return;
    SDL_FRect dst;
    dst.w = fabsf(w * scale_x);
    dst.h = fabsf(h * scale_y);
    dst.x = x + pivot_x - fabsf(pivot_x * scale_x);
    dst.y = y + pivot_y - fabsf(pivot_y * scale_y);
    SDL_FPoint center = { fabsf(pivot_x * scale_x), fabsf(pivot_y * scale_y) };
    SDL_SetTextureColorMod(e->sdl_tex, tint_r, tint_g, tint_b);
    SDL_SetTextureAlphaMod(e->sdl_tex, tint_a);
    SDL_FRect src = { (float)e->atlas_x + src_u0 * e->w,
        (float)e->atlas_y + src_v0 * e->h,
        (src_u1 - src_u0) * e->w, (src_v1 - src_v0) * e->h };
    SDL_FlipMode flip = SDL_FLIP_NONE;
    if (scale_x < 0.0f) flip = (SDL_FlipMode)(flip | SDL_FLIP_HORIZONTAL);
    if (scale_y < 0.0f) flip = (SDL_FlipMode)(flip | SDL_FLIP_VERTICAL);
    SDL_RenderTextureRotated(
        g_renderer, e->sdl_tex, &src, &dst, (double)rot_degrees, &center, flip);
}

STASIS_EXPORT void stasis_gfx_draw_sprite(int handle, float x, float y, float w, float h,
                                          int rot_degrees, int a) {
    if (a < 0) a = 0;
    if (a > 255) a = 255;
    stasis_gfx_draw_sprite_internal(handle, x, y, w, h, (float)rot_degrees,
        0xffffff00u | (uint32_t)a, 0.0f, 0.0f, 0.0f, 0.0f,
        w * 0.5f, h * 0.5f, 1.0f, 1.0f, 1);
}

/*
 * Deprecated host compatibility entry point. Guest code uses the canonical
 * frame writer and never stages/copies these arrays.
 */
STASIS_EXPORT void stasis_gfx_draw_sprites(const int32_t* cmd_i32, const float* cmd_f32, int sprite_count) {
    if (!cmd_i32 || !cmd_f32 || sprite_count <= 0) return;
    for (int i = 0; i < sprite_count; i++) {
        const int base_i = i * STASIS_RENDER_SPRITE_I32_STRIDE;
        const int base_f = i * STASIS_RENDER_SPRITE_F32_STRIDE;
        stasis_gfx_draw_sprite_internal(
            cmd_i32[base_i + 0],
            cmd_f32[base_f + 0],
            cmd_f32[base_f + 1],
            cmd_f32[base_f + 2],
            cmd_f32[base_f + 3],
            cmd_f32[base_f + 12],
            (uint32_t)cmd_i32[base_i + 1],
            cmd_f32[base_f + 4], cmd_f32[base_f + 5],
            cmd_f32[base_f + 6], cmd_f32[base_f + 7],
            cmd_f32[base_f + 8], cmd_f32[base_f + 9],
            cmd_f32[base_f + 10], cmd_f32[base_f + 11],
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
    if (scancode < 0 || scancode >= SDL_SCANCODE_COUNT) return 0;
    return g_keyboard_state[scancode] ? 1 : 0;
}

/*
 * Get current time in milliseconds
 */
static uint64_t stasis_recording_clock_us(void) {
    if (!g_recording_presentation || g_recording_fps == 0) return 0;
    return ((uint64_t)(uint32_t)g_debug_frame_counter * 1000000ull) /
        (uint64_t)g_recording_fps;
}

STASIS_EXPORT int stasis_get_time_ms(void) {
    if (g_recording_presentation) {
        uint64_t millis = stasis_recording_clock_us() / 1000ull;
        return millis > (uint64_t)INT_MAX ? INT_MAX : (int)millis;
    }
    return (int)SDL_GetTicks();
}

/*
 * Get current time in microseconds (truncated to i32).
 */
STASIS_EXPORT int stasis_get_time_us(void) {
    if (g_recording_presentation) {
        uint64_t micros = stasis_recording_clock_us();
        return micros > (uint64_t)INT_MAX ? INT_MAX : (int)micros;
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
    if (ms > 0) SDL_Delay((Uint32)ms);
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

static int stasis_storage_path(
    const char* scope,
    const char* key,
    const char* extension,
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
    written = snprintf(path, capacity, "%s%s.%s", root, key, extension);
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
    if (!stasis_storage_path(scope, key, "i32", path, sizeof(path), &root)) return fallback;
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
    if (!stasis_storage_path(scope, key, "i32", path, sizeof(path), &root)) return 0;
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

STASIS_EXPORT int stasis_storage_load_ascii(const char* scope, const char* key, char* out, int capacity) {
    char path[1024];
    char* root = NULL;
    FILE* file;
    int count;
    int trailing;
    int index;
    if (!out || capacity <= 0 ||
        !stasis_storage_path(scope, key, "ascii", path, sizeof(path), &root)) return -1;
    file = fopen(path, "rb");
    SDL_free(root);
    if (!file) return -1;
    count = (int)fread(out, 1, (size_t)capacity, file);
    trailing = fgetc(file);
    fclose(file);
    if (trailing != EOF) return -1;
    for (index = 0; index < count; index += 1) {
        unsigned char ch = (unsigned char)out[index];
        if (ch < 32 || ch > 126) return -1;
    }
    return count;
}

STASIS_EXPORT int stasis_storage_save_ascii(
    const char* scope,
    const char* key,
    const char* value,
    int length
) {
    char path[1024];
    char temp_path[1032];
    char* root = NULL;
    FILE* file;
    int path_written;
    int index;
    int ok = 1;
    if (!value || length < 0 ||
        !stasis_storage_path(scope, key, "ascii", path, sizeof(path), &root)) return 0;
    SDL_free(root);
    for (index = 0; index < length; index += 1) {
        unsigned char ch = (unsigned char)value[index];
        if (ch < 32 || ch > 126) return 0;
    }
    path_written = snprintf(temp_path, sizeof(temp_path), "%s.tmp", path);
    if (path_written < 0 || (size_t)path_written >= sizeof(temp_path)) return 0;
    file = fopen(temp_path, "wb");
    if (!file) return 0;
    if (fwrite(value, 1, (size_t)length, file) != (size_t)length) ok = 0;
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

STASIS_EXPORT int stasis_clipboard_load_ascii(char* out, int capacity) {
    char* text;
    size_t count;
    size_t index;
    if (!out || capacity <= 0) return -1;
    text = SDL_GetClipboardText();
    if (!text) return -1;
    count = strlen(text);
    if (count > (size_t)capacity) {
        SDL_free(text);
        return -1;
    }
    for (index = 0; index < count; index += 1) {
        unsigned char ch = (unsigned char)text[index];
        if (ch < 32 || ch > 126) {
            SDL_free(text);
            return -1;
        }
    }
    memcpy(out, text, count);
    SDL_free(text);
    return (int)count;
}

STASIS_EXPORT int stasis_clipboard_save_ascii(const char* value, int length) {
    char* text;
    int index;
    int result;
    if (!value || length < 0) return 0;
    for (index = 0; index < length; index += 1) {
        unsigned char ch = (unsigned char)value[index];
        if (ch < 32 || ch > 126) return 0;
    }
    text = (char*)malloc((size_t)length + 1);
    if (!text) return 0;
    memcpy(text, value, (size_t)length);
    text[length] = '\0';
    result = SDL_SetClipboardText(text) ? 1 : 0;
    free(text);
    return result;
}

/*
 * Audio - init/shutdown and ring-buffer push API
 */
STASIS_EXPORT int stasis_audio_load_wav(const char* path) {
    char resolved[1024];
    if (!path || !*path || !resolve_asset_path(path, resolved, sizeof(resolved))) {
        stasis_report_runtime_errorf("Audio path could not be resolved: %s", path ? path : "");
        return 0;
    }
    if (!stasis_audio_ensure_init()) goto fail;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    int handle = stasis_audio_assets_load_wav(&g_audio_assets, resolved);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
    if (handle == 0) {
        stasis_report_runtime_errorf("Audio failed to load: %s", path);
    }
    return handle;

fail:
    return 0;
}

static int stasis_audio_load_asset(const char* path) {
    char resolved[1024];
    if (!path || !*path || !resolve_asset_path(path, resolved, sizeof(resolved))) {
        stasis_report_runtime_errorf("Audio path could not be resolved: %s", path ? path : "");
        return 0;
    }
    if (!stasis_audio_ensure_init()) return 0;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    int handle = stasis_audio_assets_load(&g_audio_assets, resolved);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
    if (handle == 0) {
        stasis_report_runtime_errorf("Audio failed to load: %s", path);
    }
    return handle;
}

STASIS_EXPORT void stasis_audio_release(int asset_handle) {
    if (!g_audio_stream && !g_recording_audio_enabled) return;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    stasis_audio_assets_release(&g_audio_assets, asset_handle);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
}

STASIS_EXPORT int stasis_audio_play(int asset_handle, int loop, float volume, float pan) {
    if (!stasis_audio_ensure_init()) return 0;
    if (volume < 0.0f) volume = 0.0f;
    if (volume > 1.0f) volume = 1.0f;
    if (pan < -1.0f) pan = -1.0f;
    if (pan > 1.0f) pan = 1.0f;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    int voice_handle = stasis_audio_assets_play(&g_audio_assets, asset_handle, loop, volume, pan);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
    return voice_handle;
}

STASIS_EXPORT void stasis_audio_stop(int voice_handle) {
    if (!g_audio_stream && !g_recording_audio_enabled) return;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    stasis_audio_assets_stop_voice(&g_audio_assets, voice_handle);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
}

STASIS_EXPORT int stasis_audio_voice_is_playing(int voice_handle) {
    if (!g_audio_stream && !g_recording_audio_enabled) return 0;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    int playing = stasis_audio_assets_voice_is_playing(&g_audio_assets, voice_handle);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
    return playing;
}

STASIS_EXPORT void stasis_audio_voice_set_paused(int voice_handle, int paused) {
    if (!g_audio_stream && !g_recording_audio_enabled) return;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    stasis_audio_assets_voice_set_paused(&g_audio_assets, voice_handle, paused);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
}

STASIS_EXPORT void stasis_audio_voice_set_volume_pan(int voice_handle, float volume, float pan) {
    if (!g_audio_stream && !g_recording_audio_enabled) return;
    if (volume < 0.0f) volume = 0.0f;
    if (volume > 1.0f) volume = 1.0f;
    if (pan < -1.0f) pan = -1.0f;
    if (pan > 1.0f) pan = 1.0f;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    stasis_audio_assets_voice_set_volume_pan(&g_audio_assets, voice_handle, volume, pan);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
}

/* Brickout-compatible convenience API. Both categories use the same bounded WAV/MP3 asset
 * table; music is exclusive per asset while effects may overlap. */
STASIS_EXPORT int stasis_audio_load_music(const char* path) {
    return stasis_audio_load_asset(path);
}

STASIS_EXPORT int stasis_audio_load_effect(const char* path) {
    return stasis_audio_load_asset(path);
}

STASIS_EXPORT int stasis_audio_play_music(int asset_handle, int loop, float volume) {
    if (!g_audio_stream && !g_recording_audio_enabled) {
        return stasis_audio_play(asset_handle, loop, volume, 0.0f) > 0;
    }
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    stasis_audio_assets_stop_asset(&g_audio_assets, asset_handle);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
    return stasis_audio_play(asset_handle, loop, volume, 0.0f) > 0;
}

STASIS_EXPORT void stasis_audio_stop_music(int asset_handle) {
    if (!g_audio_stream && !g_recording_audio_enabled) return;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    stasis_audio_assets_stop_asset(&g_audio_assets, asset_handle);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
}

STASIS_EXPORT void stasis_audio_pause_music(int asset_handle, int paused) {
    if (!g_audio_stream && !g_recording_audio_enabled) return;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    stasis_audio_assets_set_asset_paused(&g_audio_assets, asset_handle, paused);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
}

STASIS_EXPORT void stasis_audio_set_music_volume(int asset_handle, float volume) {
    if (!g_audio_stream && !g_recording_audio_enabled) return;
    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
    stasis_audio_assets_set_asset_volume(&g_audio_assets, asset_handle, volume);
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);
}

STASIS_EXPORT int stasis_audio_play_effect(int asset_handle, float volume) {
    return stasis_audio_play(asset_handle, 0, volume, 0.0f) > 0;
}

STASIS_EXPORT int stasis_audio_init(int sample_rate, int channels, int target_latency_frames) {
    if (channels != 0 && channels != 2) return 0;
    if (target_latency_frames > STASIS_AUDIO_MAX_TARGET_LATENCY_FRAMES) return 0;
    if (g_recording_audio_enabled && sample_rate > 0 && sample_rate != 48000) return 0;
    if (sample_rate > 0) g_audio_sample_rate = sample_rate;
    g_audio_channels = 2;
    if (target_latency_frames > 0) g_audio_target_latency_frames = target_latency_frames;

    if (g_recording_audio_enabled) {
        return stasis_audio_ensure_ring_init();
    }
    if (g_audio_stream) {
        stasis_audio_shutdown_internal();
    }

    return stasis_audio_ensure_init();
}

STASIS_EXPORT void stasis_audio_shutdown(void) {
    stasis_audio_shutdown_internal();
}

STASIS_EXPORT int stasis_audio_is_available(void) {
    if (g_recording_audio_enabled) return stasis_audio_ensure_ring_init();
    return stasis_audio_ensure_init();
}

STASIS_EXPORT int stasis_audio_get_sample_rate(void) {
    if (g_recording_audio_enabled) return g_audio_sample_rate;
    if (!stasis_audio_ensure_init()) return 0;
    return g_audio_sample_rate;
}

STASIS_EXPORT int stasis_audio_get_channels(void) {
    if (g_recording_audio_enabled) return g_audio_channels;
    if (!stasis_audio_ensure_init()) return 0;
    return g_audio_channels;
}

STASIS_EXPORT int stasis_audio_get_queued_frames(void) {
    if (g_recording_audio_enabled) {
        if (!stasis_audio_ensure_ring_init() || g_audio_channels <= 0) return 0;
        return g_audio_queued_samples / g_audio_channels;
    }
    if (!stasis_audio_ensure_init()) return 0;

    int queued = 0;
    SDL_LockAudioStream(g_audio_stream);
    if (g_audio_channels > 0) {
        queued = g_audio_queued_samples / g_audio_channels;
    }
    SDL_UnlockAudioStream(g_audio_stream);
    return queued;
}

STASIS_EXPORT int stasis_audio_get_underruns(void) {
    if (g_recording_audio_enabled) return 0;
    if (!stasis_audio_ensure_init()) return 0;

    int underruns = 0;
    SDL_LockAudioStream(g_audio_stream);
    underruns = g_audio_underruns;
    SDL_UnlockAudioStream(g_audio_stream);
    return underruns;
}

STASIS_EXPORT int stasis_audio_push_f32_interleaved(const float* interleaved_lr, int frame_count) {
    if (!interleaved_lr || frame_count <= 0) return 0;
    if (g_recording_audio_enabled) {
        if (!stasis_audio_ensure_ring_init()) return 0;
    } else if (!stasis_audio_ensure_init()) {
        return 0;
    }

    int accepted_samples = 0;
    const int requested_samples = frame_count * g_audio_channels;

    if (g_audio_stream) SDL_LockAudioStream(g_audio_stream);
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
    if (g_audio_stream) SDL_UnlockAudioStream(g_audio_stream);

    if (g_audio_channels <= 0) return 0;
    return accepted_samples / g_audio_channels;
}

STASIS_EXPORT int stasis_set_recording_audio_config(int enabled) {
    if (g_audio_stream || g_audio_initialized) return 0;
    if (!enabled && g_recording_audio_enabled) {
        stasis_asset_tasks_shutdown();
        stasis_audio_shutdown_internal();
    }
    g_recording_audio_enabled = enabled != 0;
    if (g_recording_audio_enabled) {
        g_audio_sample_rate = 48000;
        g_audio_channels = 2;
    }
    return 1;
}

STASIS_EXPORT int stasis_recording_audio_pull_f32_interleaved(
    float* output_stereo,
    int frame_count
) {
    if (!g_recording_audio_enabled || !output_stereo || frame_count <= 0) return 0;
    return stasis_audio_mix_output(output_stereo, frame_count);
}

/*
 * Configure fullscreen post-processing parameters.
 * strength: 0-1, phase/time: seconds, speed: oscillation scalar, color: rgb tint (0-1).
 */
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
    if (g_audio_stream) {
        if (paused) {
            SDL_PauseAudioStreamDevice(g_audio_stream);
        } else {
            SDL_ResumeAudioStreamDevice(g_audio_stream);
        }
    }
    if (paused) {
        stasis_renderer_lifecycle_pause(&g_resource_lifecycle);
        g_resource_frame_ready = false;
    } else if (g_resource_lifecycle.state == STASIS_RENDERER_PAUSED) {
        stasis_renderer_lifecycle_resume(&g_resource_lifecycle);
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
 * Check if window was resized since last call.
 * Returns 1 if resized, 0 otherwise. Clears the flag after reading.
 */
STASIS_EXPORT int stasis_gfx_window_resized(void) {
    int result = g_window_resized ? 1 : 0;
    g_window_resized = false;
    return result;
}

static void stasis_asset_tasks_shutdown(void) {
    if (!g_asset_task_mutex) return;
    SDL_LockMutex(g_asset_task_mutex);
    g_asset_task_stop = 1;
    if (g_asset_task_condition) SDL_BroadcastCondition(g_asset_task_condition);
    SDL_UnlockMutex(g_asset_task_mutex);
    if (g_asset_task_thread) SDL_WaitThread(g_asset_task_thread, NULL);
    g_asset_task_thread = NULL;

    for (int i = 0; i < STASIS_ASSET_TASK_CAPACITY; i++) {
        StasisAssetTask* task = &g_asset_tasks[i];
        int kind = task->kind;
        int handle = task->handle;
        task->handle = 0;
        stasis_asset_task_clear(task);
        if (handle > 0 && kind == STASIS_ASSET_KIND_SPRITE) stasis_gfx_release_sprite(handle);
        if (handle > 0 && kind == STASIS_ASSET_KIND_AUDIO) stasis_audio_release(handle);
    }
    if (g_asset_task_condition) SDL_DestroyCondition(g_asset_task_condition);
    SDL_DestroyMutex(g_asset_task_mutex);
    g_asset_task_condition = NULL;
    g_asset_task_mutex = NULL;
    g_asset_task_stop = 0;
    g_asset_task_next_id = 1;
}

/*
 * Cleanup and shutdown
 */
STASIS_EXPORT void stasis_shutdown(void) {
    stasis_asset_tasks_shutdown();
    stasis_audio_shutdown_internal();
    gfx_asset_watch_shutdown();

    for (int i = 0; i < g_sprite_capacity; i++) {
        if (g_sprites[i].used) {
            g_sprites[i].sdl_tex = NULL;
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
    stasis_sprite_atlas_reset(1);
    memset(&g_sprite_fallback, 0, sizeof(g_sprite_fallback));
    g_sprite_fallback.page_index = -1;

    for (int i = 0; i < MAX_FONTS; i++) {
        stasis_release_font(&g_fonts[i]);
    }
    stasis_reset_text_cache();
    if (g_renderer) {
        SDL_DestroyRenderer(g_renderer);
        g_renderer = NULL;
    }
    if (g_window) {
        SDL_DestroyWindow(g_window);
        g_window = NULL;
    }
    SDL_Quit();
    memset(&g_resource_lifecycle, 0, sizeof(g_resource_lifecycle));
    memset(&g_test_display_override, 0, sizeof(g_test_display_override));
    g_available_width = 0;
    g_available_height = 0;
    g_x11_scale_controlled_window = false;
    g_window_minimized = false;
    g_render_accepted_frames = 0;
    g_render_rejected_frames = 0;
    g_render_presented_frames = 0;
    g_render_last_trace = 0;
    g_render_last_validation = STASIS_RENDER_VALID;
    g_render_last_display_generation = 0;
    g_render_last_density_generation = 0;
    g_render_logged_validation_mask = 0;
    g_render_contract_logged = false;
    g_render_trace_enabled = -1;
    SDL_SetAtomicInt(&g_performance_metrics_requested, 0);
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
#define STASIS_TEXT_GEOMETRY_BATCH_QUADS 256

static StasisTextRun g_text_runs[STASIS_MAX_TEXT_RUNS];
static unsigned char g_text_run_bytes[STASIS_TEXT_RUN_MAX_BYTES];
static int g_text_run_bytes_used = 0;
static StasisTextQuad g_text_run_quads[STASIS_TEXT_RUN_MAX_QUADS];
static int g_text_run_quads_used = 0;
static SDL_Vertex g_text_geometry_vertices[STASIS_TEXT_GEOMETRY_BATCH_QUADS * 4];
static int g_text_geometry_indices[STASIS_TEXT_GEOMETRY_BATCH_QUADS * 6];
static bool g_text_geometry_indices_ready = false;

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

static int stasis_font_atlas_pixels(int atlas_size, size_t* pixels_out) {
    if (atlas_size <= 0 || !pixels_out) return 0;
    const size_t extent = (size_t)atlas_size;
    if (extent > SIZE_MAX / extent) return 0;
    *pixels_out = extent * extent;
    return 1;
}

static int stasis_build_font_atlas(StasisFont* font) {
    if (!font || !font->ttf_buffer || font->font_size <= 0) return 0;

    const int replaces_existing = font->raster_size > 0 && font->atlas_size > 0;

    const float pixel_scale = g_pixel_scale < 1.0f ? 1.0f : g_pixel_scale;
    const int raster_size = stasis_current_scaled_extent(font->font_size);
    int atlas_size = stasis_display_font_atlas_extent(pixel_scale);
    size_t atlas_pixels = 0;
    unsigned char* atlas_bitmap = NULL;
    stbtt_bakedchar baked_chars[FONT_NUM_CHARS];
    int result = 0;
    for (;;) {
        if (!stasis_font_atlas_pixels(atlas_size, &atlas_pixels)) return 0;
        atlas_bitmap = (unsigned char*)malloc(atlas_pixels);
        if (!atlas_bitmap) return 0;
        result = stbtt_BakeFontBitmap(font->ttf_buffer, 0, (float)raster_size,
            atlas_bitmap, atlas_size, atlas_size, FONT_FIRST_CHAR, FONT_NUM_CHARS, baked_chars);
        if (result > 0) break;

        free(atlas_bitmap);
        atlas_bitmap = NULL;
        const int next_atlas_size = stasis_display_font_atlas_next_extent(atlas_size);
        if (next_atlas_size == 0) {
            SDL_Log("stasis_load_font: BakeFontBitmap failed size=%d atlas=%d", raster_size, atlas_size);
            return 0;
        }
        atlas_size = next_atlas_size;
    }

    if (true) {
        if (!g_renderer) {
            free(atlas_bitmap);
            return 0;
        }
        if (atlas_pixels > SIZE_MAX / 4u) {
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
        if (!SDL_UpdateTexture(texture, NULL, rgba, atlas_size * 4)) {
            SDL_DestroyTexture(texture);
            free(rgba);
            free(atlas_bitmap);
            return 0;
        }
        free(rgba);
        if (font->sdl_texture) SDL_DestroyTexture(font->sdl_texture);
        font->sdl_texture = texture;
    } else {
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
    stasis_log_font_preparation(font, replaces_existing);
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
            (int)ch - FONT_FIRST_CHAR, &pos_x, &pos_y, &quad, 0);
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

    int restored = g_renderer != NULL;
    for (int i = 0; i < g_sprite_capacity; i++) {
        SpriteEntry* entry = &g_sprites[i];
        if (!entry->used || !entry->path) continue;
        if (!sprite_build_into_entry_sized(entry, entry->path, entry->max_w, entry->max_h)) {
            stasis_report_runtime_errorf("Renderer restore failed for sprite: %s", entry->path);
            SDL_Log("Stasis renderer restore failed: stage=sprite handle=%d path=%s logical=%dx%d raster=%dx%d backend=%s surface_generation=%u renderer_generation=%u reason=%s failure=texture_rebuild_failed",
                sprite_handle_for_slot(i), entry->path, entry->max_w, entry->max_h,
                entry->w, entry->h, "sdl",
                g_resource_lifecycle.surface_generation,
                g_resource_lifecycle.renderer_generation,
                stasis_renderer_reason_name(g_resource_lifecycle.reason));
            restored = 0;
        }
    }
    if (!sprite_fallback_get()) {
        stasis_host_report_runtime_error("Renderer restore failed for fallback texture");
        SDL_Log("Stasis renderer restore failed: stage=fallback handle=0 path=<procedural> logical=2x2 raster=2x2 backend=%s surface_generation=%u renderer_generation=%u reason=%s failure=texture_rebuild_failed",
            "sdl",
            g_resource_lifecycle.surface_generation,
            g_resource_lifecycle.renderer_generation,
            stasis_renderer_reason_name(g_resource_lifecycle.reason));
        restored = 0;
    }
    for (int i = 0; i < MAX_FONTS; i++) {
        StasisFont* font = &g_fonts[i];
        if (!font->active) continue;
        if (!stasis_build_font_atlas(font)) {
            stasis_host_report_runtime_error("Renderer restore failed for a font atlas");
            SDL_Log("Stasis renderer restore failed: stage=font handle=%d path=<retained-font-bytes> logical=%dx%d raster=%dx%d backend=%s surface_generation=%u renderer_generation=%u reason=%s failure=atlas_rebuild_failed",
                i + 1, font->font_size, font->font_size, font->raster_size,
                font->raster_size, "sdl",
                g_resource_lifecycle.surface_generation,
                g_resource_lifecycle.renderer_generation,
                stasis_renderer_reason_name(g_resource_lifecycle.reason));
            restored = 0;
        }
    }
    if (restored && !stasis_rebuild_text_runs()) {
        stasis_host_report_runtime_error("Renderer restore failed for cached text");
        SDL_Log("Stasis renderer restore failed: stage=cached_text handle=0 path=<retained-text-runs> logical=0x0 raster=0x0 backend=%s surface_generation=%u renderer_generation=%u reason=%s failure=quad_rebuild_failed",
            "sdl",
            g_resource_lifecycle.surface_generation,
            g_resource_lifecycle.renderer_generation,
            stasis_renderer_reason_name(g_resource_lifecycle.reason));
        restored = 0;
    }

    stasis_renderer_lifecycle_finish_restore(&g_resource_lifecycle, restored);
    if (restored) {
        SDL_Log("Stasis renderer resources restored: backend=%s surface_generation=%u renderer_generation=%u reason=%s sprites=%d",
            "sdl",
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

static void stasis_prepare_text_geometry_indices(void) {
    if (g_text_geometry_indices_ready) return;
    for (int i = 0; i < STASIS_TEXT_GEOMETRY_BATCH_QUADS; i++) {
        const int vertex = i * 4;
        const int index = i * 6;
        g_text_geometry_indices[index + 0] = vertex + 0;
        g_text_geometry_indices[index + 1] = vertex + 1;
        g_text_geometry_indices[index + 2] = vertex + 2;
        g_text_geometry_indices[index + 3] = vertex + 0;
        g_text_geometry_indices[index + 4] = vertex + 2;
        g_text_geometry_indices[index + 5] = vertex + 3;
    }
    g_text_geometry_indices_ready = true;
}

static void stasis_draw_cached_text_sdl(
    const StasisTextRun* run,
    const StasisFont* font,
    float x,
    float y,
    Uint8 color_r,
    Uint8 color_g,
    Uint8 color_b,
    Uint8 color_a
) {
    /* SDL_RenderGeometry ignores texture modulation, so preserve the old
       8-bit text tint through per-vertex color. */
    const SDL_FColor color = {
        (float)color_r / 255.0f,
        (float)color_g / 255.0f,
        (float)color_b / 255.0f,
        (float)color_a / 255.0f};
    stasis_prepare_text_geometry_indices();
    for (int batch_start = 0; batch_start < run->quad_count;
         batch_start += STASIS_TEXT_GEOMETRY_BATCH_QUADS) {
        int batch_count = run->quad_count - batch_start;
        if (batch_count > STASIS_TEXT_GEOMETRY_BATCH_QUADS) {
            batch_count = STASIS_TEXT_GEOMETRY_BATCH_QUADS;
        }
        for (int i = 0; i < batch_count; i++) {
            const StasisTextQuad* q =
                &g_text_run_quads[run->quad_off + batch_start + i];
            const int src_x = (int)(q->s0 * (float)font->atlas_size);
            const int src_y = (int)(q->t0 * (float)font->atlas_size);
            const int src_w = (int)((q->s1 - q->s0) * (float)font->atlas_size);
            const int src_h = (int)((q->t1 - q->t0) * (float)font->atlas_size);
            SDL_Vertex* vertices = &g_text_geometry_vertices[i * 4];
            const float s0 = (float)src_x / (float)font->atlas_size;
            const float t0 = (float)src_y / (float)font->atlas_size;
            const float s1 = (float)(src_x + src_w) / (float)font->atlas_size;
            const float t1 = (float)(src_y + src_h) / (float)font->atlas_size;

            vertices[0].position = (SDL_FPoint){x + q->x0, y + q->y0};
            vertices[1].position = (SDL_FPoint){x + q->x1, y + q->y0};
            vertices[2].position = (SDL_FPoint){x + q->x1, y + q->y1};
            vertices[3].position = (SDL_FPoint){x + q->x0, y + q->y1};
            vertices[0].tex_coord = (SDL_FPoint){s0, t0};
            vertices[1].tex_coord = (SDL_FPoint){s1, t0};
            vertices[2].tex_coord = (SDL_FPoint){s1, t1};
            vertices[3].tex_coord = (SDL_FPoint){s0, t1};
            for (int vertex = 0; vertex < 4; vertex++) {
                vertices[vertex].color = color;
            }
        }
        SDL_RenderGeometry(
            g_renderer,
            font->sdl_texture,
            g_text_geometry_vertices,
            batch_count * 4,
            g_text_geometry_indices,
            batch_count * 6);
    }
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

    if (true) {
        if (!font->sdl_texture || !g_renderer) return;

        SDL_SetTextureBlendMode(font->sdl_texture, SDL_BLENDMODE_BLEND);
        const Uint8 color_r =
            (Uint8)(r < 0.0f ? 0 : (r > 1.0f ? 255 : (int)(r * 255.0f)));
        const Uint8 color_g =
            (Uint8)(g < 0.0f ? 0 : (g > 1.0f ? 255 : (int)(g * 255.0f)));
        const Uint8 color_b =
            (Uint8)(b < 0.0f ? 0 : (b > 1.0f ? 255 : (int)(b * 255.0f)));
        const Uint8 color_a =
            (Uint8)(a < 0.0f ? 0 : (a > 1.0f ? 255 : (int)(a * 255.0f)));
        stasis_draw_cached_text_sdl(
            run, font, x, y, color_r, color_g, color_b, color_a);
        return;
    }

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

STASIS_EXPORT float stasis_gfx_measure_text_cached_height(int run_handle) {
    if (run_handle <= 0 || run_handle > STASIS_MAX_TEXT_RUNS) return 0.0f;
    StasisTextRun* run = &g_text_runs[run_handle - 1];
    if (!run->active) return 0.0f;
    if (!stasis_ensure_font_ready(run->font_handle)) return 0.0f;
    return run->height;
}

/* Load a TrueType font from disk */
STASIS_EXPORT int stasis_load_font(const char* path, int font_size) {
    if (!path || font_size <= 0) return 0;
    if (!g_window) return 0;

    char resolved[1024];
    if (!resolve_asset_path(path, resolved, sizeof(resolved))) {
        stasis_report_runtime_errorf("Font path could not be resolved: %s", path);
        SDL_Log("stasis_load_font: could not resolve %s", path);
        return 0;
    }

    /* Read first so reuse is based on the bytes that build the retained atlas,
       not filesystem timestamp granularity. */
    FILE* f = fopen(resolved, "rb");
    if (!f) {
        stasis_report_runtime_errorf("Font failed to open: %s", path);
        SDL_Log("stasis_load_font: failed to open %s", resolved);
        return 0;
    }

    fseek(f, 0, SEEK_END);
    long file_size = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (file_size <= 0) {
        fclose(f);
        stasis_report_runtime_errorf("Font data is empty: %s", path);
        SDL_Log("stasis_load_font: empty font %s", resolved);
        return 0;
    }
    size_t size = (size_t)file_size;
    unsigned char* ttf_buffer = (unsigned char*)malloc(size);
    if (!ttf_buffer) {
        fclose(f);
        SDL_Log("stasis_load_font: malloc failed");
        return 0;
    }
    if (fread(ttf_buffer, 1, size, f) != size) {
        fclose(f);
        free(ttf_buffer);
        stasis_report_runtime_errorf("Font data could not be read: %s", path);
        SDL_Log("stasis_load_font: short read for %s", resolved);
        return 0;
    }
    fclose(f);

    /* Reuse an identical load. If the file changed in place, retain the stable
       handle while replacing its backing resources. */
    int slot = -1;
    for (int i = 0; i < MAX_FONTS; i++) {
        if (g_fonts[i].active && g_fonts[i].font_size == font_size &&
            strcmp(g_fonts[i].source_path, resolved) == 0) {
            if (g_fonts[i].source_size == (uint64_t)size && g_fonts[i].ttf_buffer &&
                memcmp(g_fonts[i].ttf_buffer, ttf_buffer, size) == 0) {
                free(ttf_buffer);
                return i + 1;
            }
            stasis_release_font(&g_fonts[i]);
            stasis_reset_text_cache();
            slot = i;
            break;
        }
    }

    /* Find free slot */
    if (slot == -1) {
        for (int i = 0; i < MAX_FONTS; i++) {
            if (!g_fonts[i].active) {
                slot = i;
                break;
            }
        }
    }

    if (slot == -1) {
        free(ttf_buffer);
        SDL_Log("stasis_load_font: no free font slots");
        return 0;
    }

    /* Initialize font */
    StasisFont* font = &g_fonts[slot];
    memset(font, 0, sizeof(*font));
    if (!stbtt_InitFont(&font->font_info, ttf_buffer, 0)) {
        free(ttf_buffer);
        stasis_report_runtime_errorf("Font data is invalid: %s", path);
        SDL_Log("stasis_load_font: stbtt_InitFont failed for %s", resolved);
        return 0;
    }

    font->ttf_buffer = ttf_buffer;
    font->font_size = font_size;
    snprintf(font->source_path, sizeof(font->source_path), "%s", resolved);
    font->source_size = (uint64_t)size;
    stbtt_GetFontVMetrics(&font->font_info, &font->ascent, &font->descent, &font->line_gap);
    font->active = true;
    if (!stasis_build_font_atlas(font)) {
        stasis_report_runtime_errorf("Font atlas creation failed: %s", path);
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

    if (true) {
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
                    SDL_FRect source = {
                        (float)src.x, (float)src.y, (float)src.w, (float)src.h};
                    SDL_RenderTexture(g_renderer, font->sdl_texture, &source, &dst);
                }
            }

            text++;
        }
        return;
    }

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
                              c - FONT_FIRST_CHAR, &pos_x, &pos_y, &quad, 0);
        }
        text++;
    }

    return pos_x / font->pixel_scale;
}
