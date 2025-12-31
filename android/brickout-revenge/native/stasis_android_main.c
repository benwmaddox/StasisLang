#define SDL_MAIN_HANDLED 1
#include <SDL.h>

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

// Stasis program entrypoints (compiled from brickout_revenge.stasis).
extern int main(void);
extern int tick(void);

// Runtime API (provided by runtime/stasis_graphics.c).
extern int stasis_should_quit(void);

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
