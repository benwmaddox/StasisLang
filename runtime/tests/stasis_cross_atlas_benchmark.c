#include "stasis_cross_atlas_prototype.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <time.h>
#endif

#define FIXTURE_COUNT 4096u
#define SAMPLE_COUNT 31u
#define ITERATIONS_PER_SAMPLE 1000u

static uint64_t now_ns(void) {
#ifdef _WIN32
    LARGE_INTEGER frequency;
    LARGE_INTEGER counter;
    QueryPerformanceFrequency(&frequency);
    QueryPerformanceCounter(&counter);
    return (uint64_t)((counter.QuadPart * 1000000000ull) / frequency.QuadPart);
#else
    struct timespec value;
    clock_gettime(CLOCK_MONOTONIC, &value);
    return (uint64_t)value.tv_sec * 1000000000ull + (uint64_t)value.tv_nsec;
#endif
}

static int compare_u64(const void *left, const void *right) {
    uint64_t a = *(const uint64_t *)left;
    uint64_t b = *(const uint64_t *)right;
    return (a > b) - (a < b);
}

static void fill_fixture(StasisCrossAtlasInstance *instances) {
    memset(instances, 0, sizeof(*instances) * FIXTURE_COUNT);
    for (uint32_t index = 0; index < FIXTURE_COUNT; index++) {
        instances[index].destination[0] = (float)(index % 128u);
        instances[index].destination[1] = (float)(index / 128u);
        instances[index].destination[2] = 16.0f;
        instances[index].destination[3] = 16.0f;
        instances[index].uv_crop[2] = 1.0f;
        instances[index].uv_crop[3] = 1.0f;
        instances[index].scale[0] = 1.0f;
        instances[index].scale[1] = 1.0f;
        instances[index].tint_rgba = 0xffffffffu;
        instances[index].resource_id = index % 8u;
        instances[index].binding_domain_id = 1u;
        instances[index].order = index;
    }
}

static void benchmark_profile(
    const StasisCrossAtlasProfile *profile,
    const StasisCrossAtlasInstance *instances,
    StasisCrossAtlasRun *runs,
    int last
) {
    uint64_t samples[SAMPLE_COUNT];
    uint64_t sorted[SAMPLE_COUNT];
    StasisCrossAtlasPlan plan;
    volatile uint32_t guard = 0;
    for (uint32_t sample = 0; sample < SAMPLE_COUNT; sample++) {
        uint64_t start = now_ns();
        for (uint32_t iteration = 0; iteration < ITERATIONS_PER_SAMPLE; iteration++) {
            plan = stasis_cross_atlas_plan(profile, instances, FIXTURE_COUNT, runs, FIXTURE_COUNT, 0);
            guard ^= plan.order_hash;
        }
        samples[sample] = (now_ns() - start) / ITERATIONS_PER_SAMPLE;
    }
    memcpy(sorted, samples, sizeof(samples));
    qsort(sorted, SAMPLE_COUNT, sizeof(sorted[0]), compare_u64);
    printf("    {\"profile\":\"%s\",\"binding\":%u,\"fixture_instances\":%u,",
        profile->name, (unsigned)profile->binding, FIXTURE_COUNT);
    printf("\"planner_ns\":{\"min\":%llu,\"p50\":%llu,\"p95\":%llu,\"max\":%llu},",
        (unsigned long long)sorted[0], (unsigned long long)sorted[15],
        (unsigned long long)sorted[29], (unsigned long long)sorted[30]);
    fputs("\"raw_samples_ns\":[", stdout);
    for (uint32_t index = 0; index < SAMPLE_COUNT; index++) {
        printf("%s%llu", index == 0 ? "" : ",", (unsigned long long)samples[index]);
    }
    fputs("],", stdout);
    printf("\"modeled\":{\"baseline_upload_calls\":%u,\"baseline_upload_bytes\":%llu,",
        plan.baseline.upload_calls, (unsigned long long)plan.baseline.upload_bytes);
    printf("\"baseline_texture_binds\":%u,\"baseline_draws\":%u,",
        plan.baseline.texture_binds, plan.baseline.draw_calls);
    printf("\"prototype_upload_calls\":%u,\"prototype_upload_bytes\":%llu,",
        plan.prototype.upload_calls, (unsigned long long)plan.prototype.upload_bytes);
    printf("\"prototype_texture_binds\":%u,\"prototype_draws\":%u,\"queue_submissions\":",
        plan.prototype.texture_binds, plan.prototype.draw_calls);
    if (plan.prototype.queue_submissions == UINT32_MAX) fputs("null", stdout);
    else printf("%u", plan.prototype.queue_submissions);
    fputs("},", stdout);
    printf("\"gpu_frame_time\":null,\"guard\":%u}%s\n", guard, last ? "" : ",");
}

int main(void) {
    StasisCrossAtlasInstance *instances = malloc(sizeof(*instances) * FIXTURE_COUNT);
    StasisCrossAtlasRun *runs = malloc(sizeof(*runs) * FIXTURE_COUNT);
    if (instances == NULL || runs == NULL) return 2;
    fill_fixture(instances);
    const StasisCrossAtlasProfile profiles[] = {
        {"desktop_native_bindless", STASIS_CROSS_ATLAS_BINDLESS, 65535, 0, 1, 1},
        {"android_texture_array", STASIS_CROSS_ATLAS_TEXTURE_ARRAY, 4096, 0, 1, 1},
        {"webgl2_texture_array", STASIS_CROSS_ATLAS_TEXTURE_ARRAY, 1024, 0, 1, 1},
        {"canvas_conventional", STASIS_CROSS_ATLAS_CONVENTIONAL, 4096, 0, 0, STASIS_CROSS_ATLAS_QUEUE_UNAVAILABLE}
    };
    puts("{");
    puts("  \"schema\":1,");
    puts("  \"method\":{\"samples\":31,\"iterations_per_sample\":1000,\"quantiles\":\"nearest rank after ascending sort: p50=index15, p95=index29\",\"gpu_measurement\":\"unavailable\"},");
    puts("  \"profiles\":[");
    for (size_t index = 0; index < sizeof(profiles) / sizeof(profiles[0]); index++) {
        benchmark_profile(&profiles[index], instances, runs,
            index + 1 == sizeof(profiles) / sizeof(profiles[0]));
    }
    puts("  ]");
    puts("}");
    free(runs);
    free(instances);
    return 0;
}
