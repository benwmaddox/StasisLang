#ifndef STASIS_SPRITE_ATLAS_POLICY_H
#define STASIS_SPRITE_ATLAS_POLICY_H

#include <limits.h>
#include <stdint.h>

typedef struct {
    int eligible;
    uint64_t group_id;
    uint32_t member_count;
    uint64_t logical_pixel_area;
    uint32_t max_logical_width;
    uint32_t max_logical_height;
} StasisSpriteAtlasPolicyV3;

static inline StasisSpriteAtlasPolicyV3 stasis_sprite_atlas_policy_v3_standalone(void) {
    StasisSpriteAtlasPolicyV3 policy = {0, 0, 0, 0, 0, 0};
    return policy;
}

static inline StasisSpriteAtlasPolicyV3 stasis_sprite_atlas_policy_v3_make(
    int eligible,
    uint64_t group_id,
    uint32_t member_count,
    uint64_t logical_pixel_area,
    uint32_t max_logical_width,
    uint32_t max_logical_height) {
    StasisSpriteAtlasPolicyV3 policy = stasis_sprite_atlas_policy_v3_standalone();
    if (!eligible || group_id == 0 || member_count < 2 || logical_pixel_area == 0 ||
        max_logical_width == 0 || max_logical_height == 0) {
        return policy;
    }
    policy.eligible = 1;
    policy.group_id = group_id;
    policy.member_count = member_count;
    policy.logical_pixel_area = logical_pixel_area;
    policy.max_logical_width = max_logical_width;
    policy.max_logical_height = max_logical_height;
    return policy;
}

static inline int stasis_sprite_atlas_policy_v3_equal(
    const StasisSpriteAtlasPolicyV3* left,
    const StasisSpriteAtlasPolicyV3* right) {
    return left && right && left->eligible == right->eligible &&
           left->group_id == right->group_id &&
           left->member_count == right->member_count &&
           left->logical_pixel_area == right->logical_pixel_area &&
           left->max_logical_width == right->max_logical_width &&
           left->max_logical_height == right->max_logical_height;
}

static inline int stasis_sprite_atlas_extent_fits(
    int realized_width,
    int realized_height,
    int page_width,
    int page_height,
    int max_texture_extent,
    int padding) {
    if (realized_width <= 0 || realized_height <= 0 || padding < 0 || padding > INT_MAX / 2 ||
        page_width <= 0 || page_height <= 0 || max_texture_extent <= 0) {
        return 0;
    }
    if (realized_width > INT_MAX - padding * 2 ||
        realized_height > INT_MAX - padding * 2) {
        return 0;
    }
    const int required_width = realized_width + padding * 2;
    const int required_height = realized_height + padding * 2;
    return required_width <= page_width && required_height <= page_height &&
           required_width <= max_texture_extent && required_height <= max_texture_extent;
}

static inline int stasis_sprite_atlas_realized_group_compatible(
    uint64_t left_group_id,
    uint64_t right_group_id,
    int same_format,
    int same_sampler,
    int same_backend) {
    return left_group_id != 0 && left_group_id == right_group_id &&
           same_format && same_sampler && same_backend;
}

static inline uint64_t stasis_sprite_atlas_saturating_add_u64(uint64_t left, uint64_t right) {
    return left > UINT64_MAX - right ? UINT64_MAX : left + right;
}

static inline uint64_t stasis_sprite_atlas_saturating_mul_u64(uint64_t left, uint64_t right) {
    return right != 0 && left > UINT64_MAX / right ? UINT64_MAX : left * right;
}

static inline uint64_t stasis_sprite_atlas_ceil_mul_div_u64(
    uint64_t value,
    uint64_t multiplier,
    uint64_t divisor) {
    if (divisor == 0) return UINT64_MAX;
    const uint64_t product = stasis_sprite_atlas_saturating_mul_u64(value, multiplier);
    if (product == UINT64_MAX) return UINT64_MAX;
    return product / divisor + (product % divisor != 0);
}

static inline int stasis_sprite_atlas_next_extent(int required, int cap) {
    if (required <= 0 || cap <= 0 || required > cap) return 0;
    int extent = 64 < cap ? 64 : cap;
    while (extent < required && extent <= cap / 2) extent *= 2;
    return extent < required ? cap : extent;
}

/*
 * Select the smallest deterministic page implied by the compiler's group
 * evidence and this member's realized scale. Config and device extents are
 * ceilings, not allocation requests.
 */
static inline int stasis_sprite_atlas_page_size_v3(
    const StasisSpriteAtlasPolicyV3* policy,
    int logical_width,
    int logical_height,
    int realized_width,
    int realized_height,
    int configured_max_width,
    int configured_max_height,
    int max_texture_extent,
    int padding,
    int* out_width,
    int* out_height) {
    if (!policy || !policy->eligible || logical_width <= 0 || logical_height <= 0 ||
        realized_width <= 0 || realized_height <= 0 || configured_max_width <= 0 ||
        configured_max_height <= 0 || max_texture_extent <= 0 || padding < 0 ||
        padding > INT_MAX / 2 || !out_width || !out_height) {
        return 0;
    }
    const int cap_width = configured_max_width < max_texture_extent
        ? configured_max_width : max_texture_extent;
    const int cap_height = configured_max_height < max_texture_extent
        ? configured_max_height : max_texture_extent;
    const uint64_t max_realized_width = stasis_sprite_atlas_ceil_mul_div_u64(
        policy->max_logical_width, (uint64_t)realized_width, (uint64_t)logical_width);
    const uint64_t max_realized_height = stasis_sprite_atlas_ceil_mul_div_u64(
        policy->max_logical_height, (uint64_t)realized_height, (uint64_t)logical_height);
    if (max_realized_width > (uint64_t)(INT_MAX - padding * 2) ||
        max_realized_height > (uint64_t)(INT_MAX - padding * 2)) {
        return 0;
    }
    const int required_width = (int)max_realized_width + padding * 2;
    const int required_height = (int)max_realized_height + padding * 2;
    int page_width = stasis_sprite_atlas_next_extent(required_width, cap_width);
    int page_height = stasis_sprite_atlas_next_extent(required_height, cap_height);
    if (page_width == 0 || page_height == 0) return 0;

    uint64_t target_area = stasis_sprite_atlas_ceil_mul_div_u64(
        policy->logical_pixel_area, (uint64_t)realized_width, (uint64_t)logical_width);
    target_area = stasis_sprite_atlas_ceil_mul_div_u64(
        target_area, (uint64_t)realized_height, (uint64_t)logical_height);
    const uint64_t padded_member_overhead = stasis_sprite_atlas_saturating_add_u64(
        stasis_sprite_atlas_saturating_mul_u64(
            (uint64_t)(padding * 2), max_realized_width + max_realized_height),
        (uint64_t)padding * (uint64_t)padding * 4u);
    target_area = stasis_sprite_atlas_saturating_add_u64(
        target_area,
        stasis_sprite_atlas_saturating_mul_u64(policy->member_count, padded_member_overhead));
    const uint64_t cap_area = (uint64_t)cap_width * (uint64_t)cap_height;
    if (target_area > cap_area) target_area = cap_area;

    while ((uint64_t)page_width * (uint64_t)page_height < target_area) {
        const int can_grow_width = page_width < cap_width;
        const int can_grow_height = page_height < cap_height;
        if (!can_grow_width && !can_grow_height) break;
        if (can_grow_width && (!can_grow_height || page_width <= page_height)) {
            page_width = page_width <= cap_width / 2 ? page_width * 2 : cap_width;
        } else {
            page_height = page_height <= cap_height / 2 ? page_height * 2 : cap_height;
        }
    }
    *out_width = page_width;
    *out_height = page_height;
    return 1;
}

#endif
