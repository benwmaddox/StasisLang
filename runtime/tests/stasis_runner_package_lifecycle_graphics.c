#include <stdint.h>
#include <stdlib.h>

#include "../stasis_render_contract.h"

#if defined(_WIN32)
#define STASIS_TEST_EXPORT __declspec(dllexport)
#else
#define STASIS_TEST_EXPORT __attribute__((visibility("default")))
#endif

STASIS_TEST_EXPORT int stasis_graphics_runtime_abi_version(void)
{
    return STASIS_GRAPHICS_RUNTIME_ABI_VERSION;
}

STASIS_TEST_EXPORT int stasis_set_asset_root(const char *path)
{
    return path && path[0];
}

STASIS_TEST_EXPORT int stasis_init_window(int width, int height, const char *title)
{
    (void)width;
    (void)height;
    (void)title;
#if defined(_WIN32)
    return _putenv_s("STASIS_TEST_WINDOW_READY", "1") == 0;
#else
    return setenv("STASIS_TEST_WINDOW_READY", "1", 1) == 0;
#endif
}

STASIS_TEST_EXPORT int stasis_set_fullscreen(int enabled)
{
    (void)enabled;
    return 1;
}

STASIS_TEST_EXPORT void stasis_host_get_frame(int32_t *out_i32, float *out_f32)
{
    (void)out_i32;
    (void)out_f32;
}
