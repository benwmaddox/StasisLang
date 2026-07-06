#include <jni.h>
#include <android/log.h>
#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#define STASIS_ANDROID_LOG_TAG "StasisWorkshop"

static int has_suffix(const char *value, const char *suffix) {
    size_t value_len = strlen(value);
    size_t suffix_len = strlen(suffix);
    if (value_len < suffix_len) {
        return 0;
    }
    return strcmp(value + value_len - suffix_len, suffix) == 0;
}

static int scan_stasis_files(const char *path, int *file_count, long *byte_count) {
    DIR *dir = opendir(path);
    if (dir == NULL) {
        return -1;
    }

    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL) {
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) {
            continue;
        }

        size_t path_len = strlen(path);
        size_t name_len = strlen(entry->d_name);
        char *child = (char *)malloc(path_len + 1 + name_len + 1);
        if (child == NULL) {
            closedir(dir);
            return -2;
        }
        memcpy(child, path, path_len);
        child[path_len] = '/';
        memcpy(child + path_len + 1, entry->d_name, name_len + 1);

        struct stat info;
        if (stat(child, &info) == 0) {
            if (S_ISDIR(info.st_mode)) {
                int result = scan_stasis_files(child, file_count, byte_count);
                free(child);
                if (result != 0) {
                    closedir(dir);
                    return result;
                }
                continue;
            }

            if (S_ISREG(info.st_mode) && has_suffix(entry->d_name, ".stasis")) {
                FILE *file = fopen(child, "rb");
                if (file == NULL) {
                    free(child);
                    closedir(dir);
                    return -3;
                }
                if (fseek(file, 0, SEEK_END) == 0) {
                    long size = ftell(file);
                    if (size > 0) {
                        *byte_count += size;
                    }
                }
                fclose(file);
                *file_count += 1;
            }
        }
        free(child);
    }

    closedir(dir);
    return 0;
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeStatus(JNIEnv *env, jclass activity_class) {
    (void)activity_class;
    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "native smoke entry loaded");
    return (*env)->NewStringUTF(env, "Stasis Android native smoke loaded");
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeCompileProject(JNIEnv *env, jclass activity_class, jstring project_root) {
    (void)activity_class;

    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "CompileNotLinked: unable to read project root");
    }

    int file_count = 0;
    long byte_count = 0;
    int result = scan_stasis_files(root, &file_count, &byte_count);
    (*env)->ReleaseStringUTFChars(env, project_root, root);

    char message[192];
    if (result != 0) {
        snprintf(message, sizeof(message), "CompileNotLinked: native probe failed (%d)", result);
    } else {
        snprintf(message, sizeof(message), "CompileNotLinked: native probe read %d .stasis files (%ld bytes)", file_count, byte_count);
    }

    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "%s", message);
    return (*env)->NewStringUTF(env, message);
}