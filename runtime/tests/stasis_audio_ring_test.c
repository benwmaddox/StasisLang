#include "stasis_audio_ring.h"

#include <stdio.h>
#include <stdlib.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static void test_bounded_fifo_and_wrap(void) {
    StasisAudioRing ring;
    float storage[8] = {0};
    float first[6] = {1, 2, 3, 4, 5, 6};
    float second[6] = {7, 8, 9, 10, 11, 12};
    float out[8] = {0};
    CHECK(stasis_audio_ring_initialize(&ring, storage, 4, 2));
    stasis_audio_ring_set_accepting(&ring, 1);
    CHECK(stasis_audio_ring_push(&ring, first, 3) == 3);
    CHECK(stasis_audio_ring_consume(&ring, out, 2) == 2);
    CHECK(out[0] == 1 && out[3] == 4);
    CHECK(stasis_audio_ring_push(&ring, second, 3) == 3);
    CHECK(stasis_audio_ring_push(&ring, second, 1) == 0);
    CHECK(stasis_audio_ring_queued_frames(&ring) == 4);
    CHECK(stasis_audio_ring_consume(&ring, out, 4) == 4);
    CHECK(out[0] == 5 && out[1] == 6 && out[2] == 7 && out[7] == 12);
}

static void test_pause_discards_and_underrun_is_explicit(void) {
    StasisAudioRing ring;
    float storage[8] = {0};
    float input[4] = {1, 2, 3, 4};
    float out[8] = {1, 1, 1, 1, 1, 1, 1, 1};
    CHECK(stasis_audio_ring_initialize(&ring, storage, 4, 2));
    stasis_audio_ring_set_accepting(&ring, 1);
    CHECK(stasis_audio_ring_push(&ring, input, 2) == 2);
    stasis_audio_ring_set_accepting(&ring, 0);
    CHECK(stasis_audio_ring_queued_frames(&ring) == 0);
    CHECK(stasis_audio_ring_consume(&ring, out, 2) == 0);
    CHECK(out[0] == 0 && out[3] == 0);
    CHECK(stasis_audio_ring_underruns(&ring) == 0);
    stasis_audio_ring_set_accepting(&ring, 1);
    CHECK(stasis_audio_ring_consume(&ring, out, 2) == 0);
    CHECK(stasis_audio_ring_underruns(&ring) == 1);
    stasis_audio_ring_reset(&ring);
    CHECK(stasis_audio_ring_queued_frames(&ring) == 0);
    CHECK(stasis_audio_ring_underruns(&ring) == 0);
}

int main(void) {
    test_bounded_fifo_and_wrap();
    test_pause_discards_and_underrun_is_explicit();
    puts("stasis_audio_ring_test: ok");
    return 0;
}
