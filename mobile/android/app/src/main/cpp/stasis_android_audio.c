#include "stasis_android_audio.h"
#include "stasis_asset_path.h"
#include "stasis_audio_assets.h"
#include "stasis_audio_ring.h"

#include <aaudio/AAudio.h>
#include <android/log.h>
#include <dlfcn.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define STASIS_AUDIO_LOG_TAG "StasisAudio"
#define STASIS_AUDIO_MAX_FRAMES 32768
#define STASIS_AUDIO_MAX_CHANNELS 2
#define STASIS_AUDIO_DEFAULT_RATE 48000
#define STASIS_AUDIO_DEFAULT_CHANNELS 2
#define STASIS_AUDIO_DEFAULT_LATENCY 2048
#define STASIS_AUDIO_PROJECT_ROOT_SIZE 1024
#define STASIS_AUDIO_RESOLVED_PATH_SIZE 2048

typedef struct StasisAndroidAudioContext {
    AAudioStream *stream;
    StasisAudioRing ring;
    float storage[STASIS_AUDIO_MAX_FRAMES * STASIS_AUDIO_MAX_CHANNELS];
    int sample_rate;
    int channels;
    int target_latency;
    _Atomic uint32_t users;
    _Atomic uint32_t callbacks;
    _Atomic int retired;
    _Atomic int started;
    _Atomic int error;
} StasisAndroidAudioContext;

static _Atomic(StasisAndroidAudioContext *) audio_context;
static atomic_flag audio_context_lock = ATOMIC_FLAG_INIT;
static _Atomic int audio_paused;
static _Atomic int audio_focused;
static _Atomic int audio_attempted;
static _Atomic int audio_init_error;
static pthread_mutex_t audio_assets_lock = PTHREAD_MUTEX_INITIALIZER;
static StasisAudioAssetStore audio_assets;
static char audio_project_root[STASIS_AUDIO_PROJECT_ROOT_SIZE];

static int audio_initialize(
    int sample_rate,
    int channels,
    int target_latency_frames,
    int reset_assets
);

static void audio_assets_reset_locked(int clear_project_root) {
    stasis_audio_assets_reset(&audio_assets);
    if (clear_project_root) audio_project_root[0] = '\0';
}

static void audio_assets_ensure_initialized_locked(void) {
    if (audio_assets.next_asset_handle <= 0 || audio_assets.next_voice_handle <= 0) {
        stasis_audio_assets_reset(&audio_assets);
    }
}

static int audio_guest_path_escapes_root(const char *path) {
    const char *cursor = path;
    size_t depth = 0;
    if (!path) return 1;
    while (*cursor != '\0') {
        const char *segment;
        size_t segment_len;
        while (*cursor == '/' || *cursor == '\\') cursor += 1;
        if (*cursor == '\0') break;
        segment = cursor;
        while (*cursor != '\0' && *cursor != '/' && *cursor != '\\') cursor += 1;
        segment_len = (size_t)(cursor - segment);
        if (segment_len == 1 && segment[0] == '.') continue;
        if (segment_len == 2 && segment[0] == '.' && segment[1] == '.') {
            if (depth == 0) return 1;
            depth -= 1;
            continue;
        }
        depth += 1;
    }
    return 0;
}

static int audio_resolve_path_locked(
    const char *path,
    char *resolved,
    size_t resolved_size
) {
    char normalized[STASIS_AUDIO_RESOLVED_PATH_SIZE];
    int written;
    if (!audio_project_root[0] || !path || !*path ||
        audio_guest_path_escapes_root(path) ||
        !stasis_asset_normalize_relative_path(path, normalized, sizeof(normalized))) {
        return 0;
    }
    written = snprintf(resolved, resolved_size, "%s/%s", audio_project_root, normalized);
    return written > 0 && (size_t)written < resolved_size;
}

static void audio_assets_reset(int clear_project_root) {
    pthread_mutex_lock(&audio_assets_lock);
    audio_assets_reset_locked(clear_project_root);
    pthread_mutex_unlock(&audio_assets_lock);
}

typedef void (*StasisAaudioSetBuilderIntFn)(AAudioStreamBuilder *, int32_t);

static void audio_set_builder_metadata(AAudioStreamBuilder *builder) {
    void *aaudio_library = dlopen("libaaudio.so", RTLD_NOW | RTLD_LOCAL);
    StasisAaudioSetBuilderIntFn set_usage;
    StasisAaudioSetBuilderIntFn set_content_type;
    if (aaudio_library == NULL) return;
    set_usage = (StasisAaudioSetBuilderIntFn)dlsym(
        aaudio_library, "AAudioStreamBuilder_setUsage");
    set_content_type = (StasisAaudioSetBuilderIntFn)dlsym(
        aaudio_library, "AAudioStreamBuilder_setContentType");
    if (set_usage != NULL) set_usage(builder, (int32_t)AAUDIO_USAGE_GAME);
    if (set_content_type != NULL) {
        set_content_type(builder, (int32_t)AAUDIO_CONTENT_TYPE_MUSIC);
    }
}

static void audio_lock_context(void) {
    while (atomic_flag_test_and_set_explicit(&audio_context_lock, memory_order_acquire)) {
        sched_yield();
    }
}

static void audio_unlock_context(void) {
    atomic_flag_clear_explicit(&audio_context_lock, memory_order_release);
}

static StasisAndroidAudioContext *audio_context_acquire(void) {
    StasisAndroidAudioContext *context;
    audio_lock_context();
    context = atomic_load_explicit(&audio_context, memory_order_relaxed);
    if (context != NULL) {
        atomic_fetch_add_explicit(&context->users, 1, memory_order_acquire);
    }
    audio_unlock_context();
    return context;
}

static void audio_context_release(StasisAndroidAudioContext *context) {
    atomic_fetch_sub_explicit(&context->users, 1, memory_order_release);
}

static int audio_callback_enter(StasisAndroidAudioContext *context) {
    atomic_fetch_add_explicit(&context->callbacks, 1, memory_order_acquire);
    if (atomic_load_explicit(&context->retired, memory_order_acquire)) {
        atomic_fetch_sub_explicit(&context->callbacks, 1, memory_order_release);
        return 0;
    }
    return 1;
}

static void audio_callback_leave(StasisAndroidAudioContext *context) {
    atomic_fetch_sub_explicit(&context->callbacks, 1, memory_order_release);
}

static aaudio_data_callback_result_t audio_data_callback(
    AAudioStream *stream,
    void *user_data,
    void *audio_data,
    int32_t num_frames
) {
    StasisAndroidAudioContext *context = user_data;
    (void)stream;
    if (!audio_callback_enter(context)) {
        if (num_frames > 0) {
            memset(audio_data, 0,
                (size_t)num_frames * (size_t)context->channels * sizeof(float));
        }
        return AAUDIO_CALLBACK_RESULT_STOP;
    }
    if (num_frames > 0) {
        stasis_audio_ring_consume(&context->ring,
            (float *)audio_data, (uint32_t)num_frames);
        /* The device callback must never wait behind a guest-side asset load or
         * control mutation. A missed try-lock leaves the ring output intact and
         * the next callback gets another chance to mix decoded voices. */
        if (context->channels == 2 &&
            pthread_mutex_trylock(&audio_assets_lock) == 0) {
            stasis_audio_assets_mix(&audio_assets, (float *)audio_data,
                num_frames, context->sample_rate);
            pthread_mutex_unlock(&audio_assets_lock);
        }
    }
    audio_callback_leave(context);
    return AAUDIO_CALLBACK_RESULT_CONTINUE;
}

static void audio_error_callback(
    AAudioStream *stream,
    void *user_data,
    aaudio_result_t error
) {
    StasisAndroidAudioContext *context = user_data;
    (void)stream;
    if (!audio_callback_enter(context)) return;
    atomic_store_explicit(&context->started, 0, memory_order_release);
    atomic_store_explicit(&context->error, (int)error, memory_order_release);
    stasis_audio_ring_set_accepting(&context->ring, 0);
    __android_log_print(ANDROID_LOG_ERROR, STASIS_AUDIO_LOG_TAG,
        "AAudio stream error: %s", AAudio_convertResultToText(error));
    audio_callback_leave(context);
}

static int audio_should_run(void) {
    return !atomic_load_explicit(&audio_paused, memory_order_acquire) &&
        atomic_load_explicit(&audio_focused, memory_order_acquire);
}

static void audio_update_running_state(void) {
    StasisAndroidAudioContext *context = audio_context_acquire();
    aaudio_result_t result;
    int rate;
    int channels;
    int latency;
    if (context == NULL) return;
    if (audio_should_run()) {
        if (atomic_load_explicit(&context->error, memory_order_acquire) != 0) {
            rate = context->sample_rate;
            channels = context->channels;
            latency = context->target_latency;
            audio_context_release(context);
            audio_initialize(rate, channels, latency, 0);
            return;
        }
        if (!atomic_load_explicit(&context->retired, memory_order_acquire) &&
            !atomic_load_explicit(&context->started, memory_order_acquire)) {
            result = AAudioStream_requestStart(context->stream);
            if (result == AAUDIO_OK) {
                audio_lock_context();
                if (context == atomic_load_explicit(&audio_context, memory_order_relaxed) &&
                    !atomic_load_explicit(&context->retired, memory_order_acquire)) {
                    atomic_store_explicit(&context->started, 1, memory_order_release);
                    stasis_audio_ring_set_accepting(&context->ring, 1);
                }
                audio_unlock_context();
            } else {
                atomic_store_explicit(&context->error, (int)result, memory_order_release);
                __android_log_print(ANDROID_LOG_ERROR, STASIS_AUDIO_LOG_TAG,
                    "AAudio start failed: %s", AAudio_convertResultToText(result));
            }
        }
    } else {
        stasis_audio_ring_set_accepting(&context->ring, 0);
        if (atomic_exchange_explicit(&context->started, 0, memory_order_acq_rel)) {
            AAudioStream_requestPause(context->stream);
            AAudioStream_requestFlush(context->stream);
        }
    }
    audio_context_release(context);
}

static void audio_retire_context(StasisAndroidAudioContext *context) {
    if (context == NULL) return;
    atomic_store_explicit(&context->retired, 1, memory_order_release);
    stasis_audio_ring_set_accepting(&context->ring, 0);
    while (atomic_load_explicit(&context->users, memory_order_acquire) != 0) {
        sched_yield();
    }
    AAudioStream_requestStop(context->stream);
    AAudioStream_close(context->stream);
    while (atomic_load_explicit(&context->callbacks, memory_order_acquire) != 0) {
        sched_yield();
    }
    free(context);
}

static void audio_close_context(void) {
    StasisAndroidAudioContext *context;
    audio_lock_context();
    context = atomic_exchange_explicit(&audio_context, NULL, memory_order_relaxed);
    if (context != NULL) {
        atomic_store_explicit(&context->retired, 1, memory_order_release);
    }
    audio_unlock_context();
    audio_retire_context(context);
}

static int audio_initialize(
    int sample_rate,
    int channels,
    int target_latency_frames,
    int reset_assets
) {
    AAudioStreamBuilder *builder = NULL;
    StasisAndroidAudioContext *context = NULL;
    aaudio_result_t result = AAUDIO_ERROR_INTERNAL;
    int32_t burst;
    uint32_t capacity;
    if (sample_rate < 8000 || sample_rate > 192000 ||
        (channels != 1 && channels != 2) || target_latency_frames <= 0) {
        return 0;
    }
    audio_close_context();
    if (reset_assets) audio_assets_reset(0);
    atomic_store_explicit(&audio_attempted, 1, memory_order_release);
    context = calloc(1, sizeof(*context));
    if (context == NULL) goto fail;
    context->channels = channels;
    result = AAudio_createStreamBuilder(&builder);
    if (result != AAUDIO_OK || builder == NULL) goto fail;
    AAudioStreamBuilder_setDirection(builder, AAUDIO_DIRECTION_OUTPUT);
    AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_FLOAT);
    audio_set_builder_metadata(builder);
    AAudioStreamBuilder_setSampleRate(builder, sample_rate);
    AAudioStreamBuilder_setChannelCount(builder, channels);
    AAudioStreamBuilder_setPerformanceMode(builder, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
    AAudioStreamBuilder_setSharingMode(builder, AAUDIO_SHARING_MODE_SHARED);
    AAudioStreamBuilder_setDataCallback(builder, audio_data_callback, context);
    AAudioStreamBuilder_setErrorCallback(builder, audio_error_callback, context);
    result = AAudioStreamBuilder_openStream(builder, &context->stream);
    AAudioStreamBuilder_delete(builder);
    builder = NULL;
    if (result != AAUDIO_OK || context->stream == NULL) goto fail;
    context->sample_rate = AAudioStream_getSampleRate(context->stream);
    context->channels = AAudioStream_getChannelCount(context->stream);
    context->target_latency = target_latency_frames;
    if (context->sample_rate <= 0 || context->channels != channels ||
        AAudioStream_getFormat(context->stream) != AAUDIO_FORMAT_PCM_FLOAT) {
        result = AAUDIO_ERROR_INVALID_FORMAT;
        goto fail;
    }
    burst = AAudioStream_getFramesPerBurst(context->stream);
    capacity = (uint32_t)target_latency_frames * 4U;
    if (burst > 0 && capacity < (uint32_t)burst * 4U) capacity = (uint32_t)burst * 4U;
    if (capacity > STASIS_AUDIO_MAX_FRAMES) capacity = STASIS_AUDIO_MAX_FRAMES;
    if (capacity < 256U) capacity = 256U;
    if (!stasis_audio_ring_initialize(&context->ring,
            context->storage, capacity, context->channels)) {
        result = AAUDIO_ERROR_INTERNAL;
        goto fail;
    }
    atomic_store_explicit(&audio_init_error, 0, memory_order_release);
    audio_lock_context();
    atomic_store_explicit(&audio_context, context, memory_order_relaxed);
    audio_unlock_context();
    audio_update_running_state();
    __android_log_print(ANDROID_LOG_INFO, STASIS_AUDIO_LOG_TAG,
        "AAudio opened rate=%d channels=%d capacity_frames=%u",
        context->sample_rate, context->channels, capacity);
    return 1;

fail:
    if (builder != NULL) AAudioStreamBuilder_delete(builder);
    if (context != NULL && context->stream != NULL) {
        audio_retire_context(context);
        context = NULL;
    }
    free(context);
    atomic_store_explicit(&audio_init_error, (int)result, memory_order_release);
    __android_log_print(ANDROID_LOG_ERROR, STASIS_AUDIO_LOG_TAG,
        "AAudio initialization failed: %s", AAudio_convertResultToText(result));
    return 0;
}

int stasis_audio_init(int sample_rate, int channels, int target_latency_frames) {
    /* Explicit guest reinitialization clears decoded voices but preserves the
     * project root installed immediately before main() calls audio_init(). */
    return audio_initialize(sample_rate, channels, target_latency_frames, 1);
}

void stasis_audio_shutdown(void) {
    audio_close_context();
    audio_assets_reset(1);
    atomic_store_explicit(&audio_attempted, 0, memory_order_release);
}

int stasis_audio_is_available(void) {
    StasisAndroidAudioContext *context = audio_context_acquire();
    int error;
    int rate;
    int channels;
    int latency;
    if (context == NULL) {
        if (atomic_load_explicit(&audio_attempted, memory_order_acquire)) return 0;
        return stasis_audio_init(STASIS_AUDIO_DEFAULT_RATE,
            STASIS_AUDIO_DEFAULT_CHANNELS, STASIS_AUDIO_DEFAULT_LATENCY);
    }
    error = atomic_load_explicit(&context->error, memory_order_acquire);
    rate = context->sample_rate;
    channels = context->channels;
    latency = context->target_latency;
    audio_context_release(context);
    if (error != 0 && audio_should_run()) {
        return audio_initialize(rate, channels, latency, 0);
    }
    return 1;
}

int stasis_audio_get_sample_rate(void) {
    StasisAndroidAudioContext *context;
    int value = 0;
    if (!stasis_audio_is_available()) return 0;
    context = audio_context_acquire();
    if (context != NULL) {
        value = context->sample_rate;
        audio_context_release(context);
    }
    return value;
}

int stasis_audio_get_channels(void) {
    StasisAndroidAudioContext *context;
    int value = 0;
    if (!stasis_audio_is_available()) return 0;
    context = audio_context_acquire();
    if (context != NULL) {
        value = context->channels;
        audio_context_release(context);
    }
    return value;
}

int stasis_audio_get_queued_frames(void) {
    StasisAndroidAudioContext *context = audio_context_acquire();
    int value = 0;
    if (context != NULL) {
        value = (int)stasis_audio_ring_queued_frames(&context->ring);
        audio_context_release(context);
    }
    return value;
}

int stasis_audio_get_underruns(void) {
    StasisAndroidAudioContext *context = audio_context_acquire();
    int value = 0;
    if (context != NULL) {
        value = (int)stasis_audio_ring_underruns(&context->ring);
        audio_context_release(context);
    }
    return value;
}

int stasis_audio_push_f32_interleaved(const float *samples, int frame_count) {
    StasisAndroidAudioContext *context;
    int value = 0;
    if (!stasis_audio_is_available() || samples == NULL || frame_count <= 0) return 0;
    context = audio_context_acquire();
    if (context != NULL) {
        value = (int)stasis_audio_ring_push(
            &context->ring, samples, (uint32_t)frame_count);
        audio_context_release(context);
    }
    return value;
}

int stasis_audio_set_project_root(const char *project_root) {
    size_t length;
    if (!project_root || !*project_root) {
        audio_assets_reset(1);
        return 0;
    }
    length = strlen(project_root);
    if (length >= sizeof(audio_project_root)) {
        audio_assets_reset(1);
        return 0;
    }

    pthread_mutex_lock(&audio_assets_lock);
    if (strcmp(audio_project_root, project_root) != 0) {
        audio_assets_reset_locked(0);
        memcpy(audio_project_root, project_root, length + 1);
    }
    pthread_mutex_unlock(&audio_assets_lock);
    return 1;
}

static int audio_load_asset(const char *path, int wav_only) {
    char resolved[STASIS_AUDIO_RESOLVED_PATH_SIZE];
    int handle;
    pthread_mutex_lock(&audio_assets_lock);
    if (!audio_resolve_path_locked(path, resolved, sizeof(resolved))) {
        pthread_mutex_unlock(&audio_assets_lock);
        __android_log_print(ANDROID_LOG_WARN, STASIS_AUDIO_LOG_TAG,
            "rejected guest audio path: %s", path ? path : "");
        return 0;
    }
    audio_assets_ensure_initialized_locked();
    handle = wav_only
        ? stasis_audio_assets_load_wav(&audio_assets, resolved)
        : stasis_audio_assets_load(&audio_assets, resolved);
    pthread_mutex_unlock(&audio_assets_lock);
    if (handle <= 0) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_AUDIO_LOG_TAG,
            "failed to load guest audio path: %s", path);
    }
    return handle;
}

int stasis_audio_load_wav(const char *path) {
    return audio_load_asset(path, 1);
}

void stasis_audio_release(int asset_handle) {
    pthread_mutex_lock(&audio_assets_lock);
    stasis_audio_assets_release(&audio_assets, asset_handle);
    pthread_mutex_unlock(&audio_assets_lock);
}

int stasis_audio_play(int asset_handle, int loop, float volume, float pan) {
    int voice_handle;
    pthread_mutex_lock(&audio_assets_lock);
    audio_assets_ensure_initialized_locked();
    voice_handle = stasis_audio_assets_play(
        &audio_assets, asset_handle, loop, volume, pan);
    pthread_mutex_unlock(&audio_assets_lock);
    return voice_handle;
}

void stasis_audio_stop(int voice_handle) {
    pthread_mutex_lock(&audio_assets_lock);
    stasis_audio_assets_stop_voice(&audio_assets, voice_handle);
    pthread_mutex_unlock(&audio_assets_lock);
}

int stasis_audio_voice_is_playing(int voice_handle) {
    int playing;
    pthread_mutex_lock(&audio_assets_lock);
    playing = stasis_audio_assets_voice_is_playing(&audio_assets, voice_handle);
    pthread_mutex_unlock(&audio_assets_lock);
    return playing;
}

void stasis_audio_voice_set_paused(int voice_handle, int paused) {
    pthread_mutex_lock(&audio_assets_lock);
    stasis_audio_assets_voice_set_paused(&audio_assets, voice_handle, paused);
    pthread_mutex_unlock(&audio_assets_lock);
}

void stasis_audio_voice_set_volume_pan(int voice_handle, float volume, float pan) {
    pthread_mutex_lock(&audio_assets_lock);
    stasis_audio_assets_voice_set_volume_pan(
        &audio_assets, voice_handle, volume, pan);
    pthread_mutex_unlock(&audio_assets_lock);
}

int stasis_audio_load_music(const char *path) {
    return audio_load_asset(path, 0);
}

int stasis_audio_load_effect(const char *path) {
    return audio_load_asset(path, 0);
}

int stasis_audio_play_music(int asset_handle, int loop, float volume) {
    int voice_handle;
    pthread_mutex_lock(&audio_assets_lock);
    audio_assets_ensure_initialized_locked();
    /* Music is exclusive per asset, matching the desktop convenience API. */
    stasis_audio_assets_stop_asset(&audio_assets, asset_handle);
    voice_handle = stasis_audio_assets_play(
        &audio_assets, asset_handle, loop, volume, 0.0f);
    pthread_mutex_unlock(&audio_assets_lock);
    return voice_handle > 0;
}

void stasis_audio_stop_music(int asset_handle) {
    pthread_mutex_lock(&audio_assets_lock);
    stasis_audio_assets_stop_asset(&audio_assets, asset_handle);
    pthread_mutex_unlock(&audio_assets_lock);
}

void stasis_audio_pause_music(int asset_handle, int paused) {
    pthread_mutex_lock(&audio_assets_lock);
    stasis_audio_assets_set_asset_paused(&audio_assets, asset_handle, paused);
    pthread_mutex_unlock(&audio_assets_lock);
}

void stasis_audio_set_music_volume(int asset_handle, float volume) {
    pthread_mutex_lock(&audio_assets_lock);
    stasis_audio_assets_set_asset_volume(&audio_assets, asset_handle, volume);
    pthread_mutex_unlock(&audio_assets_lock);
}

int stasis_audio_play_effect(int asset_handle, float volume) {
    return stasis_audio_play(asset_handle, 0, volume, 0.0f) > 0;
}

void stasis_android_audio_set_paused(int paused) {
    atomic_store_explicit(&audio_paused, paused != 0, memory_order_release);
    audio_update_running_state();
}

void stasis_android_audio_set_focus(int focused) {
    atomic_store_explicit(&audio_focused, focused != 0, memory_order_release);
    audio_update_running_state();
}

int stasis_android_audio_is_requested(void) {
    StasisAndroidAudioContext *context = audio_context_acquire();
    if (context == NULL) return 0;
    /* A device error does not cancel the guest's request for audio. Keeping
     * this true lets Java reacquire focus, after which native maintenance
     * recreates the failed stream. Shutdown and failed initialization have no
     * context. */
    audio_context_release(context);
    /* MainActivity polls this once per frame. Maintain the device stream here
     * so an error while focus is retained also recovers without guest polling. */
    audio_update_running_state();
    return 1;
}

int stasis_android_audio_is_running(void) {
    StasisAndroidAudioContext *context = audio_context_acquire();
    int value = 0;
    if (context != NULL) {
        value = atomic_load_explicit(&context->started, memory_order_acquire);
        audio_context_release(context);
    }
    return value;
}

int stasis_android_audio_last_error(void) {
    StasisAndroidAudioContext *context = audio_context_acquire();
    int value;
    if (context == NULL) {
        return atomic_load_explicit(&audio_init_error, memory_order_acquire);
    }
    value = atomic_load_explicit(&context->error, memory_order_acquire);
    audio_context_release(context);
    return value;
}
