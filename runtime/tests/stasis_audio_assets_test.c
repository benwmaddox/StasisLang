#include "stasis_audio_assets.h"

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void write_u16(FILE* file, uint16_t value) {
    fputc(value & 0xff, file);
    fputc((value >> 8) & 0xff, file);
}

static void write_u32(FILE* file, uint32_t value) {
    fputc(value & 0xff, file);
    fputc((value >> 8) & 0xff, file);
    fputc((value >> 16) & 0xff, file);
    fputc((value >> 24) & 0xff, file);
}

static int write_fixture(const char* path) {
    const int16_t samples[4] = { 0, 16384, -16384, 8192 };
    FILE* file = fopen(path, "wb");
    if (!file) return 0;
    fwrite("RIFF", 1, 4, file);
    write_u32(file, 36 + sizeof(samples));
    fwrite("WAVEfmt ", 1, 8, file);
    write_u32(file, 16);
    write_u16(file, 1);
    write_u16(file, 1);
    write_u32(file, 24000);
    write_u32(file, 48000);
    write_u16(file, 2);
    write_u16(file, 16);
    fwrite("data", 1, 4, file);
    write_u32(file, sizeof(samples));
    fwrite(samples, sizeof(samples), 1, file);
    return fclose(file) == 0;
}

static int near(float actual, float expected) {
    return fabsf(actual - expected) < 0.001f;
}

int main(void) {
    const char* path = "stasis_audio_assets_test.wav";
    StasisAudioAssetStore store;
    float output[20];
    memset(&store, 0, sizeof(store));
    stasis_audio_assets_reset(&store);
    if (!write_fixture(path)) return 1;

    int asset = stasis_audio_assets_load_wav(&store, path);
    remove(path);
    if (asset <= 0 || store.assets[0].channels != 1 ||
        store.assets[0].sample_rate != 24000 || store.assets[0].frame_count != 4) return 2;

    int centered = stasis_audio_assets_play(&store, asset, 0, 0.5f, 0.0f);
    int right = stasis_audio_assets_play(&store, asset, 0, 0.5f, 1.0f);
    if (centered <= 0 || right <= 0 || centered == right) return 3;
    memset(output, 0, sizeof(output));
    stasis_audio_assets_mix(&store, output, 10, 48000);
    if (!near(output[2], 0.125f) || !near(output[3], 0.25f)) return 4;
    if (stasis_audio_assets_voice_is_playing(&store, centered)) return 5;

    int looped = stasis_audio_assets_play(&store, asset, 1, 1.0f, -1.0f);
    if (looped <= 0) return 6;
    stasis_audio_assets_set_asset_paused(&store, asset, 1);
    memset(output, 0, sizeof(output));
    stasis_audio_assets_mix(&store, output, 2, 48000);
    if (!near(output[0], 0.0f) || !stasis_audio_assets_voice_is_playing(&store, looped)) return 7;
    stasis_audio_assets_set_asset_paused(&store, asset, 0);
    stasis_audio_assets_set_asset_volume(&store, asset, 0.25f);
    stasis_audio_assets_voice_set_volume_pan(&store, looped, 0.25f, -1.0f);
    stasis_audio_assets_mix(&store, output, 10, 48000);
    if (!stasis_audio_assets_voice_is_playing(&store, looped)) return 8;
    stasis_audio_assets_stop_asset(&store, asset);
    if (stasis_audio_assets_voice_is_playing(&store, looped)) return 9;
    stasis_audio_assets_release(&store, asset);
    if (stasis_audio_assets_play(&store, asset, 0, 1.0f, 0.0f) != 0) return 10;
    stasis_audio_assets_reset(&store);
    return 0;
}
