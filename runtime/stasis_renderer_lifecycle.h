#ifndef STASIS_RENDERER_LIFECYCLE_H
#define STASIS_RENDERER_LIFECYCLE_H

#include <stdint.h>

typedef enum {
    STASIS_RENDERER_UNAVAILABLE = 0,
    STASIS_RENDERER_READY = 1,
    STASIS_RENDERER_PAUSED = 2,
    STASIS_RENDERER_RESTORE_PENDING = 3,
    STASIS_RENDERER_RESTORING = 4,
    STASIS_RENDERER_RESTORE_FAILED = 5
} StasisRendererResourceState;

typedef enum {
    STASIS_RENDERER_REASON_NONE = 0,
    STASIS_RENDERER_REASON_SURFACE_CHANGED = 1,
    STASIS_RENDERER_REASON_TARGETS_RESET = 2,
    STASIS_RENDERER_REASON_DEVICE_RESET = 3,
    STASIS_RENDERER_REASON_BACKGROUND = 4,
    STASIS_RENDERER_REASON_FOREGROUND = 5
} StasisRendererResourceReason;

typedef struct {
    uint32_t surface_generation;
    uint32_t renderer_generation;
    uint32_t restore_attempts;
    uint32_t restore_failures;
    StasisRendererResourceState state;
    StasisRendererResourceReason reason;
} StasisRendererLifecycle;

static uint32_t stasis_renderer_next_generation(uint32_t generation) {
    generation += 1u;
    return generation == 0u ? 1u : generation;
}

static void stasis_renderer_lifecycle_initialize(StasisRendererLifecycle* lifecycle) {
    if (!lifecycle) return;
    lifecycle->surface_generation = 1u;
    lifecycle->renderer_generation = 1u;
    lifecycle->restore_attempts = 0u;
    lifecycle->restore_failures = 0u;
    lifecycle->state = STASIS_RENDERER_READY;
    lifecycle->reason = STASIS_RENDERER_REASON_NONE;
}

static void stasis_renderer_lifecycle_surface_changed(StasisRendererLifecycle* lifecycle) {
    if (!lifecycle || lifecycle->state == STASIS_RENDERER_UNAVAILABLE) return;
    lifecycle->surface_generation =
        stasis_renderer_next_generation(lifecycle->surface_generation);
    lifecycle->state = STASIS_RENDERER_RESTORE_PENDING;
    lifecycle->reason = STASIS_RENDERER_REASON_SURFACE_CHANGED;
}

static void stasis_renderer_lifecycle_renderer_reset(
    StasisRendererLifecycle* lifecycle,
    StasisRendererResourceReason reason
) {
    if (!lifecycle || lifecycle->state == STASIS_RENDERER_UNAVAILABLE) return;
    lifecycle->surface_generation =
        stasis_renderer_next_generation(lifecycle->surface_generation);
    lifecycle->renderer_generation =
        stasis_renderer_next_generation(lifecycle->renderer_generation);
    lifecycle->state = STASIS_RENDERER_RESTORE_PENDING;
    lifecycle->reason = reason;
}

static void stasis_renderer_lifecycle_pause(StasisRendererLifecycle* lifecycle) {
    if (!lifecycle || lifecycle->state == STASIS_RENDERER_UNAVAILABLE) return;
    lifecycle->state = STASIS_RENDERER_PAUSED;
    lifecycle->reason = STASIS_RENDERER_REASON_BACKGROUND;
}

static void stasis_renderer_lifecycle_resume(StasisRendererLifecycle* lifecycle) {
    if (!lifecycle || lifecycle->state != STASIS_RENDERER_PAUSED) return;
    lifecycle->surface_generation =
        stasis_renderer_next_generation(lifecycle->surface_generation);
    lifecycle->renderer_generation =
        stasis_renderer_next_generation(lifecycle->renderer_generation);
    lifecycle->state = STASIS_RENDERER_RESTORE_PENDING;
    lifecycle->reason = STASIS_RENDERER_REASON_FOREGROUND;
}

static int stasis_renderer_lifecycle_begin_restore(StasisRendererLifecycle* lifecycle) {
    if (!lifecycle) return 0;
    if (lifecycle->state != STASIS_RENDERER_RESTORE_PENDING &&
        lifecycle->state != STASIS_RENDERER_RESTORE_FAILED) {
        return 0;
    }
    lifecycle->restore_attempts += 1u;
    lifecycle->state = STASIS_RENDERER_RESTORING;
    return 1;
}

static void stasis_renderer_lifecycle_finish_restore(
    StasisRendererLifecycle* lifecycle,
    int succeeded
) {
    if (!lifecycle || lifecycle->state != STASIS_RENDERER_RESTORING) return;
    if (succeeded) {
        lifecycle->state = STASIS_RENDERER_READY;
    } else {
        lifecycle->restore_failures += 1u;
        lifecycle->state = STASIS_RENDERER_RESTORE_FAILED;
    }
}

static int stasis_renderer_lifecycle_can_present(const StasisRendererLifecycle* lifecycle) {
    return lifecycle && lifecycle->state == STASIS_RENDERER_READY;
}

#endif
