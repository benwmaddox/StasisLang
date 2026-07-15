#ifndef STASIS_MOBILE_RUNTIME_H
#define STASIS_MOBILE_RUNTIME_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define STASIS_MOBILE_RUNTIME_ABI_VERSION 1u

typedef int32_t (*StasisMobileI32Entry)(void);
typedef void (*StasisMobileVoidEntry)(void);

typedef struct StasisMobileGame {
    StasisMobileI32Entry main_entry;
    StasisMobileI32Entry tick_entry;
    StasisMobileI32Entry render_entry;
    StasisMobileVoidEntry on_code_swap_entry;
    int32_t *host_i32;
    float *host_f32;
    int32_t *gfx_cmd_i32;
    float *gfx_cmd_f32;
    uint8_t *gfx_cmd_u8;
    int32_t *host_req_seq;
    int32_t *host_req_flags;
    int32_t *host_req_window_w_px;
    int32_t *host_req_window_h_px;
} StasisMobileGame;

typedef struct StasisMobileRuntimeConfig {
    uint32_t abi_version;
    size_t struct_size;
    int32_t window_width;
    int32_t window_height;
    const char *window_title;
    StasisMobileGame game;
} StasisMobileRuntimeConfig;

typedef struct StasisMobileRuntime {
    StasisMobileRuntimeConfig config;
    uint64_t frame_count;
    int32_t last_result;
    uint8_t started;
    uint8_t paused;
    uint8_t focused;
    uint8_t stopped;
} StasisMobileRuntime;

enum {
    STASIS_MOBILE_RUNTIME_OK = 0,
    STASIS_MOBILE_RUNTIME_STOP = 1,
    STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT = -1,
    STASIS_MOBILE_RUNTIME_ABI_MISMATCH = -2,
    STASIS_MOBILE_RUNTIME_WINDOW_FAILED = -3,
    STASIS_MOBILE_RUNTIME_NOT_STARTED = -4
};

int32_t stasis_mobile_runtime_start(
    StasisMobileRuntime *runtime,
    const StasisMobileRuntimeConfig *config);
int32_t stasis_mobile_runtime_step(StasisMobileRuntime *runtime);
void stasis_mobile_runtime_set_paused(StasisMobileRuntime *runtime, int paused);
void stasis_mobile_runtime_set_focused(StasisMobileRuntime *runtime, int focused);
void stasis_mobile_runtime_shutdown(StasisMobileRuntime *runtime);

#ifdef __cplusplus
}
#endif

#endif
