#include <math.h>
#include <stdio.h>
#include <string.h>

extern int stasis_set_recording_audio_config(int enabled);
extern int stasis_audio_init(int sample_rate, int channels, int target_latency_frames);
extern int stasis_audio_is_available(void);
extern int stasis_audio_get_sample_rate(void);
extern int stasis_audio_get_channels(void);
extern int stasis_audio_push_f32_interleaved(const float* samples, int frame_count);
extern int stasis_recording_audio_pull_f32_interleaved(float* output, int frame_count);
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
    stasis_audio_shutdown();
    if (!stasis_set_recording_audio_config(0)) return 24;
    puts("stasis recording audio offline mixer contract passed");
    return 0;
}
