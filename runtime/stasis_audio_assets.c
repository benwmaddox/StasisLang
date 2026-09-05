#include "stasis_audio_assets.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MINIMP3_IMPLEMENTATION
#define MINIMP3_ONLY_MP3
#include "minimp3_ex.h"

static uint16_t read_u16(const uint8_t* bytes) {
    return (uint16_t)((uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8));
}

static uint32_t read_u32(const uint8_t* bytes) {
    return (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
        ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
}

static int find_asset(const StasisAudioAssetStore* store, int handle) {
    if (!store || handle <= 0) return -1;
    for (int i = 0; i < STASIS_AUDIO_MAX_ASSETS; i++) {
        if (store->assets[i].handle == handle && store->assets[i].samples) return i;
    }
    return -1;
}

static int find_open_asset_slot(const StasisAudioAssetStore* store) {
    if (!store) return -1;
    for (int i = 0; i < STASIS_AUDIO_MAX_ASSETS; i++) {
        if (!store->assets[i].samples) return i;
    }
    return -1;
}

static int store_samples(
    StasisAudioAssetStore* store,
    int16_t* samples,
    int sample_rate,
    int channels,
    int frame_count
) {
    int slot = find_open_asset_slot(store);
    if (slot < 0 || !samples || sample_rate <= 0 || (channels != 1 && channels != 2) ||
        frame_count <= 0) return 0;
    store->assets[slot].handle = store->next_asset_handle++;
    if (store->next_asset_handle <= 0) store->next_asset_handle = 1;
    store->assets[slot].sample_rate = sample_rate;
    store->assets[slot].channels = channels;
    store->assets[slot].frame_count = frame_count;
    store->assets[slot].samples = samples;
    return store->assets[slot].handle;
}

void stasis_audio_decoded_free(StasisDecodedAudio* decoded) {
    if (!decoded) return;
    free(decoded->samples);
    memset(decoded, 0, sizeof(*decoded));
}

int stasis_audio_assets_store_decoded(
    StasisAudioAssetStore* store,
    StasisDecodedAudio* decoded
) {
    if (!decoded) return 0;
    int handle = store_samples(
        store,
        decoded->samples,
        decoded->sample_rate,
        decoded->channels,
        decoded->frame_count
    );
    if (handle > 0) memset(decoded, 0, sizeof(*decoded));
    return handle;
}

static float clamp_unit(float value) {
    if (value < -1.0f) return -1.0f;
    if (value > 1.0f) return 1.0f;
    return value;
}

static float sample_at(const StasisAudioAsset* asset, int frame, int channel) {
    if (!asset || !asset->samples || asset->frame_count <= 0) return 0.0f;
    if (frame < 0) frame = 0;
    if (frame >= asset->frame_count) frame = asset->frame_count - 1;
    if (asset->channels == 1) channel = 0;
    return (float)asset->samples[frame * asset->channels + channel] / 32768.0f;
}

void stasis_audio_assets_reset(StasisAudioAssetStore* store) {
    if (!store) return;
    for (int i = 0; i < STASIS_AUDIO_MAX_ASSETS; i++) free(store->assets[i].samples);
    memset(store, 0, sizeof(*store));
    store->next_asset_handle = 1;
    store->next_voice_handle = 1;
}

int stasis_audio_assets_load_wav(StasisAudioAssetStore* store, const char* path) {
    FILE* file = NULL;
    uint8_t* bytes = NULL;
    int16_t* samples = NULL;
    long file_size = 0;
    uint16_t format = 0;
    uint16_t channels = 0;
    uint16_t bits_per_sample = 0;
    uint32_t sample_rate = 0;
    const uint8_t* data = NULL;
    uint32_t data_size = 0;

    if (!store || !path || !*path) return 0;
    file = fopen(path, "rb");
    if (!file) return 0;
    if (fseek(file, 0, SEEK_END) != 0) goto fail;
    file_size = ftell(file);
    if (file_size < 44 || file_size > STASIS_AUDIO_MAX_FILE_BYTES) goto fail;
    if (fseek(file, 0, SEEK_SET) != 0) goto fail;
    bytes = (uint8_t*)malloc((size_t)file_size);
    if (!bytes || fread(bytes, 1, (size_t)file_size, file) != (size_t)file_size) goto fail;
    fclose(file);
    file = NULL;

    if (memcmp(bytes, "RIFF", 4) != 0 || memcmp(bytes + 8, "WAVE", 4) != 0) goto fail;
    size_t offset = 12;
    while (offset + 8 <= (size_t)file_size) {
        const uint8_t* chunk = bytes + offset;
        uint32_t chunk_size = read_u32(chunk + 4);
        offset += 8;
        if ((uint64_t)offset + chunk_size > (uint64_t)file_size) goto fail;
        if (memcmp(chunk, "fmt ", 4) == 0 && chunk_size >= 16) {
            format = read_u16(bytes + offset);
            channels = read_u16(bytes + offset + 2);
            sample_rate = read_u32(bytes + offset + 4);
            bits_per_sample = read_u16(bytes + offset + 14);
        } else if (memcmp(chunk, "data", 4) == 0 && !data) {
            data = bytes + offset;
            data_size = chunk_size;
        }
        offset += chunk_size + (chunk_size & 1u);
    }
    if (format != 1 || (channels != 1 && channels != 2) || bits_per_sample != 16 ||
        sample_rate < 8000 || sample_rate > 384000 || !data || data_size == 0 ||
        data_size % ((uint32_t)channels * 2u) != 0) goto fail;

    samples = (int16_t*)malloc(data_size);
    if (!samples) goto fail;
    memcpy(samples, data, data_size);
    free(bytes);
    bytes = NULL;
    int handle = store_samples(
        store,
        samples,
        (int)sample_rate,
        (int)channels,
        (int)(data_size / ((uint32_t)channels * 2u))
    );
    if (handle <= 0) goto fail;
    return handle;

fail:
    if (file) fclose(file);
    free(bytes);
    free(samples);
    return 0;
}

int stasis_audio_assets_load_mp3(StasisAudioAssetStore* store, const char* path) {
    FILE* file = NULL;
    uint8_t* bytes = NULL;
    int16_t* samples = NULL;
    long file_size = 0;
    mp3dec_ex_t decoder;
    int decoder_open = 0;
    int result = 0;

    if (!store || !path || !*path) return 0;
    file = fopen(path, "rb");
    if (!file) return 0;
    if (fseek(file, 0, SEEK_END) != 0) goto done;
    file_size = ftell(file);
    if (file_size <= 0 || file_size > STASIS_AUDIO_MAX_FILE_BYTES) goto done;
    if (fseek(file, 0, SEEK_SET) != 0) goto done;
    bytes = (uint8_t*)malloc((size_t)file_size);
    if (!bytes || fread(bytes, 1, (size_t)file_size, file) != (size_t)file_size) goto done;
    fclose(file);
    file = NULL;
    if (mp3dec_detect_buf(bytes, (size_t)file_size) != 0) goto done;
    memset(&decoder, 0, sizeof(decoder));
    if (mp3dec_ex_open_buf(&decoder, bytes, (size_t)file_size, MP3D_SEEK_TO_SAMPLE) != 0) goto done;
    decoder_open = 1;
    if ((decoder.info.channels != 1 && decoder.info.channels != 2) ||
        decoder.info.hz < 8000 || decoder.info.hz > 384000 || decoder.samples == 0 ||
        decoder.samples > (uint64_t)STASIS_AUDIO_MAX_DECODED_BYTES / sizeof(int16_t) ||
        decoder.samples / (uint64_t)decoder.info.channels > 2147483647u) goto done;
    samples = (int16_t*)malloc((size_t)decoder.samples * sizeof(int16_t));
    if (!samples) goto done;
    size_t decoded = mp3dec_ex_read(&decoder, samples, (size_t)decoder.samples);
    if (decoded == 0 || decoded % (size_t)decoder.info.channels != 0) goto done;
    result = store_samples(
        store,
        samples,
        decoder.info.hz,
        decoder.info.channels,
        (int)(decoded / (size_t)decoder.info.channels)
    );
    if (result > 0) samples = NULL;

done:
    if (file) fclose(file);
    if (decoder_open) mp3dec_ex_close(&decoder);
    free(bytes);
    free(samples);
    return result;
}

int stasis_audio_decode(const char* path, StasisDecodedAudio* decoded) {
    FILE* file = NULL;
    uint8_t signature[12];
    size_t read = 0;
    if (!decoded) return 0;
    memset(decoded, 0, sizeof(*decoded));
    if (!path || !*path) return 0;

    file = fopen(path, "rb");
    if (!file) return 0;
    read = fread(signature, 1, sizeof(signature), file);
    fclose(file);

    StasisAudioAssetStore temporary;
    memset(&temporary, 0, sizeof(temporary));
    stasis_audio_assets_reset(&temporary);
    int handle = 0;
    if (read >= sizeof(signature) && memcmp(signature, "RIFF", 4) == 0 &&
        memcmp(signature + 8, "WAVE", 4) == 0) {
        handle = stasis_audio_assets_load_wav(&temporary, path);
    } else {
        handle = stasis_audio_assets_load_mp3(&temporary, path);
    }
    if (handle <= 0) {
        stasis_audio_assets_reset(&temporary);
        return 0;
    }
    for (int i = 0; i < STASIS_AUDIO_MAX_ASSETS; i++) {
        if (temporary.assets[i].handle != handle || !temporary.assets[i].samples) continue;
        decoded->sample_rate = temporary.assets[i].sample_rate;
        decoded->channels = temporary.assets[i].channels;
        decoded->frame_count = temporary.assets[i].frame_count;
        decoded->samples = temporary.assets[i].samples;
        temporary.assets[i].samples = NULL;
        stasis_audio_assets_reset(&temporary);
        return 1;
    }
    stasis_audio_assets_reset(&temporary);
    return 0;
}

int stasis_audio_assets_load(StasisAudioAssetStore* store, const char* path) {
    FILE* file = NULL;
    uint8_t signature[12];
    size_t read = 0;
    if (!store || !path || !*path) return 0;
    file = fopen(path, "rb");
    if (!file) return 0;
    read = fread(signature, 1, sizeof(signature), file);
    fclose(file);
    if (read >= sizeof(signature) && memcmp(signature, "RIFF", 4) == 0 &&
        memcmp(signature + 8, "WAVE", 4) == 0) {
        return stasis_audio_assets_load_wav(store, path);
    }
    return stasis_audio_assets_load_mp3(store, path);
}

void stasis_audio_assets_release(StasisAudioAssetStore* store, int asset_handle) {
    int asset_index = find_asset(store, asset_handle);
    if (asset_index < 0) return;
    stasis_audio_assets_stop_asset(store, asset_handle);
    free(store->assets[asset_index].samples);
    memset(&store->assets[asset_index], 0, sizeof(store->assets[asset_index]));
}

int stasis_audio_assets_play(
    StasisAudioAssetStore* store,
    int asset_handle,
    int loop,
    float volume,
    float pan
) {
    int asset_index = find_asset(store, asset_handle);
    if (asset_index < 0) return 0;
    volume = clamp_unit(volume);
    if (volume < 0.0f) volume = 0.0f;
    pan = clamp_unit(pan);
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (!store->voices[i].active) {
            store->voices[i].handle = store->next_voice_handle++;
            if (store->next_voice_handle <= 0) store->next_voice_handle = 1;
            store->voices[i].asset_index = asset_index;
            store->voices[i].cursor = 0.0;
            store->voices[i].volume = volume;
            store->voices[i].pan = pan;
            store->voices[i].loop = loop != 0;
            store->voices[i].paused = 0;
            store->voices[i].active = 1;
            return store->voices[i].handle;
        }
    }
    return 0;
}

void stasis_audio_assets_stop_voice(StasisAudioAssetStore* store, int voice_handle) {
    if (!store) return;
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (store->voices[i].active && store->voices[i].handle == voice_handle) {
            store->voices[i].active = 0;
            return;
        }
    }
}

void stasis_audio_assets_stop_asset(StasisAudioAssetStore* store, int asset_handle) {
    int asset_index = find_asset(store, asset_handle);
    if (asset_index < 0) return;
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (store->voices[i].active && store->voices[i].asset_index == asset_index) {
            store->voices[i].active = 0;
        }
    }
}

void stasis_audio_assets_set_asset_paused(
    StasisAudioAssetStore* store,
    int asset_handle,
    int paused
) {
    int asset_index = find_asset(store, asset_handle);
    if (asset_index < 0) return;
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (store->voices[i].active && store->voices[i].asset_index == asset_index) {
            store->voices[i].paused = paused != 0;
        }
    }
}

void stasis_audio_assets_set_asset_volume(
    StasisAudioAssetStore* store,
    int asset_handle,
    float volume
) {
    int asset_index = find_asset(store, asset_handle);
    if (asset_index < 0) return;
    volume = clamp_unit(volume);
    if (volume < 0.0f) volume = 0.0f;
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (store->voices[i].active && store->voices[i].asset_index == asset_index) {
            store->voices[i].volume = volume;
        }
    }
}

int stasis_audio_assets_voice_is_playing(const StasisAudioAssetStore* store, int voice_handle) {
    if (!store) return 0;
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (store->voices[i].active && store->voices[i].handle == voice_handle) return 1;
    }
    return 0;
}

void stasis_audio_assets_voice_set_paused(
    StasisAudioAssetStore* store,
    int voice_handle,
    int paused
) {
    if (!store) return;
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (store->voices[i].active && store->voices[i].handle == voice_handle) {
            store->voices[i].paused = paused != 0;
            return;
        }
    }
}

void stasis_audio_assets_voice_set_volume_pan(
    StasisAudioAssetStore* store,
    int voice_handle,
    float volume,
    float pan
) {
    if (!store) return;
    volume = clamp_unit(volume);
    if (volume < 0.0f) volume = 0.0f;
    pan = clamp_unit(pan);
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (store->voices[i].active && store->voices[i].handle == voice_handle) {
            store->voices[i].volume = volume;
            store->voices[i].pan = pan;
            return;
        }
    }
}

int stasis_audio_assets_has_active_voice(const StasisAudioAssetStore* store) {
    if (!store) return 0;
    for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
        if (store->voices[i].active && !store->voices[i].paused) return 1;
    }
    return 0;
}

void stasis_audio_assets_mix(
    StasisAudioAssetStore* store,
    float* output_stereo,
    int frame_count,
    int output_sample_rate
) {
    if (!store || !output_stereo || frame_count <= 0 || output_sample_rate <= 0) return;
    for (int frame = 0; frame < frame_count; frame++) {
        float left = output_stereo[frame * 2];
        float right = output_stereo[frame * 2 + 1];
        for (int i = 0; i < STASIS_AUDIO_MAX_VOICES; i++) {
            StasisAudioVoice* voice = &store->voices[i];
            if (!voice->active || voice->paused || voice->asset_index < 0 ||
                voice->asset_index >= STASIS_AUDIO_MAX_ASSETS) continue;
            StasisAudioAsset* asset = &store->assets[voice->asset_index];
            if (!asset->samples || asset->frame_count <= 0) {
                voice->active = 0;
                continue;
            }
            int source_frame = (int)voice->cursor;
            if (source_frame >= asset->frame_count) {
                if (!voice->loop) {
                    voice->active = 0;
                    continue;
                }
                voice->cursor = fmod(voice->cursor, (double)asset->frame_count);
                source_frame = (int)voice->cursor;
            }
            int next_frame = source_frame + 1;
            if (next_frame >= asset->frame_count) next_frame = voice->loop ? 0 : source_frame;
            float fraction = (float)(voice->cursor - (double)source_frame);
            float source_left = sample_at(asset, source_frame, 0);
            float source_right = sample_at(asset, source_frame, 1);
            source_left += (sample_at(asset, next_frame, 0) - source_left) * fraction;
            source_right += (sample_at(asset, next_frame, 1) - source_right) * fraction;
            float left_gain = voice->volume * (voice->pan > 0.0f ? 1.0f - voice->pan : 1.0f);
            float right_gain = voice->volume * (voice->pan < 0.0f ? 1.0f + voice->pan : 1.0f);
            left += source_left * left_gain;
            right += source_right * right_gain;
            voice->cursor += (double)asset->sample_rate / (double)output_sample_rate;
        }
        output_stereo[frame * 2] = clamp_unit(left);
        output_stereo[frame * 2 + 1] = clamp_unit(right);
    }
}
