#ifndef STASIS_RENDER_CONTRACT_H
#define STASIS_RENDER_CONTRACT_H

#include <stddef.h>
#include <stdint.h>
#include <string.h>

/* Canonical guest-to-renderer command ABI used by JIT and AOT runtimes. */
#define STASIS_RENDER_V2_MAGIC 0x47584631
#define STASIS_RENDER_V2_VERSION 2
#define STASIS_RENDER_V3_VERSION 3
#define STASIS_RENDER_V4_VERSION 4
#define STASIS_RENDER_CURRENT_VERSION STASIS_RENDER_V4_VERSION
#define STASIS_RENDER_V2_TRACE_VERSION 2
#define STASIS_RENDER_V3_TRACE_VERSION 3
#define STASIS_RENDER_V4_TRACE_VERSION 4
#define STASIS_GRAPHICS_RUNTIME_ABI_VERSION 2

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
#define STASIS_RENDER_I_ORDER_COUNT 22
#define STASIS_RENDER_I_DROPPED_ORDER 23
#define STASIS_RENDER_I_RECT_COUNT 24
#define STASIS_RENDER_I_DROPPED_RECTS 25
#define STASIS_RENDER_I_SPRITE_BASE 32

#define STASIS_RENDER_F_CLEAR_BASE 0
#define STASIS_RENDER_F_LINE_BASE 4

#define STASIS_RENDER_MAX_GEOMETRY 10000
#define STASIS_RENDER_GEOMETRY_F32_STRIDE 8
#define STASIS_RENDER_MAX_LINES STASIS_RENDER_MAX_GEOMETRY
#define STASIS_RENDER_LINE_F32_STRIDE STASIS_RENDER_GEOMETRY_F32_STRIDE
#define STASIS_RENDER_MAX_SPRITES 4096
#define STASIS_RENDER_SPRITE_I32_STRIDE 3
#define STASIS_RENDER_SPRITE_F32_STRIDE 4
#define STASIS_RENDER_MAX_TEXT 2048
#define STASIS_RENDER_TEXT_I32_STRIDE 3
#define STASIS_RENDER_TEXT_F32_STRIDE 6
#define STASIS_RENDER_TEXT_MAX_BYTES 65536
#define STASIS_RENDER_MAX_ORDER \
    (STASIS_RENDER_MAX_LINES + STASIS_RENDER_MAX_SPRITES + STASIS_RENDER_MAX_TEXT)
#define STASIS_RENDER_ORDER_KIND_SCALE 16384
#define STASIS_RENDER_ORDER_LINE 1
#define STASIS_RENDER_ORDER_SPRITE 2
#define STASIS_RENDER_ORDER_TEXT 3
#define STASIS_RENDER_ORDER_RECT 4

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
#define STASIS_RENDER_F_RECT_REVERSE_BASE \
    (STASIS_RENDER_F_SPRITE_BASE - STASIS_RENDER_GEOMETRY_F32_STRIDE)
#define STASIS_RENDER_I_ORDER_BASE \
    (STASIS_RENDER_I_TEXT_BASE + \
     STASIS_RENDER_MAX_TEXT * STASIS_RENDER_TEXT_I32_STRIDE)
#define STASIS_RENDER_V2_I32_COUNT STASIS_RENDER_I_ORDER_BASE
#define STASIS_RENDER_I32_COUNT \
    (STASIS_RENDER_I_ORDER_BASE + STASIS_RENDER_MAX_ORDER)
#define STASIS_RENDER_F32_COUNT \
    (STASIS_RENDER_F_TEXT_BASE + \
     STASIS_RENDER_MAX_TEXT * STASIS_RENDER_TEXT_F32_STRIDE)
#define STASIS_RENDER_U8_COUNT STASIS_RENDER_TEXT_MAX_BYTES

typedef enum StasisRenderValidation {
    STASIS_RENDER_VALID = 0,
    STASIS_RENDER_NULL_I32 = 1,
    STASIS_RENDER_NULL_F32 = 2,
    STASIS_RENDER_BAD_MAGIC = 3,
    STASIS_RENDER_BAD_VERSION = 4
} StasisRenderValidation;

static inline StasisRenderValidation stasis_render_validate(
    const int32_t *cmd_i32,
    const float *cmd_f32
) {
    if (cmd_i32 == NULL) return STASIS_RENDER_NULL_I32;
    if (cmd_f32 == NULL) return STASIS_RENDER_NULL_F32;
    if (cmd_i32[STASIS_RENDER_I_MAGIC] != STASIS_RENDER_V2_MAGIC) {
        return STASIS_RENDER_BAD_MAGIC;
    }
    if (cmd_i32[STASIS_RENDER_I_VERSION] != STASIS_RENDER_V2_VERSION &&
        cmd_i32[STASIS_RENDER_I_VERSION] != STASIS_RENDER_V3_VERSION &&
        cmd_i32[STASIS_RENDER_I_VERSION] != STASIS_RENDER_V4_VERSION) {
        return STASIS_RENDER_BAD_VERSION;
    }
    return STASIS_RENDER_VALID;
}

static inline const char *stasis_render_validation_name(
    StasisRenderValidation validation
) {
    switch (validation) {
        case STASIS_RENDER_VALID: return "ok";
        case STASIS_RENDER_NULL_I32: return "missing_i32_buffer";
        case STASIS_RENDER_NULL_F32: return "missing_f32_buffer";
        case STASIS_RENDER_BAD_MAGIC: return "invalid_magic";
        case STASIS_RENDER_BAD_VERSION: return "unsupported_version";
        default: return "unknown_validation_failure";
    }
}

static inline int32_t stasis_render_clamp_count(int32_t value, int32_t maximum) {
    if (value < 0) return 0;
    return value > maximum ? maximum : value;
}

static inline int stasis_render_is_valid(const int32_t *cmd_i32) {
    return cmd_i32 != NULL &&
        cmd_i32[STASIS_RENDER_I_MAGIC] == STASIS_RENDER_V2_MAGIC &&
        (cmd_i32[STASIS_RENDER_I_VERSION] == STASIS_RENDER_V2_VERSION ||
         cmd_i32[STASIS_RENDER_I_VERSION] == STASIS_RENDER_V3_VERSION ||
         cmd_i32[STASIS_RENDER_I_VERSION] == STASIS_RENDER_V4_VERSION);
}

static inline int32_t stasis_render_rect_count(
    const int32_t *cmd_i32,
    int32_t line_count
) {
    if (cmd_i32[STASIS_RENDER_I_VERSION] != STASIS_RENDER_V4_VERSION) return 0;
    return stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_RECT_COUNT],
        STASIS_RENDER_MAX_GEOMETRY - line_count);
}

static inline int stasis_render_is_empty_submission(const int32_t *cmd_i32) {
    if (!stasis_render_is_valid(cmd_i32)) return 0;
    const int32_t line_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_LINE_COUNT], STASIS_RENDER_MAX_LINES);
    const int32_t version = cmd_i32[STASIS_RENDER_I_VERSION];
    const int32_t order_count = version >= STASIS_RENDER_V3_VERSION
        ? stasis_render_clamp_count(
            cmd_i32[STASIS_RENDER_I_ORDER_COUNT], STASIS_RENDER_MAX_ORDER)
        : 0;
    return cmd_i32[STASIS_RENDER_I_FLAGS] == 0 &&
        line_count == 0 &&
        stasis_render_rect_count(cmd_i32, line_count) == 0 &&
        stasis_render_clamp_count(
            cmd_i32[STASIS_RENDER_I_SPRITE_COUNT], STASIS_RENDER_MAX_SPRITES) == 0 &&
        stasis_render_clamp_count(
            cmd_i32[STASIS_RENDER_I_TEXT_COUNT], STASIS_RENDER_MAX_TEXT) == 0 &&
        order_count == 0;
}

static inline int stasis_render_text_span_is_valid(
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

static inline uint32_t stasis_render_trace_line(
    uint32_t hash,
    const float *cmd_f32,
    int32_t index
) {
    hash = stasis_render_trace_mix_u32(hash, 2u);
    const int32_t base = STASIS_RENDER_F_LINE_BASE +
        index * STASIS_RENDER_LINE_F32_STRIDE;
    for (int field = 0; field < STASIS_RENDER_LINE_F32_STRIDE; field++) {
        hash = stasis_render_trace_mix_f32(hash, cmd_f32[base + field]);
    }
    return hash;
}

static inline uint32_t stasis_render_trace_sprite(
    uint32_t hash,
    const int32_t *cmd_i32,
    const float *cmd_f32,
    int32_t index
) {
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
    return hash;
}

static inline uint32_t stasis_render_trace_rect(
    uint32_t hash,
    const float *cmd_f32,
    int32_t index
) {
    hash = stasis_render_trace_mix_u32(hash, 7u);
    const int32_t base = STASIS_RENDER_F_RECT_REVERSE_BASE -
        index * STASIS_RENDER_GEOMETRY_F32_STRIDE;
    for (int field = 0; field < STASIS_RENDER_GEOMETRY_F32_STRIDE; field++) {
        hash = stasis_render_trace_mix_f32(hash, cmd_f32[base + field]);
    }
    return hash;
}

static inline uint32_t stasis_render_trace_text(
    uint32_t hash,
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8,
    int32_t text_bytes_used,
    int32_t index
) {
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
    if (byte_length > 0 && stasis_render_text_span_is_valid(
            byte_offset, byte_length, text_bytes_used)) {
        hash = stasis_render_trace_mix_bytes(
            hash, cmd_u8 == NULL ? NULL : cmd_u8 + byte_offset, byte_length);
    }
    return hash;
}

/*
 * Returns a backend-independent trace of the interpreted frame. V3 and V4
 * consume the bounded cross-category order stream. V2, and ordered frames
 * with no order entries, retain their version-specific compatibility order.
 */
static inline uint32_t stasis_render_trace(
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8
) {
    if (!stasis_render_is_valid(cmd_i32) || cmd_f32 == NULL) return 0;

    const int32_t flags = cmd_i32[STASIS_RENDER_I_FLAGS];
    const int32_t line_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_LINE_COUNT], STASIS_RENDER_MAX_LINES);
    const int32_t rect_count = stasis_render_rect_count(cmd_i32, line_count);
    const int32_t sprite_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_SPRITE_COUNT], STASIS_RENDER_MAX_SPRITES);
    const int32_t text_count = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_TEXT_COUNT], STASIS_RENDER_MAX_TEXT);
    const int32_t text_bytes_used = stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_TEXT_BYTES_USED], STASIS_RENDER_TEXT_MAX_BYTES);

    uint32_t hash = 2166136261u;
    const int32_t version = cmd_i32[STASIS_RENDER_I_VERSION];
    hash = stasis_render_trace_mix_u32(
        hash,
        version == STASIS_RENDER_V4_VERSION
            ? STASIS_RENDER_V4_TRACE_VERSION
            : (version == STASIS_RENDER_V3_VERSION
                ? STASIS_RENDER_V3_TRACE_VERSION
                : STASIS_RENDER_V2_TRACE_VERSION));
    if ((flags & STASIS_RENDER_FLAG_CLEAR) != 0) {
        hash = stasis_render_trace_mix_u32(hash, 1u);
        for (int index = 0; index < 4; index++) {
            hash = stasis_render_trace_mix_f32(
                hash, cmd_f32[STASIS_RENDER_F_CLEAR_BASE + index]);
        }
    }

    const int32_t order_count = version >= STASIS_RENDER_V3_VERSION
        ? stasis_render_clamp_count(
            cmd_i32[STASIS_RENDER_I_ORDER_COUNT], STASIS_RENDER_MAX_ORDER)
        : 0;
    if (order_count > 0) {
        for (int32_t order_index = 0; order_index < order_count; order_index++) {
            const int32_t entry = cmd_i32[STASIS_RENDER_I_ORDER_BASE + order_index];
            if (entry < 0) continue;
            const int32_t kind = entry / STASIS_RENDER_ORDER_KIND_SCALE;
            const int32_t index = entry % STASIS_RENDER_ORDER_KIND_SCALE;
            if (kind == STASIS_RENDER_ORDER_LINE && index < line_count) {
                hash = stasis_render_trace_line(hash, cmd_f32, index);
            } else if (kind == STASIS_RENDER_ORDER_SPRITE && index < sprite_count) {
                hash = stasis_render_trace_sprite(hash, cmd_i32, cmd_f32, index);
            } else if (kind == STASIS_RENDER_ORDER_TEXT && index < text_count) {
                hash = stasis_render_trace_text(
                    hash, cmd_i32, cmd_f32, cmd_u8, text_bytes_used, index);
            } else if (kind == STASIS_RENDER_ORDER_RECT && index < rect_count) {
                hash = stasis_render_trace_rect(hash, cmd_f32, index);
            }
        }
    } else {
        for (int32_t index = 0; index < line_count; index++) {
            hash = stasis_render_trace_line(hash, cmd_f32, index);
        }
        for (int32_t index = 0; index < rect_count; index++) {
            hash = stasis_render_trace_rect(hash, cmd_f32, index);
        }
        for (int32_t index = 0; index < sprite_count; index++) {
            hash = stasis_render_trace_sprite(hash, cmd_i32, cmd_f32, index);
        }
        for (int32_t index = 0; index < text_count; index++) {
            hash = stasis_render_trace_text(
                hash, cmd_i32, cmd_f32, cmd_u8, text_bytes_used, index);
        }
    }

    if ((flags & STASIS_RENDER_FLAG_PRESENT) != 0) {
        hash = stasis_render_trace_mix_u32(hash, 6u);
    }
    return hash;
}

/* Compatibility name retained for existing JIT/AOT host imports. */
static inline uint32_t stasis_render_v2_trace(
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8
) {
    return stasis_render_trace(cmd_i32, cmd_f32, cmd_u8);
}

#endif
