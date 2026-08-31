#include "stasis_cross_atlas_prototype.h"

#include <stdio.h>
#include <string.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at line %d: %s\n", __LINE__, #condition); \
        return 1; \
    } \
} while (0)

static StasisCrossAtlasProfile profile(StasisCrossAtlasBinding binding, uint32_t capacity) {
    StasisCrossAtlasProfile result = {
        "test", binding, capacity, 0x0fu, 1, 1
    };
    return result;
}

static StasisCrossAtlasInstance sprite(uint32_t order, uint32_t resource, uint16_t binding_domain) {
    StasisCrossAtlasInstance result;
    memset(&result, 0, sizeof(result));
    result.destination[0] = (float)order;
    result.destination[2] = 16.0f;
    result.destination[3] = 16.0f;
    result.uv_crop[2] = 1.0f;
    result.uv_crop[3] = 1.0f;
    result.scale[0] = 1.0f;
    result.scale[1] = 1.0f;
    result.tint_rgba = 0xffffffffu;
    result.resource_id = resource;
    result.binding_domain_id = binding_domain;
    result.order = order;
    return result;
}

static int test_order_and_full_semantics(void) {
    StasisCrossAtlasInstance instances[3] = {
        sprite(20, 1, 0), sprite(10, 2, 1), sprite(30, 1, 0)
    };
    instances[0].tint_rgba = 0x40ffffffu;
    instances[1].destination[0] = 19.25f;
    instances[1].destination[1] = -3.5f;
    instances[1].uv_crop[0] = 0.125f;
    instances[1].uv_crop[1] = 0.25f;
    instances[1].uv_crop[2] = 0.5f;
    instances[1].uv_crop[3] = 0.75f;
    instances[1].pivot[0] = 4.0f;
    instances[1].pivot[1] = 8.0f;
    instances[1].scale[0] = -2.0f;
    instances[1].scale[1] = 3.0f;
    instances[1].rotation = 0.75f;
    instances[1].tint_rgba = 0x7f3366ccu;
    instances[1].flags = 3u;
    StasisCrossAtlasInstance before[3];
    memcpy(before, instances, sizeof(instances));

    StasisCrossAtlasRun runs[3];
    StasisCrossAtlasProfile bindless = profile(STASIS_CROSS_ATLAS_BINDLESS, 32);
    StasisCrossAtlasPlan plan = stasis_cross_atlas_plan(
        &bindless, instances, 3, runs, 3, 0);
    CHECK(plan.prototype_used);
    CHECK(plan.run_count == 1);
    CHECK(runs[0].first_instance == 0 && runs[0].instance_count == 3);
    CHECK(memcmp(before, instances, sizeof(instances)) == 0);
    CHECK(plan.order_hash == 0x198908e7u);
    CHECK(plan.prototype.upload_calls == 1 && plan.prototype.upload_bytes == 240);
    CHECK(plan.prototype.draw_calls == 1 && plan.prototype.queue_submissions == 1);
    return 0;
}

static int test_split_trace(void) {
    StasisCrossAtlasInstance instances[7];
    for (uint32_t index = 0; index < 7; index++) instances[index] = sprite(index, 1, 0);
    instances[1].clip_id = 2;
    instances[2].clip_id = 2; instances[2].material_id = 3;
    instances[3].clip_id = 2; instances[3].material_id = 3; instances[3].blend_mode = 1;
    instances[4] = instances[3]; instances[4].order = 4; instances[4].pass_id = 1;
    instances[5] = instances[4]; instances[5].order = 5;
    instances[6] = instances[5]; instances[6].order = 6;
    StasisCrossAtlasRun runs[7];
    StasisCrossAtlasProfile bindless = profile(STASIS_CROSS_ATLAS_BINDLESS, 2);
    StasisCrossAtlasPlan plan = stasis_cross_atlas_plan(&bindless, instances, 7, runs, 7, 0);
    CHECK(plan.run_count == 6);
    CHECK(runs[1].reason_before == STASIS_CROSS_ATLAS_SPLIT_CLIP);
    CHECK(runs[2].reason_before == STASIS_CROSS_ATLAS_SPLIT_MATERIAL);
    CHECK(runs[3].reason_before == STASIS_CROSS_ATLAS_SPLIT_BLEND_FILTER);
    CHECK(runs[4].reason_before == STASIS_CROSS_ATLAS_SPLIT_PASS);
    CHECK(runs[5].reason_before == STASIS_CROSS_ATLAS_SPLIT_CAPACITY);
    CHECK(runs[4].instance_count == 2 && runs[5].instance_count == 1);
    CHECK(plan.prototype.pass_changes == 1);
    return 0;
}

static int test_binding_domains(void) {
    StasisCrossAtlasInstance instances[4] = {
        sprite(0, 10, 7), sprite(1, 11, 7), sprite(2, 10, 7), sprite(3, 11, 7)
    };
    StasisCrossAtlasRun runs[4];
    StasisCrossAtlasProfile conventional = profile(STASIS_CROSS_ATLAS_CONVENTIONAL, 16);
    StasisCrossAtlasPlan conventional_plan = stasis_cross_atlas_plan(
        &conventional, instances, 4, runs, 4, 0);
    CHECK(conventional_plan.run_count == 4);
    CHECK(runs[1].reason_before == STASIS_CROSS_ATLAS_SPLIT_TEXTURE);
    CHECK(conventional_plan.prototype.texture_binds == 4);

    StasisCrossAtlasProfile array = profile(STASIS_CROSS_ATLAS_TEXTURE_ARRAY, 16);
    StasisCrossAtlasPlan array_plan = stasis_cross_atlas_plan(&array, instances, 4, runs, 4, 0);
    CHECK(array_plan.run_count == 1);
    CHECK(array_plan.prototype.texture_binds == 1);

    instances[2].binding_domain_id = 8;
    instances[3].binding_domain_id = 8;
    array_plan = stasis_cross_atlas_plan(&array, instances, 4, runs, 4, 0);
    CHECK(array_plan.run_count == 2);
    CHECK(runs[1].reason_before == STASIS_CROSS_ATLAS_SPLIT_BINDING_DOMAIN);
    CHECK(array_plan.prototype.texture_binds == 2);

    StasisCrossAtlasProfile mega = profile(STASIS_CROSS_ATLAS_MEGA_ATLAS, 16);
    StasisCrossAtlasPlan mega_plan = stasis_cross_atlas_plan(&mega, instances, 4, runs, 4, 0);
    CHECK(mega_plan.run_count == 2);
    CHECK(runs[1].reason_before == STASIS_CROSS_ATLAS_SPLIT_BINDING_DOMAIN);

    StasisCrossAtlasProfile bindless = profile(STASIS_CROSS_ATLAS_BINDLESS, 16);
    StasisCrossAtlasPlan bindless_plan = stasis_cross_atlas_plan(
        &bindless, instances, 4, runs, 4, 0);
    CHECK(bindless_plan.run_count == 1);
    CHECK(bindless_plan.prototype.texture_binds == 1);
    return 0;
}

static int test_transactional_fallbacks(void) {
    StasisCrossAtlasInstance instances[2] = {sprite(0, 1, 0), sprite(1, 2, 0)};
    StasisCrossAtlasRun runs[2] = {{99, 99, STASIS_CROSS_ATLAS_SPLIT_CAPACITY}};
    StasisCrossAtlasProfile bindless = profile(STASIS_CROSS_ATLAS_BINDLESS, 16);
    instances[1].feature_flags = 0x80u;
    StasisCrossAtlasPlan unsupported = stasis_cross_atlas_plan(&bindless, instances, 2, runs, 2, 0);
    CHECK(!unsupported.prototype_used);
    CHECK(unsupported.fallback_reason == STASIS_CROSS_ATLAS_FALLBACK_UNSUPPORTED_FEATURE);
    CHECK(unsupported.run_count == 0);
    CHECK(unsupported.prototype.draw_calls == unsupported.baseline.draw_calls);
    CHECK(runs[0].first_instance == 99);

    instances[1].feature_flags = 0;
    StasisCrossAtlasPlan failed = stasis_cross_atlas_plan(&bindless, instances, 2, runs, 2, 1);
    CHECK(!failed.prototype_used);
    CHECK(failed.fallback_reason == STASIS_CROSS_ATLAS_FALLBACK_UPLOAD_FAILURE);
    CHECK(failed.run_count == 0 && failed.prototype.draw_calls == 2);

    StasisCrossAtlasPlan no_space = stasis_cross_atlas_plan(&bindless, instances, 2, runs, 0, 0);
    CHECK(!no_space.prototype_used);
    CHECK(no_space.fallback_reason == STASIS_CROSS_ATLAS_FALLBACK_OUTPUT_CAPACITY);
    CHECK(no_space.run_count == 0);
    return 0;
}

static int test_safe_maximum_and_layout(void) {
    CHECK(sizeof(StasisCrossAtlasInstance) == 80);
    CHECK(offsetof(StasisCrossAtlasInstance, tint_rgba) == 52);
    CHECK(offsetof(StasisCrossAtlasInstance, resource_id) == 56);
    CHECK(offsetof(StasisCrossAtlasInstance, clip_id) == 64);
    CHECK(offsetof(StasisCrossAtlasInstance, binding_domain_id) == 68);
    CHECK(offsetof(StasisCrossAtlasInstance, feature_flags) == 76);
    StasisCrossAtlasInstance one = sprite(0, 1, 0);
    StasisCrossAtlasRun run;
    StasisCrossAtlasProfile bindless = profile(STASIS_CROSS_ATLAS_BINDLESS, 1);
    StasisCrossAtlasPlan too_many = stasis_cross_atlas_plan(
        &bindless, &one, STASIS_CROSS_ATLAS_SAFE_MAX_INSTANCES + 1u, &run, 1, 0);
    CHECK(!too_many.prototype_used);
    CHECK(too_many.fallback_reason == STASIS_CROSS_ATLAS_FALLBACK_SAFE_MAXIMUM);
    return 0;
}

int main(void) {
    CHECK(test_order_and_full_semantics() == 0);
    CHECK(test_split_trace() == 0);
    CHECK(test_binding_domains() == 0);
    CHECK(test_transactional_fallbacks() == 0);
    CHECK(test_safe_maximum_and_layout() == 0);
    puts("cross-atlas prototype contract: ok");
    return 0;
}
