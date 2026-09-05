#include "stasis_mobile_runtime.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#define check(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static void test_fills_the_remainder_of_a_high_refresh_frame(void) {
    StasisMobileFramePacer pacer;
    stasis_mobile_frame_pacer_reset(&pacer, 1000);

    check(stasis_mobile_frame_pacer_wait_ns(&pacer, 1000 + 8333333) == 8333334);
    check(pacer.next_deadline_ns == 1000 + 33333334);
    check(stasis_mobile_frame_pacer_wait_ns(&pacer, 1000 + 25000000) == 8333334);
}

static void test_vsync_at_sixty_hz_needs_no_extra_sleep(void) {
    StasisMobileFramePacer pacer;
    stasis_mobile_frame_pacer_reset(&pacer, 2000);

    check(stasis_mobile_frame_pacer_wait_ns(
        &pacer,
        2000 + STASIS_MOBILE_FRAME_INTERVAL_NS
    ) == 0);
    check(pacer.next_deadline_ns == 2000 + (2 * STASIS_MOBILE_FRAME_INTERVAL_NS));
}

static void test_small_overrun_preserves_the_next_absolute_deadline(void) {
    StasisMobileFramePacer pacer;
    stasis_mobile_frame_pacer_reset(&pacer, 3000);

    check(stasis_mobile_frame_pacer_wait_ns(
        &pacer,
        3000 + STASIS_MOBILE_FRAME_INTERVAL_NS + 1000000
    ) == 0);
    check(pacer.next_deadline_ns == 3000 + (2 * STASIS_MOBILE_FRAME_INTERVAL_NS));
}

static void test_long_pause_resets_without_a_catch_up_burst(void) {
    StasisMobileFramePacer pacer;
    stasis_mobile_frame_pacer_reset(&pacer, 4000);

    uint64_t resumed_ns = 4000 + 1000000000ULL;
    check(stasis_mobile_frame_pacer_wait_ns(&pacer, resumed_ns) == 0);
    check(pacer.next_deadline_ns == resumed_ns + STASIS_MOBILE_FRAME_INTERVAL_NS);
    check(stasis_mobile_frame_pacer_wait_ns(&pacer, resumed_ns + 1000000) ==
        STASIS_MOBILE_FRAME_INTERVAL_NS - 1000000);
}

int main(void) {
    test_fills_the_remainder_of_a_high_refresh_frame();
    test_vsync_at_sixty_hz_needs_no_extra_sleep();
    test_small_overrun_preserves_the_next_absolute_deadline();
    test_long_pause_resets_without_a_catch_up_burst();
    puts("stasis_mobile_frame_pacer_test: ok");
    return 0;
}
