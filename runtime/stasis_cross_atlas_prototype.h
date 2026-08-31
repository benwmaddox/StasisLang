#ifndef STASIS_CROSS_ATLAS_PROTOTYPE_H
#define STASIS_CROSS_ATLAS_PROTOTYPE_H

#include <stddef.h>
#include <stdint.h>

#define STASIS_CROSS_ATLAS_INSTANCE_BYTES 80u
#define STASIS_CROSS_ATLAS_SAFE_MAX_INSTANCES ((size_t)(UINT32_MAX / STASIS_CROSS_ATLAS_INSTANCE_BYTES))

typedef struct StasisCrossAtlasInstance {
    float destination[4];
    float uv_crop[4];
    float pivot[2];
    float scale[2];
    float rotation;
    uint32_t tint_rgba;
    uint32_t resource_id;
    uint32_t order;
    uint32_t clip_id;
    uint16_t binding_domain_id;
    uint16_t material_id;
    uint8_t blend_mode;
    uint8_t filter_mode;
    uint8_t pass_id;
    uint8_t flags;
    uint32_t feature_flags;
} StasisCrossAtlasInstance;

_Static_assert(sizeof(StasisCrossAtlasInstance) == STASIS_CROSS_ATLAS_INSTANCE_BYTES,
    "cross-atlas instance layout must remain exactly 80 bytes");

typedef enum StasisCrossAtlasBinding {
    STASIS_CROSS_ATLAS_CONVENTIONAL = 0,
    STASIS_CROSS_ATLAS_MEGA_ATLAS = 1,
    STASIS_CROSS_ATLAS_TEXTURE_ARRAY = 2,
    STASIS_CROSS_ATLAS_BINDLESS = 3
} StasisCrossAtlasBinding;

typedef enum StasisCrossAtlasQueueMode {
    STASIS_CROSS_ATLAS_QUEUE_UNAVAILABLE = 0,
    STASIS_CROSS_ATLAS_QUEUE_ONE = 1,
    STASIS_CROSS_ATLAS_QUEUE_PER_RUN = 2
} StasisCrossAtlasQueueMode;

typedef enum StasisCrossAtlasSplitReason {
    STASIS_CROSS_ATLAS_SPLIT_FRAME_START = 0,
    STASIS_CROSS_ATLAS_SPLIT_TEXTURE = 1,
    STASIS_CROSS_ATLAS_SPLIT_BINDING_DOMAIN = 2,
    STASIS_CROSS_ATLAS_SPLIT_CLIP = 3,
    STASIS_CROSS_ATLAS_SPLIT_PASS = 4,
    STASIS_CROSS_ATLAS_SPLIT_MATERIAL = 5,
    STASIS_CROSS_ATLAS_SPLIT_BLEND_FILTER = 6,
    STASIS_CROSS_ATLAS_SPLIT_CAPACITY = 7
} StasisCrossAtlasSplitReason;

typedef enum StasisCrossAtlasFallbackReason {
    STASIS_CROSS_ATLAS_FALLBACK_NONE = 0,
    STASIS_CROSS_ATLAS_FALLBACK_INVALID_ARGUMENT = 1,
    STASIS_CROSS_ATLAS_FALLBACK_SAFE_MAXIMUM = 2,
    STASIS_CROSS_ATLAS_FALLBACK_UNSUPPORTED_FEATURE = 3,
    STASIS_CROSS_ATLAS_FALLBACK_UPLOAD_FAILURE = 4,
    STASIS_CROSS_ATLAS_FALLBACK_OUTPUT_CAPACITY = 5
} StasisCrossAtlasFallbackReason;

typedef struct StasisCrossAtlasProfile {
    const char *name;
    StasisCrossAtlasBinding binding;
    uint32_t max_instances_per_draw;
    uint32_t supported_feature_flags;
    uint8_t one_frame_upload;
    uint8_t queue_submission_mode;
} StasisCrossAtlasProfile;

typedef struct StasisCrossAtlasRun {
    uint32_t first_instance;
    uint32_t instance_count;
    StasisCrossAtlasSplitReason reason_before;
} StasisCrossAtlasRun;

typedef struct StasisCrossAtlasCounters {
    uint64_t upload_bytes;
    uint32_t upload_calls;
    uint32_t texture_binds;
    uint32_t draw_calls;
    uint32_t pass_changes;
    uint32_t queue_submissions;
} StasisCrossAtlasCounters;

typedef struct StasisCrossAtlasPlan {
    StasisCrossAtlasCounters baseline;
    StasisCrossAtlasCounters prototype;
    uint32_t run_count;
    uint32_t input_count;
    uint32_t order_hash;
    uint8_t prototype_used;
    StasisCrossAtlasFallbackReason fallback_reason;
} StasisCrossAtlasPlan;

StasisCrossAtlasPlan stasis_cross_atlas_plan(
    const StasisCrossAtlasProfile *profile,
    const StasisCrossAtlasInstance *instances,
    size_t instance_count,
    StasisCrossAtlasRun *runs,
    size_t run_capacity,
    int inject_upload_failure
);

const char *stasis_cross_atlas_split_reason_name(StasisCrossAtlasSplitReason reason);
const char *stasis_cross_atlas_fallback_reason_name(StasisCrossAtlasFallbackReason reason);

#endif
