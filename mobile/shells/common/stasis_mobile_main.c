#define SDL_MAIN_HANDLED
#include <SDL3/SDL.h>
#include <SDL3/SDL_main.h>

#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#if defined(_WIN32)
#include <io.h>
#endif

#include "published_aot_symbols.h"
#if defined(__has_include)
#if __has_include("stasis_package_provenance.h")
#include "stasis_package_provenance.h"
#else
#define STASIS_PACKAGE_BUILD_LABEL "production-monolith"
#define STASIS_PACKAGE_RELEASE_TAG "direct-bound"
#define STASIS_PACKAGE_SOURCE_COMMIT "unknown"
#endif
#else
#include "stasis_package_provenance.h"
#endif
#if defined(STASIS_ENABLE_SEAM_TESTS)
#include "stasis_mobile_aot_runtime.h"
int stasis_set_recording_audio_config(int enabled);
int stasis_audio_get_queued_frames(void);
int stasis_recording_audio_pull_f32_interleaved(float *output_stereo, int frame_count);
int stasis_audio_play(int asset_handle, int loop, float volume, float pan);
void stasis_audio_stop(int voice_handle);
int stasis_audio_voice_is_playing(int voice_handle);
#endif
#include "stasis_mobile_runtime.h"

void stasis_host_report_runtime_error(const char *message);
#if defined(__APPLE__) && !defined(__ANDROID__) && defined(STASIS_NETWORK_ENABLED)
void stasis_mobile_network_present_join_url(void);
#endif
#if defined(STASIS_ENABLE_SEAM_TESTS)
int stasis_test_get_render_submission_state(int32_t *out_i32, int32_t capacity);
int stasis_gfx_get_resource_lifecycle(int32_t *out_i32, int count);

static int32_t hash_global_path(const char *path) {
    uint32_t hash = 2166136261U;
    while (*path != '\0') {
        hash ^= (uint8_t)*path++;
        hash *= 16777619U;
    }
    return (int32_t)hash;
}

static int32_t seam_i32(const char *path) {
    return stasis_jit_global_i32_load(hash_global_path(path));
}

static float seam_f32(const char *path) {
    return stasis_jit_global_f32_load(hash_global_path(path));
}

static int seam_it021_audio = 0;
static int seam_audio_queued_before = 0;
static int seam_audio_queued_after = 0;
static int seam_audio_frames_mixed = 0;
static int seam_audio_nonzero_after_prefix = 0;
static int seam_audio_voice_state = 0;
static uint32_t seam_audio_sample_checksum = 0;
static uint32_t seam_audio_replay_checksum = 0;
static int seam_audio_replay_matches = 0;

static uint32_t checksum_audio(
    const float *output,
    int first_frame,
    int frame_count,
    int *nonzero_samples
) {
    uint32_t checksum = 2166136261U;
    *nonzero_samples = 0;
    for (int frame = first_frame; frame < frame_count; frame++) {
        for (int channel = 0; channel < 2; channel++) {
            float sample = output[frame * 2 + channel];
            if (sample > 0.0001f || sample < -0.0001f) (*nonzero_samples)++;
            int32_t quantized = (int32_t)(sample * 100000.0f);
            checksum ^= (uint32_t)quantized;
            checksum *= 16777619U;
        }
    }
    return checksum;
}

static void collect_it021_audio_telemetry(void) {
    float output[64] = {0};
    float replay_output[64] = {0};
    const int frame_count = 32;
    int asset_handle = seam_i32("seam_audio_handle");
    int voice_handle = seam_i32("seam_voice_handle");
    seam_audio_queued_before = stasis_audio_get_queued_frames();
    seam_audio_frames_mixed = stasis_recording_audio_pull_f32_interleaved(output, frame_count);
    seam_audio_queued_after = stasis_audio_get_queued_frames();
    int direct_prefix = seam_audio_queued_before;
    if (direct_prefix < 0) direct_prefix = 0;
    if (direct_prefix > seam_audio_frames_mixed) direct_prefix = seam_audio_frames_mixed;
    seam_audio_sample_checksum = checksum_audio(
        output,
        direct_prefix,
        seam_audio_frames_mixed,
        &seam_audio_nonzero_after_prefix
    );
    seam_audio_voice_state = stasis_audio_voice_is_playing(voice_handle);
    stasis_audio_stop(voice_handle);
    int replay_voice = stasis_audio_play(asset_handle, 0, 0.5f, 0.0f);
    int replay_frames = stasis_recording_audio_pull_f32_interleaved(replay_output, frame_count);
    int replay_nonzero = 0;
    seam_audio_replay_checksum = checksum_audio(
        replay_output,
        direct_prefix,
        replay_frames,
        &replay_nonzero
    );
    seam_audio_replay_matches = replay_voice > 0 && replay_frames == seam_audio_frames_mixed &&
        replay_nonzero == seam_audio_nonzero_after_prefix &&
        seam_audio_replay_checksum == seam_audio_sample_checksum;
    stasis_audio_stop(replay_voice);
}

static void log_seam_marker(const char *test_id, const char *event, int32_t frame) {
    int32_t render[7] = {0};
    int32_t lifecycle[6] = {0};
    int has_render = stasis_test_get_render_submission_state(render, 7);
    int has_lifecycle = stasis_gfx_get_resource_lifecycle(lifecycle, 6);
    int32_t checksum = seam_i32("seam_state_checksum");
    SDL_Log(
        "Stasis seam: {\"schema\":\"stasis.seam_test.v1\",\"test_id\":\"%s\","
        "\"event\":\"%s\",\"frame\":%d,\"state_checksum\":%d,"
        "\"accepted\":%d,\"rejected\":%d,\"presented\":%d,"
        "\"validation\":%d,\"command_trace\":%u,"
        "\"probe_sequence\":%d,\"probe_kind\":%d,\"probe_tick\":%d,"
        "\"pointer_id\":%d,\"pointer_count\":%d,\"is_down\":%d,"
        "\"went_down\":%d,\"went_up\":%d,\"down_count\":%d,"
        "\"move_count\":%d,\"up_count\":%d,\"state_transitions\":%d,"
        "\"input_phase\":%d,\"x\":%.3f,\"y\":%.3f,"
        "\"dx\":%.3f,\"dy\":%.3f,\"x_n\":%.4f,\"y_n\":%.4f,"
        "\"safe_x\":%.3f,\"safe_y\":%.3f,\"safe_w\":%.3f,\"safe_h\":%.3f,"
        "\"logical_w\":%.3f,\"logical_h\":%.3f,"
        "\"native_w\":%d,\"native_h\":%d,"
        "\"drawable_w\":%d,\"drawable_h\":%d,"
        "\"display_generation\":%d,\"density_generation\":%d,"
        "\"frame_display_generation\":%d,\"frame_density_generation\":%d,"
        "\"content_scale\":%.4f,\"raster_scale\":%.4f,"
        "\"resource_state\":%d,\"surface_generation\":%d,"
        "\"renderer_generation\":%d,\"restore_attempts\":%d,"
        "\"restore_failures\":%d,\"restore_reason\":%d,"
        "\"asset_root\":\"%s\",\"asset_manifest_sha256\":\"%s\","
        "\"sprite_handle\":%d,\"font_handle\":%d,\"cached_text_handle\":%d,"
        "\"audio_handle\":%d,\"voice_handle\":%d,"
        "\"direct_text_width\":%.3f,\"cached_text_width\":%.3f,"
        "\"audio_queued_before\":%d,\"audio_queued_after\":%d,"
        "\"audio_frames_mixed\":%d,\"audio_nonzero_after_prefix\":%d,"
        "\"audio_voice_state\":%d,\"audio_sample_checksum\":%u,"
        "\"audio_replay_checksum\":%u,\"audio_replay_matches\":%d}",
        test_id,
        event,
        frame,
        checksum,
        has_render ? render[0] : 0,
        has_render ? render[1] : 0,
        has_render ? render[2] : 0,
        has_render ? render[3] : 0,
        has_render ? (uint32_t)render[4] : 0U,
        seam_i32("seam_probe_sequence"),
        seam_i32("seam_probe_kind"),
        seam_i32("seam_probe_tick"),
        seam_i32("seam_pointer_id"),
        seam_i32("seam_pointer_count"),
        seam_i32("seam_pointer_is_down"),
        seam_i32("seam_pointer_went_down"),
        seam_i32("seam_pointer_went_up"),
        seam_i32("seam_down_count"),
        seam_i32("seam_move_count"),
        seam_i32("seam_up_count"),
        seam_i32("seam_state_transitions"),
        seam_i32("seam_input_phase"),
        seam_f32("seam_pointer_x"),
        seam_f32("seam_pointer_y"),
        seam_f32("seam_pointer_dx"),
        seam_f32("seam_pointer_dy"),
        seam_f32("seam_pointer_x_n"),
        seam_f32("seam_pointer_y_n"),
        seam_f32("seam_safe_x"),
        seam_f32("seam_safe_y"),
        seam_f32("seam_safe_w"),
        seam_f32("seam_safe_h"),
        seam_f32("seam_logical_w"),
        seam_f32("seam_logical_h"),
        seam_i32("seam_native_w"),
        seam_i32("seam_native_h"),
        seam_i32("seam_drawable_w"),
        seam_i32("seam_drawable_h"),
        seam_i32("seam_display_generation"),
        seam_i32("seam_density_generation"),
        has_render ? render[5] : 0,
        has_render ? render[6] : 0,
        seam_f32("seam_content_scale"),
        seam_f32("seam_raster_scale"),
        has_lifecycle ? lifecycle[0] : 0,
        has_lifecycle ? lifecycle[1] : 0,
        has_lifecycle ? lifecycle[2] : 0,
        has_lifecycle ? lifecycle[3] : 0,
        has_lifecycle ? lifecycle[4] : 0,
        has_lifecycle ? lifecycle[5] : 0,
        SDL_getenv("STASIS_ASSET_ROOT") ? SDL_getenv("STASIS_ASSET_ROOT") : "",
        SDL_getenv("STASIS_ASSET_MANIFEST_SHA256") ? SDL_getenv("STASIS_ASSET_MANIFEST_SHA256") : "",
        seam_i32("seam_sprite_handle"),
        seam_i32("seam_font_handle"),
        seam_i32("seam_cached_text_handle"),
        seam_i32("seam_audio_handle"),
        seam_i32("seam_voice_handle"),
        seam_f32("seam_direct_text_width"),
        seam_f32("seam_cached_text_width"),
        seam_audio_queued_before,
        seam_audio_queued_after,
        seam_audio_frames_mixed,
        seam_audio_nonzero_after_prefix,
        seam_audio_voice_state,
        seam_audio_sample_checksum,
        seam_audio_replay_checksum,
        seam_audio_replay_matches
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
#elif defined(_WIN32) && defined(STASIS_WINDOWS_MONOLITH)
    const char *base = SDL_GetBasePath();
    char path[1024];
    int written;
    if (base == NULL) {
        return -1;
    }
    written = snprintf(path, sizeof(path), "%sapp", base);
    if (written < 0 || (size_t)written >= sizeof(path)) {
        return -1;
    }
    return _putenv_s("STASIS_ASSET_ROOT", _access(path, 0) == 0 ? path : ".");
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
    seam_it021_audio = seam_test_id != NULL && strcmp(seam_test_id, "IT-021") == 0;
    if (seam_it021_audio && !stasis_set_recording_audio_config(1)) {
        SDL_Log("Stasis IT-021 failed to enable recording audio configuration");
    }
#endif
    int status = stasis_mobile_runtime_initialize(&config, &game);
    if (status != STASIS_MOBILE_RUNTIME_OK) {
        report_runtime_status("Stasis mobile initialization", status);
        SDL_Log("Stasis mobile initialization stopped with status %d", status);
    } else {
#if defined(__APPLE__) && !defined(__ANDROID__) && defined(STASIS_NETWORK_ENABLED)
        stasis_mobile_network_present_join_url();
#endif
#if defined(STASIS_ENABLE_SEAM_TESTS)
        if (seam_it021_audio) collect_it021_audio_telemetry();
        if (seam_test_id != NULL && seam_test_id[0] != '\0') {
            log_seam_marker(seam_test_id, "initialized", 0);
        }
#endif
    }
    StasisMobileFramePacer frame_pacer;
#if defined(STASIS_ENABLE_SEAM_TESTS)
    int32_t frame = 0;
    int32_t last_probe_sequence = 0;
    int32_t last_lifecycle[6] = {-1, -1, -1, -1, -1, -1};
#endif
    stasis_mobile_frame_pacer_reset(&frame_pacer, SDL_GetTicksNS());
    while (status == STASIS_MOBILE_RUNTIME_OK) {
        status = stasis_mobile_runtime_step();
        if (status == STASIS_MOBILE_RUNTIME_OK) {
#if defined(STASIS_ENABLE_SEAM_TESTS)
            frame++;
            if (seam_test_id != NULL && (frame == 1 || (frame > 0 && frame % 30 == 0))) {
                log_seam_marker(seam_test_id, frame == 1 ? "frame" : "stable", frame);
            }
            if (seam_test_id != NULL) {
                int32_t lifecycle[6] = {0};
                if (stasis_gfx_get_resource_lifecycle(lifecycle, 6)) {
                    int changed = 0;
                    for (int index = 0; index < 6; index++) {
                        if (lifecycle[index] != last_lifecycle[index]) {
                            changed = 1;
                            last_lifecycle[index] = lifecycle[index];
                        }
                    }
                    if (changed) {
                        log_seam_marker(seam_test_id, "lifecycle", frame);
                    }
                }
                int32_t probe_sequence = seam_i32("seam_probe_sequence");
                if (probe_sequence != last_probe_sequence) {
                    log_seam_marker(seam_test_id, "probe", frame);
                    last_probe_sequence = probe_sequence;
                }
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

#if defined(_WIN32) && defined(STASIS_WINDOWS_MONOLITH)
int main(int argc, char **argv) {
    return SDL_main(argc, argv);
}
#endif
