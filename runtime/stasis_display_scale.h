#ifndef STASIS_DISPLAY_SCALE_H
#define STASIS_DISPLAY_SCALE_H

#include <math.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct {
    float x;
    float y;
    float w;
    float h;
} StasisDisplayViewport;

typedef struct {
    int logical_w;
    int logical_h;
    int native_w;
    int native_h;
    int drawable_w;
    int drawable_h;
    StasisDisplayViewport native_viewport;
    StasisDisplayViewport drawable_viewport;
    StasisDisplayViewport safe_logical_viewport;
    float content_scale;
    float raster_scale;
} StasisDisplayMetrics;

typedef struct {
    int64_t numerator;
    int64_t denominator;
} StasisDisplayPreparationScale;

#define STASIS_DISPLAY_RASTER_SCALE_MAX 8

static float stasis_display_clampf(float value, float minimum, float maximum) {
    if (value < minimum) return minimum;
    if (value > maximum) return maximum;
    return value;
}

static StasisDisplayViewport stasis_display_fit_viewport(
    int logical_w,
    int logical_h,
    int target_w,
    int target_h
) {
    StasisDisplayViewport viewport = {0.0f, 0.0f, 0.0f, 0.0f};
    if (logical_w <= 0 || logical_h <= 0 || target_w <= 0 || target_h <= 0) {
        return viewport;
    }

    const float scale_x = (float)target_w / (float)logical_w;
    const float scale_y = (float)target_h / (float)logical_h;
    const float scale = scale_x < scale_y ? scale_x : scale_y;
    viewport.w = floorf((float)logical_w * scale + 0.5f);
    viewport.h = floorf((float)logical_h * scale + 0.5f);
    if (viewport.w < 1.0f) viewport.w = 1.0f;
    if (viewport.h < 1.0f) viewport.h = 1.0f;
    viewport.x = floorf(((float)target_w - viewport.w) * 0.5f);
    viewport.y = floorf(((float)target_h - viewport.h) * 0.5f);
    return viewport;
}

static int stasis_display_bottom_origin_y(
    int target_h,
    StasisDisplayViewport viewport
) {
    return target_h - (int)viewport.y - (int)viewport.h;
}

static StasisDisplayMetrics stasis_display_metrics(
    int logical_w,
    int logical_h,
    int native_w,
    int native_h,
    int drawable_w,
    int drawable_h,
    StasisDisplayViewport safe_native_viewport
) {
    StasisDisplayMetrics metrics;
    metrics.logical_w = logical_w > 0 ? logical_w : 1;
    metrics.logical_h = logical_h > 0 ? logical_h : 1;
    metrics.native_w = native_w > 0 ? native_w : metrics.logical_w;
    metrics.native_h = native_h > 0 ? native_h : metrics.logical_h;
    metrics.drawable_w = drawable_w > 0 ? drawable_w : metrics.native_w;
    metrics.drawable_h = drawable_h > 0 ? drawable_h : metrics.native_h;
    metrics.native_viewport = stasis_display_fit_viewport(
        metrics.logical_w, metrics.logical_h, metrics.native_w, metrics.native_h);
    metrics.drawable_viewport = stasis_display_fit_viewport(
        metrics.logical_w, metrics.logical_h, metrics.drawable_w, metrics.drawable_h);
    const float drawable_scale_x =
        metrics.drawable_viewport.w / (float)metrics.logical_w;
    const float drawable_scale_y =
        metrics.drawable_viewport.h / (float)metrics.logical_h;
    metrics.content_scale = drawable_scale_x < drawable_scale_y
        ? drawable_scale_x : drawable_scale_y;
    metrics.raster_scale = stasis_display_clampf(
        metrics.content_scale, 1.0f, (float)STASIS_DISPLAY_RASTER_SCALE_MAX);

    if (safe_native_viewport.w <= 0.0f || safe_native_viewport.h <= 0.0f) {
        safe_native_viewport.x = 0.0f;
        safe_native_viewport.y = 0.0f;
        safe_native_viewport.w = (float)metrics.native_w;
        safe_native_viewport.h = (float)metrics.native_h;
    }

    const float content_left = metrics.native_viewport.x;
    const float content_top = metrics.native_viewport.y;
    const float content_right = content_left + metrics.native_viewport.w;
    const float content_bottom = content_top + metrics.native_viewport.h;
    const float safe_left = stasis_display_clampf(
        safe_native_viewport.x, content_left, content_right);
    const float safe_top = stasis_display_clampf(
        safe_native_viewport.y, content_top, content_bottom);
    const float safe_right = stasis_display_clampf(
        safe_native_viewport.x + safe_native_viewport.w, safe_left, content_right);
    const float safe_bottom = stasis_display_clampf(
        safe_native_viewport.y + safe_native_viewport.h, safe_top, content_bottom);
    const float logical_per_native_x =
        (float)metrics.logical_w / metrics.native_viewport.w;
    const float logical_per_native_y =
        (float)metrics.logical_h / metrics.native_viewport.h;
    metrics.safe_logical_viewport.x =
        (safe_left - content_left) * logical_per_native_x;
    metrics.safe_logical_viewport.y =
        (safe_top - content_top) * logical_per_native_y;
    metrics.safe_logical_viewport.w =
        (safe_right - safe_left) * logical_per_native_x;
    metrics.safe_logical_viewport.h =
        (safe_bottom - safe_top) * logical_per_native_y;
    return metrics;
}

static void stasis_display_native_to_logical_xy(
    const StasisDisplayMetrics* metrics,
    float native_x,
    float native_y,
    float* logical_x,
    float* logical_y
) {
    if (!metrics || !logical_x || !logical_y ||
        metrics->native_viewport.w <= 0.0f || metrics->native_viewport.h <= 0.0f) {
        return;
    }
    *logical_x = (native_x - metrics->native_viewport.x) *
        (float)metrics->logical_w / metrics->native_viewport.w;
    *logical_y = (native_y - metrics->native_viewport.y) *
        (float)metrics->logical_h / metrics->native_viewport.h;
}

static void stasis_display_logical_to_native_xy(
    const StasisDisplayMetrics* metrics,
    float logical_x,
    float logical_y,
    float* native_x,
    float* native_y
) {
    if (!metrics || !native_x || !native_y ||
        metrics->logical_w <= 0 || metrics->logical_h <= 0) {
        return;
    }
    *native_x = metrics->native_viewport.x + logical_x *
        metrics->native_viewport.w / (float)metrics->logical_w;
    *native_y = metrics->native_viewport.y + logical_y *
        metrics->native_viewport.h / (float)metrics->logical_h;
}

static float stasis_display_pixel_scale(
    int logical_w,
    int logical_h,
    int drawable_w,
    int drawable_h
) {
    StasisDisplayViewport safe = {0.0f, 0.0f, (float)drawable_w, (float)drawable_h};
    return stasis_display_metrics(
        logical_w, logical_h, drawable_w, drawable_h, drawable_w, drawable_h, safe).raster_scale;
}

static int stasis_display_scaled_extent(int logical_extent, float pixel_scale) {
    if (logical_extent <= 0) return 0;
    if (pixel_scale < 1.0f) pixel_scale = 1.0f;
    double scaled = ceil((double)logical_extent * (double)pixel_scale);
    if (scaled > 65536.0) return 65536;
    return (int)scaled;
}

static int64_t stasis_display_gcd_i64(int64_t left, int64_t right) {
    while (right != 0) {
        const int64_t remainder = left % right;
        left = right;
        right = remainder;
    }
    return left;
}

static StasisDisplayPreparationScale stasis_display_preparation_scale(
    int logical_w,
    int logical_h,
    int drawable_w,
    int drawable_h
) {
    StasisDisplayPreparationScale scale = {1, 1};
    if (logical_w <= 0 || logical_h <= 0 || drawable_w <= 0 || drawable_h <= 0) {
        return scale;
    }
    scale.numerator = drawable_w;
    scale.denominator = logical_w;
    if ((int64_t)drawable_h * logical_w < (int64_t)drawable_w * logical_h) {
        scale.numerator = drawable_h;
        scale.denominator = logical_h;
    }
    if (scale.numerator < scale.denominator) scale.numerator = scale.denominator;
    const int64_t maximum_numerator =
        scale.denominator * STASIS_DISPLAY_RASTER_SCALE_MAX;
    if (scale.numerator > maximum_numerator) scale.numerator = maximum_numerator;
    const int64_t divisor = stasis_display_gcd_i64(scale.numerator, scale.denominator);
    scale.numerator /= divisor;
    scale.denominator /= divisor;
    return scale;
}

static int stasis_display_preparation_scale_changed(
    StasisDisplayPreparationScale previous,
    StasisDisplayPreparationScale next
) {
    return previous.numerator != next.numerator || previous.denominator != next.denominator;
}

static int stasis_display_scaled_extent_for_backing(
    int logical_extent,
    int logical_w,
    int logical_h,
    int drawable_w,
    int drawable_h
) {
    if (logical_extent <= 0) return 0;
    if (logical_extent >= 65536) return 65536;
    const StasisDisplayPreparationScale scale = stasis_display_preparation_scale(
        logical_w, logical_h, drawable_w, drawable_h);
    const int64_t scaled =
        ((int64_t)logical_extent * scale.numerator + scale.denominator - 1) /
        scale.denominator;
    return scaled > 65536 ? 65536 : (int)scaled;
}

static int stasis_display_scaled_window_extent(int logical_extent, float display_scale) {
    if (logical_extent <= 0) return 0;
    if (!isfinite(display_scale) || display_scale < 1.0f) display_scale = 1.0f;
    if (display_scale > (float)STASIS_DISPLAY_RASTER_SCALE_MAX) {
        display_scale = (float)STASIS_DISPLAY_RASTER_SCALE_MAX;
    }
    const double scaled = ceil((double)logical_extent * (double)display_scale);
    return scaled > 65536.0 ? 65536 : (int)scaled;
}

static int stasis_display_scale_control_is_valid(const char* value) {
    if (!value || !*value) return 0;
    char* end = NULL;
    const double parsed = strtod(value, &end);
    return end != value && *end == 0 && isfinite(parsed) && parsed > 0.0;
}

static int stasis_display_should_apply_windowed_extent(
    int explicit_window_request,
    int fullscreen,
    int maximized,
    int minimized
) {
    if (fullscreen) return 0;
    if (explicit_window_request) return 1;
    return !maximized && !minimized;
}

#define STASIS_DISPLAY_FONT_ATLAS_MIN_EXTENT 512
#define STASIS_DISPLAY_FONT_ATLAS_MAX_EXTENT 4096

static int stasis_display_font_atlas_extent(float pixel_scale) {
    if (pixel_scale <= 1.0f) return STASIS_DISPLAY_FONT_ATLAS_MIN_EXTENT;
    if (pixel_scale <= 4.0f) return 1024;
    return 2048;
}

/* Return the next bounded power-of-two atlas size, or zero at the cap. */
static int stasis_display_font_atlas_next_extent(int atlas_extent) {
    if (atlas_extent < STASIS_DISPLAY_FONT_ATLAS_MIN_EXTENT) {
        return STASIS_DISPLAY_FONT_ATLAS_MIN_EXTENT;
    }
    if (atlas_extent >= STASIS_DISPLAY_FONT_ATLAS_MAX_EXTENT) return 0;
    int next_extent = STASIS_DISPLAY_FONT_ATLAS_MIN_EXTENT;
    while (next_extent <= atlas_extent) {
        if (next_extent > STASIS_DISPLAY_FONT_ATLAS_MAX_EXTENT / 2) {
            return STASIS_DISPLAY_FONT_ATLAS_MAX_EXTENT;
        }
        next_extent *= 2;
    }
    return next_extent;
}

#endif
