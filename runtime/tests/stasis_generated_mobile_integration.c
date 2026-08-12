#include "stasis_mobile_runtime.h"
#include "stasis_mobile_aot_runtime.h"
#include "stasis_render_contract.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define IT012_EXPECTED_TRACE 3312025514u

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "IT-012 check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

extern void stasis_aot_bind_runtime_globals(void);
extern int32_t stasis_mobile_main_entry(void);
extern int32_t stasis_mobile_tick_entry(void);
extern int32_t stasis_mobile_render_entry(void);

static int32_t submitted_frames;
static uint32_t submitted_trace;
static int32_t submitted_rects;
static int32_t polled_events;
static int32_t pause_transitions;
static int32_t last_pause_value;
static int32_t shutdowns;
static int32_t next_bind_mode;
static int32_t record_step_order;
static int32_t step_order;
static int32_t applied_seq;
static int32_t applied_flags;
static int32_t applied_width;
static int32_t applied_height;
static int32_t submit_tick_marker;
static int32_t submit_render_score;

static int32_t hash_path(const char *text) {
    uint32_t hash = 2166136261u;
    while (*text != '\0') {
        hash ^= (uint8_t)*text++;
        hash *= 16777619u;
    }
    return (int32_t)hash;
}

int stasis_init_window(int width, int height, const char *title) {
    return width == 320 && height == 180 && title != NULL;
}
int stasis_should_quit(void) { return 0; }
int stasis_mobile_poll_events(void) { polled_events += 1; return 0; }
void stasis_mobile_set_paused(int paused) {
    pause_transitions += 1;
    last_pause_value = paused;
}
void stasis_host_get_frame(int32_t *out_i32, float *out_f32) {
    if (record_step_order) step_order = step_order * 10 + 1;
    out_i32[10] += 1;
    out_i32[100] = 77;
    out_f32[50] = 320.0f;
    out_f32[51] = 180.0f;
}
void stasis_host_bulk_apply_requests(
    const int32_t *seq,
    const int32_t *flags,
    const int32_t *width,
    const int32_t *height
) {
    if (record_step_order) step_order = step_order * 10 + 2;
    applied_seq = *seq;
    applied_flags = *flags;
    applied_width = *width;
    applied_height = *height;
}
void stasis_gfx_submit_u8(int32_t *i32s, const float *f32s, const uint8_t *u8s) {
    if (record_step_order) step_order = step_order * 10 + 3;
    submitted_frames += 1;
    submitted_trace = stasis_render_trace(i32s, f32s, u8s);
    submitted_rects = i32s[STASIS_RENDER_I_RECT_COUNT];
    submit_tick_marker = stasis_jit_global_i32_load(hash_path("tick_host_marker"));
    submit_render_score = stasis_jit_global_i32_load(hash_path("render_score"));
}
uint64_t stasis_host_performance_counter(void) { return 100; }
int stasis_host_performance_metrics_enabled(void) { return 1; }
uint64_t stasis_host_performance_elapsed_us(uint64_t started, uint64_t finished) {
    return finished - started;
}
void stasis_host_set_performance_metrics(uint64_t tick_us, uint64_t render_us) {
    (void)tick_us;
    (void)render_us;
}

void stasis_host_log_message(const char *message) { (void)message; }
void stasis_shutdown(void) { shutdowns += 1; }

int stasis_audio_init(int rate, int channels, int latency) {
    return rate > 0 && channels > 0 && latency > 0;
}
void stasis_audio_shutdown(void) {}
int stasis_audio_is_available(void) { return 0; }
int stasis_audio_get_sample_rate(void) { return 48000; }
int stasis_audio_get_channels(void) { return 2; }
int stasis_audio_get_queued_frames(void) { return 0; }
int stasis_audio_get_underruns(void) { return 0; }
int stasis_audio_push_f32_interleaved(const float *samples, int frames) {
    return samples == NULL ? 0 : frames;
}
int stasis_audio_load_wav(const char *path) { return path == NULL ? 0 : 1; }
void stasis_audio_release(int handle) { (void)handle; }
int stasis_audio_play(int handle, int loop, float volume, float pan) {
    (void)loop; (void)volume; (void)pan; return handle;
}
void stasis_audio_stop(int handle) { (void)handle; }
int stasis_audio_voice_is_playing(int handle) { return handle > 0; }
void stasis_audio_voice_set_paused(int handle, int paused) { (void)handle; (void)paused; }
void stasis_audio_voice_set_volume_pan(int handle, float volume, float pan) {
    (void)handle; (void)volume; (void)pan;
}
int stasis_audio_load_music(const char *path) { return path == NULL ? 0 : 1; }
int stasis_audio_load_effect(const char *path) { return path == NULL ? 0 : 1; }
int stasis_audio_play_music(int handle, int loop, float volume) {
    (void)loop; (void)volume; return handle;
}
void stasis_audio_stop_music(int handle) { (void)handle; }
void stasis_audio_pause_music(int handle, int paused) { (void)handle; (void)paused; }
void stasis_audio_set_music_volume(int handle, float volume) { (void)handle; (void)volume; }
int stasis_audio_play_effect(int handle, float volume) { (void)volume; return handle; }
int stasis_gfx_load_sprite(const char *path, int max_w, int max_h) {
    return path != NULL && max_w > 0 && max_h > 0;
}
void stasis_gfx_release_sprite(int handle) { (void)handle; }
int stasis_gfx_dump_bmp(const char *path) { return path != NULL; }
int stasis_gfx_dump_png(const char *path) { return path != NULL; }
int stasis_gfx_cache_text(int font, const char *text) { return font + (text != NULL); }
int stasis_gfx_poll_reload(int handle) { return handle; }
float stasis_gfx_measure_text_cached(int handle) { return (float)handle; }
float stasis_gfx_measure_text_cached_height(int handle) { return (float)handle; }
int stasis_load_font(const char *path, int size) { return path == NULL ? 0 : size; }
float stasis_measure_text(int font, const char *text) { return text == NULL ? 0.0f : (float)font; }
void stasis_sleep_ms(int ms) { (void)ms; }
int stasis_storage_load_i32(const char *scope, const char *key, int fallback) {
    (void)scope; (void)key; return fallback;
}
int stasis_storage_save_i32(const char *scope, const char *key, int value) {
    (void)value; return scope != NULL && key != NULL;
}
int stasis_storage_load_ascii(const char *scope, const char *key, char *out, int capacity) {
    (void)scope; (void)key; (void)out; (void)capacity; return 0;
}
int stasis_storage_save_ascii(const char *scope, const char *key, const char *value, int length) {
    (void)value; (void)length; return scope != NULL && key != NULL;
}
int stasis_clipboard_load_ascii(char *out, int capacity) { (void)out; (void)capacity; return 0; }
int stasis_clipboard_save_ascii(const char *value, int length) {
    return value != NULL && length >= 0;
}

static void bind_runtime_with_mode(void) {
    stasis_aot_bind_runtime_globals();
    stasis_jit_global_i32_store(hash_path("lifecycle_mode"), next_bind_mode);
}

static void reset_frame_observations(void) {
    submitted_frames = 0;
    submitted_trace = 0;
    submitted_rects = 0;
}

int main(void) {
    const StasisMobileRuntimeConfig config = {320, 180, "IT-012 generated mobile AOT"};
    const StasisMobileGameEntries entries = {
        bind_runtime_with_mode,
        stasis_mobile_main_entry,
        stasis_mobile_tick_entry,
        stasis_mobile_render_entry
    };
    next_bind_mode = 0;
    CHECK(stasis_mobile_runtime_initialize(&config, &entries) == STASIS_MOBILE_RUNTIME_OK);
    CHECK(stasis_jit_global_i32_load(hash_path("score")) == 10);
    CHECK(stasis_jit_global_i32_load(hash_path("entry_trace")) == 1);
    CHECK(stasis_jit_global_i32_array_load(hash_path("host_i32"), 0, 10) == 0);
    stasis_mobile_runtime_set_paused(1);
    CHECK(pause_transitions == 1);
    CHECK(last_pause_value == 1);
    CHECK(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_OK);
    CHECK(polled_events == 1);
    CHECK(stasis_jit_global_i32_load(hash_path("entry_trace")) == 1);
    CHECK(stasis_jit_global_i32_load(hash_path("score")) == 10);
    CHECK(submitted_frames == 0);
    stasis_mobile_runtime_set_paused(0);
    CHECK(pause_transitions == 2);
    CHECK(last_pause_value == 0);
    stasis_jit_global_i32_store(hash_path("host_req_seq"), 41);
    stasis_jit_global_i32_store(hash_path("host_req_flags"), 5);
    stasis_jit_global_i32_store(hash_path("host_req_window_w_px"), 640);
    stasis_jit_global_i32_store(hash_path("host_req_window_h_px"), 360);
    record_step_order = 1;
    CHECK(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_OK);
    record_step_order = 0;
    CHECK(stasis_jit_global_i32_load(hash_path("score")) == 15);
    CHECK(stasis_jit_global_i32_load(hash_path("entry_trace")) == 123);
    CHECK(stasis_jit_global_i32_load(hash_path("tick_host_marker")) == 77);
    CHECK(stasis_jit_global_i32_load(hash_path("render_score")) == 15);
    CHECK(step_order == 123);
    CHECK(applied_seq == 41 && applied_flags == 5);
    CHECK(applied_width == 640 && applied_height == 360);
    CHECK(submit_tick_marker == 77 && submit_render_score == 15);
    CHECK(submitted_frames == 1);
    CHECK(submitted_rects == 1);
    CHECK(submitted_trace == IT012_EXPECTED_TRACE);
    printf("stasis.seam_test.v1 IT-012 state=15 frames=1 rects=1 trace=%u\n", submitted_trace);
    printf("stasis.seam_test.v1 IT-014 order=123 marker=77 request=41:5:640:360 render_score=15 frames=1\n");
    stasis_mobile_runtime_shutdown();

    CHECK(stasis_mobile_runtime_is_initialized() == 0);
    CHECK(stasis_mobile_runtime_last_entry_result() == 0);
    CHECK(shutdowns == 1);
    reset_frame_observations();
    CHECK(stasis_mobile_runtime_initialize(&config, &entries) == STASIS_MOBILE_RUNTIME_OK);
    CHECK(stasis_jit_global_i32_load(hash_path("score")) == 10);
    CHECK(stasis_jit_global_i32_load(hash_path("entry_trace")) == 1);
    CHECK(stasis_jit_global_i32_array_load(hash_path("host_i32"), 0, 10) == 0);
    CHECK(submitted_frames == 0);
    stasis_mobile_runtime_shutdown();

    next_bind_mode = 2;
    reset_frame_observations();
    CHECK(stasis_mobile_runtime_initialize(&config, &entries) == STASIS_MOBILE_RUNTIME_OK);
    CHECK(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    CHECK(stasis_mobile_runtime_last_entry_result() == 22);
    CHECK(stasis_jit_global_i32_load(hash_path("entry_trace")) == 12);
    CHECK(submitted_frames == 0);
    stasis_mobile_runtime_shutdown();

    next_bind_mode = 3;
    reset_frame_observations();
    CHECK(stasis_mobile_runtime_initialize(&config, &entries) == STASIS_MOBILE_RUNTIME_OK);
    CHECK(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    CHECK(stasis_mobile_runtime_last_entry_result() == 33);
    CHECK(stasis_jit_global_i32_load(hash_path("entry_trace")) == 123);
    CHECK(submitted_frames == 0);
    stasis_mobile_runtime_shutdown();

    next_bind_mode = 1;
    reset_frame_observations();
    CHECK(stasis_mobile_runtime_initialize(&config, &entries) == STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    CHECK(stasis_mobile_runtime_last_entry_result() == 11);
    CHECK(stasis_jit_global_i32_load(hash_path("entry_trace")) == 1);
    CHECK(submitted_frames == 0);
    stasis_mobile_runtime_shutdown();
    CHECK(shutdowns == 5);
    printf("stasis.seam_test.v1 IT-013 order=123 paused_poll=1 reinit=1 main_stop=11 tick_stop=22 render_stop=33 frames_after_failures=0\n");
    return 0;
}
