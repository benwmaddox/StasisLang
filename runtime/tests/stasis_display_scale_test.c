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

static void test_desktop_density_tiers_preserve_logical_geometry(void) {
    const int scales[] = {100, 125, 150, 200};
    const int drawable_widths[] = {800, 1000, 1200, 1600};
    const int drawable_heights[] = {600, 750, 900, 1200};
    for (int index = 0; index < 4; index++) {
        StasisDisplayMetrics metrics = metrics_for(
            800, 600, 800, 600,
            drawable_widths[index], drawable_heights[index]);
        const float expected_scale = (float)scales[index] / 100.0f;
        CHECK(metrics.logical_w == 800);
        CHECK(metrics.logical_h == 600);
        CHECK(close_enough(metrics.content_scale, expected_scale));
        CHECK(close_enough(metrics.raster_scale, expected_scale));
        CHECK(stasis_display_scaled_extent(33, metrics.raster_scale) ==
            (int)ceilf(33.0f * expected_scale));

        float native_x = 0.0f;
        float native_y = 0.0f;
        float logical_x = 0.0f;
        float logical_y = 0.0f;
        stasis_display_logical_to_native_xy(
            &metrics, 613.25f, 411.75f, &native_x, &native_y);
        stasis_display_native_to_logical_xy(
            &metrics, native_x, native_y, &logical_x, &logical_y);
        CHECK(close_enough(logical_x, 613.25f));
        CHECK(close_enough(logical_y, 411.75f));
    }
    CHECK(stasis_display_scaled_extent_for_backing(18, 720, 360, 1920, 986) == 48);
    CHECK(stasis_display_scaled_extent_for_backing(13, 720, 360, 1920, 986) == 35);
    CHECK(stasis_display_scaled_extent_for_backing(18, 720, 360, 1921, 986) == 49);
}

static void test_preparation_scale_is_exact_and_bounded(void) {
    const StasisDisplayPreparationScale unity =
        stasis_display_preparation_scale(2000, 1000, 2000, 1000);
    const StasisDisplayPreparationScale fractional =
        stasis_display_preparation_scale(2000, 1000, 2001, 1001);
    CHECK(unity.numerator == 1);
    CHECK(unity.denominator == 1);
    CHECK(fractional.numerator == 2001);
    CHECK(fractional.denominator == 2000);
    CHECK(stasis_display_preparation_scale_changed(unity, fractional));
    CHECK(stasis_display_scaled_extent_for_backing(2000, 2000, 1000, 2001, 1001) == 2001);

    const StasisDisplayPreparationScale over_eight =
        stasis_display_preparation_scale(100, 50, 901, 451);
    const StasisDisplayPreparationScale farther_over_eight =
        stasis_display_preparation_scale(100, 50, 1200, 600);
    CHECK(over_eight.numerator == STASIS_DISPLAY_RASTER_SCALE_MAX);
    CHECK(over_eight.denominator == 1);
    CHECK(!stasis_display_preparation_scale_changed(over_eight, farther_over_eight));
    CHECK(stasis_display_scaled_extent_for_backing(100, 100, 50, 901, 451) == 800);
    CHECK(stasis_display_scaled_extent_for_backing(9000, 100, 50, 901, 451) == 65536);
}

static void test_x11_content_scale_selects_window_backing(void) {
    CHECK(stasis_display_scaled_window_extent(720, 1.0f) == 720);
    CHECK(stasis_display_scaled_window_extent(360, 1.0f) == 360);
    CHECK(stasis_display_scaled_window_extent(720, 1.25f) == 900);
    CHECK(stasis_display_scaled_window_extent(360, 1.25f) == 450);
    CHECK(stasis_display_scaled_window_extent(720, 1.5f) == 1080);
    CHECK(stasis_display_scaled_window_extent(360, 1.5f) == 540);
    CHECK(stasis_display_scaled_window_extent(720, 2.0f) == 1440);
    CHECK(stasis_display_scaled_window_extent(360, 2.0f) == 720);
    CHECK(stasis_display_scaled_window_extent(720, 0.5f) == 720);
    CHECK(stasis_display_scaled_window_extent(10000, 20.0f) == 65536);
}

static void test_x11_scale_control_requires_an_explicit_valid_factor(void) {
    CHECK(stasis_display_scale_control_is_valid("1.0"));
    CHECK(stasis_display_scale_control_is_valid("1.25"));
    CHECK(stasis_display_scale_control_is_valid("1.5"));
    CHECK(stasis_display_scale_control_is_valid("2.0"));
    CHECK(!stasis_display_scale_control_is_valid(NULL));
    CHECK(!stasis_display_scale_control_is_valid(""));
    CHECK(!stasis_display_scale_control_is_valid("0"));
    CHECK(!stasis_display_scale_control_is_valid("-1"));
    CHECK(!stasis_display_scale_control_is_valid("not-a-scale"));
    CHECK(!stasis_display_scale_control_is_valid("1.25x"));
}

static void test_explicit_window_extent_survives_stale_maximized_state(void) {
    CHECK(stasis_display_should_apply_windowed_extent(1, 0, 1, 0));
    CHECK(stasis_display_should_apply_windowed_extent(1, 0, 0, 1));
    CHECK(!stasis_display_should_apply_windowed_extent(1, 1, 0, 0));
    CHECK(!stasis_display_should_apply_windowed_extent(0, 0, 1, 0));
    CHECK(!stasis_display_should_apply_windowed_extent(0, 0, 0, 1));
    CHECK(stasis_display_should_apply_windowed_extent(0, 0, 0, 0));
}

static void test_full_backing_and_fitted_content_remain_distinct(void) {
    StasisDisplayMetrics portrait = metrics_for(
        360, 720, 1920, 960, 1920, 960);
    CHECK(portrait.drawable_w == 1920);
    CHECK(portrait.drawable_h == 960);
    CHECK(close_enough(portrait.drawable_viewport.x, 720.0f));
    CHECK(close_enough(portrait.drawable_viewport.y, 0.0f));
    CHECK(close_enough(portrait.drawable_viewport.w, 480.0f));
    CHECK(close_enough(portrait.drawable_viewport.h, 960.0f));
    CHECK(close_enough(portrait.raster_scale, 4.0f / 3.0f));

    StasisDisplayMetrics landscape = metrics_for(
        720, 360, 1920, 960, 1920, 960);
    CHECK(landscape.drawable_w == 1920);
    CHECK(landscape.drawable_h == 960);
    CHECK(close_enough(landscape.drawable_viewport.x, 0.0f));
    CHECK(close_enough(landscape.drawable_viewport.y, 0.0f));
    CHECK(close_enough(landscape.drawable_viewport.w, 1920.0f));
    CHECK(close_enough(landscape.drawable_viewport.h, 960.0f));
    CHECK(close_enough(landscape.raster_scale, 8.0f / 3.0f));
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
    CHECK(close_enough(stasis_display_font_raster_scale(0.5f), 2.0f));
    CHECK(close_enough(stasis_display_font_raster_scale(1.0f), 2.0f));
    CHECK(close_enough(stasis_display_font_raster_scale(1.5f), 2.0f));
    CHECK(close_enough(stasis_display_font_raster_scale(3.0f), 3.0f));
    CHECK(close_enough(stasis_display_font_raster_scale(20.0f), 20.0f));
    CHECK(stasis_display_scaled_extent(13, stasis_display_font_raster_scale(1.0f)) == 26);
    CHECK(stasis_display_font_scaled_extent_for_backing(18, 720, 360, 1920, 986) == 50);
    CHECK(stasis_display_font_scaled_extent_for_backing(18, 800, 600, 1200, 900) == 36);
    CHECK(stasis_display_font_scaled_extent_for_backing(18, 360, 720, 1081, 2161) == 55);
    CHECK(stasis_display_font_scaled_extent_for_backing(13, 64, 64, 1280, 1280) == 260);
    CHECK(close_enough(stasis_display_font_logical_scale(18, 55), 55.0f / 18.0f));
    StasisDisplayPreparationScale above_sprite_cap =
        stasis_display_text_preparation_scale(64, 64, 640, 640);
    CHECK(above_sprite_cap.numerator == 10 && above_sprite_cap.denominator == 1);
    CHECK(stasis_display_font_atlas_extent(stasis_display_font_raster_scale(1.0f)) == 1024);
    CHECK(stasis_display_font_atlas_next_extent(512) == 1024);
    CHECK(stasis_display_font_atlas_next_extent(1024) == 2048);
    CHECK(stasis_display_font_atlas_next_extent(2048) == 4096);
    CHECK(stasis_display_font_atlas_next_extent(4096) == 0);
    CHECK(stasis_display_font_atlas_next_extent(8192) == 0);
    CHECK(stasis_display_font_atlas_next_extent(0) == 512);
    CHECK(stasis_display_font_atlas_next_extent(513) == 1024);
}

static void test_wide_game_uses_maximal_centered_surface(void) {
    const struct {
        int width;
        int height;
        int x;
        int y;
        int content_width;
        int content_height;
    } cases[] = {
        {1280, 720, 0, 72, 1280, 576},
        {1920, 1080, 0, 108, 1920, 864},
        {2560, 1440, 0, 144, 2560, 1152},
        {2560, 1080, 80, 0, 2400, 1080},
        {844, 390, 0, 5, 844, 380},
        {390, 844, 0, 334, 390, 176},
    };
    for (size_t index = 0; index < sizeof(cases) / sizeof(cases[0]); index++) {
        const StasisDisplayMetrics metrics = metrics_for(
            1600, 720, cases[index].width, cases[index].height,
            cases[index].width * 2, cases[index].height * 2);
        CHECK(close_enough(metrics.native_viewport.x, (float)cases[index].x));
        CHECK(close_enough(metrics.native_viewport.y, (float)cases[index].y));
        CHECK(close_enough(metrics.native_viewport.w, (float)cases[index].content_width));
        CHECK(close_enough(metrics.native_viewport.h, (float)cases[index].content_height));
        CHECK(close_enough(metrics.drawable_viewport.x, 2.0f * cases[index].x));
        CHECK(close_enough(metrics.drawable_viewport.y, 2.0f * cases[index].y));
        CHECK(close_enough(metrics.drawable_viewport.w, 2.0f * cases[index].content_width));
        CHECK(fabsf(metrics.drawable_viewport.h - 2.0f * cases[index].content_height) <= 1.0f);
        const float points[][2] = {{0.0f, 0.0f}, {800.0f, 360.0f}, {1600.0f, 720.0f}};
        for (size_t point = 0; point < sizeof(points) / sizeof(points[0]); point++) {
            float native_x = -1.0f;
            float native_y = -1.0f;
            float logical_x = -1.0f;
            float logical_y = -1.0f;
            stasis_display_logical_to_native_xy(
                &metrics, points[point][0], points[point][1], &native_x, &native_y);
            stasis_display_native_to_logical_xy(
                &metrics, native_x, native_y, &logical_x, &logical_y);
            CHECK(close_enough(logical_x, points[point][0]));
            CHECK(close_enough(logical_y, points[point][1]));
        }
    }
}

static void test_mobile_safe_fit_uses_usable_surface_and_pointer_edges(void) {
    const StasisDisplayViewport safe = {100.0f, 20.0f, 2200.0f, 1040.0f};
    const StasisDisplayMetrics metrics = stasis_display_metrics_safe_fit(
        1600, 720, 2400, 1080, 4800, 2160, safe);
    CHECK(close_enough(metrics.native_viewport.x, 100.0f));
    CHECK(close_enough(metrics.native_viewport.y, 45.0f));
    CHECK(close_enough(metrics.native_viewport.w, 2200.0f));
    CHECK(close_enough(metrics.native_viewport.h, 990.0f));
    CHECK(close_enough(metrics.drawable_viewport.x, 200.0f));
    CHECK(close_enough(metrics.drawable_viewport.y, 90.0f));
    CHECK(close_enough(metrics.drawable_viewport.w, 4400.0f));
    CHECK(close_enough(metrics.drawable_viewport.h, 1980.0f));
    CHECK(close_enough(metrics.safe_logical_viewport.x, 0.0f));
    CHECK(close_enough(metrics.safe_logical_viewport.y, 0.0f));
    CHECK(close_enough(metrics.safe_logical_viewport.w, 1600.0f));
    CHECK(close_enough(metrics.safe_logical_viewport.h, 720.0f));
    const float points[][2] = {{0.0f, 0.0f}, {800.0f, 360.0f}, {1600.0f, 720.0f}};
    for (size_t point = 0; point < sizeof(points) / sizeof(points[0]); point++) {
        float native_x = -1.0f;
        float native_y = -1.0f;
        float logical_x = -1.0f;
        float logical_y = -1.0f;
        stasis_display_logical_to_native_xy(
            &metrics, points[point][0], points[point][1], &native_x, &native_y);
        stasis_display_native_to_logical_xy(
            &metrics, native_x, native_y, &logical_x, &logical_y);
        CHECK(close_enough(logical_x, points[point][0]));
        CHECK(close_enough(logical_y, points[point][1]));
    }
}

static void test_mobile_safe_fit_handles_rotation_and_no_drawable_pixel(void) {
    const StasisDisplayMetrics portrait = stasis_display_metrics_safe_fit(
        360, 720, 1080, 2400, 2160, 4800,
        (StasisDisplayViewport){0.0f, 100.0f, 1080.0f, 2100.0f});
    CHECK(close_enough(portrait.native_viewport.x, 15.0f));
    CHECK(close_enough(portrait.native_viewport.y, 100.0f));
    CHECK(close_enough(portrait.native_viewport.w, 1050.0f));
    CHECK(close_enough(portrait.native_viewport.h, 2100.0f));
    CHECK(close_enough(portrait.content_scale, 35.0f / 6.0f));
    const StasisDisplayMetrics fractional = stasis_display_metrics_safe_fit(
        1600, 720, 390, 844, 780, 1688,
        (StasisDisplayViewport){0.0f, 0.0f, 390.0f, 844.0f});
    CHECK(close_enough(fractional.native_viewport.y, 334.5f));
    CHECK(close_enough(fractional.native_viewport.h, 175.0f));
    CHECK(close_enough(fractional.drawable_viewport.y, 668.5f));
    CHECK(close_enough(fractional.drawable_viewport.h, 351.0f));
    const StasisDisplayViewport fractional_edges = stasis_display_safe_drawable_rect(
        1000, 1000, 1501, 1501,
        (StasisDisplayViewport){3.0f, 5.0f, 900.0f, 900.0f});
    CHECK(close_enough(fractional_edges.x, 5.0f));
    CHECK(close_enough(fractional_edges.y, 8.0f));
    CHECK(close_enough(fractional_edges.w, 1350.0f));
    CHECK(close_enough(fractional_edges.h, 1350.0f));
    const StasisDisplayViewport near_equal = stasis_display_fit_within_rect(
        1600, 720, (StasisDisplayViewport){0.0f, 0.0f, 50000.0f, 22501.0f});
    CHECK(close_enough(near_equal.w, 50000.0f));
    CHECK(close_enough(near_equal.h, 22501.0f));
    const StasisDisplayViewport empty = stasis_display_safe_drawable_rect(
        1000, 1000, 1, 1,
        (StasisDisplayViewport){100.0f, 100.0f, 100.0f, 100.0f});
    CHECK(close_enough(empty.w, 0.0f));
    CHECK(close_enough(empty.h, 0.0f));
}

static void test_sprite_preparation_uses_physical_axis_and_instance_transform(void) {
    StasisDisplayPreparationScale scale = stasis_display_sprite_preparation_scale(
        1600, 720, 2531, 1140);
    CHECK(scale.numerator == 19);
    CHECK(scale.denominator == 12);
    CHECK(stasis_display_sprite_scaled_extent(32, scale, 1.0) == 51);
    CHECK(stasis_display_sprite_scaled_extent(32, scale, 1.5) == 76);

    scale = stasis_display_sprite_preparation_scale(100, 100, 901, 899);
    CHECK(scale.numerator == 901);
    CHECK(scale.denominator == 100);
    CHECK(stasis_display_sprite_scaled_extent(10, scale, 1.0) == 91);

    scale = stasis_display_sprite_preparation_scale(100, 100, 50, 40);
    CHECK(scale.numerator == 1);
    CHECK(scale.denominator == 1);
    CHECK(stasis_display_sprite_scaled_extent(10, scale, 0.5) == 10);

    scale.numerator = 0;
    scale.denominator = 0;
    CHECK(stasis_display_sprite_scaled_extent(10, scale, 1.0) == 10);
}

int main(void) {
    test_phone_scale_preserves_logical_canvas();
    test_wide_game_uses_maximal_centered_surface();
    test_mobile_safe_fit_uses_usable_surface_and_pointer_edges();
    test_mobile_safe_fit_handles_rotation_and_no_drawable_pixel();
    test_pointer_mapping_round_trips_through_letterbox();
    test_fractional_and_downscale_metrics_are_distinct();
    test_desktop_density_tiers_preserve_logical_geometry();
    test_preparation_scale_is_exact_and_bounded();
    test_x11_content_scale_selects_window_backing();
    test_x11_scale_control_requires_an_explicit_valid_factor();
    test_explicit_window_extent_survives_stale_maximized_state();
    test_full_backing_and_fitted_content_remain_distinct();
    test_orientation_change_keeps_logical_dimensions();
    test_odd_fractional_viewport_uses_renderer_rounding();
    test_safe_native_area_maps_to_logical_viewport();
    test_maximized_portrait_pointer_mapping();
    test_extreme_density_and_extent_are_bounded();
    test_font_atlas_growth_is_bounded_and_deterministic();
    test_sprite_preparation_uses_physical_axis_and_instance_transform();
    return 0;
}
