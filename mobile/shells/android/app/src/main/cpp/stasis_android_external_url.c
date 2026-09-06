#include <jni.h>
#include <stdint.h>

#include <SDL3/SDL_system.h>

int stasis_platform_open_external_url(const char *url, int32_t length) {
    JNIEnv *env;
    jobject activity;
    jclass activity_class;
    jmethodID method;
    jbyteArray bytes;
    jboolean accepted;
    if (url == NULL || length <= 0) return 0;
    env = (JNIEnv *)SDL_GetAndroidJNIEnv();
    activity = (jobject)SDL_GetAndroidActivity();
    if (env == NULL || activity == NULL) return 0;
    activity_class = (*env)->GetObjectClass(env, activity);
    if (activity_class == NULL) {
        (*env)->DeleteLocalRef(env, activity);
        return 0;
    }
    method = (*env)->GetMethodID(
        env, activity_class, "openExternalUrlFromNative", "([B)Z");
    if (method == NULL) {
        (*env)->ExceptionClear(env);
        (*env)->DeleteLocalRef(env, activity_class);
        (*env)->DeleteLocalRef(env, activity);
        return 0;
    }
    bytes = (*env)->NewByteArray(env, (jsize)length);
    if (bytes == NULL) {
        (*env)->ExceptionClear(env);
        (*env)->DeleteLocalRef(env, activity_class);
        (*env)->DeleteLocalRef(env, activity);
        return 0;
    }
    (*env)->SetByteArrayRegion(env, bytes, 0, (jsize)length, (const jbyte *)url);
    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionClear(env);
        accepted = JNI_FALSE;
    } else {
        accepted = (*env)->CallBooleanMethod(env, activity, method, bytes);
    }
    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionClear(env);
        accepted = JNI_FALSE;
    }
    (*env)->DeleteLocalRef(env, bytes);
    (*env)->DeleteLocalRef(env, activity_class);
    (*env)->DeleteLocalRef(env, activity);
    return accepted == JNI_TRUE ? 1 : 0;
}
