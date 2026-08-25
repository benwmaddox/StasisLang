#include "stasis_audio_ring.h"

#include <stddef.h>
#include <string.h>

#define STASIS_AUDIO_RING_ACCEPTING (UINT64_C(1) << 63)
#define STASIS_AUDIO_RING_WRITE_MASK (STASIS_AUDIO_RING_ACCEPTING - 1)

static uint32_t minimum_u32(uint32_t left, uint32_t right) {
    return left < right ? left : right;
}

int stasis_audio_ring_initialize(
    StasisAudioRing *ring,
    float *storage,
    uint32_t capacity_frames,
    int32_t channels
) {
    if (ring == NULL || storage == NULL || capacity_frames == 0 ||
        channels <= 0 || channels > 2) {
        return 0;
    }
    *ring = (StasisAudioRing){0};
    ring->samples = storage;
    ring->capacity_frames = capacity_frames;
    ring->channels = channels;
    memset(storage, 0, (size_t)capacity_frames * (size_t)channels * sizeof(float));
    return 1;
}

void stasis_audio_ring_reset(StasisAudioRing *ring) {
    if (ring == NULL) return;
    atomic_store_explicit(&ring->read_frame, 0, memory_order_release);
    atomic_store_explicit(&ring->write_state, 0, memory_order_release);
    atomic_store_explicit(&ring->underruns, 0, memory_order_release);
}

void stasis_audio_ring_set_accepting(StasisAudioRing *ring, int accepting) {
    if (ring == NULL) return;
    if (accepting) {
        atomic_fetch_or_explicit(&ring->write_state,
            STASIS_AUDIO_RING_ACCEPTING, memory_order_release);
    } else {
        uint64_t state = atomic_fetch_and_explicit(&ring->write_state,
            STASIS_AUDIO_RING_WRITE_MASK, memory_order_acq_rel);
        uint64_t write = state & STASIS_AUDIO_RING_WRITE_MASK;
        atomic_store_explicit(&ring->read_frame, write, memory_order_release);
    }
}

uint32_t stasis_audio_ring_queued_frames(const StasisAudioRing *ring) {
    uint64_t read;
    uint64_t write;
    uint64_t queued;
    if (ring == NULL || ring->capacity_frames == 0) return 0;
    read = atomic_load_explicit(&ring->read_frame, memory_order_acquire);
    write = atomic_load_explicit(&ring->write_state, memory_order_acquire) &
        STASIS_AUDIO_RING_WRITE_MASK;
    queued = write >= read ? write - read : 0;
    return (uint32_t)(queued > ring->capacity_frames ? ring->capacity_frames : queued);
}

uint32_t stasis_audio_ring_underruns(const StasisAudioRing *ring) {
    return ring == NULL ? 0 :
        atomic_load_explicit(&ring->underruns, memory_order_acquire);
}

uint32_t stasis_audio_ring_push(
    StasisAudioRing *ring,
    const float *interleaved,
    uint32_t frame_count
) {
    uint64_t read;
    uint64_t state;
    uint64_t write;
    uint32_t free_frames;
    uint32_t accepted;
    uint32_t first;
    size_t channels;
    if (ring == NULL || interleaved == NULL || frame_count == 0 ||
        ring->capacity_frames == 0) {
        return 0;
    }
    state = atomic_load_explicit(&ring->write_state, memory_order_acquire);
    if ((state & STASIS_AUDIO_RING_ACCEPTING) == 0) return 0;
    read = atomic_load_explicit(&ring->read_frame, memory_order_acquire);
    write = state & STASIS_AUDIO_RING_WRITE_MASK;
    free_frames = ring->capacity_frames - minimum_u32(
        (uint32_t)(write >= read ? write - read : 0), ring->capacity_frames);
    accepted = minimum_u32(frame_count, free_frames);
    if (accepted == 0) return 0;
    channels = (size_t)ring->channels;
    first = minimum_u32(accepted, ring->capacity_frames -
        (uint32_t)(write % ring->capacity_frames));
    memcpy(ring->samples + (write % ring->capacity_frames) * channels,
        interleaved, (size_t)first * channels * sizeof(float));
    if (accepted > first) {
        memcpy(ring->samples, interleaved + (size_t)first * channels,
            (size_t)(accepted - first) * channels * sizeof(float));
    }
    if (!atomic_compare_exchange_strong_explicit(&ring->write_state, &state,
            STASIS_AUDIO_RING_ACCEPTING | (write + accepted),
            memory_order_release, memory_order_relaxed)) {
        return 0;
    }
    return accepted;
}

uint32_t stasis_audio_ring_consume(
    StasisAudioRing *ring,
    float *interleaved,
    uint32_t frame_count
) {
    uint64_t read;
    uint64_t write;
    uint32_t available;
    uint32_t consumed;
    uint32_t first;
    size_t channels;
    if (ring == NULL || interleaved == NULL || frame_count == 0) return 0;
    channels = (size_t)ring->channels;
    read = atomic_load_explicit(&ring->read_frame, memory_order_relaxed);
    write = atomic_load_explicit(&ring->write_state, memory_order_acquire) &
        STASIS_AUDIO_RING_WRITE_MASK;
    available = minimum_u32(
        (uint32_t)(write >= read ? write - read : 0), ring->capacity_frames);
    consumed = minimum_u32(frame_count, available);
    first = minimum_u32(consumed, ring->capacity_frames -
        (uint32_t)(read % ring->capacity_frames));
    if (first > 0) {
        memcpy(interleaved,
            ring->samples + (read % ring->capacity_frames) * channels,
            (size_t)first * channels * sizeof(float));
    }
    if (consumed > first) {
        memcpy(interleaved + (size_t)first * channels, ring->samples,
            (size_t)(consumed - first) * channels * sizeof(float));
    }
    if (!atomic_compare_exchange_strong_explicit(&ring->read_frame, &read,
            read + consumed, memory_order_release, memory_order_relaxed)) {
        memset(interleaved, 0, (size_t)frame_count * channels * sizeof(float));
        return 0;
    }
    if (consumed < frame_count) {
        memset(interleaved + (size_t)consumed * channels, 0,
            (size_t)(frame_count - consumed) * channels * sizeof(float));
        if ((atomic_load_explicit(&ring->write_state, memory_order_acquire) &
                STASIS_AUDIO_RING_ACCEPTING) != 0) {
            atomic_fetch_add_explicit(&ring->underruns, 1, memory_order_relaxed);
        }
    }
    return consumed;
}
