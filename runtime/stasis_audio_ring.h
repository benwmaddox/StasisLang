#ifndef STASIS_AUDIO_RING_H
#define STASIS_AUDIO_RING_H

#include <stdatomic.h>
#include <stdint.h>

typedef struct StasisAudioRing {
    float *samples;
    uint32_t capacity_frames;
    int32_t channels;
    _Atomic uint64_t read_frame;
    _Atomic uint64_t write_state;
    _Atomic uint32_t underruns;
} StasisAudioRing;

int stasis_audio_ring_initialize(
    StasisAudioRing *ring,
    float *storage,
    uint32_t capacity_frames,
    int32_t channels
);
void stasis_audio_ring_reset(StasisAudioRing *ring);
void stasis_audio_ring_set_accepting(StasisAudioRing *ring, int accepting);
uint32_t stasis_audio_ring_queued_frames(const StasisAudioRing *ring);
uint32_t stasis_audio_ring_underruns(const StasisAudioRing *ring);
uint32_t stasis_audio_ring_push(
    StasisAudioRing *ring,
    const float *interleaved,
    uint32_t frame_count
);
uint32_t stasis_audio_ring_consume(
    StasisAudioRing *ring,
    float *interleaved,
    uint32_t frame_count
);

#endif
