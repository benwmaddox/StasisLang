#include "stasis_mobile_runtime.h"
#include "stasis_mobile_aot_runtime.h"

#include <stddef.h>
#include <string.h>

/* Implemented by the SDL-only stasis_graphics.c linked into the mobile core. */
int stasis_init_window(int width, int height, const char *title);
int stasis_should_quit(void);
int stasis_mobile_poll_events(void);
void stasis_mobile_set_paused(int paused);
void stasis_begin_frame(void);
void stasis_end_frame(void);
void stasis_host_get_frame(int32_t *out_i32, float *out_f32);
void stasis_host_bulk_apply_requests(
    const int32_t *seq,
    const int32_t *flags,
    const int32_t *width,
    const int32_t *height
);
void stasis_gfx_submit_u8(
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8
);
void stasis_shutdown(void);

static int32_t host_i32[768];
static float host_f32[64];
static int32_t gfx_cmd_i32[34848];
static float gfx_cmd_f32[92292];
static uint8_t gfx_cmd_u8[65536];
static int32_t host_req_seq;
static int32_t host_req_flags;
static int32_t host_req_window_w_px;
static int32_t host_req_window_h_px;

typedef struct StasisMobileRuntimeState {
    StasisMobileGameEntries entries;
    int initialized;
    int paused;
} StasisMobileRuntimeState;

static StasisMobileRuntimeState runtime_state;

static int32_t hash_global_path(const char *path) {
    uint32_t hash = 2166136261U;
    while (*path != '\0') {
        hash ^= (uint8_t)*path++;
        hash *= 16777619U;
    }
    return (int32_t)hash;
}

static void bind_host_globals(void) {
    memset(host_i32, 0, sizeof(host_i32));
    memset(host_f32, 0, sizeof(host_f32));
    memset(gfx_cmd_i32, 0, sizeof(gfx_cmd_i32));
    memset(gfx_cmd_f32, 0, sizeof(gfx_cmd_f32));
    memset(gfx_cmd_u8, 0, sizeof(gfx_cmd_u8));
    host_req_seq = 0;
    host_req_flags = 0;
    host_req_window_w_px = 0;
    host_req_window_h_px = 0;
    stasis_jit_register_global_i32_array(hash_global_path("host_i32"), 0, host_i32, 768);
    stasis_jit_register_global_f32_array(hash_global_path("host_f32"), 0, host_f32, 64);
    stasis_jit_register_global_i32_array(hash_global_path("gfx_cmd_i32"), 0, gfx_cmd_i32, 34848);
    stasis_jit_register_global_f32_array(hash_global_path("gfx_cmd_f32"), 0, gfx_cmd_f32, 92292);
    stasis_jit_register_global_u8_array(hash_global_path("gfx_cmd_u8"), 0, gfx_cmd_u8, 65536);
    stasis_jit_register_global_i32_ptr(hash_global_path("host_req_seq"), &host_req_seq);
    stasis_jit_register_global_i32_ptr(hash_global_path("host_req_flags"), &host_req_flags);
    stasis_jit_register_global_i32_ptr(
        hash_global_path("host_req_window_w_px"), &host_req_window_w_px);
    stasis_jit_register_global_i32_ptr(
        hash_global_path("host_req_window_h_px"), &host_req_window_h_px);
}

static int entries_are_valid(const StasisMobileGameEntries *entries) {
    return entries != NULL &&
        entries->bind_runtime_entry != NULL &&
        entries->main_entry != NULL &&
        entries->tick_entry != NULL &&
        entries->render_entry != NULL;
}

int32_t stasis_mobile_runtime_initialize(
    const StasisMobileRuntimeConfig *config,
    const StasisMobileGameEntries *entries
) {
    if (runtime_state.initialized) {
        return STASIS_MOBILE_RUNTIME_ALREADY_INITIALIZED;
    }
    if (config == NULL || config->width <= 0 || config->height <= 0 ||
        !entries_are_valid(entries)) {
        return STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT;
    }
    stasis_mobile_aot_reset();
    if (!stasis_init_window(config->width, config->height, config->title)) {
        return STASIS_MOBILE_RUNTIME_GRAPHICS_UNAVAILABLE;
    }

    runtime_state.entries = *entries;
    runtime_state.initialized = 1;
    runtime_state.paused = 0;
    runtime_state.entries.bind_runtime_entry();
    bind_host_globals();
    stasis_host_bulk_apply_requests(
        &host_req_seq,
        &host_req_flags,
        &host_req_window_w_px,
        &host_req_window_h_px
    );
    runtime_state.entries.main_entry();
    stasis_host_bulk_apply_requests(
        &host_req_seq,
        &host_req_flags,
        &host_req_window_w_px,
        &host_req_window_h_px
    );
    return STASIS_MOBILE_RUNTIME_OK;
}

int32_t stasis_mobile_runtime_step(void) {
    if (!runtime_state.initialized) {
        return STASIS_MOBILE_RUNTIME_NOT_INITIALIZED;
    }
    if (runtime_state.paused) {
        return stasis_mobile_poll_events()
            ? STASIS_MOBILE_RUNTIME_STOP_REQUESTED
            : STASIS_MOBILE_RUNTIME_OK;
    }
    if (stasis_should_quit()) {
        return STASIS_MOBILE_RUNTIME_STOP_REQUESTED;
    }

    stasis_host_get_frame(host_i32, host_f32);
    stasis_host_bulk_apply_requests(
        &host_req_seq,
        &host_req_flags,
        &host_req_window_w_px,
        &host_req_window_h_px
    );
    runtime_state.entries.tick_entry();
    stasis_begin_frame();
    runtime_state.entries.render_entry();
    stasis_gfx_submit_u8(gfx_cmd_i32, gfx_cmd_f32, gfx_cmd_u8);
    stasis_end_frame();
    return STASIS_MOBILE_RUNTIME_OK;
}

void stasis_mobile_runtime_set_paused(int32_t paused) {
    if (runtime_state.initialized) {
        runtime_state.paused = paused != 0;
        stasis_mobile_set_paused(runtime_state.paused);
    }
}

int32_t stasis_mobile_runtime_is_initialized(void) {
    return runtime_state.initialized;
}

void stasis_mobile_runtime_shutdown(void) {
    if (!runtime_state.initialized) {
        return;
    }
    stasis_shutdown();
    stasis_mobile_aot_reset();
    runtime_state = (StasisMobileRuntimeState){0};
}
