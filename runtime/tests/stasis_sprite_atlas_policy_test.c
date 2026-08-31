#include "stasis_sprite_atlas_policy.h"

#include <stdio.h>

#define CHECK(expression) do { \
    if (!(expression)) { \
        fprintf(stderr, "CHECK failed: %s:%d: %s\n", __FILE__, __LINE__, #expression); \
        return 1; \
    } \
} while (0)

int main(void) {
    CHECK(stasis_sprite_atlas_extent_fits(2048, 2048, 4096, 4096, 4096, 2));
    CHECK(stasis_sprite_atlas_extent_fits(4096, 4096, 8192, 8192, 8192, 2));
    CHECK(!stasis_sprite_atlas_extent_fits(4096, 4096, 8192, 8192, 2048, 2));
    CHECK(!stasis_sprite_atlas_extent_fits(2048, 2048, 2048, 2048, 2048, 2));
    /* Mixed realized sizes are compatible when the compiler group identity agrees. */
    CHECK(stasis_sprite_atlas_realized_group_compatible(101, 101, 1, 1, 1));
    CHECK(!stasis_sprite_atlas_realized_group_compatible(101, 202, 1, 1, 1));
    CHECK(!stasis_sprite_atlas_realized_group_compatible(
        101, 101, 1, 0, 1));

    StasisSpriteAtlasPolicyV3 invalid = stasis_sprite_atlas_policy_v3_make(
        1, 0, 8, 8u * 256u * 256u, 256, 256);
    CHECK(!invalid.eligible);
    StasisSpriteAtlasPolicyV3 policy = stasis_sprite_atlas_policy_v3_make(
        1, 101, 8, 8u * 256u * 256u, 512, 256);
    CHECK(policy.eligible);
    CHECK(stasis_sprite_atlas_policy_v3_equal(&policy, &policy));

    int page_w = 0;
    int page_h = 0;
    CHECK(stasis_sprite_atlas_page_size_v3(
        &policy, 256, 256, 256, 256, 2048, 2048, 4096, 2, &page_w, &page_h));
    CHECK(page_w == 1024 && page_h == 1024);
    CHECK(page_w < 2048 && page_h < 2048);

    /* Device/config caps are ceilings and too-large groups spill to more pages. */
    StasisSpriteAtlasPolicyV3 large_group = stasis_sprite_atlas_policy_v3_make(
        1, 202, 64, 64u * 512u * 512u, 512, 512);
    CHECK(stasis_sprite_atlas_page_size_v3(
        &large_group, 512, 512, 512, 512, 1024, 2048, 4096, 2, &page_w, &page_h));
    CHECK(page_w == 1024 && page_h == 2048);
    CHECK(!stasis_sprite_atlas_page_size_v3(
        &policy, 256, 256, 4096, 4096, 2048, 2048, 2048, 2, &page_w, &page_h));
    return 0;
}
