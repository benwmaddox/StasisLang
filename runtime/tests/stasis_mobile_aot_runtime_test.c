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
static float last_audio_samples[4];
static int last_audio_frames;
static int sprite_handle_to_load = 1;
static int released_sprite_handle;
static char last_cached_text[64];
static int released_sprite_count;
static char saved_scope[64];
static char saved_key[64];
static int saved_value;
static int has_saved_value;
static char saved_ascii[64];
static int saved_ascii_length;
static char clipboard_ascii[64] = "GG1-clipboard";
static int profile_start_logs;
static int profile_row_logs;
static int profile_done_logs;
static char profile_row[256];

void stasis_host_log_message(const char *message) {
    if (message == NULL) return;
    if (strncmp(message, "STASIS_PROFILE_START|", 21) == 0) profile_start_logs += 1;
    if (strncmp(message, "STASIS_PROFILE|", 15) == 0) {
        profile_row_logs += 1;
        snprintf(profile_row, sizeof(profile_row), "%s", message);
    }
    if (strncmp(message, "STASIS_PROFILE_DONE|", 20) == 0) profile_done_logs += 1;
}

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
    int index;
    if (samples == NULL || frames <= 0) return 0;
    last_audio_frames = frames;
    for (index = 0; index < 4 && index < frames * 2; index += 1) {
        last_audio_samples[index] = samples[index];
    }
    return frames;
}
int stasis_audio_load_wav(const char *path) { return path ? 1 : 0; }
void stasis_audio_release(int asset_handle) { (void)asset_handle; }
int stasis_audio_play(int asset_handle, int loop, float volume, float pan) {
    return asset_handle + loop + (int)volume + (int)pan;
}
void stasis_audio_stop(int voice_handle) { (void)voice_handle; }
int stasis_audio_voice_is_playing(int voice_handle) { return voice_handle > 0; }
void stasis_audio_voice_set_paused(int voice_handle, int paused) { (void)voice_handle; (void)paused; }
void stasis_audio_voice_set_volume_pan(int voice_handle, float volume, float pan) {
    (void)voice_handle; (void)volume; (void)pan;
}
int stasis_audio_load_music(const char *path) { return path ? 2 : 0; }
int stasis_audio_load_effect(const char *path) { return path ? 3 : 0; }
int stasis_audio_play_music(int asset_handle, int loop, float volume) {
    return asset_handle + loop + (int)volume;
}
void stasis_audio_stop_music(int asset_handle) { (void)asset_handle; }
void stasis_audio_pause_music(int asset_handle, int paused) { (void)asset_handle; (void)paused; }
void stasis_audio_set_music_volume(int asset_handle, float volume) {
    (void)asset_handle; (void)volume;
}
int stasis_audio_play_effect(int asset_handle, float volume) {
    return asset_handle + (int)volume;
}
int stasis_gfx_load_sprite(const char *path, int max_w, int max_h) {
    if (path != NULL) {
        strncpy(last_sprite_path, path, sizeof(last_sprite_path) - 1);
        last_sprite_path[sizeof(last_sprite_path) - 1] = '\0';
    }
    return path != NULL && max_w > 0 && max_h > 0 ? sprite_handle_to_load : 0;
}
int stasis_asset_request_sprite(const char *path, int max_w, int max_h) { return path && max_w && max_h ? 31 : 0; }
int stasis_asset_request_audio(const char *path) { return path ? 32 : 0; }
int stasis_asset_task_poll(int task) { return task > 0 ? 3 : 0; }
int stasis_asset_task_take_handle(int task) { return task > 0 ? 33 : 0; }
void stasis_asset_task_cancel(int task) { (void)task; }
void stasis_gfx_release_sprite(int handle) {
    released_sprite_handle = handle;
    released_sprite_count += 1;
}
int stasis_gfx_dump_bmp(const char *path) { return path != NULL; }
int stasis_gfx_dump_png(const char *path) { return path != NULL; }
int stasis_gfx_cache_text(int font, const char *text) {
    if (text != NULL) {
        strncpy(last_cached_text, text, sizeof(last_cached_text) - 1);
        last_cached_text[sizeof(last_cached_text) - 1] = '\0';
    }
    return font + (text != NULL);
}
int stasis_gfx_replace_text(int handle, int font, const char *text) { return handle > 0 ? handle : font + (text != NULL); }
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
int stasis_storage_load_ascii(const char *scope, const char *key, char *out, int capacity) {
    int count = saved_ascii_length;
    if (scope == NULL || key == NULL || count > capacity) return -1;
    memcpy(out, saved_ascii, (size_t)count);
    return count;
}
int stasis_storage_save_ascii(const char *scope, const char *key, const char *value, int length) {
    if (scope == NULL || key == NULL || value == NULL || length < 0 || length > 63) return 0;
    memcpy(saved_ascii, value, (size_t)length);
    saved_ascii_length = length;
    return 1;
}
int stasis_clipboard_load_ascii(char *out, int capacity) {
    int count = (int)strlen(clipboard_ascii);
    if (out == NULL || count > capacity) return -1;
    memcpy(out, clipboard_ascii, (size_t)count);
    return count;
}
int stasis_clipboard_save_ascii(const char *value, int length) {
    if (value == NULL || length < 0 || length > 63) return 0;
    memcpy(clipboard_ascii, value, (size_t)length);
    clipboard_ascii[length] = '\0';
    return 1;
}

int main(void) {
    enum { STRESS_LITERAL_COUNT = 640 };
    int32_t external = 4;
    int32_t external_array[3] = {7, 8, 9};
    float external_audio[4] = {0.1f, -0.1f, 0.2f, -0.2f};
    int32_t overlapping_i32[5] = {1, 2, 3, 4, 5};
    float overlapping_f32[5] = {1, 2, 3, 4, 5};
    uint8_t external_u8[4] = {1, 2, 3, 4};
    uint8_t aot_text_out[16] = {0};
    uint8_t dynamic_path[] = "sprite.bmp";
    uint8_t dynamic_price[] = {0xe2, 0x82, 0xac, '2', '.', '9', '9'};
    uint8_t malformed_utf8[] = {0xc3, 0x28};
    uint8_t ascii_value[] = "GG1-test";
    uint8_t ascii_out[32] = {0};
    uint8_t platform_key[] = "power_up";
    int32_t platform_fields[5] = {0};
    uint8_t platform_text[32] = {0};
    uint8_t literal_out[8] = {0};
    uint8_t utf8_out[16] = {0};
    uint8_t raw_out[4] = {0};
    int32_t sprite_handle[1] = {0};
    int32_t sprite_width[1] = {0};
    int32_t sprite_height[1] = {0};
    int32_t text_font[1] = {0};
    int32_t text_handle[1] = {0};
    float text_width[1] = {0};
    float text_height[1] = {0};
    int32_t *owned;
    char stress_literals[STRESS_LITERAL_COUNT][32];
    int stress_index;
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

    stasis_jit_register_global_f32_array(24, 0, external_audio, 4);
    CHECK(stasis_jit_global_f32_array_ptr(24, 0, 4) == external_audio);
    CHECK(stasis_jit_audio_push_f32_interleaved(24, 2) == 2);
    CHECK(last_audio_frames == 2);
    CHECK(last_audio_samples[0] == 0.1f && last_audio_samples[1] == -0.1f);
    CHECK(last_audio_samples[2] == 0.2f && last_audio_samples[3] == -0.2f);

    stasis_jit_register_global_u8_array(22, 0, external_u8, 4);
    CHECK(stasis_jit_global_u8_array_ptr(22, 0, 4) == external_u8);
    stasis_jit_global_i32_array_store(22, 0, 1, 258);
    CHECK(external_u8[1] == 2);
    CHECK(stasis_jit_global_i32_array_load(22, 0, 1) == 2);
    stasis_jit_sys_memcpy_u8(22, 2, 22, 0, 2);
    CHECK(external_u8[2] == 1 && external_u8[3] == 2);
    stasis_jit_upsert_string_literal(22, "literal-must-not-win");
    stasis_jit_register_global_u8_array(54, 0, raw_out, sizeof(raw_out));
    stasis_jit_sys_memcpy_u8(54, 0, 22, 0, 4);
    CHECK(memcmp(raw_out, external_u8, sizeof(raw_out)) == 0);

    stasis_jit_register_global_u8_array(26, 0, aot_text_out, sizeof(aot_text_out));
    stasis_jit_upsert_string_literal(27, "AOT text");
    CHECK(stasis_jit_collection_i32_load(27, 1) == 8);
    CHECK(stasis_jit_collection_i32_load(27, 2) == 8);
    CHECK(stasis_jit_collection_i32_load(27, 3) == 8);
    stasis_jit_sys_memcpy_u8(26, 0, 27, 0, 8);
    CHECK(memcmp(aot_text_out, "AOT text", 8) == 0);

    stasis_jit_register_global_i32_array(24, 0, overlapping_i32, 5);
    stasis_jit_upsert_string_literal(24, "i32-array-must-win");
    memset(raw_out, 0, sizeof(raw_out));
    stasis_jit_sys_memcpy_u8(54, 0, 24, 0, 4);
    CHECK(memcmp(raw_out, "\1\2\3\4", sizeof(raw_out)) == 0);
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

    stasis_jit_register_global_i32_array(100, hash_text("sprite_ref"), sprite_handle, 1);
    stasis_jit_register_global_i32_array(100, hash_text("width"), sprite_width, 1);
    stasis_jit_register_global_i32_array(100, hash_text("height"), sprite_height, 1);
    CHECK(stasis_jit_sprite_load_from(100, 0, 1, 23, 48, 24) == 1);
    CHECK(sprite_handle[0] == 1 && sprite_width[0] == 48 && sprite_height[0] == 24);
    sprite_handle_to_load = 1;
    CHECK(stasis_jit_sprite_load_from(100, 0, 1, 23, 56, 28) == 1);
    CHECK(sprite_handle[0] == 1 && sprite_width[0] == 56 && sprite_height[0] == 28);
    CHECK(released_sprite_count == 1 && released_sprite_handle == 1);
    sprite_handle_to_load = -1520461853;
    CHECK(stasis_jit_sprite_load_from(100, 0, 1, 23, 64, 32) == 1);
    CHECK(sprite_handle[0] == -1520461853 && sprite_width[0] == 64 && sprite_height[0] == 32);
    CHECK(released_sprite_count == 2 && released_sprite_handle == 1);

    stasis_jit_register_global_i32_array(101, hash_text("font"), text_font, 1);
    stasis_jit_register_global_i32_array(101, hash_text("handle"), text_handle, 1);
    stasis_jit_register_global_f32_array(101, hash_text("width"), text_width, 1);
    stasis_jit_register_global_f32_array(101, hash_text("height"), text_height, 1);
    CHECK(stasis_jit_text_run_load_from(101, 0, 1, 7, 23) == 1);
    CHECK(text_font[0] == 7 && text_handle[0] == 8);
    CHECK(text_width[0] == 8.0f && text_height[0] == 9.0f);
    CHECK(stasis_jit_text_run_replace_from(101, 0, 1, 9, 23) == 1);
    CHECK(text_font[0] == 9 && text_handle[0] == 8);
    CHECK(text_width[0] == 8.0f && text_height[0] == 9.0f);
    CHECK(stasis_jit_text_run_replace_from(101, 0, 1, 0, 23) == 0);
    CHECK(text_font[0] == 9 && text_handle[0] == 8);

    stasis_jit_register_global_u8_array(26, 0, dynamic_price, sizeof(dynamic_price));
    stasis_jit_collection_i32_store(26, 1, sizeof(dynamic_price));
    CHECK(stasis_jit_text_run_load_from(101, 0, 1, 7, 26) == 1);
    CHECK(memcmp(last_cached_text, dynamic_price, sizeof(dynamic_price)) == 0);
    CHECK(last_cached_text[sizeof(dynamic_price)] == '\0');

    stasis_jit_register_global_u8_array(27, 0, malformed_utf8, sizeof(malformed_utf8));
    stasis_jit_collection_i32_store(27, 1, sizeof(malformed_utf8));
    CHECK(stasis_jit_text_run_load_from(101, 0, 1, 7, 27) == 0);
    CHECK(text_font[0] == 7 && text_handle[0] == 8);

    stasis_jit_upsert_string_literal(40, "sample_game");
    stasis_jit_upsert_string_literal(41, "unlocked_tier");
    CHECK(stasis_jit_collection_i32_load(40, 1) == 11);
    CHECK(stasis_jit_collection_i32_load(40, 2) == 11);
    CHECK(stasis_jit_collection_i32_load(40, 3) == 11);
    {
        const char utf8_literal[] = {
            'h', (char)0xc3, (char)0xa9, 'l', 'l', 'o', ' ',
            (char)0xf0, (char)0x9f, (char)0x8c, (char)0x8d, 0};
        stasis_jit_upsert_string_literal(45, utf8_literal);
        CHECK(stasis_jit_collection_i32_load(45, 1) == 11);
        CHECK(stasis_jit_collection_i32_load(45, 2) == 11);
        CHECK(stasis_jit_collection_i32_load(45, 3) == 7);
        stasis_jit_register_global_u8_array(53, 0, utf8_out, sizeof(utf8_out));
        stasis_jit_sys_memcpy_u8(53, 0, 45, 0, 11);
        CHECK(memcmp(utf8_out, utf8_literal, 11) == 0);
    }
    stasis_jit_upsert_string_literal(50, "field");
    stasis_jit_register_global_u8_array(52, 0, literal_out, sizeof(literal_out));
    stasis_jit_sys_memcpy_u8(52, 1, 50, 1, 3);
    CHECK(memcmp(literal_out, "\0iel\0\0\0\0", sizeof(literal_out)) == 0);
    stasis_jit_sys_memmove_u8(52, 0, 50, 0, 5);
    CHECK(memcmp(literal_out, "field", 5) == 0);
    memset(literal_out, 0x7f, sizeof(literal_out));
    stasis_jit_sys_memcpy_u8(52, 0, 50, 99, 2);
    CHECK(literal_out[0] == 0 && literal_out[1] == 0);
    {
        const char truncated_literal[] = {'x', (char)0xc3, 0};
        stasis_jit_upsert_string_literal(46, truncated_literal);
        CHECK(stasis_jit_collection_i32_load(46, 1) == 2);
        CHECK(stasis_jit_collection_i32_load(46, 3) == 2);
    }
    CHECK(stasis_jit_collection_i32_load(404, 1) == 0);
    CHECK(stasis_jit_collection_i32_load(40, 0) == 0);
    CHECK(stasis_jit_collection_i32_load(40, 4) == 0);
    stasis_jit_collection_i32_store(45, 1, 23);
    CHECK(stasis_jit_collection_i32_load(45, 1) == 23);
    CHECK(stasis_jit_storage_load_i32(40, 41, 1) == 1);
    CHECK(stasis_jit_storage_save_i32(40, 41, 4) == 1);
    CHECK(stasis_jit_storage_load_i32(40, 41, 1) == 4);
    CHECK(strcmp(saved_scope, "sample_game") == 0);
    CHECK(strcmp(saved_key, "unlocked_tier") == 0);
    stasis_jit_register_global_u8_array(42, 0, ascii_value, sizeof(ascii_value) - 1);
    stasis_jit_collection_i32_store(42, 1, sizeof(ascii_value) - 1);
    stasis_jit_register_global_u8_array(43, 0, ascii_out, sizeof(ascii_out));
    CHECK(stasis_jit_storage_save_ascii(40, 41, 42, sizeof(ascii_value) - 1) == 1);
    CHECK(stasis_jit_storage_load_ascii(40, 41, 43, sizeof(ascii_out)) == 8);
    CHECK(memcmp(ascii_out, "GG1-test", 8) == 0);
    CHECK(stasis_jit_clipboard_save_ascii(42, sizeof(ascii_value) - 1) == 1);
    memset(ascii_out, 0, sizeof(ascii_out));
    CHECK(stasis_jit_clipboard_load_ascii(43, sizeof(ascii_out)) == 8);
    CHECK(memcmp(ascii_out, "GG1-test", 8) == 0);

    stasis_jit_register_global_u8_array(44, 0, platform_key, sizeof(platform_key) - 1);
    stasis_jit_collection_i32_store(44, 1, sizeof(platform_key) - 1);
    stasis_jit_register_global_i32_array(45, 0, platform_fields, 5);
    stasis_jit_register_global_u8_array(46, 0, platform_text, sizeof(platform_text));
    CHECK(stasis_jit_platform_service_submit(1, 1, 77, 44, 8) == 1);
    CHECK(stasis_jit_platform_service_poll(45, 5, 46, sizeof(platform_text)) == 1);
    CHECK(platform_fields[0] == 1 && platform_fields[1] == 1);
    CHECK(platform_fields[2] == 77 && platform_fields[3] == 4);
    CHECK(platform_fields[4] == 0);
    CHECK(stasis_jit_collection_i32_load(46, 1) == 0);
    CHECK(stasis_jit_collection_i32_load(46, 3) == 0);
    CHECK(stasis_jit_platform_service_poll(45, 5, 46, sizeof(platform_text)) == 0);

    for (stress_index = 0; stress_index < STRESS_LITERAL_COUNT; stress_index += 1) {
        snprintf(stress_literals[stress_index], sizeof(stress_literals[stress_index]),
            "asset/path/%d.ttf", stress_index);
        stasis_jit_upsert_string_literal(1000 + stress_index, stress_literals[stress_index]);
    }
    CHECK(stasis_jit_load_font(1000 + STRESS_LITERAL_COUNT - 1, 19) == 19);
    stasis_jit_clear_string_literal_table();
    CHECK(stasis_jit_load_font(1000 + STRESS_LITERAL_COUNT - 1, 19) == 0);
    stasis_jit_upsert_string_literal(2000, "asset/path/rebound.ttf");
    CHECK(stasis_jit_load_font(2000, 23) == 23);

    owned = stasis_jit_global_i32_array_ptr(21, 0, 4);
    CHECK(owned != NULL);
    stasis_jit_global_i32_array_store(21, 0, 3, 99);
    CHECK(owned[3] == 99);

    stasis_jit_register_code_ptr(30, (int64_t)(uintptr_t)&add_two);
    CHECK(stasis_jit_call_i32_2(30, 5, 7) == 12);
    CHECK(stasis_jit_call_i32_0(999) == 0);

    stasis_jit_profile_register_function(77, "render");
    stasis_jit_profile_configure(0, 1);
    stasis_jit_profile_frame_begin();
    stasis_jit_profile_frame_enter(77);
    stasis_jit_profile_frame_leave(77);
    stasis_jit_profile_frame_end();
    CHECK(profile_start_logs == 1);
    CHECK(profile_row_logs == 1);
    CHECK(strncmp(profile_row, "STASIS_PROFILE|render|1|", 24) == 0);
    CHECK(profile_done_logs == 1);

    stasis_mobile_aot_reset();
    CHECK(stasis_jit_global_i32_load(10) == 0);
    puts("stasis_mobile_aot_runtime_test: ok");
    return 0;
}
