#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#include <unistd.h>
#endif

typedef int (*stasis_entry_fn)(void);

static void print_usage(void)
{
    fprintf(stderr, "usage: stasis_runner <dll_path> [entry]\n");
    fprintf(stderr, "  entry defaults to run_tests (use main for run mode)\n");
}

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

int main(int argc, char **argv)
{
    if (argc < 2)
    {
        print_usage();
        return 1;
    }

    const char *dll_path = argv[1];
    const char *entry_name = argc >= 3 ? argv[2] : "run_tests";

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

    stasis_entry_fn entry = (stasis_entry_fn)symbol;
    int result = entry();
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
