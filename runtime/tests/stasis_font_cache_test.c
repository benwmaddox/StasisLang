#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#if defined(_WIN32)
#include <sys/utime.h>
#else
#include <utime.h>
#endif

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_load_font(const char* path, int font_size);

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static unsigned char* read_file(const char* path, size_t* size_out) {
    FILE* file = fopen(path, "rb");
    CHECK(file != NULL);
    CHECK(fseek(file, 0, SEEK_END) == 0);
    long length = ftell(file);
    CHECK(length > 0);
    CHECK(fseek(file, 0, SEEK_SET) == 0);
    unsigned char* bytes = (unsigned char*)malloc((size_t)length);
    CHECK(bytes != NULL);
    CHECK(fread(bytes, 1, (size_t)length, file) == (size_t)length);
    CHECK(fclose(file) == 0);
    *size_out = (size_t)length;
    return bytes;
}

static void write_file(const char* path, const unsigned char* bytes, size_t size) {
    FILE* file = fopen(path, "wb");
    CHECK(file != NULL);
    CHECK(fwrite(bytes, 1, size, file) == size);
    CHECK(fclose(file) == 0);
}

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

    const char* replacement_path = "stasis_font_cache_test_replacement.ttf";
    remove(replacement_path);
    size_t replacement_size = 0;
    unsigned char* replacement = read_file(STASIS_TEST_FONT_PATH, &replacement_size);
    write_file(replacement_path, replacement, replacement_size);
#if defined(_WIN32)
    struct _stat replacement_stat;
    CHECK(_stat(replacement_path, &replacement_stat) == 0);
#else
    struct stat replacement_stat;
    CHECK(stat(replacement_path, &replacement_stat) == 0);
#endif
    int replacement_handle = stasis_load_font(replacement_path, 22);
    CHECK(replacement_handle > 0);

    memset(replacement, 0, replacement_size);
    write_file(replacement_path, replacement, replacement_size);
#if defined(_WIN32)
    struct _utimbuf original_times = {replacement_stat.st_atime, replacement_stat.st_mtime};
    CHECK(_utime(replacement_path, &original_times) == 0);
#else
    struct utimbuf original_times = {replacement_stat.st_atime, replacement_stat.st_mtime};
    CHECK(utime(replacement_path, &original_times) == 0);
#endif
    CHECK(stasis_load_font(replacement_path, 22) == 0);
    free(replacement);
    CHECK(remove(replacement_path) == 0);

    stasis_shutdown();
    puts("stasis_font_cache_test: ok");
    return 0;
}
