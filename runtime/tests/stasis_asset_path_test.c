#include "stasis_asset_path.h"

#include <stdio.h>
#include <string.h>

#define CHECK_PATH(input, expected) do { \
    char resolved[128] = {0}; \
    if (!stasis_asset_normalize_relative_path(input, resolved, sizeof(resolved)) || \
        strcmp(resolved, expected) != 0) { \
        fprintf(stderr, "asset path mismatch: %s -> %s (expected %s)\n", \
            input, resolved, expected); \
        return 1; \
    } \
} while (0)

int main(void) {
    CHECK_PATH("assets/ball.svg", "assets/ball.svg");
    CHECK_PATH("../assets/fonts/ui.ttf", "assets/fonts/ui.ttf");
    CHECK_PATH("../../assets/sprites/unit.svg", "assets/sprites/unit.svg");
    CHECK_PATH("src/../assets/ball.svg", "assets/ball.svg");
    CHECK_PATH("./assets\\ball.svg", "assets/ball.svg");
    CHECK_PATH("/assets/ball.svg", "assets/ball.svg");
    CHECK_PATH("/assets/fonts/../ball.svg", "assets/ball.svg");

    char too_small[4] = {0};
    if (stasis_asset_normalize_relative_path("assets/ball.svg", too_small, sizeof(too_small))) {
        fprintf(stderr, "asset path unexpectedly fit in bounded output\n");
        return 1;
    }
    char absolute[128] = {0};
    if (stasis_asset_normalize_relative_path("/tmp/ball.svg", absolute, sizeof(absolute)) ||
        stasis_asset_normalize_relative_path("C:\\tmp\\ball.svg", absolute, sizeof(absolute)) ||
        stasis_asset_normalize_relative_path("C:tmp\\ball.svg", absolute, sizeof(absolute)) ||
        stasis_asset_normalize_relative_path("/assets/../../tmp/ball.svg", absolute, sizeof(absolute)) ||
        stasis_asset_normalize_relative_path("\\\\server\\share\\ball.svg", absolute, sizeof(absolute))) {
        fprintf(stderr, "absolute asset path unexpectedly accepted\n");
        return 1;
    }
    return 0;
}
