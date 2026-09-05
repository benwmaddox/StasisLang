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

int stasis_audio_set_project_root(const char *project_root);
int stasis_audio_load_wav(const char *path);
void stasis_audio_release(int asset_handle);
int stasis_audio_play(int asset_handle, int loop, float volume, float pan);
void stasis_audio_stop(int voice_handle);
int stasis_audio_voice_is_playing(int voice_handle);
void stasis_audio_voice_set_paused(int voice_handle, int paused);
void stasis_audio_voice_set_volume_pan(int voice_handle, float volume, float pan);
int stasis_audio_load_music(const char *path);
int stasis_audio_load_effect(const char *path);
int stasis_audio_play_music(int asset_handle, int loop, float volume);
void stasis_audio_stop_music(int asset_handle);
void stasis_audio_pause_music(int asset_handle, int paused);
void stasis_audio_set_music_volume(int asset_handle, float volume);
int stasis_audio_play_effect(int asset_handle, float volume);

void stasis_android_audio_set_paused(int paused);
void stasis_android_audio_set_focus(int focused);
int stasis_android_audio_is_requested(void);
int stasis_android_audio_is_running(void);
int stasis_android_audio_last_error(void);

#endif
