#include <jni.h>
#include <android/log.h>
#include <dirent.h>
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#define STASIS_ANDROID_LOG_TAG "StasisWorkshop"
#define STASIS_COMPILE_MANIFEST_RELATIVE_PATH "build/native_compile_manifest.txt"
#define STASIS_FUNCTION_ARTIFACT_DIR "build/functions"
#define STASIS_RUNTIME_STATE_RELATIVE_PATH "build/runtime_state.txt"
#define FNV_OFFSET_BASIS 1469598103934665603ULL
#define FNV_PRIME 1099511628211ULL

typedef char *(*stasis_android_bridge_compile_project_fn)(const char *project_root, const char *entry_file);
typedef char *(*stasis_android_bridge_run_tick_fn)(const char *project_root, const char *entry_file, int touch_y, int touch_active, int screen_w, int screen_h);
typedef void (*stasis_android_bridge_free_string_fn)(char *value);
typedef struct CompileStats {
    int file_count;
    int function_count;
    int struct_count;
    int global_count;
    int has_main;
    int has_tick;
    int has_on_code_swap;
    long byte_count;
    uint64_t project_hash;
    char error[160];
} CompileStats;

typedef struct PreviousManifest {
    int found;
    int functions;
    int structs;
    int globals;
    uint64_t project_hash;
} PreviousManifest;
typedef struct RustBridgeApi {
    void *handle;
    stasis_android_bridge_compile_project_fn compile_project;
    stasis_android_bridge_run_tick_fn run_tick;
    stasis_android_bridge_free_string_fn free_string;
    int attempted;
} RustBridgeApi;

static RustBridgeApi rust_bridge_api = {0};
static char *read_file_text(const char *path, long *size_out);

static int has_suffix(const char *value, const char *suffix) {
    size_t value_len = strlen(value);
    size_t suffix_len = strlen(suffix);
    if (value_len < suffix_len) {
        return 0;
    }
    return strcmp(value + value_len - suffix_len, suffix) == 0;
}

static void hash_bytes(CompileStats *stats, const char *value, size_t length) {
    for (size_t index = 0; index < length; index += 1) {
        stats->project_hash ^= (unsigned char)value[index];
        stats->project_hash *= FNV_PRIME;
    }
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
static uint64_t hash_slice(const char *source, size_t length) {
    uint64_t hash = FNV_OFFSET_BASIS;
    for (size_t index = 0; index < length; index += 1) {
        hash ^= (unsigned char)source[index];
        hash *= FNV_PRIME;
    }
    return hash;
}

static const char *find_matching_function_end(const char *body_start) {
    int depth = 0;
    int line_comment = 0;
    int block_comment = 0;
    int string_literal = 0;

    for (const char *cursor = body_start; *cursor != '\0'; cursor += 1) {
        char current = *cursor;
        char next = *(cursor + 1);

        if (line_comment) {
            if (current == '\n') {
                line_comment = 0;
            }
            continue;
        }
        if (block_comment) {
            if (current == '*' && next == '/') {
                block_comment = 0;
                cursor += 1;
            }
            continue;
        }
        if (string_literal) {
            if (current == '\\' && next != '\0') {
                cursor += 1;
                continue;
            }
            if (current == '"') {
                string_literal = 0;
            }
            continue;
        }
        if (current == '/' && next == '/') {
            line_comment = 1;
            cursor += 1;
            continue;
        }
        if (current == '/' && next == '*') {
            block_comment = 1;
            cursor += 1;
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
            if (depth == 0) {
                return cursor + 1;
            }
        }
    }

    return NULL;
}

static void write_escaped_manifest_field(FILE *file, const char *start, size_t length) {
    for (size_t index = 0; index < length; index += 1) {
        char value = start[index];
        if (value == '\n' || value == '\r' || value == '|') {
            fputc(' ', file);
        } else {
            fputc(value, file);
        }
    }
}

static int write_function_artifact(
        const char *artifact_dir,
        const char *path,
        const char *signature_start,
        size_t signature_length,
        uint64_t signature_hash,
        uint64_t body_hash) {
    char artifact_path[1200];
    snprintf(artifact_path, sizeof(artifact_path), "%s/%016llx.stub", artifact_dir, (unsigned long long)body_hash);

    FILE *artifact = fopen(artifact_path, "wb");
    if (artifact == NULL) {
        return -1;
    }

    fprintf(artifact, "status=CompiledStub\n");
    fprintf(artifact, "source=");
    write_escaped_manifest_field(artifact, path, strlen(path));
    fprintf(artifact, "\n");
    fprintf(artifact, "signature=");
    write_escaped_manifest_field(artifact, signature_start, signature_length);
    fprintf(artifact, "\n");
    fprintf(artifact, "signature_hash=%016llx\n", (unsigned long long)signature_hash);
    fprintf(artifact, "body_hash=%016llx\n", (unsigned long long)body_hash);
    fclose(artifact);
    return 0;
}
static int write_function_manifest_entries(FILE *manifest, const char *artifact_dir, const char *path, const char *source) {
    const char *cursor = source;
    while ((cursor = strstr(cursor, "function ")) != NULL) {
        const char *signature_start = cursor + strlen("function ");
        const char *body_start = strchr(signature_start, '{');
        if (body_start == NULL) {
            break;
        }

        const char *body_end = find_matching_function_end(body_start);
        if (body_end == NULL) {
            break;
        }

        const char *signature_end = body_start;
        while (signature_end > signature_start && (*(signature_end - 1) == ' ' || *(signature_end - 1) == '\n' || *(signature_end - 1) == '\r' || *(signature_end - 1) == '\t')) {
            signature_end -= 1;
        }

        uint64_t signature_hash = hash_slice(signature_start, (size_t)(signature_end - signature_start));
        uint64_t body_hash = hash_slice(body_start, (size_t)(body_end - body_start));
        fprintf(manifest, "function=");
        write_escaped_manifest_field(manifest, path, strlen(path));
        fprintf(manifest, "|");
        write_escaped_manifest_field(manifest, signature_start, (size_t)(signature_end - signature_start));
        fprintf(
                manifest,
                "|signature_hash=%016llx|body_hash=%016llx|artifact=%s/%016llx.stub\n",
                (unsigned long long)signature_hash,
                (unsigned long long)body_hash,
                STASIS_FUNCTION_ARTIFACT_DIR,
                (unsigned long long)body_hash);

        if (write_function_artifact(
                artifact_dir,
                path,
                signature_start,
                (size_t)(signature_end - signature_start),
                signature_hash,
                body_hash) != 0) {
            return -1;
        }

        cursor = body_end;
    }
    return 0;
}

static int append_function_entries_for_project(FILE *manifest, const char *artifact_dir, const char *path) {
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
            return -1;
        }
        memcpy(child, path, path_len);
        child[path_len] = '/';
        memcpy(child + path_len + 1, entry->d_name, name_len + 1);

        struct stat info;
        if (stat(child, &info) == 0) {
            if (S_ISDIR(info.st_mode)) {
                int result = append_function_entries_for_project(manifest, artifact_dir, child);
                free(child);
                if (result != 0) {
                    closedir(dir);
                    return result;
                }
                continue;
            }
            if (S_ISREG(info.st_mode) && has_suffix(entry->d_name, ".stasis")) {
                long size = 0;
                char *source = read_file_text(child, &size);
                (void)size;
                if (source == NULL) {
                    free(child);
                    closedir(dir);
                    return -1;
                }
                if (write_function_manifest_entries(manifest, artifact_dir, child, source) != 0) {
                    free(source);
                    free(child);
                    closedir(dir);
                    return -1;
                }
                free(source);
            }
        }
        free(child);
    }

    closedir(dir);
    return 0;
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

static int parse_manifest_i32(const char *manifest, const char *key, int *out) {
    const char *cursor = strstr(manifest, key);
    if (cursor == NULL) {
        return 0;
    }
    cursor += strlen(key);
    *out = atoi(cursor);
    return 1;
}

static int parse_manifest_u64(const char *manifest, const char *key, uint64_t *out) {
    const char *cursor = strstr(manifest, key);
    if (cursor == NULL) {
        return 0;
    }
    cursor += strlen(key);
    *out = (uint64_t)strtoull(cursor, NULL, 16);
    return 1;
}

static void read_previous_compile_manifest(const char *project_root, PreviousManifest *previous) {
    memset(previous, 0, sizeof(*previous));

    char manifest_path[1200];
    snprintf(manifest_path, sizeof(manifest_path), "%s/%s", project_root, STASIS_COMPILE_MANIFEST_RELATIVE_PATH);

    long size = 0;
    char *manifest = read_file_text(manifest_path, &size);
    if (manifest == NULL || size == 0) {
        free(manifest);
        return;
    }

    previous->found = 1;
    parse_manifest_u64(manifest, "project_hash=", &previous->project_hash);
    parse_manifest_i32(manifest, "functions=", &previous->functions);
    parse_manifest_i32(manifest, "structs=", &previous->structs);
    parse_manifest_i32(manifest, "globals=", &previous->globals);
    free(manifest);
}

static const char *classify_reload(const CompileStats *stats, const PreviousManifest *previous) {
    if (!previous->found) {
        return "InitialCompile";
    }
    if (previous->project_hash == stats->project_hash) {
        return "NoChange";
    }
    if (previous->functions != stats->function_count ||
        previous->structs != stats->struct_count ||
        previous->globals != stats->global_count) {
        return "ResetRequired";
    }
    return "FastReload";
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
    hash_bytes(stats, path, strlen(path));
    hash_bytes(stats, "\n", 1);
    hash_bytes(stats, source, (size_t)size);

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

static int ensure_directory(const char *path, CompileStats *stats) {
    struct stat info;
    if (stat(path, &info) == 0) {
        if (S_ISDIR(info.st_mode)) {
            return 0;
        }
        set_error(stats, "CompileError: build path is not a directory", path);
        return -1;
    }

    if (mkdir(path, 0700) != 0) {
        set_error(stats, "CompileError: unable to create build directory", path);
        return -1;
    }
    return 0;
}

static int write_runtime_state(const char *project_root, const CompileStats *stats, const char *reload_classification) {
    if (strcmp(reload_classification, "NoChange") == 0 || strcmp(reload_classification, "FastReload") == 0) {
        return 0;
    }

    char state_path[1200];
    snprintf(state_path, sizeof(state_path), "%s/%s", project_root, STASIS_RUNTIME_STATE_RELATIVE_PATH);

    FILE *state = fopen(state_path, "wb");
    if (state == NULL) {
        return -1;
    }

    fprintf(state, "status=RuntimeStateReady\n");
    fprintf(state, "project_hash=%016llx\n", (unsigned long long)stats->project_hash);
    fprintf(state, "reload=%s\n", reload_classification);
    fprintf(state, "tick_count=0\n");
    fprintf(state, "globals=%d\n", stats->global_count);
    fclose(state);
    return 0;
}

static int write_compile_manifest(const char *project_root, const CompileStats *stats, const char *reload_classification) {
    char build_dir[1024];
    snprintf(build_dir, sizeof(build_dir), "%s/build", project_root);

    CompileStats mutable_stats = *stats;
    if (ensure_directory(build_dir, &mutable_stats) != 0) {
        return -1;
    }

    char artifact_dir[1024];
    snprintf(artifact_dir, sizeof(artifact_dir), "%s/%s", project_root, STASIS_FUNCTION_ARTIFACT_DIR);
    if (ensure_directory(artifact_dir, &mutable_stats) != 0) {
        return -1;
    }

    char manifest_path[1200];
    snprintf(manifest_path, sizeof(manifest_path), "%s/%s", project_root, STASIS_COMPILE_MANIFEST_RELATIVE_PATH);

    FILE *file = fopen(manifest_path, "wb");
    if (file == NULL) {
        return -1;
    }

    fprintf(file, "status=CompilePlanned\n");
    fprintf(file, "reload=%s\n", reload_classification);
    fprintf(file, "project_hash=%016llx\n", (unsigned long long)stats->project_hash);
    fprintf(file, "files=%d\n", stats->file_count);
    fprintf(file, "bytes=%ld\n", stats->byte_count);
    fprintf(file, "functions=%d\n", stats->function_count);
    fprintf(file, "structs=%d\n", stats->struct_count);
    fprintf(file, "globals=%d\n", stats->global_count);
    fprintf(file, "roots=main,tick%s\n", stats->has_on_code_swap ? ",on_code_swap" : "");
    fprintf(file, "entrypoint=main\n");
    fprintf(file, "entrypoint=tick\n");
    if (stats->has_on_code_swap) {
        fprintf(file, "entrypoint=on_code_swap\n");
    }
    fprintf(file, "runtime_state=%s\n", STASIS_RUNTIME_STATE_RELATIVE_PATH);
    fclose(file);

    if (write_runtime_state(project_root, stats, reload_classification) != 0) {
        return -1;
    }
    return 0;
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

    rust_bridge_api.compile_project =
            (stasis_android_bridge_compile_project_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_compile_project");
    rust_bridge_api.run_tick =
            (stasis_android_bridge_run_tick_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tick");
    rust_bridge_api.free_string =
            (stasis_android_bridge_free_string_fn)dlsym(rust_bridge_api.handle, "stasis_android_bridge_free_string");
    if (rust_bridge_api.compile_project == NULL ||
        rust_bridge_api.run_tick == NULL ||
        rust_bridge_api.free_string == NULL) {
        __android_log_print(ANDROID_LOG_WARN, STASIS_ANDROID_LOG_TAG, "Rust Android bridge missing required symbols");
        return NULL;
    }

    return &rust_bridge_api;
}

static int try_rust_bridge_compile(const char *project_root, char *message, size_t message_size) {
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->compile_project == NULL || bridge->free_string == NULL) {
        return 0;
    }

    char *bridge_message = bridge->compile_project(project_root, "src/main.stasis");
    if (bridge_message == NULL) {
        snprintf(message, message_size, "CompileError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
}

static int try_rust_bridge_run_tick(const char *project_root, int touch_y, int touch_active, int screen_w, int screen_h, char *message, size_t message_size) {
    RustBridgeApi *bridge = load_rust_bridge_api();
    if (bridge == NULL || bridge->run_tick == NULL || bridge->free_string == NULL) {
        return 0;
    }

    char *bridge_message = bridge->run_tick(project_root, "src/main.stasis", touch_y, touch_active, screen_w, screen_h);
    if (bridge_message == NULL) {
        snprintf(message, message_size, "RunError: Rust Android bridge returned null message");
        return 1;
    }

    snprintf(message, message_size, "%s", bridge_message);
    bridge->free_string(bridge_message);
    return 1;
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

    char message[256];
    if (try_rust_bridge_compile(root, message, sizeof(message)) != 0) {
        (*env)->ReleaseStringUTFChars(env, project_root, root);
        __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "%s", message);
        return (*env)->NewStringUTF(env, message);
    }

    CompileStats stats;
    memset(&stats, 0, sizeof(stats));
    stats.project_hash = FNV_OFFSET_BASIS;
    int result = scan_stasis_files(root, &stats);
    PreviousManifest previous;
    read_previous_compile_manifest(root, &previous);
    const char *reload_classification = classify_reload(&stats, &previous);

    if (result != 0 || stats.error[0] != '\0') {
        snprintf(message, sizeof(message), "%s", stats.error[0] == '\0' ? "CompileError: unknown native check failure" : stats.error);
    } else if (stats.file_count == 0) {
        snprintf(message, sizeof(message), "CompileError: no .stasis files found");
    } else if (!stats.has_main || !stats.has_tick) {
        snprintf(message, sizeof(message), "CompileError: missing lifecycle root main=%d tick=%d", stats.has_main, stats.has_tick);
    } else if (write_compile_manifest(root, &stats, reload_classification) != 0) {
        snprintf(message, sizeof(message), "CompileError: unable to write native compile manifest");
    } else {
        snprintf(
                message,
                sizeof(message),
                "CompilePlanned: reload=%s files=%d functions=%d hash=%016llx manifest=%s state=%s",
                reload_classification,
                stats.file_count,
                stats.function_count,
                (unsigned long long)stats.project_hash,
                STASIS_COMPILE_MANIFEST_RELATIVE_PATH,
                STASIS_RUNTIME_STATE_RELATIVE_PATH);
    }

    (*env)->ReleaseStringUTFChars(env, project_root, root);
    __android_log_print(ANDROID_LOG_INFO, STASIS_ANDROID_LOG_TAG, "%s", message);
    return (*env)->NewStringUTF(env, message);
}

JNIEXPORT jstring JNICALL
Java_com_stasislang_workshop_MainActivity_nativeRunTick(JNIEnv *env, jclass activity_class, jstring project_root, jint touch_y, jint touch_active, jint screen_w, jint screen_h) {
    (void)activity_class;

    const char *root = (*env)->GetStringUTFChars(env, project_root, NULL);
    if (root == NULL) {
        return (*env)->NewStringUTF(env, "RunError: unable to read project root");
    }

    char message[1024];
    if (try_rust_bridge_run_tick(root, (int)touch_y, (int)touch_active, (int)screen_w, (int)screen_h, message, sizeof(message)) != 0) {
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

