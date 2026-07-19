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

static void test_phone_scale_uses_uniform_letterbox_factor(void) {
    float scale = stasis_display_pixel_scale(360, 720, 1080, 2400);
    CHECK(close_enough(scale, 3.0f));
    CHECK(stasis_display_scaled_extent(96, scale) == 288);
    CHECK(stasis_display_scaled_extent(128, scale) == 384);
    CHECK(stasis_display_font_atlas_extent(scale) == 1024);
    CHECK(stasis_display_logical_stroke_samples(scale) == 3);
}

static void test_native_coordinates_map_to_logical_space(void) {
    CHECK(close_enough(stasis_display_native_to_logical(540.0f, 1080, 360), 180.0f));
    CHECK(close_enough(stasis_display_native_to_logical(1200.0f, 2400, 720), 360.0f));
}

static void test_low_resolution_never_downsamples_asset_bakes(void) {
    float scale = stasis_display_pixel_scale(800, 600, 640, 480);
    CHECK(close_enough(scale, 1.0f));
    CHECK(stasis_display_scaled_extent(96, scale) == 96);
    CHECK(stasis_display_font_atlas_extent(scale) == 512);
    CHECK(stasis_display_logical_stroke_samples(scale) == 1);
}

static void test_extreme_density_is_bounded(void) {
    float scale = stasis_display_pixel_scale(1, 1, 32768, 32768);
    CHECK(close_enough(scale, 8.0f));
    CHECK(stasis_display_scaled_extent(10000, scale) == 65536);
    CHECK(stasis_display_font_atlas_extent(scale) == 2048);
    CHECK(stasis_display_logical_stroke_samples(scale) == 8);
}

static void test_fractional_density_covers_a_full_logical_stroke(void) {
    CHECK(stasis_display_logical_stroke_samples(2.625f) == 3);
}

int main(void) {
    test_phone_scale_uses_uniform_letterbox_factor();
    test_native_coordinates_map_to_logical_space();
    test_low_resolution_never_downsamples_asset_bakes();
    test_extreme_density_is_bounded();
    test_fractional_density_covers_a_full_logical_stroke();
    return 0;
}
