#include "stasis_android_audio.h"
#include "stasis_audio_ring.h"

#include <aaudio/AAudio.h>
#include <android/log.h>
#include <dlfcn.h>
#include <sched.h>
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
    if (context == NULL) return;
    if (audio_should_run()) {
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

int stasis_audio_init(int sample_rate, int channels, int target_latency_frames) {
    AAudioStreamBuilder *builder = NULL;
    StasisAndroidAudioContext *context = NULL;
    aaudio_result_t result = AAUDIO_ERROR_INTERNAL;
    int32_t burst;
    uint32_t capacity;
    if (sample_rate < 8000 || sample_rate > 192000 ||
        (channels != 1 && channels != 2) || target_latency_frames <= 0) {
        return 0;
    }
    stasis_audio_shutdown();
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

void stasis_audio_shutdown(void) {
    StasisAndroidAudioContext *context;
    audio_lock_context();
    context = atomic_exchange_explicit(&audio_context, NULL, memory_order_relaxed);
    if (context != NULL) {
        atomic_store_explicit(&context->retired, 1, memory_order_release);
    }
    audio_unlock_context();
    audio_retire_context(context);
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
        stasis_audio_shutdown();
        return stasis_audio_init(rate, channels, latency);
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

void stasis_android_audio_set_paused(int paused) {
    atomic_store_explicit(&audio_paused, paused != 0, memory_order_release);
    audio_update_running_state();
}

void stasis_android_audio_set_focus(int focused) {
    atomic_store_explicit(&audio_focused, focused != 0, memory_order_release);
    audio_update_running_state();
}

int stasis_android_audio_is_requested(void) {
    return atomic_load_explicit(&audio_attempted, memory_order_acquire);
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
