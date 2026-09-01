#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

#include "../stasis_render_contract.h"

#if defined(_WIN32)
#include <windows.h>
#else
#include <unistd.h>
#endif

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_asset_request_sprite(const char* path, int max_w, int max_h);
int stasis_asset_request_sprite_with_policy(
    const char* path, int max_w, int max_h, int atlas_eligible);
int stasis_asset_request_sprite_with_policy_v3(
    const char* path,
    int max_w,
    int max_h,
    int atlas_eligible,
    uint64_t group_id,
    uint32_t member_count,
    uint64_t logical_pixel_area,
    uint32_t max_logical_width,
    uint32_t max_logical_height);
int stasis_asset_request_audio(const char* path);
int stasis_asset_task_poll(int task);
int stasis_asset_task_take_handle(int task);
void stasis_asset_task_cancel(int task);
void stasis_gfx_release_sprite(int handle);
int stasis_gfx_load_sprite(const char* path, int max_w, int max_h);
void stasis_gfx_set_next_sprite_atlas_policy_v3(
    int eligible,
    uint64_t group_id,
    uint32_t member_count,
    uint64_t logical_pixel_area,
    uint32_t max_logical_width,
    uint32_t max_logical_height);
int stasis_test_get_sprite_state(int handle, int* out_i32, int capacity);
int stasis_test_get_render_submission_state(int32_t* out_i32, int32_t capacity);
void stasis_gfx_submit(int32_t* cmd_i32, const float* cmd_f32);
int stasis_test_push_display_event(
    int kind,
    int logical_w,
    int logical_h,
    int native_w,
    int native_h,
    int drawable_w,
    int drawable_h,
    int available_w,
    int available_h,
    int safe_x,
    int safe_y,
    int safe_w,
    int safe_h);
int stasis_mobile_poll_events(void);
void stasis_host_get_frame(int32_t* out_i32, float* out_f32);
void stasis_audio_release(int handle);
void stasis_sleep_ms(int ms);

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static int wait_for_task(int task) {
    for (int tick = 0; tick < 1000; tick++) {
        int state = stasis_asset_task_poll(task);
        if (state == 3 || state == 4) return state;
        CHECK(state == 1 || state == 2);
        stasis_sleep_ms(1);
    }
    return 0;
}

static int atlas_allocations_overlap(const int* left, const int* right) {
    const int left_x = left[13];
    const int left_y = left[14];
    const int right_x = right[13];
    const int right_y = right[14];
    return left_x < right_x + right[15] && right_x < left_x + left[15] &&
           left_y < right_y + right[16] && right_y < left_y + left[16];
}

int main(void) {
#if defined(_WIN32)
    _putenv_s("STASIS_ENABLE_TEST_INPUT", "1");
#else
    setenv("STASIS_ENABLE_TEST_INPUT", "1", 1);
#endif
    CHECK(stasis_init_window(64, 64, "stasis_asset_tasks_test"));

    int sprite_task = stasis_asset_request_sprite(STASIS_TEST_SPRITE_PATH, 32, 32);
    int audio_task = stasis_asset_request_audio(STASIS_TEST_AUDIO_PATH);
    CHECK(sprite_task > 0);
    CHECK(audio_task > 0);

    CHECK(wait_for_task(sprite_task) == 3);
    CHECK(wait_for_task(audio_task) == 3);
    int sprite = stasis_asset_task_take_handle(sprite_task);
    int audio = stasis_asset_task_take_handle(audio_task);
    CHECK(sprite > 0);
    CHECK(audio > 0);
    CHECK(stasis_asset_task_poll(sprite_task) == 0);
    CHECK(stasis_asset_task_poll(audio_task) == 0);

    int sprite_state[7] = {0};
    CHECK(stasis_test_get_sprite_state(sprite, sprite_state, 5) == 1);
    CHECK(sprite_state[0] == 1 && sprite_state[1] == 1 && sprite_state[3] >= 1);
    CHECK(sprite_state[4] == 0);

    /* The legacy boolean ABI is standalone-safe under the v3 contract. */
    int legacy_task = stasis_asset_request_sprite_with_policy(
        STASIS_TEST_SPRITE_PATH, 20, 20, 1);
    CHECK(legacy_task > 0);
    CHECK(wait_for_task(legacy_task) == 3);
    int legacy_sprite = stasis_asset_task_take_handle(legacy_task);
    CHECK(legacy_sprite > 0);
    CHECK(stasis_test_get_sprite_state(legacy_sprite, sprite_state, 7) == 1);
    CHECK(sprite_state[4] == 0 && sprite_state[5] == 0 && sprite_state[6] == 0);

    /* Each async request owns its complete policy, even when requests interleave. */
    const uint64_t hot_group = UINT64_C(0x12345678abcdef01);
    int hot_task = stasis_asset_request_sprite_with_policy(
        STASIS_TEST_SPRITE_PATH, 22, 22, 1);
    int cold_task = stasis_asset_request_sprite_with_policy_v3(
        STASIS_TEST_SPRITE_PATH, 28, 28, 0, 99, 8, 8192, 32, 32);
    int v3_hot_task = stasis_asset_request_sprite_with_policy_v3(
        STASIS_TEST_SPRITE_PATH, 24, 24, 1, hot_group, 8, 8192, 32, 32);
    CHECK(hot_task > 0 && cold_task > 0 && v3_hot_task > 0);
    CHECK(wait_for_task(hot_task) == 3);
    int hot_sprite = stasis_asset_task_take_handle(hot_task);
    CHECK(hot_sprite > 0);
    CHECK(stasis_test_get_sprite_state(hot_sprite, sprite_state, 7) == 1);
    CHECK(sprite_state[4] == 0);
    CHECK(wait_for_task(cold_task) == 3);
    int cold_sprite = stasis_asset_task_take_handle(cold_task);
    CHECK(cold_sprite > 0);
    CHECK(stasis_test_get_sprite_state(cold_sprite, sprite_state, 7) == 1);
    CHECK(sprite_state[4] == 0);
    CHECK(wait_for_task(v3_hot_task) == 3);
    int v3_hot_sprite = stasis_asset_task_take_handle(v3_hot_task);
    CHECK(v3_hot_sprite > 0);
    CHECK(stasis_test_get_sprite_state(v3_hot_sprite, sprite_state, 7) == 1);
    CHECK(sprite_state[4] == 1);
    CHECK((uint32_t)sprite_state[5] == (uint32_t)hot_group);
    CHECK((uint32_t)sprite_state[6] == (uint32_t)(hot_group >> 32));

    /* A changed accepted policy migrates the existing cache entry in place. */
    int migrated_task = stasis_asset_request_sprite_with_policy_v3(
        STASIS_TEST_SPRITE_PATH, 24, 24, 1, hot_group + 1, 8, 8192, 32, 32);
    CHECK(migrated_task > 0);
    CHECK(wait_for_task(migrated_task) == 3);
    int migrated_sprite = stasis_asset_task_take_handle(migrated_task);
    CHECK(migrated_sprite == v3_hot_sprite);
    CHECK(stasis_test_get_sprite_state(v3_hot_sprite, sprite_state, 7) == 1);
    CHECK(sprite_state[1] == 2 && sprite_state[4] == 1);
    CHECK((uint32_t)sprite_state[5] == (uint32_t)(hot_group + 1));
    stasis_gfx_release_sprite(v3_hot_sprite);
    stasis_gfx_release_sprite(migrated_sprite);
    stasis_gfx_release_sprite(hot_sprite);
    stasis_gfx_release_sprite(cold_sprite);
    stasis_gfx_release_sprite(legacy_sprite);

    int shared_task = stasis_asset_request_sprite(STASIS_TEST_SPRITE_PATH, 32, 32);
    CHECK(shared_task > 0);
    CHECK(wait_for_task(shared_task) == 3);
    int shared_sprite = stasis_asset_task_take_handle(shared_task);
    CHECK(shared_sprite == sprite);
    CHECK(stasis_test_get_sprite_state(sprite, sprite_state, 4) == 1);
    CHECK(sprite_state[0] == 1 && sprite_state[1] == 2 && sprite_state[3] == 1);

    int cancelled = stasis_asset_request_audio(STASIS_TEST_AUDIO_PATH);
    CHECK(cancelled > 0);
    stasis_asset_task_cancel(cancelled);
    CHECK(stasis_asset_task_poll(cancelled) == 0 || stasis_asset_task_poll(cancelled) == 5);

    stasis_gfx_release_sprite(sprite);
    CHECK(stasis_test_get_sprite_state(sprite, sprite_state, 4) == 1);
    CHECK(sprite_state[0] == 1 && sprite_state[1] == 1 && sprite_state[3] == 1);
    stasis_gfx_release_sprite(shared_sprite);
    CHECK(stasis_test_get_sprite_state(sprite, sprite_state, 4) == 1);
    CHECK(sprite_state[0] == 0 && sprite_state[1] == 0);

    int reused_task = stasis_asset_request_sprite(STASIS_TEST_SPRITE_PATH, 32, 32);
    CHECK(reused_task > 0);
    CHECK(wait_for_task(reused_task) == 3);
    int reused_sprite = stasis_asset_task_take_handle(reused_task);
    CHECK(reused_sprite > 0 && reused_sprite != sprite);
    CHECK(stasis_test_get_sprite_state(sprite, sprite_state, 4) == 1);
    CHECK(sprite_state[0] == 0 && sprite_state[1] == 0 && sprite_state[3] == 1);
    stasis_gfx_release_sprite(reused_sprite);
    CHECK(stasis_test_get_sprite_state(reused_sprite, sprite_state, 4) == 1);
    CHECK(sprite_state[0] == 0 && sprite_state[1] == 0 && sprite_state[3] == 0);

    /* Released shared regions are reused instead of advancing the shelf forever. */
    const uint64_t reuse_group = UINT64_C(0xfedcba9876543210);
    int atlas_state[18] = {0};
    stasis_gfx_set_next_sprite_atlas_policy_v3(1, reuse_group, 4, 1024, 16, 16);
    int reuse_anchor = stasis_gfx_load_sprite(STASIS_TEST_SPRITE_PATH, 12, 12);
    CHECK(reuse_anchor > 0);
    stasis_gfx_set_next_sprite_atlas_policy_v3(1, reuse_group, 4, 1024, 16, 16);
    int first_reuse = stasis_gfx_load_sprite(STASIS_TEST_SPRITE_PATH, 16, 16);
    CHECK(first_reuse > 0);
    CHECK(stasis_test_get_sprite_state(first_reuse, atlas_state, 18) == 1);
    const int reused_page = atlas_state[12];
    const int reused_x = atlas_state[13];
    const int reused_y = atlas_state[14];
    stasis_gfx_release_sprite(first_reuse);
    stasis_gfx_set_next_sprite_atlas_policy_v3(1, reuse_group, 4, 1024, 16, 16);
    int second_reuse = stasis_gfx_load_sprite(STASIS_TEST_SPRITE_PATH, 16, 16);
    CHECK(second_reuse > 0);
    CHECK(stasis_test_get_sprite_state(second_reuse, atlas_state, 18) == 1);
    CHECK(atlas_state[12] == reused_page);
    CHECK(atlas_state[13] == reused_x && atlas_state[14] == reused_y);

    /* A shelf wrap persists its y coordinate for every following allocation. */
    const uint64_t wrap_group = UINT64_C(0x0ddc0ffeebadf00d);
    int wrap_first_state[18] = {0};
    int wrap_second_state[18] = {0};
    int wrap_third_state[18] = {0};
    /* Complete group evidence includes one unloaded 1x63 member. */
    stasis_gfx_set_next_sprite_atlas_policy_v3(1, wrap_group, 4, 2471, 40, 63);
    int wrap_first = stasis_gfx_load_sprite(STASIS_TEST_SPRITE_PATH, 40, 40);
    stasis_gfx_set_next_sprite_atlas_policy_v3(1, wrap_group, 4, 2471, 40, 63);
    int wrap_second = stasis_gfx_load_sprite(STASIS_TEST_SPRITE_PATH, 22, 22);
    stasis_gfx_set_next_sprite_atlas_policy_v3(1, wrap_group, 4, 2471, 40, 63);
    int wrap_third = stasis_gfx_load_sprite(STASIS_TEST_SPRITE_PATH, 18, 18);
    CHECK(wrap_first > 0 && wrap_second > 0 && wrap_third > 0);
    CHECK(wrap_first != wrap_second && wrap_first != wrap_third &&
          wrap_second != wrap_third);
    CHECK(stasis_test_get_sprite_state(wrap_first, wrap_first_state, 18) == 1);
    CHECK(stasis_test_get_sprite_state(wrap_second, wrap_second_state, 18) == 1);
    CHECK(stasis_test_get_sprite_state(wrap_third, wrap_third_state, 18) == 1);
    CHECK(wrap_first_state[4] == 1 && wrap_second_state[4] == 1 &&
          wrap_third_state[4] == 1);
    CHECK((uint32_t)wrap_first_state[5] == (uint32_t)wrap_group &&
          (uint32_t)wrap_first_state[6] == (uint32_t)(wrap_group >> 32));
    CHECK(wrap_first_state[12] == wrap_second_state[12] &&
          wrap_second_state[12] == wrap_third_state[12]);

    /* The policy produces a 64x128 page; coordinates include the padded allocation. */
    CHECK(wrap_first_state[13] == 1 && wrap_first_state[14] == 6);
    CHECK(wrap_second_state[13] == 1 && wrap_second_state[14] == 48);
    CHECK(wrap_third_state[13] == 25 && wrap_third_state[14] == 48);
    CHECK(wrap_second_state[14] == wrap_third_state[14]);
    CHECK(!atlas_allocations_overlap(wrap_first_state, wrap_second_state));
    CHECK(!atlas_allocations_overlap(wrap_first_state, wrap_third_state));
    CHECK(!atlas_allocations_overlap(wrap_second_state, wrap_third_state));
    for (int i = 0; i < 3; i++) {
        const int* state = i == 0 ? wrap_first_state :
            (i == 1 ? wrap_second_state : wrap_third_state);
        CHECK(state[13] >= 1 && state[14] >= 6);
        CHECK(state[13] + state[15] <= 64);
        CHECK(state[14] + state[16] <= 128);
    }
    stasis_gfx_release_sprite(wrap_third);
    stasis_gfx_release_sprite(wrap_second);
    stasis_gfx_release_sprite(wrap_first);

    /* An explicit sprite-run clip intersects the ordered parent and is restored. */
    int32_t* frame_i32 = (int32_t*)calloc(STASIS_RENDER_I32_COUNT, sizeof(int32_t));
    float* frame_f32 = (float*)calloc(STASIS_RENDER_F32_COUNT, sizeof(float));
    CHECK(frame_i32 != NULL && frame_f32 != NULL);
    frame_i32[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_MAGIC;
    frame_i32[STASIS_RENDER_I_VERSION] = STASIS_RENDER_VERSION;
    frame_i32[STASIS_RENDER_I_SPRITE_COUNT] = 1;
    frame_i32[STASIS_RENDER_I_ORDER_COUNT] = 3;
    frame_i32[STASIS_RENDER_I_CLIP_COUNT] = 2;
    frame_i32[STASIS_RENDER_I_SPRITE_RUN_COUNT] = 1;
    frame_i32[STASIS_RENDER_I_SPRITE_BASE] = second_reuse;
    frame_i32[STASIS_RENDER_I_SPRITE_BASE + 1] = (int32_t)UINT32_C(0xffffffff);
    frame_i32[STASIS_RENDER_I_SPRITE_RUN_BASE + 0] = 0;
    frame_i32[STASIS_RENDER_I_SPRITE_RUN_BASE + 1] = 1;
    frame_i32[STASIS_RENDER_I_SPRITE_RUN_BASE + 2] = 1;
    frame_i32[STASIS_RENDER_I_ORDER_BASE + 0] =
        STASIS_RENDER_ORDER_CLIP_PUSH * STASIS_RENDER_ORDER_KIND_SCALE;
    frame_i32[STASIS_RENDER_I_ORDER_BASE + 1] =
        STASIS_RENDER_ORDER_SPRITE * STASIS_RENDER_ORDER_KIND_SCALE;
    frame_i32[STASIS_RENDER_I_ORDER_BASE + 2] =
        STASIS_RENDER_ORDER_CLIP_POP * STASIS_RENDER_ORDER_KIND_SCALE;
    frame_f32[STASIS_RENDER_F_SPRITE_BASE + 2] = 8.0f;
    frame_f32[STASIS_RENDER_F_SPRITE_BASE + 3] = 8.0f;
    frame_f32[STASIS_RENDER_F_SPRITE_BASE + 10] = 1.0f;
    frame_f32[STASIS_RENDER_F_SPRITE_BASE + 11] = 1.0f;
    frame_f32[STASIS_RENDER_F_CLIP_BASE + 0] = 0.0f;
    frame_f32[STASIS_RENDER_F_CLIP_BASE + 1] = 0.0f;
    frame_f32[STASIS_RENDER_F_CLIP_BASE + 2] = 10.0f;
    frame_f32[STASIS_RENDER_F_CLIP_BASE + 3] = 10.0f;
    frame_f32[STASIS_RENDER_F_CLIP_BASE + 4] = 5.0f;
    frame_f32[STASIS_RENDER_F_CLIP_BASE + 5] = 5.0f;
    frame_f32[STASIS_RENDER_F_CLIP_BASE + 6] = 10.0f;
    frame_f32[STASIS_RENDER_F_CLIP_BASE + 7] = 10.0f;
    stasis_gfx_submit(frame_i32, frame_f32);
    int32_t render_state[19] = {0};
    CHECK(stasis_test_get_render_submission_state(render_state, 19) == 1);
    CHECK(render_state[12] == 1 && render_state[13] == 1);
    CHECK(render_state[14] == 5 && render_state[15] == 5);
    CHECK(render_state[16] == 5 && render_state[17] == 5);
    CHECK(render_state[18] == 1);
    free(frame_i32);
    free(frame_f32);

    stasis_gfx_release_sprite(second_reuse);
    stasis_gfx_release_sprite(reuse_anchor);

    /* Dedicated page slots are destroyed and reused under churn past the cap. */
    for (int cycle = 0; cycle < 260; cycle++) {
        stasis_gfx_set_next_sprite_atlas_policy_v3(0, 0, 0, 0, 0, 0);
        int churned = stasis_gfx_load_sprite(STASIS_TEST_SPRITE_PATH, 8, 8);
        CHECK(churned > 0);
        CHECK(stasis_test_get_sprite_state(churned, atlas_state, 18) == 1);
        CHECK(atlas_state[17] <= 256);
        stasis_gfx_release_sprite(churned);
    }

    /* A display lifecycle round-trip must not restore a released generation. */
    int32_t host_i32[768] = {0};
    float host_f32[64] = {0};
    CHECK(stasis_test_push_display_event(
              2, 64, 64, 64, 64, 64, 64, 64, 64, 0, 0, 64, 64) == 1);
    CHECK(stasis_mobile_poll_events() == 0);
    stasis_host_get_frame(host_i32, host_f32);
    CHECK(host_i32[18] == 1);
    CHECK(stasis_test_push_display_event(
              3, 64, 64, 64, 64, 64, 64, 64, 64, 0, 0, 64, 64) == 1);
    CHECK(stasis_mobile_poll_events() == 0);
    stasis_host_get_frame(host_i32, host_f32);
    CHECK(host_i32[18] == 0);
    CHECK(stasis_test_get_sprite_state(sprite, sprite_state, 4) == 1);
    CHECK(sprite_state[0] == 0 && sprite_state[1] == 0 && sprite_state[3] == 0);

    stasis_gfx_release_sprite(sprite);
    stasis_audio_release(audio);
    stasis_shutdown();
    puts("stasis_asset_tasks_test: ok");
    return 0;
}
