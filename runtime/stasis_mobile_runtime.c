#include "stasis_mobile_runtime.h"

#include <string.h>

int stasis_init_window(int width, int height, const char *title);
int stasis_should_quit(void);
void stasis_shutdown(void);
void stasis_host_bulk_init(const int32_t *host_req_seq);
void stasis_host_bulk_apply_requests(
    const int32_t *host_req_seq,
    const int32_t *host_req_flags,
    const int32_t *host_req_window_w_px,
    const int32_t *host_req_window_h_px);
void stasis_host_get_frame(int32_t *out_i32, float *out_f32);
void stasis_gfx_submit_u8(
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8);

static int stasis_mobile_game_is_valid(const StasisMobileGame *game)
{
    return game &&
        game->main_entry && game->tick_entry && game->render_entry &&
        game->host_i32 && game->host_f32 &&
        game->gfx_cmd_i32 && game->gfx_cmd_f32 && game->gfx_cmd_u8;
}

static int32_t stasis_mobile_stop(StasisMobileRuntime *runtime, int32_t result)
{
    runtime->last_result = result;
    runtime->stopped = 1;
    return result;
}

int32_t stasis_mobile_runtime_start(
    StasisMobileRuntime *runtime,
    const StasisMobileRuntimeConfig *config)
{
    if (!runtime || !config || config->struct_size < sizeof(*config) ||
        !stasis_mobile_game_is_valid(&config->game))
    {
        return STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT;
    }
    if (config->abi_version != STASIS_MOBILE_RUNTIME_ABI_VERSION)
    {
        return STASIS_MOBILE_RUNTIME_ABI_MISMATCH;
    }

    memset(runtime, 0, sizeof(*runtime));
    runtime->config = *config;
    runtime->focused = 1;

    const char *title = config->window_title ? config->window_title : "Stasis";
    if (stasis_init_window(config->window_width, config->window_height, title) != 0)
    {
        return stasis_mobile_stop(runtime, STASIS_MOBILE_RUNTIME_WINDOW_FAILED);
    }

    stasis_host_bulk_init(config->game.host_req_seq);
    runtime->started = 1;
    const int32_t main_result = config->game.main_entry();
    if (main_result != 0)
    {
        return stasis_mobile_stop(runtime, main_result);
    }
    stasis_host_bulk_apply_requests(
        config->game.host_req_seq,
        config->game.host_req_flags,
        config->game.host_req_window_w_px,
        config->game.host_req_window_h_px);
    return STASIS_MOBILE_RUNTIME_OK;
}

int32_t stasis_mobile_runtime_step(StasisMobileRuntime *runtime)
{
    if (!runtime || !runtime->started)
    {
        return STASIS_MOBILE_RUNTIME_NOT_STARTED;
    }
    if (runtime->stopped)
    {
        return runtime->last_result;
    }
    if (runtime->paused || !runtime->focused)
    {
        return STASIS_MOBILE_RUNTIME_OK;
    }

    StasisMobileGame *game = &runtime->config.game;
    stasis_host_get_frame(game->host_i32, game->host_f32);
    if (stasis_should_quit() || game->host_i32[9] != 0)
    {
        return stasis_mobile_stop(runtime, STASIS_MOBILE_RUNTIME_STOP);
    }
    stasis_host_bulk_apply_requests(
        game->host_req_seq,
        game->host_req_flags,
        game->host_req_window_w_px,
        game->host_req_window_h_px);

    const int32_t tick_result = game->tick_entry();
    if (tick_result != 0)
    {
        return stasis_mobile_stop(runtime, tick_result);
    }
    const int32_t render_result = game->render_entry();
    if (render_result != 0)
    {
        return stasis_mobile_stop(runtime, render_result);
    }

    stasis_gfx_submit_u8(game->gfx_cmd_i32, game->gfx_cmd_f32, game->gfx_cmd_u8);
    runtime->frame_count += 1;
    return STASIS_MOBILE_RUNTIME_OK;
}

void stasis_mobile_runtime_set_paused(StasisMobileRuntime *runtime, int paused)
{
    if (runtime)
    {
        runtime->paused = paused ? 1 : 0;
    }
}

void stasis_mobile_runtime_set_focused(StasisMobileRuntime *runtime, int focused)
{
    if (runtime)
    {
        runtime->focused = focused ? 1 : 0;
    }
}

void stasis_mobile_runtime_shutdown(StasisMobileRuntime *runtime)
{
    if (!runtime || !runtime->started)
    {
        return;
    }
    stasis_shutdown();
    runtime->started = 0;
    runtime->stopped = 1;
}
