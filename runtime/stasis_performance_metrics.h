#ifndef STASIS_PERFORMANCE_METRICS_H
#define STASIS_PERFORMANCE_METRICS_H

#include <stdint.h>
#include <stddef.h>

#define STASIS_PERF_METRICS_VERSION 1u
#define STASIS_PERF_UNAVAILABLE UINT32_MAX
#define STASIS_PERF_BACKEND_MAX 16

/* Stable phase order shared by web, desktop, Android, and iOS HUDs. */
typedef struct StasisPerformanceMetrics {
    uint32_t version;
    uint32_t size;
    uint32_t tick_us;
    uint32_t guest_render_us;
    uint32_t host_replay_us;
    uint32_t render_prep_us;
    uint32_t gpu_submit_us;
    uint32_t gpu_execution_us;
    uint32_t frame_work_us;
    uint32_t present_wait_us;
    uint32_t commands;
    uint32_t lines;
    uint32_t rectangles;
    uint32_t sprites;
    uint32_t text;
    uint32_t instances;
    uint32_t batches;
    uint32_t draw_calls;
    uint32_t texture_switches;
    uint32_t uploaded_bytes;
    char backend[STASIS_PERF_BACKEND_MAX];
} StasisPerformanceMetrics;

#endif
