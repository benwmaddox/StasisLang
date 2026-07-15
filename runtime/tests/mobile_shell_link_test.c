#include "stasis_mobile_runtime.h"

#include <stdint.h>
#include <string.h>

#define CHECK(condition) do { if (!(condition)) return __LINE__; } while (0)

#ifndef STASIS_TEST_PLATFORM
#define STASIS_TEST_PLATFORM "unknown"
#endif

static int32_t entry(void)
{
    return 0;
}

int main(void)
{
    int32_t host_i32[16] = {0};
    float host_f32[16] = {0};
    int32_t gfx_i32[16] = {0};
    float gfx_f32[16] = {0};
    uint8_t gfx_u8[16] = {0};
    int32_t request_values[4] = {0};
    StasisMobileRuntimeConfig config;
    StasisMobileRuntime runtime;

    memset(&config, 0, sizeof(config));
    config.abi_version = STASIS_MOBILE_RUNTIME_ABI_VERSION;
    config.struct_size = sizeof(config);
    config.window_width = 640;
    config.window_height = 360;
    config.window_title = STASIS_TEST_PLATFORM;
    config.game.main_entry = entry;
    config.game.tick_entry = entry;
    config.game.render_entry = entry;
    config.game.host_i32 = host_i32;
    config.game.host_f32 = host_f32;
    config.game.gfx_cmd_i32 = gfx_i32;
    config.game.gfx_cmd_f32 = gfx_f32;
    config.game.gfx_cmd_u8 = gfx_u8;
    config.game.host_req_seq = &request_values[0];
    config.game.host_req_flags = &request_values[1];
    config.game.host_req_window_w_px = &request_values[2];
    config.game.host_req_window_h_px = &request_values[3];

    CHECK(stasis_mobile_runtime_start(&runtime, &config) == 0);
    CHECK(stasis_mobile_runtime_step(&runtime) == 0);
    CHECK(runtime.frame_count == 1);
    stasis_mobile_runtime_shutdown(&runtime);
    return 0;
}
