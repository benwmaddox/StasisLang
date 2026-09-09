#include <jni.h>
#include <android/log.h>
#include <dirent.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include "stasis_display_scale.h"
#include "stasis_render_contract.h"
#include "stasis_mobile_aot_runtime.h"
#include "stasis_platform_storage.h"
#include "stasis_android_audio.h"

#define STASIS_ANDROID_LOG_TAG "StasisWorkshop"
#define STASIS_RUNTIME_STATE_RELATIVE_PATH "build/runtime_state.txt"
#ifndef STASIS_RENDER_ACCEPTANCE
#define STASIS_RENDER_ACCEPTANCE 0
#endif
typedef char *(*stasis_android_bridge_compile_project_fn)(const char *project_root, const char *entry_file);
typedef const char *(*stasis_android_bridge_version_fn)(void);
typedef char *(*stasis_android_bridge_run_tests_fn)(const char *project_root);
typedef char *(*stasis_android_bridge_run_tick_fn)(const char *project_root, const char *entry_file, int touch_x, int touch_y, int touch_active, int screen_w, int screen_h);
typedef int (*stasis_android_bridge_run_render_frame_fn)(const char *project_root, const char *entry_file, int touch_x, int touch_y, int touch_active, int screen_w, int screen_h, int32_t *out_i32, uintptr_t out_i32_len, float *out_f32, uintptr_t out_f32_len, uint8_t *out_u8, uintptr_t out_u8_len);
typedef char *(*stasis_android_bridge_last_frame_error_fn)(void);
typedef char *(*stasis_android_bridge_inspect_runtime_state_fn)(const char *project_root);
typedef char *(*stasis_android_bridge_set_i32_global_fn)(const char *project_root, const char *entry_file, const char *path, int value);
typedef char *(*stasis_android_bridge_get_i32_global_fn)(const char *project_root, const char *entry_file, const char *path);
typedef char *(*stasis_android_bridge_resolve_sprite_asset_fn)(const char *project_root, int handle);
typedef char *(*stasis_android_bridge_drain_sprite_releases_fn)(void);
typedef char *(*stasis_android_bridge_poll_sprite_release_cancellations_fn)(void);
typedef char *(*stasis_android_bridge_resolve_cached_text_fn)(const char *project_root, int handle);
typedef char *(*stasis_android_bridge_resolve_font_fn)(const char *project_root, int handle);
typedef char *(*stasis_android_bridge_source_items_fn)(const char *project_root, const char *entry_file);
typedef char *(*stasis_android_bridge_find_references_fn)(const char *project_root, const char *entry_file, const char *symbol, uintptr_t limit);
typedef char *(*stasis_android_bridge_semantic_edit_fn)(const char *project_root, const char *entry_file, const char *request_json, int dry_run, int validate, int run_tests);
typedef int (*stasis_android_bridge_set_storage_root_fn)(const char *storage_root);
typedef void (*stasis_android_bridge_free_string_fn)(char *value);
typedef struct StasisAudioHostApi {
    int (*init)(int, int, int);
    void (*shutdown)(void);
    int (*is_available)(void);
    int (*get_sample_rate)(void);
    int (*get_channels)(void);
    int (*get_queued_frames)(void);
    int (*get_underruns)(void);
    int (*push_f32_interleaved)(const float *, int);
    int (*load_wav)(const char *);
    void (*release)(int);
    int (*play)(int, int, float, float);
    void (*stop)(int);
    int (*voice_is_playing)(int);
    void (*voice_set_paused)(int, int);
    void (*voice_set_volume_pan)(int, float, float);
    int (*load_music)(const char *);
    int (*load_effect)(const char *);
    int (*play_music)(int, int, float);
    void (*stop_music)(int);
    void (*pause_music)(int, int);
    void (*set_music_volume)(int, float);
    int (*play_effect)(int, float);
} StasisAudioHostApi;
typedef int (*stasis_android_bridge_install_audio_api_fn)(const StasisAudioHostApi *api);
typedef int (*stasis_android_external_url_host_fn)(
        const uint8_t *url, int32_t length, void *context);
typedef void (*stasis_android_bridge_set_external_url_host_fn)(
        stasis_android_external_url_host_fn callback, void *context);
typedef void (*stasis_android_bridge_external_url_action_fn)(void);
typedef char *(*stasis_codex_android_string_fn)(const char *codex_home);
typedef uint64_t (*stasis_codex_android_begin_response_fn)(void);
typedef void (*stasis_codex_android_cancel_response_fn)(void);
typedef char *(*stasis_codex_android_response_fn)(const char *codex_home, const char *request_json, uint64_t generation);
typedef char *(*stasis_codex_android_ai_contract_fn)(void);
typedef int (*stasis_codex_android_initialize_fn)(void *env, void *context);
typedef void (*stasis_codex_android_free_string_fn)(char *value);
typedef struct RustBridgeApi {
    void *handle;
    stasis_android_bridge_version_fn version;
    stasis_android_bridge_compile_project_fn compile_project;
    stasis_android_bridge_run_tests_fn run_tests;
    stasis_android_bridge_run_tick_fn run_tick;
    stasis_android_bridge_run_render_frame_fn run_render_frame;
    stasis_android_bridge_last_frame_error_fn last_frame_error;
    stasis_android_bridge_inspect_runtime_state_fn inspect_runtime_state;
    stasis_android_bridge_set_i32_global_fn set_i32_global;
    stasis_android_bridge_get_i32_global_fn get_i32_global;
    stasis_android_bridge_resolve_sprite_asset_fn resolve_sprite_asset;
    stasis_android_bridge_drain_sprite_releases_fn drain_sprite_releases;
    stasis_android_bridge_poll_sprite_release_cancellations_fn poll_sprite_release_cancellations;
    stasis_android_bridge_resolve_cached_text_fn resolve_cached_text;
    stasis_android_bridge_resolve_font_fn resolve_font;
    stasis_android_bridge_source_items_fn source_items;
    stasis_android_bridge_find_references_fn find_references;
    stasis_android_bridge_semantic_edit_fn semantic_edit;
    stasis_android_bridge_set_storage_root_fn set_storage_root;
    stasis_android_bridge_free_string_fn free_string;
    stasis_android_bridge_install_audio_api_fn install_audio_api;
    stasis_android_bridge_set_external_url_host_fn set_external_url_host;
    stasis_android_bridge_external_url_action_fn arm_external_url_action;
    stasis_android_bridge_external_url_action_fn clear_external_url_action;
    int attempted;
} RustBridgeApi;

static RustBridgeApi rust_bridge_api = {0};
static _Atomic int external_url_action_pending;
static JavaVM *external_url_vm;
static jclass external_url_activity_class;

static int workshop_open_external_url(
        const uint8_t *url, int32_t length, void *context) {
    JNIEnv *env = NULL;
    jmethodID method;
    jbyteArray bytes;
    jboolean accepted;
    (void)context;
    if (url == NULL || length <= 0 || length > 2048 || external_url_vm == NULL ||
            external_url_activity_class == NULL) return 0;
    if ((*external_url_vm)->GetEnv(
            external_url_vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK || env == NULL) return 0;
    method = (*env)->GetStaticMethodID(
            env, external_url_activity_class, "openExternalUrlFromNative", "([B)Z");
    if (method == NULL) {
        (*env)->ExceptionClear(env);
        return 0;
    }
    bytes = (*env)->NewByteArray(env, (jsize)length);
    if (bytes == NULL) {
        (*env)->ExceptionClear(env);
        return 0;
    }
    (*env)->SetByteArrayRegion(env, bytes, 0, (jsize)length, (const jbyte *)url);
    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionClear(env);
        accepted = JNI_FALSE;
    } else {
        accepted = (*env)->CallStaticBooleanMethod(
                env, external_url_activity_class, method, bytes);
    }
    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionClear(env);
        accepted = JNI_FALSE;
    }
    (*env)->DeleteLocalRef(env, bytes);
    return accepted == JNI_TRUE ? 1 : 0;
}
typedef struct CodexBridgeApi {
    void *handle;
    stasis_codex_android_initialize_fn initialize;
    stasis_codex_android_string_fn begin_device_login;
    stasis_codex_android_string_fn account_status;
    stasis_codex_android_string_fn account_rate_limits;
    stasis_codex_android_begin_response_fn begin_response;
    stasis_codex_android_cancel_response_fn cancel_response;
    stasis_codex_android_response_fn response;
    stasis_codex_android_ai_contract_fn ai_contract;
    stasis_codex_android_free_string_fn free_string;
    int attempted;
} CodexBridgeApi;

static CodexBridgeApi codex_bridge_api = {0};

typedef struct StasisJniFrameDescriptor {
    const char *lane;
    size_t byte_capacity;
    size_t alignment;
} StasisJniFrameDescriptor;

#define STASIS_JNI_FRAME_DESCRIPTOR(kind, name, bytes, alignment) \
    {name, bytes, alignment},
static const StasisJniFrameDescriptor stasis_jni_frame_descriptors[] = {
    STASIS_RENDER_BUFFER_DESCRIPTORS(STASIS_JNI_FRAME_DESCRIPTOR)
};
#undef STASIS_JNI_FRAME_DESCRIPTOR

static __thread char stasis_jni_last_frame_error[320];
static jmethodID stasis_jni_buffer_order_method;
static jobject stasis_jni_native_order;
static pthread_mutex_t stasis_jni_order_mutex = PTHREAD_MUTEX_INITIALIZER;
static atomic_int stasis_jni_order_ready = ATOMIC_VAR_INIT(0);

static void clear_stasis_jni_frame_error(void) {
    stasis_jni_last_frame_error[0] = '\0';
}

static void set_stasis_jni_frame_error(const char *lane, const char *reason,
        size_t expected, jlong actual) {
    snprintf(stasis_jni_last_frame_error, sizeof(stasis_jni_last_frame_error),
            "{\"schema\":\"stasis.workshop_jni_frame_abi.v1\","
            "\"test_id\":\"IT-026\",\"event\":\"error\","
            "\"lane\":\"%s\",\"reason\":\"%s\","
            "\"expected\":%zu,\"actual\":%lld}",
            lane, reason, expected, (long long)actual);
}

static void set_stasis_jni_frame_order_error(const char *lane) {
    snprintf(stasis_jni_last_frame_error, sizeof(stasis_jni_last_frame_error),
            "{\"schema\":\"stasis.workshop_jni_frame_abi.v1\","
            "\"test_id\":\"IT-026\",\"event\":\"error\","
            "\"lane\":\"%s\",\"reason\":\"byte_order\","
            "\"expected\":\"native\",\"actual\":\"non_native\"}", lane);
}

static int stasis_jni_clear_exception(JNIEnv *env) {
    if (!(*env)->ExceptionCheck(env)) return 0;
    (*env)->ExceptionClear(env);
    return 1;
}

static int initialize_stasis_jni_order(JNIEnv *env) {
    if (atomic_load_explicit(&stasis_jni_order_ready, memory_order_acquire)) return 1;
    if (pthread_mutex_lock(&stasis_jni_order_mutex) != 0) return 0;
    if (atomic_load_explicit(&stasis_jni_order_ready, memory_order_relaxed)) {
        pthread_mutex_unlock(&stasis_jni_order_mutex);
        return 1;
    }
    jclass buffer_class = (*env)->FindClass(env, "java/nio/ByteBuffer");
    if (stasis_jni_clear_exception(env) || buffer_class == NULL) goto failed;
    jclass order_class = (*env)->FindClass(env, "java/nio/ByteOrder");
    if (stasis_jni_clear_exception(env) || order_class == NULL) goto failed;
    jmethodID buffer_order_method = (*env)->GetMethodID(
            env, buffer_class, "order", "()Ljava/nio/ByteOrder;");
    if (stasis_jni_clear_exception(env) || buffer_order_method == NULL) goto failed;
    jmethodID native_order_method = (*env)->GetStaticMethodID(
            env, order_class, "nativeOrder", "()Ljava/nio/ByteOrder;");
    if (stasis_jni_clear_exception(env) || native_order_method == NULL) goto failed;
    jobject native_order = (*env)->CallStaticObjectMethod(env, order_class, native_order_method);
    if (stasis_jni_clear_exception(env) || native_order == NULL) goto failed;
    jobject native_order_global = (*env)->NewGlobalRef(env, native_order);
    if (stasis_jni_clear_exception(env) || native_order_global == NULL) {
        if (native_order_global != NULL) (*env)->DeleteGlobalRef(env, native_order_global);
        goto failed;
    }
    stasis_jni_buffer_order_method = buffer_order_method;
    stasis_jni_native_order = native_order_global;
    atomic_store_explicit(&stasis_jni_order_ready, 1, memory_order_release);
    pthread_mutex_unlock(&stasis_jni_order_mutex);
    return 1;

failed:
    pthread_mutex_unlock(&stasis_jni_order_mutex);
    return 0;
}

static int validate_stasis_jni_frame_buffers(JNIEnv *env,
        jobject frame_i32, jobject frame_f32, jobject frame_u8) {
    const jobject buffers[] = {frame_i32, frame_f32, frame_u8};
    clear_stasis_jni_frame_error();
    if (!initialize_stasis_jni_order(env)) {
        set_stasis_jni_frame_error("all", "jni_exception", 0, -1);
        return 0;
    }
    for (size_t index = 0; index < sizeof(buffers) / sizeof(buffers[0]); index++) {
        const StasisJniFrameDescriptor *descriptor = &stasis_jni_frame_descriptors[index];
        if (buffers[index] == NULL) {
            set_stasis_jni_frame_error(descriptor->lane, "null_buffer",
                    descriptor->byte_capacity, -1);
            return 0;
        }
        jobject actual_order = (*env)->CallObjectMethod(
                env, buffers[index], stasis_jni_buffer_order_method);
        if (stasis_jni_clear_exception(env)) {
            set_stasis_jni_frame_error(descriptor->lane, "jni_exception", 0, -1);
            return 0;
        }
        if (actual_order == NULL
                || (*env)->IsSameObject(env, actual_order, stasis_jni_native_order) == JNI_FALSE) {
            set_stasis_jni_frame_order_error(descriptor->lane);
            return 0;
        }
        void *address = (*env)->GetDirectBufferAddress(env, buffers[index]);
        jlong capacity = (*env)->GetDirectBufferCapacity(env, buffers[index]);
        if (address == NULL) {
            set_stasis_jni_frame_error(descriptor->lane, "not_direct", descriptor->byte_capacity, capacity);
            return 0;
        }
        if (capacity != (jlong)descriptor->byte_capacity) {
            set_stasis_jni_frame_error(descriptor->lane, "capacity", descriptor->byte_capacity, capacity);
            return 0;
        }
        if (((uintptr_t)address % descriptor->alignment) != 0) {
            set_stasis_jni_frame_error(descriptor->lane, "alignment", descriptor->alignment,
                    (jlong)((uintptr_t)address % descriptor->alignment));
            return 0;
        }
    }
    return 1;
}

static char *read_file_text(const char *path, long *size_out);

static char *read_file_text(const char *path, long *size_out) {
    FILE *file = fopen(path, "rb");
    if (file == NULL) {
        return NULL;
    }

    if (fseek(file, 0, SEEK_END) != 0) {
        fclose(file);
        return NULL;
    }

    long size = ftell(file);
    if (size < 0) {
        fclose(file);
        return NULL;
    }

    if (fseek(file, 0, SEEK_SET) != 0) {
        fclose(file);
        return NULL;
    }

    char *buffer = (char *)malloc((size_t)size + 1);
    if (buffer == NULL) {
        fclose(file);
        return NULL;
    }

    size_t read = fread(buffer, 1, (size_t)size, file);
    fclose(file);
    buffer[read] = '\0';
    *size_out = (long)read;
    return buffer;
}

static int parse_manifest_i32(const char *manifest, const char *key, int *out) {
    const char *cursor = strstr(manifest, key);
    if (cursor == NULL) {
        return 0;
    }
    cursor += strlen(key);
    *out = atoi(cursor);
    return 1;
}

static int read_runtime_tick_count(const char *project_root, int *tick_count) {
    char state_path[1200];
    snprintf(state_path, sizeof(state_path), "%s/%s", project_root, STASIS_RUNTIME_STATE_RELATIVE_PATH);

    long size = 0;
    char *state = read_file_text(state_path, &size);
    if (state == NULL || size == 0) {
        free(state);
        return -1;
    }

    int parsed = parse_manifest_i32(state, "tick_count=", tick_count);
    free(state);
    return parsed ? 0 : -1;
}

static int write_runtime_tick_count(const char *project_root, int tick_count) {
    char state_path[1200];
    snprintf(state_path, sizeof(state_path), "%s/%s", project_root, STASIS_RUNTIME_STATE_RELATIVE_PATH);

    FILE *state = fopen(state_path, "wb");
    if (state == NULL) {
        return -1;
    }

    fprintf(state, "status=RuntimeStateReady\n");
    fprintf(state, "tick_count=%d\n", tick_count);
    fclose(state);
    return 0;
}

static RustBridgeApi *load_rust_bridge_api(void) {
    if (rust_bridge_api.attempted) {
        return rust_bridge_api.handle == NULL ? NULL : &rust_bridge_api;
    }

    rust_bridge_api.attempted = 1;
    rust_bridge_api.handle = dlopen("libstasis_android_bridge.so", RTLD_NOW | RTLD_LOCAL);
    if (rust_bridge_api.handle == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG, "Rust Android bridge unavailable: %s", dlerror());
        return NULL;
    }

    rust_bridge_api.version =
            (stasis_android_bridge_version_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_version");
    rust_bridge_api.compile_project =
            (stasis_android_bridge_compile_project_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_compile_project");
    rust_bridge_api.run_tests =
            (stasis_android_bridge_run_tests_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tests");
    rust_bridge_api.run_tick =
            (stasis_android_bridge_run_tick_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tick");
    rust_bridge_api.run_render_frame =
            (stasis_android_bridge_run_render_frame_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_render_frame");
    rust_bridge_api.last_frame_error =
            (stasis_android_bridge_last_frame_error_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_last_frame_error");
    rust_bridge_api.inspect_runtime_state =
            (stasis_android_bridge_inspect_runtime_state_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_inspect_runtime_state");
    rust_bridge_api.set_i32_global =
            (stasis_android_bridge_set_i32_global_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_set_i32_global");
    rust_bridge_api.get_i32_global =
            (stasis_android_bridge_get_i32_global_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_get_i32_global");
    rust_bridge_api.resolve_sprite_asset =
            (stasis_android_bridge_resolve_sprite_asset_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_resolve_sprite_asset");
    rust_bridge_api.drain_sprite_releases =
            (stasis_android_bridge_drain_sprite_releases_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_drain_sprite_releases");
    rust_bridge_api.poll_sprite_release_cancellations =
            (stasis_android_bridge_poll_sprite_release_cancellations_fn)dlsym(
                    rust_bridge_api.handle, "stasis_android_bridge_poll_sprite_release_cancellations");
    rust_bridge_api.resolve_cached_text =
            (stasis_android_bridge_resolve_cached_text_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_resolve_cached_text");
    rust_bridge_api.resolve_font =
            (stasis_android_bridge_resolve_font_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_resolve_font");
    rust_bridge_api.source_items =
            (stasis_android_bridge_source_items_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_source_items");
    rust_bridge_api.find_references =
            (stasis_android_bridge_find_references_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_find_references");
    rust_bridge_api.semantic_edit =
            (stasis_android_bridge_semantic_edit_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_semantic_edit");
    rust_bridge_api.set_storage_root =
            (stasis_android_bridge_set_storage_root_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_set_storage_root");
    rust_bridge_api.free_string =
            (stasis_android_bridge_free_string_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_free_string");
    rust_bridge_api.install_audio_api =
            (stasis_android_bridge_install_audio_api_fn)dlsym(
                    rust_bridge_api.handle, "stasis_android_bridge_install_audio_api");
    rust_bridge_api.set_external_url_host =
            (stasis_android_bridge_set_external_url_host_fn)dlsym(
                    rust_bridge_api.handle, "stasis_android_bridge_set_external_url_host");
    rust_bridge_api.arm_external_url_action =
            (stasis_android_bridge_external_url_action_fn)dlsym(
                    rust_bridge_api.handle, "stasis_android_bridge_arm_external_url_action");
    rust_bridge_api.clear_external_url_action =
            (stasis_android_bridge_external_url_action_fn)dlsym(
                    rust_bridge_api.handle, "stasis_android_bridge_clear_external_url_action");
    if (rust_bridge_api.version == NULL ||
        rust_bridge_api.compile_project == NULL ||
        rust_bridge_api.run_tick == NULL ||
        rust_bridge_api.free_string == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG, "Rust Android bridge missing required symbols");
        return NULL;
    }

    if (rust_bridge_api.install_audio_api != NULL) {
        const StasisAudioHostApi audio_api = {
            stasis_audio_init,
            stasis_audio_shutdown,
            stasis_audio_is_available,
            stasis_audio_get_sample_rate,
            stasis_audio_get_channels,
            stasis_audio_get_queued_frames,
            stasis_audio_get_underruns,
            stasis_audio_push_f32_interleaved,
            stasis_audio_load_wav,
            stasis_audio_release,
            stasis_audio_play,
            stasis_audio_stop,
            stasis_audio_voice_is_playing,
            stasis_audio_voice_set_paused,
            stasis_audio_voice_set_volume_pan,
            stasis_audio_load_music,
            stasis_audio_load_effect,
            stasis_audio_play_music,
            stasis_audio_stop_music,
            stasis_audio_pause_music,
            stasis_audio_set_music_volume,
            stasis_audio_play_effect};
        rust_bridge_api.install_audio_api(&audio_api);
    }

    return &rust_bridge_api;
}

JNIEXPORT jint JNICALL
Java_com_stasislang_workshop_MainActivity_nativeSetStorageRoot(
        JNIEnv *env, jclass activity_class, jstring storage_root) {
    (void)activity_class;
    if (storage_root == NULL) return 0;
    const char *root = (*env)->GetStringUTFChars(env, storage_root, NULL);
    if (root == NULL) return 0;
    int configured = stasis_storage_set_root(root);
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge != NULL && bridge->set_storage_root != NULL) {
        configured = bridge->set_storage_root(root) && configured;
    }
    else {
        configured = 0;
    }
    (*env)->ReleaseStringUTFChars(env, storage_root, root);
    return configured;
}

static CodexBridgeApi *load_codex_bridge_api(void) {
    if (codex_bridge_api.attempted) {
        return codex_bridge_api.handle == NULL ? NULL : &codex_bridge_api;
    }

    codex_bridge_api.attempted = 1;
    codex_bridge_api.handle = dlopen("libstasis_codex_android.so", RTLD_NOW | RTLD_LOCAL);
    if (codex_bridge_api.handle == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG,
                "Phone-native Codex bridge unavailable: %s", dlerror());
        return NULL;
    }
    codex_bridge_api.begin_device_login = (stasis_codex_android_string_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_begin_device_login");
    codex_bridge_api.initialize = (stasis_codex_android_initialize_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_initialize");
    codex_bridge_api.account_status = (stasis_codex_android_string_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_account_status");
    codex_bridge_api.account_rate_limits = (stasis_codex_android_string_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_account_rate_limits");
    codex_bridge_api.begin_response = (stasis_codex_android_begin_response_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_begin_response");
    codex_bridge_api.cancel_response = (stasis_codex_android_cancel_response_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_cancel_response");
    codex_bridge_api.response = (stasis_codex_android_response_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_response");
    codex_bridge_api.ai_contract = (stasis_codex_android_ai_contract_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_ai_contract");
    codex_bridge_api.free_string = (stasis_codex_android_free_string_fn)dlsym(
            codex_bridge_api.handle, "stasis_codex_android_free_string");
    if (codex_bridge_api.initialize == NULL ||
        codex_bridge_api.begin_device_login == NULL ||
        codex_bridge_api.account_status == NULL ||
        codex_bridge_api.account_rate_limits == NULL ||
        codex_bridge_api.begin_response == NULL ||
        codex_bridge_api.cancel_response == NULL ||
        codex_bridge_api.response == NULL ||
        codex_bridge_api.free_string == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG,
                "Phone-native Codex bridge missing required symbols");
        dlclose(codex_bridge_api.handle);
        memset(&codex_bridge_api, 0, sizeof(codex_bridge_api));
        codex_bridge_api.attempted = 1;
        return NULL;
    }
    return &codex_bridge_api;
}

static jstring call_codex_bridge(JNIEnv *env, jstring codex_home, int begin_login) {
    if (codex_home == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex home was null\"}");
    }
    const char *home = (*env)->GetStringUTFChars(env, codex_home, NULL);
    if (home == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex home was unreadable\"}");
    }
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL) {
        (*env)->ReleaseStringUTFChars(env, codex_home, home);
        return (*env)->NewStringUTF(env, "{\"status\":\"unavailable\",\"error\":\"Phone-native Codex library is not packaged\"}");
    }
    char *response = begin_login ? bridge->begin_device_login(home) : bridge->account_status(home);
    (*env)->ReleaseStringUTFChars(env, codex_home, home);
    if (response == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Phone-native Codex returned no response\"}");
    }
    jstring result = (*env)->NewStringUTF(env, response);
    bridge->free_string(response);
    return result;
}

static jstring call_codex_rate_limits(JNIEnv *env, jstring codex_home) {
    if (codex_home == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex home was null\"}");
    }
    const char *home = (*env)->GetStringUTFChars(env, codex_home, NULL);
    if (home == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex home was unreadable\"}");
    }
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL) {
        (*env)->ReleaseStringUTFChars(env, codex_home, home);
        return (*env)->NewStringUTF(env, "{\"status\":\"unavailable\",\"error\":\"Phone-native Codex library is not packaged\"}");
    }
    char *response = bridge->account_rate_limits(home);
    (*env)->ReleaseStringUTFChars(env, codex_home, home);
    if (response == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Phone-native Codex returned no rate limits\"}");
    }
    jstring result = (*env)->NewStringUTF(env, response);
    bridge->free_string(response);
    return result;
}

static jstring call_codex_response(JNIEnv *env, jstring codex_home, jstring request_json, uint64_t generation) {
    if (codex_home == NULL || request_json == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex request was null\"}");
    }
    const char *home = (*env)->GetStringUTFChars(env, codex_home, NULL);
    const char *request = (*env)->GetStringUTFChars(env, request_json, NULL);
    if (home == NULL || request == NULL) {
        if (home != NULL) (*env)->ReleaseStringUTFChars(env, codex_home, home);
        if (request != NULL) (*env)->ReleaseStringUTFChars(env, request_json, request);
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Codex request was unreadable\"}");
    }
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL) {
        (*env)->ReleaseStringUTFChars(env, codex_home, home);
        (*env)->ReleaseStringUTFChars(env, request_json, request);
        return (*env)->NewStringUTF(env, "{\"status\":\"unavailable\",\"error\":\"Phone-native Codex library is not packaged\"}");
    }
    char *response = bridge->response(home, request, generation);
    (*env)->ReleaseStringUTFChars(env, codex_home, home);
    (*env)->ReleaseStringUTFChars(env, request_json, request);
    if (response == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Phone-native Codex returned no response\"}");
    }
    jstring result = (*env)->NewStringUTF(env, response);
    bridge->free_string(response);
    return result;
}

static int try_rust_bridge_run_tick(const char *project_root, int touch_x, int touch_y, int touch_active, int screen_w, int screen_h, char *message, size_t message_size) {
    if (!stasis_audio_set_project_root(project_root)) {
        snprintf(message, message_size, "RunError: invalid audio project root");
        return 1;
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->run_tick == NULL || bridge->free_string == NULL) {
        return 0;
    }

    char *bridge_message = bridge->run_tick(project_root, "src/main.stasis", touch_x, touch_y, touch_active, screen_w, screen_h);
    if (bridge_message == NULL) {
        snprintf(message, message_size, "RunError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}

static int try_rust_bridge_set_i32_global(const char *project_root, const char *path, int value, char *message, size_t message_size) {
    if (!stasis_audio_set_project_root(project_root)) {
        snprintf(message, message_size, "StateError: invalid audio project root");
        return 1;
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->set_i32_global == NULL || bridge->free_string == NULL) {
        snprintf(message, message_size, "StateError: Rust Android bridge set_i32_global unavailable");
        return 0;
    }

    char *bridge_message = bridge->set_i32_global(project_root, "src/main.stasis", path, value);
    if (bridge_message == NULL) {
        snprintf(message, message_size, "StateError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}

static int try_rust_bridge_get_i32_global(const char *project_root, const char *path, char *message, size_t message_size) {
    if (!stasis_audio_set_project_root(project_root)) {
        snprintf(message, message_size, "StateError: invalid audio project root");
        return 1;
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->get_i32_global == NULL || bridge->free_string == NULL) {
        snprintf(message, message_size, "StateError: Rust Android bridge get_i32_global unavailable");
        return 0;
    }

    char *bridge_message = bridge->get_i32_global(project_root, "src/main.stasis", path);
    if (bridge_message == NULL) {
        snprintf(message, message_size, "StateError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}

static int parse_state_value(const char *message, int *value) {
    return strstr(message, "StateGet:") != NULL && parse_manifest_i32(message, "value=", value);
}

#if STASIS_RENDER_ACCEPTANCE
static void log_workshop_it025_marker(JNIEnv *env, const char *bridge_version,
        int state_checksum, uint32_t command_trace, int render_version, int frame_token) {
    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG,
            "Stasis Workshop IT-025: {\"schema\":\"stasis.workshop_seam.v1\","
            "\"test_id\":\"IT-025\",\"event\":\"frame\","
            "\"jni_version\":%d,\"rust_bridge_version\":\"%s\","
            "\"render_version\":%d,\"state_checksum\":%d,"
            "\"command_trace\":%u,\"frame_token\":%d,\"fallback\":0,\"stub\":0}",
            (*env)->GetVersion(env), bridge_version, render_version,
            state_checksum, command_trace, frame_token);
}

#endif
static int try_rust_bridge_run_render_frame(const char *project_root, int touch_x, int touch_y, int touch_active, int screen_w, int screen_h, int32_t *out_i32, uintptr_t out_i32_len, float *out_f32, uintptr_t out_f32_len, uint8_t *out_u8, uintptr_t out_u8_len) {
    if (!stasis_audio_set_project_root(project_root)) {
        return -1;
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->run_render_frame == NULL) {
        return -1;
    }
    if (atomic_exchange(&external_url_action_pending, 0) != 0) {
        if (bridge->arm_external_url_action != NULL) bridge->arm_external_url_action();
    } else if (bridge->clear_external_url_action != NULL) {
        bridge->clear_external_url_action();
    }
    return bridge->run_render_frame(project_root, "src/main.stasis", touch_x, touch_y, touch_active,
            screen_w, screen_h, out_i32, out_i32_len, out_f32, out_f32_len,
            out_u8, out_u8_len);
}

JNIEXPORT void JNICALL
Java_com_stasislang_workshop_MainActivity_nativeArmExternalUrlAction(
        JNIEnv *env, jclass activity_class) {
    RustBridgeApi *bridge;
    if (external_url_vm == NULL) (*env)->GetJavaVM(env, &external_url_vm);
    if (external_url_activity_class == NULL) {
        external_url_activity_class = (jclass)(*env)->NewGlobalRef(env, activity_class);
    }
    bridge = load_rust_bridge_api();
    if (bridge != NULL && bridge->set_external_url_host != NULL) {
        bridge->set_external_url_host(workshop_open_external_url, NULL);
        atomic_store(&external_url_action_pending, 1);
    }
}

JNIEXPORT void JNICALL
Java_com_stasislang_workshop_MainActivity_nativeClearExternalUrlAction(
        JNIEnv *env, jclass activity_class) {
    RustBridgeApi *bridge = rust_bridge_api.handle == NULL ? NULL : &rust_bridge_api;
    (void)env;
    (void)activity_class;
    atomic_store(&external_url_action_pending, 0);
    if (bridge != NULL) {
        if (bridge->set_external_url_host != NULL) {
            bridge->set_external_url_host(NULL, NULL);
        } else if (bridge->clear_external_url_action != NULL) {
            bridge->clear_external_url_action();
        }
    }
}
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeSetRuntimeI32(JNIEnv *env, jclass activity_class, jstring project_root, jstring path, jint value) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    const char *global_path = (*env)->GetStringUTFChars(env, path, NULL);
    if (root == NULL || global_path == NULL) {
        if (root != NULL) {
            (*env)->ReleaseStringUTFChars(env, project_root, root);
        }
        if (global_path != NULL) {
            (*env)->ReleaseStringUTFChars(env, path, global_path);
        }
        return (*env)->NewStringUTF(env, "StateError: unable to read project root or path");
    }
    char message[512];
    try_rust_bridge_set_i32_global(root, global_path, (int)value, message, sizeof(message));
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    (*env)->ReleaseStringUTFChars(env, path, global_path);
    return (*env)->NewStringUTF(env, message);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeGetRuntimeI32(JNIEnv *env, jclass activity_class, jstring project_root, jstring path) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    const char *global_path = (*env)->GetStringUTFChars(env, path, NULL);
    if (root == NULL || global_path == NULL) {
        if (root != NULL) {
            (*env)->ReleaseStringUTFChars(env, project_root, root);
        }
        if (global_path != NULL) {
            (*env)->ReleaseStringUTFChars(env, path, global_path);
        }
        return (*env)->NewStringUTF(env, "StateError: unable to read project root or path");
    }
    char message[512];
    try_rust_bridge_get_i32_global(root, global_path, message, sizeof(message));
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    (*env)->ReleaseStringUTFChars(env, path, global_path);
    return (*env)->NewStringUTF(env, message);
}
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeRunTests(JNIEnv *env, jclass activity_class, jstring project_root) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "{\"kind\":\"stasis_test_run\",\"passed\":0,\"failed\":1,\"all_passed\":false,\"error\":\"unable to read project root\"}");
    }
    if (!stasis_audio_set_project_root(root)) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env, "{\"kind\":\"stasis_test_run\",\"passed\":0,\"failed\":1,\"all_passed\":false,\"error\":\"invalid audio project root\"}");
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->run_tests == NULL || bridge->free_string == NULL) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env, "{\"kind\":\"stasis_test_run\",\"passed\":0,\"failed\":1,\"all_passed\":false,\"error\":\"Rust Android bridge test runner unavailable\"}");
    }
    char *message = bridge->run_tests(root);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (message == NULL) {
        return (*env)->NewStringUTF(env, "{\"kind\":\"stasis_test_run\",\"passed\":0,\"failed\":1,\"all_passed\":false,\"error\":\"Rust Android bridge returned null test result\"}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeStatus(JNIEnv *env, jclass activity_class) {
    (void)activity_class;
    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "native smoke entry loaded");
    return (*env)->NewStringUTF(env, "Stasis Android native smoke loaded");
}

JNIEXPORT void JNICALL
Java_com_stasislang_workshop_MainActivity_nativeAudioSetPaused(
        JNIEnv *env, jclass activity_class, jboolean paused) {
    (void)env;
    (void)activity_class;
    stasis_android_audio_set_paused(paused ? 1 : 0);
}

JNIEXPORT void JNICALL
Java_com_stasislang_workshop_MainActivity_nativeAudioSetFocus(
        JNIEnv *env, jclass activity_class, jboolean focused) {
    (void)env;
    (void)activity_class;
    stasis_android_audio_set_focus(focused ? 1 : 0);
}

JNIEXPORT void JNICALL
Java_com_stasislang_workshop_MainActivity_nativeAudioShutdown(
        JNIEnv *env, jclass activity_class) {
    (void)env;
    (void)activity_class;
    stasis_audio_shutdown();
}

JNIEXPORT jboolean JNICALL
Java_com_stasislang_workshop_MainActivity_nativeAudioRequested(
        JNIEnv *env, jclass activity_class) {
    (void)env;
    (void)activity_class;
    return stasis_android_audio_is_requested() ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeResolveSpriteAsset(
        JNIEnv *env, jclass activity_class, jstring project_root, jint handle) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"unable to read project root\"}");
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->resolve_sprite_asset == NULL || bridge->free_string == NULL) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"shared sprite resolver unavailable\"}");
    }
    char *message = bridge->resolve_sprite_asset(root, (int)handle);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (message == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"shared sprite resolver returned no result\"}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeDrainSpriteReleases(
        JNIEnv *env, jclass activity_class) {
    (void)activity_class;
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->drain_sprite_releases == NULL || bridge->free_string == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"ok\",\"handles\":[]}");
    }
    char *message = bridge->drain_sprite_releases();
    if (message == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"ok\",\"handles\":[]}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativePollSpriteReleaseCancellations(
        JNIEnv *env, jclass activity_class) {
    (void)activity_class;
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->poll_sprite_release_cancellations == NULL
            || bridge->free_string == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"ok\",\"handles\":[]}");
    }
    char *message = bridge->poll_sprite_release_cancellations();
    if (message == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"ok\",\"handles\":[]}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeResolveCachedText(
        JNIEnv *env, jclass activity_class, jstring project_root, jint handle) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"unable to read project root\"}");
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->resolve_cached_text == NULL || bridge->free_string == NULL) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"cached text resolver unavailable\"}");
    }
    char *message = bridge->resolve_cached_text(root, (int)handle);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (message == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"cached text resolver returned no result\"}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeResolveFont(
        JNIEnv *env, jclass activity_class, jstring project_root, jint handle) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"unable to read project root\"}");
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->resolve_font == NULL || bridge->free_string == NULL) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"font resolver unavailable\"}");
    }
    char *message = bridge->resolve_font(root, (int)handle);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (message == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"font resolver returned no result\"}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexBeginDeviceLogin(
        JNIEnv *env, jclass activity_class, jstring codex_home) {
    (void)activity_class;
    return call_codex_bridge(env, codex_home, 1);
}

JNIEXPORT jint JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexInitialize(
        JNIEnv *env, jclass activity_class, jobject context) {
    (void)activity_class;
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL || context == NULL) return -1;
    return bridge->initialize(env, context);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexAccountStatus(
        JNIEnv *env, jclass activity_class, jstring codex_home) {
    (void)activity_class;
    return call_codex_bridge(env, codex_home, 0);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexAccountRateLimits(
        JNIEnv *env, jclass activity_class, jstring codex_home) {
    (void)activity_class;
    return call_codex_rate_limits(env, codex_home);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeSharedAiContract(
        JNIEnv *env, jclass activity_class) {
    (void)activity_class;
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge == NULL || bridge->ai_contract == NULL || bridge->free_string == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"shared AI contract unavailable\"}");
    }
    char *message = bridge->ai_contract();
    if (message == NULL) {
        return (*env)->NewStringUTF(env,
                "{\"status\":\"error\",\"error\":\"shared AI contract returned no result\"}");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexResponse(
        JNIEnv *env, jclass activity_class, jstring codex_home, jstring request_json, jlong generation) {
    (void)activity_class;
    return call_codex_response(env, codex_home, request_json, (uint64_t)generation);
}

JNIEXPORT jlong JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexBeginResponse(
        JNIEnv *env, jclass activity_class) {
    (void)env;
    (void)activity_class;
    CodexBridgeApi *bridge = load_codex_bridge_api();
    return bridge == NULL ? 0 : (jlong)bridge->begin_response();
}

JNIEXPORT void JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCodexCancelResponse(
        JNIEnv *env, jclass activity_class) {
    (void)env;
    (void)activity_class;
    CodexBridgeApi *bridge = load_codex_bridge_api();
    if (bridge != NULL) bridge->cancel_response();
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCompileProject(JNIEnv *env, jclass activity_class, jstring project_root) {
    (void)activity_class;
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "CompileError: unable to read project root");
    }

    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->compile_project == NULL || bridge->free_string == NULL) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        return (*env)->NewStringUTF(env,
                "CompileError: required Rust Android compiler bridge is unavailable");
    }

    char *message = bridge->compile_project(root, "src/main.stasis");

    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (message == NULL) {
        return (*env)->NewStringUTF(env, "CompileError: Rust Android bridge returned null message");
    }
    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "%s", message);
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeSourceItems(
        JNIEnv *env, jclass activity_class, jstring project_root) {
    (void)activity_class;
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->source_items == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Rust source item bridge unavailable\"}");
    }
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"unable to read project root\"}");
    }
    char *result = bridge->source_items(root, "src/main.stasis");
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (result == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"source item bridge returned null\"}");
    }
    jstring response = (*env)->NewStringUTF(env, result);
    bridge->free_string(result);
    return response;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeFindReferences(
        JNIEnv *env, jclass activity_class, jstring project_root, jstring symbol, jint limit) {
    (void)activity_class;
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->find_references == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Rust reference bridge unavailable\"}");
    }
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    const char *reference_symbol = (*env)->GetStringUTFChars(env, symbol, NULL);
    if (root == NULL || reference_symbol == NULL) {
        if (root != NULL) (*env)->ReleaseStringUTFChars(env, project_root, root);
        if (reference_symbol != NULL) (*env)->ReleaseStringUTFChars(env, symbol, reference_symbol);
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"unable to read reference lookup input\"}");
    }
    char *result = bridge->find_references(
            root, "src/main.stasis", reference_symbol, limit < 1 ? 1u : (uintptr_t)limit);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    (*env)->ReleaseStringUTFChars(env, symbol, reference_symbol);
    if (result == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"reference bridge returned null\"}");
    }
    jstring response = (*env)->NewStringUTF(env, result);
    bridge->free_string(result);
    return response;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeSemanticEdit(
        JNIEnv *env, jclass activity_class, jstring project_root, jstring request_json,
        jboolean dry_run, jboolean validate, jboolean run_tests) {
    (void)activity_class;
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->semantic_edit == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"Rust semantic edit bridge unavailable\"}");
    }
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    const char *request = (*env)->GetStringUTFChars(env, request_json, NULL);
    if (root == NULL || request == NULL) {
        if (root != NULL) (*env)->ReleaseStringUTFChars(env, project_root, root);
        if (request != NULL) (*env)->ReleaseStringUTFChars(env, request_json, request);
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"unable to read semantic edit input\"}");
    }
    char *result = bridge->semantic_edit(
            root, "src/main.stasis", request,
            dry_run ? 1 : 0, validate ? 1 : 0, run_tests ? 1 : 0);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    (*env)->ReleaseStringUTFChars(env, request_json, request);
    if (result == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"error\",\"error\":\"semantic edit bridge returned null\"}");
    }
    jstring response = (*env)->NewStringUTF(env, result);
    bridge->free_string(result);
    return response;
}
JNIEXPORT jint JNICALL
Java_com_stasislang_workshop_MainActivity_nativeRunFrameInto(JNIEnv *env, jclass activity_class, jstring project_root, jint touch_x, jint touch_y, jint touch_active, jint screen_w, jint screen_h, jobject frame_i32, jobject frame_f32, jobject frame_u8) {
    (void)activity_class;
    if (!validate_stasis_jni_frame_buffers(env, frame_i32, frame_f32, frame_u8)) {
        return -1;
    }
    int32_t *values_i32 = (int32_t *)(*env)->GetDirectBufferAddress(env, frame_i32);
    float *values_f32 = (float *)(*env)->GetDirectBufferAddress(env, frame_f32);
    uint8_t *values_u8 = (uint8_t *)(*env)->GetDirectBufferAddress(env, frame_u8);

    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        values_i32[0] = -1;
        return -1;
    }

    int status = try_rust_bridge_run_render_frame(
            root, (int)touch_x, (int)touch_y, (int)touch_active, (int)screen_w, (int)screen_h,
            values_i32, STASIS_RENDER_I32_COUNT, values_f32, STASIS_RENDER_F32_COUNT,
            values_u8, STASIS_RENDER_U8_COUNT);
    if (status != 0) {
        values_i32[0] = -1;
    } else {
        static int32_t *last_traced_frame;
        static int32_t last_display_generation = -1;
        static int32_t last_density_generation = -1;
        if (last_traced_frame != values_i32 ||
                last_display_generation != values_i32[STASIS_RENDER_I_DISPLAY_GENERATION] ||
                last_density_generation != values_i32[STASIS_RENDER_I_DENSITY_GENERATION]) {
            uint32_t trace = stasis_render_trace(values_i32, values_f32, values_u8);
            __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG,
                    "Stasis preview gfx_cmd v%d trace=%u flags=%d lines=%d rects=%d sprites=%d text=%d "
                    "logical=%dx%d native=%dx%d drawable=%dx%d display_gen=%d density_gen=%d",
                    values_i32[STASIS_RENDER_I_VERSION], trace,
                    values_i32[STASIS_RENDER_I_FLAGS],
                    values_i32[STASIS_RENDER_I_LINE_COUNT],
                    values_i32[STASIS_RENDER_I_RECT_COUNT],
                    values_i32[STASIS_RENDER_I_SPRITE_COUNT],
                    values_i32[STASIS_RENDER_I_TEXT_COUNT],
                    values_i32[STASIS_RENDER_I_LOGICAL_W],
                    values_i32[STASIS_RENDER_I_LOGICAL_H],
                    values_i32[STASIS_RENDER_I_NATIVE_W],
                    values_i32[STASIS_RENDER_I_NATIVE_H],
                    values_i32[STASIS_RENDER_I_DRAWABLE_W],
                    values_i32[STASIS_RENDER_I_DRAWABLE_H],
                    values_i32[STASIS_RENDER_I_DISPLAY_GENERATION],
                    values_i32[STASIS_RENDER_I_DENSITY_GENERATION]);
            last_traced_frame = values_i32;
            last_display_generation = values_i32[STASIS_RENDER_I_DISPLAY_GENERATION];
            last_density_generation = values_i32[STASIS_RENDER_I_DENSITY_GENERATION];
        }
#if STASIS_RENDER_ACCEPTANCE
        {
            static int it025_state_ready = 0;
            static int it025_state_checksum;
            static const char *it025_bridge_version;
            RustBridgeApi *bridge = load_rust_bridge_api();
            char state_message[256];
            if (!it025_state_ready) {
                it025_bridge_version = bridge == NULL || bridge->version == NULL
                        ? NULL : bridge->version();
                if (it025_bridge_version == NULL || it025_bridge_version[0] == '\0' ||
                        !try_rust_bridge_get_i32_global(root, "seam_state_checksum",
                                state_message, sizeof(state_message)) ||
                        !parse_state_value(state_message, &it025_state_checksum) ||
                        it025_state_checksum <= 0) {
                    __android_log_print(ANDROID_LOG_ERROR, STASIS_ANDROID_LOG_TAG,
                            "IT-025 state checksum was unavailable from the live Rust JIT global bridge");
                    (*env)->ReleaseStringUTFChars(env, project_root, root);
                    values_i32[0] = -1;
                    return -1;
                }
                it025_state_ready = 1;
            }
            if (it025_bridge_version == NULL) {
                __android_log_print(ANDROID_LOG_ERROR, STASIS_ANDROID_LOG_TAG,
                        "IT-025 Rust bridge version was unavailable after initialization");
                (*env)->ReleaseStringUTFChars(env, project_root, root);
                values_i32[0] = -1;
                return -1;
            }
            log_workshop_it025_marker(env, it025_bridge_version, it025_state_checksum,
                    stasis_render_trace(values_i32, values_f32, values_u8),
                    values_i32[STASIS_RENDER_I_VERSION],
                    values_i32[STASIS_RENDER_I_FRAME_TOKEN]);
        }
#endif
    }
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    return (jint)status;
}

#if STASIS_RENDER_ACCEPTANCE
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeFrameAbiDescriptor(
        JNIEnv *env, jclass activity_class) {
    (void)activity_class;
    char descriptor_json[512];
    const StasisJniFrameDescriptor *i32 = &stasis_jni_frame_descriptors[0];
    const StasisJniFrameDescriptor *f32 = &stasis_jni_frame_descriptors[1];
    const StasisJniFrameDescriptor *u8 = &stasis_jni_frame_descriptors[2];
    snprintf(descriptor_json, sizeof(descriptor_json),
            "{\"schema\":\"stasis.workshop_jni_frame_abi.v1\","
            "\"test_id\":\"IT-026\",\"event\":\"descriptor\","
            "\"lanes\":[{\"lane\":\"%s\",\"bytes\":%zu,\"alignment\":%zu},"
            "{\"lane\":\"%s\",\"bytes\":%zu,\"alignment\":%zu},"
            "{\"lane\":\"%s\",\"bytes\":%zu,\"alignment\":%zu}]}",
            i32->lane, i32->byte_capacity, i32->alignment,
            f32->lane, f32->byte_capacity, f32->alignment,
            u8->lane, u8->byte_capacity, u8->alignment);
    return (*env)->NewStringUTF(env, descriptor_json);
}

JNIEXPORT jint JNICALL
Java_com_stasislang_workshop_MainActivity_nativeFrameTrace(
        JNIEnv *env, jclass activity_class, jobject frame_i32, jobject frame_f32,
        jobject frame_u8) {
    (void)activity_class;
    if (!validate_stasis_jni_frame_buffers(env, frame_i32, frame_f32, frame_u8)) {
        return -1;
    }
    int32_t *values_i32 = (int32_t *)(*env)->GetDirectBufferAddress(env, frame_i32);
    float *values_f32 = (float *)(*env)->GetDirectBufferAddress(env, frame_f32);
    uint8_t *values_u8 = (uint8_t *)(*env)->GetDirectBufferAddress(env, frame_u8);
    return (jint)stasis_render_trace(values_i32, values_f32, values_u8);
}
#endif

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeLastFrameError(
        JNIEnv *env, jclass activity_class) {
    (void)activity_class;
    if (stasis_jni_last_frame_error[0] != '\0') {
        char message[sizeof(stasis_jni_last_frame_error)];
        snprintf(message, sizeof(message), "%s", stasis_jni_last_frame_error);
        clear_stasis_jni_frame_error();
        return (*env)->NewStringUTF(env, message);
    }
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->last_frame_error == NULL || bridge->free_string == NULL) {
        return (*env)->NewStringUTF(env, "native preview frame failed");
    }
    char *message = bridge->last_frame_error();
    if (message == NULL) {
        return (*env)->NewStringUTF(env, "native preview frame failed");
    }
    jstring result = (*env)->NewStringUTF(env, message);
    bridge->free_string(message);
    return result;
}
JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeInspectRuntimeState(
        JNIEnv *env, jclass activity_class, jstring project_root) {
    (void)activity_class;
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->inspect_runtime_state == NULL || bridge->free_string == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"RuntimeStateError\",\"error\":\"live runtime inspection unavailable\"}");
    }
    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"RuntimeStateError\",\"error\":\"unable to read project root\"}");
    }
    char *result = bridge->inspect_runtime_state(root);
    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (result == NULL) {
        return (*env)->NewStringUTF(env, "{\"status\":\"RuntimeStateError\",\"error\":\"live runtime inspection returned null\"}");
    }
    jstring response = (*env)->NewStringUTF(env, result);
    bridge->free_string(result);
    return response;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeRunTick(JNIEnv *env, jclass activity_class, jstring project_root, jint touch_x, jint touch_y, jint touch_active, jint screen_w, jint screen_h) {
    (void)activity_class;

    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "RunError: unable to read project root");
    }

    char message[1024];
    if (try_rust_bridge_run_tick(root, (int)touch_x, (int)touch_y, (int)touch_active, (int)screen_w, (int)screen_h, message, sizeof(message)) != 0) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        if (strncmp(message, "RunError", 8) == 0) {
            __android_log_print(ANDROID_LOG_ERROR, STASIS_ANDROID_LOG_TAG, "%s", message);
        }
        return (*env)->NewStringUTF(env, message);
    }

    int tick_count = 0;
    if (read_runtime_tick_count(root, &tick_count) != 0) {
        snprintf(message, sizeof(message), "RunError: compile project before running tick");
    } else if (write_runtime_tick_count(root, tick_count + 1) != 0) {
        snprintf(message, sizeof(message), "RunError: unable to update runtime state");
    } else {
        snprintf(message, sizeof(message), "RunTick: tick_count=%d state=%s", tick_count + 1, STASIS_RUNTIME_STATE_RELATIVE_PATH);
    }

    (*env)->ReleaseStringUTFChars(env, project_root, root);
    if (strncmp(message, "RunError", 8) == 0) {
        __android_log_print(ANDROID_LOG_ERROR, STASIS_ANDROID_LOG_TAG, "%s", message);
    }
    return (*env)->NewStringUTF(env, message);
}
