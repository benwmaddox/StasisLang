#include "stasis_mobile_runtime.h"
#include "stasis_mobile_aot_runtime.h"
#include "stasis_render_contract.h"

#include <stddef.h>
#include <stdio.h>
#include <string.h>

/* Implemented by the SDL-only stasis_graphics.c linked into the mobile core. */
int stasis_init_window(int width, int height, const char *title);
int stasis_should_quit(void);
int stasis_mobile_poll_events(void);
void stasis_mobile_set_paused(int paused);
void stasis_host_get_frame(int32_t *out_i32, float *out_f32);
void stasis_host_bulk_apply_requests(
    const int32_t *seq,
    const int32_t *flags,
    const int32_t *width,
    const int32_t *height
);
void stasis_gfx_submit_u8(
    int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8
);
uint64_t stasis_host_performance_counter(void);
uint64_t stasis_host_performance_elapsed_us(uint64_t started, uint64_t finished);
void stasis_host_set_performance_metrics(uint64_t tick_us, uint64_t render_us);
int stasis_host_performance_metrics_enabled(void);
void stasis_shutdown(void);

static int32_t *host_i32;
static float *host_f32;
static int32_t *gfx_cmd_i32;
static float *gfx_cmd_f32;
static uint8_t *gfx_cmd_u8;

typedef struct StasisMobileRuntimeState {
    StasisMobileGameEntries entries;
    int initialized;
    int paused;
    int32_t last_entry_result;
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

static int bind_guest_globals(void) {
    host_i32 = stasis_jit_global_i32_array_ptr(hash_global_path("host_i32"), 0, 768);
    host_f32 = stasis_jit_global_f32_array_ptr(hash_global_path("host_f32"), 0, 64);
    gfx_cmd_i32 = stasis_jit_global_i32_array_ptr(
        hash_global_path("gfx_cmd_i32"), 0, STASIS_RENDER_I32_COUNT);
    gfx_cmd_f32 = stasis_jit_global_f32_array_ptr(
        hash_global_path("gfx_cmd_f32"), 0, STASIS_RENDER_F32_COUNT);
    gfx_cmd_u8 = stasis_jit_global_u8_array_ptr(
        hash_global_path("gfx_cmd_u8"), 0, STASIS_RENDER_U8_COUNT);
    if (host_i32 == NULL || host_f32 == NULL || gfx_cmd_i32 == NULL ||
        gfx_cmd_f32 == NULL || gfx_cmd_u8 == NULL) {
        fprintf(stderr,
            "Stasis mobile runtime could not bind guest buffers: host_i32=%p host_f32=%p gfx_cmd_i32=%p gfx_cmd_f32=%p gfx_cmd_u8=%p\n",
            (void *)host_i32, (void *)host_f32, (void *)gfx_cmd_i32,
            (void *)gfx_cmd_f32, (void *)gfx_cmd_u8);
        return 0;
    }
    memset(host_i32, 0, 768 * sizeof(*host_i32));
    memset(host_f32, 0, 64 * sizeof(*host_f32));
    memset(gfx_cmd_i32, 0, STASIS_RENDER_I32_COUNT * sizeof(*gfx_cmd_i32));
    memset(gfx_cmd_f32, 0, STASIS_RENDER_F32_COUNT * sizeof(*gfx_cmd_f32));
    memset(gfx_cmd_u8, 0, STASIS_RENDER_U8_COUNT * sizeof(*gfx_cmd_u8));
    return 1;
}

static void apply_guest_host_requests(void) {
    int32_t seq = stasis_jit_global_i32_load(hash_global_path("host_req_seq"));
    int32_t flags = stasis_jit_global_i32_load(hash_global_path("host_req_flags"));
    int32_t width = stasis_jit_global_i32_load(hash_global_path("host_req_window_w_px"));
    int32_t height = stasis_jit_global_i32_load(hash_global_path("host_req_window_h_px"));
    stasis_host_bulk_apply_requests(&seq, &flags, &width, &height);
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
#if defined(STASIS_NETWORK_ENABLED)
    if (stasis_mobile_network_start_from_asset_root() < 0) {
        stasis_shutdown();
        stasis_mobile_aot_reset();
        runtime_state = (StasisMobileRuntimeState){0};
        return STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT;
    }
#endif
    if (!bind_guest_globals()) {
        stasis_mobile_network_stop();
        stasis_shutdown();
        stasis_mobile_aot_reset();
        runtime_state = (StasisMobileRuntimeState){0};
        return STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT;
    }
    apply_guest_host_requests();
    runtime_state.last_entry_result = runtime_state.entries.main_entry();
    apply_guest_host_requests();
    if (runtime_state.last_entry_result != 0) {
        fprintf(stderr, "Stasis mobile main entry requested stop with code %d\n",
            runtime_state.last_entry_result);
        return STASIS_MOBILE_RUNTIME_STOP_REQUESTED;
    }
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
        fprintf(stderr, "Stasis mobile runtime received a quit event before the frame\n");
        return STASIS_MOBILE_RUNTIME_STOP_REQUESTED;
    }

    stasis_host_get_frame(host_i32, host_f32);
    apply_guest_host_requests();
    stasis_jit_profile_frame_begin();
    const int measure_frame = stasis_host_performance_metrics_enabled();
    uint64_t tick_started = measure_frame ? stasis_host_performance_counter() : 0;
    runtime_state.last_entry_result = runtime_state.entries.tick_entry();
    uint64_t tick_finished = measure_frame ? stasis_host_performance_counter() : 0;
    if (runtime_state.last_entry_result != 0) {
        fprintf(stderr, "Stasis mobile tick entry requested stop with code %d\n",
            runtime_state.last_entry_result);
        return STASIS_MOBILE_RUNTIME_STOP_REQUESTED;
    }
    uint64_t render_started = measure_frame ? stasis_host_performance_counter() : 0;
    runtime_state.last_entry_result = runtime_state.entries.render_entry();
    uint64_t render_finished = measure_frame ? stasis_host_performance_counter() : 0;
    if (runtime_state.last_entry_result != 0) {
        fprintf(stderr, "Stasis mobile render entry requested stop with code %d\n",
            runtime_state.last_entry_result);
        return STASIS_MOBILE_RUNTIME_STOP_REQUESTED;
    }
    if (measure_frame) {
        stasis_host_set_performance_metrics(
            stasis_host_performance_elapsed_us(tick_started, tick_finished),
            stasis_host_performance_elapsed_us(render_started, render_finished));
    }
    /* Submission owns begin/present according to the guest command-buffer flags. */
    stasis_gfx_submit_u8(gfx_cmd_i32, gfx_cmd_f32, gfx_cmd_u8);
    stasis_jit_profile_frame_end();
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

int32_t stasis_mobile_runtime_last_entry_result(void) {
    return runtime_state.last_entry_result;
}

void stasis_mobile_runtime_shutdown(void) {
    if (!runtime_state.initialized) {
        return;
    }
    stasis_mobile_network_stop();
    stasis_shutdown();
    stasis_mobile_aot_reset();
    runtime_state = (StasisMobileRuntimeState){0};
}
