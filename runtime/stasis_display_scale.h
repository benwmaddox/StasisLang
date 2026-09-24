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

static StasisDisplayViewport stasis_display_clamp_safe_rect(
    int target_w, int target_h, StasisDisplayViewport safe
) {
    const StasisDisplayViewport full = {
        0.0f, 0.0f, (float)target_w, (float)target_h};
    if (target_w <= 0 || target_h <= 0 ||
        !isfinite(safe.x) || !isfinite(safe.y) ||
        !isfinite(safe.w) || !isfinite(safe.h)) return full;
    const float left = stasis_display_clampf(safe.x, 0.0f, (float)target_w);
    const float top = stasis_display_clampf(safe.y, 0.0f, (float)target_h);
    const float right = stasis_display_clampf(safe.x + safe.w, left, (float)target_w);
    const float bottom = stasis_display_clampf(safe.y + safe.h, top, (float)target_h);
    if (right <= left || bottom <= top) return full;
    return (StasisDisplayViewport){left, top, right - left, bottom - top};
}

static StasisDisplayViewport stasis_display_safe_drawable_rect(
    int native_w, int native_h, int drawable_w, int drawable_h,
    StasisDisplayViewport safe_native
) {
    const StasisDisplayViewport safe = stasis_display_clamp_safe_rect(
        native_w, native_h, safe_native);
    if (native_w <= 0 || native_h <= 0 || drawable_w <= 0 || drawable_h <= 0) {
        return (StasisDisplayViewport){0.0f, 0.0f, 0.0f, 0.0f};
    }
    const float sx = (float)drawable_w / (float)native_w;
    const float sy = (float)drawable_h / (float)native_h;
    const float left = ceilf(safe.x * sx);
    const float top = ceilf(safe.y * sy);
    const float right = floorf((safe.x + safe.w) * sx);
    const float bottom = floorf((safe.y + safe.h) * sy);
    if (right <= left || bottom <= top) {
        return (StasisDisplayViewport){0.0f, 0.0f, 0.0f, 0.0f};
    }
    return (StasisDisplayViewport){left, top, right - left, bottom - top};
}

/* Match SDL3 LETTERBOX aspect tolerance, floor, and half-pixel centering. */
static StasisDisplayViewport stasis_display_fit_within_rect(
    int logical_w, int logical_h, StasisDisplayViewport rect
) {
    StasisDisplayViewport fitted = {rect.x, rect.y, 0.0f, 0.0f};
    if (logical_w <= 0 || logical_h <= 0 || rect.w <= 0.0f || rect.h <= 0.0f) {
        return fitted;
    }
    const float want_aspect = (float)logical_w / (float)logical_h;
    const float real_aspect = rect.w / rect.h;
    if (fabsf(want_aspect - real_aspect) < 0.0001f) {
        fitted.w = rect.w;
        fitted.h = rect.h;
    } else if (want_aspect > real_aspect) {
        fitted.w = rect.w;
        fitted.h = floorf((float)logical_h * (rect.w / (float)logical_w));
    } else {
        fitted.w = floorf((float)logical_w * (rect.h / (float)logical_h));
        fitted.h = rect.h;
    }
    fitted.x += (rect.w - fitted.w) * 0.5f;
    fitted.y += (rect.h - fitted.h) * 0.5f;
    return fitted;
}

static StasisDisplayMetrics stasis_display_metrics_safe_fit(
    int logical_w, int logical_h, int native_w, int native_h,
    int drawable_w, int drawable_h, StasisDisplayViewport safe_native
) {
    StasisDisplayMetrics metrics = stasis_display_metrics(
        logical_w, logical_h, native_w, native_h,
        drawable_w, drawable_h, safe_native);
    const StasisDisplayViewport native_rect = stasis_display_clamp_safe_rect(
        metrics.native_w, metrics.native_h, safe_native);
    const StasisDisplayViewport drawable_rect = stasis_display_safe_drawable_rect(
        metrics.native_w, metrics.native_h, metrics.drawable_w, metrics.drawable_h,
        native_rect);
    metrics.native_viewport = stasis_display_fit_within_rect(
        metrics.logical_w, metrics.logical_h, native_rect);
    metrics.drawable_viewport = stasis_display_fit_within_rect(
        metrics.logical_w, metrics.logical_h, drawable_rect);
    const float sx = metrics.drawable_viewport.w / (float)metrics.logical_w;
    const float sy = metrics.drawable_viewport.h / (float)metrics.logical_h;
    metrics.content_scale = sx < sy ? sx : sy;
    metrics.raster_scale = stasis_display_clampf(
        metrics.content_scale, 1.0f, (float)STASIS_DISPLAY_RASTER_SCALE_MAX);
    metrics.safe_logical_viewport = (StasisDisplayViewport){
        0.0f, 0.0f, (float)metrics.logical_w, (float)metrics.logical_h};
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
#define STASIS_DISPLAY_FONT_RASTER_SCALE_MIN 2.0f

static float stasis_display_font_raster_scale(float pixel_scale) {
    if (!isfinite(pixel_scale) || pixel_scale < STASIS_DISPLAY_FONT_RASTER_SCALE_MIN) {
        return STASIS_DISPLAY_FONT_RASTER_SCALE_MIN;
    }
    return pixel_scale;
}

/* Text covers the larger rounded viewport transform. Unlike sprite
 * preparation, glyph sampling is not capped at the asset-density limit. */
static StasisDisplayPreparationScale stasis_display_text_preparation_scale(
    int logical_w,
    int logical_h,
    int viewport_w,
    int viewport_h
) {
    StasisDisplayPreparationScale scale = {2, 1};
    if (logical_w <= 0 || logical_h <= 0 || viewport_w <= 0 || viewport_h <= 0) {
        return scale;
    }
    scale.numerator = viewport_w;
    scale.denominator = logical_w;
    if ((int64_t)viewport_h * logical_w > (int64_t)viewport_w * logical_h) {
        scale.numerator = viewport_h;
        scale.denominator = logical_h;
    }
    if (scale.numerator < 2 * scale.denominator) {
        scale.numerator = 2;
        scale.denominator = 1;
    }
    const int64_t divisor = stasis_display_gcd_i64(scale.numerator, scale.denominator);
    scale.numerator /= divisor;
    scale.denominator /= divisor;
    return scale;
}

static int stasis_display_font_scaled_extent_for_backing(
    int logical_extent,
    int logical_w,
    int logical_h,
    int drawable_w,
    int drawable_h
) {
    if (logical_extent <= 0) return 0;
    if (logical_extent >= 65536) return 65536;
    StasisDisplayPreparationScale scale = stasis_display_text_preparation_scale(
        logical_w, logical_h, drawable_w, drawable_h);
    const int64_t scaled =
        ((int64_t)logical_extent * scale.numerator + scale.denominator - 1) /
        scale.denominator;
    return scaled > 65536 ? 65536 : (int)scaled;
}

static float stasis_display_font_logical_scale(int logical_extent, int raster_extent) {
    if (logical_extent <= 0 || raster_extent <= 0) {
        return STASIS_DISPLAY_FONT_RASTER_SCALE_MIN;
    }
    return (float)raster_extent / (float)logical_extent;
}

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
