#include "stasis_render_contract.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void build_representative_frame(
    int32_t *i32s,
    float *f32s,
    uint8_t *u8s
) {
    i32s[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_V2_MAGIC;
    i32s[STASIS_RENDER_I_VERSION] = STASIS_RENDER_V2_VERSION;
    i32s[STASIS_RENDER_I_FLAGS] =
        STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT;
    i32s[STASIS_RENDER_I_LINE_COUNT] = 1;
    i32s[STASIS_RENDER_I_SPRITE_COUNT] = 1;
    i32s[STASIS_RENDER_I_TEXT_COUNT] = 2;
    i32s[STASIS_RENDER_I_TEXT_BYTES_USED] = 5;
    i32s[STASIS_RENDER_I_LOGICAL_W] = 640;
    i32s[STASIS_RENDER_I_LOGICAL_H] = 360;
    i32s[STASIS_RENDER_I_DRAWABLE_W] = 1280;
    i32s[STASIS_RENDER_I_DRAWABLE_H] = 720;
    i32s[STASIS_RENDER_I_DISPLAY_GENERATION] = 3;
    i32s[STASIS_RENDER_I_DENSITY_GENERATION] = 5;

    f32s[0] = 0.1f; f32s[1] = 0.2f; f32s[2] = 0.3f; f32s[3] = 1.0f;
    const float line[] = {1.0f, 2.0f, 3.0f, 4.0f, 0.5f, 0.6f, 0.7f, 0.8f};
    memcpy(f32s + STASIS_RENDER_F_LINE_BASE, line, sizeof(line));

    const int32_t sprite_i32[] = {17, 45, 192};
    const float sprite_f32[] = {10.25f, 20.5f, 30.75f, 40.125f};
    memcpy(i32s + STASIS_RENDER_I_SPRITE_BASE, sprite_i32, sizeof(sprite_i32));
    memcpy(f32s + STASIS_RENDER_F_SPRITE_BASE, sprite_f32, sizeof(sprite_f32));

    int32_t *text = i32s + STASIS_RENDER_I_TEXT_BASE;
    text[0] = 3; text[1] = 0; text[2] = 4;
    text[3] = 3; text[4] = -9; text[5] = 0;
    memcpy(u8s, "test", 5);
    for (int index = 0; index < 12; index++) {
        f32s[STASIS_RENDER_F_TEXT_BASE + index] = (float)(index + 1) * 0.25f;
    }
}

int main(void) {
#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "render contract check failed at line %d: %s\n", \
            __LINE__, #condition); \
        return 1; \
    } \
} while (0)
    int32_t *first_i32 = calloc(STASIS_RENDER_I32_COUNT, sizeof(*first_i32));
    float *first_f32 = calloc(STASIS_RENDER_F32_COUNT, sizeof(*first_f32));
    uint8_t *first_u8 = calloc(STASIS_RENDER_U8_COUNT, sizeof(*first_u8));
    int32_t *second_i32 = calloc(STASIS_RENDER_I32_COUNT, sizeof(*second_i32));
    float *second_f32 = calloc(STASIS_RENDER_F32_COUNT, sizeof(*second_f32));
    uint8_t *second_u8 = calloc(STASIS_RENDER_U8_COUNT, sizeof(*second_u8));
    CHECK(first_i32 && first_f32 && first_u8);
    CHECK(second_i32 && second_f32 && second_u8);

    build_representative_frame(first_i32, first_f32, first_u8);
    build_representative_frame(second_i32, second_f32, second_u8);
    CHECK(stasis_render_v2_validate(first_i32, first_f32) == STASIS_RENDER_V2_VALID);
    CHECK(strcmp(stasis_render_v2_validation_name(STASIS_RENDER_V2_VALID), "ok") == 0);
    CHECK(stasis_render_v2_validate(NULL, first_f32) == STASIS_RENDER_V2_NULL_I32);
    CHECK(stasis_render_v2_validate(first_i32, NULL) == STASIS_RENDER_V2_NULL_F32);
    CHECK(stasis_render_v2_is_valid(first_i32));
    uint32_t first_trace = stasis_render_v2_trace(first_i32, first_f32, first_u8);
    uint32_t second_trace = stasis_render_v2_trace(second_i32, second_f32, second_u8);
    CHECK(first_trace != 0);
    CHECK(first_trace == second_trace);

    second_i32[STASIS_RENDER_I_DRAWABLE_W] = 1920;
    second_i32[STASIS_RENDER_I_DRAWABLE_H] = 1080;
    second_i32[STASIS_RENDER_I_DISPLAY_GENERATION]++;
    second_i32[STASIS_RENDER_I_DENSITY_GENERATION]++;
    CHECK(first_trace == stasis_render_v2_trace(second_i32, second_f32, second_u8));

    second_f32[STASIS_RENDER_F_SPRITE_BASE + 1] += 0.5f;
    CHECK(first_trace != stasis_render_v2_trace(second_i32, second_f32, second_u8));
    second_i32[STASIS_RENDER_I_VERSION] = 99;
    CHECK(stasis_render_v2_validate(second_i32, second_f32) == STASIS_RENDER_V2_BAD_VERSION);
    CHECK(strcmp(
        stasis_render_v2_validation_name(STASIS_RENDER_V2_BAD_VERSION),
        "unsupported_version") == 0);
    CHECK(!stasis_render_v2_is_valid(second_i32));
    CHECK(stasis_render_v2_trace(second_i32, second_f32, second_u8) == 0);

    second_i32[STASIS_RENDER_I_MAGIC] = 0;
    CHECK(stasis_render_v2_validate(second_i32, second_f32) == STASIS_RENDER_V2_BAD_MAGIC);
    CHECK(strcmp(
        stasis_render_v2_validation_name(STASIS_RENDER_V2_BAD_MAGIC),
        "invalid_magic") == 0);

    build_representative_frame(second_i32, second_f32, second_u8);
    second_i32[STASIS_RENDER_I_TEXT_BASE + 1] = 1;
    second_i32[STASIS_RENDER_I_TEXT_BASE + 2] = INT32_MAX;
    CHECK(!stasis_render_v2_text_span_is_valid(1, INT32_MAX, 5));
    CHECK(stasis_render_v2_text_span_is_valid(0, 4, 5));
    CHECK(!stasis_render_v2_text_span_is_valid(0, 5, 5));
    CHECK(stasis_render_v2_trace(second_i32, second_f32, second_u8) != 0);

    free(first_i32); free(first_f32); free(first_u8);
    free(second_i32); free(second_f32); free(second_u8);
#undef CHECK
    return 0;
}
