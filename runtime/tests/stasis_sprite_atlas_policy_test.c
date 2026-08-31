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
    CHECK(stasis_sprite_atlas_realized_group_compatible(
        2048, 2048, 2048, 2048, 1, 1, 1));
    CHECK(!stasis_sprite_atlas_realized_group_compatible(
        2048, 2048, 4096, 4096, 1, 1, 1));
    CHECK(!stasis_sprite_atlas_realized_group_compatible(
        2048, 2048, 2048, 2048, 1, 0, 1));
    return 0;
}
