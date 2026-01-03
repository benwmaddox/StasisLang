#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <inttypes.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#include <unistd.h>
#endif

#include "stasis_data.h"

typedef int (*stasis_entry_fn)(void);
typedef int (*stasis_tick_fn)(void);

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

static int copy_state_to_buffer(HMODULE lib, stasis_state_symbol *syms, uint32_t sym_count, uint8_t *buffer, uint32_t total_bytes)
{
    (void)total_bytes;
    for (uint32_t i = 0; i < sym_count; i++)
    {
        FARPROC addr = GetProcAddress(lib, syms[i].name);
        if (!addr)
        {
            fprintf(stderr, "error: state symbol not exported: %s\n", syms[i].name);
            return 1;
        }
        memcpy(buffer + syms[i].offset, (void *)addr, syms[i].size);
    }
    return 0;
}

static int copy_state_from_buffer(HMODULE lib, stasis_state_symbol *syms, uint32_t sym_count, const uint8_t *buffer, uint32_t total_bytes)
{
    (void)total_bytes;
    for (uint32_t i = 0; i < sym_count; i++)
    {
        FARPROC addr = GetProcAddress(lib, syms[i].name);
        if (!addr)
        {
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
    const char *hot_exit_path = NULL;
    const char *swap_file_path = NULL;
    const char *data_bind_json = NULL;
    const char *data_bind_meta = NULL;
    int fps = 60;

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

            set_runtime_dir(req_dll_path);

#ifdef _WIN32
            HMODULE lib = LoadLibraryA(req_dll_path);
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
        print_usage();
        return 1;
    }

    dll_path = argv[1];
    entry_name = argc >= 3 ? argv[2] : "run_tests";

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

    set_runtime_dir(dll_path);

#ifdef _WIN32
    HMODULE lib = LoadLibraryA(dll_path);
    if (!lib)
    {
        fprintf(stderr, "error: failed to load %s\n", dll_path);
        return 1;
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
    tick_name[0] = '\0';
    const char *sep = strstr(entry_name, "__");
    if (sep)
    {
        size_t prefix_len = (size_t)(sep - entry_name) + 2;
        if (prefix_len + 4 < sizeof(tick_name))
        {
            memcpy(tick_name, entry_name, prefix_len);
            memcpy(tick_name + prefix_len, "tick", 5);
        }
    }

    FARPROC tick_sym = NULL;
    if (tick_name[0] != '\0')
    {
        tick_sym = GetProcAddress(lib, tick_name);
    }

    stasis_entry_fn entry = (stasis_entry_fn)symbol;
    int result = entry();

    if (result == 0 && tick_sym)
    {
        stasis_tick_fn tick = (stasis_tick_fn)tick_sym;
        LARGE_INTEGER freq;
        QueryPerformanceFrequency(&freq);
        long long target_us = 1000000LL / (long long)fps;

        LARGE_INTEGER last;
        QueryPerformanceCounter(&last);

        for (;;)
        {
            /* Swap between ticks if requested. */
                    if (swap_file_path && file_exists(swap_file_path))
                    {
                        char new_path[2048];
                        if (read_text_file(swap_file_path, new_path, sizeof(new_path)) == 0)
                        {
                            DeleteFileA(swap_file_path);

                            LARGE_INTEGER sw_freq;
                            LARGE_INTEGER sw_t0;
                            LARGE_INTEGER sw_t1;
                            QueryPerformanceFrequency(&sw_freq);
                            char fixed_path[2048];
                            const char *load_path = new_path;
                            HMODULE old_lib = lib;

                            if (try_make_fixed_swap_path(new_path, fixed_path, sizeof(fixed_path)))
                            {
                                FreeLibrary(old_lib);
                                old_lib = NULL;
                                if (!MoveFileExA(new_path, fixed_path, MOVEFILE_REPLACE_EXISTING | MOVEFILE_COPY_ALLOWED))
                                {
                                    fprintf(stderr, "warning: failed to move hot-swap DLL to %s (err=%lu)\n", fixed_path, GetLastError());
                                    load_path = new_path;
                                }
                                else
                                {
                                    load_path = fixed_path;
                                }
                            }

                            fprintf(stderr, "HOTSWAP loading: %s\n", load_path);

                            uint8_t *buffer = NULL;
                            long long save_us = 0;
                            long long load_us = 0;
                            long long tick_us = 0;
                            long long restore_us = 0;
                            if (state_map_path)
                            {
                                buffer = (uint8_t *)malloc(total_bytes);
                                if (!buffer)
                                {
                                    fprintf(stderr, "error: out of memory\n");
                                    result = 1;
                                    break;
                                }
                                QueryPerformanceCounter(&sw_t0);
                                if (copy_state_to_buffer(lib, syms, sym_count, buffer, total_bytes) != 0)
                                {
                                    free(buffer);
                                    result = 1;
                                    break;
                                }
                                QueryPerformanceCounter(&sw_t1);
                                save_us = (sw_t1.QuadPart - sw_t0.QuadPart) * 1000000LL / sw_freq.QuadPart;
                            }

                            QueryPerformanceCounter(&sw_t0);
                            HMODULE new_lib = LoadLibraryA(load_path);
                            QueryPerformanceCounter(&sw_t1);
                            load_us = (sw_t1.QuadPart - sw_t0.QuadPart) * 1000000LL / sw_freq.QuadPart;
                            if (!new_lib)
                            {
                                fprintf(stderr, "error: failed to load %s\n", load_path);
                                free(buffer);
                                result = 1;
                                break;
                            }

                            QueryPerformanceCounter(&sw_t0);
                            FARPROC new_tick_sym = GetProcAddress(new_lib, tick_name);
                            QueryPerformanceCounter(&sw_t1);
                            tick_us = (sw_t1.QuadPart - sw_t0.QuadPart) * 1000000LL / sw_freq.QuadPart;
                            if (!new_tick_sym)
                            {
                                fprintf(stderr, "error: tick entrypoint %s not found in %s\n", tick_name, load_path);
                                FreeLibrary(new_lib);
                                free(buffer);
                                result = 1;
                                break;
                            }

                            if (buffer)
                            {
                                QueryPerformanceCounter(&sw_t0);
                                if (copy_state_from_buffer(new_lib, syms, sym_count, buffer, total_bytes) != 0)
                                {
                                    FreeLibrary(new_lib);
                                    free(buffer);
                                    result = 1;
                                    break;
                                }
                                QueryPerformanceCounter(&sw_t1);
                                restore_us = (sw_t1.QuadPart - sw_t0.QuadPart) * 1000000LL / sw_freq.QuadPart;
                                free(buffer);
                                fprintf(stderr, "HOTSWAP ok: save=%lldus load=%lldus tick=%lldus restore=%lldus bytes=%u symbols=%u\n", save_us, load_us, tick_us, restore_us, total_bytes, sym_count);
                            }
                            else
                            {
                                fprintf(stderr, "HOTSWAP ok: load=%lldus tick=%lldus\n", load_us, tick_us);
                            }

                            if (old_lib)
                            {
                                FreeLibrary(old_lib);
                            }
                            lib = new_lib;
                            tick = (stasis_tick_fn)new_tick_sym;

                            /* Update DLL handle for data binding system */
                            stasis_data_set_dll(new_lib);
                        }
                    }

            /* Poll data bindings for changes (fast path: just checks mtimes) */
            stasis_data_poll_all();

            int tick_result = tick();
            if (tick_result != 0)
            {
                result = tick_result == 1 ? 0 : tick_result;
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
    void *lib = dlopen(dll_path, RTLD_NOW);
    if (!lib)
    {
        fprintf(stderr, "error: failed to load %s: %s\n", dll_path, dlerror());
        return 1;
    }

    void *symbol = dlsym(lib, entry_name);
    if (!symbol)
    {
        fprintf(stderr, "error: entrypoint %s not found in %s\n", entry_name, dll_path);
        dlclose(lib);
        return 1;
    }

    stasis_entry_fn entry = (stasis_entry_fn)symbol;
    int result = entry();
    dlclose(lib);
    return result;
#endif
}
