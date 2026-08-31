#ifndef STASIS_SPRITE_ATLAS_POLICY_H
#define STASIS_SPRITE_ATLAS_POLICY_H

#include <limits.h>

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
    int left_width,
    int left_height,
    int right_width,
    int right_height,
    int same_format,
    int same_sampler,
    int same_backend) {
    return left_width == right_width && left_height == right_height &&
           same_format && same_sampler && same_backend;
}

#endif
