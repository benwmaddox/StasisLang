#include <jni.h>
#include <android/log.h>
#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#define STASIS_ANDROID_LOG_TAG "StasisWorkshop"

typedef struct CompileStats {
    int file_count;
    int function_count;
    int struct_count;
    int global_count;
    int has_main;
    int has_tick;
    int has_on_code_swap;
    long byte_count;
    char error[160];
} CompileStats;

static int has_suffix(const char *value, const char *suffix) {
    size_t value_len = strlen(value);
    size_t suffix_len = strlen(suffix);
    if (value_len < suffix_len) {
        return 0;
    }
    return strcmp(value + value_len - suffix_len, suffix) == 0;
}

static int starts_with_at(const char *source, size_t index, const char *token) {
    return strncmp(source + index, token, strlen(token)) == 0;
}

static int count_token(const char *source, const char *token) {
    int count = 0;
    const char *cursor = source;
    while ((cursor = strstr(cursor, token)) != NULL) {
        count += 1;
        cursor += strlen(token);
    }
    return count;
}

static void set_error(CompileStats *stats, const char *message, const char *path) {
    if (stats->error[0] != '\0') {
        return;
    }
    snprintf(stats->error, sizeof(stats->error), "%s: %s", message, path);
}

static int validate_braces(const char *source, const char *path, CompileStats *stats) {
    int depth = 0;
    int line_comment = 0;
    int block_comment = 0;
    int string_literal = 0;

    for (size_t index = 0; source[index] != '\0'; index += 1) {
        char current = source[index];
        char next = source[index + 1];

        if (line_comment) {
            if (current == '\n') {
                line_comment = 0;
            }
            continue;
        }

        if (block_comment) {
            if (current == '*' && next == '/') {
                block_comment = 0;
                index += 1;
            }
            continue;
        }

        if (string_literal) {
            if (current == '\\' && next != '\0') {
                index += 1;
                continue;
            }
            if (current == '"') {
                string_literal = 0;
            }
            continue;
        }

        if (current == '/' && next == '/') {
            line_comment = 1;
            index += 1;
            continue;
        }

        if (current == '/' && next == '*') {
            block_comment = 1;
            index += 1;
            continue;
        }

        if (current == '"') {
            string_literal = 1;
            continue;
        }

        if (current == '{') {
            depth += 1;
        } else if (current == '}') {
            depth -= 1;
            if (depth < 0) {
                set_error(stats, "CompileError: unmatched closing brace", path);
                return -1;
            }
        }
    }

    if (depth != 0) {
        set_error(stats, "CompileError: unmatched opening brace", path);
        return -1;
    }

    if (block_comment) {
        set_error(stats, "CompileError: unterminated block comment", path);
        return -1;
    }

    if (string_literal) {
        set_error(stats, "CompileError: unterminated string literal", path);
        return -1;
    }

    return 0;
}

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

static int analyze_stasis_file(const char *path, CompileStats *stats) {
    long size = 0;
    char *source = read_file_text(path, &size);
    if (source == NULL) {
        set_error(stats, "CompileError: unreadable file", path);
        return -1;
    }

    if (validate_braces(source, path, stats) != 0) {
        free(source);
        return -1;
    }

    stats->file_count += 1;
    stats->byte_count += size;
    stats->function_count += count_token(source, "function ");
    stats->struct_count += count_token(source, "struct ");
    stats->global_count += count_token(source, "global ");

    if (strstr(source, "function main(") != NULL) {
        stats->has_main = 1;
    }
    if (strstr(source, "function tick(") != NULL) {
        stats->has_tick = 1;
    }
    if (strstr(source, "function on_code_swap(") != NULL) {
        stats->has_on_code_swap = 1;
    }

    free(source);
    return 0;
}

static int scan_stasis_files(const char *path, CompileStats *stats) {
    DIR *dir = opendir(path);
    if (dir == NULL) {
        set_error(stats, "CompileError: unable to open project root", path);
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
            set_error(stats, "CompileError: out of memory", path);
            return -1;
        }
        memcpy(child, path, path_len);
        child[path_len] = '/';
        memcpy(child + path_len + 1, entry->d_name, name_len + 1);

        struct stat info;
        if (stat(child, &info) == 0) {
            if (S_ISDIR(info.st_mode)) {
                int result = scan_stasis_files(child, stats);
                free(child);
                if (result != 0) {
                    closedir(dir);
                    return result;
                }
                continue;
            }

            if (S_ISREG(info.st_mode) && has_suffix(entry->d_name, ".stasis")) {
                int result = analyze_stasis_file(child, stats);
                free(child);
                if (result != 0) {
                    closedir(dir);
                    return result;
                }
                continue;
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
        return (*env)->NewStringUTF(env, "CompileError: unable to read project root");
    }

    CompileStats stats;
    memset(&stats, 0, sizeof(stats));
    int result = scan_stasis_files(root, &stats);
    (*env)->ReleaseStringUTFChars(env, project_root, root);

    char message[256];
    if (result != 0 || stats.error[0] != '\0') {
        snprintf(message, sizeof(message), "%s", stats.error[0] == '\0' ? "CompileError: unknown native check failure" : stats.error);
    } else if (stats.file_count == 0) {
        snprintf(message, sizeof(message), "CompileError: no .stasis files found");
    } else if (!stats.has_main || !stats.has_tick) {
        snprintf(message, sizeof(message), "CompileError: missing lifecycle root main=%d tick=%d", stats.has_main, stats.has_tick);
    } else {
        snprintf(
                message,
                sizeof(message),
                "CompileChecked: files=%d bytes=%ld functions=%d structs=%d globals=%d roots=main,tick%s",
                stats.file_count,
                stats.byte_count,
                stats.function_count,
                stats.struct_count,
                stats.global_count,
                stats.has_on_code_swap ? ",on_code_swap" : "");
    }

    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "%s", message);
    return (*env)->NewStringUTF(env, message);
}