#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#if defined(_WIN32)
#define STASIS_TEST_EXPORT __declspec(dllexport)
#else
#define STASIS_TEST_EXPORT __attribute__((visibility("default")))
#endif

STASIS_TEST_EXPORT void stasis_aot_bind_runtime_globals(void) {}

STASIS_TEST_EXPORT int main(void) {
    const char *asset_root = getenv("STASIS_ASSET_ROOT");
    const char *runtime_path = getenv("STASIS_RUNTIME_LIBRARY_PATH");
    const char *window_ready = getenv("STASIS_TEST_WINDOW_READY");
    volatile float value = sinf(0.5f);
    if (!asset_root || asset_root[0] != '/' ||
        !runtime_path || runtime_path[0] != '/' ||
        !window_ready || window_ready[0] != '1' || value == 0.0f) {
        return 17;
    }
    puts("PACKAGED_RUNNER_OK");
    return 0;
}
