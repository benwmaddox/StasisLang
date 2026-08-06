#include <stdio.h>
#include <stdlib.h>

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_load_font(const char* path, int font_size);

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

int main(void) {
    CHECK(stasis_init_window(64, 64, "stasis_font_cache_test"));

    int first = stasis_load_font(STASIS_TEST_FONT_PATH, 18);
    CHECK(first > 0);
    for (int i = 0; i < 16; i++) {
        CHECK(stasis_load_font(STASIS_TEST_FONT_PATH, 18) == first);
    }

    int second_size = stasis_load_font(STASIS_TEST_FONT_PATH, 20);
    CHECK(second_size > 0);
    CHECK(second_size != first);

    stasis_shutdown();
    puts("stasis_font_cache_test: ok");
    return 0;
}
