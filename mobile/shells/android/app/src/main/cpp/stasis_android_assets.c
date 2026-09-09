#include <jni.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "stasis_performance_metrics.h"
#include "stasis_mobile_aot_runtime.h"

void stasis_host_get_latest_performance_metrics(uint32_t *tick_us, uint32_t *render_us);
int stasis_host_get_latest_performance_metrics_v1(
    StasisPerformanceMetrics *output, size_t capacity);
void stasis_host_set_performance_metrics_enabled(int enabled);
int stasis_host_copy_runtime_error(char *output, size_t output_size);

#if defined(STASIS_ENABLE_SEAM_TESTS)
JNIEXPORT void JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeSetSeamTestId(
    JNIEnv *env,
    jclass activity,
    jstring value
) {
    (void)activity;
    if (value == NULL) {
        return;
    }
    const char *test_id = (*env)->GetStringUTFChars(env, value, NULL);
    if (test_id == NULL) {
        return;
    }
    size_t length = strlen(test_id);
    int valid = length > 0 && length <= 32;
    for (size_t index = 0; valid && index < length; index++) {
        char byte = test_id[index];
        valid = (byte >= 'A' && byte <= 'Z') ||
            (byte >= '0' && byte <= '9') || byte == '-';
    }
    if (valid) {
        setenv("STASIS_SEAM_TEST_ID", test_id, 1);
        setenv("STASIS_ENABLE_TEST_INPUT", "1", 1);
    }
    (*env)->ReleaseStringUTFChars(env, value, test_id);
}
#endif

JNIEXPORT void JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeSetAssetRoot(
    JNIEnv *env,
    jclass activity,
    jstring path
) {
    (void)activity;
    const char *root = (*env)->GetStringUTFChars(env, path, NULL);
    if (root == NULL) {
        return;
    }
    setenv("STASIS_ASSET_ROOT", root, 1);
    (*env)->ReleaseStringUTFChars(env, path, root);
}

/* Java performs package verification before SDL starts.  Keep the exact
 * bounded diagnostic in the process environment so SDL_main can reject the
 * package before binding AOT entries or initializing the game. */
JNIEXPORT void JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeSetAssetVerificationError(
    JNIEnv *env,
    jclass activity,
    jstring value
) {
    (void)activity;
    if (value == NULL) {
        unsetenv("STASIS_ASSET_VERIFICATION_ERROR");
        return;
    }
    const char *message = (*env)->GetStringUTFChars(env, value, NULL);
    if (message == NULL) {
        return;
    }
    if (strlen(message) > 400) {
        setenv(
            "STASIS_ASSET_VERIFICATION_ERROR",
            "code=asset_cache_failure path=assets/manifest.json detail=diagnostic exceeded native limit",
            1
        );
        (*env)->ReleaseStringUTFChars(env, value, message);
        return;
    }
    setenv("STASIS_ASSET_VERIFICATION_ERROR", message, 1);
    (*env)->ReleaseStringUTFChars(env, value, message);
}

#if defined(STASIS_ENABLE_SEAM_TESTS)
JNIEXPORT void JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeSetAssetManifestSha256(
    JNIEnv *env,
    jclass activity,
    jstring value
) {
    (void)activity;
    if (value == NULL) return;
    const char *hash = (*env)->GetStringUTFChars(env, value, NULL);
    if (hash == NULL) return;
    size_t length = strlen(hash);
    int valid = length == 64;
    for (size_t index = 0; valid && index < length; index++) {
        char byte = hash[index];
        valid = (byte >= '0' && byte <= '9') || (byte >= 'a' && byte <= 'f');
    }
    if (valid) setenv("STASIS_ASSET_MANIFEST_SHA256", hash, 1);
    (*env)->ReleaseStringUTFChars(env, value, hash);
}
#endif

/* The join URL is deliberately a native-shell-only read.  It contains the
 * pairing secret and must never enter Stasis globals, deterministic frames,
 * logs, or the packaged guest. */
JNIEXPORT jstring JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeReadNetworkJoinUrl(
    JNIEnv *env,
    jclass activity
) {
    (void)activity;
    char url[2048];
    int32_t length = stasis_mobile_network_copy_join_url(url, sizeof(url));
    if (length <= 0 || (size_t)length >= sizeof(url)) {
        return NULL;
    }
    return (*env)->NewStringUTF(env, url);
}

JNIEXPORT jint JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeProvisionNetworkClient(
    JNIEnv *env,
    jclass activity,
    jstring value
) {
    (void)activity;
#if defined(STASIS_NETWORK_CLIENT_ENABLED)
    if (value == NULL) return -1;
    const char *join_url = (*env)->GetStringUTFChars(env, value, NULL);
    if (join_url == NULL) return -1;
    size_t length = strlen(join_url);
    int32_t result = stasis_mobile_network_client_provision(join_url, length);
    (*env)->ReleaseStringUTFChars(env, value, join_url);
    if (result == 0) result = stasis_mobile_network_client_connect();
    return result;
#else
    (void)env; (void)value;
    return -4;
#endif
}

JNIEXPORT jint JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeSetNetworkClientBackground(
    JNIEnv *env,
    jclass activity,
    jboolean background
) {
    (void)env; (void)activity;
    return stasis_mobile_network_client_set_background(
        background == JNI_TRUE ? 1 : 0);
}

JNIEXPORT void JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeShutdownNetworkClient(
    JNIEnv *env,
    jclass activity
) {
    (void)env; (void)activity;
    stasis_mobile_network_client_shutdown();
}

JNIEXPORT jboolean JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeReadPerformanceMetrics(
    JNIEnv *env,
    jclass activity,
    jfloatArray output
) {
    (void)activity;
    if (output == NULL || (*env)->GetArrayLength(env, output) < 14) {
        return JNI_FALSE;
    }
    StasisPerformanceMetrics metrics;
    if (!stasis_host_get_latest_performance_metrics_v1(&metrics, sizeof(metrics))) {
        return JNI_FALSE;
    }
    const uint32_t values_us[] = {
        metrics.tick_us, metrics.guest_render_us, metrics.host_replay_us,
        metrics.render_prep_us, metrics.gpu_submit_us, metrics.gpu_execution_us,
        metrics.frame_work_us, metrics.present_wait_us,
    };
    jfloat values[14];
    for (int index = 0; index < 8; index++) {
        values[index] = values_us[index] == STASIS_PERF_UNAVAILABLE
            ? -1.0f : (jfloat)values_us[index] / 1000.0f;
    }
    const uint32_t counts[] = {
        metrics.commands, metrics.lines, metrics.rectangles, metrics.sprites, metrics.text,
    };
    for (int index = 0; index < 5; index++) {
        values[9 + index] = counts[index] == STASIS_PERF_UNAVAILABLE
            ? -1.0f : (jfloat)counts[index];
    }
    values[8] = (jfloat)metrics.version;
    (*env)->SetFloatArrayRegion(env, output, 0, 14, values);
    return JNI_TRUE;
}

JNIEXPORT void JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeSetPerformanceMetricsEnabled(
    JNIEnv *env,
    jclass activity,
    jboolean enabled
) {
    (void)env;
    (void)activity;
    stasis_host_set_performance_metrics_enabled(enabled == JNI_TRUE ? 1 : 0);
}

JNIEXPORT jstring JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeReadRuntimeError(
    JNIEnv *env,
    jclass activity
) {
    (void)activity;
    char message[512];
    if (!stasis_host_copy_runtime_error(message, sizeof(message))) {
        return NULL;
    }
    return (*env)->NewStringUTF(env, message);
}
