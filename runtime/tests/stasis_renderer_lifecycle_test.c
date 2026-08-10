#include "stasis_renderer_lifecycle.h"

#include <stdio.h>
#include <stdlib.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static void test_restore_retry_and_generations(void) {
    StasisRendererLifecycle lifecycle = {0};
    stasis_renderer_lifecycle_initialize(&lifecycle);
    CHECK(stasis_renderer_lifecycle_can_present(&lifecycle));
    CHECK(lifecycle.surface_generation == 1u);
    CHECK(lifecycle.renderer_generation == 1u);
    CHECK(lifecycle.presentation_generation == 1u);

    stasis_renderer_lifecycle_surface_changed(&lifecycle);
    CHECK(stasis_renderer_lifecycle_can_present(&lifecycle));
    CHECK(lifecycle.surface_generation == 2u);
    CHECK(lifecycle.renderer_generation == 1u);
    CHECK(lifecycle.presentation_generation == 2u);
    stasis_renderer_lifecycle_renderer_reset(
        &lifecycle, STASIS_RENDERER_REASON_TARGETS_RESET);
    CHECK(lifecycle.presentation_generation == 3u);
    CHECK(!stasis_renderer_lifecycle_can_present(&lifecycle));
    CHECK(stasis_renderer_lifecycle_begin_restore(&lifecycle));
    stasis_renderer_lifecycle_finish_restore(&lifecycle, 0);
    CHECK(lifecycle.presentation_generation == 4u);
    CHECK(lifecycle.restore_attempts == 1u);
    CHECK(lifecycle.restore_failures == 1u);
    CHECK(stasis_renderer_lifecycle_begin_restore(&lifecycle));
    stasis_renderer_lifecycle_finish_restore(&lifecycle, 1);
    CHECK(stasis_renderer_lifecycle_can_present(&lifecycle));
    CHECK(lifecycle.restore_attempts == 2u);
}

static void test_pause_reset_and_wrap(void) {
    StasisRendererLifecycle lifecycle = {0};
    stasis_renderer_lifecycle_initialize(&lifecycle);
    stasis_renderer_lifecycle_pause(&lifecycle);
    CHECK(lifecycle.state == STASIS_RENDERER_PAUSED);
    stasis_renderer_lifecycle_resume(&lifecycle);
    CHECK(lifecycle.reason == STASIS_RENDERER_REASON_FOREGROUND);
    CHECK(lifecycle.surface_generation == 1u);
    CHECK(lifecycle.renderer_generation == 1u);
    CHECK(lifecycle.presentation_generation == 2u);
    CHECK(stasis_renderer_lifecycle_can_present(&lifecycle));

    stasis_renderer_lifecycle_renderer_reset(
        &lifecycle, STASIS_RENDERER_REASON_DEVICE_RESET);
    CHECK(lifecycle.surface_generation == 2u);
    CHECK(lifecycle.renderer_generation == 2u);
    CHECK(lifecycle.presentation_generation == 3u);
    CHECK(lifecycle.reason == STASIS_RENDERER_REASON_DEVICE_RESET);

    lifecycle.surface_generation = UINT32_MAX;
    lifecycle.renderer_generation = UINT32_MAX;
    lifecycle.presentation_generation = UINT32_MAX;
    stasis_renderer_lifecycle_renderer_reset(
        &lifecycle, STASIS_RENDERER_REASON_TARGETS_RESET);
    CHECK(lifecycle.surface_generation == 1u);
    CHECK(lifecycle.renderer_generation == 1u);
    CHECK(lifecycle.presentation_generation == 1u);
}

static void test_explicit_redraw_changes_only_presentation_generation(void) {
    StasisRendererLifecycle lifecycle = {0};
    stasis_renderer_lifecycle_initialize(&lifecycle);

    stasis_renderer_lifecycle_request_redraw(&lifecycle);

    CHECK(lifecycle.surface_generation == 1u);
    CHECK(lifecycle.renderer_generation == 1u);
    CHECK(lifecycle.presentation_generation == 2u);
    CHECK(stasis_renderer_lifecycle_can_present(&lifecycle));
}

static void test_pause_before_restore_preserves_pending_state(void) {
    StasisRendererLifecycle lifecycle = {0};
    stasis_renderer_lifecycle_initialize(&lifecycle);
    stasis_renderer_lifecycle_renderer_reset(
        &lifecycle, STASIS_RENDERER_REASON_DEVICE_RESET);
    stasis_renderer_lifecycle_pause(&lifecycle);
    stasis_renderer_lifecycle_resume(&lifecycle);
    CHECK(lifecycle.state == STASIS_RENDERER_RESTORE_PENDING);
    CHECK(!stasis_renderer_lifecycle_can_present(&lifecycle));
}

int main(void) {
    test_restore_retry_and_generations();
    test_pause_reset_and_wrap();
    test_pause_before_restore_preserves_pending_state();
    test_explicit_redraw_changes_only_presentation_generation();
    puts("stasis_renderer_lifecycle_test: ok");
    return 0;
}
