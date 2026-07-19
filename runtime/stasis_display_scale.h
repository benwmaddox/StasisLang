#ifndef STASIS_DISPLAY_SCALE_H
#define STASIS_DISPLAY_SCALE_H

#include <math.h>

static float stasis_display_pixel_scale(
    int logical_w,
    int logical_h,
    int drawable_w,
    int drawable_h
) {
    if (logical_w <= 0 || logical_h <= 0 || drawable_w <= 0 || drawable_h <= 0) {
        return 1.0f;
    }

    float scale_x = (float)drawable_w / (float)logical_w;
    float scale_y = (float)drawable_h / (float)logical_h;
    float scale = scale_x < scale_y ? scale_x : scale_y;
    if (scale < 1.0f) return 1.0f;
    if (scale > 8.0f) return 8.0f;
    return scale;
}

static int stasis_display_scaled_extent(int logical_extent, float pixel_scale) {
    if (logical_extent <= 0) return 0;
    if (pixel_scale < 1.0f) pixel_scale = 1.0f;
    double scaled = ceil((double)logical_extent * (double)pixel_scale);
    if (scaled > 65536.0) return 65536;
    return (int)scaled;
}

static int stasis_display_logical_stroke_samples(float pixel_scale) {
    if (pixel_scale < 1.0f) pixel_scale = 1.0f;
    if (pixel_scale > 8.0f) pixel_scale = 8.0f;
    return (int)ceilf(pixel_scale);
}

static float stasis_display_native_to_logical(
    float native_value,
    int native_extent,
    int logical_extent
) {
    if (native_extent <= 0 || logical_extent <= 0) return native_value;
    return native_value * (float)logical_extent / (float)native_extent;
}

static int stasis_display_font_atlas_extent(float pixel_scale) {
    if (pixel_scale <= 1.0f) return 512;
    if (pixel_scale <= 4.0f) return 1024;
    return 2048;
}

#endif
