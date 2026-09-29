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
void stasis_host_bulk_init(const int32_t* host_req_seq);
void stasis_host_bulk_apply_requests(
    const int32_t* host_req_seq, const int32_t* host_req_flags,
    const int32_t* host_req_window_w_px, const int32_t* host_req_window_h_px);
void stasis_get_display_metrics(
    int* logical_w, int* logical_h, int* native_w, int* native_h,
    int* drawable_w, int* drawable_h, int* safe_x, int* safe_y,
    int* safe_w, int* safe_h, float* content_scale, float* raster_scale,
    int* display_generation, int* density_generation);

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

    int before_native_w = 0, before_native_h = 0;
    int after_logical_w = 0, after_logical_h = 0;
    int after_native_w = 0, after_native_h = 0;
    stasis_get_display_metrics(NULL, NULL, &before_native_w, &before_native_h,
        NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);
    int32_t seq = 0;
    const int32_t canvas_flag = 8;
    const int32_t canvas_w = 1000;
    const int32_t canvas_h = 600;
    stasis_host_bulk_init(&seq);
    seq = 1;
    stasis_host_bulk_apply_requests(&seq, &canvas_flag, &canvas_w, &canvas_h);
    stasis_get_display_metrics(&after_logical_w, &after_logical_h,
        &after_native_w, &after_native_h, NULL, NULL, NULL, NULL,
        NULL, NULL, NULL, NULL, NULL, NULL);
    CHECK(after_logical_w == canvas_w && after_logical_h == canvas_h);
    CHECK(after_native_w == before_native_w && after_native_h == before_native_h);

    stasis_shutdown();
    return 0;
}
