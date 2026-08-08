#include <stdlib.h>

#include "../stasis_render_contract.h"

#if defined(_WIN32)
#define STASIS_TEST_EXPORT __declspec(dllexport)
#else
#define STASIS_TEST_EXPORT __attribute__((visibility("default")))
#endif

STASIS_TEST_EXPORT int stasis_graphics_runtime_abi_version(void) {
    return STASIS_GRAPHICS_RUNTIME_ABI_VERSION;
}

STASIS_TEST_EXPORT int stasis_set_asset_root(const char *path) {
    return path && path[0];
}

STASIS_TEST_EXPORT int stasis_init_window(int width, int height, const char *title) {
    (void)width;
    (void)height;
    (void)title;
    return setenv("STASIS_TEST_WINDOW_READY", "1", 1) == 0;
}

STASIS_TEST_EXPORT int stasis_set_fullscreen(int enabled) {
    (void)enabled;
    return 1;
}
