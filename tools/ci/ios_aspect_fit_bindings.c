#include <SDL3/SDL.h>

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "stasis_mobile_aot_runtime.h"
#include "stasis_render_contract.h"

#define HOST_I32_COUNT 768
#define HOST_F32_COUNT 64
#define POINTER_I32_BASE 548
#define POINTER_F32_BASE 6

static int32_t *host_i32;
static float *host_f32;
static int32_t *gfx_cmd_i32;
static float *gfx_cmd_f32;
static int frame;
static int native_w;
static int native_h;
static int drawable_w;
static int drawable_h;
static int left_safe_x;
static int safe_w;
static int safe_h;

void stasis_set_window_size(int width, int height);
void stasis_get_display_metrics(
    int *logical_w,
    int *logical_h,
    int *out_native_w,
    int *out_native_h,
    int *out_drawable_w,
    int *out_drawable_h,
    int *safe_x,
    int *safe_y,
    int *out_safe_w,
    int *out_safe_h,
    float *content_scale,
    float *raster_scale,
    int *display_generation,
    int *density_generation
);
int stasis_test_get_display_presentation(float *out_f32, int32_t capacity);
int stasis_test_push_display_event(
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
    int safe_h
);
int stasis_test_push_input_event(int kind, int code, float logical_x, float logical_y);

static int32_t hash_path(const char *path) {
    uint32_t hash = UINT32_C(2166136261);
    while (*path != '\0') {
        hash ^= (uint8_t)*path++;
        hash *= UINT32_C(16777619);
    }
    return (int32_t)hash;
}

static int write_receipt(const char *stage) {
    int logical_w = 0;
    int logical_h = 0;
    int current_native_w = 0;
    int current_native_h = 0;
    int current_drawable_w = 0;
    int current_drawable_h = 0;
    int safe_x = 0;
    int safe_y = 0;
    int current_safe_w = 0;
    int current_safe_h = 0;
    int display_generation = 0;
    int density_generation = 0;
    float content_scale = 0.0f;
    float raster_scale = 0.0f;
    float presentation[14] = {0};
    stasis_get_display_metrics(
        &logical_w, &logical_h,
        &current_native_w, &current_native_h,
        &current_drawable_w, &current_drawable_h,
        &safe_x, &safe_y, &current_safe_w, &current_safe_h,
        &content_scale, &raster_scale,
        &display_generation, &density_generation);
    if (!stasis_test_get_display_presentation(presentation, 14)) return 0;

    const char *home = SDL_getenv("HOME");
    if (!home || home[0] == '\0') return 0;
    char path[1024];
    int written = snprintf(
        path, sizeof(path),
        "%s/Documents/stasis-ios-aspect-fit-%s.json", home, stage);
    if (written < 0 || (size_t)written >= sizeof(path)) return 0;
    FILE *file = fopen(path, "wb");
    if (!file) return 0;
    int ok = fprintf(
        file,
        "{\"schema\":\"stasis.ios.aspect_fit.v1\","
        "\"stage\":\"%s\",\"logical\":[%d,%d],"
        "\"native\":[%d,%d],\"drawable\":[%d,%d],"
        "\"safe_logical\":[%d,%d,%d,%d],"
        "\"native_viewport\":[%.3f,%.3f,%.3f,%.3f],"
        "\"drawable_viewport\":[%.3f,%.3f,%.3f,%.3f],"
        "\"safe_drawable\":[%.3f,%.3f,%.3f,%.3f],"
        "\"content_scale\":%.6f,\"raster_scale\":%.6f,"
        "\"display_generation\":%d,\"density_generation\":%d,"
        "\"injected_safe_native\":[%d,0,%d,%d],"
        "\"pointer\":{\"id\":%d,\"down\":%d,\"went_down\":%d,"
        "\"went_up\":%d,\"x\":%.3f,\"y\":%.3f,"
        "\"x_normalized\":%.6f,\"y_normalized\":%.6f}}\n",
        stage, logical_w, logical_h,
        current_native_w, current_native_h,
        current_drawable_w, current_drawable_h,
        safe_x, safe_y, current_safe_w, current_safe_h,
        presentation[0], presentation[1], presentation[2], presentation[3],
        presentation[4], presentation[5], presentation[6], presentation[7],
        presentation[8], presentation[9], presentation[10], presentation[11],
        content_scale, raster_scale, display_generation, density_generation,
        left_safe_x, safe_w, safe_h,
        host_i32 ? host_i32[POINTER_I32_BASE] : 0,
        host_i32 ? host_i32[POINTER_I32_BASE + 1] : 0,
        host_i32 ? host_i32[POINTER_I32_BASE + 2] : 0,
        host_i32 ? host_i32[POINTER_I32_BASE + 3] : 0,
        host_f32 ? host_f32[POINTER_F32_BASE] : 0.0f,
        host_f32 ? host_f32[POINTER_F32_BASE + 1] : 0.0f,
        host_f32 ? host_f32[POINTER_F32_BASE + 4] : 0.0f,
        host_f32 ? host_f32[POINTER_F32_BASE + 5] : 0.0f) > 0;
    ok = fclose(file) == 0 && ok;
    if (ok) SDL_Log("Stasis iOS aspect-fit qualification stage=%s receipt=%s", stage, path);
    return ok;
}

void stasis_aot_bind_runtime_globals(void) {
    host_i32 = stasis_jit_global_i32_array_ptr(
        hash_path("host_i32"), 0, HOST_I32_COUNT);
    host_f32 = stasis_jit_global_f32_array_ptr(
        hash_path("host_f32"), 0, HOST_F32_COUNT);
    gfx_cmd_i32 = stasis_jit_global_i32_array_ptr(
        hash_path("gfx_cmd_i32"), 0, STASIS_RENDER_I32_COUNT);
    gfx_cmd_f32 = stasis_jit_global_f32_array_ptr(
        hash_path("gfx_cmd_f32"), 0, STASIS_RENDER_F32_COUNT);
}

int32_t stasis_mobile_main_entry(void) {
    if (!host_i32 || !host_f32 || !gfx_cmd_i32 || !gfx_cmd_f32) return 70;
    stasis_set_window_size(1600, 720);
    return 0;
}

int32_t stasis_mobile_tick_entry(void) {
    frame++;
    if (frame == 30 && !write_receipt("actual")) return 71;
    if (frame == 60) {
        int logical_w = 0;
        int logical_h = 0;
        int ignored = 0;
        float ignored_scale = 0.0f;
        stasis_get_display_metrics(
            &logical_w, &logical_h, &native_w, &native_h,
            &drawable_w, &drawable_h, &ignored, &ignored, &ignored, &ignored,
            &ignored_scale, &ignored_scale, &ignored, &ignored);
        if (logical_w != 1600 || logical_h != 720 ||
            native_w <= native_h || drawable_w <= drawable_h) {
            return 72;
        }
        left_safe_x = native_w / 14;
        if (left_safe_x < 1) left_safe_x = 1;
        safe_w = native_w - left_safe_x;
        safe_h = native_h - native_h / 18;
        if (!stasis_test_push_display_event(
                1, 1600, 720, native_w, native_h, drawable_w, drawable_h,
                safe_w, safe_h, left_safe_x, 0, safe_w, safe_h)) {
            return 73;
        }
    }
    if (frame == 90) {
        if (!write_receipt("landscape-left") ||
            !stasis_test_push_input_event(3, 723, 160.0f, 72.0f)) {
            return 74;
        }
    }
    if (frame == 91 && !write_receipt("pointer")) return 75;
    if (frame == 150) {
        if (!stasis_test_push_input_event(5, 723, 160.0f, 72.0f)) return 76;
        left_safe_x = 0;
        if (!stasis_test_push_display_event(
                4, 1600, 720, native_w, native_h, drawable_w, drawable_h,
                safe_w, safe_h, 0, 0, safe_w, safe_h)) {
            return 77;
        }
    }
    if (frame == 180 && !write_receipt("landscape-right")) return 78;
    return 0;
}

int32_t stasis_mobile_render_entry(void) {
    memset(gfx_cmd_i32, 0, STASIS_RENDER_I32_COUNT * sizeof(*gfx_cmd_i32));
    gfx_cmd_i32[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_MAGIC;
    gfx_cmd_i32[STASIS_RENDER_I_VERSION] = STASIS_RENDER_VERSION;
    gfx_cmd_i32[STASIS_RENDER_I_FLAGS] =
        STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT;
    gfx_cmd_f32[STASIS_RENDER_F_CLEAR_BASE] = 0.035f;
    gfx_cmd_f32[STASIS_RENDER_F_CLEAR_BASE + 1] = 0.075f;
    gfx_cmd_f32[STASIS_RENDER_F_CLEAR_BASE + 2] = 0.125f;
    gfx_cmd_f32[STASIS_RENDER_F_CLEAR_BASE + 3] = 1.0f;
    return 0;
}

int32_t stasis_replay_state_snapshot_size(void) {
    return 1;
}

int32_t stasis_replay_state_snapshot_write(uint8_t *output, int32_t capacity) {
    if (!output || capacity < 1) return 0;
    output[0] = 0;
    return 1;
}

int32_t stasis_replay_state_snapshot_restore(const uint8_t *input, int32_t bytes) {
    return input && bytes == 1 ? 1 : 0;
}
