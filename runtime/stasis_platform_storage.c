#include "stasis_platform_storage.h"

#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#define STASIS_STORAGE_PATH_CAPACITY 1024

static char stasis_storage_root[STASIS_STORAGE_PATH_CAPACITY];

static int stasis_storage_component_valid(const char *value) {
    size_t length;
    size_t index;
    if (value == NULL) return 0;
    length = strlen(value);
    if (length == 0 || length > 63) return 0;
    for (index = 0; index < length; index += 1) {
        unsigned char ch = (unsigned char)value[index];
        if (!((ch >= 'A' && ch <= 'Z') ||
              (ch >= 'a' && ch <= 'z') ||
              (ch >= '0' && ch <= '9') || ch == '_' || ch == '-')) return 0;
    }
    return 1;
}

static int stasis_storage_ensure_directory(const char *path) {
    if (mkdir(path, 0700) == 0 || errno == EEXIST) return 1;
    return 0;
}

int stasis_storage_set_root(const char *root) {
    size_t length;
    if (root == NULL) return 0;
    length = strlen(root);
    if (length == 0 || length >= sizeof(stasis_storage_root)) return 0;
    memcpy(stasis_storage_root, root, length + 1);
    return stasis_storage_ensure_directory(stasis_storage_root);
}

static int stasis_storage_path(
    const char *scope,
    const char *key,
    const char *extension,
    char *path,
    size_t capacity
) {
    char directory[STASIS_STORAGE_PATH_CAPACITY];
    int directory_written;
    int path_written;
    if (stasis_storage_root[0] == '\0' ||
        !stasis_storage_component_valid(scope) || !stasis_storage_component_valid(key)) return 0;
    directory_written = snprintf(directory, sizeof(directory), "%s/%s", stasis_storage_root, scope);
    if (directory_written < 0 || (size_t)directory_written >= sizeof(directory) ||
        !stasis_storage_ensure_directory(directory)) return 0;
    path_written = snprintf(path, capacity, "%s/%s.%s", directory, key, extension);
    return path_written >= 0 && (size_t)path_written < capacity;
}

int stasis_storage_load_i32(const char *scope, const char *key, int fallback) {
    char path[STASIS_STORAGE_PATH_CAPACITY];
    char buffer[64];
    char *end = NULL;
    long long parsed;
    FILE *file;
    int trailing;
    if (!stasis_storage_path(scope, key, "i32", path, sizeof(path))) return fallback;
    file = fopen(path, "rb");
    if (file == NULL) return fallback;
    if (fgets(buffer, sizeof(buffer), file) == NULL) {
        fclose(file);
        return fallback;
    }
    trailing = fgetc(file);
    fclose(file);
    if (trailing != EOF) return fallback;
    errno = 0;
    parsed = strtoll(buffer, &end, 10);
    if (errno != 0 || end == buffer) return fallback;
    while (*end != '\0' && isspace((unsigned char)*end)) end += 1;
    if (*end != '\0' || parsed < INT32_MIN || parsed > INT32_MAX) return fallback;
    return (int)parsed;
}

int stasis_storage_save_i32(const char *scope, const char *key, int value) {
    char path[STASIS_STORAGE_PATH_CAPACITY];
    char temporary[STASIS_STORAGE_PATH_CAPACITY + 8];
    FILE *file;
    int temporary_written;
    int ok = 1;
    if (!stasis_storage_path(scope, key, "i32", path, sizeof(path))) return 0;
    temporary_written = snprintf(temporary, sizeof(temporary), "%s.tmp", path);
    if (temporary_written < 0 || (size_t)temporary_written >= sizeof(temporary)) return 0;
    file = fopen(temporary, "wb");
    if (file == NULL) return 0;
    if (fprintf(file, "%d\n", value) < 0) ok = 0;
    if (ok && fflush(file) != 0) ok = 0;
    if (fclose(file) != 0) ok = 0;
    if (!ok || rename(temporary, path) != 0) {
        remove(temporary);
        return 0;
    }
    return 1;
}

int stasis_storage_load_ascii(const char *scope, const char *key, char *out, int capacity) {
    char path[STASIS_STORAGE_PATH_CAPACITY];
    FILE *file;
    int count;
    int trailing;
    int index;
    if (out == NULL || capacity <= 0 ||
        !stasis_storage_path(scope, key, "ascii", path, sizeof(path))) return -1;
    file = fopen(path, "rb");
    if (file == NULL) return -1;
    count = (int)fread(out, 1, (size_t)capacity, file);
    trailing = fgetc(file);
    fclose(file);
    if (trailing != EOF) return -1;
    for (index = 0; index < count; index += 1) {
        unsigned char ch = (unsigned char)out[index];
        if (ch < 32 || ch > 126) return -1;
    }
    return count;
}

int stasis_storage_save_ascii(const char *scope, const char *key, const char *value, int length) {
    char path[STASIS_STORAGE_PATH_CAPACITY];
    char temporary[STASIS_STORAGE_PATH_CAPACITY + 8];
    FILE *file;
    int temporary_written;
    int index;
    int ok = 1;
    if (value == NULL || length < 0 ||
        !stasis_storage_path(scope, key, "ascii", path, sizeof(path))) return 0;
    for (index = 0; index < length; index += 1) {
        unsigned char ch = (unsigned char)value[index];
        if (ch < 32 || ch > 126) return 0;
    }
    temporary_written = snprintf(temporary, sizeof(temporary), "%s.tmp", path);
    if (temporary_written < 0 || (size_t)temporary_written >= sizeof(temporary)) return 0;
    file = fopen(temporary, "wb");
    if (file == NULL) return 0;
    if (fwrite(value, 1, (size_t)length, file) != (size_t)length) ok = 0;
    if (ok && fflush(file) != 0) ok = 0;
    if (fclose(file) != 0) ok = 0;
    if (!ok || rename(temporary, path) != 0) {
        remove(temporary);
        return 0;
    }
    return 1;
}
