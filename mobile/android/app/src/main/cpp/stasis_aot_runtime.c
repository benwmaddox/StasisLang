#include "stasis_aot_runtime.h"

#if STASIS_ANDROID_PUBLISHED_AOT
#include <android/log.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "published_aot_symbols.h"
#include "published_aot_strings.h"

#define STASIS_SCALAR_CAPACITY 512
#define STASIS_ARRAY_CAPACITY 512
#define STASIS_ASSET_HANDLE_CAPACITY 4096

typedef struct I32ScalarSlot { int32_t hash; int32_t value; int used; } I32ScalarSlot;
typedef struct F32ScalarSlot { int32_t hash; float value; int used; } F32ScalarSlot;
typedef struct I32ArraySlot { int32_t collection_hash; int32_t field_hash; int32_t *values; int capacity; int used; } I32ArraySlot;
typedef struct F32ArraySlot { int32_t collection_hash; int32_t field_hash; float *values; int capacity; int used; } F32ArraySlot;

static I32ScalarSlot i32_scalars[STASIS_SCALAR_CAPACITY];
static F32ScalarSlot f32_scalars[STASIS_SCALAR_CAPACITY];
static I32ArraySlot i32_arrays[STASIS_ARRAY_CAPACITY];
static F32ArraySlot f32_arrays[STASIS_ARRAY_CAPACITY];
static int aot_main_ran;
static int32_t frame_count;
static int32_t next_asset_handle = 1;
static int previous_touch_active;
static int32_t sprite_literal_ids[STASIS_ASSET_HANDLE_CAPACITY];
static int32_t text_run_literal_ids[STASIS_ASSET_HANDLE_CAPACITY];
static int32_t font_literal_ids[STASIS_ASSET_HANDLE_CAPACITY];
static int32_t font_sizes[STASIS_ASSET_HANDLE_CAPACITY];

static const char *stasis_literal_for_id(int32_t id) {
    int index;
    for (index = 0; index < STASIS_AOT_STRING_LITERAL_COUNT; index += 1) {
        if (STASIS_AOT_STRING_LITERALS[index].id == id) return STASIS_AOT_STRING_LITERALS[index].value;
    }
    return NULL;
}

static int32_t stasis_hash_path(const char *path) {
    uint32_t hash = 2166136261U;
    const unsigned char *cursor = (const unsigned char *)path;
    while (*cursor != '\0') { hash ^= (uint32_t)*cursor++; hash *= 16777619U; }
    return (int32_t)hash;
}

static I32ScalarSlot *find_i32_scalar(int32_t hash) {
    int index;
    for (index = 0; index < STASIS_SCALAR_CAPACITY; index += 1) {
        if (i32_scalars[index].used && i32_scalars[index].hash == hash) return &i32_scalars[index];
        if (!i32_scalars[index].used) { i32_scalars[index].used = 1; i32_scalars[index].hash = hash; return &i32_scalars[index]; }
    }
    return NULL;
}

static F32ScalarSlot *find_f32_scalar(int32_t hash) {
    int index;
    for (index = 0; index < STASIS_SCALAR_CAPACITY; index += 1) {
        if (f32_scalars[index].used && f32_scalars[index].hash == hash) return &f32_scalars[index];
        if (!f32_scalars[index].used) { f32_scalars[index].used = 1; f32_scalars[index].hash = hash; return &f32_scalars[index]; }
    }
    return NULL;
}

static int next_capacity(int current, int needed) {
    int result = current > 0 ? current : 16;
    while (result < needed && result < 1048576) result *= 2;
    return result < needed ? needed : result;
}

static I32ArraySlot *find_i32_array(int32_t collection_hash, int32_t field_hash, int needed) {
    int index;
    for (index = 0; index < STASIS_ARRAY_CAPACITY; index += 1) {
        I32ArraySlot *slot = &i32_arrays[index];
        if (!slot->used) { slot->used = 1; slot->collection_hash = collection_hash; slot->field_hash = field_hash; }
        if (slot->collection_hash != collection_hash || slot->field_hash != field_hash) continue;
        if (needed > slot->capacity) {
            int capacity = next_capacity(slot->capacity, needed);
            int32_t *values = (int32_t *)realloc(slot->values, (size_t)capacity * sizeof(int32_t));
            if (values == NULL) return NULL;
            memset(values + slot->capacity, 0, (size_t)(capacity - slot->capacity) * sizeof(int32_t));
            slot->values = values; slot->capacity = capacity;
        }
        return slot;
    }
    return NULL;
}

static F32ArraySlot *find_f32_array(int32_t collection_hash, int32_t field_hash, int needed) {
    int index;
    for (index = 0; index < STASIS_ARRAY_CAPACITY; index += 1) {
        F32ArraySlot *slot = &f32_arrays[index];
        if (!slot->used) { slot->used = 1; slot->collection_hash = collection_hash; slot->field_hash = field_hash; }
        if (slot->collection_hash != collection_hash || slot->field_hash != field_hash) continue;
        if (needed > slot->capacity) {
            int capacity = next_capacity(slot->capacity, needed);
            float *values = (float *)realloc(slot->values, (size_t)capacity * sizeof(float));
            if (values == NULL) return NULL;
            memset(values + slot->capacity, 0, (size_t)(capacity - slot->capacity) * sizeof(float));
            slot->values = values; slot->capacity = capacity;
        }
        return slot;
    }
    return NULL;
}

int32_t stasis_jit_global_i32_load(int32_t hash) { I32ScalarSlot *slot = find_i32_scalar(hash); return slot == NULL ? 0 : slot->value; }
void stasis_jit_global_i32_store(int32_t hash, int32_t value) { I32ScalarSlot *slot = find_i32_scalar(hash); if (slot != NULL) slot->value = value; }
float stasis_jit_global_f32_load(int32_t hash) { F32ScalarSlot *slot = find_f32_scalar(hash); return slot == NULL ? 0.0f : slot->value; }
void stasis_jit_global_f32_store(int32_t hash, float value) { F32ScalarSlot *slot = find_f32_scalar(hash); if (slot != NULL) slot->value = value; }
int32_t stasis_jit_global_i32_array_load(int32_t collection_hash, int32_t field_hash, int32_t index) { I32ArraySlot *slot = index < 0 ? NULL : find_i32_array(collection_hash, field_hash, index + 1); return slot == NULL ? 0 : slot->values[index]; }
void stasis_jit_global_i32_array_store(int32_t collection_hash, int32_t field_hash, int32_t index, int32_t value) { I32ArraySlot *slot = index < 0 ? NULL : find_i32_array(collection_hash, field_hash, index + 1); if (slot != NULL) slot->values[index] = value; }
int64_t stasis_jit_global_i32_array_ptr(int32_t collection_hash, int32_t field_hash, int32_t length) { I32ArraySlot *slot = find_i32_array(collection_hash, field_hash, length); return slot == NULL ? 0 : (int64_t)(intptr_t)slot->values; }
float stasis_jit_global_f32_array_load(int32_t collection_hash, int32_t field_hash, int32_t index) { F32ArraySlot *slot = index < 0 ? NULL : find_f32_array(collection_hash, field_hash, index + 1); return slot == NULL ? 0.0f : slot->values[index]; }
void stasis_jit_global_f32_array_store(int32_t collection_hash, int32_t field_hash, int32_t index, float value) { F32ArraySlot *slot = index < 0 ? NULL : find_f32_array(collection_hash, field_hash, index + 1); if (slot != NULL) slot->values[index] = value; }
int64_t stasis_jit_global_f32_array_ptr(int32_t collection_hash, int32_t field_hash, int32_t length) { F32ArraySlot *slot = find_f32_array(collection_hash, field_hash, length); return slot == NULL ? 0 : (int64_t)(intptr_t)slot->values; }
float stasis_jit_sin_fast(float value) { return sinf(value); }
float stasis_jit_cos_fast(float value) { return cosf(value); }
int32_t stasis_jit_gfx_load_sprite(int32_t path_id, int32_t max_w, int32_t max_h) {
    int32_t handle = next_asset_handle++;
    (void)max_w;
    (void)max_h;
    if (handle > 0 && handle < STASIS_ASSET_HANDLE_CAPACITY) sprite_literal_ids[handle] = path_id;
    return handle;
}
int32_t stasis_jit_load_font(int32_t path_id, int32_t size) {
    int32_t handle = next_asset_handle++;
    if (handle > 0 && handle < STASIS_ASSET_HANDLE_CAPACITY) {
        font_literal_ids[handle] = path_id;
        font_sizes[handle] = size;
    }
    return handle;
}
float stasis_jit_measure_text(int32_t font, int32_t text_id) { (void)font; (void)text_id; return 0.0f; }
int32_t stasis_gfx_cache_text(int32_t font, int32_t text_id) {
    int32_t handle = next_asset_handle++;
    (void)font;
    if (handle > 0 && handle < STASIS_ASSET_HANDLE_CAPACITY) text_run_literal_ids[handle] = text_id;
    return handle;
}
float stasis_gfx_measure_text_cached(int32_t run_handle) { (void)run_handle; return 0.0f; }
int64_t stasis_jit_lookup_code_ptr(int32_t fn_id_raw) { (void)fn_id_raw; return 0; }

const char *stasis_published_sprite_path(int32_t handle) {
    if (handle <= 0 || handle >= STASIS_ASSET_HANDLE_CAPACITY) return NULL;
    return stasis_literal_for_id(sprite_literal_ids[handle]);
}

const char *stasis_published_text_for_run(int32_t run_handle) {
    if (run_handle <= 0 || run_handle >= STASIS_ASSET_HANDLE_CAPACITY) return NULL;
    return stasis_literal_for_id(text_run_literal_ids[run_handle]);
}

const char *stasis_published_font_path(int32_t handle) {
    if (handle <= 0 || handle >= STASIS_ASSET_HANDLE_CAPACITY) return NULL;
    return stasis_literal_for_id(font_literal_ids[handle]);
}

int32_t stasis_published_font_size(int32_t handle) {
    if (handle <= 0 || handle >= STASIS_ASSET_HANDLE_CAPACITY) return 14;
    return font_sizes[handle] > 0 ? font_sizes[handle] : 14;
}

void stasis_published_init_globals(void) { }

static void write_host_frame(int touch_x, int touch_y, int touch_active, int screen_w, int screen_h) {
    int active = touch_active != 0;
    int pressed = touch_active == 2;
    int32_t host_i32 = stasis_hash_path("host_i32");
    int32_t host_f32 = stasis_hash_path("host_f32");
    stasis_jit_global_i32_array_store(host_i32, 0, 1, screen_w);
    stasis_jit_global_i32_array_store(host_i32, 0, 2, screen_h);
    stasis_jit_global_i32_array_store(host_i32, 0, 5, screen_w);
    stasis_jit_global_i32_array_store(host_i32, 0, 6, screen_h);
    stasis_jit_global_i32_array_store(host_i32, 0, 7, active ? 1 : 0);
    stasis_jit_global_i32_array_store(host_i32, 0, 10, frame_count);
    stasis_jit_global_i32_array_store(host_i32, 0, 12, screen_w);
    stasis_jit_global_i32_array_store(host_i32, 0, 13, screen_h);
    stasis_jit_global_i32_array_store(host_i32, 0, 14, 1);
    stasis_jit_global_i32_array_store(host_i32, 0, 16, 60);
    stasis_jit_global_i32_array_store(host_i32, 0, 544, 0);
    stasis_jit_global_i32_array_store(host_i32, 0, 545, active ? 1 : 0);
    stasis_jit_global_i32_array_store(host_i32, 0, 546, pressed || (active && !previous_touch_active) ? 1 : 0);
    stasis_jit_global_i32_array_store(host_i32, 0, 547, !active && previous_touch_active ? 1 : 0);
    stasis_jit_global_f32_array_store(host_f32, 0, 0, (float)touch_x);
    stasis_jit_global_f32_array_store(host_f32, 0, 1, (float)touch_y);
    stasis_jit_global_f32_array_store(host_f32, 0, 4, screen_w > 0 ? (float)touch_x / (float)screen_w : 0.0f);
    stasis_jit_global_f32_array_store(host_f32, 0, 5, screen_h > 0 ? (float)touch_y / (float)screen_h : 0.0f);
    previous_touch_active = active ? 1 : 0;
}

static void pack_preview_frame(int32_t *out_values, uintptr_t out_len) {
    int32_t gfx_i32 = stasis_hash_path("gfx_cmd_i32");
    int32_t gfx_f32 = stasis_hash_path("gfx_cmd_f32");
    int32_t sprite_count = stasis_jit_global_i32_array_load(gfx_i32, 0, 4);
    int32_t line_count = stasis_jit_global_i32_array_load(gfx_i32, 0, 3);
    int32_t text_count = stasis_jit_global_i32_array_load(gfx_i32, 0, 7);
    int32_t count;
    int32_t index;
    if (out_values == NULL || out_len < 62) return;
    memset(out_values, 0, (size_t)out_len * sizeof(int32_t));
    if (sprite_count < 0) sprite_count = 0;
    if (line_count < 0) line_count = 0;
    if (text_count < 0) text_count = 0;
    count = line_count + sprite_count + text_count;
    if (frame_count <= 1 || frame_count % 120 == 0) {
        __android_log_print(ANDROID_LOG_INFO, "StasisWorkshop", "published frame=%d lines=%d sprites=%d text=%d assets=%d", frame_count, line_count, sprite_count, text_count, next_asset_handle - 1);
    }
    if (count > STASIS_PUBLISHED_MAX_COMMANDS) count = STASIS_PUBLISHED_MAX_COMMANDS;
    out_values[1] = frame_count;
    out_values[4] = aot_main_ran ? 1 : 0;
    out_values[5] = count;
    for (index = 0; index < count; index += 1) {
        int32_t dest = 6 + index * 7;
        if (index < line_count) {
            int32_t source = 4 + index * 8;
            float x1 = stasis_jit_global_f32_array_load(gfx_f32, 0, source);
            float y1 = stasis_jit_global_f32_array_load(gfx_f32, 0, source + 1);
            float x2 = stasis_jit_global_f32_array_load(gfx_f32, 0, source + 2);
            float y2 = stasis_jit_global_f32_array_load(gfx_f32, 0, source + 3);
            int red = (int)(stasis_jit_global_f32_array_load(gfx_f32, 0, source + 4) * 255.0f);
            int green = (int)(stasis_jit_global_f32_array_load(gfx_f32, 0, source + 5) * 255.0f);
            int blue = (int)(stasis_jit_global_f32_array_load(gfx_f32, 0, source + 6) * 255.0f);
            out_values[dest] = 2;
            out_values[dest + 1] = (int32_t)x1;
            out_values[dest + 2] = (int32_t)y1;
            out_values[dest + 3] = (int32_t)x2;
            out_values[dest + 4] = (int32_t)y2;
            out_values[dest + 5] = ((red & 255) << 16) | ((green & 255) << 8) | (blue & 255);
        } else if (index < line_count + sprite_count) {
            int32_t source = 32 + (index - line_count) * 7;
            int32_t handle = stasis_jit_global_i32_array_load(gfx_i32, 0, source);
            out_values[dest] = 1;
            out_values[dest + 1] = stasis_jit_global_i32_array_load(gfx_i32, 0, source + 1);
            out_values[dest + 2] = stasis_jit_global_i32_array_load(gfx_i32, 0, source + 2);
            out_values[dest + 3] = stasis_jit_global_i32_array_load(gfx_i32, 0, source + 3);
            out_values[dest + 4] = stasis_jit_global_i32_array_load(gfx_i32, 0, source + 4);
            out_values[dest + 5] = stasis_jit_global_i32_array_load(gfx_i32, 0, source + 6);
            out_values[dest + 6] = handle;
        } else {
            int32_t text_index = index - line_count - sprite_count;
            int32_t source_i = 28704 + text_index * 3;
            int32_t source_f = 80004 + text_index * 6;
            int32_t cached_run = -stasis_jit_global_i32_array_load(gfx_i32, 0, source_i + 1);
            int red = (int)(stasis_jit_global_f32_array_load(gfx_f32, 0, source_f + 2) * 255.0f);
            int green = (int)(stasis_jit_global_f32_array_load(gfx_f32, 0, source_f + 3) * 255.0f);
            int blue = (int)(stasis_jit_global_f32_array_load(gfx_f32, 0, source_f + 4) * 255.0f);
            out_values[dest] = 3;
            out_values[dest + 1] = (int32_t)stasis_jit_global_f32_array_load(gfx_f32, 0, source_f);
            out_values[dest + 2] = (int32_t)stasis_jit_global_f32_array_load(gfx_f32, 0, source_f + 1);
            out_values[dest + 3] = stasis_jit_global_i32_array_load(gfx_i32, 0, source_i);
            out_values[dest + 5] = ((red & 255) << 16) | ((green & 255) << 8) | (blue & 255);
            out_values[dest + 6] = cached_run;
        }
    }
}

int stasis_published_run_tick_frame(int touch_x, int touch_y, int touch_active, int screen_w, int screen_h, int32_t *out_values, uintptr_t out_len) {
    write_host_frame(touch_x, touch_y, touch_active, screen_w, screen_h);
    if (!aot_main_ran) { STASIS_AOT_MAIN(); aot_main_ran = 1; }
    STASIS_AOT_TICK();
    STASIS_AOT_RENDER();
    frame_count += 1;
    pack_preview_frame(out_values, out_len);
    return 0;
}
#endif
