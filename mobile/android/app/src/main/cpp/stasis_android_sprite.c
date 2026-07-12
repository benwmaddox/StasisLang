#include <jni.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#define NANOSVG_IMPLEMENTATION
#include "nanosvg.h"
#define NANOSVGRAST_IMPLEMENTATION
#include "nanosvgrast.h"

#define STASIS_MAX_SPRITE_DIMENSION 16384
#define STASIS_MAX_SPRITE_PIXELS 16000000
#define STASIS_MAX_SPRITE_FILE_BYTES (64 * 1024 * 1024)

static jintArray stasis_rasterize_svg_image(
        JNIEnv *env, NSVGimage *image, jint width, jint height) {
    if (image == NULL || image->width <= 0.0f || image->height <= 0.0f) {
        if (image != NULL) nsvgDelete(image);
        return NULL;
    }
    float scale_x = (float)width / image->width;
    float scale_y = (float)height / image->height;
    float scale = scale_x < scale_y ? scale_x : scale_y;
    int content_w = (int)ceilf(image->width * scale);
    int content_h = (int)ceilf(image->height * scale);
    if (content_w < 1) content_w = 1;
    if (content_h < 1) content_h = 1;
    if (content_w > width) content_w = width;
    if (content_h > height) content_h = height;
    float tx = (float)(width - content_w) * 0.5f;
    float ty = (float)(height - content_h) * 0.5f;

    NSVGrasterizer *rasterizer = nsvgCreateRasterizer();
    size_t pixel_count = (size_t)width * (size_t)height;
    unsigned char *rgba = (unsigned char *)calloc(pixel_count, 4u);
    jint *argb = (jint *)malloc(pixel_count * sizeof(jint));
    if (rasterizer == NULL || rgba == NULL || argb == NULL) {
        free(argb);
        free(rgba);
        if (rasterizer != NULL) nsvgDeleteRasterizer(rasterizer);
        nsvgDelete(image);
        return NULL;
    }
    nsvgRasterize(rasterizer, image, tx, ty, scale, rgba, width, height, width * 4);
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
    nsvgDeleteRasterizer(rasterizer);
    nsvgDelete(image);
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
    NSVGimage *image = nsvgParseFromFile(native_path, "px", 96.0f);
    (*env)->ReleaseStringUTFChars(env, path, native_path);
    return stasis_rasterize_svg_image(env, image, width, height);
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
    char *source = (char *)malloc((size_t)length + 1u);
    if (source == NULL) return NULL;
    (*env)->GetByteArrayRegion(env, bytes, 0, length, (jbyte *)source);
    if ((*env)->ExceptionCheck(env)) {
        free(source);
        return NULL;
    }
    source[length] = '\0';
    NSVGimage *image = nsvgParse(source, "px", 96.0f);
    free(source);
    return stasis_rasterize_svg_image(env, image, width, height);
}
