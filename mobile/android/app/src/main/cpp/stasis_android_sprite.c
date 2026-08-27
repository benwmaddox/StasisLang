#include <jni.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/stat.h>

#include "stasis_svg.h"

#define STASIS_MAX_SPRITE_DIMENSION 16384
#define STASIS_MAX_SPRITE_PIXELS 16000000
#define STASIS_MAX_SPRITE_FILE_BYTES (64 * 1024 * 1024)

static jintArray stasis_rgba_to_jint_array(
        JNIEnv *env, unsigned char *rgba, jint width, jint height) {
    size_t pixel_count = (size_t)width * (size_t)height;
    jint *argb = (jint *)malloc(pixel_count * sizeof(jint));
    if (rgba == NULL || argb == NULL) {
        free(rgba);
        free(argb);
        return NULL;
    }
    for (size_t index = 0; index < pixel_count; index += 1) {
        unsigned char *pixel = rgba + index * 4u;
        argb[index] = (jint)(((uint32_t)pixel[3] << 24) |
                ((uint32_t)pixel[0] << 16) | ((uint32_t)pixel[1] << 8) | pixel[2]);
    }
    jintArray result = (*env)->NewIntArray(env, (jsize)pixel_count);
    if (result != NULL) {
        (*env)->SetIntArrayRegion(env, result, 0, (jsize)pixel_count, argb);
    }
    free(argb);
    free(rgba);
    return result;
}

JNIEXPORT jintArray JNICALL
Java_com_stasislang_workshop_MainActivity_nativeDecodeSvgSprite(
        JNIEnv *env, jclass activity_class, jstring path, jint width, jint height) {
    (void)activity_class;
    if (path == NULL || width <= 0 || height <= 0 ||
        width > STASIS_MAX_SPRITE_DIMENSION || height > STASIS_MAX_SPRITE_DIMENSION ||
        (int64_t)width * (int64_t)height > STASIS_MAX_SPRITE_PIXELS) {
        return NULL;
    }
    const char *native_path = (*env)->GetStringUTFChars(env, path, NULL);
    if (native_path == NULL) return NULL;
    struct stat info;
    if (stat(native_path, &info) != 0 || info.st_size < 0 ||
        info.st_size > STASIS_MAX_SPRITE_FILE_BYTES) {
        (*env)->ReleaseStringUTFChars(env, path, native_path);
        return NULL;
    }
    unsigned char *rgba = NULL;
    int output_width = 0;
    int output_height = 0;
    int rasterized = stasis_svg_rasterize_file(
        native_path, width, height, &rgba, &output_width, &output_height);
    (*env)->ReleaseStringUTFChars(env, path, native_path);
    if (!rasterized || output_width != width || output_height != height) {
        free(rgba);
        return NULL;
    }
    return stasis_rgba_to_jint_array(env, rgba, width, height);
}

JNIEXPORT jintArray JNICALL
Java_com_stasislang_workshop_MainActivity_nativeDecodeSvgSpriteBytes(
        JNIEnv *env, jclass activity_class, jbyteArray bytes, jint width, jint height) {
    (void)activity_class;
    if (bytes == NULL || width <= 0 || height <= 0 ||
        width > STASIS_MAX_SPRITE_DIMENSION || height > STASIS_MAX_SPRITE_DIMENSION ||
        (int64_t)width * (int64_t)height > STASIS_MAX_SPRITE_PIXELS) {
        return NULL;
    }
    jsize length = (*env)->GetArrayLength(env, bytes);
    if (length <= 0 || length > STASIS_MAX_SPRITE_FILE_BYTES) return NULL;
    char *source = (char *)malloc((size_t)length);
    if (source == NULL) return NULL;
    (*env)->GetByteArrayRegion(env, bytes, 0, length, (jbyte *)source);
    if ((*env)->ExceptionCheck(env)) {
        free(source);
        return NULL;
    }
    unsigned char *rgba = NULL;
    int output_width = 0;
    int output_height = 0;
    int rasterized = stasis_svg_rasterize_memory(
        source, (size_t)length, width, height, &rgba, &output_width, &output_height);
    free(source);
    if (!rasterized || output_width != width || output_height != height) {
        free(rgba);
        return NULL;
    }
    return stasis_rgba_to_jint_array(env, rgba, width, height);
}
