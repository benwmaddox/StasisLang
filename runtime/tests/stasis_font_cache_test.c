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
int stasis_font_status(int handle);
void stasis_gfx_release_font(int handle);
int stasis_gfx_cache_text(int font, const char* text);
int stasis_gfx_replace_text(int handle, int font, const char* text);
float stasis_gfx_measure_text_cached(int handle);
float stasis_gfx_measure_text_cached_height(int handle);
void stasis_begin_frame(void);
int stasis_test_push_display_event(
    int kind, int logical_w, int logical_h, int native_w, int native_h,
    int drawable_w, int drawable_h, int available_w, int available_h,
    int safe_x, int safe_y, int safe_w, int safe_h);
int SDL_setenv_unsafe(const char* name, const char* value, int overwrite);
int SDL_unsetenv_unsafe(const char* name);

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

static void set_environment_value(const char* name, const char* value) {
    /* SDL_getenv reads SDL's synchronized cache after SDL_Init. The unsafe
       SDL helper updates that cache and the process environment together. */
    CHECK(SDL_setenv_unsafe(name, value, 1) == 0);
}

static void clear_environment_value(const char* name) {
    CHECK(SDL_unsetenv_unsafe(name) == 0);
}

static void test_density_rebuild_failure_retries(void) {
    stasis_shutdown();
    CHECK(stasis_init_window(64, 64, "stasis_font_cache_test_density_retry"));
    int font = stasis_load_font(STASIS_TEST_FONT_PATH, 18);
    CHECK(font > 0);
    int run = stasis_gfx_cache_text(font, "density retry");
    CHECK(run > 0);
    const float prior_width = stasis_gfx_measure_text_cached(run);
    const float prior_height = stasis_gfx_measure_text_cached_height(run);
    CHECK(prior_width > 0.0f && prior_height > 0.0f);

    set_environment_value("STASIS_ENABLE_TEST_INPUT", "1");
    CHECK(stasis_test_push_display_event(
        1, 64, 64, 128, 128, 128, 128, 128, 128, 0, 0, 128, 128));
    stasis_begin_frame();
    set_environment_value("STASIS_TEST_FONT_REBUILD_FAILURES", "2");
    CHECK(stasis_gfx_measure_text_cached(run) == 0.0f);
    CHECK(stasis_gfx_measure_text_cached(run) == 0.0f);
    clear_environment_value("STASIS_TEST_FONT_REBUILD_FAILURES");
    const float recovered_width = stasis_gfx_measure_text_cached(run);
    const float recovered_height = stasis_gfx_measure_text_cached_height(run);
    CHECK(recovered_width > 0.0f && recovered_height > 0.0f);
    CHECK(recovered_width >= prior_width - 1.0f && recovered_width <= prior_width + 1.0f);
    CHECK(recovered_height >= prior_height - 1.0f && recovered_height <= prior_height + 1.0f);

    /* A pending rebuild must not cross a full shutdown. The second lifecycle
       leaves one retry pending so reset_text_cache is exercised while stale
       handles remain invalid. */
    stasis_shutdown();
    CHECK(stasis_gfx_measure_text_cached(run) == 0.0f);
    CHECK(stasis_init_window(64, 64, "stasis_font_cache_test_density_pending_reset"));
    int pending_font = stasis_load_font(STASIS_TEST_FONT_PATH, 18);
    CHECK(pending_font > 0);
    int pending_run = stasis_gfx_cache_text(pending_font, "pending reset");
    CHECK(pending_run > 0);
    set_environment_value("STASIS_TEST_FONT_REBUILD_FAILURES", "2");
    CHECK(stasis_test_push_display_event(
        1, 64, 64, 256, 256, 256, 256, 256, 256, 0, 0, 256, 256));
    stasis_begin_frame();
    CHECK(stasis_gfx_measure_text_cached(pending_run) == 0.0f);
    stasis_shutdown();
    CHECK(stasis_gfx_measure_text_cached(pending_run) == 0.0f);
    /* A fresh request would fail if the pending rebuild or the remaining
       shutdown fault crossed the lifecycle boundary. */
    set_environment_value("STASIS_TEST_FONT_REBUILD_FAILURES", "1");

    CHECK(stasis_init_window(64, 64, "stasis_font_cache_test_density_reinit"));
    int reset_font = stasis_load_font(STASIS_TEST_FONT_PATH, 18);
    CHECK(reset_font > 0);
    int reset_run = stasis_gfx_cache_text(reset_font, "fresh after reset");
    CHECK(reset_run > 0);
    clear_environment_value("STASIS_TEST_FONT_REBUILD_FAILURES");
    CHECK(stasis_gfx_measure_text_cached(reset_run) > 0.0f);
    CHECK(stasis_gfx_measure_text_cached_height(reset_run) > 0.0f);
    stasis_gfx_release_font(reset_font);
    stasis_shutdown();
    clear_environment_value("STASIS_ENABLE_TEST_INPUT");
}

static void test_failed_immutable_cache_rolls_back_bytes(void) {
    stasis_shutdown();
    CHECK(stasis_init_window(64, 64, "stasis_font_cache_test_byte_rollback"));
    int font = stasis_load_font(STASIS_TEST_FONT_PATH, 18);
    CHECK(font > 0);

    char filled[1001];
    for (int index = 0; index < 60; index++) {
        int prefix = snprintf(filled, sizeof(filled), "%02d", index);
        CHECK(prefix > 0 && prefix < (int)sizeof(filled));
        memset(filled + prefix, 'A', sizeof(filled) - (size_t)prefix - 1);
        filled[sizeof(filled) - 1] = 0;
        CHECK(stasis_gfx_cache_text(font, filled) > 0);
    }

    char newlines[8193];
    memset(newlines, '\n', sizeof(newlines) - 1);
    newlines[sizeof(newlines) - 1] = 0;
    for (int attempt = 0; attempt < 24; attempt++) {
        CHECK(stasis_gfx_cache_text(font, newlines) == 0);
    }

    char surviving[5521];
    memset(surviving, 'A', sizeof(surviving) - 1);
    surviving[sizeof(surviving) - 1] = 0;
    CHECK(stasis_gfx_cache_text(font, surviving) > 0);

    stasis_gfx_release_font(font);
}

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
    CHECK(stasis_font_status(first) == 3);
    CHECK(stasis_font_status(0) == 0);
    for (int i = 0; i < 16; i++) {
        CHECK(stasis_load_font(STASIS_TEST_FONT_PATH, 18) == first);
    }

    int second_size = stasis_load_font(STASIS_TEST_FONT_PATH, 20);
    CHECK(second_size > 0);
    CHECK(second_size != first);
    int retained_fixed_run = stasis_gfx_cache_text(second_size, "retained before compaction");
    CHECK(retained_fixed_run > 0);

    int fixed_run = stasis_gfx_cache_text(first, "score 0");
    CHECK(fixed_run > 0);
    for (int i = 0; i < 16; i++) stasis_gfx_release_font(first);
    CHECK(stasis_gfx_measure_text_cached(fixed_run) > 0.0f);
    int shared_first = stasis_load_font(STASIS_TEST_FONT_PATH, 18);
    CHECK(shared_first == first);
    stasis_gfx_release_font(shared_first);
    CHECK(stasis_gfx_measure_text_cached(fixed_run) > 0.0f);
    CHECK(stasis_gfx_cache_text(first, "score 0") == fixed_run);
    int dynamic_run = stasis_gfx_replace_text(fixed_run, first, "score 0");
    CHECK(dynamic_run > 0 && dynamic_run != fixed_run);
    CHECK(stasis_gfx_cache_text(first, "score 0") == fixed_run);
    char score[32];
    for (int i = 1; i <= 5000; i++) {
        int written = snprintf(score, sizeof(score), "score %d", i);
        CHECK(written > 0 && (size_t)written < sizeof(score));
        CHECK(stasis_gfx_replace_text(dynamic_run, first, score) == dynamic_run);
    }
    CHECK(stasis_gfx_replace_text(dynamic_run, first, "Punktzahl \xc3\xa4") == dynamic_run);
    CHECK(stasis_gfx_replace_text(dynamic_run, second_size, "score 8") == dynamic_run);
    float prior_width = stasis_gfx_measure_text_cached(dynamic_run);
    float prior_height = stasis_gfx_measure_text_cached_height(dynamic_run);
    const char malformed[] = {(char)0xc3, '(', 0};
    CHECK(stasis_gfx_replace_text(dynamic_run, first, malformed) == 0);
    CHECK(stasis_gfx_replace_text(dynamic_run, 0, "invalid font") == 0);
    char oversized[1025];
    memset(oversized, 'x', sizeof(oversized) - 1);
    oversized[sizeof(oversized) - 1] = 0;
    CHECK(stasis_gfx_replace_text(dynamic_run, first, oversized) == 0);
    CHECK(stasis_gfx_measure_text_cached(dynamic_run) == prior_width);
    CHECK(stasis_gfx_measure_text_cached_height(dynamic_run) == prior_height);
    for (int i = 1; i < 16; i++) {
        int written = snprintf(score, sizeof(score), "dynamic %d", i);
        CHECK(written > 0 && (size_t)written < sizeof(score));
        CHECK(stasis_gfx_replace_text(0, first, score) > 0);
    }
    CHECK(stasis_gfx_replace_text(0, first, "capacity failure") == 0);
    CHECK(stasis_gfx_measure_text_cached(dynamic_run) == prior_width);

    int large = stasis_load_font(STASIS_TEST_FONT_PATH, 100);
    CHECK(large > 0);
    CHECK(stasis_load_font(STASIS_TEST_FONT_PATH, 100) == large);

    stasis_gfx_release_font(first);
    CHECK(stasis_font_status(first) == 0);
    CHECK(stasis_gfx_measure_text_cached(fixed_run) == 0.0f);
    CHECK(stasis_gfx_measure_text_cached(dynamic_run) == prior_width);
    CHECK(stasis_gfx_cache_text(first, "stale handle") == 0);
    int appended_after_compaction = stasis_gfx_cache_text(second_size, "appended after compaction");
    CHECK(appended_after_compaction > 0);
    stasis_gfx_release_font(large);
    CHECK(stasis_gfx_measure_text_cached(retained_fixed_run) > 0.0f);
    CHECK(stasis_gfx_measure_text_cached(appended_after_compaction) > 0.0f);
    stasis_gfx_release_font(large);
    stasis_gfx_release_font(second_size);
    int reused = stasis_load_font(STASIS_TEST_FONT_PATH, 19);
    CHECK(reused > 0);
    CHECK(reused != first);
    int reused_run = stasis_gfx_cache_text(reused, "reused safely");
    CHECK(reused_run > 0);
    stasis_gfx_release_font(reused);
    CHECK(stasis_gfx_cache_text(reused, "retired handle") == 0);
    int after_release = stasis_load_font(STASIS_TEST_FONT_PATH, 21);
    CHECK(after_release > 0 && after_release != reused);
    int after_release_run = stasis_gfx_cache_text(after_release, "new generation");
    CHECK(after_release_run > 0 && after_release_run != reused_run);
    CHECK(stasis_gfx_measure_text_cached(reused_run) == 0.0f);
    stasis_gfx_release_font(after_release);

    int previous_font = 0;
    for (int size = 4; size < 68; size++) {
        int font = stasis_load_font(STASIS_TEST_FONT_PATH, size);
        CHECK(font > 0 && font != previous_font);
        stasis_gfx_release_font(font);
        CHECK(stasis_gfx_cache_text(font, "released resize font") == 0);
        previous_font = font;
    }

    int stale_reset_font = stasis_load_font(STASIS_TEST_FONT_PATH, 69);
    int stale_reset_run = stasis_gfx_cache_text(stale_reset_font, "stale across reset");
    CHECK(stale_reset_font > 0 && stale_reset_run > 0);
    stasis_shutdown();
    CHECK(stasis_init_window(64, 64, "stasis_font_cache_test_reset"));
    int reset_replacement = stasis_load_font(STASIS_TEST_FONT_PATH, 69);
    CHECK(reset_replacement > 0 && reset_replacement != stale_reset_font);
    CHECK(stasis_gfx_measure_text_cached(stale_reset_run) == 0.0f);
    stasis_gfx_release_font(reset_replacement);

    test_density_rebuild_failure_retries();
    test_failed_immutable_cache_rolls_back_bytes();

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
    int replacement_run = stasis_gfx_cache_text(replacement_handle, "retained after failed reload");
    CHECK(replacement_run > 0);
    float replacement_width = stasis_gfx_measure_text_cached(replacement_run);
    float replacement_height = stasis_gfx_measure_text_cached_height(replacement_run);
    CHECK(replacement_width > 0.0f && replacement_height > 0.0f);

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
    CHECK(stasis_gfx_cache_text(replacement_handle, "retained after failed reload") == replacement_run);
    CHECK(stasis_gfx_measure_text_cached(replacement_run) == replacement_width);
    CHECK(stasis_gfx_measure_text_cached_height(replacement_run) == replacement_height);
    /* A failed acquisition must not retain the old font or consume its owner. */
    stasis_gfx_release_font(replacement_handle);
    CHECK(stasis_gfx_cache_text(replacement_handle, "released after failed reload") == 0);
    CHECK(stasis_gfx_measure_text_cached(replacement_run) == 0.0f);
    free(replacement);

    stasis_shutdown();
    puts("stasis_font_cache_test: ok");
    return 0;
}
