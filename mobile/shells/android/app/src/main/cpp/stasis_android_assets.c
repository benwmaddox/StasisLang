#include <jni.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

void stasis_host_get_latest_performance_metrics(uint32_t *tick_us, uint32_t *render_us);
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

JNIEXPORT jboolean JNICALL
Java_@STASIS_JNI_PACKAGE@_MainActivity_nativeReadPerformanceMetrics(
    JNIEnv *env,
    jclass activity,
    jfloatArray output
) {
    (void)activity;
    if (output == NULL || (*env)->GetArrayLength(env, output) < 2) {
        return JNI_FALSE;
    }
    uint32_t tick_us = 0;
    uint32_t render_us = 0;
    stasis_host_get_latest_performance_metrics(&tick_us, &render_us);
    jfloat values[2] = {(jfloat)tick_us / 1000.0f, (jfloat)render_us / 1000.0f};
    (*env)->SetFloatArrayRegion(env, output, 0, 2, values);
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
