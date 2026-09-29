#include <math.h>
#include <stdio.h>
#include <string.h>
#include "stasis_recording_audio.h"

extern int stasis_set_recording_audio_config(int enabled);
extern int stasis_audio_init(int sample_rate, int channels, int target_latency_frames);
extern int stasis_audio_is_available(void);
extern int stasis_audio_get_sample_rate(void);
extern int stasis_audio_get_channels(void);
extern int stasis_audio_push_f32_interleaved(const float* samples, int frame_count);
extern int stasis_recording_audio_pull_f32_interleaved(float* output, int frame_count);
extern int stasis_recording_audio_configure_device_v1(
    const StasisRecordingAudioDeviceConfigV1* config);
extern int stasis_recording_audio_set_device_state_v1(int accepting_pushes, int paused);
extern int stasis_recording_audio_advance_v1(float* output, int frame_count);
extern int stasis_recording_audio_get_health_v1(StasisRecordingAudioHealthV1* output);
extern int stasis_audio_play(int asset_handle, int loop, float volume, float pan);
extern int stasis_audio_voice_is_playing(int voice_handle);
extern void stasis_audio_voice_set_volume_pan(int voice_handle, float volume, float pan);
extern int stasis_asset_request_audio(const char* path);
extern int stasis_asset_task_poll(int task_id);
extern int stasis_asset_task_take_handle(int task_id);
extern void stasis_audio_shutdown(void);

#ifndef STASIS_TEST_AUDIO_PATH
#error STASIS_TEST_AUDIO_PATH is required
#endif

#define STASIS_ASSET_TASK_LOADED 3

static int near(float actual, float expected) {
    return fabsf(actual - expected) < 0.0001f;
}

int main(void) {
    if (!stasis_set_recording_audio_config(1)) return 1;
    if (stasis_audio_init(44100, 2, 1024) != 0) return 2;
    if (stasis_audio_get_sample_rate() != 48000) return 3;
    if (stasis_audio_init(48000, 2, (1 << 20) + 1) != 0) return 4;
    if (!stasis_audio_init(48000, 2, 1024)) return 2;
    if (!stasis_audio_is_available() || stasis_audio_get_sample_rate() != 48000 ||
        stasis_audio_get_channels() != 2) return 5;

    const float pushed[] = { 0.25f, -0.25f, 0.5f, -0.5f };
    if (stasis_audio_push_f32_interleaved(pushed, 2) != 2) return 6;
    float output[8];
    memset(output, 0, sizeof(output));
    if (stasis_recording_audio_pull_f32_interleaved(output, 2) != 2) return 7;
    if (!near(output[0], pushed[0]) || !near(output[1], pushed[1]) ||
        !near(output[2], pushed[2]) || !near(output[3], pushed[3])) return 8;
    memset(output, 0, sizeof(output));
    if (stasis_recording_audio_pull_f32_interleaved(output, 2) != 2) return 9;
    for (int i = 0; i < 4; i++) if (!near(output[i], 0.0f)) return 10;

    int task = stasis_asset_request_audio(STASIS_TEST_AUDIO_PATH);
    if (task <= 0) {
        fprintf(stderr, "audio task request failed\n");
        return 11;
    }
    int state = stasis_asset_task_poll(task);
    if (state != STASIS_ASSET_TASK_LOADED) {
        fprintf(stderr, "audio task state=%d expected immediate loaded\n", state);
        return 12;
    }
    int asset = stasis_asset_task_take_handle(task);
    if (asset <= 0) return 13;
    int voice = stasis_audio_play(asset, 1, 0.5f, 0.0f);
    if (voice <= 0) return 14;
    float mixed[4096 * 2];
    memset(mixed, 0, sizeof(mixed));
    if (stasis_recording_audio_pull_f32_interleaved(mixed, 4096) != 4096) return 15;
    int non_silent = 0;
    for (int i = 0; i < 4096 * 2; i++) {
        if (fabsf(mixed[i]) > 0.0001f) non_silent = 1;
    }
    if (!non_silent || !stasis_audio_voice_is_playing(voice)) return 16;
    stasis_audio_voice_set_volume_pan(voice, 0.25f, 1.0f);
    memset(mixed, 0, sizeof(mixed));
    if (stasis_recording_audio_pull_f32_interleaved(mixed, 4096) != 4096) return 17;
    for (int i = 0; i < 4096; i++) {
        if (!near(mixed[i * 2], 0.0f) && fabsf(mixed[i * 2]) > 0.0001f) return 18;
    }

    stasis_audio_shutdown();
    if (!stasis_set_recording_audio_config(0)) return 19;
    if (!stasis_set_recording_audio_config(1)) return 20;
    if (!stasis_audio_init(48000, 2, 1024)) return 21;
    memset(output, 0, sizeof(output));
    if (stasis_recording_audio_pull_f32_interleaved(output, 2) != 2) return 22;
    for (int i = 0; i < 4; i++) if (!near(output[i], 0.0f)) return 23;

    const StasisRecordingAudioDeviceConfigV1 config = {
        (uint32_t)sizeof(StasisRecordingAudioDeviceConfigV1),
        STASIS_RECORDING_AUDIO_DEVICE_CONFIG_V1_VERSION,
        50,
        100,
    };
    if (!stasis_recording_audio_configure_device_v1(&config)) return 25;
    if (!stasis_audio_is_available()) return 26;
    if (stasis_audio_push_f32_interleaved(pushed, 2) != 0) return 27;
    if (stasis_audio_get_queued_frames() != 0) return 28;

    float callback[960 * 2];
    memset(callback, 1, sizeof(callback));
    if (stasis_recording_audio_advance_v1(callback, 960) != 960) return 29;
    for (int i = 0; i < 960 * 2; i++) if (!near(callback[i], 0.0f)) return 30;

    StasisRecordingAudioHealthV1 health = { 0 };
    health.struct_size = (uint32_t)sizeof(StasisRecordingAudioHealthV1);
    if (!stasis_recording_audio_get_health_v1(&health)) return 31;
    if (health.version != STASIS_RECORDING_AUDIO_HEALTH_V1_VERSION ||
        health.push_attempts != 1 || health.requested_frames != 2 ||
        health.accepted_frames != 0 || health.refused_pushes != 1 ||
        health.refused_frames != 2 || health.callbacks != 1 ||
        health.output_frames != 960 || health.underruns != 1 ||
        health.first_sound_frame != UINT32_MAX || !near(health.peak, 0.0f) ||
        !near(health.rms, 0.0f)) return 32;

    if (!stasis_recording_audio_set_device_state_v1(1, 0)) return 33;
    if (stasis_audio_push_f32_interleaved(pushed, 2) != 2) return 34;
    memset(callback, 0, 4 * sizeof(float));
    if (stasis_recording_audio_advance_v1(callback, 2) != 2) return 35;
    for (int i = 0; i < 4; i++) if (!near(callback[i], pushed[i])) return 36;
    if (!stasis_recording_audio_get_health_v1(&health)) return 37;
    if (health.push_attempts != 2 || health.requested_frames != 4 ||
        health.accepted_frames != 2 || health.refused_pushes != 1 ||
        health.callbacks != 2 || health.output_frames != 962 ||
        health.first_sound_frame != 960 || !near(health.peak, 0.5f) ||
        !near(health.rms, sqrtf(0.625f / 1924.0f))) return 38;

    if (stasis_audio_push_f32_interleaved(pushed, 2) != 2) return 39;
    if (stasis_audio_get_queued_frames() != 2) return 40;
    if (!stasis_recording_audio_set_device_state_v1(1, 1)) return 41;
    if (stasis_audio_get_queued_frames() != 0) return 42;
    if (stasis_audio_push_f32_interleaved(pushed, 2) != 0) return 43;
    memset(callback, 1, 4 * sizeof(float));
    if (stasis_recording_audio_advance_v1(callback, 2) != 2) return 44;
    for (int i = 0; i < 4; i++) if (!near(callback[i], 0.0f)) return 45;

    if (!stasis_recording_audio_set_device_state_v1(1, 0)) return 46;
    const float resumed[] = { 0.25f, -0.25f, 0.0f, 0.0f };
    if (stasis_audio_push_f32_interleaved(resumed, 2) != 2) return 47;
    if (stasis_recording_audio_advance_v1(callback, 2) != 2) return 48;
    if (!stasis_recording_audio_get_health_v1(&health)) return 49;
    if (health.push_attempts != 5 || health.requested_frames != 10 ||
        health.accepted_frames != 6 || health.refused_pushes != 2 ||
        health.refused_frames != 4 || health.callbacks != 4 ||
        health.output_frames != 966 || health.underruns != 1 ||
        health.longest_silent_run_after_sound != 2 ||
        health.trailing_silent_frames != 1 ||
        !near(health.rms, sqrtf(0.75f / 1932.0f))) return 50;

    stasis_audio_shutdown();
    memset(&health, 0, sizeof(health));
    health.struct_size = (uint32_t)sizeof(StasisRecordingAudioHealthV1);
    if (!stasis_recording_audio_get_health_v1(&health)) return 51;
    if (health.push_attempts != 5 || health.accepted_frames != 6 ||
        health.refused_pushes != 2 || health.callbacks != 4 ||
        health.output_frames != 966 || health.underruns != 1) return 52;

    if (!stasis_audio_is_available() ||
        !stasis_audio_init(48000, 2, 1024)) return 53;
    if (stasis_audio_push_f32_interleaved(pushed, 2) != 2) return 54;
    if (stasis_recording_audio_advance_v1(callback, 2) != 2) return 55;
    if (!stasis_recording_audio_get_health_v1(&health)) return 56;
    if (health.push_attempts != 6 || health.requested_frames != 12 ||
        health.accepted_frames != 8 || health.refused_pushes != 2 ||
        health.callbacks != 5 || health.output_frames != 968 ||
        health.underruns != 1 || health.first_sound_frame != 960 ||
        health.longest_silent_run_after_sound != 2 ||
        health.trailing_silent_frames != 0 ||
        !near(health.rms, sqrtf(1.375f / 1936.0f))) return 57;

    const StasisRecordingAudioDeviceConfigV1 unsupported_version = {
        (uint32_t)sizeof(StasisRecordingAudioDeviceConfigV1), 2, 50, 0
    };
    if (stasis_recording_audio_configure_device_v1(&unsupported_version)) return 58;
    stasis_audio_shutdown();
    if (!stasis_set_recording_audio_config(0)) return 24;
    puts("stasis recording audio offline mixer contract passed");
    return 0;
}
