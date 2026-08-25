#include "stasis_display_scale.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static int close_enough(float left, float right) {
    return fabsf(left - right) < 0.001f;
}

static StasisDisplayMetrics metrics_for(
    int logical_w,
    int logical_h,
    int native_w,
    int native_h,
    int drawable_w,
    int drawable_h
) {
    StasisDisplayViewport safe = {0.0f, 0.0f, (float)native_w, (float)native_h};
    return stasis_display_metrics(
        logical_w, logical_h, native_w, native_h, drawable_w, drawable_h, safe);
}

static void test_phone_scale_preserves_logical_canvas(void) {
    StasisDisplayMetrics metrics = metrics_for(360, 720, 1080, 2400, 1080, 2400);
    CHECK(metrics.logical_w == 360);
    CHECK(metrics.logical_h == 720);
    CHECK(metrics.native_w == 1080);
    CHECK(metrics.drawable_h == 2400);
    CHECK(close_enough(metrics.content_scale, 3.0f));
    CHECK(close_enough(metrics.raster_scale, 3.0f));
    CHECK(close_enough(metrics.native_viewport.x, 0.0f));
    CHECK(close_enough(metrics.native_viewport.y, 120.0f));
    CHECK(close_enough(metrics.native_viewport.w, 1080.0f));
    CHECK(close_enough(metrics.native_viewport.h, 2160.0f));
    CHECK(stasis_display_scaled_extent(96, metrics.raster_scale) == 288);
    CHECK(stasis_display_font_atlas_extent(metrics.raster_scale) == 1024);
}

static void test_pointer_mapping_round_trips_through_letterbox(void) {
    StasisDisplayMetrics metrics = metrics_for(360, 720, 1080, 2400, 1080, 2400);
    float logical_x = 0.0f;
    float logical_y = 0.0f;
    float native_x = 0.0f;
    float native_y = 0.0f;
    stasis_display_native_to_logical_xy(
        &metrics, 540.0f, 1200.0f, &logical_x, &logical_y);
    CHECK(close_enough(logical_x, 180.0f));
    CHECK(close_enough(logical_y, 360.0f));
    stasis_display_logical_to_native_xy(
        &metrics, logical_x, logical_y, &native_x, &native_y);
    CHECK(close_enough(native_x, 540.0f));
    CHECK(close_enough(native_y, 1200.0f));
}

static void test_fractional_and_downscale_metrics_are_distinct(void) {
    StasisDisplayMetrics fractional = metrics_for(800, 600, 1200, 900, 1200, 900);
    CHECK(close_enough(fractional.content_scale, 1.5f));
    CHECK(close_enough(fractional.raster_scale, 1.5f));

    StasisDisplayMetrics downscale = metrics_for(800, 600, 640, 480, 640, 480);
    CHECK(close_enough(downscale.content_scale, 0.8f));
    CHECK(close_enough(downscale.raster_scale, 1.0f));
    CHECK(stasis_display_scaled_extent(96, downscale.raster_scale) == 96);
}

static void test_orientation_change_keeps_logical_dimensions(void) {
    StasisDisplayMetrics portrait = metrics_for(360, 720, 1080, 2400, 1080, 2400);
    StasisDisplayMetrics landscape = metrics_for(360, 720, 2400, 1080, 2400, 1080);
    CHECK(portrait.logical_w == landscape.logical_w);
    CHECK(portrait.logical_h == landscape.logical_h);
    CHECK(close_enough(landscape.content_scale, 1.5f));
    CHECK(close_enough(landscape.native_viewport.x, 930.0f));
    CHECK(close_enough(landscape.native_viewport.y, 0.0f));
}

static void test_odd_fractional_viewport_uses_renderer_rounding(void) {
    StasisDisplayMetrics metrics = metrics_for(360, 720, 2400, 1081, 2400, 1081);
    CHECK(close_enough(metrics.native_viewport.x, 929.0f));
    CHECK(close_enough(metrics.native_viewport.y, 0.0f));
    CHECK(close_enough(metrics.native_viewport.w, 541.0f));
    CHECK(close_enough(metrics.native_viewport.h, 1081.0f));

    float logical_x = -1.0f;
    float logical_y = -1.0f;
    stasis_display_native_to_logical_xy(
        &metrics, 1470.0f, 1081.0f, &logical_x, &logical_y);
    CHECK(close_enough(logical_x, 360.0f));
    CHECK(close_enough(logical_y, 720.0f));

    StasisDisplayMetrics vertical = metrics_for(
        360, 720, 1080, 2401, 1080, 2401);
    CHECK(close_enough(vertical.drawable_viewport.y, 120.0f));
    CHECK(stasis_display_bottom_origin_y(
        vertical.drawable_h, vertical.drawable_viewport) == 121);

    StasisDisplayMetrics narrow = metrics_for(800, 200, 1, 100, 1, 100);
    CHECK(close_enough(narrow.native_viewport.w, 1.0f));
    CHECK(close_enough(narrow.native_viewport.h, 1.0f));
    CHECK(close_enough(narrow.native_viewport.y, 49.0f));
    CHECK(isfinite(narrow.safe_logical_viewport.w));
    CHECK(isfinite(narrow.safe_logical_viewport.h));
}

static void test_safe_native_area_maps_to_logical_viewport(void) {
    StasisDisplayViewport safe = {0.0f, 180.0f, 1080.0f, 2040.0f};
    StasisDisplayMetrics metrics = stasis_display_metrics(
        360, 720, 1080, 2400, 1080, 2400, safe);
    CHECK(close_enough(metrics.safe_logical_viewport.x, 0.0f));
    CHECK(close_enough(metrics.safe_logical_viewport.y, 20.0f));
    CHECK(close_enough(metrics.safe_logical_viewport.w, 360.0f));
    CHECK(close_enough(metrics.safe_logical_viewport.h, 680.0f));
}

static void test_maximized_portrait_pointer_mapping(void) {
    StasisDisplayMetrics metrics = stasis_display_metrics(
        360, 720, 1920, 986, 1920, 986,
        (StasisDisplayViewport){0.0f, 0.0f, 1920.0f, 986.0f});
    CHECK(close_enough(metrics.native_viewport.x, 713.0f));
    CHECK(close_enough(metrics.native_viewport.y, 0.0f));
    CHECK(close_enough(metrics.native_viewport.w, 493.0f));
    CHECK(close_enough(metrics.native_viewport.h, 986.0f));

    float logical_x = 0.0f;
    float logical_y = 0.0f;
    stasis_display_native_to_logical_xy(
        &metrics, 959.5f, 493.0f, &logical_x, &logical_y);
    CHECK(close_enough(logical_x, 180.0f));
    CHECK(close_enough(logical_y, 360.0f));

    stasis_display_native_to_logical_xy(
        &metrics, 713.0f, 985.0f, &logical_x, &logical_y);
    CHECK(close_enough(logical_x, 0.0f));
    CHECK(logical_y > 719.0f && logical_y <= 720.0f);
}

static void test_extreme_density_and_extent_are_bounded(void) {
    StasisDisplayMetrics metrics = metrics_for(1, 1, 32768, 32768, 32768, 32768);
    CHECK(close_enough(metrics.raster_scale, 8.0f));
    CHECK(stasis_display_scaled_extent(10000, metrics.raster_scale) == 65536);
    CHECK(stasis_display_font_atlas_extent(metrics.raster_scale) == 2048);
}

static void test_font_atlas_growth_is_bounded_and_deterministic(void) {
    CHECK(stasis_display_font_atlas_next_extent(512) == 1024);
    CHECK(stasis_display_font_atlas_next_extent(1024) == 2048);
    CHECK(stasis_display_font_atlas_next_extent(2048) == 4096);
    CHECK(stasis_display_font_atlas_next_extent(4096) == 0);
    CHECK(stasis_display_font_atlas_next_extent(8192) == 0);
    CHECK(stasis_display_font_atlas_next_extent(0) == 512);
    CHECK(stasis_display_font_atlas_next_extent(513) == 1024);
}

int main(void) {
    test_phone_scale_preserves_logical_canvas();
    test_pointer_mapping_round_trips_through_letterbox();
    test_fractional_and_downscale_metrics_are_distinct();
    test_orientation_change_keeps_logical_dimensions();
    test_odd_fractional_viewport_uses_renderer_rounding();
    test_safe_native_area_maps_to_logical_viewport();
    test_maximized_portrait_pointer_mapping();
    test_extreme_density_and_extent_are_bounded();
    test_font_atlas_growth_is_bounded_and_deterministic();
    return 0;
}
