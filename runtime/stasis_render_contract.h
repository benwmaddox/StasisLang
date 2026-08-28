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
#define STASIS_RENDER_V5_VERSION 5
#define STASIS_RENDER_V6_VERSION 6
#define STASIS_RENDER_CURRENT_VERSION STASIS_RENDER_V6_VERSION
#define STASIS_RENDER_V2_TRACE_VERSION 2
#define STASIS_RENDER_V3_TRACE_VERSION 3
#define STASIS_RENDER_V4_TRACE_VERSION 4
#define STASIS_RENDER_V5_TRACE_VERSION 5
#define STASIS_RENDER_V6_TRACE_VERSION 6
#define STASIS_GRAPHICS_RUNTIME_ABI_VERSION 3

#define STASIS_RENDER_FLAG_CLEAR 1
#define STASIS_RENDER_FLAG_PRESENT 2

#define STASIS_RENDER_I_MAGIC 0
#define STASIS_RENDER_I_VERSION 1
#define STASIS_RENDER_I_FLAGS 2
#define STASIS_RENDER_I_LINE_COUNT 3
#define STASIS_RENDER_I_SPRITE_COUNT 4
#define STASIS_RENDER_I_DROPPED_LINES 5
#define STASIS_RENDER_I_DROPPED_SPRITES 6
#define STASIS_RENDER_I_TEXT_COUNT 7
#define STASIS_RENDER_I_DROPPED_TEXT 8
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
/* Reserved header slot carrying the monotonically increasing Android frame token. */
#define STASIS_RENDER_I_FRAME_TOKEN 26
/* V6 ordered clipping descriptor counts. V2-V5 leave these slots unused. */
#define STASIS_RENDER_I_CLIP_COUNT 27
#define STASIS_RENDER_I_DROPPED_CLIPS 28
#define STASIS_RENDER_I_SPRITE_BASE 32

#define STASIS_RENDER_F_CLEAR_BASE 0
#define STASIS_RENDER_F_LINE_BASE 4

#define STASIS_RENDER_MAX_GEOMETRY 10000
#define STASIS_RENDER_GEOMETRY_F32_STRIDE 8
#define STASIS_RENDER_MAX_LINES STASIS_RENDER_MAX_GEOMETRY
#define STASIS_RENDER_LINE_F32_STRIDE STASIS_RENDER_GEOMETRY_F32_STRIDE
#define STASIS_RENDER_MAX_SPRITES 4096
#define STASIS_RENDER_SPRITE_I32_STRIDE 3
#define STASIS_RENDER_LEGACY_SPRITE_F32_STRIDE 4
#define STASIS_RENDER_SPRITE_F32_STRIDE 8
#define STASIS_RENDER_MAX_TEXT 2048
#define STASIS_RENDER_TEXT_I32_STRIDE 3
#define STASIS_RENDER_TEXT_F32_STRIDE 6
#define STASIS_RENDER_TEXT_MAX_BYTES 65536
#define STASIS_RENDER_MAX_CLIPS 256
#define STASIS_RENDER_CLIP_F32_STRIDE 4
#define STASIS_RENDER_V5_MAX_ORDER \
    (STASIS_RENDER_MAX_LINES + STASIS_RENDER_MAX_SPRITES + STASIS_RENDER_MAX_TEXT)
#define STASIS_RENDER_MAX_ORDER \
    (STASIS_RENDER_V5_MAX_ORDER + 2 * STASIS_RENDER_MAX_CLIPS)
#define STASIS_RENDER_ORDER_KIND_SCALE 16384
#define STASIS_RENDER_ORDER_LINE 1
#define STASIS_RENDER_ORDER_SPRITE 2
#define STASIS_RENDER_ORDER_TEXT 3
#define STASIS_RENDER_ORDER_RECT 4
#define STASIS_RENDER_ORDER_CLIP_PUSH 5
#define STASIS_RENDER_ORDER_CLIP_POP 6

#define STASIS_RENDER_I_TEXT_BASE \
    (STASIS_RENDER_I_SPRITE_BASE + \
     STASIS_RENDER_MAX_SPRITES * STASIS_RENDER_SPRITE_I32_STRIDE)
#define STASIS_RENDER_LEGACY_F_TEXT_BASE \
    (STASIS_RENDER_F_LINE_BASE + \
     STASIS_RENDER_MAX_LINES * STASIS_RENDER_LINE_F32_STRIDE + \
     STASIS_RENDER_MAX_SPRITES * STASIS_RENDER_LEGACY_SPRITE_F32_STRIDE)
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
#define STASIS_RENDER_F_CLIP_BASE \
    (STASIS_RENDER_F_TEXT_BASE + \
     STASIS_RENDER_MAX_TEXT * STASIS_RENDER_TEXT_F32_STRIDE)
#define STASIS_RENDER_V2_I32_COUNT STASIS_RENDER_I_ORDER_BASE
#define STASIS_RENDER_V5_I32_COUNT \
    (STASIS_RENDER_I_ORDER_BASE + STASIS_RENDER_V5_MAX_ORDER)
#define STASIS_RENDER_I32_COUNT \
    (STASIS_RENDER_I_ORDER_BASE + STASIS_RENDER_MAX_ORDER)
#define STASIS_RENDER_V5_F32_COUNT \
    (STASIS_RENDER_F_TEXT_BASE + \
     STASIS_RENDER_MAX_TEXT * STASIS_RENDER_TEXT_F32_STRIDE)
#define STASIS_RENDER_F32_COUNT \
    (STASIS_RENDER_F_CLIP_BASE + \
     STASIS_RENDER_MAX_CLIPS * STASIS_RENDER_CLIP_F32_STRIDE)
#define STASIS_RENDER_U8_COUNT STASIS_RENDER_TEXT_MAX_BYTES

#define STASIS_RENDER_BUFFER_DESCRIPTORS(X) \
    X(I32, "i32", STASIS_RENDER_I32_COUNT * sizeof(int32_t), _Alignof(int32_t)) \
    X(F32, "f32", STASIS_RENDER_F32_COUNT * sizeof(float), _Alignof(float)) \
    X(U8, "u8", STASIS_RENDER_U8_COUNT * sizeof(uint8_t), _Alignof(uint8_t))

static inline int32_t stasis_render_sprite_f32_stride(int32_t version) {
    return version >= STASIS_RENDER_V5_VERSION
        ? STASIS_RENDER_SPRITE_F32_STRIDE
        : STASIS_RENDER_LEGACY_SPRITE_F32_STRIDE;
}

static inline int32_t stasis_render_f_text_base(int32_t version) {
    return version >= STASIS_RENDER_V5_VERSION
        ? STASIS_RENDER_F_TEXT_BASE
        : STASIS_RENDER_LEGACY_F_TEXT_BASE;
}

typedef enum StasisRenderValidation {
    STASIS_RENDER_VALID = 0,
    STASIS_RENDER_NULL_I32 = 1,
    STASIS_RENDER_NULL_F32 = 2,
    STASIS_RENDER_BAD_MAGIC = 3,
    STASIS_RENDER_BAD_VERSION = 4,
    STASIS_RENDER_NEGATIVE_COUNT = 5,
    STASIS_RENDER_EXCESSIVE_COUNT = 6,
    STASIS_RENDER_BAD_TEXT_SPAN = 7,
    STASIS_RENDER_BAD_ORDER_REFERENCE = 8,
    STASIS_RENDER_BAD_CLIP_STACK = 9
} StasisRenderValidation;

static inline int stasis_render_text_span_is_valid(
    int32_t byte_offset,
    int32_t byte_length,
    int32_t text_bytes_used
) {
    return byte_offset >= 0 && byte_length >= 0 &&
        byte_offset < text_bytes_used &&
        byte_length < text_bytes_used - byte_offset;
}

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
        cmd_i32[STASIS_RENDER_I_VERSION] != STASIS_RENDER_V4_VERSION &&
        cmd_i32[STASIS_RENDER_I_VERSION] != STASIS_RENDER_V5_VERSION &&
        cmd_i32[STASIS_RENDER_I_VERSION] != STASIS_RENDER_V6_VERSION) {
        return STASIS_RENDER_BAD_VERSION;
    }
    const int32_t version = cmd_i32[STASIS_RENDER_I_VERSION];
    const int32_t line_count = cmd_i32[STASIS_RENDER_I_LINE_COUNT];
    const int32_t sprite_count = cmd_i32[STASIS_RENDER_I_SPRITE_COUNT];
    const int32_t text_count = cmd_i32[STASIS_RENDER_I_TEXT_COUNT];
    const int32_t text_bytes_used = cmd_i32[STASIS_RENDER_I_TEXT_BYTES_USED];
    const int32_t rect_count = version >= STASIS_RENDER_V4_VERSION
        ? cmd_i32[STASIS_RENDER_I_RECT_COUNT] : 0;
    const int32_t order_count = version >= STASIS_RENDER_V3_VERSION
        ? cmd_i32[STASIS_RENDER_I_ORDER_COUNT] : 0;
    const int32_t clip_count = version >= STASIS_RENDER_V6_VERSION
        ? cmd_i32[STASIS_RENDER_I_CLIP_COUNT] : 0;
    if (line_count < 0 || sprite_count < 0 || text_count < 0 ||
        text_bytes_used < 0 || rect_count < 0 || order_count < 0 ||
        clip_count < 0) {
        return STASIS_RENDER_NEGATIVE_COUNT;
    }
    const int32_t max_order = version >= STASIS_RENDER_V6_VERSION
        ? STASIS_RENDER_MAX_ORDER : STASIS_RENDER_V5_MAX_ORDER;
    if (line_count > STASIS_RENDER_MAX_LINES ||
        rect_count > STASIS_RENDER_MAX_GEOMETRY - line_count ||
        sprite_count > STASIS_RENDER_MAX_SPRITES ||
        text_count > STASIS_RENDER_MAX_TEXT ||
        text_bytes_used > STASIS_RENDER_TEXT_MAX_BYTES ||
        clip_count > STASIS_RENDER_MAX_CLIPS ||
        order_count > max_order) {
        return STASIS_RENDER_EXCESSIVE_COUNT;
    }
    for (int32_t index = 0; index < text_count; index++) {
        const int32_t base = STASIS_RENDER_I_TEXT_BASE +
            index * STASIS_RENDER_TEXT_I32_STRIDE;
        const int32_t offset = cmd_i32[base + 1];
        const int32_t length = cmd_i32[base + 2];
        if (offset < 0) {
            if (offset == INT32_MIN || length != 0) {
                return STASIS_RENDER_BAD_TEXT_SPAN;
            }
        } else if (!stasis_render_text_span_is_valid(
                offset, length, text_bytes_used)) {
            return STASIS_RENDER_BAD_TEXT_SPAN;
        }
    }
    int32_t clip_depth = 0;
    for (int32_t order = 0; order < order_count; order++) {
        const int32_t entry = cmd_i32[STASIS_RENDER_I_ORDER_BASE + order];
        if (entry < 0) return STASIS_RENDER_BAD_ORDER_REFERENCE;
        const int32_t kind = entry / STASIS_RENDER_ORDER_KIND_SCALE;
        const int32_t index = entry % STASIS_RENDER_ORDER_KIND_SCALE;
        int valid =
            (kind == STASIS_RENDER_ORDER_LINE && index < line_count) ||
            (kind == STASIS_RENDER_ORDER_RECT && index < rect_count) ||
            (kind == STASIS_RENDER_ORDER_SPRITE && index < sprite_count) ||
            (kind == STASIS_RENDER_ORDER_TEXT && index < text_count);
        if (version >= STASIS_RENDER_V6_VERSION &&
            kind == STASIS_RENDER_ORDER_CLIP_PUSH && index < clip_count) {
            clip_depth++;
            valid = clip_depth <= STASIS_RENDER_MAX_CLIPS;
        } else if (version >= STASIS_RENDER_V6_VERSION &&
                   kind == STASIS_RENDER_ORDER_CLIP_POP && index == 0) {
            if (clip_depth <= 0) return STASIS_RENDER_BAD_CLIP_STACK;
            clip_depth--;
            valid = 1;
        }
        if (!valid) return STASIS_RENDER_BAD_ORDER_REFERENCE;
    }
    if (version >= STASIS_RENDER_V6_VERSION && clip_depth != 0) {
        return STASIS_RENDER_BAD_CLIP_STACK;
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
        case STASIS_RENDER_NEGATIVE_COUNT: return "negative_count";
        case STASIS_RENDER_EXCESSIVE_COUNT: return "excessive_count";
        case STASIS_RENDER_BAD_TEXT_SPAN: return "invalid_text_span";
        case STASIS_RENDER_BAD_ORDER_REFERENCE: return "invalid_order_reference";
        case STASIS_RENDER_BAD_CLIP_STACK: return "invalid_clip_stack";
        default: return "unknown_validation_failure";
    }
}

static inline const char *stasis_render_validation_stage(
    StasisRenderValidation validation
) {
    switch (validation) {
        case STASIS_RENDER_BAD_MAGIC:
        case STASIS_RENDER_BAD_VERSION:
        case STASIS_RENDER_NULL_I32:
        case STASIS_RENDER_NULL_F32:
            return "command_header";
        case STASIS_RENDER_NEGATIVE_COUNT:
        case STASIS_RENDER_EXCESSIVE_COUNT:
            return "command_counts";
        case STASIS_RENDER_BAD_TEXT_SPAN:
            return "text_span";
        case STASIS_RENDER_BAD_ORDER_REFERENCE:
            return "order_reference";
        case STASIS_RENDER_BAD_CLIP_STACK:
            return "clip_stack";
        default:
            return "none";
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
         cmd_i32[STASIS_RENDER_I_VERSION] == STASIS_RENDER_V4_VERSION ||
         cmd_i32[STASIS_RENDER_I_VERSION] == STASIS_RENDER_V5_VERSION ||
         cmd_i32[STASIS_RENDER_I_VERSION] == STASIS_RENDER_V6_VERSION);
}

static inline int32_t stasis_render_rect_count(
    const int32_t *cmd_i32,
    int32_t line_count
) {
    if (cmd_i32[STASIS_RENDER_I_VERSION] < STASIS_RENDER_V4_VERSION) return 0;
    return stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_RECT_COUNT],
        STASIS_RENDER_MAX_GEOMETRY - line_count);
}

static inline int32_t stasis_render_clip_count(const int32_t *cmd_i32) {
    if (cmd_i32 == NULL || cmd_i32[STASIS_RENDER_I_VERSION] < STASIS_RENDER_V6_VERSION) {
        return 0;
    }
    return stasis_render_clamp_count(
        cmd_i32[STASIS_RENDER_I_CLIP_COUNT], STASIS_RENDER_MAX_CLIPS);
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
    int32_t version,
    const int32_t *cmd_i32,
    const float *cmd_f32,
    int32_t index
) {
    hash = stasis_render_trace_mix_u32(hash, 3u);
    const int32_t base_i = STASIS_RENDER_I_SPRITE_BASE +
        index * STASIS_RENDER_SPRITE_I32_STRIDE;
    const int32_t sprite_stride = stasis_render_sprite_f32_stride(version);
    const int32_t base_f = STASIS_RENDER_F_SPRITE_BASE +
        index * sprite_stride;
    for (int field = 0; field < STASIS_RENDER_SPRITE_I32_STRIDE; field++) {
        hash = stasis_render_trace_mix_u32(hash, (uint32_t)cmd_i32[base_i + field]);
    }
    for (int field = 0; field < sprite_stride; field++) {
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
    int32_t version,
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8,
    int32_t text_bytes_used,
    int32_t index
) {
    const int32_t meta_base = STASIS_RENDER_I_TEXT_BASE +
        index * STASIS_RENDER_TEXT_I32_STRIDE;
    const int32_t float_base = stasis_render_f_text_base(version) +
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

static inline uint32_t stasis_render_trace_clip_push(
    uint32_t hash,
    const float *cmd_f32,
    int32_t index
) {
    hash = stasis_render_trace_mix_u32(hash, 8u);
    const int32_t base = STASIS_RENDER_F_CLIP_BASE +
        index * STASIS_RENDER_CLIP_F32_STRIDE;
    for (int field = 0; field < STASIS_RENDER_CLIP_F32_STRIDE; field++) {
        hash = stasis_render_trace_mix_f32(hash, cmd_f32[base + field]);
    }
    return hash;
}

static inline uint32_t stasis_render_trace_clip_pop(uint32_t hash) {
    return stasis_render_trace_mix_u32(hash, 9u);
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
    if (stasis_render_validate(cmd_i32, cmd_f32) != STASIS_RENDER_VALID) return 0;

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
        version == STASIS_RENDER_V6_VERSION
            ? STASIS_RENDER_V6_TRACE_VERSION
            : (version == STASIS_RENDER_V5_VERSION
            ? STASIS_RENDER_V5_TRACE_VERSION
            : (version == STASIS_RENDER_V4_VERSION
                ? STASIS_RENDER_V4_TRACE_VERSION
                : (version == STASIS_RENDER_V3_VERSION
                ? STASIS_RENDER_V3_TRACE_VERSION
                : STASIS_RENDER_V2_TRACE_VERSION))));
    if ((flags & STASIS_RENDER_FLAG_CLEAR) != 0) {
        hash = stasis_render_trace_mix_u32(hash, 1u);
        for (int index = 0; index < 4; index++) {
            hash = stasis_render_trace_mix_f32(
                hash, cmd_f32[STASIS_RENDER_F_CLEAR_BASE + index]);
        }
    }

    const int32_t order_count = version >= STASIS_RENDER_V3_VERSION
        ? stasis_render_clamp_count(
            cmd_i32[STASIS_RENDER_I_ORDER_COUNT],
            version >= STASIS_RENDER_V6_VERSION
                ? STASIS_RENDER_MAX_ORDER : STASIS_RENDER_V5_MAX_ORDER)
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
                hash = stasis_render_trace_sprite(hash, version, cmd_i32, cmd_f32, index);
            } else if (kind == STASIS_RENDER_ORDER_TEXT && index < text_count) {
                hash = stasis_render_trace_text(
                    hash, version, cmd_i32, cmd_f32, cmd_u8, text_bytes_used, index);
            } else if (kind == STASIS_RENDER_ORDER_RECT && index < rect_count) {
                hash = stasis_render_trace_rect(hash, cmd_f32, index);
            } else if (kind == STASIS_RENDER_ORDER_CLIP_PUSH &&
                       index < stasis_render_clip_count(cmd_i32)) {
                hash = stasis_render_trace_clip_push(hash, cmd_f32, index);
            } else if (kind == STASIS_RENDER_ORDER_CLIP_POP && index == 0) {
                hash = stasis_render_trace_clip_pop(hash);
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
            hash = stasis_render_trace_sprite(hash, version, cmd_i32, cmd_f32, index);
        }
        for (int32_t index = 0; index < text_count; index++) {
            hash = stasis_render_trace_text(
                hash, version, cmd_i32, cmd_f32, cmd_u8, text_bytes_used, index);
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
