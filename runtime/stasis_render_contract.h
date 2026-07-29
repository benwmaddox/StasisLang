#ifndef STASIS_RENDER_CONTRACT_H
#define STASIS_RENDER_CONTRACT_H

#include <stddef.h>
#include <stdint.h>
#include <string.h>

/* Canonical guest-to-renderer command ABI used by JIT and AOT runtimes. */
#define STASIS_RENDER_V2_MAGIC 0x47584631
#define STASIS_RENDER_V2_VERSION 2
#define STASIS_RENDER_V2_TRACE_VERSION 2
#define STASIS_GRAPHICS_RUNTIME_ABI_VERSION 1

#define STASIS_RENDER_FLAG_CLEAR 1
#define STASIS_RENDER_FLAG_PRESENT 2

#define STASIS_RENDER_I_MAGIC 0
#define STASIS_RENDER_I_VERSION 1
#define STASIS_RENDER_I_FLAGS 2
#define STASIS_RENDER_I_LINE_COUNT 3
#define STASIS_RENDER_I_SPRITE_COUNT 4
#define STASIS_RENDER_I_TEXT_COUNT 7
#define STASIS_RENDER_I_TEXT_BYTES_USED 9
#define STASIS_RENDER_I_LOGICAL_W 10
#define STASIS_RENDER_I_LOGICAL_H 11
#define STASIS_RENDER_I_NATIVE_W 12
#define STASIS_RENDER_I_NATIVE_H 13
#define STASIS_RENDER_I_DRAWABLE_W 14
#define STASIS_RENDER_I_DRAWABLE_H 15
#define STASIS_RENDER_I_SAFE_X 16
#define STASIS_RENDER_I_SAFE_Y 17
#define STASIS_RENDER_I_SAFE_W 18
#define STASIS_RENDER_I_SAFE_H 19
#define STASIS_RENDER_I_DISPLAY_GENERATION 20
#define STASIS_RENDER_I_DENSITY_GENERATION 21
#define STASIS_RENDER_I_SPRITE_BASE 32

#define STASIS_RENDER_F_CLEAR_BASE 0
#define STASIS_RENDER_F_LINE_BASE 4

#define STASIS_RENDER_MAX_LINES 10000
#define STASIS_RENDER_LINE_F32_STRIDE 8
#define STASIS_RENDER_MAX_SPRITES 4096
#define STASIS_RENDER_SPRITE_I32_STRIDE 3
#define STASIS_RENDER_SPRITE_F32_STRIDE 4
#define STASIS_RENDER_MAX_TEXT 2048
#define STASIS_RENDER_TEXT_I32_STRIDE 3
#define STASIS_RENDER_TEXT_F32_STRIDE 6
#define STASIS_RENDER_TEXT_MAX_BYTES 65536

#define STASIS_RENDER_I_TEXT_BASE \
    (STASIS_RENDER_I_SPRITE_BASE + \
     STASIS_RENDER_MAX_SPRITES * STASIS_RENDER_SPRITE_I32_STRIDE)
#define STASIS_RENDER_F_TEXT_BASE \
    (STASIS_RENDER_F_LINE_BASE + \
     STASIS_RENDER_MAX_LINES * STASIS_RENDER_LINE_F32_STRIDE + \
     STASIS_RENDER_MAX_SPRITES * STASIS_RENDER_SPRITE_F32_STRIDE)
#define STASIS_RENDER_F_SPRITE_BASE \
    (STASIS_RENDER_F_LINE_BASE + \
     STASIS_RENDER_MAX_LINES * STASIS_RENDER_LINE_F32_STRIDE)
#define STASIS_RENDER_I32_COUNT \
    (STASIS_RENDER_I_TEXT_BASE + \
     STASIS_RENDER_MAX_TEXT * STASIS_RENDER_TEXT_I32_STRIDE)
#define STASIS_RENDER_F32_COUNT \
    (STASIS_RENDER_F_TEXT_BASE + \
     STASIS_RENDER_MAX_TEXT * STASIS_RENDER_TEXT_F32_STRIDE)
#define STASIS_RENDER_U8_COUNT STASIS_RENDER_TEXT_MAX_BYTES

typedef enum StasisRenderV2Validation {
    STASIS_RENDER_V2_VALID = 0,
    STASIS_RENDER_V2_NULL_I32 = 1,
    STASIS_RENDER_V2_NULL_F32 = 2,
    STASIS_RENDER_V2_BAD_MAGIC = 3,
    STASIS_RENDER_V2_BAD_VERSION = 4
} StasisRenderV2Validation;

static inline StasisRenderV2Validation stasis_render_v2_validate(
    const int32_t *cmd_i32,
    const float *cmd_f32
) {
    if (cmd_i32 == NULL) return STASIS_RENDER_V2_NULL_I32;
    if (cmd_f32 == NULL) return STASIS_RENDER_V2_NULL_F32;
    if (cmd_i32[STASIS_RENDER_I_MAGIC] != STASIS_RENDER_V2_MAGIC) {
        return STASIS_RENDER_V2_BAD_MAGIC;
    }
    if (cmd_i32[STASIS_RENDER_I_VERSION] != STASIS_RENDER_V2_VERSION) {
        return STASIS_RENDER_V2_BAD_VERSION;
    }
    return STASIS_RENDER_V2_VALID;
}

static inline const char *stasis_render_v2_validation_name(
    StasisRenderV2Validation validation
) {
    switch (validation) {
        case STASIS_RENDER_V2_VALID: return "ok";
        case STASIS_RENDER_V2_NULL_I32: return "missing_i32_buffer";
        case STASIS_RENDER_V2_NULL_F32: return "missing_f32_buffer";
        case STASIS_RENDER_V2_BAD_MAGIC: return "invalid_magic";
        case STASIS_RENDER_V2_BAD_VERSION: return "unsupported_version";
        default: return "unknown_validation_failure";
    }
}

static inline int32_t stasis_render_clamp_count(int32_t value, int32_t maximum) {
    if (value < 0) return 0;
    return value > maximum ? maximum : value;
}

static inline int stasis_render_v2_is_valid(const int32_t *cmd_i32) {
    return cmd_i32 != NULL &&
        cmd_i32[STASIS_RENDER_I_MAGIC] == STASIS_RENDER_V2_MAGIC &&
        cmd_i32[STASIS_RENDER_I_VERSION] == STASIS_RENDER_V2_VERSION;
}

static inline int stasis_render_v2_text_span_is_valid(
    int32_t byte_offset,
    int32_t byte_length,
    int32_t text_bytes_used
) {
    return byte_offset >= 0 && byte_length >= 0 &&
        byte_offset < text_bytes_used &&
        byte_length < text_bytes_used - byte_offset;
}

static inline uint32_t stasis_render_trace_mix_u32(uint32_t hash, uint32_t value) {
    hash ^= (value >> 0) & 0xffu; hash *= 16777619u;
    hash ^= (value >> 8) & 0xffu; hash *= 16777619u;
    hash ^= (value >> 16) & 0xffu; hash *= 16777619u;
    hash ^= (value >> 24) & 0xffu; hash *= 16777619u;
    return hash;
}

static inline uint32_t stasis_render_trace_mix_f32(uint32_t hash, float value) {
    uint32_t bits = 0;
    memcpy(&bits, &value, sizeof(bits));
    return stasis_render_trace_mix_u32(hash, bits);
}

static inline uint32_t stasis_render_trace_mix_bytes(
    uint32_t hash,
    const uint8_t *bytes,
    int32_t length
) {
    if (bytes == NULL || length <= 0) return hash;
    for (int32_t index = 0; index < length; index++) {
        hash ^= bytes[index];
        hash *= 16777619u;
    }
    return hash;
}

/*
 * Returns a backend-independent trace of the interpreted v2 frame. The trace
 * includes command kind markers, normalized counts, every consumed value, and
 * the fixed clear -> line -> sprite -> cached/text -> present order.
 */
static inline uint32_t stasis_render_v2_trace(
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8
) {
    if (!stasis_render_v2_is_valid(cmd_i32) || cmd_f32 == NULL) return 0;

    const int32_t flags = cmd_i32[STASIS_RENDER_I_FLAGS];
    const int32_t line_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_LINE_COUNT], STASIS_RENDER_MAX_LINES);
    const int32_t sprite_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_SPRITE_COUNT], STASIS_RENDER_MAX_SPRITES);
    const int32_t text_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_TEXT_COUNT], STASIS_RENDER_MAX_TEXT);
    const int32_t text_bytes_used = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_TEXT_BYTES_USED], STASIS_RENDER_TEXT_MAX_BYTES);

    uint32_t hash = 2166136261u;
    hash = stasis_render_trace_mix_u32(hash, STASIS_RENDER_V2_TRACE_VERSION);
    if ((flags & STASIS_RENDER_FLAG_CLEAR) != 0) {
        hash = stasis_render_trace_mix_u32(hash, 1u);
        for (int index = 0; index < 4; index++) {
            hash = stasis_render_trace_mix_f32(
                hash, cmd_f32[STASIS_RENDER_F_CLEAR_BASE + index]);
        }
    }

    for (int32_t index = 0; index < line_count; index++) {
        hash = stasis_render_trace_mix_u32(hash, 2u);
        const int32_t base = STASIS_RENDER_F_LINE_BASE +
            index * STASIS_RENDER_LINE_F32_STRIDE;
        for (int field = 0; field < STASIS_RENDER_LINE_F32_STRIDE; field++) {
            hash = stasis_render_trace_mix_f32(hash, cmd_f32[base + field]);
        }
    }

    for (int32_t index = 0; index < sprite_count; index++) {
        hash = stasis_render_trace_mix_u32(hash, 3u);
        const int32_t base_i = STASIS_RENDER_I_SPRITE_BASE +
            index * STASIS_RENDER_SPRITE_I32_STRIDE;
        const int32_t base_f = STASIS_RENDER_F_SPRITE_BASE +
            index * STASIS_RENDER_SPRITE_F32_STRIDE;
        for (int field = 0; field < STASIS_RENDER_SPRITE_I32_STRIDE; field++) {
            hash = stasis_render_trace_mix_u32(hash, (uint32_t)cmd_i32[base_i + field]);
        }
        for (int field = 0; field < STASIS_RENDER_SPRITE_F32_STRIDE; field++) {
            hash = stasis_render_trace_mix_f32(hash, cmd_f32[base_f + field]);
        }
    }

    for (int32_t index = 0; index < text_count; index++) {
        const int32_t meta_base = STASIS_RENDER_I_TEXT_BASE +
            index * STASIS_RENDER_TEXT_I32_STRIDE;
        const int32_t float_base = STASIS_RENDER_F_TEXT_BASE +
            index * STASIS_RENDER_TEXT_F32_STRIDE;
        const int32_t byte_offset = cmd_i32[meta_base + 1];
        const int32_t byte_length = cmd_i32[meta_base + 2];
        hash = stasis_render_trace_mix_u32(hash, byte_offset < 0 ? 4u : 5u);
        for (int field = 0; field < STASIS_RENDER_TEXT_I32_STRIDE; field++) {
            hash = stasis_render_trace_mix_u32(hash, (uint32_t)cmd_i32[meta_base + field]);
        }
        for (int field = 0; field < STASIS_RENDER_TEXT_F32_STRIDE; field++) {
            hash = stasis_render_trace_mix_f32(hash, cmd_f32[float_base + field]);
        }
        if (byte_length > 0 && stasis_render_v2_text_span_is_valid(
                byte_offset, byte_length, text_bytes_used)) {
            hash = stasis_render_trace_mix_bytes(
                hash, cmd_u8 == NULL ? NULL : cmd_u8 + byte_offset, byte_length);
        }
    }

    if ((flags & STASIS_RENDER_FLAG_PRESENT) != 0) {
        hash = stasis_render_trace_mix_u32(hash, 6u);
    }
    return hash;
}

#endif
