#include <stdint.h>

#define CHECK(condition) do { if (!(condition)) return __LINE__; } while (0)

int stasis_init_window(int width, int height, const char* title);
int stasis_host_get_window_placement(
    int32_t* out_i32, int32_t capacity, float* out_f32, int32_t float_capacity);
int stasis_host_apply_window_placement(
    int32_t x, int32_t y, int32_t width, int32_t height, int32_t raise);
int stasis_host_focus_window(void);
int stasis_host_get_monitor_usable_bounds(
    int32_t x, int32_t y, int32_t* out_i32, int32_t capacity,
    float* out_f32, int32_t float_capacity);
void stasis_shutdown(void);

int main(void) {
    int32_t placement[10] = {0};
    float window_scales[2] = {0.0f, 0.0f};
    int32_t monitor[4] = {0};
    float monitor_scales[2] = {0.0f, 0.0f};

    CHECK(stasis_init_window(640, 480, "Stasis window placement contract"));
    CHECK(stasis_host_get_window_placement(placement, 10, window_scales, 2));
    CHECK(placement[2] > 0 && placement[3] > 0);
    CHECK(placement[6] > 0 && placement[7] > 0);
    CHECK(window_scales[0] > 0.0f && window_scales[1] > 0.0f);

    CHECK(stasis_host_get_monitor_usable_bounds(
        placement[4], placement[5], monitor, 4, monitor_scales, 2));
    CHECK(monitor[0] == placement[4] && monitor[1] == placement[5]);
    CHECK(monitor[2] == placement[6] && monitor[3] == placement[7]);
    CHECK(monitor_scales[0] > 0.0f && monitor_scales[1] > 0.0f);

    const int32_t width = monitor[2] < 640 ? monitor[2] : 640;
    const int32_t height = monitor[3] < 480 ? monitor[3] : 480;
    CHECK(stasis_host_apply_window_placement(
        monitor[0], monitor[1], width, height, 0));
    CHECK(stasis_host_get_window_placement(placement, 10, window_scales, 2));
    CHECK(placement[0] == monitor[0] && placement[1] == monitor[1]);
    CHECK(placement[2] == width && placement[3] == height);
    CHECK(stasis_host_focus_window());

    stasis_shutdown();
    return 0;
}
