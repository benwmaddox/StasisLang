#include "stasis_mobile_runtime.h"

#include "fake_sdl_runtime.h"

#include <stdint.h>
#include <string.h>

#define CHECK(condition) do { if (!(condition)) return __LINE__; } while (0)

static int main_count;
static int tick_count;
static int render_count;
static int tick_result;
static int swap_count;

static int32_t game_main(void)
{
    main_count += 1;
    return 0;
}

static int32_t game_tick(void)
{
    tick_count += 1;
    return tick_result;
}

static void game_on_code_swap(void)
{
    swap_count += 1;
}

static int32_t game_render(void)
{
    render_count += 1;
    return 0;
}

static StasisMobileRuntimeConfig test_config(void)
{
    static int32_t host_i32[16];
    static float host_f32[16];
    static int32_t gfx_i32[16];
    static float gfx_f32[16];
    static uint8_t gfx_u8[16];
    static int32_t request_values[4];
    StasisMobileRuntimeConfig config;

    memset(&config, 0, sizeof(config));
    config.abi_version = STASIS_MOBILE_RUNTIME_ABI_VERSION;
    config.struct_size = sizeof(config);
    config.window_width = 800;
    config.window_height = 600;
    config.window_title = "test";
    config.game.main_entry = game_main;
    config.game.tick_entry = game_tick;
    config.game.render_entry = game_render;
    config.game.on_code_swap_entry = game_on_code_swap;
    config.game.host_i32 = host_i32;
    config.game.host_f32 = host_f32;
    config.game.gfx_cmd_i32 = gfx_i32;
    config.game.gfx_cmd_f32 = gfx_f32;
    config.game.gfx_cmd_u8 = gfx_u8;
    config.game.host_req_seq = &request_values[0];
    config.game.host_req_flags = &request_values[1];
    config.game.host_req_window_w_px = &request_values[2];
    config.game.host_req_window_h_px = &request_values[3];
    return config;
}

int main(void)
{
    StasisMobileRuntime runtime;
    StasisMobileRuntimeConfig config = test_config();

    fake_sdl_runtime_reset();
    memset(&runtime, 0, sizeof(runtime));
    CHECK(stasis_mobile_runtime_start(&runtime, &config) == 0);
    CHECK(main_count == 1);
    CHECK(fake_init_count == 1);

    CHECK(stasis_mobile_runtime_step(&runtime) == 0);
    CHECK(tick_count == 1);
    CHECK(render_count == 1);
    CHECK(fake_host_frame_count == 1);
    CHECK(fake_submit_count == 1);
    CHECK(runtime.frame_count == 1);
    CHECK(swap_count == 0);

    stasis_mobile_runtime_set_paused(&runtime, 1);
    CHECK(stasis_mobile_runtime_step(&runtime) == 0);
    stasis_mobile_runtime_set_paused(&runtime, 0);
    stasis_mobile_runtime_set_focused(&runtime, 0);
    CHECK(stasis_mobile_runtime_step(&runtime) == 0);
    CHECK(tick_count == 1);

    stasis_mobile_runtime_set_focused(&runtime, 1);
    tick_result = 7;
    CHECK(stasis_mobile_runtime_step(&runtime) == 7);
    CHECK(render_count == 1);
    CHECK(stasis_mobile_runtime_step(&runtime) == 7);

    stasis_mobile_runtime_shutdown(&runtime);
    stasis_mobile_runtime_shutdown(&runtime);
    CHECK(fake_shutdown_count == 1);

    config.abi_version += 1;
    CHECK(stasis_mobile_runtime_start(&runtime, &config) ==
        STASIS_MOBILE_RUNTIME_ABI_MISMATCH);
    config.abi_version = STASIS_MOBILE_RUNTIME_ABI_VERSION;
    config.game.render_entry = 0;
    CHECK(stasis_mobile_runtime_start(&runtime, &config) ==
        STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT);
    return 0;
}
