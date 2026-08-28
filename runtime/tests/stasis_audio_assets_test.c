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

static int write_corrupt_fixture(const char* path) {
    static const unsigned char bytes[] = {
        'R', 'I', 'F', 'F', 0, 0, 0, 0, 'W', 'A', 'V', 'E',
        'f', 'm', 't', ' ', 1, 0, 0, 0, 0
    };
    FILE* file = fopen(path, "wb");
    if (!file) return 0;
    fwrite(bytes, sizeof(bytes), 1, file);
    return fclose(file) == 0;
}

static int near(float actual, float expected) {
    return fabsf(actual - expected) < 0.001f;
}

int main(void) {
    const char* path = "stasis_audio_assets_test.wav";
    const char* corrupt_path = "stasis_audio_assets_corrupt.wav";
    StasisAudioAssetStore store;
    float output[20];
    float mp3_output[2048];
    StasisDecodedAudio decoded;
    int capacity_voices[STASIS_AUDIO_MAX_VOICES];
    memset(&store, 0, sizeof(store));
    stasis_audio_assets_reset(&store);
    if (!write_fixture(path)) return 1;

    memset(&decoded, 0, sizeof(decoded));
    if (!stasis_audio_decode(path, &decoded) || decoded.sample_rate != 24000 ||
        decoded.channels != 1 || decoded.frame_count != 4 || !decoded.samples ||
        decoded.samples[1] != 16384) return 2;
    stasis_audio_decoded_free(&decoded);

    int asset = stasis_audio_assets_load_wav(&store, path);
    remove(path);
    if (asset <= 0 || store.assets[0].channels != 1 ||
        store.assets[0].sample_rate != 24000 || store.assets[0].frame_count != 4) return 3;

    if (!write_corrupt_fixture(corrupt_path)) return 4;
    if (stasis_audio_assets_load(&store, corrupt_path) != 0 ||
        stasis_audio_assets_load_wav(&store, corrupt_path) != 0) return 5;
    remove(corrupt_path);
    if (stasis_audio_assets_load(&store, "stasis_audio_assets_missing.wav") != 0 ||
        stasis_audio_decode("stasis_audio_assets_missing.wav", &decoded) != 0) return 6;

    int centered = stasis_audio_assets_play(&store, asset, 0, 0.5f, 0.0f);
    int right = stasis_audio_assets_play(&store, asset, 0, 0.5f, 1.0f);
    if (centered <= 0 || right <= 0 || centered == right) return 7;
    memset(output, 0, sizeof(output));
    stasis_audio_assets_mix(&store, output, 10, 48000);
    if (!near(output[2], 0.125f) || !near(output[3], 0.25f)) return 8;
    if (stasis_audio_assets_voice_is_playing(&store, centered)) return 9;

    int looped = stasis_audio_assets_play(&store, asset, 1, 1.0f, -1.0f);
    if (looped <= 0) return 10;
    stasis_audio_assets_set_asset_paused(&store, asset, 1);
    memset(output, 0, sizeof(output));
    stasis_audio_assets_mix(&store, output, 2, 48000);
    if (!near(output[0], 0.0f) || !stasis_audio_assets_voice_is_playing(&store, looped)) return 11;
    stasis_audio_assets_set_asset_paused(&store, asset, 0);
    stasis_audio_assets_set_asset_volume(&store, asset, 0.25f);
    stasis_audio_assets_voice_set_volume_pan(&store, looped, 0.25f, -1.0f);
    memset(output, 0, sizeof(output));
    stasis_audio_assets_mix(&store, output, 10, 48000);
    if (!stasis_audio_assets_voice_is_playing(&store, looped) || output[0] < -1.0f ||
        output[0] > 1.0f || output[1] < -1.0f || output[1] > 1.0f) return 12;
    stasis_audio_assets_stop_asset(&store, asset);
    if (stasis_audio_assets_voice_is_playing(&store, looped)) return 13;
    stasis_audio_assets_release(&store, asset);
    if (stasis_audio_assets_play(&store, asset, 0, 1.0f, 0.0f) != 0) return 14;

    if (!write_fixture(path)) return 15;
    asset = stasis_audio_assets_load_wav(&store, path);
    remove(path);
    if (asset <= 0) return 16;
    memset(capacity_voices, 0, sizeof(capacity_voices));
    for (int index = 0; index < STASIS_AUDIO_MAX_VOICES; index++) {
        capacity_voices[index] = stasis_audio_assets_play(&store, asset, 1, 0.05f, 0.0f);
        if (capacity_voices[index] <= 0) return 17;
    }
    if (stasis_audio_assets_play(&store, asset, 1, 0.05f, 0.0f) != 0) return 18;
    for (int index = 0; index < STASIS_AUDIO_MAX_VOICES; index++) {
        if (!stasis_audio_assets_voice_is_playing(&store, capacity_voices[index])) return 19;
    }
    memset(output, 0, sizeof(output));
    stasis_audio_assets_mix(&store, output, 1, 48000);
    if (!stasis_audio_assets_has_active_voice(&store) || output[0] < -1.0f ||
        output[0] > 1.0f || output[1] < -1.0f || output[1] > 1.0f) return 20;
    stasis_audio_assets_stop_asset(&store, asset);
    stasis_audio_assets_release(&store, asset);

#ifdef STASIS_TEST_MP3_PATH
    if (stasis_audio_assets_load_wav(&store, STASIS_TEST_MP3_PATH) != 0) return 25;
    memset(&decoded, 0, sizeof(decoded));
    if (!stasis_audio_decode(STASIS_TEST_MP3_PATH, &decoded) ||
        decoded.sample_rate != 24000 || decoded.channels != 1 ||
        decoded.frame_count != 18000 || !decoded.samples) return 21;
    stasis_audio_decoded_free(&decoded);
    int mp3 = stasis_audio_assets_load(&store, STASIS_TEST_MP3_PATH);
    if (mp3 <= 0 || store.assets[0].channels != 1 ||
        store.assets[0].sample_rate != 24000 || store.assets[0].frame_count != 18000) return 22;
    int mp3_voice = stasis_audio_assets_play(&store, mp3, 0, 0.5f, 0.0f);
    if (mp3_voice <= 0) return 23;
    memset(mp3_output, 0, sizeof(mp3_output));
    stasis_audio_assets_mix(&store, mp3_output, 1024, 48000);
    float mp3_energy = 0.0f;
    for (size_t index = 0; index < sizeof(mp3_output) / sizeof(mp3_output[0]); index++) {
        mp3_energy += fabsf(mp3_output[index]);
    }
    if (!stasis_audio_assets_voice_is_playing(&store, mp3_voice) ||
        mp3_energy <= 0.001f) return 24;
    stasis_audio_assets_release(&store, mp3);
#endif
    stasis_audio_assets_reset(&store);
    return 0;
}
