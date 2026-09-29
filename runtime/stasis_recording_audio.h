#ifndef STASIS_RECORDING_AUDIO_H
#define STASIS_RECORDING_AUDIO_H

#include <stdint.h>

#define STASIS_RECORDING_AUDIO_DEVICE_CONFIG_V1_VERSION 1
#define STASIS_RECORDING_AUDIO_HEALTH_V1_VERSION 1
#define STASIS_RECORDING_AUDIO_MAX_CALLBACK_HZ 1000
#define STASIS_RECORDING_AUDIO_MAX_REFUSAL_MS 600000

typedef struct {
    uint32_t struct_size;
    uint32_t version;
    uint32_t callback_hz;
    uint32_t refuse_push_for_ms;
} StasisRecordingAudioDeviceConfigV1;

typedef struct {
    uint32_t struct_size;
    uint32_t version;
    uint32_t push_attempts;
    uint32_t requested_frames;
    uint32_t accepted_frames;
    uint32_t refused_pushes;
    uint32_t refused_frames;
    uint32_t callbacks;
    uint32_t output_frames;
    uint32_t underruns;
    uint32_t first_sound_frame;
    uint32_t longest_silent_run_after_sound;
    uint32_t trailing_silent_frames;
    float peak;
    float rms;
} StasisRecordingAudioHealthV1;

#endif
