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

typedef int (*stasis_entry_fn)(void);

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
    fprintf(stderr, "usage: stasis_runner <dll_path> [entry] --state <snapshot_path> --state-map <map_path> [--hot-exit-file <path>]\n");
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
    enable_vt_processing(GetStdHandle(STD_OUTPUT_HANDLE));
    enable_vt_processing(GetStdHandle(STD_ERROR_HANDLE));
#endif

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

            char *dll_path = (char *)malloc(dll_len + 1);
            char *entry_name = (char *)malloc(entry_len + 1);
            if (!dll_path || !entry_name)
            {
                free(dll_path);
                free(entry_name);
                fprintf(stderr, "ERR out of memory\n");
                fflush(stderr);
                continue;
            }

            if (fread(dll_path, 1, dll_len, stdin) != dll_len ||
                fread(entry_name, 1, entry_len, stdin) != entry_len)
            {
                free(dll_path);
                free(entry_name);
                fprintf(stderr, "ERR failed to read request\n");
                fflush(stderr);
                continue;
            }

            dll_path[dll_len] = '\0';
            entry_name[entry_len] = '\0';

            set_runtime_dir(dll_path);

#ifdef _WIN32
            HMODULE lib = LoadLibraryA(dll_path);
            if (!lib)
            {
                fprintf(stderr, "ERR failed to load\n");
                fflush(stderr);
                free(dll_path);
                free(entry_name);
                continue;
            }

            FARPROC symbol = GetProcAddress(lib, entry_name);
            if (!symbol)
            {
                fprintf(stderr, "ERR entrypoint not found\n");
                fflush(stderr);
                FreeLibrary(lib);
                free(dll_path);
                free(entry_name);
                continue;
            }

            stasis_entry_fn entry = (stasis_entry_fn)symbol;
            int result = entry();
            FreeLibrary(lib);
#else
            void *lib = dlopen(dll_path, RTLD_NOW);
            if (!lib)
            {
                fprintf(stderr, "ERR failed to load\n");
                fflush(stderr);
                free(dll_path);
                free(entry_name);
                continue;
            }

            void *symbol = dlsym(lib, entry_name);
            if (!symbol)
            {
                fprintf(stderr, "ERR entrypoint not found\n");
                fflush(stderr);
                dlclose(lib);
                free(dll_path);
                free(entry_name);
                continue;
            }

            stasis_entry_fn entry = (stasis_entry_fn)symbol;
            int result = entry();
            dlclose(lib);
#endif

            free(dll_path);
            free(entry_name);

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

    const char *dll_path = argv[1];
    const char *entry_name = argc >= 3 ? argv[2] : "run_tests";

    const char *state_path = NULL;
    const char *state_map_path = NULL;
    const char *hot_exit_path = NULL;
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
    }

    if ((state_path != NULL) != (state_map_path != NULL))
    {
        fprintf(stderr, "error: --state and --state-map must be provided together.\n");
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
    if (state_path && state_map_path)
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

    stasis_entry_fn entry = (stasis_entry_fn)symbol;
    int result = entry();

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
