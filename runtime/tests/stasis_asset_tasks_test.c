#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

#if defined(_WIN32)
#include <windows.h>
#else
#include <unistd.h>
#endif

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_asset_request_sprite(const char* path, int max_w, int max_h);
int stasis_asset_request_audio(const char* path);
int stasis_asset_task_poll(int task);
int stasis_asset_task_take_handle(int task);
void stasis_asset_task_cancel(int task);
void stasis_gfx_release_sprite(int handle);
int stasis_test_get_sprite_state(int handle, int* out_i32, int capacity);
int stasis_test_push_display_event(
    int kind,
    int logical_w,
    int logical_h,
    int native_w,
    int native_h,
    int drawable_w,
    int drawable_h,
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

    int sprite_state[4] = {0};
    CHECK(stasis_test_get_sprite_state(sprite, sprite_state, 4) == 1);
    CHECK(sprite_state[0] == 1 && sprite_state[1] == 1 && sprite_state[3] >= 1);

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

    /* A display lifecycle round-trip must not restore a released generation. */
    int32_t host_i32[768] = {0};
    float host_f32[64] = {0};
    CHECK(stasis_test_push_display_event(2, 64, 64, 64, 64, 64, 64, 0, 0, 64, 64) == 1);
    CHECK(stasis_mobile_poll_events() == 0);
    stasis_host_get_frame(host_i32, host_f32);
    CHECK(host_i32[18] == 1);
    CHECK(stasis_test_push_display_event(3, 64, 64, 64, 64, 64, 64, 0, 0, 64, 64) == 1);
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
