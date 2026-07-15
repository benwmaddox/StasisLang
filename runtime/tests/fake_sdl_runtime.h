#ifndef STASIS_MOBILE_FAKE_SDL_RUNTIME_H
#define STASIS_MOBILE_FAKE_SDL_RUNTIME_H

extern int fake_init_count;
extern int fake_shutdown_count;
extern int fake_host_frame_count;
extern int fake_submit_count;
extern int fake_quit_requested;

void fake_sdl_runtime_reset(void);

#endif
