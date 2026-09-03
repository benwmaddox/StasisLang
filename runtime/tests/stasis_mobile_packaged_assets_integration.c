#include "stasis_audio_assets.h"
#include "stasis_mobile_aot_runtime.h"
#include "stasis_render_contract.h"

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "IT-015 check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

extern void stasis_aot_bind_runtime_globals(void);
extern int32_t stasis_mobile_main_entry(void);
extern int32_t stasis_mobile_tick_entry(void);
extern int32_t stasis_mobile_render_entry(void);
extern int stasis_set_asset_root(const char *path);
extern int stasis_init_window(int width, int height, const char *title);
extern void stasis_gfx_submit_u8(int32_t *i32s, const float *f32s, const uint8_t *u8s);
extern int stasis_load_font(const char *path, int size);
extern int stasis_audio_load_wav(const char *path);
extern int stasis_audio_load_music(const char *path);
extern void stasis_sleep_ms(int ms);
extern int stasis_host_copy_runtime_error(char *output, size_t output_size);

/* The production function is intentionally not part of the shared-library ABI yet.
 * The generated bridge references it even when the fixture does not call it. */
float stasis_gfx_measure_text_cached_height(int handle) {
    (void)handle;
    return 0.0f;
}

static int32_t hash_path(const char *text) {
    uint32_t hash = 2166136261u;
    while (*text != '\0') {
        hash ^= (uint8_t)*text++;
        hash *= 16777619u;
    }
    return (int32_t)hash;
}

static int near(float actual, float expected) {
    return fabsf(actual - expected) < 0.001f;
}

static void join_path(char *out, size_t capacity, const char *root, const char *relative) {
    int written = snprintf(out, capacity, "%s/%s", root, relative);
    CHECK(written > 0 && (size_t)written < capacity);
}

static uint32_t it015_expected_frame_trace(
    int32_t sprite_handle,
    int32_t font_handle,
    int32_t cached_handle
) {
    int32_t *expected_i32 = calloc(
        (size_t)STASIS_RENDER_I32_COUNT, sizeof(*expected_i32));
    float *expected_f32 = calloc(
        (size_t)STASIS_RENDER_F32_COUNT, sizeof(*expected_f32));
    uint8_t *expected_u8 = calloc(
        (size_t)STASIS_RENDER_U8_COUNT, sizeof(*expected_u8));
    if (expected_i32 == NULL || expected_f32 == NULL || expected_u8 == NULL) {
        fprintf(stderr, "IT-015 could not allocate semantic expected frame\n");
        free(expected_i32);
        free(expected_f32);
        free(expected_u8);
        exit(1);
    }

    expected_i32[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_MAGIC;
    expected_i32[STASIS_RENDER_I_VERSION] = STASIS_RENDER_VERSION;
    expected_i32[STASIS_RENDER_I_FLAGS] =
        STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT;
    expected_i32[STASIS_RENDER_I_SPRITE_COUNT] = 1;
    expected_i32[STASIS_RENDER_I_SPRITE_RUN_COUNT] = 1;
    expected_i32[STASIS_RENDER_I_TEXT_COUNT] = 2;
    expected_i32[STASIS_RENDER_I_TEXT_BYTES_USED] = 7;
    expected_i32[STASIS_RENDER_I_ORDER_COUNT] = 3;

    expected_f32[STASIS_RENDER_F_CLEAR_BASE + 0] = 0.04f;
    expected_f32[STASIS_RENDER_F_CLEAR_BASE + 1] = 0.07f;
    expected_f32[STASIS_RENDER_F_CLEAR_BASE + 2] = 0.12f;
    expected_f32[STASIS_RENDER_F_CLEAR_BASE + 3] = 1.0f;

    const int32_t sprite_i32_base = STASIS_RENDER_I_SPRITE_BASE;
    const int32_t sprite_f32_base = STASIS_RENDER_F_SPRITE_BASE;
    const int32_t sprite_run_i32_base = STASIS_RENDER_I_SPRITE_RUN_BASE;
    expected_i32[sprite_i32_base + 0] = sprite_handle;
    expected_i32[sprite_i32_base + 1] = -1;
    expected_i32[sprite_i32_base + 2] = 0;
    expected_i32[sprite_run_i32_base + 0] = 0;
    expected_i32[sprite_run_i32_base + 1] = 1;
    expected_i32[sprite_run_i32_base + 2] = STASIS_RENDER_SPRITE_CLIP_ORDERED;
    expected_f32[sprite_f32_base + 0] = 52.0f;
    expected_f32[sprite_f32_base + 1] = 28.0f;
    expected_f32[sprite_f32_base + 2] = 64.0f;
    expected_f32[sprite_f32_base + 3] = 64.0f;
    expected_f32[sprite_f32_base + 4] = 0.0f;
    expected_f32[sprite_f32_base + 5] = 0.0f;
    expected_f32[sprite_f32_base + 6] = 0.0f;
    expected_f32[sprite_f32_base + 7] = 0.0f;
    expected_f32[sprite_f32_base + 8] = 32.0f;
    expected_f32[sprite_f32_base + 9] = 32.0f;
    expected_f32[sprite_f32_base + 10] = 1.0f;
    expected_f32[sprite_f32_base + 11] = 1.0f;
    expected_f32[sprite_f32_base + 12] = 0.0f;

    const int32_t direct_text_i32_base = STASIS_RENDER_I_TEXT_BASE;
    const int32_t cached_text_i32_base =
        STASIS_RENDER_I_TEXT_BASE + STASIS_RENDER_TEXT_I32_STRIDE;
    const int32_t direct_text_f32_base = STASIS_RENDER_F_TEXT_BASE;
    const int32_t cached_text_f32_base =
        STASIS_RENDER_F_TEXT_BASE + STASIS_RENDER_TEXT_F32_STRIDE;
    expected_i32[direct_text_i32_base + 0] = font_handle;
    expected_i32[direct_text_i32_base + 1] = 0;
    expected_i32[direct_text_i32_base + 2] = 6;
    expected_f32[direct_text_f32_base + 0] = 30.0f;
    expected_f32[direct_text_f32_base + 1] = 112.0f;
    expected_f32[direct_text_f32_base + 2] = 1.0f;
    expected_f32[direct_text_f32_base + 3] = 0.8f;
    expected_f32[direct_text_f32_base + 4] = 0.1f;
    expected_f32[direct_text_f32_base + 5] = 1.0f;
    expected_i32[cached_text_i32_base + 0] = font_handle;
    expected_i32[cached_text_i32_base + 1] = -cached_handle;
    expected_f32[cached_text_f32_base + 0] = 175.0f;
    expected_f32[cached_text_f32_base + 1] = 112.0f;
    expected_f32[cached_text_f32_base + 2] = 0.1f;
    expected_f32[cached_text_f32_base + 3] = 0.9f;
    expected_f32[cached_text_f32_base + 4] = 1.0f;
    expected_f32[cached_text_f32_base + 5] = 1.0f;
    memcpy(expected_u8, "BUNDLE", 6);

    expected_i32[STASIS_RENDER_I_ORDER_BASE + 0] =
        STASIS_RENDER_ORDER_SPRITE * STASIS_RENDER_ORDER_KIND_SCALE;
    expected_i32[STASIS_RENDER_I_ORDER_BASE + 1] =
        STASIS_RENDER_ORDER_TEXT * STASIS_RENDER_ORDER_KIND_SCALE;
    expected_i32[STASIS_RENDER_I_ORDER_BASE + 2] =
        STASIS_RENDER_ORDER_TEXT * STASIS_RENDER_ORDER_KIND_SCALE + 1;

    const StasisRenderValidation validation =
        stasis_render_validate(expected_i32, expected_f32);
    if (validation != STASIS_RENDER_VALID) {
        fprintf(
            stderr,
            "IT-015 semantic expected frame is invalid: %s\n",
            stasis_render_validation_name(validation));
        free(expected_i32);
        free(expected_f32);
        free(expected_u8);
        exit(1);
    }
    const uint32_t trace =
        stasis_render_trace(expected_i32, expected_f32, expected_u8);
    free(expected_i32);
    free(expected_f32);
    free(expected_u8);
    return trace;
}

int main(int argc, char **argv) {
    int32_t *gfx_i32;
    float *gfx_f32;
    uint8_t *gfx_u8;
    char effect_path[1024];
    char music_path[1024];
    char diagnostic[512];
    StasisAudioAssetStore store;
    StasisAudioAssetStore repeat_store;
    float output[8];
    float music_output[2048];
    float repeat_output[2048];

    CHECK(argc == 2);
    CHECK(stasis_set_asset_root(argv[1]) == 1);
    CHECK(stasis_init_window(320, 180, "IT-015 mobile packaged assets") == 1);
    stasis_aot_bind_runtime_globals();
    CHECK(stasis_mobile_main_entry() == 0);
    for (int tick = 0; tick < 1000; tick++) {
        CHECK(stasis_mobile_tick_entry() == 0);
        if (stasis_jit_global_i32_load(hash_path("seam_audio_event_order")) == 12) {
            break;
        }
        stasis_sleep_ms(1);
    }
    CHECK(stasis_jit_global_i32_load(hash_path("seam_audio_event_order")) == 12);
    CHECK(stasis_jit_global_i32_load(hash_path("seam_music_handle")) > 0);
    CHECK(stasis_jit_global_i32_load(hash_path("seam_effect_handle")) > 0);
    CHECK(stasis_jit_global_i32_load(hash_path("seam_music_played")) == 1);
    CHECK(stasis_jit_global_i32_load(hash_path("seam_effect_played")) == 1);
    CHECK(stasis_mobile_render_entry() == 0);

    gfx_i32 = stasis_jit_global_i32_array_ptr(hash_path("gfx_cmd_i32"), 0, 67888);
    gfx_f32 = stasis_jit_global_f32_array_ptr(hash_path("gfx_cmd_f32"), 0, 146564);
    gfx_u8 = stasis_jit_global_u8_array_ptr(hash_path("gfx_cmd_u8"), 0, 65536);
    CHECK(gfx_i32 != NULL && gfx_f32 != NULL && gfx_u8 != NULL);
    CHECK(gfx_i32[4] == 1 && gfx_i32[7] == 2 && gfx_i32[22] == 3);
    CHECK(memcmp(gfx_u8, "BUNDLE", 6) == 0);
    const int32_t sprite_handle =
        stasis_jit_global_i32_load(hash_path("seam_sprite_handle"));
    const int32_t font_handle =
        stasis_jit_global_i32_load(hash_path("seam_font_handle"));
    const int32_t cached_handle =
        stasis_jit_global_i32_load(hash_path("seam_cached_handle"));
    CHECK(sprite_handle > 0 && font_handle > 0 && cached_handle > 0);
    const uint32_t expected_trace =
        it015_expected_frame_trace(sprite_handle, font_handle, cached_handle);
    const uint32_t actual_trace = stasis_render_trace(gfx_i32, gfx_f32, gfx_u8);
    if (actual_trace != expected_trace) {
        fprintf(
            stderr,
            "IT-015 semantic trace mismatch: expected=%u actual=%u\n",
            expected_trace,
            actual_trace);
    }
    CHECK(actual_trace == expected_trace);
    stasis_gfx_submit_u8(gfx_i32, gfx_f32, gfx_u8);

    memset(&store, 0, sizeof(store));
    stasis_audio_assets_reset(&store);
    join_path(effect_path, sizeof(effect_path), argv[1], "assets/effect.wav");
    join_path(music_path, sizeof(music_path), argv[1], "assets/music.mp3");
    int offline_asset = stasis_audio_assets_load_wav(&store, effect_path);
    CHECK(offline_asset > 0);
    int offline_voice = stasis_audio_assets_play(&store, offline_asset, 0, 0.5f, 0.0f);
    CHECK(offline_voice > 0);
    memset(output, 0, sizeof(output));
    stasis_audio_assets_mix(&store, output, 4, 48000);
    CHECK(near(output[2], 0.125f) && near(output[3], 0.125f));
    CHECK(stasis_audio_assets_voice_is_playing(&store, offline_voice) == 1);
    stasis_audio_assets_stop_voice(&store, offline_voice);

    int offline_music = stasis_audio_assets_load(&store, music_path);
    CHECK(offline_music > 0);
    int offline_music_voice = stasis_audio_assets_play(&store, offline_music, 1, 0.2f, 0.0f);
    CHECK(offline_music_voice > 0);
    memset(music_output, 0, sizeof(music_output));
    stasis_audio_assets_mix(&store, music_output, 1024, 48000);
    float music_energy = 0.0f;
    for (size_t index = 0; index < sizeof(music_output) / sizeof(music_output[0]); index++) {
        music_energy += fabsf(music_output[index]);
    }
    CHECK(music_energy > 0.001f);
    CHECK(stasis_audio_assets_voice_is_playing(&store, offline_music_voice) == 1);

    memset(&repeat_store, 0, sizeof(repeat_store));
    stasis_audio_assets_reset(&repeat_store);
    int repeat_music = stasis_audio_assets_load(&repeat_store, music_path);
    CHECK(repeat_music > 0);
    int repeat_voice = stasis_audio_assets_play(&repeat_store, repeat_music, 1, 0.2f, 0.0f);
    CHECK(repeat_voice > 0);
    memset(repeat_output, 0, sizeof(repeat_output));
    stasis_audio_assets_mix(&repeat_store, repeat_output, 1024, 48000);
    CHECK(memcmp(music_output, repeat_output, sizeof(music_output)) == 0);

    CHECK(stasis_load_font("../../outside.ttf", 18) == 0);
    CHECK(stasis_host_copy_runtime_error(diagnostic, sizeof(diagnostic)) == 1);
    CHECK(strstr(diagnostic, "../../outside.ttf") != NULL);
    CHECK(stasis_audio_load_music("../../missing.wav") == 0);
    CHECK(stasis_host_copy_runtime_error(diagnostic, sizeof(diagnostic)) == 1);
    CHECK(strstr(diagnostic, "../../missing.wav") != NULL);

    printf(
        "stasis.seam_test.v1 IT-015 sprite=%d font=%d cached=%d audio=%d voice=%d "
        "music=%d effect=%d events=%d music_played=%d effect_played=%d "
        "trace=%u samples=%.3f:%.3f offline_active=1 diagnostic=%s\n",
        sprite_handle,
        font_handle,
        cached_handle,
        stasis_jit_global_i32_load(hash_path("seam_audio_handle")),
        stasis_jit_global_i32_load(hash_path("seam_voice_handle")),
        stasis_jit_global_i32_load(hash_path("seam_music_handle")),
        stasis_jit_global_i32_load(hash_path("seam_effect_handle")),
        stasis_jit_global_i32_load(hash_path("seam_audio_event_order")),
        stasis_jit_global_i32_load(hash_path("seam_music_played")),
        stasis_jit_global_i32_load(hash_path("seam_effect_played")),
        actual_trace,
        output[2], output[3], diagnostic);
    stasis_audio_assets_release(&repeat_store, repeat_music);
    stasis_audio_assets_reset(&repeat_store);
    stasis_audio_assets_reset(&store);
    return 0;
}
