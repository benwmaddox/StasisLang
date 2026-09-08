#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_gfx_load_sprite(const char* path, int max_w, int max_h);
int stasis_test_get_sprite_state(int handle, int32_t* out_i32, int32_t capacity);

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static void load(int width, int height, int32_t state[18]) {
    const int handle = stasis_gfx_load_sprite(STASIS_TEST_SPRITE_PATH, width, height);
    CHECK(handle > 0);
    CHECK(stasis_test_get_sprite_state(handle, state, 18) == 1);
    CHECK(state[7] == width && state[8] == height);
}

static int overlaps(const int32_t a[18], const int32_t b[18]) {
    return a[12] == b[12] && a[13] < b[13] + b[15] && b[13] < a[13] + a[15]
        && a[14] < b[14] + b[16] && b[14] < a[14] + a[16];
}

static void prepare_last_shelf(int32_t first[18], int32_t second[18], int32_t third[18]) {
    CHECK(stasis_init_window(512, 512, "Atlas reservation regression") == 1);
    /* Standalone images share real 512px cold pages; includes the 1px gutters. */
    load(400, 394, first);
    load(100, 4, second);
    load(400, 98, third);
    CHECK(first[12] == second[12] && second[12] == third[12]);
    CHECK(first[13] == 1 && first[14] == 6);
    CHECK(second[13] == 403 && second[14] == 6);
    CHECK(third[13] == 1 && third[14] == 402);
    CHECK(!overlaps(first, second) && !overlaps(first, third) && !overlaps(second, third));
}

static void failed_wrap_preserves_live_shelf(void) {
    int32_t first[18], second[18], third[18], rejected_page[18], smaller[18];
    prepare_last_shelf(first, second, third);
    /* The proposed shelf begins at 502. 9+2px is one pixel beyond page bottom. */
    load(120, 9, rejected_page);
    CHECK(rejected_page[12] != third[12]);
    load(120, 4, smaller);
    CHECK(smaller[12] == third[12]);
    CHECK(!overlaps(third, smaller));
    CHECK(smaller[13] == 1 && smaller[14] == 502);
    CHECK(!overlaps(first, smaller) && !overlaps(second, smaller));
    stasis_shutdown();
}

static void reservation_boundaries(void) {
    for (int delta = -1; delta <= 1; delta++) {
        int32_t first[18], second[18], third[18], candidate[18];
        prepare_last_shelf(first, second, third);
        /* Proposed bottom is 511, 512, or 513. Equality remains inside. */
        load(120, 8 + delta, candidate);
        CHECK((candidate[12] == third[12]) == (delta <= 0));
        if (delta <= 0) CHECK(candidate[13] == 1 && candidate[14] == 502);
        CHECK(!overlaps(first, candidate) && !overlaps(second, candidate) && !overlaps(third, candidate));
        stasis_shutdown();
    }
    for (int delta = -1; delta <= 1; delta++) {
        int32_t first[18], second[18], third[18], candidate[18];
        prepare_last_shelf(first, second, third);
        /* Proposed right edge is 511, 512, or 513. Only the last wraps. */
        load(107 + delta, 4, candidate);
        CHECK(candidate[12] == third[12]);
        CHECK(candidate[13] == (delta <= 0 ? 403 : 1));
        CHECK(candidate[14] == (delta <= 0 ? 402 : 502));
        CHECK(!overlaps(first, candidate) && !overlaps(second, candidate) && !overlaps(third, candidate));
        stasis_shutdown();
    }
}

int main(void) {
#if defined(_WIN32)
    _putenv_s("STASIS_ENABLE_TEST_INPUT", "1");
#else
    setenv("STASIS_ENABLE_TEST_INPUT", "1", 1);
#endif
    failed_wrap_preserves_live_shelf();
    reservation_boundaries();
    puts("sprite atlas failed-reservation and boundary contracts passed");
    return 0;
}
