#ifndef STASIS_AOT_RUNTIME_H
#define STASIS_AOT_RUNTIME_H

#include <stdint.h>

#define STASIS_PUBLISHED_MAX_COMMANDS 64
#define STASIS_PUBLISHED_FRAME_I32_COUNT (6 + STASIS_PUBLISHED_MAX_COMMANDS * 7)

void stasis_published_init_globals(void);
int stasis_published_run_tick_frame(
        int touch_x,
        int touch_y,
        int touch_active,
        int screen_w,
        int screen_h,
        int32_t *out_values,
        uintptr_t out_len);
const char *stasis_published_sprite_path(int32_t handle);
const char *stasis_published_text_for_run(int32_t run_handle);

#endif
