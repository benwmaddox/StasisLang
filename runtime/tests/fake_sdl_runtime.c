#include "fake_sdl_runtime.h"

#include <stdint.h>
#include <string.h>

int fake_init_count;
int fake_shutdown_count;
int fake_host_frame_count;
int fake_submit_count;
int fake_quit_requested;

void fake_sdl_runtime_reset(void)
{
    fake_init_count = 0;
    fake_shutdown_count = 0;
    fake_host_frame_count = 0;
    fake_submit_count = 0;
    fake_quit_requested = 0;
}

int stasis_init_window(int width, int height, const char *title)
{
    fake_init_count += 1;
    return width > 0 && height > 0 && title ? 0 : -1;
}

int stasis_should_quit(void)
{
    return fake_quit_requested;
}

void stasis_shutdown(void)
{
    fake_shutdown_count += 1;
}

void stasis_host_bulk_init(const int32_t *host_req_seq)
{
    (void)host_req_seq;
}

void stasis_host_bulk_apply_requests(
    const int32_t *host_req_seq,
    const int32_t *host_req_flags,
    const int32_t *host_req_window_w_px,
    const int32_t *host_req_window_h_px)
{
    (void)host_req_seq;
    (void)host_req_flags;
    (void)host_req_window_w_px;
    (void)host_req_window_h_px;
}

void stasis_host_get_frame(int32_t *out_i32, float *out_f32)
{
    fake_host_frame_count += 1;
    memset(out_i32, 0, sizeof(int32_t) * 16);
    memset(out_f32, 0, sizeof(float) * 16);
}

void stasis_gfx_submit_u8(
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8)
{
    if (cmd_i32 && cmd_f32 && cmd_u8)
    {
        fake_submit_count += 1;
    }
}
