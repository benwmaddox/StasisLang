/*
 * stasis_runner.c
 *
 * High-level job:
 * - Load a compiled Stasis program as a shared library (DLL/.so).
 * - Call its entrypoint (`<module>__main`), then repeatedly call `<module>__tick`.
 * - Optionally support hot-swap: watch a small "swap file" that names a replacement library
 *   and (optionally) a new state-map. When a swap is requested, migrate global state
 *   between the old and new libraries and continue ticking without restarting the process.
 * - Optionally support data binding: bind a JSON config file to exported globals in the
 *   currently-loaded library (updated after each swap).
 *
 * Two runner backends (same control flow, different platform calls):
 * - Windows: LoadLibrary / GetProcAddress / FreeLibrary
 * - Linux/macOS: dlopen / dlsym / dlclose
 *
 * Hot-swap model (disk/DLL path):
 * - The compiler emits a state map (name + size per persisted symbol).
 * - On swap, we copy bytes of each symbol from the old library into a buffer, then copy
 *   those bytes into the new library's symbols (by name). Missing symbols are tolerated
 *   and reported.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <inttypes.h>
#include <time.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#include <errno.h>
#include <unistd.h>
#endif

#include "stasis_data.h"
#include "stasis_render_contract.h"

typedef int (*stasis_entry_fn)(void);
typedef int (*stasis_tick_fn)(void);
typedef void (*stasis_aot_bind_runtime_globals_fn)(void);
typedef void (*stasis_sys_set_args_fn)(int argc, const char *const *argv);
typedef void (*stasis_host_get_frame_fn)(int32_t *out_i32, float *out_f32);
typedef void (*stasis_gfx_submit_u8_fn)(const int32_t *cmd_i32, const float *cmd_f32, const uint8_t *cmd_u8);
typedef void (*stasis_host_bulk_init_fn)(const int32_t *host_req_seq);
typedef void (*stasis_host_bulk_apply_requests_fn)(
    const int32_t *host_req_seq,
    const int32_t *host_req_flags,
    const int32_t *host_req_window_w_px,
    const int32_t *host_req_window_h_px);
typedef void (*stasis_host_set_performance_metrics_fn)(uint64_t tick_us, uint64_t render_us);
typedef int (*stasis_host_bulk_step_fn)(
    int32_t *host_i32,
    float *host_f32,
    int32_t *gfx_cmd_i32,
    float *gfx_cmd_f32,
    uint8_t *gfx_cmd_u8,
    const int32_t *host_req_seq,
    const int32_t *host_req_flags,
    const int32_t *host_req_window_w_px,
    const int32_t *host_req_window_h_px,
    stasis_tick_fn tick_fn);
typedef int (*stasis_init_window_fn)(int width, int height, const char *title);
typedef int (*stasis_set_fullscreen_fn)(int enabled);
typedef void (*stasis_set_window_size_fn)(int width, int height);
typedef int (*stasis_graphics_runtime_abi_version_fn)(void);
typedef int (*stasis_graphics_set_asset_root_fn)(const char *path);

static int stasis_env_flag(const char *name, int default_value)
{
    const char *v = getenv(name);
    if (!v || !v[0])
    {
        return default_value;
    }
    if (v[0] == '0')
    {
        return 0;
    }
    if (v[0] == '1')
    {
        return 1;
    }
    return default_value;
}

#ifndef _WIN32
static void stasis_sleep_us(long long usec)
{
    if (usec <= 0)
    {
        return;
    }
    struct timespec req;
    req.tv_sec = (time_t)(usec / 1000000LL);
    req.tv_nsec = (long)((usec % 1000000LL) * 1000LL);
    while (nanosleep(&req, &req) != 0 && errno == EINTR)
    {
        /* retry */
    }
}

static int stasis_rebind_bulk_pointers_linux(
    void *lib,
    stasis_host_bulk_init_fn host_bulk_init,
    stasis_host_bulk_step_fn host_bulk_step,
    stasis_host_get_frame_fn host_get_frame,
    stasis_gfx_submit_u8_fn gfx_submit_u8,
    int32_t **host_req_seq,
    int32_t **host_req_flags,
    int32_t **host_req_window_w_px,
    int32_t **host_req_window_h_px,
    int32_t **host_i32,
    float **host_f32,
    int32_t **gfx_cmd_i32,
    float **gfx_cmd_f32,
    uint8_t **gfx_cmd_u8,
    int32_t *last_req_seq)
{
    if (!lib)
    {
        return 0;
    }

    if (host_req_seq)
    {
        *host_req_seq = (int32_t *)dlsym(lib, "host_req_seq");
    }
    if (host_req_flags)
    {
        *host_req_flags = (int32_t *)dlsym(lib, "host_req_flags");
    }
    if (host_req_window_w_px)
    {
        *host_req_window_w_px = (int32_t *)dlsym(lib, "host_req_window_w_px");
    }
    if (host_req_window_h_px)
    {
        *host_req_window_h_px = (int32_t *)dlsym(lib, "host_req_window_h_px");
    }

    if (host_i32)
    {
        *host_i32 = (int32_t *)dlsym(lib, "host_i32");
    }
    if (host_f32)
    {
        *host_f32 = (float *)dlsym(lib, "host_f32");
    }
    if (gfx_cmd_i32)
    {
        *gfx_cmd_i32 = (int32_t *)dlsym(lib, "gfx_cmd_i32");
    }
    if (gfx_cmd_f32)
    {
        *gfx_cmd_f32 = (float *)dlsym(lib, "gfx_cmd_f32");
    }
    if (gfx_cmd_u8)
    {
        *gfx_cmd_u8 = (uint8_t *)dlsym(lib, "gfx_cmd_u8");
    }

    if (host_bulk_init && host_req_seq && *host_req_seq)
    {
        host_bulk_init(*host_req_seq);
    }

    if (last_req_seq && host_req_seq)
    {
        *last_req_seq = *host_req_seq ? **host_req_seq : 0;
    }

    const int have_host_frame = host_i32 && host_f32 &&
        *host_i32 && *host_f32 && host_get_frame;
    const int have_bulk_step = host_i32 && host_f32 && gfx_cmd_i32 && gfx_cmd_f32 && gfx_cmd_u8 &&
        *host_i32 && *host_f32 && *gfx_cmd_i32 && *gfx_cmd_f32 && *gfx_cmd_u8 &&
        host_bulk_step;
    if (have_host_frame || have_bulk_step)
    {
        return 1;
    }

    return 0;
}
#endif

typedef struct stasis_state_symbol
{
    char *name;
    uint32_t size;
    uint32_t offset;
} stasis_state_symbol;

#ifdef _WIN32
typedef struct stasis_hot_exit_args
{
    HMODULE lib;
    const char *state_path;
    uint64_t map_hash;
    stasis_state_symbol *syms;
    uint32_t sym_count;
    uint32_t total_bytes;
    const char *hot_exit_path;
} stasis_hot_exit_args;
#endif

static char *stasis_strdup(const char *s)
{
    size_t len = strlen(s);
    char *out = (char *)malloc(len + 1);
    if (!out)
    {
        return NULL;
    }
    memcpy(out, s, len);
    out[len] = '\0';
    return out;
}

static void print_usage(void)
{
    fprintf(stderr, "usage: stasis_runner <dll_path> [entry]\n");
    fprintf(stderr, "  entry defaults to run_tests (use main for run mode)\n");
    fprintf(stderr, "usage: stasis_runner --server\n");
    fprintf(stderr, "usage: stasis_runner <dll_path> [entry] --state-map <map_path> [--state <snapshot_path>] [--hot-exit-file <path>]\n");
    fprintf(stderr, "       --data-bind <json_path> <struct_meta_path>  Register data hot-reload binding\n");
}

#ifdef _WIN32
static void enable_vt_processing(HANDLE handle)
{
    DWORD mode = 0;
    if (!GetConsoleMode(handle, &mode))
    {
        return;
    }
    mode |= ENABLE_VIRTUAL_TERMINAL_PROCESSING;
    SetConsoleMode(handle, mode);
}
#endif

static void set_runtime_dir(const char *dll_path)
{
    char dir_buffer[1024];
    size_t len = strlen(dll_path);
    if (len >= sizeof(dir_buffer))
    {
        return;
    }

    strncpy(dir_buffer, dll_path, sizeof(dir_buffer) - 1);
    dir_buffer[sizeof(dir_buffer) - 1] = '\0';

    char *slash = strrchr(dir_buffer, '\\');
    char *fslash = strrchr(dir_buffer, '/');
    char *sep = slash > fslash ? slash : fslash;
    if (!sep)
    {
        return;
    }

    *sep = '\0';

#ifdef _WIN32
    SetDllDirectoryA(dir_buffer);
    SetCurrentDirectoryA(dir_buffer);
#else
    chdir(dir_buffer);
#endif
}

static char *stasis_trim_ascii(char *text)
{
    while (*text == ' ' || *text == '\t' || *text == '\r' || *text == '\n')
    {
        text++;
    }
    char *end = text + strlen(text);
    while (end > text && (end[-1] == ' ' || end[-1] == '\t' || end[-1] == '\r' || end[-1] == '\n'))
    {
        end--;
    }
    *end = '\0';
    return text;
}

static int stasis_try_get_self_path(const char *argv0, char *out, size_t out_cap)
{
    if (!out || out_cap == 0)
    {
        return 0;
    }
#ifdef _WIN32
    DWORD written = GetModuleFileNameA(NULL, out, (DWORD)out_cap);
    if (written == 0 || written >= out_cap)
    {
        return 0;
    }
    return 1;
#else
    if (!argv0 || !argv0[0])
    {
        return 0;
    }
    strncpy(out, argv0, out_cap - 1);
    out[out_cap - 1] = '\0';
    return 1;
#endif
}

static int stasis_extract_dir(const char *path, char *out, size_t out_cap)
{
    if (!path || !out || out_cap == 0)
    {
        return 0;
    }
    size_t len = strlen(path);
    if (len >= out_cap)
    {
        return 0;
    }
    strncpy(out, path, out_cap - 1);
    out[out_cap - 1] = '\0';

#ifdef _WIN32
    if (strncmp(out, "\\\\?\\UNC\\", 8) == 0)
    {
        size_t remainder = strlen(out + 8);
        memmove(out + 2, out + 8, remainder + 1);
        out[0] = '\\';
        out[1] = '\\';
    }
    else if (strncmp(out, "\\\\?\\", 4) == 0)
    {
        memmove(out, out + 4, strlen(out + 4) + 1);
    }
#endif

    char *slash = strrchr(out, '\\');
    char *fslash = strrchr(out, '/');
    char *sep = slash > fslash ? slash : fslash;
    if (!sep)
    {
        return 0;
    }
    *sep = '\0';
    return 1;
}

static int stasis_set_current_dir(const char *path)
{
    if (!path || !path[0])
    {
        return 0;
    }
#ifdef _WIN32
    return SetCurrentDirectoryA(path) != 0;
#else
    return chdir(path) == 0;
#endif
}

static int stasis_set_asset_root(const char *path)
{
    if (!path || !path[0])
    {
        return 0;
    }
#ifdef _WIN32
    return _putenv_s("STASIS_ASSET_ROOT", path) == 0;
#else
    return setenv("STASIS_ASSET_ROOT", path, 1) == 0;
#endif
}

static int stasis_set_packaged_graphics_path(const char *exe_dir)
{
    char path[2080];
#ifdef _WIN32
    if (!exe_dir || !exe_dir[0] ||
        snprintf(path, sizeof(path), "%s\\stasis_graphics.dll", exe_dir) >= (int)sizeof(path))
    {
        return 0;
    }
    return SetEnvironmentVariableA("STASIS_RUNTIME_DLL_PATH", path) != 0 &&
           _putenv_s("STASIS_RUNTIME_DLL_PATH", path) == 0;
#else
#if defined(__APPLE__)
    const char *runtime_name = "libstasis_graphics.dylib";
#else
    const char *runtime_name = "libstasis_graphics.so";
#endif
    if (!exe_dir || !exe_dir[0] ||
        snprintf(path, sizeof(path), "%s/%s", exe_dir, runtime_name) >= (int)sizeof(path))
    {
        return 0;
    }
    return setenv("STASIS_RUNTIME_LIBRARY_PATH", path, 1) == 0;
#endif
}

static int stasis_try_load_launch_config(
    const char *argv0,
    char *dll_out,
    size_t dll_out_cap,
    char *entry_out,
    size_t entry_out_cap,
    char *tick_out,
    size_t tick_out_cap,
    char *render_out,
    size_t render_out_cap,
    char *data_json_out,
    size_t data_json_out_cap,
    char *data_meta_out,
    size_t data_meta_out_cap,
    int *fps_out)
{
    char self_path[2048];
    char launch_path[2080];
    char exe_dir[2048];
#ifdef _WIN32
    char payload_dir[2080];
#endif
    char line[2048];

    if (!stasis_try_get_self_path(argv0, self_path, sizeof(self_path)))
    {
        return 0;
    }
    if (!stasis_extract_dir(self_path, exe_dir, sizeof(exe_dir)))
    {
        return 0;
    }
    if (snprintf(launch_path, sizeof(launch_path), "%s.launch", self_path) >= (int)sizeof(launch_path))
    {
        return 0;
    }

    FILE *file = fopen(launch_path, "rb");
#ifdef _WIN32
    if (!file)
    {
        const char *exe_name = strrchr(self_path, '\\');
        const char *forward_name = strrchr(self_path, '/');
        if (!exe_name || (forward_name && forward_name > exe_name))
        {
            exe_name = forward_name;
        }
        exe_name = exe_name ? exe_name + 1 : self_path;
        int payload_written = snprintf(payload_dir, sizeof(payload_dir), "%s\\app", exe_dir);
        int launch_written = payload_written > 0 && payload_written < (int)sizeof(payload_dir)
            ? snprintf(launch_path, sizeof(launch_path), "%s\\%s.launch", payload_dir, exe_name)
            : -1;
        if (launch_written > 0 && launch_written < (int)sizeof(launch_path))
        {
            file = fopen(launch_path, "rb");
            if (file)
            {
                strncpy(exe_dir, payload_dir, sizeof(exe_dir) - 1);
                exe_dir[sizeof(exe_dir) - 1] = '\0';
            }
        }
    }
#endif
    if (!file)
    {
        return 0;
    }

    if (dll_out && dll_out_cap > 0)
    {
        dll_out[0] = '\0';
    }
    if (entry_out && entry_out_cap > 0)
    {
        entry_out[0] = '\0';
    }
    if (tick_out && tick_out_cap > 0)
    {
        tick_out[0] = '\0';
    }
    if (render_out && render_out_cap > 0)
    {
        render_out[0] = '\0';
    }
    if (data_json_out && data_json_out_cap > 0)
    {
        data_json_out[0] = '\0';
    }
    if (data_meta_out && data_meta_out_cap > 0)
    {
        data_meta_out[0] = '\0';
    }
    if (fps_out)
    {
        *fps_out = 60;
    }

    while (fgets(line, sizeof(line), file))
    {
        char *trimmed = stasis_trim_ascii(line);
        if (!trimmed[0] || trimmed[0] == '#')
        {
            continue;
        }
        char *eq = strchr(trimmed, '=');
        if (!eq)
        {
            continue;
        }
        *eq = '\0';
        char *key = stasis_trim_ascii(trimmed);
        char *value = stasis_trim_ascii(eq + 1);

        if (strcmp(key, "dll") == 0 && dll_out && dll_out_cap > 0)
        {
            strncpy(dll_out, value, dll_out_cap - 1);
            dll_out[dll_out_cap - 1] = '\0';
            continue;
        }
        if (strcmp(key, "entry") == 0 && entry_out && entry_out_cap > 0)
        {
            strncpy(entry_out, value, entry_out_cap - 1);
            entry_out[entry_out_cap - 1] = '\0';
            continue;
        }
        if (strcmp(key, "tick") == 0 && tick_out && tick_out_cap > 0)
        {
            strncpy(tick_out, value, tick_out_cap - 1);
            tick_out[tick_out_cap - 1] = '\0';
            continue;
        }
        if (strcmp(key, "render") == 0 && render_out && render_out_cap > 0)
        {
            strncpy(render_out, value, render_out_cap - 1);
            render_out[render_out_cap - 1] = '\0';
            continue;
        }
        if (strcmp(key, "data_bind_json") == 0 && data_json_out && data_json_out_cap > 0)
        {
            strncpy(data_json_out, value, data_json_out_cap - 1);
            data_json_out[data_json_out_cap - 1] = '\0';
            continue;
        }
        if (strcmp(key, "data_bind_meta") == 0 && data_meta_out && data_meta_out_cap > 0)
        {
            strncpy(data_meta_out, value, data_meta_out_cap - 1);
            data_meta_out[data_meta_out_cap - 1] = '\0';
            continue;
        }
        if (strcmp(key, "fps") == 0 && fps_out)
        {
            int value_i32 = atoi(value);
            if (value_i32 >= 1 && value_i32 <= 240)
            {
                *fps_out = value_i32;
            }
            continue;
        }
    }
    fclose(file);

    if (!dll_out || !dll_out[0])
    {
        return 0;
    }
    if (!entry_out || !entry_out[0])
    {
        strncpy(entry_out, "main", entry_out_cap - 1);
        entry_out[entry_out_cap - 1] = '\0';
    }

#ifdef _WIN32
    if (dll_out && dll_out[0] && dll_out[0] != '\\' && dll_out[0] != '/' && dll_out[1] != ':')
    {
        char resolved_dll[4096];
        int resolved_written = snprintf(
            resolved_dll, sizeof(resolved_dll), "%s\\%s", exe_dir, dll_out);
        if (resolved_written <= 0 || resolved_written >= (int)sizeof(resolved_dll) ||
            (size_t)resolved_written >= dll_out_cap)
        {
            fprintf(stderr, "error: generated launcher DLL path is too long\n");
            return 0;
        }
        memcpy(dll_out, resolved_dll, (size_t)resolved_written + 1);
    }
#else
    /* dlopen does not search the current directory for a bare library name. */
    if (dll_out[0] != '/')
    {
        char absolute_dll[4096];
        int written = snprintf(absolute_dll, sizeof(absolute_dll), "%s/%s", exe_dir, dll_out);
        if (written < 0 || (size_t)written >= sizeof(absolute_dll) ||
            (size_t)written >= dll_out_cap)
        {
            return 0;
        }
        memcpy(dll_out, absolute_dll, (size_t)written + 1);
    }
#endif

    /* Anchor assets to the resolved launch payload, independent of caller CWD. */
    if (!stasis_set_current_dir(exe_dir) ||
        !stasis_set_asset_root(exe_dir) ||
        !stasis_set_packaged_graphics_path(exe_dir))
    {
        fprintf(stderr, "error: failed to anchor generated launcher at %s\n", exe_dir);
        return 0;
    }
    return 1;
}

static void stasis_build_related_symbol_names(
    const char *entry_name,
    char *tick_name,
    size_t tick_name_cap,
    char *render_name,
    size_t render_name_cap)
{
    if (tick_name && tick_name_cap > 0)
    {
        tick_name[0] = '\0';
    }
    if (render_name && render_name_cap > 0)
    {
        render_name[0] = '\0';
    }
    if (!entry_name || !entry_name[0])
    {
        return;
    }

    const char *sep = strstr(entry_name, "__");
    if (sep)
    {
        size_t prefix_len = (size_t)(sep - entry_name) + 2;
        if (tick_name && prefix_len + 4 < tick_name_cap)
        {
            memcpy(tick_name, entry_name, prefix_len);
            memcpy(tick_name + prefix_len, "tick", 5);
        }
        if (render_name && prefix_len + 6 < render_name_cap)
        {
            memcpy(render_name, entry_name, prefix_len);
            memcpy(render_name + prefix_len, "render", 7);
        }
        return;
    }

    if (tick_name && tick_name_cap >= 5)
    {
        memcpy(tick_name, "tick", 5);
    }
    if (render_name && render_name_cap >= 7)
    {
        memcpy(render_name, "render", 7);
    }
}

#ifdef _WIN32
static int file_exists(const char *path);

typedef BOOL(WINAPI *stasis_SetDefaultDllDirectoriesFn)(DWORD);
typedef DLL_DIRECTORY_COOKIE(WINAPI *stasis_AddDllDirectoryFn)(PCWSTR);

static int stasis_path_dirname(const char *path, char *out, size_t out_cap)
{
    size_t len = strlen(path);
    if (len == 0 || len + 1 > out_cap)
    {
        return 1;
    }

    memcpy(out, path, len + 1);

    char *slash = strrchr(out, '\\');
    char *fslash = strrchr(out, '/');
    char *sep = slash > fslash ? slash : fslash;
    if (!sep)
    {
        return 1;
    }

    *sep = '\0';
    return out[0] == '\0' ? 1 : 0;
}

static int stasis_add_dll_directory_utf8(stasis_AddDllDirectoryFn add_dir, const char *utf8_path)
{
    wchar_t wide[1024];
    int got = MultiByteToWideChar(CP_UTF8, 0, utf8_path, -1, wide, (int)(sizeof(wide) / sizeof(wide[0])));
    if (got == 0)
    {
        got = MultiByteToWideChar(CP_ACP, 0, utf8_path, -1, wide, (int)(sizeof(wide) / sizeof(wide[0])));
        if (got == 0)
        {
            return 1;
        }
    }

    return add_dir(wide) ? 0 : 1;
}

static void stasis_try_add_relative_runtime_dir(const char *base_dir, const char *rel, char *out_dir, size_t out_cap)
{
    if (out_dir[0] != '\0')
    {
        return;
    }

    char probe_dir[1024];
    char probe_dll[1024];
    probe_dir[0] = '\0';
    probe_dll[0] = '\0';

    size_t base_len = strlen(base_dir);
    size_t rel_len = strlen(rel);
    if (base_len + 1 + rel_len + 1 >= sizeof(probe_dir))
    {
        return;
    }

    memcpy(probe_dir, base_dir, base_len);
    probe_dir[base_len] = '\\';
    memcpy(probe_dir + base_len + 1, rel, rel_len);
    probe_dir[base_len + 1 + rel_len] = '\0';

    size_t dir_len = strlen(probe_dir);
    const char *dll_name = "stasis_graphics.dll";
    size_t dll_len = strlen(dll_name);
    if (dir_len + 1 + dll_len + 1 >= sizeof(probe_dll))
    {
        return;
    }

    memcpy(probe_dll, probe_dir, dir_len);
    probe_dll[dir_len] = '\\';
    memcpy(probe_dll + dir_len + 1, dll_name, dll_len);
    probe_dll[dir_len + 1 + dll_len] = '\0';

    if (file_exists(probe_dll))
    {
        strncpy(out_dir, probe_dir, out_cap - 1);
        out_dir[out_cap - 1] = '\0';
    }
}

static void stasis_setup_runtime_dirs(const char *exe_path, const char *dll_path, char *out_runtime_dir, size_t out_cap)
{
    out_runtime_dir[0] = '\0';

    char program_dir[1024];
    program_dir[0] = '\0';
    if (dll_path && stasis_path_dirname(dll_path, program_dir, sizeof(program_dir)) == 0)
    {
        char runtime_probe[1024];
        int probe_written = snprintf(
            runtime_probe, sizeof(runtime_probe), "%s\\stasis_graphics.dll", program_dir);
        if (probe_written > 0 && probe_written < (int)sizeof(runtime_probe) &&
            file_exists(runtime_probe))
        {
            strncpy(out_runtime_dir, program_dir, out_cap - 1);
            out_runtime_dir[out_cap - 1] = '\0';
            return;
        }
    }

    char exe_dir[1024];
    exe_dir[0] = '\0';
    if (exe_path && stasis_path_dirname(exe_path, exe_dir, sizeof(exe_dir)) == 0)
    {
        char probe[1024];
        probe[0] = '\0';

        size_t dir_len = strlen(exe_dir);
        const char *dll_name = "stasis_graphics.dll";
        size_t dll_len = strlen(dll_name);
        if (dir_len + 1 + dll_len + 1 < sizeof(probe))
        {
            memcpy(probe, exe_dir, dir_len);
            probe[dir_len] = '\\';
            memcpy(probe + dir_len + 1, dll_name, dll_len);
            probe[dir_len + 1 + dll_len] = '\0';
            if (file_exists(probe))
            {
                strncpy(out_runtime_dir, exe_dir, out_cap - 1);
                out_runtime_dir[out_cap - 1] = '\0';
                return;
            }
        }

        /* Common layouts when the runner is copied to repo root or build/. */
        stasis_try_add_relative_runtime_dir(exe_dir, "runtime\\build\\bin\\Release", out_runtime_dir, out_cap);
        stasis_try_add_relative_runtime_dir(exe_dir, "..\\runtime\\build\\bin\\Release", out_runtime_dir, out_cap);
    }
}

static void stasis_enable_dll_search(const char *exe_path, const char *dll_path)
{
    char dll_dir[1024];
    dll_dir[0] = '\0';
    if (dll_path)
    {
        (void)stasis_path_dirname(dll_path, dll_dir, sizeof(dll_dir));
    }

    char runtime_dir[1024];
    stasis_setup_runtime_dirs(exe_path, dll_path, runtime_dir, sizeof(runtime_dir));

    if (dll_dir[0] != '\0')
    {
        /* Keep program-relative file IO working as before. */
        SetCurrentDirectoryA(dll_dir);
    }

    HMODULE k32 = GetModuleHandleA("kernel32.dll");
    stasis_SetDefaultDllDirectoriesFn set_default = NULL;
    stasis_AddDllDirectoryFn add_dir = NULL;
    if (k32)
    {
        set_default = (stasis_SetDefaultDllDirectoriesFn)GetProcAddress(k32, "SetDefaultDllDirectories");
        add_dir = (stasis_AddDllDirectoryFn)GetProcAddress(k32, "AddDllDirectory");
    }

    if (set_default && add_dir)
    {
        set_default(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS);
        if (dll_dir[0] != '\0')
        {
            (void)stasis_add_dll_directory_utf8(add_dir, dll_dir);
        }
        if (runtime_dir[0] != '\0')
        {
            (void)stasis_add_dll_directory_utf8(add_dir, runtime_dir);
        }
    }
    else
    {
        /* Fallback: app directory is still searched; add program DLL directory as well. */
        if (dll_dir[0] != '\0')
        {
            SetDllDirectoryA(dll_dir);
        }
    }

    /* Optional preload: keep runtime DLLs resident across swaps. */
    if (runtime_dir[0] != '\0')
    {
        char gfx_path[1024];
        gfx_path[0] = '\0';
        size_t dir_len = strlen(runtime_dir);
        const char *dll_name = "stasis_graphics.dll";
        size_t dll_len = strlen(dll_name);
        if (dir_len + 1 + dll_len + 1 < sizeof(gfx_path))
        {
            memcpy(gfx_path, runtime_dir, dir_len);
            gfx_path[dir_len] = '\\';
            memcpy(gfx_path + dir_len + 1, dll_name, dll_len);
            gfx_path[dir_len + 1 + dll_len] = '\0';
            if (file_exists(gfx_path))
            {
                (void)LoadLibraryA(gfx_path);
            }
        }
    }
}

static HMODULE stasis_load_program_library(const char *path)
{
    HMODULE lib = LoadLibraryExA(path,
                                 NULL,
                                 LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR);
    if (!lib)
    {
        lib = LoadLibraryA(path);
    }
    return lib;
}
#endif

#ifdef _WIN32
static void stasis_try_set_sys_args(HMODULE lib, int argc, char **argv)
{
    FARPROC sym = GetProcAddress(lib, "stasis_sys_set_args");
    if (sym)
    {
        ((stasis_sys_set_args_fn)sym)(argc, (const char *const *)argv);
    }
}
#else
static void stasis_try_set_sys_args(void *lib, int argc, char **argv)
{
    void *sym = dlsym(lib, "stasis_sys_set_args");
    if (sym)
    {
        ((stasis_sys_set_args_fn)sym)(argc, (const char *const *)argv);
    }
}
#endif

static int read_state_map(const char *path, uint64_t *out_hash, stasis_state_symbol **out_syms, uint32_t *out_count, uint32_t *out_total)
{
    FILE *f = fopen(path, "rb");
    if (!f)
    {
        fprintf(stderr, "error: failed to open state map: %s\n", path);
        return 1;
    }

    char line[4096];
    if (!fgets(line, (int)sizeof(line), f))
    {
        fclose(f);
        fprintf(stderr, "error: empty state map: %s\n", path);
        return 1;
    }

    if (strncmp(line, "STASIS_STATE_MAP 1", 18) != 0)
    {
        fclose(f);
        fprintf(stderr, "error: invalid state map header: %s\n", path);
        return 1;
    }

    if (!fgets(line, (int)sizeof(line), f))
    {
        fclose(f);
        fprintf(stderr, "error: missing state map metadata: %s\n", path);
        return 1;
    }

    unsigned long long hash = 0;
    unsigned long count = 0;
    unsigned long total = 0;
    if (sscanf(line, "hash=%llx count=%lu bytes=%lu", &hash, &count, &total) != 3 || count == 0)
    {
        fclose(f);
        fprintf(stderr, "error: invalid state map metadata: %s\n", path);
        return 1;
    }

    stasis_state_symbol *syms = (stasis_state_symbol *)calloc(count, sizeof(stasis_state_symbol));
    if (!syms)
    {
        fclose(f);
        fprintf(stderr, "error: out of memory\n");
        return 1;
    }

    uint32_t offset = 0;
    for (unsigned long i = 0; i < count; i++)
    {
        if (!fgets(line, (int)sizeof(line), f))
        {
            fclose(f);
            fprintf(stderr, "error: truncated state map: %s\n", path);
            for (unsigned long j = 0; j < i; j++)
            {
                free(syms[j].name);
            }
            free(syms);
            return 1;
        }

        char name[2048];
        unsigned long size = 0;
        if (sscanf(line, "%2047s %lu", name, &size) != 2 || size == 0)
        {
            fclose(f);
            fprintf(stderr, "error: invalid state map entry: %s\n", path);
            for (unsigned long j = 0; j < i; j++)
            {
                free(syms[j].name);
            }
            free(syms);
            return 1;
        }

        syms[i].name = stasis_strdup(name);
        if (!syms[i].name)
        {
            fclose(f);
            fprintf(stderr, "error: out of memory\n");
            for (unsigned long j = 0; j < i; j++)
            {
                free(syms[j].name);
            }
            free(syms);
            return 1;
        }

        syms[i].size = (uint32_t)size;
        syms[i].offset = offset;
        offset += (uint32_t)size;
    }

    fclose(f);
    *out_hash = (uint64_t)hash;
    *out_syms = syms;
    *out_count = (uint32_t)count;
    *out_total = (uint32_t)total;
    return 0;
}

/* Free a state map parsed by read_state_map(). Safe to call on NULL. */
static void free_state_map(stasis_state_symbol **syms, uint32_t *sym_count)
{
    if (!syms || !*syms || !sym_count)
    {
        return;
    }
    for (uint32_t i = 0; i < *sym_count; i++)
    {
        free((*syms)[i].name);
    }
    free(*syms);
    *syms = NULL;
    *sym_count = 0;
}

static int load_state_snapshot(const char *path, uint64_t expected_hash, uint32_t expected_bytes, uint8_t **out_data)
{
    FILE *f = fopen(path, "rb");
    if (!f)
    {
        return 2; /* missing is not an error */
    }

    char header[256];
    if (!fgets(header, (int)sizeof(header), f))
    {
        fclose(f);
        fprintf(stderr, "error: empty state snapshot: %s\n", path);
        return 1;
    }

    unsigned long long hash = 0;
    unsigned long bytes = 0;
    if (sscanf(header, "STASIS_STATE_SNAP 1 hash=%llx bytes=%lu", &hash, &bytes) != 2)
    {
        fclose(f);
        fprintf(stderr, "error: invalid state snapshot header: %s\n", path);
        return 1;
    }

    if ((uint64_t)hash != expected_hash)
    {
        fclose(f);
        fprintf(stderr, "error: state snapshot hash mismatch (expected=%016llx got=%016llx)\n", (unsigned long long)expected_hash, hash);
        return 1;
    }

    if ((uint32_t)bytes != expected_bytes)
    {
        fclose(f);
        fprintf(stderr, "error: state snapshot size mismatch (expected=%u got=%lu)\n", expected_bytes, bytes);
        return 1;
    }

    uint8_t *data = (uint8_t *)malloc(expected_bytes);
    if (!data)
    {
        fclose(f);
        fprintf(stderr, "error: out of memory\n");
        return 1;
    }

    size_t got = fread(data, 1, expected_bytes, f);
    fclose(f);
    if (got != expected_bytes)
    {
        free(data);
        fprintf(stderr, "error: truncated state snapshot: %s\n", path);
        return 1;
    }

    *out_data = data;
    return 0;
}

static int save_state_snapshot(const char *path, uint64_t hash, const uint8_t *data, uint32_t bytes)
{
    FILE *f = fopen(path, "wb");
    if (!f)
    {
        fprintf(stderr, "error: failed to write state snapshot: %s\n", path);
        return 1;
    }

    fprintf(f, "STASIS_STATE_SNAP 1 hash=%016llx bytes=%u\n", (unsigned long long)hash, bytes);
    if (bytes > 0)
    {
        size_t wrote = fwrite(data, 1, bytes, f);
        if (wrote != bytes)
        {
            fclose(f);
            fprintf(stderr, "error: failed to write state snapshot payload: %s\n", path);
            return 1;
        }
    }

    fclose(f);
    return 0;
}

#ifdef _WIN32
static int file_exists(const char *path)
{
    DWORD attr = GetFileAttributesA(path);
    return attr != INVALID_FILE_ATTRIBUTES && (attr & FILE_ATTRIBUTE_DIRECTORY) == 0;
}

static int move_file_replace_retry(const char *src, const char *dst, int attempts, int sleep_ms)
{
    for (int i = 0; i < attempts; i++)
    {
        if (MoveFileExA(src, dst, MOVEFILE_REPLACE_EXISTING | MOVEFILE_COPY_ALLOWED))
        {
            return 1;
        }

        DWORD err = GetLastError();
        if (err != ERROR_SHARING_VIOLATION && err != ERROR_ACCESS_DENIED)
        {
            return 0;
        }

        Sleep((DWORD)sleep_ms);
    }

    return 0;
}

static int read_text_file(const char *path, char *out, size_t out_cap)
{
    FILE *f = fopen(path, "rb");
    if (!f)
    {
        return 1;
    }

    size_t got = fread(out, 1, out_cap - 1, f);
    fclose(f);
    out[got] = '\0';

    /* trim whitespace */
    while (got > 0 && (out[got - 1] == '\n' || out[got - 1] == '\r' || out[got - 1] == ' ' || out[got - 1] == '\t'))
    {
        out[--got] = '\0';
    }
    size_t start = 0;
    while (out[start] == ' ' || out[start] == '\t')
    {
        start++;
    }
    if (start > 0)
    {
        memmove(out, out + start, strlen(out + start) + 1);
    }

    return out[0] == '\0' ? 1 : 0;
}

static int read_swap_file(const char *path, char *dll_out, size_t dll_cap, char *map_out, size_t map_cap)
{
    FILE *f = fopen(path, "rb");
    if (!f)
    {
        return 1;
    }

    if (!fgets(dll_out, (int)dll_cap, f))
    {
        fclose(f);
        return 1;
    }
    if (dll_out[0] == '\0')
    {
        fclose(f);
        return 1;
    }

    size_t dll_len = strlen(dll_out);
    while (dll_len > 0 && (dll_out[dll_len - 1] == '\n' || dll_out[dll_len - 1] == '\r' || dll_out[dll_len - 1] == ' ' || dll_out[dll_len - 1] == '\t'))
    {
        dll_out[--dll_len] = '\0';
    }
    size_t start = 0;
    while (dll_out[start] == ' ' || dll_out[start] == '\t')
    {
        start++;
    }
    if (start > 0)
    {
        memmove(dll_out, dll_out + start, strlen(dll_out + start) + 1);
    }

    if (map_out && map_cap > 0)
    {
        map_out[0] = '\0';
        if (fgets(map_out, (int)map_cap, f))
        {
            size_t map_len = strlen(map_out);
            while (map_len > 0 && (map_out[map_len - 1] == '\n' || map_out[map_len - 1] == '\r' || map_out[map_len - 1] == ' ' || map_out[map_len - 1] == '\t'))
            {
                map_out[--map_len] = '\0';
            }
            size_t map_start = 0;
            while (map_out[map_start] == ' ' || map_out[map_start] == '\t')
            {
                map_start++;
            }
            if (map_start > 0)
            {
                memmove(map_out, map_out + map_start, strlen(map_out + map_start) + 1);
            }
        }
    }

    fclose(f);
    return dll_out[0] == '\0' ? 1 : 0;
}

static int copy_state_to_buffer(HMODULE lib, stasis_state_symbol *syms, uint32_t sym_count, uint8_t *buffer, uint32_t total_bytes, int allow_missing, uint32_t *missing_count)
{
    (void)total_bytes;
    for (uint32_t i = 0; i < sym_count; i++)
    {
        FARPROC addr = GetProcAddress(lib, syms[i].name);
        if (!addr)
        {
            if (allow_missing)
            {
                if (missing_count)
                {
                    (*missing_count)++;
                }
                memset(buffer + syms[i].offset, 0, syms[i].size);
                continue;
            }
            fprintf(stderr, "error: state symbol not exported: %s\n", syms[i].name);
            return 1;
        }
        memcpy(buffer + syms[i].offset, (void *)addr, syms[i].size);
    }
    return 0;
}

static int copy_state_from_buffer(HMODULE lib, stasis_state_symbol *syms, uint32_t sym_count, const uint8_t *buffer, uint32_t total_bytes, int allow_missing, uint32_t *missing_count)
{
    (void)total_bytes;
    for (uint32_t i = 0; i < sym_count; i++)
    {
        FARPROC addr = GetProcAddress(lib, syms[i].name);
        if (!addr)
        {
            if (allow_missing)
            {
                if (missing_count)
                {
                    (*missing_count)++;
                }
                continue;
            }
            fprintf(stderr, "error: state symbol not exported: %s\n", syms[i].name);
            return 1;
        }
        memcpy((void *)addr, buffer + syms[i].offset, syms[i].size);
    }
    return 0;
}

static int try_make_fixed_swap_path(const char *input, char *out, size_t out_cap)
{
    const char *suffix_a = ".swapA.dll";
    const char *suffix_b = ".swapB.dll";
    const char *fixed = ".swap.dll";
    size_t len = strlen(input);
    size_t a_len = strlen(suffix_a);
    size_t b_len = strlen(suffix_b);

    if (len >= a_len && _stricmp(input + len - a_len, suffix_a) == 0)
    {
        size_t base_len = len - a_len;
        size_t fixed_len = strlen(fixed);
        if (base_len + fixed_len + 1 > out_cap)
        {
            return 0;
        }
        memcpy(out, input, base_len);
        memcpy(out + base_len, fixed, fixed_len + 1);
        return 1;
    }

    if (len >= b_len && _stricmp(input + len - b_len, suffix_b) == 0)
    {
        size_t base_len = len - b_len;
        size_t fixed_len = strlen(fixed);
        if (base_len + fixed_len + 1 > out_cap)
        {
            return 0;
        }
        memcpy(out, input, base_len);
        memcpy(out + base_len, fixed, fixed_len + 1);
        return 1;
    }

    return 0;
}

static void stasis_rebind_bulk_pointers(
    HMODULE dll,
    int *bulk_active,
    int32_t **host_i32,
    float **host_f32,
    int32_t **gfx_cmd_i32,
    float **gfx_cmd_f32,
    uint8_t **gfx_cmd_u8,
    stasis_host_get_frame_fn *host_get_frame,
    stasis_gfx_submit_u8_fn *gfx_submit_u8,
    stasis_host_bulk_init_fn *host_bulk_init,
    stasis_host_bulk_apply_requests_fn *host_bulk_apply_requests,
    stasis_host_bulk_step_fn *host_bulk_step,
    int32_t **host_req_seq,
    int32_t **host_req_flags,
    int32_t **host_req_window_w_px,
    int32_t **host_req_window_h_px,
    int32_t *last_req_seq)
{
    if (bulk_active)
    {
        *bulk_active = 0;
    }
    if (host_i32)
    {
        *host_i32 = NULL;
    }
    if (host_f32)
    {
        *host_f32 = NULL;
    }
    if (gfx_cmd_i32)
    {
        *gfx_cmd_i32 = NULL;
    }
    if (gfx_cmd_f32)
    {
        *gfx_cmd_f32 = NULL;
    }
    if (gfx_cmd_u8)
    {
        *gfx_cmd_u8 = NULL;
    }

    if (host_req_seq)
    {
        *host_req_seq = (int32_t *)GetProcAddress(dll, "host_req_seq");
    }
    if (host_req_flags)
    {
        *host_req_flags = (int32_t *)GetProcAddress(dll, "host_req_flags");
    }
    if (host_req_window_w_px)
    {
        *host_req_window_w_px = (int32_t *)GetProcAddress(dll, "host_req_window_w_px");
    }
    if (host_req_window_h_px)
    {
        *host_req_window_h_px = (int32_t *)GetProcAddress(dll, "host_req_window_h_px");
    }
    if (last_req_seq)
    {
        *last_req_seq = host_req_seq && *host_req_seq ? **host_req_seq : 0;
    }

    if (host_i32)
    {
        *host_i32 = (int32_t *)GetProcAddress(dll, "host_i32");
    }
    if (host_f32)
    {
        *host_f32 = (float *)GetProcAddress(dll, "host_f32");
    }
    if (gfx_cmd_i32)
    {
        *gfx_cmd_i32 = (int32_t *)GetProcAddress(dll, "gfx_cmd_i32");
    }
    if (gfx_cmd_f32)
    {
        *gfx_cmd_f32 = (float *)GetProcAddress(dll, "gfx_cmd_f32");
    }
    if (gfx_cmd_u8)
    {
        *gfx_cmd_u8 = (uint8_t *)GetProcAddress(dll, "gfx_cmd_u8");
    }

    HMODULE gfx = GetModuleHandleA("stasis_graphics.dll");
    if (gfx)
    {
        if (host_get_frame)
        {
            *host_get_frame = (stasis_host_get_frame_fn)GetProcAddress(gfx, "stasis_host_get_frame");
        }
        if (gfx_submit_u8)
        {
            *gfx_submit_u8 = (stasis_gfx_submit_u8_fn)GetProcAddress(gfx, "stasis_gfx_submit_u8");
        }
        if (host_bulk_init)
        {
            *host_bulk_init = (stasis_host_bulk_init_fn)GetProcAddress(gfx, "stasis_host_bulk_init");
        }
        if (host_bulk_apply_requests)
        {
            *host_bulk_apply_requests = (stasis_host_bulk_apply_requests_fn)GetProcAddress(gfx, "stasis_host_bulk_apply_requests");
        }
        if (host_bulk_step)
        {
            *host_bulk_step = (stasis_host_bulk_step_fn)GetProcAddress(gfx, "stasis_host_bulk_step");
        }
    }

    if (bulk_active)
    {
        const int have_host_frame = host_i32 && host_f32 &&
            *host_i32 && *host_f32 &&
            host_get_frame && *host_get_frame;
        const int have_bulk_step = host_i32 && host_f32 && gfx_cmd_i32 && gfx_cmd_f32 && gfx_cmd_u8 &&
            *host_i32 && *host_f32 &&
            *gfx_cmd_i32 && *gfx_cmd_f32 && *gfx_cmd_u8 &&
            host_bulk_step && *host_bulk_step;
        if (have_host_frame || have_bulk_step)
        {
            *bulk_active = 1;
        }
    }
}

static DWORD WINAPI hot_exit_thread(LPVOID user_data)
{
    stasis_hot_exit_args *args = (stasis_hot_exit_args *)user_data;
    if (!args || !args->hot_exit_path || args->hot_exit_path[0] == '\0')
    {
        return 0;
    }

    /* Clear stale triggers (best-effort). */
    DeleteFileA(args->hot_exit_path);

    for (;;)
    {
        DWORD attr = GetFileAttributesA(args->hot_exit_path);
        if (attr != INVALID_FILE_ATTRIBUTES)
        {
            DeleteFileA(args->hot_exit_path);

            uint8_t *save_data = (uint8_t *)malloc(args->total_bytes);
            if (!save_data)
            {
                fprintf(stderr, "error: out of memory\n");
                fflush(stderr);
                ExitProcess(1);
            }

            LARGE_INTEGER freq;
            LARGE_INTEGER t0;
            LARGE_INTEGER t1;
            QueryPerformanceFrequency(&freq);

            QueryPerformanceCounter(&t0);
            for (uint32_t i = 0; i < args->sym_count; i++)
            {
                FARPROC addr = GetProcAddress(args->lib, args->syms[i].name);
                if (!addr)
                {
                    fprintf(stderr, "error: state symbol not exported: %s\n", args->syms[i].name);
                    fflush(stderr);
                    ExitProcess(1);
                }
                memcpy(save_data + args->syms[i].offset, (void *)addr, args->syms[i].size);
            }
            QueryPerformanceCounter(&t1);
            long long save_copy_us = (t1.QuadPart - t0.QuadPart) * 1000000LL / freq.QuadPart;

            QueryPerformanceCounter(&t0);
            int save_result = save_state_snapshot(args->state_path, args->map_hash, save_data, args->total_bytes);
            QueryPerformanceCounter(&t1);
            long long save_io_us = (t1.QuadPart - t0.QuadPart) * 1000000LL / freq.QuadPart;
            if (save_result != 0)
            {
                fflush(stderr);
                ExitProcess(1);
            }

            fprintf(stderr, "HOTSTATE save: io=%lldus copy=%lldus bytes=%u symbols=%u (hot-exit)\n", save_io_us, save_copy_us, args->total_bytes, args->sym_count);
            fflush(stderr);
            ExitProcess(0);
        }

        Sleep(10);
    }
}
#else
static int file_exists(const char *path)
{
    if (!path || path[0] == '\0')
    {
        return 0;
    }

    return access(path, F_OK) == 0;
}

static int read_swap_file(const char *path, char *dll_out, size_t dll_cap, char *map_out, size_t map_cap)
{
    FILE *f = fopen(path, "rb");
    if (!f)
    {
        return 1;
    }

    if (!fgets(dll_out, (int)dll_cap, f))
    {
        fclose(f);
        return 1;
    }
    if (dll_out[0] == '\0')
    {
        fclose(f);
        return 1;
    }

    size_t dll_len = strlen(dll_out);
    while (dll_len > 0 && (dll_out[dll_len - 1] == '\n' || dll_out[dll_len - 1] == '\r' || dll_out[dll_len - 1] == ' ' || dll_out[dll_len - 1] == '\t'))
    {
        dll_out[--dll_len] = '\0';
    }
    size_t start = 0;
    while (dll_out[start] == ' ' || dll_out[start] == '\t')
    {
        start++;
    }
    if (start > 0)
    {
        memmove(dll_out, dll_out + start, strlen(dll_out + start) + 1);
    }

    if (map_out && map_cap > 0)
    {
        map_out[0] = '\0';
        if (fgets(map_out, (int)map_cap, f))
        {
            size_t map_len = strlen(map_out);
            while (map_len > 0 && (map_out[map_len - 1] == '\n' || map_out[map_len - 1] == '\r' || map_out[map_len - 1] == ' ' || map_out[map_len - 1] == '\t'))
            {
                map_out[--map_len] = '\0';
            }
            size_t map_start = 0;
            while (map_out[map_start] == ' ' || map_out[map_start] == '\t')
            {
                map_start++;
            }
            if (map_start > 0)
            {
                memmove(map_out, map_out + map_start, strlen(map_out + map_start) + 1);
            }
        }
    }

    fclose(f);
    return dll_out[0] == '\0' ? 1 : 0;
}

static int copy_state_to_buffer(void *lib, stasis_state_symbol *syms, uint32_t sym_count, uint8_t *buffer, uint32_t total_bytes, int allow_missing, uint32_t *missing_count)
{
    (void)total_bytes;
    for (uint32_t i = 0; i < sym_count; i++)
    {
        void *addr = dlsym(lib, syms[i].name);
        if (!addr)
        {
            if (allow_missing)
            {
                if (missing_count)
                {
                    (*missing_count)++;
                }
                memset(buffer + syms[i].offset, 0, syms[i].size);
                continue;
            }
            fprintf(stderr, "error: state symbol not exported: %s\n", syms[i].name);
            return 1;
        }
        memcpy(buffer + syms[i].offset, (void *)addr, syms[i].size);
    }
    return 0;
}

static int copy_state_from_buffer(void *lib, stasis_state_symbol *syms, uint32_t sym_count, const uint8_t *buffer, uint32_t total_bytes, int allow_missing, uint32_t *missing_count)
{
    (void)total_bytes;
    for (uint32_t i = 0; i < sym_count; i++)
    {
        void *addr = dlsym(lib, syms[i].name);
        if (!addr)
        {
            if (allow_missing)
            {
                if (missing_count)
                {
                    (*missing_count)++;
                }
                continue;
            }
            fprintf(stderr, "error: state symbol not exported: %s\n", syms[i].name);
            return 1;
        }
        memcpy((void *)addr, buffer + syms[i].offset, syms[i].size);
    }
    return 0;
}

static int try_make_fixed_swap_path(const char *input, char *out, size_t out_cap)
{
    const char *suffix_a = ".swapA.so";
    const char *suffix_b = ".swapB.so";
    const char *fixed = ".swap.so";
    size_t len = strlen(input);
    size_t a_len = strlen(suffix_a);
    size_t b_len = strlen(suffix_b);

    if (len >= a_len && strcmp(input + len - a_len, suffix_a) == 0)
    {
        size_t base_len = len - a_len;
        size_t fixed_len = strlen(fixed);
        if (base_len + fixed_len + 1 > out_cap)
        {
            return 0;
        }
        memcpy(out, input, base_len);
        memcpy(out + base_len, fixed, fixed_len + 1);
        return 1;
    }

    if (len >= b_len && strcmp(input + len - b_len, suffix_b) == 0)
    {
        size_t base_len = len - b_len;
        size_t fixed_len = strlen(fixed);
        if (base_len + fixed_len + 1 > out_cap)
        {
            return 0;
        }
        memcpy(out, input, base_len);
        memcpy(out + base_len, fixed, fixed_len + 1);
        return 1;
    }

    return 0;
}
#endif

int main(int argc, char **argv)
{
#ifdef _WIN32
    setvbuf(stdout, NULL, _IONBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);
    enable_vt_processing(GetStdHandle(STD_OUTPUT_HANDLE));
    enable_vt_processing(GetStdHandle(STD_ERROR_HANDLE));
#endif

    const char *dll_path = NULL;
    const char *entry_name = NULL;
    const char *state_path = NULL;
    const char *state_map_path = NULL;
    char state_map_buf[2048];
    state_map_buf[0] = '\0';
    const char *hot_exit_path = NULL;
    const char *swap_file_path = NULL;
    const char *data_bind_json = NULL;
    const char *data_bind_meta = NULL;
    const char *tick_name_override = NULL;
    const char *render_name_override = NULL;
    char launch_dll_buf[2048];
    char launch_entry_buf[512];
    char launch_tick_buf[512];
    char launch_render_buf[512];
    char launch_data_json_buf[2048];
    char launch_data_meta_buf[2048];
    int fps = 60;
    launch_dll_buf[0] = '\0';
    launch_entry_buf[0] = '\0';
    launch_tick_buf[0] = '\0';
    launch_render_buf[0] = '\0';
    launch_data_json_buf[0] = '\0';
    launch_data_meta_buf[0] = '\0';

    if (argc >= 2 && strcmp(argv[1], "--server") == 0)
    {
        char line[256];
        fprintf(stderr, "READY\n");
        fflush(stderr);

        while (fgets(line, sizeof(line), stdin))
        {
            size_t line_len = strlen(line);
            while (line_len > 0 && (line[line_len - 1] == '\n' || line[line_len - 1] == '\r'))
            {
                line[--line_len] = '\0';
            }

            if (line_len == 0)
            {
                continue;
            }

            if (strcmp(line, "QUIT") == 0)
            {
                break;
            }

            if (strncmp(line, "RUN ", 4) != 0)
            {
                fprintf(stderr, "ERR invalid request\n");
                fflush(stderr);
                continue;
            }

            unsigned long dll_len = 0;
            unsigned long entry_len = 0;
            if (sscanf(line + 4, "%lu %lu", &dll_len, &entry_len) != 2 || dll_len == 0 || entry_len == 0)
            {
                fprintf(stderr, "ERR invalid request\n");
                fflush(stderr);
                continue;
            }

            char *req_dll_path = (char *)malloc(dll_len + 1);
            char *req_entry_name = (char *)malloc(entry_len + 1);
            if (!req_dll_path || !req_entry_name)
            {
                free(req_dll_path);
                free(req_entry_name);
                fprintf(stderr, "ERR out of memory\n");
                fflush(stderr);
                continue;
            }

            if (fread(req_dll_path, 1, dll_len, stdin) != dll_len ||
                fread(req_entry_name, 1, entry_len, stdin) != entry_len)
            {
                free(req_dll_path);
                free(req_entry_name);
                fprintf(stderr, "ERR failed to read request\n");
                fflush(stderr);
                continue;
            }

            req_dll_path[dll_len] = '\0';
            req_entry_name[entry_len] = '\0';

#ifndef _WIN32
            set_runtime_dir(req_dll_path);
#endif

#ifdef _WIN32
            stasis_enable_dll_search(argv[0], req_dll_path);
            HMODULE lib = stasis_load_program_library(req_dll_path);
            if (!lib)
            {
                fprintf(stderr, "ERR failed to load\n");
                fflush(stderr);
                free(req_dll_path);
                free(req_entry_name);
                continue;
            }

            FARPROC symbol = GetProcAddress(lib, req_entry_name);
            if (!symbol)
            {
                fprintf(stderr, "ERR entrypoint not found\n");
                fflush(stderr);
                FreeLibrary(lib);
                free(req_dll_path);
                free(req_entry_name);
                continue;
            }

            stasis_try_set_sys_args(lib, argc, argv);
            stasis_entry_fn entry = (stasis_entry_fn)symbol;
            int result = entry();
            FreeLibrary(lib);
#else
            void *lib = dlopen(req_dll_path, RTLD_NOW);
            if (!lib)
            {
                fprintf(stderr, "ERR failed to load\n");
                fflush(stderr);
                free(req_dll_path);
                free(req_entry_name);
                continue;
            }

            void *symbol = dlsym(lib, req_entry_name);
            if (!symbol)
            {
                fprintf(stderr, "ERR entrypoint not found\n");
                fflush(stderr);
                dlclose(lib);
                free(req_dll_path);
                free(req_entry_name);
                continue;
            }

            stasis_try_set_sys_args(lib, argc, argv);
            stasis_entry_fn entry = (stasis_entry_fn)symbol;
            int result = entry();
            dlclose(lib);
#endif

            free(req_dll_path);
            free(req_entry_name);

            fprintf(stderr, "OK %d\n", result);
            fflush(stderr);
        }

        return 0;
    }

    if (argc < 2)
    {
        if (!stasis_try_load_launch_config(
                argv[0],
                launch_dll_buf,
                sizeof(launch_dll_buf),
                launch_entry_buf,
                sizeof(launch_entry_buf),
                launch_tick_buf,
                sizeof(launch_tick_buf),
                launch_render_buf,
                sizeof(launch_render_buf),
                launch_data_json_buf,
                sizeof(launch_data_json_buf),
                launch_data_meta_buf,
                sizeof(launch_data_meta_buf),
                &fps))
        {
            print_usage();
            return 1;
        }
        dll_path = launch_dll_buf;
        entry_name = launch_entry_buf[0] ? launch_entry_buf : "main";
        if (launch_data_json_buf[0] && launch_data_meta_buf[0])
        {
            data_bind_json = launch_data_json_buf;
            data_bind_meta = launch_data_meta_buf;
        }
        if (launch_tick_buf[0])
        {
            tick_name_override = launch_tick_buf;
        }
        if (launch_render_buf[0])
        {
            render_name_override = launch_render_buf;
        }
    }
    else
    {
        dll_path = argv[1];
        entry_name = argc >= 3 ? argv[2] : "run_tests";
    }

    for (int i = 2; i < argc; i++)
    {
        if (strcmp(argv[i], "--state") == 0 && i + 1 < argc)
        {
            state_path = argv[++i];
            continue;
        }
        if (strcmp(argv[i], "--state-map") == 0 && i + 1 < argc)
        {
            state_map_path = argv[++i];
            strncpy(state_map_buf, state_map_path, sizeof(state_map_buf) - 1);
            state_map_buf[sizeof(state_map_buf) - 1] = '\0';
            state_map_path = state_map_buf;
            continue;
        }
        if (strcmp(argv[i], "--hot-exit-file") == 0 && i + 1 < argc)
        {
            hot_exit_path = argv[++i];
            continue;
        }
        if (strcmp(argv[i], "--swap-file") == 0 && i + 1 < argc)
        {
            swap_file_path = argv[++i];
            continue;
        }
        if (strcmp(argv[i], "--fps") == 0 && i + 1 < argc)
        {
            fps = atoi(argv[++i]);
            if (fps < 1)
            {
                fps = 1;
            }
            if (fps > 240)
            {
                fps = 240;
            }
            continue;
        }
        if (strcmp(argv[i], "--data-bind") == 0 && i + 2 < argc)
        {
            data_bind_json = argv[++i];
            data_bind_meta = argv[++i];
            continue;
        }
    }

    if (state_path != NULL && state_map_path == NULL)
    {
        fprintf(stderr, "error: --state requires --state-map.\n");
        return 1;
    }

#ifndef _WIN32
    set_runtime_dir(dll_path);
#endif

#ifdef _WIN32
    stasis_enable_dll_search(argv[0], dll_path);
    int runner_diag = stasis_env_flag("STASIS_RUNNER_DIAG", 0);
    HMODULE lib = stasis_load_program_library(dll_path);
    if (!lib)
    {
        DWORD err = GetLastError();
        char msg[512];
        msg[0] = '\0';
        FormatMessageA(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
                       NULL,
                       err,
                       0,
                       msg,
                       (DWORD)sizeof(msg),
                       NULL);
        fprintf(stderr, "error: failed to load %s (err=%lu %s)\n", dll_path, err, msg);
        return 1;
    }

    {
        FARPROC bind_globals_sym = GetProcAddress(lib, "stasis_aot_bind_runtime_globals");
        if (bind_globals_sym)
        {
            ((stasis_aot_bind_runtime_globals_fn)bind_globals_sym)();
        }
    }

    /* Set DLL handle for data binding system */
    stasis_data_set_dll(lib);

    /* Register data binding if specified */
    if (data_bind_json && data_bind_meta)
    {
        int handle = stasis_data_bind(data_bind_json, data_bind_meta);
        if (handle == 0)
        {
            fprintf(stderr, "warning: failed to register data binding\n");
        }
    }

    FARPROC symbol = GetProcAddress(lib, entry_name);
    if (!symbol)
    {
        fprintf(stderr, "error: entrypoint %s not found in %s\n", entry_name, dll_path);
        FreeLibrary(lib);
        return 1;
    }
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: dll=%s entry=%s tick=%s render=%s\n",
                dll_path ? dll_path : "(null)",
                entry_name ? entry_name : "(null)",
                tick_name_override ? tick_name_override : "(auto)",
                render_name_override ? render_name_override : "(auto)");
        fflush(stderr);
    }

    /* Default window policy: create a small window if graphics runtime is present.
       Programs can request their preferred size/fullscreen via host_window_request globals. */
    HMODULE gfx = GetModuleHandleA("stasis_graphics.dll");
    stasis_init_window_fn init_window = NULL;
    stasis_set_fullscreen_fn set_fullscreen = NULL;
    stasis_set_window_size_fn set_window_size = NULL;
    if (gfx)
    {
        stasis_graphics_runtime_abi_version_fn graphics_abi =
            (stasis_graphics_runtime_abi_version_fn)GetProcAddress(gfx, "stasis_graphics_runtime_abi_version");
        if (!graphics_abi || graphics_abi() != STASIS_GRAPHICS_RUNTIME_ABI_VERSION)
        {
            fprintf(stderr,
                    "error: incompatible stasis_graphics.dll (expected ABI %d)\n",
                    STASIS_GRAPHICS_RUNTIME_ABI_VERSION);
            FreeLibrary(lib);
            return 1;
        }
        stasis_graphics_set_asset_root_fn set_graphics_asset_root =
            (stasis_graphics_set_asset_root_fn)GetProcAddress(gfx, "stasis_set_asset_root");
        const char *launcher_asset_root = getenv("STASIS_ASSET_ROOT");
        if (!set_graphics_asset_root || !launcher_asset_root ||
            !set_graphics_asset_root(launcher_asset_root))
        {
            fprintf(stderr, "error: stasis_graphics.dll rejected the launcher asset root\n");
            FreeLibrary(lib);
            return 1;
        }
        init_window = (stasis_init_window_fn)GetProcAddress(gfx, "stasis_init_window");
        set_fullscreen = (stasis_set_fullscreen_fn)GetProcAddress(gfx, "stasis_set_fullscreen");
        set_window_size = (stasis_set_window_size_fn)GetProcAddress(gfx, "stasis_set_window_size");
        if (init_window)
        {
            const char *start_fs = getenv("STASIS_START_FULLSCREEN");
            int want_fullscreen = (start_fs && strcmp(start_fs, "1") == 0) ? 1 : 0;
            (void)init_window(640, 360, "Stasis");
            if (want_fullscreen && set_fullscreen)
            {
                (void)set_fullscreen(1);
            }
        }
    }

    stasis_state_symbol *syms = NULL;
    uint32_t sym_count = 0;
    uint32_t total_bytes = 0;
    uint64_t map_hash = 0;
    uint8_t *restore_data = NULL;
    stasis_hot_exit_args hot_exit_args = {0};
    if (state_map_path)
    {
        if (read_state_map(state_map_path, &map_hash, &syms, &sym_count, &total_bytes) != 0)
        {
            FreeLibrary(lib);
            return 1;
        }

        uint32_t computed_total = 0;
        for (uint32_t i = 0; i < sym_count; i++)
        {
            computed_total += syms[i].size;
        }
        if (computed_total != total_bytes)
        {
            fprintf(stderr, "error: state map bytes mismatch (computed=%u header=%u)\n", computed_total, total_bytes);
            FreeLibrary(lib);
            return 1;
        }

        if (state_path)
        {
            LARGE_INTEGER freq;
            LARGE_INTEGER t0;
            LARGE_INTEGER t1;
            QueryPerformanceFrequency(&freq);
            QueryPerformanceCounter(&t0);
            int load_result = load_state_snapshot(state_path, map_hash, total_bytes, &restore_data);
            QueryPerformanceCounter(&t1);
            long long restore_io_us = (t1.QuadPart - t0.QuadPart) * 1000000LL / freq.QuadPart;
            if (load_result == 1)
            {
                FreeLibrary(lib);
                return 1;
            }
            if (load_result == 0)
            {
                QueryPerformanceCounter(&t0);
                for (uint32_t i = 0; i < sym_count; i++)
                {
                    FARPROC addr = GetProcAddress(lib, syms[i].name);
                    if (!addr)
                    {
                        fprintf(stderr, "error: state symbol not exported: %s\n", syms[i].name);
                        FreeLibrary(lib);
                        return 1;
                    }
                    memcpy((void *)addr, restore_data + syms[i].offset, syms[i].size);
                }
                QueryPerformanceCounter(&t1);
                long long restore_copy_us = (t1.QuadPart - t0.QuadPart) * 1000000LL / freq.QuadPart;
                fprintf(stderr, "HOTSTATE restore: io=%lldus copy=%lldus bytes=%u symbols=%u\n", restore_io_us, restore_copy_us, total_bytes, sym_count);
            }
            else
            {
                fprintf(stderr, "HOTSTATE restore: none\n");
            }
        }
    }

    HANDLE hot_exit_handle = NULL;
    if (state_path && state_map_path && hot_exit_path && hot_exit_path[0] != '\0')
    {
        hot_exit_args.lib = lib;
        hot_exit_args.state_path = state_path;
        hot_exit_args.map_hash = map_hash;
        hot_exit_args.syms = syms;
        hot_exit_args.sym_count = sym_count;
        hot_exit_args.total_bytes = total_bytes;
        hot_exit_args.hot_exit_path = hot_exit_path;
        hot_exit_handle = CreateThread(NULL, 0, hot_exit_thread, &hot_exit_args, 0, NULL);
    }

    /* Optional tick loop: if `<module>__tick` is exported, call init once then tick at target FPS. */
    char tick_name[512];
    char render_name[512];
    tick_name[0] = '\0';
    render_name[0] = '\0';
    if (tick_name_override && tick_name_override[0])
    {
        strncpy(tick_name, tick_name_override, sizeof(tick_name) - 1);
        tick_name[sizeof(tick_name) - 1] = '\0';
    }
    if (render_name_override && render_name_override[0])
    {
        strncpy(render_name, render_name_override, sizeof(render_name) - 1);
        render_name[sizeof(render_name) - 1] = '\0';
    }
    if (tick_name[0] == '\0' && render_name[0] == '\0')
    {
        stasis_build_related_symbol_names(
            entry_name,
            tick_name,
            sizeof(tick_name),
            render_name,
            sizeof(render_name));
    }

    FARPROC tick_sym = NULL;
    if (tick_name[0] != '\0')
    {
        tick_sym = GetProcAddress(lib, tick_name);
    }
    FARPROC render_sym = NULL;
    if (render_name[0] != '\0')
    {
        render_sym = GetProcAddress(lib, render_name);
    }

    /* Host window request globals are used by bulk mode (defined in src/runtime/host_window_request.stasis). */
    int32_t *host_req_seq = (int32_t *)GetProcAddress(lib, "host_req_seq");
    int32_t *host_req_flags = (int32_t *)GetProcAddress(lib, "host_req_flags");
    int32_t *host_req_window_w_px = (int32_t *)GetProcAddress(lib, "host_req_window_w_px");
    int32_t *host_req_window_h_px = (int32_t *)GetProcAddress(lib, "host_req_window_h_px");

    /* Bulk host loop API (stasis_graphics.dll). */
    stasis_host_bulk_init_fn host_bulk_init = NULL;
    stasis_host_bulk_apply_requests_fn host_bulk_apply_requests = NULL;
    stasis_host_bulk_step_fn host_bulk_step = NULL;
    if (GetModuleHandleA("stasis_graphics.dll"))
    {
        HMODULE gfx_bulk = GetModuleHandleA("stasis_graphics.dll");
        host_bulk_init = (stasis_host_bulk_init_fn)GetProcAddress(gfx_bulk, "stasis_host_bulk_init");
        host_bulk_apply_requests = (stasis_host_bulk_apply_requests_fn)GetProcAddress(gfx_bulk, "stasis_host_bulk_apply_requests");
        host_bulk_step = (stasis_host_bulk_step_fn)GetProcAddress(gfx_bulk, "stasis_host_bulk_step");
    }
    if (host_bulk_init)
    {
        /* Capture baseline seq before main(). */
        host_bulk_init(host_req_seq);
    }

    stasis_try_set_sys_args(lib, argc, argv);
    stasis_entry_fn entry = (stasis_entry_fn)symbol;
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: calling entry\n");
        fflush(stderr);
    }
    int result = entry();
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: entry returned=%d\n", result);
        fflush(stderr);
    }

    if (result == 0 && (tick_sym || render_sym))
    {
        stasis_tick_fn tick = (stasis_tick_fn)tick_sym;
        stasis_tick_fn render = (stasis_tick_fn)render_sym;
        LARGE_INTEGER freq;
        QueryPerformanceFrequency(&freq);
        long long target_us = 1000000LL / (long long)fps;

        /* Tick execution requires HostFrame bulk globals; gfx command buffers are optional. */
        int bulk_active = 0;
        int32_t *host_i32 = NULL;
        float *host_f32 = NULL;
        int32_t *gfx_cmd_i32 = NULL;
        float *gfx_cmd_f32 = NULL;
        uint8_t *gfx_cmd_u8 = NULL;

        stasis_host_get_frame_fn host_get_frame = NULL;
        stasis_host_set_performance_metrics_fn host_set_performance_metrics = NULL;
        stasis_gfx_submit_u8_fn gfx_submit_u8 = NULL;
        int32_t last_req_seq = host_req_seq ? *host_req_seq : 0;

        host_i32 = (int32_t *)GetProcAddress(lib, "host_i32");
        host_f32 = (float *)GetProcAddress(lib, "host_f32");
        gfx_cmd_i32 = (int32_t *)GetProcAddress(lib, "gfx_cmd_i32");
        gfx_cmd_f32 = (float *)GetProcAddress(lib, "gfx_cmd_f32");
        gfx_cmd_u8 = (uint8_t *)GetProcAddress(lib, "gfx_cmd_u8");

        HMODULE gfx = GetModuleHandleA("stasis_graphics.dll");
        if (gfx)
        {
            host_get_frame = (stasis_host_get_frame_fn)GetProcAddress(gfx, "stasis_host_get_frame");
            host_set_performance_metrics = (stasis_host_set_performance_metrics_fn)GetProcAddress(
                gfx,
                "stasis_host_set_performance_metrics");
            gfx_submit_u8 = (stasis_gfx_submit_u8_fn)GetProcAddress(gfx, "stasis_gfx_submit_u8");
        }

        if ((host_i32 && host_f32 && host_get_frame) ||
            (host_i32 && host_f32 && gfx_cmd_i32 && gfx_cmd_f32 && gfx_cmd_u8 && host_bulk_step))
        {
            bulk_active = 1;
        }
        if (!bulk_active)
        {
            fprintf(stderr, "error: stasis_runner requires HostFrame bulk globals for tick execution\n");
            result = 1;
        }

        const int log_cmd_hdr = stasis_env_flag("STASIS_GFX_LOG_CMD_HDR", 0);
        int log_cmd_remaining = log_cmd_hdr ? 5 : 0;

        /* Apply any request the program made during main(). */
        if (bulk_active && host_bulk_apply_requests)
        {
            host_bulk_apply_requests(host_req_seq, host_req_flags, host_req_window_w_px, host_req_window_h_px);
        }
        else if (bulk_active && host_req_seq && host_req_flags && init_window && set_fullscreen && *host_req_seq != last_req_seq)
        {
            last_req_seq = *host_req_seq;
            const int flags = *host_req_flags;
            if ((flags & 1) != 0 && set_window_size && host_req_window_w_px && host_req_window_h_px)
            {
                (void)set_fullscreen(0);
                set_window_size(*host_req_window_w_px, *host_req_window_h_px);
            }
            else if ((flags & 2) != 0)
            {
                (void)set_fullscreen(1);
            }
        }

        LARGE_INTEGER last;
        QueryPerformanceCounter(&last);

        for (;;)
        {
            /* Swap between ticks if requested. */
                    if (swap_file_path && file_exists(swap_file_path))
                    {
                        char new_path[2048];
                        char map_path[2048];
                        map_path[0] = '\0';
                        if (read_swap_file(swap_file_path, new_path, sizeof(new_path), map_path, sizeof(map_path)) == 0)
                        {
                            DeleteFileA(swap_file_path);

                            if (!file_exists(new_path))
                            {
                                fprintf(stderr, "HOTSWAP warning: DLL not found: %s\n", new_path);
                                fflush(stderr);
                                continue;
                            }

                            stasis_state_symbol *next_syms = syms;
                            uint32_t next_sym_count = sym_count;
                            uint32_t next_total_bytes = total_bytes;
                            uint64_t next_map_hash = map_hash;
                            char next_map_buf[2048];
                            const char *next_map_path = state_map_path;
                            int next_map_owned = 0;

                            if (map_path[0] != '\0' && (!state_map_path || strcmp(map_path, state_map_path) != 0))
                            {
                                uint64_t new_hash = 0;
                                uint32_t new_count = 0;
                                uint32_t new_total = 0;
                                stasis_state_symbol *new_syms = NULL;
                                if (read_state_map(map_path, &new_hash, &new_syms, &new_count, &new_total) == 0)
                                {
                                    next_syms = new_syms;
                                    next_sym_count = new_count;
                                    next_total_bytes = new_total;
                                    next_map_hash = new_hash;
                                    strncpy(next_map_buf, map_path, sizeof(next_map_buf) - 1);
                                    next_map_buf[sizeof(next_map_buf) - 1] = '\0';
                                    next_map_path = next_map_buf;
                                    next_map_owned = 1;
                                }
                                else
                                {
                                    fprintf(stderr, "HOTSWAP warning: failed to read state map: %s\n", map_path);
                                    fflush(stderr);
                                }
                            }

                            LARGE_INTEGER sw_freq;
                            LARGE_INTEGER sw_t0;
                            LARGE_INTEGER sw_t1;
                            QueryPerformanceFrequency(&sw_freq);

                            fprintf(stderr, "HOTSWAP loading: %s\n", new_path);
                            fflush(stderr);

                            uint8_t *buffer = NULL;
                            uint32_t missing_save = 0;
                            uint32_t missing_restore = 0;
                            uint32_t truncated = 0;
                            long long save_us = 0;
                            long long load_us = 0;
                            long long tick_us = 0;
                            long long restore_us = 0;
                            if (next_map_path)
                            {
                                buffer = (uint8_t *)malloc(next_total_bytes);
                                if (!buffer)
                                {
                                    fprintf(stderr, "HOTSWAP warning: out of memory\n");
                                    fflush(stderr);
                                    if (next_map_owned)
                                    {
                                        stasis_state_symbol *tmp_syms = next_syms;
                                        uint32_t tmp_count = next_sym_count;
                                        free_state_map(&tmp_syms, &tmp_count);
                                    }
                                    continue;
                                }
                                memset(buffer, 0, next_total_bytes);

                                QueryPerformanceCounter(&sw_t0);
                                for (uint32_t i = 0; i < next_sym_count; i++)
                                {
                                    const char *name = next_syms[i].name;
                                    FARPROC addr = GetProcAddress(lib, name);
                                    if (!addr)
                                    {
                                        missing_save++;
                                        continue;
                                    }

                                    uint32_t old_size = next_syms[i].size;
                                    for (uint32_t j = 0; j < sym_count; j++)
                                    {
                                        if (strcmp(syms[j].name, name) == 0)
                                        {
                                            old_size = syms[j].size;
                                            break;
                                        }
                                    }

                                    uint32_t copy_n = old_size < next_syms[i].size ? old_size : next_syms[i].size;
                                    if (copy_n < next_syms[i].size)
                                    {
                                        truncated++;
                                    }
                                    memcpy(buffer + next_syms[i].offset, (void *)addr, copy_n);
                                }
                                QueryPerformanceCounter(&sw_t1);
                                save_us = (sw_t1.QuadPart - sw_t0.QuadPart) * 1000000LL / sw_freq.QuadPart;
                            }

                            QueryPerformanceCounter(&sw_t0);
                            HMODULE new_lib = stasis_load_program_library(new_path);
                            QueryPerformanceCounter(&sw_t1);
                            load_us = (sw_t1.QuadPart - sw_t0.QuadPart) * 1000000LL / sw_freq.QuadPart;
                            if (!new_lib)
                            {
                                DWORD err = GetLastError();
                                char msg[512];
                                msg[0] = '\0';
                                FormatMessageA(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
                                               NULL,
                                               err,
                                               0,
                                               msg,
                                               (DWORD)sizeof(msg),
                                               NULL);
                                fprintf(stderr, "HOTSWAP warning: failed to load %s (err=%lu %s)\n", new_path, err, msg);
                                fflush(stderr);
                                free(buffer);
                                if (next_map_owned)
                                {
                                    stasis_state_symbol *tmp_syms = next_syms;
                                    uint32_t tmp_count = next_sym_count;
                                    free_state_map(&tmp_syms, &tmp_count);
                                }
                                continue;
                            }

                            FARPROC new_tick_sym = NULL;
                            FARPROC new_render_sym = NULL;
                            QueryPerformanceCounter(&sw_t0);
                            if (tick_name[0] != '\0')
                            {
                                new_tick_sym = GetProcAddress(new_lib, tick_name);
                            }
                            if (render_name[0] != '\0')
                            {
                                new_render_sym = GetProcAddress(new_lib, render_name);
                            }
                            QueryPerformanceCounter(&sw_t1);
                            tick_us = (sw_t1.QuadPart - sw_t0.QuadPart) * 1000000LL / sw_freq.QuadPart;
                            if (!new_tick_sym && !new_render_sym)
                            {
                                fprintf(stderr, "HOTSWAP warning: tick/render entrypoints not found in %s\n", new_path);
                                fflush(stderr);
                                FreeLibrary(new_lib);
                                free(buffer);
                                if (next_map_owned)
                                {
                                    stasis_state_symbol *tmp_syms = next_syms;
                                    uint32_t tmp_count = next_sym_count;
                                    free_state_map(&tmp_syms, &tmp_count);
                                }
                                continue;
                            }

                            if (buffer)
                            {
                                QueryPerformanceCounter(&sw_t0);
                                for (uint32_t i = 0; i < next_sym_count; i++)
                                {
                                    FARPROC addr = GetProcAddress(new_lib, next_syms[i].name);
                                    if (!addr)
                                    {
                                        missing_restore++;
                                        continue;
                                    }
                                    memcpy((void *)addr, buffer + next_syms[i].offset, next_syms[i].size);
                                }
                                QueryPerformanceCounter(&sw_t1);
                                restore_us = (sw_t1.QuadPart - sw_t0.QuadPart) * 1000000LL / sw_freq.QuadPart;
                                free(buffer);
                                fprintf(
                                    stderr,
                                    "HOTSWAP ok: save=%.3fms load=%.3fms tick=%.3fms restore=%.3fms bytes=%u symbols=%u\n",
                                    (double)save_us / 1000.0,
                                    (double)load_us / 1000.0,
                                    (double)tick_us / 1000.0,
                                    (double)restore_us / 1000.0,
                                    next_total_bytes,
                                    next_sym_count);
                                fflush(stderr);
                                if (missing_save > 0 || missing_restore > 0 || truncated > 0)
                                {
                                    fprintf(stderr, "HOTSWAP warning: state migration issues (missing save=%u restore=%u truncated=%u); consider restarting to resync state.\n", missing_save, missing_restore, truncated);
                                    fflush(stderr);
                                }
                            }
                            else
                            {
                                fprintf(stderr, "HOTSWAP ok: load=%.3fms tick=%.3fms\n", (double)load_us / 1000.0, (double)tick_us / 1000.0);
                                fflush(stderr);
                            }

                            FreeLibrary(lib);
                            lib = new_lib;
                            {
                                FARPROC bind_globals_sym = GetProcAddress(lib, "stasis_aot_bind_runtime_globals");
                                if (bind_globals_sym)
                                {
                                    ((stasis_aot_bind_runtime_globals_fn)bind_globals_sym)();
                                }
                            }
                            tick = (stasis_tick_fn)new_tick_sym;
                            render = (stasis_tick_fn)new_render_sym;
                            stasis_try_set_sys_args(lib, argc, argv);
                            stasis_rebind_bulk_pointers(
                                lib,
                                &bulk_active,
                                &host_i32,
                                &host_f32,
                                &gfx_cmd_i32,
                                &gfx_cmd_f32,
                                &gfx_cmd_u8,
                                &host_get_frame,
                                &gfx_submit_u8,
                                &host_bulk_init,
                                &host_bulk_apply_requests,
                                &host_bulk_step,
                                &host_req_seq,
                                &host_req_flags,
                                &host_req_window_w_px,
                                &host_req_window_h_px,
                                &last_req_seq);

                            if (host_bulk_init)
                            {
                                host_bulk_init(host_req_seq);
                            }

                            if (next_map_owned)
                            {
                                free_state_map(&syms, &sym_count);
                                syms = next_syms;
                                sym_count = next_sym_count;
                                total_bytes = next_total_bytes;
                                map_hash = next_map_hash;
                                strncpy(state_map_buf, next_map_path, sizeof(state_map_buf) - 1);
                                state_map_buf[sizeof(state_map_buf) - 1] = '\0';
                                state_map_path = state_map_buf;
                                fprintf(stderr, "HOTSWAP map: %s\n", state_map_path);
                                fflush(stderr);
                            }

                            /* Update DLL handle for data binding system */
                            stasis_data_set_dll(new_lib);
                        }
                    }

            /* Poll data bindings for changes (fast path: just checks mtimes) */
            stasis_data_poll_all();

            if (bulk_active && host_bulk_step && !render && gfx_cmd_i32 && gfx_cmd_f32 && gfx_cmd_u8)
            {
                int step_result = host_bulk_step(
                    host_i32,
                    host_f32,
                    gfx_cmd_i32,
                    gfx_cmd_f32,
                    gfx_cmd_u8,
                    host_req_seq,
                    host_req_flags,
                    host_req_window_w_px,
                    host_req_window_h_px,
                    tick);
                if (step_result != 0)
                {
                    result = step_result == 1 ? 0 : step_result;
                    break;
                }
            }
            else if (bulk_active)
            {
                if (!host_get_frame)
                {
                    fprintf(stderr, "error: manual tick/render path requires stasis_host_get_frame\n");
                    result = 1;
                    break;
                }
                host_get_frame(host_i32, host_f32);

                /* Exit if host requested quit (avoid requiring guest queries). */
                if (host_i32[9] != 0)
                {
                    result = 0;
                    break;
                }

                if (host_bulk_apply_requests)
                {
                    host_bulk_apply_requests(host_req_seq, host_req_flags, host_req_window_w_px, host_req_window_h_px);
                }
                else if (host_req_seq && host_req_flags && init_window && set_fullscreen && *host_req_seq != last_req_seq)
                {
                    last_req_seq = *host_req_seq;
                    const int flags = *host_req_flags;
                    if ((flags & 1) != 0 && set_window_size && host_req_window_w_px && host_req_window_h_px)
                    {
                        (void)set_fullscreen(0);
                        set_window_size(*host_req_window_w_px, *host_req_window_h_px);
                    }
                    else if ((flags & 2) != 0)
                    {
                        (void)set_fullscreen(1);
                    }
                }

                int step_result = 0;
                uint64_t tick_us = 0;
                uint64_t render_us = 0;
                if (tick)
                {
                    LARGE_INTEGER phase_started;
                    LARGE_INTEGER phase_finished;
                    QueryPerformanceCounter(&phase_started);
                    step_result = tick();
                    QueryPerformanceCounter(&phase_finished);
                    tick_us = (uint64_t)((phase_finished.QuadPart - phase_started.QuadPart) * 1000000LL / freq.QuadPart);
                    if (step_result != 0)
                    {
                        result = step_result == 1 ? 0 : step_result;
                        break;
                    }
                }

                if (render)
                {
                    LARGE_INTEGER phase_started;
                    LARGE_INTEGER phase_finished;
                    QueryPerformanceCounter(&phase_started);
                    step_result = render();
                    QueryPerformanceCounter(&phase_finished);
                    render_us = (uint64_t)((phase_finished.QuadPart - phase_started.QuadPart) * 1000000LL / freq.QuadPart);
                    if (step_result != 0)
                    {
                        result = step_result == 1 ? 0 : step_result;
                        break;
                    }
                }

                if (log_cmd_remaining > 0 && gfx_cmd_i32)
                {
                    fprintf(stderr, "GFX_CMD flags=%d lines=%d sprites=%d text=%d\n",
                            gfx_cmd_i32[2], gfx_cmd_i32[3], gfx_cmd_i32[4], gfx_cmd_i32[7]);
                    log_cmd_remaining--;
                }

                if (gfx_submit_u8 && gfx_cmd_i32 && gfx_cmd_f32 && gfx_cmd_u8)
                {
                    if (host_set_performance_metrics)
                    {
                        host_set_performance_metrics(tick_us, render_us);
                    }
                    gfx_submit_u8(gfx_cmd_i32, gfx_cmd_f32, gfx_cmd_u8);
                }
            }
            else
            {
                fprintf(stderr, "error: HostFrame bulk globals became unavailable during tick loop\n");
                result = 1;
                break;
            }

            LARGE_INTEGER now;
            QueryPerformanceCounter(&now);
            long long elapsed_us = (now.QuadPart - last.QuadPart) * 1000000LL / freq.QuadPart;
            last = now;

            long long sleep_us = target_us - elapsed_us;
            while (sleep_us > 0)
            {
                /* Break host sleep if a swap is pending. */
                if (swap_file_path && file_exists(swap_file_path))
                {
                    break;
                }
                DWORD ms = (DWORD)(sleep_us / 1000LL);
                if (ms == 0)
                {
                    ms = 1;
                }
                if (ms > 5)
                {
                    ms = 5;
                }
                Sleep(ms);
                sleep_us -= (long long)ms * 1000LL;
            }
        }
    }

    if (hot_exit_handle)
    {
        CloseHandle(hot_exit_handle);
    }

    if (state_path && state_map_path)
    {
        uint8_t *save_data = (uint8_t *)malloc(total_bytes);
        if (!save_data)
        {
            fprintf(stderr, "error: out of memory\n");
            FreeLibrary(lib);
            return 1;
        }

        LARGE_INTEGER freq;
        LARGE_INTEGER t0;
        LARGE_INTEGER t1;
        QueryPerformanceFrequency(&freq);
        QueryPerformanceCounter(&t0);
        for (uint32_t i = 0; i < sym_count; i++)
        {
            FARPROC addr = GetProcAddress(lib, syms[i].name);
            if (!addr)
            {
                fprintf(stderr, "error: state symbol not exported: %s\n", syms[i].name);
                FreeLibrary(lib);
                return 1;
            }
            memcpy(save_data + syms[i].offset, (void *)addr, syms[i].size);
        }
        QueryPerformanceCounter(&t1);
        long long save_copy_us = (t1.QuadPart - t0.QuadPart) * 1000000LL / freq.QuadPart;

        QueryPerformanceCounter(&t0);
        int save_result = save_state_snapshot(state_path, map_hash, save_data, total_bytes);
        QueryPerformanceCounter(&t1);
        long long save_io_us = (t1.QuadPart - t0.QuadPart) * 1000000LL / freq.QuadPart;
        if (save_result != 0)
        {
            free(save_data);
            FreeLibrary(lib);
            return 1;
        }

        fprintf(stderr, "HOTSTATE save: io=%lldus copy=%lldus bytes=%u symbols=%u\n", save_io_us, save_copy_us, total_bytes, sym_count);
        free(save_data);
    }

    if (state_map_path)
    {
        free(restore_data);
        for (uint32_t i = 0; i < sym_count; i++)
        {
            free(syms[i].name);
        }
        free(syms);
    }
    FreeLibrary(lib);
    return result;
#else
    set_runtime_dir(dll_path);

    int runner_diag = 0;
    {
        const char *diag_env = getenv("STASIS_RUNNER_DIAG");
        runner_diag = diag_env && diag_env[0] == '1';
    }
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: dll=%s entry=%s\n", dll_path ? dll_path : "(null)", entry_name ? entry_name : "(null)");
        fflush(stderr);
    }

    const char *graphics_path = getenv("STASIS_RUNTIME_LIBRARY_PATH");
    void *gfx_lib = NULL;
    if (graphics_path && graphics_path[0])
    {
        gfx_lib = dlopen(graphics_path, RTLD_NOW | RTLD_GLOBAL);
        if (!gfx_lib)
        {
            fprintf(stderr, "error: failed to load packaged graphics runtime %s: %s\n",
                    graphics_path, dlerror());
            return 1;
        }
        stasis_graphics_runtime_abi_version_fn graphics_abi =
            (stasis_graphics_runtime_abi_version_fn)dlsym(gfx_lib, "stasis_graphics_runtime_abi_version");
        if (!graphics_abi || graphics_abi() != STASIS_GRAPHICS_RUNTIME_ABI_VERSION)
        {
            fprintf(stderr, "error: incompatible packaged graphics runtime (expected ABI %d)\n",
                    STASIS_GRAPHICS_RUNTIME_ABI_VERSION);
            dlclose(gfx_lib);
            return 1;
        }
        stasis_graphics_set_asset_root_fn set_graphics_asset_root =
            (stasis_graphics_set_asset_root_fn)dlsym(gfx_lib, "stasis_set_asset_root");
        const char *launcher_asset_root = getenv("STASIS_ASSET_ROOT");
        if (!set_graphics_asset_root || !launcher_asset_root ||
            !set_graphics_asset_root(launcher_asset_root))
        {
            fprintf(stderr, "error: packaged graphics runtime rejected the launcher asset root\n");
            dlclose(gfx_lib);
            return 1;
        }
        stasis_init_window_fn init_window =
            (stasis_init_window_fn)dlsym(gfx_lib, "stasis_init_window");
        stasis_set_fullscreen_fn set_fullscreen =
            (stasis_set_fullscreen_fn)dlsym(gfx_lib, "stasis_set_fullscreen");
        if (init_window)
        {
            const char *start_fs = getenv("STASIS_START_FULLSCREEN");
            int want_fullscreen = (start_fs && strcmp(start_fs, "1") == 0) ? 1 : 0;
            (void)init_window(640, 360, "Stasis");
            if (want_fullscreen && set_fullscreen)
            {
                (void)set_fullscreen(1);
            }
        }
    }

    void *lib = dlopen(dll_path, RTLD_NOW);
    if (!lib)
    {
        fprintf(stderr, "error: failed to load %s: %s\n", dll_path, dlerror());
        return 1;
    }
    {
        void *bind_globals_sym = dlsym(lib, "stasis_aot_bind_runtime_globals");
        if (bind_globals_sym)
        {
            ((stasis_aot_bind_runtime_globals_fn)bind_globals_sym)();
        }
    }
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: dlopen ok\n");
        fflush(stderr);
    }

    void *symbol = dlsym(lib, entry_name);
    if (!symbol)
    {
        fprintf(stderr, "error: entrypoint %s not found in %s\n", entry_name, dll_path);
        dlclose(lib);
        return 1;
    }
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: entry symbol ok\n");
        fflush(stderr);
    }

    char tick_name[512];
    char render_name[512];
    tick_name[0] = '\0';
    render_name[0] = '\0';
    if (tick_name_override && tick_name_override[0])
    {
        strncpy(tick_name, tick_name_override, sizeof(tick_name) - 1);
        tick_name[sizeof(tick_name) - 1] = '\0';
    }
    if (render_name_override && render_name_override[0])
    {
        strncpy(render_name, render_name_override, sizeof(render_name) - 1);
        render_name[sizeof(render_name) - 1] = '\0';
    }
    if (tick_name[0] == '\0' && render_name[0] == '\0')
    {
        stasis_build_related_symbol_names(
            entry_name,
            tick_name,
            sizeof(tick_name),
            render_name,
            sizeof(render_name));
    }
    void *tick_sym = NULL;
    if (tick_name[0] != '\0')
    {
        tick_sym = dlsym(lib, tick_name);
    }
    void *render_sym = NULL;
    if (render_name[0] != '\0')
    {
        render_sym = dlsym(lib, render_name);
    }
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: tick_name=%s tick_sym=%s render_name=%s render_sym=%s\n",
                tick_name[0] ? tick_name : "(none)",
                tick_sym ? "yes" : "no",
                render_name[0] ? render_name : "(none)",
                render_sym ? "yes" : "no");
        fflush(stderr);
    }

    uint64_t map_hash = 0;
    stasis_state_symbol *syms = NULL;
    uint32_t sym_count = 0;
    uint32_t total_bytes = 0;
    if (state_map_path)
    {
        if (read_state_map(state_map_path, &map_hash, &syms, &sym_count, &total_bytes) != 0)
        {
            dlclose(lib);
            return 1;
        }
    }

    stasis_data_set_dll(lib);
    if (data_bind_json && data_bind_meta)
    {
        int handle = stasis_data_bind(data_bind_json, data_bind_meta);
        if (handle == 0)
        {
            fprintf(stderr, "warning: failed to register data binding\n");
        }
    }
    stasis_try_set_sys_args(lib, argc, argv);

    stasis_entry_fn entry = (stasis_entry_fn)symbol;
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: calling entry\n");
        fflush(stderr);
    }
    int result = entry();
    if (runner_diag)
    {
        fprintf(stderr, "RUNNER_DIAG: entry returned=%d\n", result);
        fflush(stderr);
    }

    if (result == 0 && (tick_sym || render_sym))
    {
        stasis_tick_fn tick = (stasis_tick_fn)tick_sym;
        stasis_tick_fn render = (stasis_tick_fn)render_sym;
        long long target_us = 1000000LL / (long long)fps;
        struct timespec ts_last;
        clock_gettime(CLOCK_MONOTONIC, &ts_last);
        long long last_us = ts_last.tv_sec * 1000000LL + ts_last.tv_nsec / 1000LL;
        int tick_diag_count = 0;

        /* Bulk host loop API (stasis_graphics.so). */
        stasis_host_bulk_init_fn host_bulk_init = NULL;
        stasis_host_bulk_apply_requests_fn host_bulk_apply_requests = NULL;
        stasis_host_bulk_step_fn host_bulk_step = NULL;
        stasis_host_get_frame_fn host_get_frame = NULL;
        stasis_gfx_submit_u8_fn gfx_submit_u8 = NULL;

        if (!gfx_lib)
        {
            gfx_lib = dlopen("libstasis_graphics.so", RTLD_NOW | RTLD_GLOBAL);
        }
        if (!gfx_lib)
        {
            gfx_lib = dlopen("stasis_graphics.so", RTLD_NOW | RTLD_GLOBAL);
        }
        if (!gfx_lib)
        {
            gfx_lib = RTLD_DEFAULT;
        }

        host_bulk_init = (stasis_host_bulk_init_fn)dlsym(gfx_lib, "stasis_host_bulk_init");
        host_bulk_apply_requests = (stasis_host_bulk_apply_requests_fn)dlsym(gfx_lib, "stasis_host_bulk_apply_requests");
        host_bulk_step = (stasis_host_bulk_step_fn)dlsym(gfx_lib, "stasis_host_bulk_step");
        host_get_frame = (stasis_host_get_frame_fn)dlsym(gfx_lib, "stasis_host_get_frame");
        gfx_submit_u8 = (stasis_gfx_submit_u8_fn)dlsym(gfx_lib, "stasis_gfx_submit_u8");

        int32_t *host_req_seq = NULL;
        int32_t *host_req_flags = NULL;
        int32_t *host_req_window_w_px = NULL;
        int32_t *host_req_window_h_px = NULL;

        int bulk_active = 0;
        int32_t *host_i32 = NULL;
        float *host_f32 = NULL;
        int32_t *gfx_cmd_i32 = NULL;
        float *gfx_cmd_f32 = NULL;
        uint8_t *gfx_cmd_u8 = NULL;
        int32_t last_req_seq = host_req_seq ? *host_req_seq : 0;

        bulk_active = stasis_rebind_bulk_pointers_linux(
            lib,
            host_bulk_init,
            host_bulk_step,
            host_get_frame,
            gfx_submit_u8,
            &host_req_seq,
            &host_req_flags,
            &host_req_window_w_px,
            &host_req_window_h_px,
            &host_i32,
            &host_f32,
            &gfx_cmd_i32,
            &gfx_cmd_f32,
            &gfx_cmd_u8,
            &last_req_seq);
        if (!bulk_active)
        {
            fprintf(stderr, "error: stasis_runner requires HostFrame bulk globals for tick execution\n");
            result = 1;
        }

        if (bulk_active && host_bulk_apply_requests)
        {
            host_bulk_apply_requests(host_req_seq, host_req_flags, host_req_window_w_px, host_req_window_h_px);
        }
        for (;;)
        {
            if (swap_file_path && file_exists(swap_file_path))
            {
                char new_path[2048];
                char map_path[2048];
                map_path[0] = '\0';
                if (read_swap_file(swap_file_path, new_path, sizeof(new_path), map_path, sizeof(map_path)) == 0)
                {
                    unlink(swap_file_path);

                    if (!file_exists(new_path))
                    {
                        fprintf(stderr, "HOTSWAP warning: library not found: %s\n", new_path);
                        fflush(stderr);
                        continue;
                    }

                    stasis_state_symbol *next_syms = syms;
                    uint32_t next_sym_count = sym_count;
                    uint32_t next_total_bytes = total_bytes;
                    uint64_t next_map_hash = map_hash;
                    char next_map_buf[2048];
                    const char *next_map_path = state_map_path;
                    int next_map_owned = 0;

                    if (map_path[0] != '\0' && (!state_map_path || strcmp(map_path, state_map_path) != 0))
                    {
                        uint64_t new_hash = 0;
                        uint32_t new_count = 0;
                        uint32_t new_total = 0;
                        stasis_state_symbol *new_syms = NULL;
                        if (read_state_map(map_path, &new_hash, &new_syms, &new_count, &new_total) == 0)
                        {
                            next_syms = new_syms;
                            next_sym_count = new_count;
                            next_total_bytes = new_total;
                            next_map_hash = new_hash;
                            strncpy(next_map_buf, map_path, sizeof(next_map_buf) - 1);
                            next_map_buf[sizeof(next_map_buf) - 1] = '\0';
                            next_map_path = next_map_buf;
                            next_map_owned = 1;
                        }
                        else
                        {
                            fprintf(stderr, "HOTSWAP warning: failed to read state map: %s\n", map_path);
                            fflush(stderr);
                        }
                    }

                    fprintf(stderr, "HOTSWAP loading: %s\n", new_path);
                    fflush(stderr);

                    uint8_t *buffer = NULL;
                    uint32_t missing_save = 0;
                    uint32_t missing_restore = 0;
                    uint32_t truncated = 0;
                    long long save_us = 0;
                    long long load_us = 0;
                    long long tick_us = 0;
                    long long restore_us = 0;
                    if (next_map_path)
                    {
                        buffer = (uint8_t *)malloc(next_total_bytes);
                        if (!buffer)
                        {
                            fprintf(stderr, "HOTSWAP warning: out of memory\n");
                            fflush(stderr);
                            if (next_map_owned)
                            {
                                stasis_state_symbol *tmp_syms = next_syms;
                                uint32_t tmp_count = next_sym_count;
                                free_state_map(&tmp_syms, &tmp_count);
                            }
                            continue;
                        }
                        memset(buffer, 0, next_total_bytes);

                        struct timespec t0;
                        struct timespec t1;
                        clock_gettime(CLOCK_MONOTONIC, &t0);
                        for (uint32_t i = 0; i < next_sym_count; i++)
                        {
                            const char *name = next_syms[i].name;
                            void *addr = dlsym(lib, name);
                            if (!addr)
                            {
                                missing_save++;
                                continue;
                            }

                            uint32_t old_size = next_syms[i].size;
                            for (uint32_t j = 0; j < sym_count; j++)
                            {
                                if (strcmp(syms[j].name, name) == 0)
                                {
                                    old_size = syms[j].size;
                                    break;
                                }
                            }

                            uint32_t copy_n = old_size < next_syms[i].size ? old_size : next_syms[i].size;
                            if (copy_n < next_syms[i].size)
                            {
                                truncated++;
                            }
                            memcpy(buffer + next_syms[i].offset, addr, copy_n);
                        }
                        clock_gettime(CLOCK_MONOTONIC, &t1);
                        save_us = (t1.tv_sec - t0.tv_sec) * 1000000LL + (t1.tv_nsec - t0.tv_nsec) / 1000LL;
                    }

                    struct timespec t0;
                    struct timespec t1;
                    clock_gettime(CLOCK_MONOTONIC, &t0);
                    void *new_lib = dlopen(new_path, RTLD_NOW);
                    clock_gettime(CLOCK_MONOTONIC, &t1);
                    load_us = (t1.tv_sec - t0.tv_sec) * 1000000LL + (t1.tv_nsec - t0.tv_nsec) / 1000LL;
                    if (!new_lib)
                    {
                        fprintf(stderr, "HOTSWAP warning: failed to load %s: %s\n", new_path, dlerror());
                        fflush(stderr);
                        free(buffer);
                        if (next_map_owned)
                        {
                            stasis_state_symbol *tmp_syms = next_syms;
                            uint32_t tmp_count = next_sym_count;
                            free_state_map(&tmp_syms, &tmp_count);
                        }
                        continue;
                    }

                    void *new_tick_sym = NULL;
                    void *new_render_sym = NULL;
                    clock_gettime(CLOCK_MONOTONIC, &t0);
                    if (tick_name[0] != '\0')
                    {
                        new_tick_sym = dlsym(new_lib, tick_name);
                    }
                    if (render_name[0] != '\0')
                    {
                        new_render_sym = dlsym(new_lib, render_name);
                    }
                    clock_gettime(CLOCK_MONOTONIC, &t1);
                    tick_us = (t1.tv_sec - t0.tv_sec) * 1000000LL + (t1.tv_nsec - t0.tv_nsec) / 1000LL;
                    if (!new_tick_sym && !new_render_sym)
                    {
                        fprintf(stderr, "HOTSWAP warning: tick/render entrypoints not found in %s\n", new_path);
                        fflush(stderr);
                        dlclose(new_lib);
                        free(buffer);
                        if (next_map_owned)
                        {
                            stasis_state_symbol *tmp_syms = next_syms;
                            uint32_t tmp_count = next_sym_count;
                            free_state_map(&tmp_syms, &tmp_count);
                        }
                        continue;
                    }

                    if (buffer)
                    {
                        clock_gettime(CLOCK_MONOTONIC, &t0);
                        for (uint32_t i = 0; i < next_sym_count; i++)
                        {
                            void *addr = dlsym(new_lib, next_syms[i].name);
                            if (!addr)
                            {
                                missing_restore++;
                                continue;
                            }
                            memcpy(addr, buffer + next_syms[i].offset, next_syms[i].size);
                        }
                        clock_gettime(CLOCK_MONOTONIC, &t1);
                        restore_us = (t1.tv_sec - t0.tv_sec) * 1000000LL + (t1.tv_nsec - t0.tv_nsec) / 1000LL;
                        free(buffer);
                        fprintf(
                            stderr,
                            "HOTSWAP ok: save=%.3fms load=%.3fms tick=%.3fms restore=%.3fms bytes=%u symbols=%u\n",
                            (double)save_us / 1000.0,
                            (double)load_us / 1000.0,
                            (double)tick_us / 1000.0,
                            (double)restore_us / 1000.0,
                            next_total_bytes,
                            next_sym_count);
                        fflush(stderr);
                        if (missing_save > 0 || missing_restore > 0 || truncated > 0)
                        {
                            fprintf(stderr, "HOTSWAP warning: state migration issues (missing save=%u restore=%u truncated=%u); consider restarting to resync state.\n", missing_save, missing_restore, truncated);
                            fflush(stderr);
                        }
                    }
                    else
                    {
                        fprintf(stderr, "HOTSWAP ok: load=%.3fms tick=%.3fms\n", (double)load_us / 1000.0, (double)tick_us / 1000.0);
                        fflush(stderr);
                    }

                    dlclose(lib);
                    lib = new_lib;
                    {
                        void *bind_globals_sym = dlsym(lib, "stasis_aot_bind_runtime_globals");
                        if (bind_globals_sym)
                        {
                            ((stasis_aot_bind_runtime_globals_fn)bind_globals_sym)();
                        }
                    }
                    tick = (stasis_tick_fn)new_tick_sym;
                    render = (stasis_tick_fn)new_render_sym;
                    stasis_try_set_sys_args(lib, argc, argv);
                    stasis_data_set_dll(new_lib);
                    bulk_active = stasis_rebind_bulk_pointers_linux(
                        lib,
                        host_bulk_init,
                        host_bulk_step,
                        host_get_frame,
                        gfx_submit_u8,
                        &host_req_seq,
                        &host_req_flags,
                        &host_req_window_w_px,
                        &host_req_window_h_px,
                        &host_i32,
                        &host_f32,
                        &gfx_cmd_i32,
                        &gfx_cmd_f32,
                        &gfx_cmd_u8,
                        &last_req_seq);

                    if (next_map_owned)
                    {
                        free_state_map(&syms, &sym_count);
                        syms = next_syms;
                        sym_count = next_sym_count;
                        total_bytes = next_total_bytes;
                        map_hash = next_map_hash;
                        strncpy(state_map_buf, next_map_path, sizeof(state_map_buf) - 1);
                        state_map_buf[sizeof(state_map_buf) - 1] = '\0';
                        state_map_path = state_map_buf;
                        fprintf(stderr, "HOTSWAP map: %s\n", state_map_path);
                        fflush(stderr);
                    }
                }
            }

            void *current_host_i32 = dlsym(lib, "host_i32");
            if (current_host_i32 && current_host_i32 != (void *)host_i32)
            {
                bulk_active = stasis_rebind_bulk_pointers_linux(
                    lib,
                    host_bulk_init,
                    host_bulk_step,
                    host_get_frame,
                    gfx_submit_u8,
                    &host_req_seq,
                    &host_req_flags,
                    &host_req_window_w_px,
                    &host_req_window_h_px,
                    &host_i32,
                    &host_f32,
                    &gfx_cmd_i32,
                    &gfx_cmd_f32,
                    &gfx_cmd_u8,
                    &last_req_seq);
            }

            stasis_data_poll_all();

            if (bulk_active && host_bulk_step && !render && gfx_cmd_i32 && gfx_cmd_f32 && gfx_cmd_u8)
            {
                int step_result = host_bulk_step(
                    host_i32,
                    host_f32,
                    gfx_cmd_i32,
                    gfx_cmd_f32,
                    gfx_cmd_u8,
                    host_req_seq,
                    host_req_flags,
                    host_req_window_w_px,
                    host_req_window_h_px,
                    tick);
                if (step_result != 0)
                {
                    result = step_result == 1 ? 0 : step_result;
                    break;
                }
            }
            else if (bulk_active)
            {
                if (host_get_frame)
                {
                    host_get_frame(host_i32, host_f32);
                }
                else
                {
                    fprintf(stderr, "error: manual tick/render path requires stasis_host_get_frame\n");
                    result = 1;
                    break;
                }
                if (host_i32 && host_i32[9] != 0)
                {
                    result = 0;
                    break;
                }
                if (host_bulk_apply_requests)
                {
                    host_bulk_apply_requests(host_req_seq, host_req_flags, host_req_window_w_px, host_req_window_h_px);
                }
                else if (host_req_seq && host_req_flags && *host_req_seq != last_req_seq)
                {
                    last_req_seq = *host_req_seq;
                }

                if (runner_diag && tick_diag_count < 10)
                {
                    fprintf(stderr, "RUNNER_DIAG: tick start %d\n", tick_diag_count);
                    fflush(stderr);
                }

                int tick_result = 0;
                if (tick)
                {
                    tick_result = tick();
                }
                if (runner_diag && tick_diag_count < 10)
                {
                    fprintf(stderr, "RUNNER_DIAG: tick end %d result=%d\n", tick_diag_count, tick_result);
                    fflush(stderr);
                    tick_diag_count++;
                }
                if (tick_result != 0)
                {
                    result = tick_result == 1 ? 0 : tick_result;
                    break;
                }
                if (render)
                {
                    int render_result = render();
                    if (render_result != 0)
                    {
                        result = render_result == 1 ? 0 : render_result;
                        break;
                    }
                }
                if (gfx_submit_u8 && gfx_cmd_i32 && gfx_cmd_f32 && gfx_cmd_u8)
                {
                    gfx_submit_u8(gfx_cmd_i32, gfx_cmd_f32, gfx_cmd_u8);
                }
            }
            else
            {
                fprintf(stderr, "error: HostFrame bulk globals became unavailable during tick loop\n");
                result = 1;
                break;
            }

            struct timespec ts_now;
            clock_gettime(CLOCK_MONOTONIC, &ts_now);
            long long now_us = ts_now.tv_sec * 1000000LL + ts_now.tv_nsec / 1000LL;
            long long elapsed_us = now_us - last_us;
            last_us = now_us;

            long long sleep_us = target_us - elapsed_us;
            while (sleep_us > 0)
            {
                if (swap_file_path && file_exists(swap_file_path))
                {
                    break;
                }

                long long ms = sleep_us / 1000LL;
                if (ms <= 0)
                {
                    ms = 1;
                }
                stasis_sleep_us(ms * 1000LL);
                sleep_us -= ms * 1000LL;
            }
        }
    }

    if (state_map_path)
    {
        free_state_map(&syms, &sym_count);
    }
    dlclose(lib);
    return result;
#endif
}
