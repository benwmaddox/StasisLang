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

int main(int argc, char **argv) {
    int32_t *gfx_i32;
    float *gfx_f32;
    uint8_t *gfx_u8;
    char audio_path[1024];
    char diagnostic[512];
    StasisAudioAssetStore store;
    float output[8];

    CHECK(argc == 2);
    CHECK(stasis_set_asset_root(argv[1]) == 1);
    CHECK(stasis_init_window(320, 180, "IT-015 mobile packaged assets") == 1);
    stasis_aot_bind_runtime_globals();
    CHECK(stasis_mobile_main_entry() == 0);
    CHECK(stasis_mobile_tick_entry() == 0);
    CHECK(stasis_mobile_render_entry() == 0);

    gfx_i32 = stasis_jit_global_i32_array_ptr(hash_path("gfx_cmd_i32"), 0, 34608);
    gfx_f32 = stasis_jit_global_f32_array_ptr(hash_path("gfx_cmd_f32"), 0, 125060);
    gfx_u8 = stasis_jit_global_u8_array_ptr(hash_path("gfx_cmd_u8"), 0, 65536);
    CHECK(gfx_i32 != NULL && gfx_f32 != NULL && gfx_u8 != NULL);
    CHECK(gfx_i32[4] == 1 && gfx_i32[7] == 2 && gfx_i32[22] == 3);
    CHECK(memcmp(gfx_u8, "BUNDLE", 6) == 0);
    stasis_gfx_submit_u8(gfx_i32, gfx_f32, gfx_u8);

    memset(&store, 0, sizeof(store));
    stasis_audio_assets_reset(&store);
    join_path(audio_path, sizeof(audio_path), argv[1], "assets/tone.wav");
    int offline_asset = stasis_audio_assets_load_wav(&store, audio_path);
    CHECK(offline_asset > 0);
    int offline_voice = stasis_audio_assets_play(&store, offline_asset, 0, 0.5f, 0.0f);
    CHECK(offline_voice > 0);
    memset(output, 0, sizeof(output));
    stasis_audio_assets_mix(&store, output, 4, 48000);
    CHECK(near(output[2], 0.125f) && near(output[3], 0.125f));
    CHECK(stasis_audio_assets_voice_is_playing(&store, offline_voice) == 1);

    CHECK(stasis_load_font("../../outside.ttf", 18) == 0);
    CHECK(stasis_host_copy_runtime_error(diagnostic, sizeof(diagnostic)) == 1);
    CHECK(strstr(diagnostic, "../../outside.ttf") != NULL);
    CHECK(stasis_audio_load_wav("../../missing.wav") == 0);
    CHECK(stasis_host_copy_runtime_error(diagnostic, sizeof(diagnostic)) == 1);
    CHECK(strstr(diagnostic, "../../missing.wav") != NULL);

    printf(
        "stasis.seam_test.v1 IT-015 sprite=%d font=%d cached=%d audio=%d voice=%d "
        "trace=%u samples=%.3f:%.3f offline_active=1 diagnostic=%s\n",
        stasis_jit_global_i32_load(hash_path("seam_sprite_handle")),
        stasis_jit_global_i32_load(hash_path("seam_font_handle")),
        stasis_jit_global_i32_load(hash_path("seam_cached_handle")),
        stasis_jit_global_i32_load(hash_path("seam_audio_handle")),
        stasis_jit_global_i32_load(hash_path("seam_voice_handle")),
        stasis_render_trace(gfx_i32, gfx_f32, gfx_u8),
        output[2], output[3], diagnostic);
    stasis_audio_assets_reset(&store);
    return 0;
}
