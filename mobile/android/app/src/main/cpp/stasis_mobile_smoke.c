#include <jni.h>
#include <android/log.h>
#include <dirent.h>
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include "stasis_render_contract.h"
#include "stasis_mobile_aot_runtime.h"
#if STASIS_ANDROID_PUBLISHED_AOT
#include "published_aot_symbols.h"
#endif

#define STASIS_ANDROID_LOG_TAG "StasisWorkshop"
#define STASIS_COMPILE_MANIFEST_RELATIVE_PATH "build/native_compile_manifest.txt"
#define STASIS_FUNCTION_ARTIFACT_DIR "build/functions"
#define STASIS_RUNTIME_STATE_RELATIVE_PATH "build/runtime_state.txt"
#define STASIS_RENDER_COMMAND_CAPACITY 8
#define STASIS_RENDER_COMMAND_STRIDE 13
#define STASIS_RENDER_FRAME_HEADER_SIZE 6
#define STASIS_RENDER_FRAME_I32_CAPACITY \
    (STASIS_RENDER_FRAME_HEADER_SIZE + STASIS_RENDER_COMMAND_CAPACITY * STASIS_RENDER_COMMAND_STRIDE)
#define FNV_OFFSET_BASIS 1469598103934665603ULL
#define FNV_PRIME 1099511628211ULL

typedef char *(*stasis_android_bridge_compile_project_fn)(const char *project_root, const char *entry_file);
typedef char *(*stasis_android_bridge_run_tests_fn)(const char *project_root);
typedef char *(*stasis_android_bridge_run_tick_fn)(const char *project_root, const char *entry_file, int touch_x, int touch_y, int touch_active, int screen_w, int screen_h);
typedef int (*stasis_android_bridge_run_tick_frame_fn)(const char *project_root, const char *entry_file, int touch_x, int touch_y, int touch_active, int screen_w, int screen_h, int32_t *out_i32, uintptr_t out_i32_len, float *out_f32, uintptr_t out_f32_len, uint8_t *out_u8, uintptr_t out_u8_len);
typedef char *(*stasis_android_bridge_last_frame_error_fn)(void);
typedef char *(*stasis_android_bridge_set_i32_global_fn)(const char *project_root, const char *entry_file, const char *path, int value);
typedef char *(*stasis_android_bridge_get_i32_global_fn)(const char *project_root, const char *entry_file, const char *path);
typedef char *(*stasis_android_bridge_resolve_sprite_asset_fn)(const char *project_root, int handle);
typedef char *(*stasis_android_bridge_resolve_cached_text_fn)(const char *project_root, int handle);
typedef char *(*stasis_android_bridge_resolve_font_fn)(const char *project_root, int handle);
typedef char *(*stasis_android_bridge_source_items_fn)(const char *project_root, const char *entry_file);
typedef char *(*stasis_android_bridge_semantic_edit_fn)(const char *project_root, const char *entry_file, const char *request_json, int dry_run, int validate, int run_tests);
typedef void (*stasis_android_bridge_free_string_fn)(char *value);
typedef char *(*stasis_codex_android_string_fn)(const char *codex_home);
typedef uint64_t (*stasis_codex_android_begin_response_fn)(void);
typedef void (*stasis_codex_android_cancel_response_fn)(void);
typedef char *(*stasis_codex_android_response_fn)(const char *codex_home, const char *request_json, uint64_t generation);
typedef int (*stasis_codex_android_initialize_fn)(void *env, void *context);
typedef void (*stasis_codex_android_free_string_fn)(char *value);
typedef struct CompileStats {
    int file_count;
    int function_count;
    int struct_count;
    int global_count;
    int has_main;
    int has_tick;
    int has_on_code_swap;
    long byte_count;
    uint64_t project_hash;
    char error[160];
} CompileStats;

typedef struct PreviousManifest {
    int found;
    int functions;
    int structs;
    int globals;
    uint64_t project_hash;
} PreviousManifest;
typedef struct RustBridgeApi {
    void *handle;
    stasis_android_bridge_compile_project_fn compile_project;
    stasis_android_bridge_run_tests_fn run_tests;
    stasis_android_bridge_run_tick_fn run_tick;
    stasis_android_bridge_run_tick_frame_fn run_tick_frame;
    stasis_android_bridge_last_frame_error_fn last_frame_error;
    stasis_android_bridge_set_i32_global_fn set_i32_global;
    stasis_android_bridge_get_i32_global_fn get_i32_global;
    stasis_android_bridge_resolve_sprite_asset_fn resolve_sprite_asset;
    stasis_android_bridge_resolve_cached_text_fn resolve_cached_text;
    stasis_android_bridge_resolve_font_fn resolve_font;
    stasis_android_bridge_source_items_fn source_items;
    stasis_android_bridge_semantic_edit_fn semantic_edit;
    stasis_android_bridge_free_string_fn free_string;
    int attempted;
} RustBridgeApi;

static RustBridgeApi rust_bridge_api = {0};
typedef struct CodexBridgeApi {
    void *handle;
    stasis_codex_android_initialize_fn initialize;
    stasis_codex_android_string_fn begin_device_login;
    stasis_codex_android_string_fn account_status;
    stasis_codex_android_string_fn account_rate_limits;
    stasis_codex_android_begin_response_fn begin_response;
    stasis_codex_android_cancel_response_fn cancel_response;
    stasis_codex_android_response_fn response;
    stasis_codex_android_free_string_fn free_string;
    int attempted;
} CodexBridgeApi;

static CodexBridgeApi codex_bridge_api = {0};
#if STASIS_ANDROID_PUBLISHED_AOT && !STASIS_RENDER_V1_DIRECT
typedef struct PublishedRenderCommand {
    int32_t kind;
    int32_t x;
    int32_t y;
    int32_t w;
    int32_t h;
    int32_t color;
    int32_t asset;
    int32_t rotation_degrees;
    int32_t alpha;
    int32_t clip_x;
    int32_t clip_y;
    int32_t clip_w;
    int32_t clip_h;
} PublishedRenderCommand;

typedef struct PublishedI32Global {
    const char *path;
    int32_t *value;
    int32_t hash;
} PublishedI32Global;

static int32_t published_input_touch_x;
static int32_t published_input_touch_y;
static int32_t published_input_touch_active;
static int32_t published_input_screen_w;
static int32_t published_input_screen_h;
static int32_t published_game_tick_count;
static int32_t published_game_screen_w;
static int32_t published_game_screen_h;
static int32_t published_game_player_y;
static int32_t published_game_ai_y;
static int32_t published_game_ball_x;
static int32_t published_game_ball_y;
static int32_t published_game_ball_vx;
static int32_t published_game_ball_vy;
static int32_t published_game_ball_age_ticks;
static int32_t published_game_enemy_paddle_speed_x100;
static int32_t published_game_player_score;
static int32_t published_game_ai_score;
static int32_t published_render_command_count;
static int32_t published_render_command_schema_version;
static PublishedRenderCommand published_render_commands[STASIS_RENDER_COMMAND_CAPACITY];
static int published_aot_globals_initialized;
static int published_aot_main_ran;
static int32_t published_runtime_tick_count;

#define RENDER_GLOBALS(index) \
    {"Render.command" #index "_kind", &published_render_commands[index].kind, 0}, \
    {"Render.command" #index "_x", &published_render_commands[index].x, 0}, \
    {"Render.command" #index "_y", &published_render_commands[index].y, 0}, \
    {"Render.command" #index "_w", &published_render_commands[index].w, 0}, \
    {"Render.command" #index "_h", &published_render_commands[index].h, 0}, \
    {"Render.command" #index "_color", &published_render_commands[index].color, 0}, \
    {"Render.command" #index "_asset", &published_render_commands[index].asset, 0}, \
    {"Render.command" #index "_rotation_degrees", &published_render_commands[index].rotation_degrees, 0}, \
    {"Render.command" #index "_alpha", &published_render_commands[index].alpha, 0}, \
    {"Render.command" #index "_clip_x", &published_render_commands[index].clip_x, 0}, \
    {"Render.command" #index "_clip_y", &published_render_commands[index].clip_y, 0}, \
    {"Render.command" #index "_clip_w", &published_render_commands[index].clip_w, 0}, \
    {"Render.command" #index "_clip_h", &published_render_commands[index].clip_h, 0}

static PublishedI32Global published_i32_globals[] = {
    {"Input.touch_x", &published_input_touch_x, 0},
    {"Input.touch_y", &published_input_touch_y, 0},
    {"Input.touch_active", &published_input_touch_active, 0},
    {"Input.screen_w", &published_input_screen_w, 0},
    {"Input.screen_h", &published_input_screen_h, 0},
    {"GameState.tick_count", &published_game_tick_count, 0},
    {"GameState.screen_w", &published_game_screen_w, 0},
    {"GameState.screen_h", &published_game_screen_h, 0},
    {"GameState.player_y", &published_game_player_y, 0},
    {"GameState.ai_y", &published_game_ai_y, 0},
    {"GameState.ball_x", &published_game_ball_x, 0},
    {"GameState.ball_y", &published_game_ball_y, 0},
    {"GameState.ball_vx", &published_game_ball_vx, 0},
    {"GameState.ball_vy", &published_game_ball_vy, 0},
    {"GameState.ball_age_ticks", &published_game_ball_age_ticks, 0},
    {"GameState.enemy_paddle_speed_x100", &published_game_enemy_paddle_speed_x100, 0},
    {"GameState.player_score", &published_game_player_score, 0},
    {"GameState.ai_score", &published_game_ai_score, 0},
    {"Render.command_count", &published_render_command_count, 0},
    {"Render.command_schema_version", &published_render_command_schema_version, 0},
    RENDER_GLOBALS(0),
    RENDER_GLOBALS(1),
    RENDER_GLOBALS(2),
    RENDER_GLOBALS(3),
    RENDER_GLOBALS(4),
    RENDER_GLOBALS(5),
    RENDER_GLOBALS(6),
    RENDER_GLOBALS(7)
};

#undef RENDER_GLOBALS

static int32_t stasis_published_hash_path(const char *path) {
    uint32_t hash = 2166136261U;
    const unsigned char *cursor = (const unsigned char *)path;
    while (*cursor != '\0') {
        hash ^= (uint32_t)(*cursor);
        hash *= 16777619U;
        cursor += 1;
    }
    return (int32_t)hash;
}

static void stasis_published_init_globals(void) {
    if (published_aot_globals_initialized) {
        return;
    }
    size_t count = sizeof(published_i32_globals) / sizeof(published_i32_globals[0]);
    for (size_t index = 0; index < count; index += 1) {
        published_i32_globals[index].hash = stasis_published_hash_path(published_i32_globals[index].path);
    }
    published_aot_globals_initialized = 1;
}

static int32_t *stasis_published_find_i32_global(int32_t path_hash) {
    stasis_published_init_globals();
    size_t count = sizeof(published_i32_globals) / sizeof(published_i32_globals[0]);
    for (size_t index = 0; index < count; index += 1) {
        if (published_i32_globals[index].hash == path_hash) {
            return published_i32_globals[index].value;
        }
    }
    return NULL;
}

int32_t stasis_jit_global_i32_load(int32_t path_hash) {
    int32_t *value = stasis_published_find_i32_global(path_hash);
    return value == NULL ? 0 : *value;
}

void stasis_jit_global_i32_store(int32_t path_hash, int32_t value) {
    int32_t *target = stasis_published_find_i32_global(path_hash);
    if (target != NULL) {
        *target = value;
    }
}

int64_t stasis_jit_lookup_code_ptr(int32_t fn_id_raw) {
    (void)fn_id_raw;
    return 0;
}

static void stasis_published_pack_frame(int32_t *out, uintptr_t out_len) {
    if (out_len < STASIS_RENDER_FRAME_I32_CAPACITY) {
        return;
    }
    memset(out, 0, sizeof(int32_t) * STASIS_RENDER_FRAME_I32_CAPACITY);
    int32_t command_count = published_render_command_count;
    if (command_count < 0) {
        command_count = 0;
    }
    if (command_count > STASIS_RENDER_COMMAND_CAPACITY) {
        command_count = STASIS_RENDER_COMMAND_CAPACITY;
    }
    out[0] = 0;
    out[1] = published_runtime_tick_count;
    out[2] = published_game_tick_count;
    out[3] = 0;
    out[4] = published_aot_main_ran ? 1 : 0;
    out[5] = command_count;
    for (int32_t index = 0; index < command_count; index += 1) {
        int base = STASIS_RENDER_FRAME_HEADER_SIZE + index * STASIS_RENDER_COMMAND_STRIDE;
        out[base] = published_render_commands[index].kind;
        out[base + 1] = published_render_commands[index].x;
        out[base + 2] = published_render_commands[index].y;
        out[base + 3] = published_render_commands[index].w;
        out[base + 4] = published_render_commands[index].h;
        out[base + 5] = published_render_commands[index].color;
        out[base + 6] = published_render_commands[index].asset;
        out[base + 7] = published_render_commands[index].rotation_degrees;
        out[base + 8] = published_render_command_schema_version >= 2
                ? published_render_commands[index].alpha : 255;
        if (published_render_command_schema_version >= 3) {
            out[base + 9] = published_render_commands[index].clip_x;
            out[base + 10] = published_render_commands[index].clip_y;
            out[base + 11] = published_render_commands[index].clip_w;
            out[base + 12] = published_render_commands[index].clip_h;
        }
    }
}

static int stasis_published_run_tick_frame(int touch_x, int touch_y, int touch_active, int screen_w, int screen_h, int32_t *out_values, uintptr_t out_len) {
    if (out_values == NULL || out_len < STASIS_RENDER_FRAME_I32_CAPACITY) {
        return -1;
    }
    stasis_published_init_globals();
    published_input_touch_x = touch_x;
    published_input_touch_y = touch_y;
    published_input_touch_active = touch_active;
    published_input_screen_w = screen_w;
    published_input_screen_h = screen_h;
    if (!published_aot_main_ran) {
        STASIS_AOT_MAIN();
        published_aot_main_ran = 1;
    }
    STASIS_AOT_TICK();
    STASIS_AOT_RENDER();
    published_runtime_tick_count += 1;
    stasis_published_pack_frame(out_values, out_len);
    return 0;
}

static int stasis_published_run_tick_frame_v1(
        int touch_x, int touch_y, int touch_active, int screen_w, int screen_h,
        int32_t *out_i32, float *out_f32, uint8_t *out_u8) {
    int32_t legacy[STASIS_RENDER_FRAME_I32_CAPACITY] = {0};
    int status = stasis_published_run_tick_frame(
            touch_x, touch_y, touch_active, screen_w, screen_h,
            legacy, STASIS_RENDER_FRAME_I32_CAPACITY);
    if (status != 0) return status;
    (void)out_u8;
    out_i32[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_V1_MAGIC;
    out_i32[STASIS_RENDER_I_VERSION] = STASIS_RENDER_V1_VERSION;
    out_i32[STASIS_RENDER_I_FLAGS] = STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT;
    out_i32[STASIS_RENDER_I_LINE_COUNT] = 0;
    out_i32[STASIS_RENDER_I_SPRITE_COUNT] = 0;
    out_i32[STASIS_RENDER_I_TEXT_COUNT] = 0;
    out_i32[STASIS_RENDER_I_TEXT_BYTES_USED] = 0;
    out_f32[STASIS_RENDER_F_CLEAR_BASE + 0] = 15.0f / 255.0f;
    out_f32[STASIS_RENDER_F_CLEAR_BASE + 1] = 20.0f / 255.0f;
    out_f32[STASIS_RENDER_F_CLEAR_BASE + 2] = 28.0f / 255.0f;
    out_f32[STASIS_RENDER_F_CLEAR_BASE + 3] = 1.0f;
    int32_t command_count = stasis_render_clamp_count(
            legacy[5], STASIS_RENDER_COMMAND_CAPACITY);
    for (int32_t index = 0; index < command_count; index += 1) {
        int32_t base = STASIS_RENDER_FRAME_HEADER_SIZE + index * STASIS_RENDER_COMMAND_STRIDE;
        if (legacy[base] == 1) {
            int32_t line = out_i32[STASIS_RENDER_I_LINE_COUNT];
            if (line > STASIS_RENDER_MAX_LINES - 4) continue;
            float x = (float)legacy[base + 1];
            float y = (float)legacy[base + 2];
            float right = x + (float)legacy[base + 3];
            float bottom = y + (float)legacy[base + 4];
            int32_t color = legacy[base + 5];
            float r = (float)((color >> 16) & 255) / 255.0f;
            float g = (float)((color >> 8) & 255) / 255.0f;
            float b = (float)(color & 255) / 255.0f;
            float a = (float)legacy[base + 8] / 255.0f;
            const float points[16] = {
                x, y, right, y, right, y, right, bottom,
                right, bottom, x, bottom, x, bottom, x, y};
            for (int edge = 0; edge < 4; edge += 1) {
                int32_t line_base = STASIS_RENDER_F_LINE_BASE +
                        (line + edge) * STASIS_RENDER_LINE_F32_STRIDE;
                out_f32[line_base + 0] = points[edge * 4 + 0];
                out_f32[line_base + 1] = points[edge * 4 + 1];
                out_f32[line_base + 2] = points[edge * 4 + 2];
                out_f32[line_base + 3] = points[edge * 4 + 3];
                out_f32[line_base + 4] = r;
                out_f32[line_base + 5] = g;
                out_f32[line_base + 6] = b;
                out_f32[line_base + 7] = a;
            }
            out_i32[STASIS_RENDER_I_LINE_COUNT] = line + 4;
        } else if (legacy[base] == 2) {
            int32_t sprite = out_i32[STASIS_RENDER_I_SPRITE_COUNT];
            if (sprite >= STASIS_RENDER_MAX_SPRITES) continue;
            int32_t sprite_base = STASIS_RENDER_I_SPRITE_BASE +
                    sprite * STASIS_RENDER_SPRITE_I32_STRIDE;
            out_i32[sprite_base + 0] = legacy[base + 6];
            out_i32[sprite_base + 1] = legacy[base + 1];
            out_i32[sprite_base + 2] = legacy[base + 2];
            out_i32[sprite_base + 3] = legacy[base + 3];
            out_i32[sprite_base + 4] = legacy[base + 4];
            out_i32[sprite_base + 5] = legacy[base + 7];
            out_i32[sprite_base + 6] = legacy[base + 8];
            out_i32[STASIS_RENDER_I_SPRITE_COUNT] = sprite + 1;
        }
    }
    return 0;
}
#endif

#if STASIS_ANDROID_PUBLISHED_AOT && STASIS_RENDER_V1_DIRECT
#define STASIS_PUBLISHED_MAX_FONTS 64
#define STASIS_PUBLISHED_MAX_TEXT_RUNS 4096
typedef struct PublishedFontResource { char path[256]; int32_t size; } PublishedFontResource;
typedef struct PublishedTextResource { int32_t font; char text[1025]; float width; } PublishedTextResource;
static PublishedFontResource published_fonts[STASIS_PUBLISHED_MAX_FONTS];
static PublishedTextResource published_text_runs[STASIS_PUBLISHED_MAX_TEXT_RUNS];
static int32_t published_font_count;
static int32_t published_text_run_count;
static char published_resource_error[256];

extern int32_t stasis_published_sprite_handle_for_path(const char *path);

int stasis_gfx_load_sprite(const char *path, int max_w, int max_h) {
    if (path == NULL || max_w <= 0 || max_h <= 0) {
        snprintf(published_resource_error, sizeof(published_resource_error),
                "sprite load rejected: invalid path or dimensions");
        return 0;
    }
    int32_t handle = stasis_published_sprite_handle_for_path(path);
    if (handle == 0) {
        snprintf(published_resource_error, sizeof(published_resource_error),
                "sprite is not declared in the packaged asset manifest: %.160s", path);
    }
    return handle;
}
void stasis_gfx_release_sprite(int handle) { (void)handle; }
int stasis_gfx_dump_bmp(const char *path) { (void)path; return 0; }
int stasis_gfx_dump_png(const char *path) { (void)path; return 0; }
int stasis_gfx_poll_reload(int handle) { (void)handle; return 0; }

int stasis_load_font(const char *path, int size) {
    if (path == NULL || size <= 0) {
        snprintf(published_resource_error, sizeof(published_resource_error),
                "font load rejected: invalid path or size");
        return 0;
    }
    for (int32_t index = 0; index < published_font_count; index += 1) {
        if (published_fonts[index].size == size && strcmp(published_fonts[index].path, path) == 0) {
            return index + 1;
        }
    }
    if (published_font_count >= STASIS_PUBLISHED_MAX_FONTS || strlen(path) >= 256) {
        snprintf(published_resource_error, sizeof(published_resource_error), "font registry full or path too long");
        return 0;
    }
    PublishedFontResource *font = &published_fonts[published_font_count++];
    snprintf(font->path, sizeof(font->path), "%s", path);
    font->size = size;
    return published_font_count;
}

float stasis_measure_text(int font, const char *text) {
    if (font <= 0 || font > published_font_count || text == NULL) return 0.0f;
    return (float)strlen(text) * (float)published_fonts[font - 1].size * 0.6f;
}

int stasis_gfx_cache_text(int font, const char *text) {
    if (font <= 0 || font > published_font_count || text == NULL) {
        snprintf(published_resource_error, sizeof(published_resource_error),
                "cached text rejected: font handle was not loaded or text is null");
        return 0;
    }
    for (int32_t index = 0; index < published_text_run_count; index += 1) {
        if (published_text_runs[index].font == font && strcmp(published_text_runs[index].text, text) == 0) {
            return index + 1;
        }
    }
    if (published_text_run_count >= STASIS_PUBLISHED_MAX_TEXT_RUNS || strlen(text) >= 1025) {
        snprintf(published_resource_error, sizeof(published_resource_error), "cached text registry full or text too long");
        return 0;
    }
    PublishedTextResource *run = &published_text_runs[published_text_run_count++];
    run->font = font;
    snprintf(run->text, sizeof(run->text), "%s", text);
    run->width = stasis_measure_text(font, text);
    return published_text_run_count;
}

float stasis_gfx_measure_text_cached(int handle) {
    return handle > 0 && handle <= published_text_run_count
            ? published_text_runs[handle - 1].width : 0.0f;
}

int stasis_audio_init(int sample_rate, int channels, int target_latency_frames) {
    (void)sample_rate; (void)channels; (void)target_latency_frames; return 0;
}
void stasis_audio_shutdown(void) {}
int stasis_audio_is_available(void) { return 0; }
int stasis_audio_get_sample_rate(void) { return 0; }
int stasis_audio_get_channels(void) { return 0; }
int stasis_audio_get_queued_frames(void) { return 0; }
int stasis_audio_get_underruns(void) { return 0; }
int stasis_audio_push_f32_interleaved(const float *samples, int frames) {
    (void)samples; (void)frames; return 0;
}
void stasis_sleep_ms(int ms) {
    if (ms <= 0) return;
    struct timespec delay = { ms / 1000, (long)(ms % 1000) * 1000000L };
    nanosleep(&delay, NULL);
}
int stasis_get_time_ms(void) {
    struct timespec now;
    return clock_gettime(CLOCK_MONOTONIC, &now) == 0
            ? (int)(now.tv_sec * 1000 + now.tv_nsec / 1000000) : 0;
}
int stasis_get_time_us(void) {
    struct timespec now;
    return clock_gettime(CLOCK_MONOTONIC, &now) == 0
            ? (int)(now.tv_sec * 1000000 + now.tv_nsec / 1000) : 0;
}

static int published_v1_initialized;
static int32_t *published_v1_i32;
static float *published_v1_f32;
static uint8_t *published_v1_u8;
static int32_t published_host_i32[768];
static float published_host_f32[64];
static int32_t published_previous_touch_x;
static int32_t published_previous_touch_y;
static int32_t published_previous_touch_active;
static int32_t published_has_previous_input;

static int32_t stasis_published_hash_path(const char *path) {
    uint32_t hash = 2166136261U;
    while (*path != '\0') {
        hash ^= (uint8_t)*path++;
        hash *= 16777619U;
    }
    return (int32_t)hash;
}

static void stasis_published_write_host_frame(
        int touch_x, int touch_y, int touch_active, int screen_w, int screen_h) {
    int32_t was_down = published_has_previous_input && published_previous_touch_active != 0;
    int32_t is_down = touch_active != 0;
    published_host_i32[0] = stasis_get_time_ms();
    published_host_i32[1] = screen_w;
    published_host_i32[2] = screen_h;
    published_host_i32[5] = screen_w;
    published_host_i32[6] = screen_h;
    published_host_i32[7] = 1;
    published_host_i32[12] = screen_w;
    published_host_i32[13] = screen_h;
    published_host_i32[14] = 1;
    published_host_i32[16] = 60;
    published_host_i32[17] = 1;
    published_host_i32[19] = stasis_get_time_us();
    published_host_i32[544] = 0;
    published_host_i32[545] = is_down;
    published_host_i32[546] = is_down && !was_down;
    published_host_i32[547] = !is_down && was_down;
    published_host_f32[0] = (float)touch_x;
    published_host_f32[1] = (float)touch_y;
    published_host_f32[2] = published_has_previous_input
            ? (float)(touch_x - published_previous_touch_x) : 0.0f;
    published_host_f32[3] = published_has_previous_input
            ? (float)(touch_y - published_previous_touch_y) : 0.0f;
    published_host_f32[4] = screen_w > 0 ? (float)touch_x / (float)screen_w : 0.0f;
    published_host_f32[5] = screen_h > 0 ? (float)touch_y / (float)screen_h : 0.0f;
    published_previous_touch_x = touch_x;
    published_previous_touch_y = touch_y;
    published_previous_touch_active = touch_active;
    published_has_previous_input = 1;
}

static int stasis_published_run_tick_frame_v1(
        int touch_x, int touch_y, int touch_active, int screen_w, int screen_h,
        int32_t *out_i32, float *out_f32, uint8_t *out_u8) {
    if (published_v1_initialized &&
            (out_i32 != published_v1_i32 || out_f32 != published_v1_f32 || out_u8 != published_v1_u8)) {
        published_v1_initialized = 0;
        published_v1_i32 = NULL;
        published_v1_f32 = NULL;
        published_v1_u8 = NULL;
        published_font_count = 0;
        published_text_run_count = 0;
        published_has_previous_input = 0;
        memset(published_host_i32, 0, sizeof(published_host_i32));
        memset(published_host_f32, 0, sizeof(published_host_f32));
    }
    if (!published_v1_initialized) {
        published_resource_error[0] = '\0';
        stasis_mobile_aot_reset();
        STASIS_AOT_BIND_RUNTIME_GLOBALS();
        stasis_jit_register_global_i32_array(
                stasis_published_hash_path("host_i32"), 0, published_host_i32, 768);
        stasis_jit_register_global_f32_array(
                stasis_published_hash_path("host_f32"), 0, published_host_f32, 64);
        stasis_jit_register_global_i32_array(
                stasis_published_hash_path("gfx_cmd_i32"), 0, out_i32, STASIS_RENDER_I32_COUNT);
        stasis_jit_register_global_f32_array(
                stasis_published_hash_path("gfx_cmd_f32"), 0, out_f32, STASIS_RENDER_F32_COUNT);
        stasis_jit_register_global_u8_array(
                stasis_published_hash_path("gfx_cmd_u8"), 0, out_u8, STASIS_RENDER_U8_COUNT);
        published_v1_i32 = out_i32;
        published_v1_f32 = out_f32;
        published_v1_u8 = out_u8;
        stasis_published_write_host_frame(touch_x, touch_y, touch_active, screen_w, screen_h);
        if (STASIS_AOT_MAIN() != 0) return -1;
        if (published_resource_error[0] != '\0') return -1;
        published_v1_initialized = 1;
    }
    stasis_published_write_host_frame(touch_x, touch_y, touch_active, screen_w, screen_h);
    if (STASIS_AOT_TICK() != 0 || STASIS_AOT_RENDER() != 0) return -1;
    if (published_resource_error[0] != '\0') return -1;
    published_host_i32[10] += 1;
    return stasis_render_v1_is_valid(out_i32) ? 0 : -1;
}
#endif
static char *read_file_text(const char *path, long *size_out);

static int has_suffix(const char *value, const char *suffix) {
    size_t value_len = strlen(value);
    size_t suffix_len = strlen(suffix);
    if (value_len < suffix_len) {
        return 0;
    }
    return strcmp(value + value_len - suffix_len, suffix) == 0;
}

static void hash_bytes(CompileStats *stats, const char *value, size_t length) {
    for (size_t index = 0; index < length; index += 1) {
        stats->project_hash ^= (unsigned char)value[index];
        stats->project_hash *= FNV_PRIME;
    }
}

static int count_token(const char *source, const char *token) {
    int count = 0;
    const char *cursor = source;
    while ((cursor = strstr(cursor, token)) != NULL) {
        count += 1;
        cursor += strlen(token);
    }
    return count;
}
static uint64_t hash_slice(const char *source, size_t length) {
    uint64_t hash = FNV_OFFSET_BASIS;
    for (size_t index = 0; index < length; index += 1) {
        hash ^= (unsigned char)source[index];
        hash *= FNV_PRIME;
    }
    return hash;
}

static const char *find_matching_function_end(const char *body_start) {
    int depth = 0;
    int line_comment = 0;
    int block_comment = 0;
    int string_literal = 0;

    for (const char *cursor = body_start; *cursor != '\0'; cursor += 1) {
        char current = *cursor;
        char next = *(cursor + 1);

        if (line_comment) {
            if (current == '\n') {
                line_comment = 0;
            }
            continue;
        }
        if (block_comment) {
            if (current == '*' && next == '/') {
                block_comment = 0;
                cursor += 1;
            }
            continue;
        }
        if (string_literal) {
            if (current == '\\' && next != '\0') {
                cursor += 1;
                continue;
            }
            if (current == '"') {
                string_literal = 0;
            }
            continue;
        }
        if (current == '/' && next == '/') {
            line_comment = 1;
            cursor += 1;
            continue;
        }
        if (current == '/' && next == '*') {
            block_comment = 1;
            cursor += 1;
            continue;
        }
        if (current == '"') {
            string_literal = 1;
            continue;
        }
        if (current == '{') {
            depth += 1;
        } else if (current == '}') {
            depth -= 1;
            if (depth == 0) {
                return cursor + 1;
            }
        }
    }

    return NULL;
}

static void write_escaped_manifest_field(FILE *file, const char *start, size_t length) {
    for (size_t index = 0; index < length; index += 1) {
        char value = start[index];
        if (value == '\n' || value == '\r' || value == '|') {
            fputc(' ', file);
        } else {
            fputc(value, file);
        }
    }
}

static int write_function_artifact(
        const char *artifact_dir,
        const char *path,
        const char *signature_start,
        size_t signature_length,
        uint64_t signature_hash,
        uint64_t body_hash) {
    char artifact_path[1200];
    snprintf(artifact_path, sizeof(artifact_path), "%s/%016llx.stub", artifact_dir, (unsigned long long)body_hash);

    FILE *artifact = fopen(artifact_path, "wb");
    if (artifact == NULL) {
        return -1;
    }

    fprintf(artifact, "status=CompiledStub\n");
    fprintf(artifact, "source=");
    write_escaped_manifest_field(artifact, path, strlen(path));
    fprintf(artifact, "\n");
    fprintf(artifact, "signature=");
    write_escaped_manifest_field(artifact, signature_start, signature_length);
    fprintf(artifact, "\n");
    fprintf(artifact, "signature_hash=%016llx\n", (unsigned long long)signature_hash);
    fprintf(artifact, "body_hash=%016llx\n", (unsigned long long)body_hash);
    fclose(artifact);
    return 0;
}
static int write_function_manifest_entries(FILE *manifest, const char *artifact_dir, const char *path, const char *source) {
    const char *cursor = source;
    while ((cursor = strstr(cursor, "function ")) != NULL) {
        const char *signature_start = cursor + strlen("function ");
        const char *body_start = strchr(signature_start, '{');
        if (body_start == NULL) {
            break;
        }

        const char *body_end = find_matching_function_end(body_start);
        if (body_end == NULL) {
            break;
        }

        const char *signature_end = body_start;
        while (signature_end > signature_start && (*(signature_end - 1) == ' ' || *(signature_end - 1) == '\n' || *(signature_end - 1) == '\r' || *(signature_end - 1) == '\t')) {
            signature_end -= 1;
        }

        uint64_t signature_hash = hash_slice(signature_start, (size_t)(signature_end - signature_start));
        uint64_t body_hash = hash_slice(body_start, (size_t)(body_end - body_start));
        fprintf(manifest, "function=");
        write_escaped_manifest_field(manifest, path, strlen(path));
        fprintf(manifest, "|");
        write_escaped_manifest_field(manifest, signature_start, (size_t)(signature_end - signature_start));
        fprintf(
                manifest,
                "|signature_hash=%016llx|body_hash=%016llx|artifact=%s/%016llx.stub\n",
                (unsigned long long)signature_hash,
                (unsigned long long)body_hash,
                STASIS_FUNCTION_ARTIFACT_DIR,
                (unsigned long long)body_hash);

        if (write_function_artifact(
                artifact_dir,
                path,
                signature_start,
                (size_t)(signature_end - signature_start),
                signature_hash,
                body_hash) != 0) {
            return -1;
        }

        cursor = body_end;
    }
    return 0;
}

static int append_function_entries_for_project(FILE *manifest, const char *artifact_dir, const char *path) {
    DIR *dir = opendir(path);
    if (dir == NULL) {
        return -1;
    }

    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL) {
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) {
            continue;
        }

        size_t path_len = strlen(path);
        size_t name_len = strlen(entry->d_name);
        char *child = (char *)malloc(path_len + 1 + name_len + 1);
        if (child == NULL) {
            closedir(dir);
            return -1;
        }
        memcpy(child, path, path_len);
        child[path_len] = '/';
        memcpy(child + path_len + 1, entry->d_name, name_len + 1);

        struct stat info;
        if (stat(child, &info) == 0) {
            if (S_ISDIR(info.st_mode)) {
                int result = append_function_entries_for_project(manifest, artifact_dir, child);
                free(child);
                if (result != 0) {
                    closedir(dir);
                    return result;
                }
                continue;
            }
            if (S_ISREG(info.st_mode) && has_suffix(entry->d_name, ".stasis")) {
                long size = 0;
                char *source = read_file_text(child, &size);
                (void)size;
                if (source == NULL) {
                    free(child);
                    closedir(dir);
                    return -1;
                }
                if (write_function_manifest_entries(manifest, artifact_dir, child, source) != 0) {
                    free(source);
                    free(child);
                    closedir(dir);
                    return -1;
                }
                free(source);
            }
        }
        free(child);
    }

    closedir(dir);
    return 0;
}

static void set_error(CompileStats *stats, const char *message, const char *path) {
    if (stats->error[0] != '\0') {
        return;
    }
    snprintf(stats->error, sizeof(stats->error), "%s: %s", message, path);
}

static int validate_braces(const char *source, const char *path, CompileStats *stats) {
    int depth = 0;
    int line_comment = 0;
    int block_comment = 0;
    int string_literal = 0;

    for (size_t index = 0; source[index] != '\0'; index += 1) {
        char current = source[index];
        char next = source[index + 1];

        if (line_comment) {
            if (current == '\n') {
                line_comment = 0;
            }
            continue;
        }

        if (block_comment) {
            if (current == '*' && next == '/') {
                block_comment = 0;
                index += 1;
            }
            continue;
        }

        if (string_literal) {
            if (current == '\\' && next != '\0') {
                index += 1;
                continue;
            }
            if (current == '"') {
                string_literal = 0;
            }
            continue;
        }

        if (current == '/' && next == '/') {
            line_comment = 1;
            index += 1;
            continue;
        }

        if (current == '/' && next == '*') {
            block_comment = 1;
            index += 1;
            continue;
        }

        if (current == '"') {
            string_literal = 1;
            continue;
        }

        if (current == '{') {
            depth += 1;
        } else if (current == '}') {
            depth -= 1;
            if (depth < 0) {
                set_error(stats, "CompileError: unmatched closing brace", path);
                return -1;
            }
        }
    }

    if (depth != 0) {
        set_error(stats, "CompileError: unmatched opening brace", path);
        return -1;
    }

    if (block_comment) {
        set_error(stats, "CompileError: unterminated block comment", path);
        return -1;
    }

    if (string_literal) {
        set_error(stats, "CompileError: unterminated string literal", path);
        return -1;
    }

    return 0;
}

static char *read_file_text(const char *path, long *size_out) {
    FILE *file = fopen(path, "rb");
    if (file == NULL) {
        return NULL;
    }

    if (fseek(file, 0, SEEK_END) != 0) {
        fclose(file);
        return NULL;
    }

    long size = ftell(file);
    if (size < 0) {
        fclose(file);
        return NULL;
    }

    if (fseek(file, 0, SEEK_SET) != 0) {
        fclose(file);
        return NULL;
    }

    char *buffer = (char *)malloc((size_t)size + 1);
    if (buffer == NULL) {
        fclose(file);
        return NULL;
    }

    size_t read = fread(buffer, 1, (size_t)size, file);
    fclose(file);
    buffer[read] = '\0';
    *size_out = (long)read;
    return buffer;
}

static int parse_manifest_i32(const char *manifest, const char *key, int *out) {
    const char *cursor = strstr(manifest, key);
    if (cursor == NULL) {
        return 0;
    }
    cursor += strlen(key);
    *out = atoi(cursor);
    return 1;
}

static int parse_manifest_u64(const char *manifest, const char *key, uint64_t *out) {
    const char *cursor = strstr(manifest, key);
    if (cursor == NULL) {
        return 0;
    }
    cursor += strlen(key);
    *out = (uint64_t)strtoull(cursor, NULL, 16);
    return 1;
}

static void read_previous_compile_manifest(const char *project_root, PreviousManifest *previous) {
    memset(previous, 0, sizeof(*previous));

    char manifest_path[1200];
    snprintf(manifest_path, sizeof(manifest_path), "%s/%s", project_root, STASIS_COMPILE_MANIFEST_RELATIVE_PATH);

    long size = 0;
    char *manifest = read_file_text(manifest_path, &size);
    if (manifest == NULL || size == 0) {
        free(manifest);
        return;
    }

    previous->found = 1;
    parse_manifest_u64(manifest, "project_hash=", &previous->project_hash);
    parse_manifest_i32(manifest, "functions=", &previous->functions);
    parse_manifest_i32(manifest, "structs=", &previous->structs);
    parse_manifest_i32(manifest, "globals=", &previous->globals);
    free(manifest);
}

static const char *classify_reload(const CompileStats *stats, const PreviousManifest *previous) {
    if (!previous->found) {
        return "InitialCompile";
    }
    if (previous->project_hash == stats->project_hash) {
        return "NoChange";
    }
    if (previous->functions != stats->function_count ||
        previous->structs != stats->struct_count ||
        previous->globals != stats->global_count) {
        return "ResetRequired";
    }
    return "FastReload";
}
static int analyze_stasis_file(const char *path, CompileStats *stats) {
    long size = 0;
    char *source = read_file_text(path, &size);
    if (source == NULL) {
        set_error(stats, "CompileError: unreadable file", path);
        return -1;
    }

    if (validate_braces(source, path, stats) != 0) {
        free(source);
        return -1;
    }

    stats->file_count += 1;
    stats->byte_count += size;
    stats->function_count += count_token(source, "function ");
    stats->struct_count += count_token(source, "struct ");
    stats->global_count += count_token(source, "global ");
    hash_bytes(stats, path, strlen(path));
    hash_bytes(stats, "\n", 1);
    hash_bytes(stats, source, (size_t)size);

    if (strstr(source, "function main(") != NULL) {
        stats->has_main = 1;
    }
    if (strstr(source, "function tick(") != NULL) {
        stats->has_tick = 1;
    }
    if (strstr(source, "function on_code_swap(") != NULL) {
        stats->has_on_code_swap = 1;
    }

    free(source);
    return 0;
}

static int scan_stasis_files(const char *path, CompileStats *stats) {
    DIR *dir = opendir(path);
    if (dir == NULL) {
        set_error(stats, "CompileError: unable to open project root", path);
        return -1;
    }

    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL) {
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) {
            continue;
        }

        size_t path_len = strlen(path);
        size_t name_len = strlen(entry->d_name);
        char *child = (char *)malloc(path_len + 1 + name_len + 1);
        if (child == NULL) {
            closedir(dir);
            set_error(stats, "CompileError: out of memory", path);
            return -1;
        }
        memcpy(child, path, path_len);
        child[path_len] = '/';
        memcpy(child + path_len + 1, entry->d_name, name_len + 1);

        struct stat info;
        if (stat(child, &info) == 0) {
            if (S_ISDIR(info.st_mode)) {
                int result = scan_stasis_files(child, stats);
                free(child);
                if (result != 0) {
                    closedir(dir);
                    return result;
                }
                continue;
            }

            if (S_ISREG(info.st_mode) && has_suffix(entry->d_name, ".stasis")) {
                int result = analyze_stasis_file(child, stats);
                free(child);
                if (result != 0) {
                    closedir(dir);
                    return result;
                }
                continue;
            }
        }
        free(child);
    }

    closedir(dir);
    return 0;
}

static int ensure_directory(const char *path, CompileStats *stats) {
    struct stat info;
    if (stat(path, &info) == 0) {
        if (S_ISDIR(info.st_mode)) {
            return 0;
        }
        set_error(stats, "CompileError: build path is not a directory", path);
        return -1;
    }

    if (mkdir(path, 0700) != 0) {
        set_error(stats, "CompileError: unable to create build directory", path);
        return -1;
    }
    return 0;
}

static int write_runtime_state(const char *project_root, const CompileStats *stats, const char *reload_classification) {
    if (strcmp(reload_classification, "NoChange") == 0 || strcmp(reload_classification, "FastReload") == 0) {
        return 0;
    }

    char state_path[1200];
    snprintf(state_path, sizeof(state_path), "%s/%s", project_root, STASIS_RUNTIME_STATE_RELATIVE_PATH);

    FILE *state = fopen(state_path, "wb");
    if (state == NULL) {
        return -1;
    }

    fprintf(state, "status=RuntimeStateReady\n");
    fprintf(state, "project_hash=%016llx\n", (unsigned long long)stats->project_hash);
    fprintf(state, "reload=%s\n", reload_classification);
    fprintf(state, "tick_count=0\n");
    fprintf(state, "globals=%d\n", stats->global_count);
    fclose(state);
    return 0;
}

static int write_compile_manifest(const char *project_root, const CompileStats *stats, const char *reload_classification) {
    char build_dir[1024];
    snprintf(build_dir, sizeof(build_dir), "%s/build", project_root);

    CompileStats mutable_stats = *stats;
    if (ensure_directory(build_dir, &mutable_stats) != 0) {
        return -1;
    }

    char artifact_dir[1024];
    snprintf(artifact_dir, sizeof(artifact_dir), "%s/%s", project_root, STASIS_FUNCTION_ARTIFACT_DIR);
    if (ensure_directory(artifact_dir, &mutable_stats) != 0) {
        return -1;
    }

    char manifest_path[1200];
    snprintf(manifest_path, sizeof(manifest_path), "%s/%s", project_root, STASIS_COMPILE_MANIFEST_RELATIVE_PATH);

    FILE *file = fopen(manifest_path, "wb");
    if (file == NULL) {
        return -1;
    }

    fprintf(file, "status=CompilePlanned\n");
    fprintf(file, "reload=%s\n", reload_classification);
    fprintf(file, "project_hash=%016llx\n", (unsigned long long)stats->project_hash);
    fprintf(file, "files=%d\n", stats->file_count);
    fprintf(file, "bytes=%ld\n", stats->byte_count);
    fprintf(file, "functions=%d\n", stats->function_count);
    fprintf(file, "structs=%d\n", stats->struct_count);
    fprintf(file, "globals=%d\n", stats->global_count);
    fprintf(file, "roots=main,tick%s\n", stats->has_on_code_swap ? ",on_code_swap" : "");
    fprintf(file, "entrypoint=main\n");
    fprintf(file, "entrypoint=tick\n");
    if (stats->has_on_code_swap) {
        fprintf(file, "entrypoint=on_code_swap\n");
    }
    fprintf(file, "runtime_state=%s\n", STASIS_RUNTIME_STATE_RELATIVE_PATH);
    fclose(file);

    if (write_runtime_state(project_root, stats, reload_classification) != 0) {
        return -1;
    }
    return 0;
}

static int read_runtime_tick_count(const char *project_root, int *tick_count) {
    char state_path[1200];
    snprintf(state_path, sizeof(state_path), "%s/%s", project_root, STASIS_RUNTIME_STATE_RELATIVE_PATH);

    long size = 0;
    char *state = read_file_text(state_path, &size);
    if (state == NULL || size == 0) {
        free(state);
        return -1;
    }

    int parsed = parse_manifest_i32(state, "tick_count=", tick_count);
    free(state);
    return parsed ? 0 : -1;
}

static int write_runtime_tick_count(const char *project_root, int tick_count) {
    char state_path[1200];
    snprintf(state_path, sizeof(state_path), "%s/%s", project_root, STASIS_RUNTIME_STATE_RELATIVE_PATH);

    FILE *state = fopen(state_path, "wb");
    if (state == NULL) {
        return -1;
    }

    fprintf(state, "status=RuntimeStateReady\n");
    fprintf(state, "tick_count=%d\n", tick_count);
    fclose(state);
    return 0;
}

static RustBridgeApi *load_rust_bridge_api(void) {
    if (rust_bridge_api.attempted) {
        return rust_bridge_api.handle == NULL ? NULL : &rust_bridge_api;
    }

    rust_bridge_api.attempted = 1;
    rust_bridge_api.handle = dlopen("libstasis_android_bridge.so", RTLD_NOW | RTLD_LOCAL);
    if (rust_bridge_api.handle == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG, "Rust Android bridge unavailable: %s", dlerror());
        return NULL;
    }

    rust_bridge_api.compile_project =
            (stasis_android_bridge_compile_project_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_compile_project");
    rust_bridge_api.run_tests =
            (stasis_android_bridge_run_tests_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tests");
    rust_bridge_api.run_tick =
            (stasis_android_bridge_run_tick_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tick");
    rust_bridge_api.run_tick_frame =
            (stasis_android_bridge_run_tick_frame_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tick_frame_v1");
    rust_bridge_api.last_frame_error =
            (stasis_android_bridge_last_frame_error_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_last_frame_error");
    rust_bridge_api.set_i32_global =
            (stasis_android_bridge_set_i32_global_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_set_i32_global");
    rust_bridge_api.get_i32_global =
            (stasis_android_bridge_get_i32_global_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_get_i32_global");
    rust_bridge_api.resolve_sprite_asset =
            (stasis_android_bridge_resolve_sprite_asset_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_resolve_sprite_asset");
    rust_bridge_api.resolve_cached_text =
            (stasis_android_bridge_resolve_cached_text_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_resolve_cached_text");
    rust_bridge_api.resolve_font =
            (stasis_android_bridge_resolve_font_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_resolve_font");
    rust_bridge_api.source_items =
            (stasis_android_bridge_source_items_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_source_items");
    rust_bridge_api.semantic_edit =
            (stasis_android_bridge_semantic_edit_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_semantic_edit");
    rust_bridge_api.free_string =
            (stasis_android_bridge_free_string_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_free_string");
    if (rust_bridge_api.compile_project == NULL ||
        rust_bridge_api.run_tick == NULL ||
        rust_bridge_api.free_string == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG, "Rust Android bridge missing required symbols");
        return NULL;
    }

    return &rust_bridge_api;
}

static CodexBridgeApi *load_codex_bridge_api(void) {
    if (codex_bridge_api.attempted) {
        return codex_bridge_api.handle == NULL ? NULL : &codex_bridge_api;
    }

    codex_bridge_api.attempted = 1;
    codex_bridge_api.handle = dlopen("libstasis_codex_android.so", RTLD_NOW | RTLD_LOCAL);
    if (codex_bridge_api.handle == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG,
                "Phone-native Codex bridge unavailable: %s", dlerror());
        return NULL;
    }
    codex_bridge_api.begin_device_login = (stasis_codex_android_string_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_begin_device_login");
    codex_bridge_api.initialize = (stasis_codex_android_initialize_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_initialize");
    codex_bridge_api.account_status = (stasis_codex_android_string_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_account_status");
    codex_bridge_api.account_rate_limits = (stasis_codex_android_string_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_account_rate_limits");
    codex_bridge_api.begin_response = (stasis_codex_android_begin_response_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_begin_response");
    codex_bridge_api.cancel_response = (stasis_codex_android_cancel_response_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_cancel_response");
    codex_bridge_api.response = (stasis_codex_android_response_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_response");
    codex_bridge_api.free_string = (stasis_codex_android_free_string_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_free_string");
    if (codex_bridge_api.initialize == NULL ||
        codex_bridge_api.begin_device_login == NULL ||
        codex_bridge_api.account_status == NULL ||
        codex_bridge_api.account_rate_limits == NULL ||
        codex_bridge_api.begin_response == NULL ||
        codex_bridge_api.cancel_response == NULL ||
        codex_bridge_api.response == NULL ||
        codex_bridge_api.free_string == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG,
                "Phone-native Codex bridge missing required symbols");
        return NULL;
    }
    return &codex_bridge_api;
}

static jstring call_codex_bridge(JNIEnv *env, jstring codex_home, int begin_login) {
    if (codex_home == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex home was null\"}");
    }
    const char *home = (*env)->GetStringUTFChars(env, codex_home, NULL);
    if (home == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex home was unreadable\"}");
    }
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL) {
        (*env)->ReleaseStringUTFChars(env, codex_home, home);
        return (*env)->NewStringUTF(env, "{\"status\":\"unavailable\",\"error\":\"Phone-native Codex library is not packaged\"}");
    }
    char *response = begin_login ? bridge->begin_device_login(home) : bridge->account_status(home);
    (*env)->ReleaseStringUTFChars(env, codex_home, home);
    if (response == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Phone-native Codex returned no response\"}");
    }
    jstring result = (*env)->NewStringUTF(env, response);
    bridge->free_string(response);
    return result;
}

static jstring call_codex_rate_limits(JNIEnv *env, jstring codex_home) {
    if (codex_home == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex home was null\"}");
    }
    const char *home = (*env)->GetStringUTFChars(env, codex_home, NULL);
    if (home == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex home was unreadable\"}");
    }
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL) {
        (*env)->ReleaseStringUTFChars(env, codex_home, home);
        return (*env)->NewStringUTF(env, "{\"status\":\"unavailable\",\"error\":\"Phone-native Codex library is not packaged\"}");
    }
    char *response = bridge->account_rate_limits(home);
    (*env)->ReleaseStringUTFChars(env, codex_home, home);
    if (response == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Phone-native Codex returned no rate limits\"}");
    }
    jstring result = (*env)->NewStringUTF(env, response);
    bridge->free_string(response);
    return result;
}

static jstring call_codex_response(JNIEnv *env, jstring codex_home, jstring request_json, uint64_t generation) {
    if (codex_home == NULL || request_json == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex request was null\"}");
    }
    const char *home = (*env)->GetStringUTFChars(env, codex_home, NULL);
    const char *request = (*env)->GetStringUTFChars(env, request_json, NULL);
    if (home == NULL || request == NULL) {
        if (home != NULL) (*env)->ReleaseStringUTFChars(env, codex_home, home);
        if (request != NULL) (*env)->ReleaseStringUTFChars(env, request_json, request);
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex request was unreadable\"}");
    }
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL) {
        (*env)->ReleaseStringUTFChars(env, codex_home, home);
        (*env)->ReleaseStringUTFChars(env, request_json, request);
        return (*env)->NewStringUTF(env, "{\"status\":\"unavailable\",\"error\":\"Phone-native Codex library is not packaged\"}");
    }
    char *response = bridge->response(home, request, generation);
    (*env)->ReleaseStringUTFChars(env, codex_home, home);
    (*env)->ReleaseStringUTFChars(env, request_json, request);
    if (response == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Phone-native Codex returned no response\"}");
    }
    jstring result = (*env)->NewStringUTF(env, response);
    bridge->free_string(response);
    return result;
}

static int try_rust_bridge_compile(const char *project_root, char *message, size_t message_size) {
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->compile_project == NULL || bridge->free_string == NULL) {
        return 0;
    }

    char *bridge_message = bridge->compile_project(project_root, "src/main.stasis");
    if (bridge_message == NULL) {
        snprintf(message, message_size, "CompileError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}

static int try_rust_bridge_run_tick(const char *project_root, int touch_x, int touch_y, int touch_active, int screen_w, int screen_h, char *message, size_t message_size) {
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->run_tick == NULL || bridge->free_string == NULL) {
        return 0;
    }

    char *bridge_message = bridge->run_tick(project_root, "src/main.stasis", touch_x, touch_y, touch_active, screen_w, screen_h);
    if (bridge_message == NULL) {
        snprintf(message, message_size, "RunError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}

static int try_rust_bridge_set_i32_global(const char *project_root, const char *path, int value, char *message, size_t message_size) {
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->set_i32_global == NULL || bridge->free_string == NULL) {
        snprintf(message, message_size, "StateError: Rust Android bridge set_i32_global unavailable");
        return 0;
    }

    char *bridge_message = bridge->set_i32_global(project_root, "src/main.stasis", path, value);
    if (bridge_message == NULL) {
        snprintf(message, message_size, "StateError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}

static int try_rust_bridge_run_tests(const char *project_root, char *message, size_t message_size) {
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->run_tests == NULL || bridge->free_string == NULL) {
        return 0;
    }
    char *bridge_message = bridge->run_tests(project_root);
    if (bridge_message == NULL) {
        snprintf(message, message_size, "{\"kind\":\"stasis_test_run\",\"passed\":0,\"failed\":1,\"all_passed\":false,\"error\":\"Rust Android bridge returned null test result\"}");
        return 1;
    }
    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}

static int try_rust_bridge_get_i32_global(const char *project_root, const char *path, char *message, size_t message_size) {
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->get_i32_global == NULL || bridge->free_string == NULL) {
        snprintf(message, message_size, "StateError: Rust Android bridge get_i32_global unavailable");
        return 0;
    }

    char *bridge_message = bridge->get_i32_global(project_root, "src/main.stasis", path);
    if (bridge_message == NULL) {
        snprintf(message, message_size, "StateError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}
static int try_rust_bridge_run_tick_frame(const char *project_root, int touch_x, int touch_y, int touch_active, int screen_w, int screen_h, int32_t *out_i32, uintptr_t out_i32_len, float *out_f32, uintptr_t out_f32_len, uint8_t *out_u8, uintptr_t out_u8_len) {
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->run_tick_frame == NULL) {
        return -1;
    }
    return bridge->run_tick_frame(project_root, "src/main.stasis", touch_x, touch_y, touch_active,
            screen_w, screen_h, out_i32, out_i32_len, out_f32, out_f32_len,
            out_u8, out_u8_len);
}
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeSetRuntimeI32(JNIEnv *env, jclass activity_class, jstring project_root, jstring path, jint value) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    const char *global_path = (*env)->GetStringUTFChars(env, path, NULL);
    if (root == NULL || global_path == NULL) {
        if (root != NULL) {
            (*env)->ReleaseStringUTFChars(env, project_root, root);
        }
        if (global_path != NULL) {
            (*env)->ReleaseStringUTFChars(env, path, global_path);
        }
        return (*env)->NewStringUTF(env, "StateError: unable to read project root or path");
    }
    char message[512];
    try_rust_bridge_set_i32_global(root, global_path, (int)value, message, sizeof(message));
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    (*env)->ReleaseStringUTFChars(env, path, global_path);
    return (*env)->NewStringUTF(env, message);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeGetRuntimeI32(JNIEnv *env, jclass activity_class, jstring project_root, jstring path) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    const char *global_path = (*env)->GetStringUTFChars(env, path, NULL);
    if (root == NULL || global_path == NULL) {
        if (root != NULL) {
            (*env)->ReleaseStringUTFChars(env, project_root, root);
        }
        if (global_path != NULL) {
            (*env)->ReleaseStringUTFChars(env, path, global_path);
        }
        return (*env)->NewStringUTF(env, "StateError: unable to read project root or path");
    }
    char message[512];
    try_rust_bridge_get_i32_global(root, global_path, message, sizeof(message));
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    (*env)->ReleaseStringUTFChars(env, path, global_path);
    return (*env)->NewStringUTF(env, message);
}
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeRunTests(JNIEnv *env, jclass activity_class, jstring project_root) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "{\"kind\":\"stasis_test_run\",\"passed\":0,\"failed\":1,\"all_passed\":false,\"error\":\"unable to read project root\"}");
    }
    char message[8192];
    if (try_rust_bridge_run_tests(root, message, sizeof(message)) == 0) {
        snprintf(message, sizeof(message), "{\"kind\":\"stasis_test_run\",\"passed\":0,\"failed\":1,\"all_passed\":false,\"error\":\"Rust Android bridge test runner unavailable\"}");
    }
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    return (*env)->NewStringUTF(env, message);
}
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeStatus(JNIEnv *env, jclass activity_class) {
    (void)activity_class;
    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "native smoke entry loaded");
    return (*env)->NewStringUTF(env, "Stasis Android native smoke loaded");
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeResolveSpriteAsset(
        JNIEnv *env, jclass activity_class, jstring project_root, jint handle) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"unable to read project root\"}");
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->resolve_sprite_asset == NULL || bridge->free_string == NULL) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"shared sprite resolver unavailable\"}");
    }
    char *message = bridge->resolve_sprite_asset(root, (int)handle);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (message == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"shared sprite resolver returned no result\"}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeResolveCachedText(
        JNIEnv *env, jclass activity_class, jstring project_root, jint handle) {
    (void)activity_class;
#if STASIS_ANDROID_PUBLISHED_AOT && STASIS_RENDER_V1_DIRECT
    (void)project_root;
    if (handle <= 0 || handle > published_text_run_count) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"cached text handle was not loaded\"}");
    }
    PublishedTextResource *run = &published_text_runs[handle - 1];
    PublishedFontResource *font = &published_fonts[run->font - 1];
    const char *asset_path = strstr(font->path, "assets/");
    if (asset_path == NULL) asset_path = font->path;
    char escaped_text[(sizeof(run->text) - 1) * 6 + 1];
    if (!stasis_mobile_json_escape(run->text, escaped_text, sizeof(escaped_text))) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"cached text could not be JSON encoded\"}");
    }
    char response[sizeof(escaped_text) + 1024];
    snprintf(response, sizeof(response),
            "{\"status\":\"ok\",\"handle\":%d,\"font\":%d,\"font_asset\":\"stasis_game/%s\",\"font_size\":%d,\"text\":\"%s\",\"measured_width\":%.3f}",
            (int)handle, (int)run->font, asset_path, (int)font->size,
            escaped_text, (double)run->width);
    return (*env)->NewStringUTF(env, response);
#else
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"unable to read project root\"}");
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->resolve_cached_text == NULL || bridge->free_string == NULL) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"cached text resolver unavailable\"}");
    }
    char *message = bridge->resolve_cached_text(root, (int)handle);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (message == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"cached text resolver returned no result\"}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
#endif
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeResolveFont(
        JNIEnv *env, jclass activity_class, jstring project_root, jint handle) {
    (void)activity_class;
#if STASIS_ANDROID_PUBLISHED_AOT && STASIS_RENDER_V1_DIRECT
    (void)project_root;
    if (handle <= 0 || handle > published_font_count) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"font handle was not loaded\"}");
    }
    PublishedFontResource *font = &published_fonts[handle - 1];
    const char *asset_path = strstr(font->path, "assets/");
    if (asset_path == NULL) asset_path = font->path;
    char response[768];
    snprintf(response, sizeof(response),
            "{\"status\":\"ok\",\"handle\":%d,\"font_asset\":\"stasis_game/%s\",\"font_size\":%d}",
            (int)handle, asset_path, (int)font->size);
    return (*env)->NewStringUTF(env, response);
#else
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"unable to read project root\"}");
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->resolve_font == NULL || bridge->free_string == NULL) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"font resolver unavailable\"}");
    }
    char *message = bridge->resolve_font(root, (int)handle);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (message == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"font resolver returned no result\"}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
#endif
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexBeginDeviceLogin(
        JNIEnv *env, jclass activity_class, jstring codex_home) {
    (void)activity_class;
    return call_codex_bridge(env, codex_home, 1);
}

JNIEXPORT jint JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexInitialize(
        JNIEnv *env, jclass activity_class, jobject context) {
    (void)activity_class;
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL || context == NULL) return -1;
    return bridge->initialize(env, context);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexAccountStatus(
        JNIEnv *env, jclass activity_class, jstring codex_home) {
    (void)activity_class;
    return call_codex_bridge(env, codex_home, 0);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexAccountRateLimits(
        JNIEnv *env, jclass activity_class, jstring codex_home) {
    (void)activity_class;
    return call_codex_rate_limits(env, codex_home);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexResponse(
        JNIEnv *env, jclass activity_class, jstring codex_home, jstring request_json, jlong generation) {
    (void)activity_class;
    return call_codex_response(env, codex_home, request_json, (uint64_t)generation);
}

JNIEXPORT jlong JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexBeginResponse(
        JNIEnv *env, jclass activity_class) {
    (void)env;
    (void)activity_class;
    CodexBridgeApi *bridge = load_codex_bridge_api();
    return bridge == NULL ? 0 : (jlong)bridge->begin_response();
}

JNIEXPORT void JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexCancelResponse(
        JNIEnv *env, jclass activity_class) {
    (void)env;
    (void)activity_class;
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge != NULL) bridge->cancel_response();
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCompileProject(JNIEnv *env, jclass activity_class, jstring project_root) {
    (void)activity_class;
#if STASIS_ANDROID_PUBLISHED_AOT
    (void)project_root;
#if !STASIS_RENDER_V1_DIRECT
    stasis_published_init_globals();
#endif
    return (*env)->NewStringUTF(env, "CompilePlanned: reload=PublishedAot files=0 functions=0 hash=0000000000000000 manifest=published_aot state=compiled status=0");
#else

    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "CompileError: unable to read project root");
    }

    char message[256];
    if (try_rust_bridge_compile(root, message, sizeof(message)) != 0) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "%s", message);
        return (*env)->NewStringUTF(env, message);
    }

    CompileStats stats;
    memset(&stats, 0, sizeof(stats));
    stats.project_hash = FNV_OFFSET_BASIS;
    int result = scan_stasis_files(root, &stats);
    PreviousManifest previous;
    read_previous_compile_manifest(root, &previous);
    const char *reload_classification = classify_reload(&stats, &previous);

    if (result != 0 || stats.error[0] != '\0') {
        snprintf(message, sizeof(message), "%s", stats.error[0] == '\0' ? "CompileError: unknown native check failure" : stats.error);
    } else if (stats.file_count == 0) {
        snprintf(message, sizeof(message), "CompileError: no .stasis files found");
    } else if (!stats.has_main || !stats.has_tick) {
        snprintf(message, sizeof(message), "CompileError: missing lifecycle root main=%d tick=%d", stats.has_main, stats.has_tick);
    } else if (write_compile_manifest(root, &stats, reload_classification) != 0) {
        snprintf(message, sizeof(message), "CompileError: unable to write native compile manifest");
    } else {
        snprintf(
                message,
                sizeof(message),
                "CompilePlanned: reload=%s files=%d functions=%d hash=%016llx manifest=%s state=%s",
                reload_classification,
                stats.file_count,
                stats.function_count,
                (unsigned long long)stats.project_hash,
                STASIS_COMPILE_MANIFEST_RELATIVE_PATH,
                STASIS_RUNTIME_STATE_RELATIVE_PATH);
    }

    (*env)->ReleaseStringUTFChars(env, project_root, root);
    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "%s", message);
    return (*env)->NewStringUTF(env, message);
#endif
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeSourceItems(
        JNIEnv *env, jclass activity_class, jstring project_root) {
    (void)activity_class;
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->source_items == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Rust source item bridge unavailable\"}");
    }
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"unable to read project root\"}");
    }
    char *result = bridge->source_items(root, "src/main.stasis");
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (result == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"source item bridge returned null\"}");
    }
    jstring response = (*env)->NewStringUTF(env, result);
    bridge->free_string(result);
    return response;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeSemanticEdit(
        JNIEnv *env, jclass activity_class, jstring project_root, jstring request_json,
        jboolean dry_run, jboolean validate, jboolean run_tests) {
    (void)activity_class;
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->semantic_edit == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Rust semantic edit bridge unavailable\"}");
    }
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    const char *request = (*env)->GetStringUTFChars(env, request_json, NULL);
    if (root == NULL || request == NULL) {
        if (root != NULL) (*env)->ReleaseStringUTFChars(env, project_root, root);
        if (request != NULL) (*env)->ReleaseStringUTFChars(env, request_json, request);
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"unable to read semantic edit input\"}");
    }
    char *result = bridge->semantic_edit(
            root, "src/main.stasis", request,
            dry_run ? 1 : 0, validate ? 1 : 0, run_tests ? 1 : 0);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    (*env)->ReleaseStringUTFChars(env, request_json, request);
    if (result == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"semantic edit bridge returned null\"}");
    }
    jstring response = (*env)->NewStringUTF(env, result);
    bridge->free_string(result);
    return response;
}
JNIEXPORT jint JNICALL
Java_com_stasislang_workshop_MainActivity_nativeRunFrameInto(JNIEnv *env, jclass activity_class, jstring project_root, jint touch_x, jint touch_y, jint touch_active, jint screen_w, jint screen_h, jobject frame_i32, jobject frame_f32, jobject frame_u8) {
    (void)activity_class;
    int32_t *values_i32 = (int32_t *)(*env)->GetDirectBufferAddress(env, frame_i32);
    float *values_f32 = (float *)(*env)->GetDirectBufferAddress(env, frame_f32);
    uint8_t *values_u8 = (uint8_t *)(*env)->GetDirectBufferAddress(env, frame_u8);
    jlong bytes_i32 = (*env)->GetDirectBufferCapacity(env, frame_i32);
    jlong bytes_f32 = (*env)->GetDirectBufferCapacity(env, frame_f32);
    jlong bytes_u8 = (*env)->GetDirectBufferCapacity(env, frame_u8);
    if (values_i32 == NULL || values_f32 == NULL || values_u8 == NULL
            || bytes_i32 < (jlong)(STASIS_RENDER_I32_COUNT * sizeof(int32_t))
            || bytes_f32 < (jlong)(STASIS_RENDER_F32_COUNT * sizeof(float))
            || bytes_u8 < (jlong)STASIS_RENDER_U8_COUNT) {
        return -1;
    }

#if STASIS_ANDROID_PUBLISHED_AOT
    (void)project_root;
    int status = stasis_published_run_tick_frame_v1(
            (int)touch_x, (int)touch_y, (int)touch_active, (int)screen_w, (int)screen_h,
            values_i32, values_f32, values_u8);
#else
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        values_i32[0] = -1;
        return -1;
    }

    int status = try_rust_bridge_run_tick_frame(
            root, (int)touch_x, (int)touch_y, (int)touch_active, (int)screen_w, (int)screen_h,
            values_i32, STASIS_RENDER_I32_COUNT, values_f32, STASIS_RENDER_F32_COUNT,
            values_u8, STASIS_RENDER_U8_COUNT);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
#endif
    if (status != 0) {
        values_i32[0] = -1;
    } else {
        static int32_t *last_traced_frame;
        if (last_traced_frame != values_i32) {
            uint32_t trace = stasis_render_v1_trace(values_i32, values_f32, values_u8);
            __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG,
                    "Stasis preview gfx_cmd v1 trace=%u flags=%d lines=%d sprites=%d text=%d",
                    trace, values_i32[STASIS_RENDER_I_FLAGS],
                    values_i32[STASIS_RENDER_I_LINE_COUNT],
                    values_i32[STASIS_RENDER_I_SPRITE_COUNT],
                    values_i32[STASIS_RENDER_I_TEXT_COUNT]);
            last_traced_frame = values_i32;
        }
    }
    return (jint)status;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeLastFrameError(
        JNIEnv *env, jclass activity_class) {
    (void)activity_class;
#if STASIS_ANDROID_PUBLISHED_AOT && STASIS_RENDER_V1_DIRECT
    const char *message = published_resource_error[0] == '\0'
            ? "native preview frame failed" : published_resource_error;
    return (*env)->NewStringUTF(env, message);
#else
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->last_frame_error == NULL || bridge->free_string == NULL) {
        return (*env)->NewStringUTF(env, "native preview frame failed");
    }
    char *message = bridge->last_frame_error();
    if (message == NULL) {
        return (*env)->NewStringUTF(env, "native preview frame failed");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
#endif
}
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeRunTick(JNIEnv *env, jclass activity_class, jstring project_root, jint touch_x, jint touch_y, jint touch_active, jint screen_w, jint screen_h) {
    (void)activity_class;

    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "RunError: unable to read project root");
    }

    char message[1024];
    if (try_rust_bridge_run_tick(root, (int)touch_x, (int)touch_y, (int)touch_active, (int)screen_w, (int)screen_h, message, sizeof(message)) != 0) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        if (strncmp(message, "RunError", 8) == 0) {
            __android_log_print(ANDROID_LOG_ERROR, STASIS_ANDROID_LOG_TAG, "%s", message);
        }
        return (*env)->NewStringUTF(env, message);
    }

    int tick_count = 0;
    if (read_runtime_tick_count(root, &tick_count) != 0) {
        snprintf(message, sizeof(message), "RunError: compile project before running tick");
    } else if (write_runtime_tick_count(root, tick_count + 1) != 0) {
        snprintf(message, sizeof(message), "RunError: unable to update runtime state");
    } else {
        snprintf(message, sizeof(message), "RunTick: tick_count=%d state=%s", tick_count + 1, STASIS_RUNTIME_STATE_RELATIVE_PATH);
    }

    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (strncmp(message, "RunError", 8) == 0) {
        __android_log_print(ANDROID_LOG_ERROR, STASIS_ANDROID_LOG_TAG, "%s", message);
    }
    return (*env)->NewStringUTF(env, message);
}
