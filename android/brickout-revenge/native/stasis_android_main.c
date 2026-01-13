#define SDL_MAIN_HANDLED 1
#include <SDL.h>

#include <stdint.h>

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

// Stasis program entrypoints (compiled from Brickout Revenge v1).
extern int main(void);
extern int tick(void);

// Runtime API (provided by runtime/stasis_graphics.c).
extern int stasis_should_quit(void);
extern void stasis_host_get_frame(int32_t *out_i32, float *out_f32);
extern int stasis_host_get_keyboard_state(uint8_t *out_u8, int max_bytes);
extern void stasis_gfx_submit_u8(const int32_t *cmd_i32, const float *cmd_f32, const uint8_t *cmd_u8);
extern int stasis_init_window(int width, int height, const char *title);
extern void stasis_set_window_size(int width, int height);
extern int stasis_set_fullscreen(int enabled);

// Bulk host ABI globals (provided by the Stasis program).
extern int32_t host_i32[];
extern float host_f32[];
extern uint8_t host_keys[];
extern int32_t gfx_cmd_i32[];
extern float gfx_cmd_f32[];
extern uint8_t gfx_cmd_u8[];

// Window request ABI (optional; provided by the Stasis program if it uses graphics stdlib).
extern int32_t host_req_seq;
extern int32_t host_req_flags;
extern int32_t host_req_window_w_px;
extern int32_t host_req_window_h_px;

static void stasis_apply_window_request(int32_t *last_seq)
{
    if (!last_seq)
    {
        return;
    }

    if (host_req_seq == *last_seq)
    {
        return;
    }
    *last_seq = host_req_seq;

    const int flags = host_req_flags;
    const int req_windowed = (flags & 1) != 0;
    const int req_fullscreen = (flags & 2) != 0;

    if (req_fullscreen)
    {
        (void)stasis_set_fullscreen(1);
        return;
    }

    if (req_windowed)
    {
        (void)stasis_set_fullscreen(0);
        if (host_req_window_w_px > 0 && host_req_window_h_px > 0)
        {
            stasis_set_window_size(host_req_window_w_px, host_req_window_h_px);
        }
    }
}

static void stasis_android_set_cwd(void)
{
    const char *external = SDL_AndroidGetExternalStoragePath();
    const char *internal = SDL_AndroidGetInternalStoragePath();
    const char *base = external && external[0] ? external : internal;
    if (!base || !base[0])
    {
        SDL_Log("stasis: no storage path available (external/internal empty)");
        return;
    }

    if (chdir(base) != 0)
    {
        SDL_Log("stasis: chdir('%s') failed: %s", base, strerror(errno));
        return;
    }

    SDL_Log("stasis: cwd set to %s", base);
}

int SDL_main(int argc, char **argv)
{
    (void)argc;
    (void)argv;

    stasis_android_set_cwd();

    // Default window policy: create fullscreen first.
    // The Stasis program may request another mode/size via host_req_* globals.
    (void)stasis_init_window(1280, 720, "Stasis");
    (void)stasis_set_fullscreen(1);

    int init_code = main();
    if (init_code != 0)
    {
        SDL_Log("stasis: main() returned %d", init_code);
        return init_code;
    }

    int32_t last_req_seq = host_req_seq;
    stasis_apply_window_request(&last_req_seq);

    for (;;)
    {
        // Bulk host mode: write HostFrame + keys into guest memory, tick, then submit command buffers.
        stasis_host_get_frame(host_i32, host_f32);
        (void)stasis_host_get_keyboard_state(host_keys, 512);
        stasis_apply_window_request(&last_req_seq);

        if (host_i32[9] != 0 || stasis_should_quit())
        {
            return 0;
        }

        int code = tick();
        if (code == 0)
        {
            stasis_gfx_submit_u8(gfx_cmd_i32, gfx_cmd_f32, gfx_cmd_u8);
            continue;
        }
        if (code == 1)
        {
            return 0;
        }
        return code;
    }
}
