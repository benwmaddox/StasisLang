#define SDL_MAIN_HANDLED 1
#include <SDL.h>

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <dlfcn.h>
#include <unistd.h>

// Stasis program entrypoints (compiled from brickout_revenge.stasis).
extern int main(void);
extern int tick(void);

// Runtime API (provided by runtime/stasis_graphics.c).
extern int stasis_should_quit(void);
extern void stasis_data_set_dll(void* dll);
extern int stasis_data_bind(const char* json_file_path, const char* struct_meta_path);
extern int stasis_data_has_error(int handle);
extern const char* stasis_data_get_error(int handle);

static void stasis_android_bind_config(void)
{
    void* self = dlopen("libmain.so", RTLD_NOW);
    if (!self)
    {
        SDL_Log("stasis: dlopen self failed: %s", dlerror());
        return;
    }

    stasis_data_set_dll(self);
    const char* data_path = "samples/brickout_revenge/data/config.json";
    const char* meta_path = "samples/brickout_revenge/data/brickout_revenge.struct-meta.json";
    int handle = stasis_data_bind(data_path, meta_path);
    if (handle == 0)
    {
        SDL_Log("stasis: data bind failed (%s, %s)", data_path, meta_path);
        return;
    }

    if (stasis_data_has_error(handle))
    {
        SDL_Log("stasis: data bind error: %s", stasis_data_get_error(handle));
    }

    int* magic = (int*)dlsym(self, "state_config_magic");
    if (magic)
    {
        SDL_Log("stasis: state_config_magic=%d", *magic);
    }
    else
    {
        SDL_Log("stasis: state_config_magic symbol not found");
    }
}

static void stasis_android_set_cwd(void)
{
    const char *external = SDL_AndroidGetExternalStoragePath();
    const char *internal = SDL_AndroidGetInternalStoragePath();
    const char *base = external && external[0] ? external : internal;
    if (!base || !base[0])
    {
        SDL_Log("stasis: no storage path available (external/internal empty)");
        return;
    }

    if (chdir(base) != 0)
    {
        SDL_Log("stasis: chdir('%s') failed: %s", base, strerror(errno));
        return;
    }

    SDL_Log("stasis: cwd set to %s", base);
}

int SDL_main(int argc, char **argv)
{
    (void)argc;
    (void)argv;

    stasis_android_set_cwd();
    stasis_android_bind_config();

    // Brickout Revenge uses main() for initialization and tick() for per-frame work.
    // Convention:
    // - tick() returns 0 to continue
    // - tick() returns 1 to exit cleanly
    int init_code = main();
    if (init_code != 0)
    {
        SDL_Log("stasis: main() returned %d", init_code);
        return init_code;
    }

    for (;;)
    {
        if (stasis_should_quit())
        {
            return 0;
        }

        int code = tick();
        if (code == 0)
        {
            continue;
        }
        if (code == 1)
        {
            return 0;
        }
        return code;
    }
}
