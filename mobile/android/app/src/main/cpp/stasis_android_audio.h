#ifndef STASIS_ANDROID_AUDIO_H
#define STASIS_ANDROID_AUDIO_H

#include <stdint.h>

int stasis_audio_init(int sample_rate, int channels, int target_latency_frames);
void stasis_audio_shutdown(void);
int stasis_audio_is_available(void);
int stasis_audio_get_sample_rate(void);
int stasis_audio_get_channels(void);
int stasis_audio_get_queued_frames(void);
int stasis_audio_get_underruns(void);
int stasis_audio_push_f32_interleaved(const float *samples, int frame_count);

void stasis_android_audio_set_paused(int paused);
void stasis_android_audio_set_focus(int focused);
int stasis_android_audio_is_requested(void);
int stasis_android_audio_is_running(void);
int stasis_android_audio_last_error(void);

#endif
