#include "../stasis_mixed_quad_planner.h"

#include <math.h>

#define CHECK(condition) do { if (!(condition)) return __LINE__; } while (0)

int main(void) {
    const int32_t abab_same[] = {0, 0, 0, 0};
    StasisMixedQuadPlan plan = stasis_mixed_quad_plan_domains(abab_same, 4, 32, 0);
    CHECK(plan.submissions == 1 && plan.domain_transitions == 0);

    const int32_t abab_forced[] = {0, 1, 0, 1};
    plan = stasis_mixed_quad_plan_domains(abab_forced, 4, 32, 0);
    CHECK(plan.submissions == 4 && plan.domain_transitions == 3);

    const int32_t abcacb_same[] = {0, 0, STASIS_MIXED_SOLID_DOMAIN,
        0, STASIS_MIXED_SOLID_DOMAIN, 0};
    plan = stasis_mixed_quad_plan_domains(abcacb_same, 6, 32, 0);
    CHECK(plan.submissions == 1 && plan.domain_transitions == 0);

    const int32_t leading_solids[] = {STASIS_MIXED_SOLID_DOMAIN,
        STASIS_MIXED_SOLID_DOMAIN, 3, STASIS_MIXED_SOLID_DOMAIN};
    plan = stasis_mixed_quad_plan_domains(leading_solids, 4, 32, 0);
    CHECK(plan.submissions == 1 && plan.domain_transitions == 0);

    const int32_t forced_with_solids[] = {0, STASIS_MIXED_SOLID_DOMAIN, 1,
        STASIS_MIXED_SOLID_DOMAIN, 0, 1};
    plan = stasis_mixed_quad_plan_domains(forced_with_solids, 6, 32, 0);
    CHECK(plan.submissions == 4 && plan.domain_transitions == 3);

    plan = stasis_mixed_quad_plan_domains(abab_same, 4, 2, 0);
    CHECK(plan.submissions == 2 && plan.domain_transitions == 0);

    float points[8];
    stasis_mixed_quad_transform(10, 20, 8, 4, 2, 1, -2, 3, 0, points);
    CHECK(fabsf(points[0] - 16.0f) < 0.001f);
    CHECK(fabsf(points[1] - 18.0f) < 0.001f);
    CHECK(fabsf(points[4] - 0.0f) < 0.001f);
    CHECK(fabsf(points[5] - 30.0f) < 0.001f);

    StasisRasterCrop crop;
    CHECK(stasis_logical_crop_to_raster(10, 5, 20, 10, 100, 50, 200, 100, &crop));
    CHECK(fabsf(crop.x - 20.0f) < 0.001f);
    CHECK(fabsf(crop.y - 10.0f) < 0.001f);
    CHECK(fabsf(crop.w - 40.0f) < 0.001f);
    CHECK(fabsf(crop.h - 20.0f) < 0.001f);

    CHECK(stasis_logical_crop_to_raster(7, 3, 11, 6, 80, 30, 120, 100, &crop));
    CHECK(fabsf(crop.x - 10.5f) < 0.001f);
    CHECK(fabsf(crop.y - 10.0f) < 0.001f);
    CHECK(fabsf(crop.w - 16.5f) < 0.001f);
    CHECK(fabsf(crop.h - 20.0f) < 0.001f);

    CHECK(stasis_logical_crop_to_raster(0, 0, 0, 0, 80, 30, 120, 100, &crop));
    CHECK(crop.x == 0.0f && crop.y == 0.0f && crop.w == 120.0f && crop.h == 100.0f);
    CHECK(stasis_logical_crop_to_raster(13, 17, 0, 0, 80, 30, 120, 100, &crop));
    CHECK(crop.x == 0.0f && crop.y == 0.0f && crop.w == 120.0f && crop.h == 100.0f);
    CHECK(stasis_logical_crop_to_raster(70, 24, 10, 6, 80, 30, 120, 100, &crop));
    CHECK(fabsf(crop.x - 105.0f) < 0.001f);
    CHECK(fabsf(crop.y - 80.0f) < 0.001f);
    CHECK(fabsf(crop.w - 15.0f) < 0.001f);
    CHECK(fabsf(crop.h - 20.0f) < 0.001f);

    CHECK(!stasis_logical_crop_to_raster(1, 0, 0, 0, 80, 30, 120, 100, &crop));
    CHECK(!stasis_logical_crop_to_raster(0, 0, 10, 0, 80, 30, 120, 100, &crop));
    CHECK(!stasis_logical_crop_to_raster(71, 24, 10, 6, 80, 30, 120, 100, &crop));
    CHECK(!stasis_logical_crop_to_raster(NAN, 0, 10, 10, 80, 30, 120, 100, &crop));
    CHECK(!stasis_logical_crop_to_raster(0, 0, 10, 10, 0, 30, 120, 100, &crop));
    return 0;
}
