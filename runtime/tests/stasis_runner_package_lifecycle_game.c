#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#if defined(_WIN32)
#define STASIS_TEST_EXPORT __declspec(dllexport)
#else
#define STASIS_TEST_EXPORT __attribute__((visibility("default")))
#endif

STASIS_TEST_EXPORT int32_t host_i32[768] = {0};
STASIS_TEST_EXPORT float host_f32[64] = {0};
STASIS_TEST_EXPORT int32_t gfx_cmd_i32[67888] = {0};
STASIS_TEST_EXPORT float gfx_cmd_f32[146564] = {0};
STASIS_TEST_EXPORT uint8_t gfx_cmd_u8[65536] = {0};

static int reset_calls;
static int finish_calls;
static int render_calls;
static int construction_active;
static int construction_invalid;
static int working_rects;
static int published_rects;

STASIS_TEST_EXPORT void stasis_aot_bind_runtime_globals(void)
{
}

STASIS_TEST_EXPORT int main(void)
{
    const char *asset_root = getenv("STASIS_ASSET_ROOT");
    const char *runtime_path = getenv("STASIS_RUNTIME_LIBRARY_PATH");
    const char *window_ready = getenv("STASIS_TEST_WINDOW_READY");
#if defined(_WIN32)
    const int paths_ready = asset_root && asset_root[0] != '\0' &&
        runtime_path && runtime_path[0] != '\0';
#else
    const int paths_ready = asset_root && asset_root[0] == '/' &&
        runtime_path && runtime_path[0] == '/';
#endif
    if (!paths_ready || !window_ready || window_ready[0] != '1')
    {
        return 17;
    }
    host_i32[9] = 0;
    return 0;
}

STASIS_TEST_EXPORT int tick(void)
{
    /* The second render exits through finish after proving nested begin rejection. */
    return 0;
}

STASIS_TEST_EXPORT void gfx_cmd_construction_reset(void)
{
    reset_calls++;
    construction_active = 1;
    construction_invalid = 0;
    working_rects = 0;
}

STASIS_TEST_EXPORT void gfx_cmd_manual_begin(void)
{
    if (construction_active)
    {
        construction_invalid = 1;
        return;
    }
    working_rects = 0;
}

STASIS_TEST_EXPORT void aot_render(void)
{
    render_calls++;
    working_rects = 1;
    if (render_calls == 2)
    {
        /* Models authored begin_frame() inside a lifecycle-v1 render callback. */
        gfx_cmd_manual_begin();
    }
}

STASIS_TEST_EXPORT int gfx_cmd_construction_finish(int result)
{
    finish_calls++;
    if (reset_calls != 1 || render_calls != 1 || finish_calls != 1)
    {
        if (reset_calls != 2 || render_calls != 2 || finish_calls != 2)
        {
            return 2;
        }
    }
    if (!construction_active)
    {
        return 3;
    }
    construction_active = 0;
    if (construction_invalid)
    {
        working_rects = 0;
        if (render_calls != 2 || published_rects != 1)
        {
            return 4;
        }
        puts("PACKAGED_RUNNER_LIFECYCLE_V1_NESTED_BEGIN_REJECTED");
        return 1;
    }
    if (render_calls != 1 || result != 0)
    {
        return 5;
    }
    published_rects = working_rects;
    puts("PACKAGED_RUNNER_LIFECYCLE_V1_PUBLISHED");
    return 0;
}

/* This is the generated-package bridge shape. The runner must call only render. */
STASIS_TEST_EXPORT int render(void)
{
    gfx_cmd_construction_reset();
    aot_render();
    return gfx_cmd_construction_finish(0);
}

/* Lifecycle-0 packages export their authored render directly. */
STASIS_TEST_EXPORT int legacy_render(void)
{
    if (reset_calls != 0 || finish_calls != 0 || render_calls != 0)
    {
        return 6;
    }
    puts("PACKAGED_RUNNER_LIFECYCLE_V0_DIRECT");
    return 1;
}
