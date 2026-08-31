#include "stasis_cross_atlas_prototype.h"

#include <limits.h>
#include <string.h>

static uint32_t mix_u32(uint32_t hash, uint32_t value) {
    for (unsigned shift = 0; shift < 32; shift += 8) {
        hash ^= (value >> shift) & 0xffu;
        hash *= 16777619u;
    }
    return hash;
}

static uint32_t order_hash(const StasisCrossAtlasInstance *instances, size_t count) {
    uint32_t hash = 2166136261u;
    for (size_t index = 0; index < count; index++) {
        hash = mix_u32(hash, instances[index].order);
        hash = mix_u32(hash, instances[index].resource_id);
    }
    return hash;
}

static int same_binding_domain(
    StasisCrossAtlasBinding binding,
    const StasisCrossAtlasInstance *left,
    const StasisCrossAtlasInstance *right
) {
    if (binding == STASIS_CROSS_ATLAS_BINDLESS) return 1;
    if (binding == STASIS_CROSS_ATLAS_MEGA_ATLAS ||
        binding == STASIS_CROSS_ATLAS_TEXTURE_ARRAY) {
        return left->binding_domain_id == right->binding_domain_id;
    }
    return left->resource_id == right->resource_id &&
        left->binding_domain_id == right->binding_domain_id;
}

static StasisCrossAtlasSplitReason split_reason(
    const StasisCrossAtlasProfile *profile,
    const StasisCrossAtlasInstance *previous,
    const StasisCrossAtlasInstance *current,
    uint32_t run_length
) {
    if (run_length >= profile->max_instances_per_draw) return STASIS_CROSS_ATLAS_SPLIT_CAPACITY;
    if (previous->pass_id != current->pass_id) return STASIS_CROSS_ATLAS_SPLIT_PASS;
    if (previous->clip_id != current->clip_id) return STASIS_CROSS_ATLAS_SPLIT_CLIP;
    if (previous->material_id != current->material_id) return STASIS_CROSS_ATLAS_SPLIT_MATERIAL;
    if (previous->blend_mode != current->blend_mode || previous->filter_mode != current->filter_mode) {
        return STASIS_CROSS_ATLAS_SPLIT_BLEND_FILTER;
    }
    if (!same_binding_domain(profile->binding, previous, current)) {
        return profile->binding == STASIS_CROSS_ATLAS_CONVENTIONAL
            ? STASIS_CROSS_ATLAS_SPLIT_TEXTURE
            : STASIS_CROSS_ATLAS_SPLIT_BINDING_DOMAIN;
    }
    return STASIS_CROSS_ATLAS_SPLIT_FRAME_START;
}

static StasisCrossAtlasCounters baseline_counters(
    const StasisCrossAtlasInstance *instances,
    size_t count
) {
    StasisCrossAtlasCounters counters = {0};
    if (count == 0) return counters;
    counters.upload_bytes = count * STASIS_CROSS_ATLAS_INSTANCE_BYTES;
    counters.upload_calls = (uint32_t)count;
    counters.draw_calls = (uint32_t)count;
    counters.texture_binds = 1;
    counters.queue_submissions = 1;
    for (size_t index = 1; index < count; index++) {
        if (!same_binding_domain(
                STASIS_CROSS_ATLAS_CONVENTIONAL,
                &instances[index - 1],
                &instances[index])) counters.texture_binds++;
        if (instances[index - 1].pass_id != instances[index].pass_id) counters.pass_changes++;
    }
    return counters;
}

static StasisCrossAtlasPlan fallback_plan(
    const StasisCrossAtlasProfile *profile,
    const StasisCrossAtlasInstance *instances,
    size_t count,
    StasisCrossAtlasFallbackReason reason
) {
    StasisCrossAtlasPlan plan;
    memset(&plan, 0, sizeof(plan));
    if (instances != NULL && count <= STASIS_CROSS_ATLAS_SAFE_MAX_INSTANCES) {
        plan.baseline = baseline_counters(instances, count);
        plan.prototype = plan.baseline;
        plan.order_hash = order_hash(instances, count);
        plan.input_count = (uint32_t)count;
    }
    if (profile != NULL &&
        profile->queue_submission_mode == STASIS_CROSS_ATLAS_QUEUE_UNAVAILABLE) {
        plan.baseline.queue_submissions = UINT32_MAX;
        plan.prototype.queue_submissions = UINT32_MAX;
    }
    plan.fallback_reason = reason;
    return plan;
}

StasisCrossAtlasPlan stasis_cross_atlas_plan(
    const StasisCrossAtlasProfile *profile,
    const StasisCrossAtlasInstance *instances,
    size_t instance_count,
    StasisCrossAtlasRun *runs,
    size_t run_capacity,
    int inject_upload_failure
) {
    if (profile == NULL || (instance_count > 0 && instances == NULL) ||
        profile->max_instances_per_draw == 0) {
        return fallback_plan(profile, instances, instance_count, STASIS_CROSS_ATLAS_FALLBACK_INVALID_ARGUMENT);
    }
    if (instance_count > STASIS_CROSS_ATLAS_SAFE_MAX_INSTANCES || instance_count > UINT32_MAX) {
        return fallback_plan(profile, NULL, 0, STASIS_CROSS_ATLAS_FALLBACK_SAFE_MAXIMUM);
    }
    for (size_t index = 0; index < instance_count; index++) {
        if ((instances[index].feature_flags & ~profile->supported_feature_flags) != 0) {
            return fallback_plan(profile, instances, instance_count, STASIS_CROSS_ATLAS_FALLBACK_UNSUPPORTED_FEATURE);
        }
    }
    if (inject_upload_failure) {
        return fallback_plan(profile, instances, instance_count, STASIS_CROSS_ATLAS_FALLBACK_UPLOAD_FAILURE);
    }

    StasisCrossAtlasPlan plan;
    memset(&plan, 0, sizeof(plan));
    plan.baseline = baseline_counters(instances, instance_count);
    plan.input_count = (uint32_t)instance_count;
    plan.order_hash = order_hash(instances, instance_count);
    plan.prototype_used = 1;
    if (instance_count == 0) return plan;

    size_t needed_runs = 1;
    uint32_t run_length = 1;
    for (size_t index = 1; index < instance_count; index++) {
        StasisCrossAtlasSplitReason reason = split_reason(profile, &instances[index - 1], &instances[index], run_length);
        if (reason != STASIS_CROSS_ATLAS_SPLIT_FRAME_START) {
            needed_runs++;
            run_length = 1;
        } else {
            run_length++;
        }
    }
    if (runs == NULL || run_capacity < needed_runs) {
        return fallback_plan(profile, instances, instance_count, STASIS_CROSS_ATLAS_FALLBACK_OUTPUT_CAPACITY);
    }

    size_t run_index = 0;
    runs[0].first_instance = 0;
    runs[0].instance_count = 1;
    runs[0].reason_before = STASIS_CROSS_ATLAS_SPLIT_FRAME_START;
    for (size_t index = 1; index < instance_count; index++) {
        StasisCrossAtlasSplitReason reason = split_reason(
            profile, &instances[index - 1], &instances[index], runs[run_index].instance_count);
        if (reason == STASIS_CROSS_ATLAS_SPLIT_FRAME_START) {
            runs[run_index].instance_count++;
        } else {
            run_index++;
            runs[run_index].first_instance = (uint32_t)index;
            runs[run_index].instance_count = 1;
            runs[run_index].reason_before = reason;
        }
    }
    plan.run_count = (uint32_t)(run_index + 1);
    plan.prototype.upload_bytes = instance_count * STASIS_CROSS_ATLAS_INSTANCE_BYTES;
    plan.prototype.upload_calls = profile->one_frame_upload ? 1u : plan.run_count;
    plan.prototype.draw_calls = plan.run_count;
    plan.prototype.texture_binds = 1u;
    for (size_t index = 1; index < instance_count; index++) {
        if (!same_binding_domain(profile->binding, &instances[index - 1], &instances[index])) {
            plan.prototype.texture_binds++;
        }
    }
    plan.prototype.pass_changes = plan.baseline.pass_changes;
    if (profile->queue_submission_mode == STASIS_CROSS_ATLAS_QUEUE_ONE) {
        plan.prototype.queue_submissions = 1u;
    } else if (profile->queue_submission_mode == STASIS_CROSS_ATLAS_QUEUE_PER_RUN) {
        plan.prototype.queue_submissions = plan.run_count;
    } else {
        plan.baseline.queue_submissions = UINT32_MAX;
        plan.prototype.queue_submissions = UINT32_MAX;
    }
    return plan;
}

const char *stasis_cross_atlas_split_reason_name(StasisCrossAtlasSplitReason reason) {
    switch (reason) {
        case STASIS_CROSS_ATLAS_SPLIT_FRAME_START: return "frame_start";
        case STASIS_CROSS_ATLAS_SPLIT_TEXTURE: return "texture";
        case STASIS_CROSS_ATLAS_SPLIT_BINDING_DOMAIN: return "binding_domain";
        case STASIS_CROSS_ATLAS_SPLIT_CLIP: return "clip";
        case STASIS_CROSS_ATLAS_SPLIT_PASS: return "pass";
        case STASIS_CROSS_ATLAS_SPLIT_MATERIAL: return "material";
        case STASIS_CROSS_ATLAS_SPLIT_BLEND_FILTER: return "blend_filter";
        case STASIS_CROSS_ATLAS_SPLIT_CAPACITY: return "capacity";
        default: return "unknown";
    }
}

const char *stasis_cross_atlas_fallback_reason_name(StasisCrossAtlasFallbackReason reason) {
    switch (reason) {
        case STASIS_CROSS_ATLAS_FALLBACK_NONE: return "none";
        case STASIS_CROSS_ATLAS_FALLBACK_INVALID_ARGUMENT: return "invalid_argument";
        case STASIS_CROSS_ATLAS_FALLBACK_SAFE_MAXIMUM: return "safe_maximum";
        case STASIS_CROSS_ATLAS_FALLBACK_UNSUPPORTED_FEATURE: return "unsupported_feature";
        case STASIS_CROSS_ATLAS_FALLBACK_UPLOAD_FAILURE: return "upload_failure";
        case STASIS_CROSS_ATLAS_FALLBACK_OUTPUT_CAPACITY: return "output_capacity";
        default: return "unknown";
    }
}
