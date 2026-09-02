#include "stasis_mobile_runtime.h"
#include "stasis_mobile_aot_runtime.h"
#include "stasis_render_contract.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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
static int32_t submitted_text_count;
static int32_t submitted_text_bytes_used;
static int32_t submitted_text_font;
static int32_t submitted_text_offset;
static int32_t submitted_text_length;
static uint8_t submitted_text_bytes[STASIS_RENDER_TEXT_MAX_BYTES];
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
    submitted_text_count = i32s[STASIS_RENDER_I_TEXT_COUNT];
    submitted_text_bytes_used = i32s[STASIS_RENDER_I_TEXT_BYTES_USED];
    if (submitted_text_count > 0) {
        const int32_t base = STASIS_RENDER_I_TEXT_BASE;
        submitted_text_font = i32s[base + 0];
        submitted_text_offset = i32s[base + 1];
        submitted_text_length = i32s[base + 2];
    }
    if (submitted_text_bytes_used > 0 && submitted_text_bytes_used <= STASIS_RENDER_TEXT_MAX_BYTES) {
        memcpy(submitted_text_bytes, u8s, (size_t)submitted_text_bytes_used);
    }
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
int stasis_asset_request_sprite(const char *path, int max_w, int max_h) { return path && max_w && max_h ? 31 : 0; }
int stasis_asset_request_audio(const char *path) { return path ? 32 : 0; }
int stasis_asset_task_poll(int task) { return task > 0 ? 3 : 0; }
int stasis_asset_task_take_handle(int task) { return task > 0 ? 33 : 0; }
void stasis_asset_task_cancel(int task) { (void)task; }
void stasis_gfx_release_sprite(int handle) { (void)handle; }
int stasis_gfx_dump_bmp(const char *path) { return path != NULL; }
int stasis_gfx_dump_png(const char *path) { return path != NULL; }
int stasis_gfx_cache_text(int font, const char *text) { return font + (text != NULL); }
int stasis_gfx_replace_text(int handle, int font, const char *text) { return handle > 0 ? handle : font + (text != NULL); }
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
    submitted_text_count = 0;
    submitted_text_bytes_used = 0;
    submitted_text_font = 0;
    submitted_text_offset = 0;
    submitted_text_length = 0;
    memset(submitted_text_bytes, 0, sizeof(submitted_text_bytes));
}

static uint32_t it012_expected_frame_trace(void) {
    int32_t *expected_i32 = calloc(
        (size_t)STASIS_RENDER_I32_COUNT, sizeof(*expected_i32));
    float *expected_f32 = calloc(
        (size_t)STASIS_RENDER_F32_COUNT, sizeof(*expected_f32));
    uint8_t *expected_u8 = calloc(
        (size_t)STASIS_RENDER_U8_COUNT, sizeof(*expected_u8));
    if (expected_i32 == NULL || expected_f32 == NULL || expected_u8 == NULL) {
        fprintf(stderr, "IT-012 could not allocate semantic expected frame\n");
        free(expected_i32);
        free(expected_f32);
        free(expected_u8);
        exit(1);
    }

    expected_i32[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_MAGIC;
    expected_i32[STASIS_RENDER_I_VERSION] = STASIS_RENDER_VERSION;
    expected_i32[STASIS_RENDER_I_FLAGS] =
        STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT;
    expected_i32[STASIS_RENDER_I_RECT_COUNT] = 1;
    expected_i32[STASIS_RENDER_I_TEXT_COUNT] = 1;
    expected_i32[STASIS_RENDER_I_TEXT_BYTES_USED] = 6;
    expected_i32[STASIS_RENDER_I_ORDER_COUNT] = 2;

    expected_f32[STASIS_RENDER_F_CLEAR_BASE + 0] = 0.05f;
    expected_f32[STASIS_RENDER_F_CLEAR_BASE + 1] = 0.10f;
    expected_f32[STASIS_RENDER_F_CLEAR_BASE + 2] = 0.15f;
    expected_f32[STASIS_RENDER_F_CLEAR_BASE + 3] = 1.0f;

    const int32_t rect_base = STASIS_RENDER_F_RECT_REVERSE_BASE;
    expected_f32[rect_base + 0] = 12.0f;
    expected_f32[rect_base + 1] = 14.0f;
    expected_f32[rect_base + 2] = 30.0f;
    expected_f32[rect_base + 3] = 18.0f;
    expected_f32[rect_base + 4] = 0.8f;
    expected_f32[rect_base + 5] = 0.2f;
    expected_f32[rect_base + 6] = 0.1f;
    expected_f32[rect_base + 7] = 1.0f;

    const int32_t text_i32_base = STASIS_RENDER_I_TEXT_BASE;
    expected_i32[text_i32_base + 0] = 7;
    expected_i32[text_i32_base + 1] = 0;
    expected_i32[text_i32_base + 2] = 5;
    const int32_t text_f32_base = STASIS_RENDER_F_TEXT_BASE;
    expected_f32[text_f32_base + 0] = 12.0f;
    expected_f32[text_f32_base + 1] = 14.0f;
    expected_f32[text_f32_base + 2] = 0.8f;
    expected_f32[text_f32_base + 3] = 0.2f;
    expected_f32[text_f32_base + 4] = 0.1f;
    expected_f32[text_f32_base + 5] = 1.0f;
    const uint8_t text_bytes[] = {'C', 'a', 'f', 0xc3u, 0xa9u, 0};
    memcpy(expected_u8, text_bytes, sizeof(text_bytes));

    expected_i32[STASIS_RENDER_I_ORDER_BASE + 0] =
        STASIS_RENDER_ORDER_RECT * STASIS_RENDER_ORDER_KIND_SCALE;
    expected_i32[STASIS_RENDER_I_ORDER_BASE + 1] =
        STASIS_RENDER_ORDER_TEXT * STASIS_RENDER_ORDER_KIND_SCALE;

    const StasisRenderValidation validation =
        stasis_render_validate(expected_i32, expected_f32);
    if (validation != STASIS_RENDER_VALID) {
        fprintf(
            stderr,
            "IT-012 semantic expected frame is invalid: %s\n",
            stasis_render_validation_name(validation));
        free(expected_i32);
        free(expected_f32);
        free(expected_u8);
        exit(1);
    }
    const uint32_t trace =
        stasis_render_trace(expected_i32, expected_f32, expected_u8);
    free(expected_i32);
    free(expected_f32);
    free(expected_u8);
    return trace;
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
    CHECK(submitted_text_count == 1);
    CHECK(submitted_text_bytes_used == 6);
    CHECK(submitted_text_font == 7);
    CHECK(submitted_text_offset == 0);
    CHECK(submitted_text_length == 5);
    CHECK(submitted_text_bytes[0] == 67);
    CHECK(submitted_text_bytes[1] == 97);
    CHECK(submitted_text_bytes[2] == 102);
    CHECK(submitted_text_bytes[3] == 195);
    CHECK(submitted_text_bytes[4] == 169);
    CHECK(submitted_text_bytes[5] == 0);
    CHECK(stasis_jit_global_i32_load(hash_path("forwarded_byte_length")) == 5);
    CHECK(stasis_jit_global_i32_load(hash_path("forwarded_char_length")) == 4);
    const uint32_t expected_trace = it012_expected_frame_trace();
    if (submitted_trace != expected_trace) {
        fprintf(
            stderr,
            "IT-012 semantic trace mismatch: expected=%u actual=%u\n",
            expected_trace,
            submitted_trace);
    }
    CHECK(submitted_trace == expected_trace);
    printf("stasis.seam_test.v1 IT-012 state=15 frames=1 rects=1 texts=1 bytes=5 chars=4 trace=%u\n", submitted_trace);
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
