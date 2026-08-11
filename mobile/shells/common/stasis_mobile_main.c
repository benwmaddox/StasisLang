#define SDL_MAIN_HANDLED
#include <SDL3/SDL.h>
#include <SDL3/SDL_main.h>

#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>

#include "published_aot_symbols.h"
#include "stasis_package_provenance.h"
#if defined(STASIS_ENABLE_SEAM_TESTS)
#include "stasis_mobile_aot_runtime.h"
#endif
#include "stasis_mobile_runtime.h"

void stasis_host_report_runtime_error(const char *message);
#if defined(STASIS_ENABLE_SEAM_TESTS)
int stasis_test_get_render_submission_state(int32_t *out_i32, int32_t capacity);

static int32_t hash_global_path(const char *path) {
    uint32_t hash = 2166136261U;
    while (*path != '\0') {
        hash ^= (uint8_t)*path++;
        hash *= 16777619U;
    }
    return (int32_t)hash;
}

static void log_seam_marker(const char *test_id, const char *event, int32_t frame) {
    int32_t render[5] = {0};
    int has_render = stasis_test_get_render_submission_state(render, 5);
    int32_t checksum = stasis_jit_global_i32_load(hash_global_path("seam_state_checksum"));
    SDL_Log(
        "Stasis seam: {\"schema\":\"stasis.seam_test.v1\",\"test_id\":\"%s\","
        "\"event\":\"%s\",\"frame\":%d,\"state_checksum\":%d,"
        "\"accepted\":%d,\"rejected\":%d,\"presented\":%d,"
        "\"validation\":%d,\"command_trace\":%u}",
        test_id,
        event,
        frame,
        checksum,
        has_render ? render[0] : 0,
        has_render ? render[1] : 0,
        has_render ? render[2] : 0,
        has_render ? render[3] : 0,
        has_render ? (uint32_t)render[4] : 0U
    );
}
#endif

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
#if defined(STASIS_ENABLE_SEAM_TESTS)
    const char *seam_test_id = SDL_getenv("STASIS_SEAM_TEST_ID");
#endif
    int status = stasis_mobile_runtime_initialize(&config, &game);
    if (status != STASIS_MOBILE_RUNTIME_OK) {
        report_runtime_status("Stasis mobile initialization", status);
        SDL_Log("Stasis mobile initialization stopped with status %d", status);
    }
#if defined(STASIS_ENABLE_SEAM_TESTS)
    else if (seam_test_id != NULL && seam_test_id[0] != '\0') {
        log_seam_marker(seam_test_id, "initialized", 0);
    }
#endif
    StasisMobileFramePacer frame_pacer;
#if defined(STASIS_ENABLE_SEAM_TESTS)
    int32_t frame = 0;
#endif
    stasis_mobile_frame_pacer_reset(&frame_pacer, SDL_GetTicksNS());
    while (status == STASIS_MOBILE_RUNTIME_OK) {
        status = stasis_mobile_runtime_step();
        if (status == STASIS_MOBILE_RUNTIME_OK) {
#if defined(STASIS_ENABLE_SEAM_TESTS)
            frame++;
            if (seam_test_id != NULL && (frame == 1 || frame == 30)) {
                log_seam_marker(seam_test_id, frame == 30 ? "stable" : "frame", frame);
            }
#endif
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
