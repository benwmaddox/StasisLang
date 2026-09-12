/* Deterministic external inputs for the verbatim desktop HostFrame writer.
 * No ABI output indices belong here: those come only from stasis_graphics.c.
 */
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#define STASIS_EXPORT
#define STASIS_MAX_POINTERS 8
#define SDL_WINDOW_INPUT_FOCUS 1
typedef uint32_t Uint32;
typedef struct {
    int id, is_down, went_down, went_up;
    float x_px, y_px, dx_px, dy_px, x_n, y_n;
} StasisPointer;
static struct {
    StasisPointer pointers[STASIS_MAX_POINTERS];
    int pointer_count, dropped_pointers;
} g_input_frame;
static struct {
    int native_w, native_h, drawable_w, drawable_h;
    float content_scale, raster_scale, logical_w, logical_h;
    struct { float x, y, w, h; } safe_logical_viewport;
} g_display_metrics = {1280, 720, 1920, 1080, 1.5f, 2.0f, 320, 180, {10, 5, 300, 170}};
static bool g_window_resized = true, g_window_minimized = false;
static int g_available_width = 640, g_available_height = 360;
static int g_display_generation = 7, g_density_generation = 9;
static int g_keyboard_event_state[512];
static void* g_window = &g_window_resized;
static int stasis_should_quit(void) { return 1; }
static int stasis_get_time_ms(void) { return 101; }
static int stasis_get_time_us(void) { return 1001; }
static Uint32 SDL_GetWindowFlags(void* window) { (void)window; return SDL_WINDOW_INPUT_FOCUS; }
static const bool* SDL_GetKeyboardState(int* count) { *count = 0; return NULL; }

/* NATIVE_WRITER */

int main(int argc, char** argv) {
    if (argc != 2) return 2;
    g_input_frame.pointer_count = atoi(argv[1]);
    g_input_frame.dropped_pointers = 2;
    for (int i = 0; i < 512; i++) g_keyboard_event_state[i] = (i * 3 + 1) % 11;
    for (int i = 0; i < STASIS_MAX_POINTERS; i++) {
        StasisPointer* p = &g_input_frame.pointers[i];
        p->id = 100 + i;
        p->is_down = i % 2;
        p->went_down = i == 0 || i == 7;
        p->went_up = i == 7;
        p->x_px = i * 10.0f + 0.25f;
        p->y_px = i * 10.0f + 0.5f;
        p->dx_px = i * 10.0f + 0.75f;
        p->dy_px = i * 10.0f + 1.0f;
        p->x_n = i / 8.0f;
        p->y_n = (8 - i) / 8.0f;
    }
    int32_t integers[768] = {0};
    float floats[64] = {0};
    stasis_host_get_frame(integers, floats);
    for (int i = 0; i < 768; i++) printf("%d\n", integers[i]);
    for (int i = 0; i < 64; i++) printf("%.9g\n", (double)floats[i]);
    return 0;
}
