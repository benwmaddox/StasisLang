#include "stasis_mobile_aot_runtime.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static int32_t add_two(int32_t left, int32_t right) {
    return left + right;
}

static int32_t hash_text(const char *text) {
    uint32_t hash = 2166136261u;
    const unsigned char *cursor = (const unsigned char *)text;
    while (*cursor != 0) {
        hash ^= (uint32_t)*cursor++;
        hash *= 16777619u;
    }
    return (int32_t)hash;
}

static char last_sprite_path[64];
static char saved_scope[64];
static char saved_key[64];
static int saved_value;
static int has_saved_value;

int stasis_audio_init(int rate, int channels, int latency) {
    return rate > 0 && channels == 2 && latency > 0;
}
void stasis_audio_shutdown(void) {}
int stasis_audio_is_available(void) { return 1; }
int stasis_audio_get_sample_rate(void) { return 48000; }
int stasis_audio_get_channels(void) { return 2; }
int stasis_audio_get_queued_frames(void) { return 0; }
int stasis_audio_get_underruns(void) { return 0; }
int stasis_audio_push_f32_interleaved(const float *samples, int frames) {
    return samples == NULL ? 0 : frames;
}
int stasis_gfx_load_sprite(const char *path, int max_w, int max_h) {
    if (path != NULL) {
        strncpy(last_sprite_path, path, sizeof(last_sprite_path) - 1);
        last_sprite_path[sizeof(last_sprite_path) - 1] = '\0';
    }
    return path != NULL && max_w > 0 && max_h > 0 ? 1 : 0;
}
void stasis_gfx_release_sprite(int handle) { (void)handle; }
int stasis_gfx_dump_bmp(const char *path) { return path != NULL; }
int stasis_gfx_dump_png(const char *path) { return path != NULL; }
int stasis_gfx_cache_text(int font, const char *text) { return font + (text != NULL); }
int stasis_gfx_poll_reload(int handle) { return handle; }
float stasis_gfx_measure_text_cached(int handle) { return (float)handle; }
float stasis_gfx_measure_text_cached_height(int handle) { return (float)handle + 1.0f; }
int stasis_load_font(const char *path, int size) { return path != NULL ? size : 0; }
float stasis_measure_text(int font, const char *text) {
    return text != NULL ? (float)font : 0.0f;
}
void stasis_sleep_ms(int ms) { (void)ms; }
int stasis_storage_load_i32(const char *scope, const char *key, int fallback) {
    if (has_saved_value && scope != NULL && key != NULL &&
        strcmp(scope, saved_scope) == 0 && strcmp(key, saved_key) == 0) {
        return saved_value;
    }
    return fallback;
}
int stasis_storage_save_i32(const char *scope, const char *key, int value) {
    if (scope == NULL || key == NULL) return 0;
    strncpy(saved_scope, scope, sizeof(saved_scope) - 1);
    saved_scope[sizeof(saved_scope) - 1] = '\0';
    strncpy(saved_key, key, sizeof(saved_key) - 1);
    saved_key[sizeof(saved_key) - 1] = '\0';
    saved_value = value;
    has_saved_value = 1;
    return 1;
}

int main(void) {
    int32_t external = 4;
    int32_t external_array[3] = {7, 8, 9};
    int32_t overlapping_i32[5] = {1, 2, 3, 4, 5};
    float overlapping_f32[5] = {1, 2, 3, 4, 5};
    uint8_t external_u8[4] = {1, 2, 3, 4};
    uint8_t dynamic_path[] = "sprite.bmp";
    int32_t sprite_handle[1] = {0};
    int32_t sprite_width[1] = {0};
    int32_t sprite_height[1] = {0};
    int32_t text_font[1] = {0};
    int32_t text_handle[1] = {0};
    float text_width[1] = {0};
    float text_height[1] = {0};
    int32_t *owned;
    char escaped_json[64];
    const char json_controls[] = {'"', '\\', '\b', '\f', '\n', '\r', '\t', 1, 'A', 0};

    CHECK(stasis_mobile_json_escape(json_controls, escaped_json, sizeof(escaped_json)) == 1);
    CHECK(strcmp(escaped_json, "\\\"\\\\\\b\\f\\n\\r\\t\\u0001A") == 0);
    CHECK(stasis_mobile_json_escape("too long", escaped_json, 2) == 0);
    CHECK(escaped_json[0] == '\0');

    stasis_mobile_aot_reset();
    stasis_jit_global_i32_store(10, 42);
    CHECK(stasis_jit_global_i32_load(10) == 42);

    stasis_jit_register_global_i32_ptr(11, &external);
    stasis_jit_global_i32_store(11, 6);
    CHECK(external == 6);

    stasis_jit_register_global_i32_array(20, 0, external_array, 3);
    CHECK(stasis_jit_global_i32_array_load(20, 0, 1) == 8);
    stasis_jit_global_i32_array_store(20, 0, 2, 12);
    CHECK(external_array[2] == 12);

    stasis_jit_register_global_u8_array(22, 0, external_u8, 4);
    CHECK(stasis_jit_global_u8_array_ptr(22, 0, 4) == external_u8);
    stasis_jit_global_i32_array_store(22, 0, 1, 258);
    CHECK(external_u8[1] == 2);
    CHECK(stasis_jit_global_i32_array_load(22, 0, 1) == 2);
    stasis_jit_sys_memcpy_u8(22, 2, 22, 0, 2);
    CHECK(external_u8[2] == 1 && external_u8[3] == 2);

    stasis_jit_register_global_i32_array(24, 0, overlapping_i32, 5);
    stasis_jit_sys_memmove_i32(24, 1, 24, 0, 4);
    CHECK(overlapping_i32[0] == 1 && overlapping_i32[1] == 1 &&
            overlapping_i32[2] == 2 && overlapping_i32[3] == 3 && overlapping_i32[4] == 4);
    stasis_jit_register_global_f32_array(25, 0, overlapping_f32, 5);
    stasis_jit_sys_memmove_f32(25, 1, 25, 0, 4);
    CHECK(overlapping_f32[0] == 1 && overlapping_f32[1] == 1 &&
            overlapping_f32[2] == 2 && overlapping_f32[3] == 3 && overlapping_f32[4] == 4);

    stasis_jit_register_global_u8_array(23, 0, dynamic_path, sizeof(dynamic_path) - 1);
    stasis_jit_collection_i32_store(23, 1, sizeof(dynamic_path) - 1);
    CHECK(stasis_jit_gfx_load_sprite(23, 32, 32) == 1);
    CHECK(strcmp(last_sprite_path, "sprite.bmp") == 0);
    CHECK(stasis_jit_gfx_dump_png(23) == 1);

    stasis_jit_register_global_i32_array(100, hash_text("handle"), sprite_handle, 1);
    stasis_jit_register_global_i32_array(100, hash_text("width"), sprite_width, 1);
    stasis_jit_register_global_i32_array(100, hash_text("height"), sprite_height, 1);
    CHECK(stasis_jit_sprite_load_from(100, 0, 1, 23, 48, 24) == 1);
    CHECK(sprite_handle[0] == 1 && sprite_width[0] == 48 && sprite_height[0] == 24);

    stasis_jit_register_global_i32_array(101, hash_text("font"), text_font, 1);
    stasis_jit_register_global_i32_array(101, hash_text("handle"), text_handle, 1);
    stasis_jit_register_global_f32_array(101, hash_text("width"), text_width, 1);
    stasis_jit_register_global_f32_array(101, hash_text("height"), text_height, 1);
    CHECK(stasis_jit_text_run_load_from(101, 0, 1, 7, 23) == 1);
    CHECK(text_font[0] == 7 && text_handle[0] == 8);
    CHECK(text_width[0] == 8.0f && text_height[0] == 9.0f);

    stasis_jit_upsert_string_literal(40, "sample_game");
    stasis_jit_upsert_string_literal(41, "unlocked_tier");
    CHECK(stasis_jit_storage_load_i32(40, 41, 1) == 1);
    CHECK(stasis_jit_storage_save_i32(40, 41, 4) == 1);
    CHECK(stasis_jit_storage_load_i32(40, 41, 1) == 4);
    CHECK(strcmp(saved_scope, "sample_game") == 0);
    CHECK(strcmp(saved_key, "unlocked_tier") == 0);

    owned = stasis_jit_global_i32_array_ptr(21, 0, 4);
    CHECK(owned != NULL);
    stasis_jit_global_i32_array_store(21, 0, 3, 99);
    CHECK(owned[3] == 99);

    stasis_jit_register_code_ptr(30, (int64_t)(uintptr_t)&add_two);
    CHECK(stasis_jit_call_i32_2(30, 5, 7) == 12);
    CHECK(stasis_jit_call_i32_0(999) == 0);

    stasis_mobile_aot_reset();
    CHECK(stasis_jit_global_i32_load(10) == 0);
    puts("stasis_mobile_aot_runtime_test: ok");
    return 0;
}
