#ifndef STASIS_AUDIO_ASSETS_H
#define STASIS_AUDIO_ASSETS_H

#include <stdint.h>

#define STASIS_AUDIO_MAX_ASSETS 64
#define STASIS_AUDIO_MAX_VOICES 32
#define STASIS_AUDIO_MAX_FILE_BYTES (16 * 1024 * 1024)

typedef struct {
    int handle;
    int sample_rate;
    int channels;
    int frame_count;
    int16_t* samples;
} StasisAudioAsset;

typedef struct {
    int handle;
    int asset_index;
    double cursor;
    float volume;
    float pan;
    int loop;
    int paused;
    int active;
} StasisAudioVoice;

typedef struct {
    StasisAudioAsset assets[STASIS_AUDIO_MAX_ASSETS];
    StasisAudioVoice voices[STASIS_AUDIO_MAX_VOICES];
    int next_asset_handle;
    int next_voice_handle;
} StasisAudioAssetStore;

void stasis_audio_assets_reset(StasisAudioAssetStore* store);
int stasis_audio_assets_load_wav(StasisAudioAssetStore* store, const char* path);
void stasis_audio_assets_release(StasisAudioAssetStore* store, int asset_handle);
int stasis_audio_assets_play(
    StasisAudioAssetStore* store,
    int asset_handle,
    int loop,
    float volume,
    float pan
);
void stasis_audio_assets_stop_voice(StasisAudioAssetStore* store, int voice_handle);
void stasis_audio_assets_stop_asset(StasisAudioAssetStore* store, int asset_handle);
void stasis_audio_assets_set_asset_paused(
    StasisAudioAssetStore* store,
    int asset_handle,
    int paused
);
void stasis_audio_assets_set_asset_volume(
    StasisAudioAssetStore* store,
    int asset_handle,
    float volume
);
int stasis_audio_assets_voice_is_playing(const StasisAudioAssetStore* store, int voice_handle);
void stasis_audio_assets_voice_set_paused(
    StasisAudioAssetStore* store,
    int voice_handle,
    int paused
);
void stasis_audio_assets_voice_set_volume_pan(
    StasisAudioAssetStore* store,
    int voice_handle,
    float volume,
    float pan
);
int stasis_audio_assets_has_active_voice(const StasisAudioAssetStore* store);
void stasis_audio_assets_mix(
    StasisAudioAssetStore* store,
    float* output_stereo,
    int frame_count,
    int output_sample_rate
);

#endif
