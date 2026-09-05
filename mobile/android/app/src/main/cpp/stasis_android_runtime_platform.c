#include <android/log.h>
#include <stdint.h>
#include <string.h>
#include <time.h>

#define STASIS_RUNTIME_LOG_TAG "StasisRuntime"

static int stable_sprite_handle(const char *path) {
    const char *name;
    const char *end;
    uint32_t hash = 2166136261U;
    const char prefix[] = "sprite:";
    size_t index;
    if (path == NULL || path[0] == '\0') return 0;
    name = strrchr(path, '/');
    name = name == NULL ? path : name + 1;
    end = strrchr(name, '.');
    if (end == NULL) end = name + strlen(name);
    for (index = 0; index < sizeof(prefix) - 1; index += 1) {
        hash ^= (uint8_t)prefix[index];
        hash *= 16777619U;
    }
    while (name < end) {
        hash ^= (uint8_t)*name++;
        hash *= 16777619U;
    }
    return hash == 0 ? 1 : (int32_t)hash;
}

int stasis_gfx_load_sprite(const char *path, int max_w, int max_h) {
    if (max_w <= 0 || max_h <= 0) return 0;
    return stable_sprite_handle(path);
}

void stasis_gfx_release_sprite(int handle) { (void)handle; }
int stasis_gfx_dump_bmp(const char *path) { (void)path; return 0; }
int stasis_gfx_cache_text(int font, const char *text) { (void)font; (void)text; return 0; }
int stasis_gfx_replace_text(int handle, int font, const char *text) { (void)handle; (void)font; (void)text; return 0; }
int stasis_gfx_poll_reload(int handle) { return handle > 0 ? 0 : -1; }
float stasis_gfx_measure_text_cached(int handle) { (void)handle; return 0.0f; }

int stasis_load_font(const char *path, int size) {
    (void)path;
    (void)size;
    __android_log_print(ANDROID_LOG_WARN, STASIS_RUNTIME_LOG_TAG,
        "published Android text rendering is unavailable");
    return 0;
}

float stasis_measure_text(int font, const char *text) {
    (void)font;
    (void)text;
    return 0.0f;
}

void stasis_sleep_ms(int milliseconds) {
    struct timespec delay;
    if (milliseconds <= 0) return;
    delay.tv_sec = milliseconds / 1000;
    delay.tv_nsec = (long)(milliseconds % 1000) * 1000000L;
    nanosleep(&delay, NULL);
}
