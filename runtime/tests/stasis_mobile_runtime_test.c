#include "stasis_mobile_runtime.h"

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Keep lifecycle checks active in Release builds where NDEBUG removes assert. */
#undef assert
#define assert(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static int init_window_result = 1;
static int should_quit_result;
static int init_window_calls;
static int should_quit_calls;
static int mobile_poll_events_calls;
static int mobile_set_paused_calls;
static int mobile_paused;
static int bind_runtime_calls;
static int main_calls;
static int tick_calls;
static int render_calls;
static int32_t main_result;
static int32_t tick_result;
static int32_t render_result;
static int begin_frame_calls;
static int end_frame_calls;
static int host_frame_calls;
static int host_request_calls;
static int32_t host_request_sequences[8];
static int gfx_submit_calls;
static int shutdown_calls;

int stasis_init_window(int width, int height, const char *title) {
    assert(width == 1280);
    assert(height == 720);
    assert(title != NULL);
    init_window_calls += 1;
    return init_window_result;
}

int stasis_should_quit(void) {
    should_quit_calls += 1;
    return should_quit_result;
}

int stasis_mobile_poll_events(void) {
    mobile_poll_events_calls += 1;
    return should_quit_result;
}

void stasis_mobile_set_paused(int paused) {
    mobile_set_paused_calls += 1;
    mobile_paused = paused;
}

void stasis_begin_frame(void) {
    begin_frame_calls += 1;
}

void stasis_end_frame(void) {
    end_frame_calls += 1;
}

void stasis_host_get_frame(int32_t *out_i32, float *out_f32) {
    assert(out_i32 != NULL);
    assert(out_f32 != NULL);
    host_frame_calls += 1;
}

void stasis_host_bulk_apply_requests(
    const int32_t *seq,
    const int32_t *flags,
    const int32_t *width,
    const int32_t *height
) {
    assert(seq != NULL && flags != NULL && width != NULL && height != NULL);
    if (host_request_calls < 8) host_request_sequences[host_request_calls] = *seq;
    host_request_calls += 1;
}

void stasis_gfx_submit_u8(
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8
) {
    assert(cmd_i32 != NULL && cmd_f32 != NULL && cmd_u8 != NULL);
    gfx_submit_calls += 1;
}

void stasis_shutdown(void) {
    shutdown_calls += 1;
}

int stasis_audio_init(int rate, int channels, int latency) { return rate + channels + latency; }
void stasis_audio_shutdown(void) {}
int stasis_audio_is_available(void) { return 1; }
int stasis_audio_get_sample_rate(void) { return 48000; }
int stasis_audio_get_channels(void) { return 2; }
int stasis_audio_get_queued_frames(void) { return 0; }
int stasis_audio_get_underruns(void) { return 0; }
int stasis_audio_push_f32_interleaved(const float *samples, int frames) { return samples ? frames : 0; }
int stasis_gfx_load_sprite(const char *path, int max_w, int max_h) { return path && max_w && max_h; }
void stasis_gfx_release_sprite(int handle) { (void)handle; }
int stasis_gfx_dump_bmp(const char *path) { return path != NULL; }
int stasis_gfx_dump_png(const char *path) { return path != NULL; }
int stasis_gfx_cache_text(int font, const char *text) { return font + (text != NULL); }
int stasis_gfx_poll_reload(int handle) { return handle; }
float stasis_gfx_measure_text_cached(int handle) { return (float)handle; }
int stasis_load_font(const char *path, int size) { return path ? size : 0; }
float stasis_measure_text(int font, const char *text) { return text ? (float)font : 0.0f; }
void stasis_sleep_ms(int ms) { (void)ms; }

static int32_t hash_path(const char *path);

static int32_t game_main(void) {
    main_calls += 1;
    stasis_jit_global_i32_store(hash_path("host_req_seq"), 1);
    stasis_jit_global_i32_store(hash_path("host_req_flags"), 1);
    stasis_jit_global_i32_store(hash_path("host_req_window_w_px"), 960);
    stasis_jit_global_i32_store(hash_path("host_req_window_h_px"), 540);
    return main_result;
}

static void bind_runtime(void) {
    bind_runtime_calls += 1;
}

static int32_t game_tick(void) {
    tick_calls += 1;
    return tick_result;
}

static int32_t game_render(void) {
    render_calls += 1;
    return render_result;
}

static StasisMobileRuntimeConfig config(void) {
    StasisMobileRuntimeConfig value = {1280, 720, "Mobile test"};
    return value;
}

static StasisMobileGameEntries entries(void) {
    StasisMobileGameEntries value = {bind_runtime, game_main, game_tick, game_render};
    return value;
}

static int32_t hash_path(const char *path) {
    uint32_t hash = 2166136261U;
    while (*path != '\0') {
        hash ^= (uint8_t)*path++;
        hash *= 16777619U;
    }
    return (int32_t)hash;
}

static void reset_fakes(void) {
    init_window_result = 1;
    should_quit_result = 0;
    init_window_calls = 0;
    should_quit_calls = 0;
    mobile_poll_events_calls = 0;
    mobile_set_paused_calls = 0;
    mobile_paused = 0;
    bind_runtime_calls = 0;
    main_calls = 0;
    tick_calls = 0;
    render_calls = 0;
    main_result = 0;
    tick_result = 0;
    render_result = 0;
    begin_frame_calls = 0;
    end_frame_calls = 0;
    host_frame_calls = 0;
    host_request_calls = 0;
    memset(host_request_sequences, 0, sizeof(host_request_sequences));
    gfx_submit_calls = 0;
    shutdown_calls = 0;
}

static void test_rejects_invalid_configuration(void) {
    StasisMobileRuntimeConfig valid_config = config();
    StasisMobileGameEntries valid_entries = entries();
    StasisMobileGameEntries missing_render = valid_entries;
    missing_render.render_entry = NULL;

    assert(stasis_mobile_runtime_initialize(NULL, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT);
    assert(stasis_mobile_runtime_initialize(&valid_config, &missing_render) ==
        STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT);
    assert(init_window_calls == 0);
}

static void test_runs_mobile_lifecycle(void) {
    StasisMobileRuntimeConfig valid_config = config();
    StasisMobileGameEntries valid_entries = entries();

    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) == 0);
    assert(stasis_mobile_runtime_is_initialized() == 1);
    assert(init_window_calls == 1);
    assert(bind_runtime_calls == 1);
    assert(main_calls == 1);
    assert(host_request_calls == 2);
    assert(host_request_sequences[0] == 0);
    assert(host_request_sequences[1] == 1);

    assert(stasis_mobile_runtime_step() == 0);
    assert(should_quit_calls == 1);
    assert(tick_calls == 1);
    assert(render_calls == 1);
    assert(host_frame_calls == 1);
    assert(host_request_calls == 3);
    assert(gfx_submit_calls == 1);
    assert(begin_frame_calls == 1);
    assert(end_frame_calls == 1);

    stasis_mobile_runtime_set_paused(1);
    assert(mobile_set_paused_calls == 1);
    assert(mobile_paused == 1);
    assert(stasis_mobile_runtime_step() == 0);
    assert(should_quit_calls == 1);
    assert(mobile_poll_events_calls == 1);
    assert(tick_calls == 1);
    assert(render_calls == 1);

    stasis_mobile_runtime_set_paused(0);
    assert(mobile_set_paused_calls == 2);
    assert(mobile_paused == 0);
    should_quit_result = 1;
    assert(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    assert(should_quit_calls == 2);
    assert(tick_calls == 1);

    stasis_mobile_runtime_shutdown();
    stasis_mobile_runtime_shutdown();
    assert(shutdown_calls == 1);
    assert(stasis_mobile_runtime_is_initialized() == 0);
    assert(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_NOT_INITIALIZED);
}

static void test_rejects_duplicate_initialization(void) {
    StasisMobileRuntimeConfig valid_config = config();
    StasisMobileGameEntries valid_entries = entries();

    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) == 0);
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_ALREADY_INITIALIZED);
    assert(init_window_calls == 1);
    assert(main_calls == 1);
    stasis_jit_global_i32_array_store(hash_path("host_i32"), 0, 0, 77);
    stasis_mobile_runtime_shutdown();
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) == 0);
    assert(stasis_jit_global_i32_array_load(hash_path("host_i32"), 0, 0) == 0);
    assert(main_calls == 2);
    stasis_mobile_runtime_shutdown();
}

static void test_reports_graphics_initialization_failure(void) {
    StasisMobileRuntimeConfig valid_config = config();
    StasisMobileGameEntries valid_entries = entries();

    init_window_result = 0;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_GRAPHICS_UNAVAILABLE);
    assert(main_calls == 0);
    assert(shutdown_calls == 0);

    init_window_result = 1;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_OK);
    assert(main_calls == 1);
    stasis_mobile_runtime_shutdown();
}

static void test_stops_on_nonzero_game_entry_results(void) {
    StasisMobileRuntimeConfig valid_config = config();
    StasisMobileGameEntries valid_entries = entries();

    main_result = 7;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    assert(stasis_mobile_runtime_last_entry_result() == 7);
    assert(tick_calls == 0 && render_calls == 0);
    stasis_mobile_runtime_shutdown();

    reset_fakes();
    tick_result = 8;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_OK);
    assert(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    assert(stasis_mobile_runtime_last_entry_result() == 8);
    assert(tick_calls == 1 && render_calls == 0);
    assert(begin_frame_calls == 0 && end_frame_calls == 0 && gfx_submit_calls == 0);
    stasis_mobile_runtime_shutdown();

    reset_fakes();
    render_result = 9;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_OK);
    assert(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    assert(stasis_mobile_runtime_last_entry_result() == 9);
    assert(tick_calls == 1 && render_calls == 1);
    assert(begin_frame_calls == 1 && end_frame_calls == 1 && gfx_submit_calls == 0);
    stasis_mobile_runtime_shutdown();
}

int main(void) {
    reset_fakes();
    test_rejects_invalid_configuration();
    test_runs_mobile_lifecycle();
    reset_fakes();
    test_rejects_duplicate_initialization();
    reset_fakes();
    test_reports_graphics_initialization_failure();
    reset_fakes();
    test_stops_on_nonzero_game_entry_results();
    puts("stasis_mobile_runtime_test: ok");
    return 0;
}
