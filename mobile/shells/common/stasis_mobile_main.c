#define SDL_MAIN_HANDLED
#include <SDL3/SDL.h>
#include <SDL3/SDL_main.h>

#include <stdio.h>
#include <stdlib.h>

#include "published_aot_symbols.h"
#include "stasis_package_provenance.h"
#include "stasis_mobile_runtime.h"

void stasis_host_report_runtime_error(const char *message);

static void report_runtime_status(const char *stage, int status) {
    char message[256];
    snprintf(message, sizeof(message), "%s stopped with status %d", stage, status);
    stasis_host_report_runtime_error(message);
}

static int configure_asset_root(void) {
#if defined(__APPLE__) && !defined(__ANDROID__)
    const char *base = SDL_GetBasePath();
    if (base == NULL) {
        return -1;
    }
    char path[1024];
    int written = snprintf(
        path,
        sizeof(path),
        "%sstasis_game/@STASIS_ASSET_BASE@",
        base
    );
    if (written < 0 || (size_t)written >= sizeof(path)) {
        return -1;
    }
    return setenv("STASIS_ASSET_ROOT", path, 1);
#else
    return 0;
#endif
}

int SDL_main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    if (configure_asset_root() != 0) {
        stasis_host_report_runtime_error("Stasis could not configure the bundled asset root");
        SDL_Log("Stasis could not configure the bundled asset root");
        return STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT;
    }
    SDL_Log(
        "Stasis provenance: %s tag=%s commit=%s renderer=gfx_cmd_v1",
        STASIS_PACKAGE_BUILD_LABEL,
        STASIS_PACKAGE_RELEASE_TAG,
        STASIS_PACKAGE_SOURCE_COMMIT
    );
    StasisMobileGameEntries game = {
        STASIS_AOT_BIND_RUNTIME_GLOBALS,
        STASIS_AOT_MAIN,
        STASIS_AOT_TICK,
        STASIS_AOT_RENDER,
    };
    StasisMobileRuntimeConfig config = {1280, 720, "@STASIS_APP_NAME@"};
    int status = stasis_mobile_runtime_initialize(&config, &game);
    if (status != STASIS_MOBILE_RUNTIME_OK) {
        report_runtime_status("Stasis mobile initialization", status);
        SDL_Log("Stasis mobile initialization stopped with status %d", status);
    }
    StasisMobileFramePacer frame_pacer;
    stasis_mobile_frame_pacer_reset(&frame_pacer, SDL_GetTicksNS());
    while (status == STASIS_MOBILE_RUNTIME_OK) {
        status = stasis_mobile_runtime_step();
        if (status == STASIS_MOBILE_RUNTIME_OK) {
            uint64_t wait_ns = stasis_mobile_frame_pacer_wait_ns(
                &frame_pacer,
                SDL_GetTicksNS()
            );
            if (wait_ns > 0) {
                SDL_DelayPrecise(wait_ns);
            }
        }
    }
    SDL_Log("Stasis mobile loop stopped with status %d", status);
    int32_t game_result = stasis_mobile_runtime_last_entry_result();
    stasis_mobile_runtime_shutdown();
    if (game_result != 0) {
        report_runtime_status("Stasis game entry", game_result);
        SDL_Log("Stasis game entry requested stop with code %d", game_result);
        return game_result;
    }
    if (status != STASIS_MOBILE_RUNTIME_STOP_REQUESTED) {
        report_runtime_status("Stasis mobile loop", status);
    }
    return status == STASIS_MOBILE_RUNTIME_STOP_REQUESTED ? 0 : status;
}
