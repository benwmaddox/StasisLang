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

static char last_sprite_path[64];
static float last_audio_samples[4];
static int last_audio_frames;

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
int stasis_gfx_load_sprite(const char *path, int max_w, int max_h) {
    if (path != NULL) {
        strncpy(last_sprite_path, path, sizeof(last_sprite_path) - 1);
        last_sprite_path[sizeof(last_sprite_path) - 1] = '\0';
    }
    return path != NULL && max_w > 0 && max_h > 0 ? 1 : 0;
}
void stasis_gfx_release_sprite(int handle) { (void)handle; }
int stasis_gfx_dump_bmp(const char *path) { return path != NULL; }
int stasis_gfx_cache_text(int font, const char *text) { return font + (text != NULL); }
int stasis_gfx_poll_reload(int handle) { return handle; }
float stasis_gfx_measure_text_cached(int handle) { return (float)handle; }
int stasis_load_font(const char *path, int size) { return path != NULL ? size : 0; }
float stasis_measure_text(int font, const char *text) {
    return text != NULL ? (float)font : 0.0f;
}
void stasis_sleep_ms(int ms) { (void)ms; }

int main(void) {
    int32_t external = 4;
    int32_t external_array[3] = {7, 8, 9};
    float external_audio[4] = {0.1f, -0.1f, 0.2f, -0.2f};
    uint8_t external_u8[4] = {1, 2, 3, 4};
    uint8_t dynamic_path[] = "sprite.bmp";
    int32_t *owned;

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
    stasis_jit_global_i32_array_store(22, 0, 1, 258);
    CHECK(external_u8[1] == 2);
    CHECK(stasis_jit_global_i32_array_load(22, 0, 1) == 2);
    stasis_jit_sys_memcpy_u8(22, 2, 22, 0, 2);
    CHECK(external_u8[2] == 1 && external_u8[3] == 2);

    stasis_jit_register_global_u8_array(23, 0, dynamic_path, sizeof(dynamic_path) - 1);
    stasis_jit_collection_i32_store(23, 1, sizeof(dynamic_path) - 1);
    CHECK(stasis_jit_gfx_load_sprite(23, 32, 32) == 1);
    CHECK(strcmp(last_sprite_path, "sprite.bmp") == 0);

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
