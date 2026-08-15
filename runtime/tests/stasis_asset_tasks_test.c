#include <stdio.h>
#include <stdlib.h>

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_asset_request_sprite(const char* path, int max_w, int max_h);
int stasis_asset_request_audio(const char* path);
int stasis_asset_task_poll(int task);
int stasis_asset_task_take_handle(int task);
void stasis_asset_task_cancel(int task);
void stasis_gfx_release_sprite(int handle);
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

    int cancelled = stasis_asset_request_audio(STASIS_TEST_AUDIO_PATH);
    CHECK(cancelled > 0);
    stasis_asset_task_cancel(cancelled);
    CHECK(stasis_asset_task_poll(cancelled) == 0 || stasis_asset_task_poll(cancelled) == 5);

    stasis_gfx_release_sprite(sprite);
    stasis_audio_release(audio);
    stasis_shutdown();
    puts("stasis_asset_tasks_test: ok");
    return 0;
}
