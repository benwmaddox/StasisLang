#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <sys/stat.h>
#if defined(_WIN32)
#include <direct.h>
#include <process.h>
#include <sys/utime.h>
#else
#include <sys/types.h>
#include <unistd.h>
#include <utime.h>
#endif

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_set_asset_root(const char* path);
int stasis_load_font(const char* path, int font_size);

static char g_temp_dir[512];
static char g_identity_paths[10][768];
static char g_replacement_path[768];
static int g_temp_dir_created;

static void cleanup_font_cache_temp_files(void) {
    if (!g_temp_dir_created) return;
    for (int i = 0; i < 10; i++) {
        if (g_identity_paths[i][0] != '\0') remove(g_identity_paths[i]);
    }
    if (g_replacement_path[0] != '\0') remove(g_replacement_path);
#if defined(_WIN32)
    _rmdir(g_temp_dir);
#else
    rmdir(g_temp_dir);
#endif
    g_temp_dir_created = 0;
}

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

static int create_unique_temp_directory(const char* parent, char* out, size_t out_size) {
#if defined(_WIN32)
    long process_id = (long)_getpid();
#else
    long process_id = (long)getpid();
#endif
    for (unsigned int attempt = 0; attempt < 100; attempt++) {
        int written = snprintf(out, out_size, "%s/stasis_font_cache_test_%ld_%u",
            parent, process_id, attempt);
        if (written <= 0 || (size_t)written >= out_size) return 0;
#if defined(_WIN32)
        if (_mkdir(out) == 0) return 1;
#else
        if (mkdir(out, 0700) == 0) return 1;
#endif
        if (errno != EEXIST) return 0;
    }
    return 0;
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

    int large = stasis_load_font(STASIS_TEST_FONT_PATH, 100);
    CHECK(large > 0);
    CHECK(stasis_load_font(STASIS_TEST_FONT_PATH, 100) == large);

    size_t identity_size = 0;
    unsigned char* identity_bytes = read_file(STASIS_TEST_FONT_PATH, &identity_size);
    char identity_names[10][64];
    int identity_handles[10];
    const char* temp_parent = getenv("TEMP");
#if !defined(_WIN32)
    if (!temp_parent || !*temp_parent) temp_parent = getenv("TMPDIR");
#endif
    if (!temp_parent || !*temp_parent) temp_parent = ".";
    CHECK(create_unique_temp_directory(temp_parent, g_temp_dir, sizeof(g_temp_dir)));
    g_temp_dir_created = 1;
    CHECK(atexit(cleanup_font_cache_temp_files) == 0);
    CHECK(strlen(g_temp_dir) < 512);
    for (int i = 0; i < 10; i++) {
        int name_written = snprintf(identity_names[i], sizeof(identity_names[i]),
            "stasis_font_cache_test_identity_%02d.ttf", i);
        CHECK(name_written > 0 && (size_t)name_written < sizeof(identity_names[i]));
        int written = snprintf(g_identity_paths[i], sizeof(g_identity_paths[i]),
            "%s/%s", g_temp_dir, identity_names[i]);
        CHECK(written > 0 && (size_t)written < sizeof(g_identity_paths[i]));
        write_file(g_identity_paths[i], identity_bytes, identity_size);
    }
    CHECK(stasis_set_asset_root(g_temp_dir));
    for (int i = 0; i < 10; i++) {
        identity_handles[i] = stasis_load_font(identity_names[i], 18);
        CHECK(identity_handles[i] > 0);
        for (int prior = 0; prior < i; prior++) {
            CHECK(identity_handles[i] != identity_handles[prior]);
        }
        CHECK(stasis_load_font(identity_names[i], 18) == identity_handles[i]);
    }
    free(identity_bytes);

    const char* replacement_name = "stasis_font_cache_test_replacement.ttf";
    int replacement_written = snprintf(g_replacement_path, sizeof(g_replacement_path),
        "%s/%s", g_temp_dir, replacement_name);
    CHECK(replacement_written > 0 && (size_t)replacement_written < sizeof(g_replacement_path));
    size_t replacement_size = 0;
    unsigned char* replacement = read_file(STASIS_TEST_FONT_PATH, &replacement_size);
    write_file(g_replacement_path, replacement, replacement_size);
#if defined(_WIN32)
    struct _stat replacement_stat;
    CHECK(_stat(g_replacement_path, &replacement_stat) == 0);
#else
    struct stat replacement_stat;
    CHECK(stat(g_replacement_path, &replacement_stat) == 0);
#endif
    int replacement_handle = stasis_load_font(replacement_name, 22);
    CHECK(replacement_handle > 0);

    memset(replacement, 0, replacement_size);
    write_file(g_replacement_path, replacement, replacement_size);
#if defined(_WIN32)
    struct _utimbuf original_times = {replacement_stat.st_atime, replacement_stat.st_mtime};
    CHECK(_utime(g_replacement_path, &original_times) == 0);
#else
    struct utimbuf original_times = {replacement_stat.st_atime, replacement_stat.st_mtime};
    CHECK(utime(g_replacement_path, &original_times) == 0);
#endif
    CHECK(stasis_load_font(replacement_name, 22) == 0);
    free(replacement);

    stasis_shutdown();
    puts("stasis_font_cache_test: ok");
    return 0;
}
