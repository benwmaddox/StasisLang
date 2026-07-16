#include <jni.h>
#include <stdlib.h>

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
