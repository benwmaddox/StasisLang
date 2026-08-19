#include "stasis_performance_metrics.h"

#include <stddef.h>
#include <stdio.h>

int main(void) {
    StasisPerformanceMetrics metrics = {0};
    metrics.version = STASIS_PERF_METRICS_VERSION;
    metrics.size = (uint32_t)sizeof(metrics);
    metrics.render_prep_us = STASIS_PERF_UNAVAILABLE;
    metrics.gpu_execution_us = STASIS_PERF_UNAVAILABLE;
    if (metrics.version != 1u || metrics.size != sizeof(metrics)
        || metrics.render_prep_us != UINT32_MAX
        || metrics.gpu_execution_us != UINT32_MAX
        || offsetof(StasisPerformanceMetrics, frame_work_us)
            >= offsetof(StasisPerformanceMetrics, present_wait_us)) {
        fputs("invalid performance metrics contract\n", stderr);
        return 1;
    }
    return 0;
}
