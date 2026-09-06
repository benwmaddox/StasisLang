#include "stasis_mobile_runtime.h"
#include "stasis_mobile_aot_runtime.h"
#include "stasis_render_contract.h"

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
static uint64_t performance_counter;
static int performance_metrics_enabled = 1;
static int performance_metrics_calls;
static uint64_t reported_tick_us;
static uint64_t reported_render_us;
static int profile_start_logs;
static int profile_row_logs;
static int profile_done_logs;
static char profile_row[256];
static int32_t game_host_i32[768];
static float game_host_f32[64];
static int32_t game_gfx_cmd_i32[STASIS_RENDER_I32_COUNT];
static float game_gfx_cmd_f32[STASIS_RENDER_F32_COUNT];
static uint8_t game_gfx_cmd_u8[STASIS_RENDER_U8_COUNT];
static int32_t game_host_req_seq;
static int32_t game_host_req_flags;
static int32_t game_host_req_window_w_px;
static int32_t game_host_req_window_h_px;
static char game_string_literals[640][32];

#if defined(STASIS_NETWORK_ENABLED)
typedef struct StasisNetworkHost {
    int active;
} StasisNetworkHost;
typedef struct StasisNetworkEvent {
    uint32_t kind;
    uint32_t connection;
    uint32_t length;
    unsigned char payload[64u * 1024u];
} StasisNetworkEvent;

static StasisNetworkHost network_host;
static int network_start_result = 1;
static int network_start_calls;
static int network_stop_calls;

int32_t stasis_network_supported(void) { return 1; }
int32_t stasis_network_random_seed(void) { return 1234; }
StasisNetworkHost *stasis_network_host_start_bind(
    uint16_t port,
    uint32_t bind_ipv4,
    const unsigned char *bundle,
    size_t bundle_length,
    uint16_t *out_port
) {
    static const unsigned char fixture[] = "network lifecycle fixture";
    assert(port == 0);
    assert(bind_ipv4 == 0);
    assert(bundle != NULL && bundle_length == sizeof(fixture) - 1);
    assert(memcmp(bundle, fixture, sizeof(fixture) - 1) == 0);
    assert(out_port != NULL);
    network_start_calls += 1;
    if (!network_start_result) return NULL;
    *out_port = 4312;
    network_host.active = 1;
    return &network_host;
}
int32_t stasis_network_host_poll(StasisNetworkHost *host, StasisNetworkEvent *event) {
    (void)host; (void)event; return 0;
}
int32_t stasis_network_host_send(
    StasisNetworkHost *host,
    uint32_t connection,
    const unsigned char *payload,
    size_t length
) {
    (void)host; (void)connection; (void)payload; (void)length; return 0;
}
int32_t stasis_network_host_status(StasisNetworkHost *host) {
    return host != NULL && host->active;
}
uint32_t stasis_network_host_overflow_count(StasisNetworkHost *host) {
    (void)host; return 0;
}
uint16_t stasis_network_host_port(StasisNetworkHost *host) {
    return host != NULL && host->active ? 4312 : 0;
}
int32_t stasis_network_host_copy_join_card(
    StasisNetworkHost *host,
    char *out,
    size_t capacity,
    size_t *out_length
) {
    const char *card = "http://192.0.2.20:4312/";
    size_t length = strlen(card);
    if (host == NULL || !host->active || capacity <= length || out_length == NULL) return -1;
    memcpy(out, card, length + 1);
    *out_length = length;
    return 0;
}
int32_t stasis_network_host_copy_join_url(
    StasisNetworkHost *host,
    char *out,
    size_t capacity,
    size_t *out_length
) {
    const char *url = "http://192.0.2.20:4312/session?pair=private";
    size_t length = strlen(url);
    if (host == NULL || !host->active || capacity <= length || out_length == NULL) return -1;
    memcpy(out, url, length + 1);
    *out_length = length;
    return 0;
}
void stasis_network_host_stop(StasisNetworkHost *host) {
    assert(host == &network_host && host->active);
    network_stop_calls += 1;
    host->active = 0;
}

static void configure_network_fixture(void) {
    static const unsigned char fixture[] = "network lifecycle fixture";
    FILE *file = fopen("network_guest.bundle", "wb");
    assert(file != NULL);
    assert(fwrite(fixture, 1, sizeof(fixture) - 1, file) == sizeof(fixture) - 1);
    assert(fclose(file) == 0);
#if defined(_WIN32)
    assert(_putenv_s("STASIS_ASSET_ROOT", ".") == 0);
#else
    assert(setenv("STASIS_ASSET_ROOT", ".", 1) == 0);
#endif
}
#endif

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
    assert(out_i32 == game_host_i32);
    assert(out_f32 == game_host_f32);
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
    int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8
) {
    assert(cmd_i32 != NULL && cmd_f32 != NULL && cmd_u8 != NULL);
    assert(cmd_i32 == game_gfx_cmd_i32);
    assert(cmd_f32 == game_gfx_cmd_f32);
    assert(cmd_u8 == game_gfx_cmd_u8);
    assert(cmd_i32[STASIS_RENDER_I_MAGIC] == STASIS_RENDER_MAGIC);
    assert(cmd_i32[STASIS_RENDER_I_VERSION] == STASIS_RENDER_VERSION);
    gfx_submit_calls += 1;
    stasis_begin_frame();
    stasis_end_frame();
}

void stasis_shutdown(void) {
    shutdown_calls += 1;
}

uint64_t stasis_host_performance_counter(void) {
    performance_counter += 10;
    return performance_counter;
}

int stasis_host_performance_metrics_enabled(void) {
    return performance_metrics_enabled;
}

uint64_t stasis_host_performance_elapsed_us(uint64_t started, uint64_t finished) {
    return finished - started;
}

void stasis_host_set_performance_metrics(uint64_t tick_us, uint64_t render_us) {
    performance_metrics_calls += 1;
    reported_tick_us = tick_us;
    reported_render_us = render_us;
}

void stasis_host_log_message(const char *message) {
    if (strncmp(message, "STASIS_PROFILE_START|", 21) == 0) profile_start_logs += 1;
    if (strncmp(message, "STASIS_PROFILE|", 15) == 0) {
        profile_row_logs += 1;
        snprintf(profile_row, sizeof(profile_row), "%s", message);
    }
    if (strncmp(message, "STASIS_PROFILE_DONE|", 20) == 0) profile_done_logs += 1;
}

int stasis_audio_init(int rate, int channels, int latency) { return rate + channels + latency; }
void stasis_audio_shutdown(void) {}
int stasis_audio_is_available(void) { return 1; }
int stasis_audio_get_sample_rate(void) { return 48000; }
int stasis_audio_get_channels(void) { return 2; }
int stasis_audio_get_queued_frames(void) { return 0; }
int stasis_audio_get_underruns(void) { return 0; }
int stasis_audio_push_f32_interleaved(const float *samples, int frames) { return samples ? frames : 0; }
int stasis_audio_load_wav(const char *path) { return path ? 1 : 0; }
void stasis_audio_release(int asset_handle) { (void)asset_handle; }
int stasis_audio_play(int asset_handle, int loop, float volume, float pan) {
    return asset_handle + loop + (int)volume + (int)pan;
}
void stasis_audio_stop(int voice_handle) { (void)voice_handle; }
int stasis_audio_voice_is_playing(int voice_handle) { return voice_handle > 0; }
void stasis_audio_voice_set_paused(int voice_handle, int paused) { (void)voice_handle; (void)paused; }
void stasis_audio_voice_set_volume_pan(int voice_handle, float volume, float pan) {
    (void)voice_handle; (void)volume; (void)pan;
}
int stasis_audio_load_music(const char *path) { return path ? 2 : 0; }
int stasis_audio_load_effect(const char *path) { return path ? 3 : 0; }
int stasis_audio_play_music(int asset_handle, int loop, float volume) {
    return asset_handle + loop + (int)volume;
}
void stasis_audio_stop_music(int asset_handle) { (void)asset_handle; }
void stasis_audio_pause_music(int asset_handle, int paused) { (void)asset_handle; (void)paused; }
void stasis_audio_set_music_volume(int asset_handle, float volume) {
    (void)asset_handle; (void)volume;
}
int stasis_audio_play_effect(int asset_handle, float volume) {
    return asset_handle + (int)volume;
}
int stasis_gfx_load_sprite(const char *path, int max_w, int max_h) { return path && max_w && max_h; }
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
float stasis_gfx_measure_text_cached_height(int handle) { return (float)handle + 1.0f; }
int stasis_load_font(const char *path, int size) { return path ? size : 0; }
float stasis_measure_text(int font, const char *text) { return text ? (float)font : 0.0f; }
void stasis_sleep_ms(int ms) { (void)ms; }
int stasis_storage_load_i32(const char *scope, const char *key, int fallback) {
    return scope && key ? fallback : 0;
}
int stasis_storage_save_i32(const char *scope, const char *key, int value) {
    (void)value;
    return scope && key;
}
int stasis_storage_load_ascii(const char *scope, const char *key, char *out, int capacity) {
    (void)scope; (void)key; (void)out; (void)capacity;
    return -1;
}
int stasis_storage_save_ascii(const char *scope, const char *key, const char *value, int length) {
    (void)value; (void)length;
    return scope && key;
}
int stasis_clipboard_load_ascii(char *out, int capacity) {
    (void)out; (void)capacity;
    return -1;
}
int stasis_clipboard_save_ascii(const char *value, int length) {
    (void)value; (void)length;
    return 1;
}

static int32_t hash_path(const char *path);

static int32_t game_main(void) {
#if defined(STASIS_NETWORK_ENABLED)
    assert(network_host.active == 1);
#endif
    main_calls += 1;
    assert(stasis_jit_load_font(1639, 19) == 19);
    game_host_req_seq = 1;
    game_host_req_flags = 1;
    game_host_req_window_w_px = 960;
    game_host_req_window_h_px = 540;
    return main_result;
}

static void bind_runtime(void) {
    int literal_index;
    bind_runtime_calls += 1;
    stasis_jit_clear_string_literal_table();
    for (literal_index = 0; literal_index < 640; literal_index += 1) {
        snprintf(game_string_literals[literal_index], sizeof(game_string_literals[literal_index]),
            "asset/path/%d.ttf", literal_index);
        stasis_jit_upsert_string_literal(1000 + literal_index, game_string_literals[literal_index]);
    }
    stasis_jit_register_global_i32_array(hash_path("host_i32"), 0, game_host_i32, 768);
    stasis_jit_register_global_f32_array(hash_path("host_f32"), 0, game_host_f32, 64);
    stasis_jit_register_global_i32_array(
        hash_path("gfx_cmd_i32"), 0, game_gfx_cmd_i32, STASIS_RENDER_I32_COUNT);
    stasis_jit_register_global_f32_array(
        hash_path("gfx_cmd_f32"), 0, game_gfx_cmd_f32, STASIS_RENDER_F32_COUNT);
    stasis_jit_register_global_u8_array(
        hash_path("gfx_cmd_u8"), 0, game_gfx_cmd_u8, STASIS_RENDER_U8_COUNT);
    stasis_jit_register_global_i32_ptr(hash_path("host_req_seq"), &game_host_req_seq);
    stasis_jit_register_global_i32_ptr(hash_path("host_req_flags"), &game_host_req_flags);
    stasis_jit_register_global_i32_ptr(
        hash_path("host_req_window_w_px"), &game_host_req_window_w_px);
    stasis_jit_register_global_i32_ptr(
        hash_path("host_req_window_h_px"), &game_host_req_window_h_px);
    stasis_jit_profile_register_function(77, "render");
    stasis_jit_profile_configure(1, 2);
}

static int32_t game_tick(void) {
    tick_calls += 1;
    return tick_result;
}

static int32_t game_render(void) {
    stasis_jit_profile_frame_enter(77);
    render_calls += 1;
    game_gfx_cmd_i32[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_MAGIC;
    game_gfx_cmd_i32[STASIS_RENDER_I_VERSION] = STASIS_RENDER_VERSION;
    stasis_jit_profile_frame_leave(77);
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
    performance_counter = 0;
    performance_metrics_enabled = 1;
    performance_metrics_calls = 0;
    reported_tick_us = 0;
    reported_render_us = 0;
    profile_start_logs = 0;
    profile_row_logs = 0;
    profile_done_logs = 0;
    profile_row[0] = '\0';
    memset(game_host_i32, 0, sizeof(game_host_i32));
    memset(game_host_f32, 0, sizeof(game_host_f32));
    memset(game_gfx_cmd_i32, 0, sizeof(game_gfx_cmd_i32));
    memset(game_gfx_cmd_f32, 0, sizeof(game_gfx_cmd_f32));
    memset(game_gfx_cmd_u8, 0, sizeof(game_gfx_cmd_u8));
    game_host_req_seq = 0;
    game_host_req_flags = 0;
    game_host_req_window_w_px = 0;
    game_host_req_window_h_px = 0;
#if defined(STASIS_NETWORK_ENABLED)
    assert(network_host.active == 0);
    network_start_result = 1;
    network_start_calls = 0;
    network_stop_calls = 0;
#endif
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
#if defined(STASIS_NETWORK_ENABLED)
    assert(network_start_calls == 1);
    assert(network_host.active == 1);
    char join_card[128] = {0};
    char private_join_url[128] = {0};
    assert(stasis_mobile_network_copy_join_card(join_card, sizeof(join_card)) > 0);
    assert(strcmp(join_card, "http://192.0.2.20:4312/") == 0);
    assert(strstr(join_card, "session") == NULL && strstr(join_card, "pair") == NULL);
    assert(stasis_mobile_network_copy_join_url(
        private_join_url, sizeof(private_join_url)) > 0);
    assert(strstr(private_join_url, "/session?pair=") != NULL);
#endif
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
    assert(performance_metrics_calls == 1);
    assert(reported_tick_us == 10);
    assert(reported_render_us == 10);

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
#if defined(STASIS_NETWORK_ENABLED)
    assert(network_stop_calls == 1);
    assert(network_host.active == 0);
#endif
    assert(stasis_mobile_runtime_is_initialized() == 0);
    assert(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_NOT_INITIALIZED);
}

#if defined(STASIS_NETWORK_ENABLED)
static void test_network_start_failure_cleans_up_runtime(void) {
    StasisMobileRuntimeConfig valid_config = config();
    StasisMobileGameEntries valid_entries = entries();

    network_start_result = 0;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT);
    assert(network_start_calls == 1);
    assert(network_stop_calls == 0);
    assert(network_host.active == 0);
    assert(shutdown_calls == 1);
    assert(stasis_mobile_runtime_is_initialized() == 0);

    network_start_result = 1;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_OK);
    assert(network_start_calls == 2);
    assert(network_host.active == 1);
    stasis_mobile_runtime_shutdown();
    assert(network_stop_calls == 1);
    assert(network_host.active == 0);
}
#endif

static void test_skips_hidden_performance_hud_measurement(void) {
    StasisMobileRuntimeConfig valid_config = config();
    StasisMobileGameEntries valid_entries = entries();
    performance_metrics_enabled = 0;

    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) == 0);
    assert(stasis_mobile_runtime_step() == 0);
    assert(tick_calls == 1);
    assert(render_calls == 1);
    assert(performance_counter == 0);
    assert(performance_metrics_calls == 0);
    stasis_mobile_runtime_shutdown();
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

static void test_reports_bounded_profile_after_warmup(void) {
    StasisMobileRuntimeConfig valid_config = config();
    StasisMobileGameEntries valid_entries = entries();

    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) == 0);
    assert(stasis_mobile_runtime_step() == 0);
    assert(profile_start_logs == 0 && profile_row_logs == 0 && profile_done_logs == 0);
    assert(stasis_mobile_runtime_step() == 0);
    assert(profile_start_logs == 1 && profile_row_logs == 0 && profile_done_logs == 0);
    assert(stasis_mobile_runtime_step() == 0);
    assert(profile_row_logs == 1 && profile_done_logs == 1);
    assert(strstr(profile_row, "STASIS_PROFILE|render|2|") == profile_row);
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
    assert(stasis_mobile_runtime_last_entry() == STASIS_MOBILE_RUNTIME_ENTRY_MAIN);
    assert(stasis_mobile_runtime_last_entry_result() == 7);
    assert(tick_calls == 0 && render_calls == 0);
    stasis_mobile_runtime_shutdown();

    reset_fakes();
    tick_result = 8;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_OK);
    assert(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    assert(stasis_mobile_runtime_last_entry() == STASIS_MOBILE_RUNTIME_ENTRY_TICK);
    assert(stasis_mobile_runtime_last_entry_result() == 8);
    assert(tick_calls == 1 && render_calls == 0);
    assert(begin_frame_calls == 0 && end_frame_calls == 0 && gfx_submit_calls == 0);
    stasis_mobile_runtime_shutdown();

    reset_fakes();
    render_result = 9;
    assert(stasis_mobile_runtime_initialize(&valid_config, &valid_entries) ==
        STASIS_MOBILE_RUNTIME_OK);
    assert(stasis_mobile_runtime_step() == STASIS_MOBILE_RUNTIME_STOP_REQUESTED);
    assert(stasis_mobile_runtime_last_entry() == STASIS_MOBILE_RUNTIME_ENTRY_RENDER);
    assert(stasis_mobile_runtime_last_entry_result() == 9);
    assert(tick_calls == 1 && render_calls == 1);
    assert(begin_frame_calls == 0 && end_frame_calls == 0 && gfx_submit_calls == 0);
    stasis_mobile_runtime_shutdown();
}

int main(void) {
#if defined(STASIS_NETWORK_ENABLED)
    configure_network_fixture();
#endif
    reset_fakes();
    test_rejects_invalid_configuration();
    test_runs_mobile_lifecycle();
    reset_fakes();
    test_skips_hidden_performance_hud_measurement();
    reset_fakes();
    test_rejects_duplicate_initialization();
    reset_fakes();
    test_reports_bounded_profile_after_warmup();
    reset_fakes();
    test_reports_graphics_initialization_failure();
    reset_fakes();
    test_stops_on_nonzero_game_entry_results();
#if defined(STASIS_NETWORK_ENABLED)
    reset_fakes();
    test_network_start_failure_cleans_up_runtime();
    assert(remove("network_guest.bundle") == 0);
#endif
    puts("stasis_mobile_runtime_test: ok");
    return 0;
}
