/*
 * stasis_data.c - Data hot-reload system for Stasis
 *
 * Provides fast (<50ms) data binding from JSON files to Stasis structs.
 * The compiler emits struct metadata that maps JSON paths to memory offsets.
 *
 * Architecture note: Since game DLLs are hot-swapped, we store symbol names
 * (not addresses) and look up current addresses via GetProcAddress when applying.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <time.h>

#ifdef _WIN32
#include <windows.h>
#include <sys/stat.h>
#else
#include <dlfcn.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

#include "cJSON.h"

/* Maximum number of data bindings */
#define MAX_DATA_BINDINGS 32

/* Maximum number of fields per binding */
#define MAX_FIELDS_PER_BINDING 256

/* Field type enum (must match compiler's FieldType) */
typedef enum {
    FIELD_TYPE_UNKNOWN = 0,
    FIELD_TYPE_BOOL,
    FIELD_TYPE_U8,
    FIELD_TYPE_U16,
    FIELD_TYPE_U32,
    FIELD_TYPE_I32,
    FIELD_TYPE_F32,
    FIELD_TYPE_F64,
    FIELD_TYPE_STRING
} FieldType;

/* Field metadata from struct-meta.json */
typedef struct {
    char name[256];        /* Flattened symbol name (e.g., "state_player_health") */
    char json_path[256];   /* Dot-separated path for JSON lookup (e.g., "player.health") */
    int offset;            /* Offset within struct (for reference) */
    int size;
    FieldType type;
    int array_count;
} FieldMeta;

/* A single data binding */
typedef struct {
    int active;
    char json_file_path[512];
    char struct_meta_path[512];
    FieldMeta fields[MAX_FIELDS_PER_BINDING];
    int field_count;
    int64_t last_mtime;        /* Last modification time */
    uint64_t last_apply_ns;    /* Last reload duration (ns) */
    int has_error;
    char error_msg[256];
} DataBinding;

/* Current DLL handle for symbol lookup (set by runner) */
#ifdef _WIN32
static HMODULE g_current_dll = NULL;
#else
static void* g_current_dll = NULL;
#endif

/* Global state */
static DataBinding g_bindings[MAX_DATA_BINDINGS];
static int g_binding_count = 0;
static int g_initialized = 0;

static uint64_t now_ns(void) {
#ifdef _WIN32
    static LARGE_INTEGER freq;
    static int initialized = 0;
    LARGE_INTEGER t;
    if (!initialized) {
        QueryPerformanceFrequency(&freq);
        initialized = 1;
    }
    QueryPerformanceCounter(&t);
    return (uint64_t)((t.QuadPart * 1000000000ULL) / (uint64_t)freq.QuadPart);
#else
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
#endif
}

/* Get file modification time */
static int64_t get_file_mtime(const char* path) {
#ifdef _WIN32
    WIN32_FILE_ATTRIBUTE_DATA data;
    if (!GetFileAttributesExA(path, GetFileExInfoStandard, &data)) {
        return -1;
    }
    ULARGE_INTEGER ft;
    ft.LowPart = data.ftLastWriteTime.dwLowDateTime;
    ft.HighPart = data.ftLastWriteTime.dwHighDateTime;
    return (int64_t)ft.QuadPart; /* 100ns ticks since 1601-01-01 */
#else
    struct stat st;
    if (stat(path, &st) != 0) {
        return -1;
    }
#if defined(__APPLE__)
    return (int64_t)st.st_mtimespec.tv_sec * 1000000000LL + (int64_t)st.st_mtimespec.tv_nsec;
#else
    return (int64_t)st.st_mtim.tv_sec * 1000000000LL + (int64_t)st.st_mtim.tv_nsec;
#endif
#endif
}

/* Parse field type from string */
static FieldType parse_field_type(const char* type_str) {
    if (!type_str) return FIELD_TYPE_UNKNOWN;
    if (strcmp(type_str, "bool") == 0) return FIELD_TYPE_BOOL;
    if (strcmp(type_str, "u8") == 0) return FIELD_TYPE_U8;
    if (strcmp(type_str, "u16") == 0) return FIELD_TYPE_U16;
    if (strcmp(type_str, "u32") == 0) return FIELD_TYPE_U32;
    if (strcmp(type_str, "i32") == 0) return FIELD_TYPE_I32;
    if (strcmp(type_str, "f32") == 0) return FIELD_TYPE_F32;
    if (strcmp(type_str, "f64") == 0) return FIELD_TYPE_F64;
    if (strcmp(type_str, "string") == 0) return FIELD_TYPE_STRING;
    return FIELD_TYPE_UNKNOWN;
}

/* Load struct metadata from JSON file */
static int load_struct_meta(DataBinding* binding, const char* meta_path) {
    FILE* f = fopen(meta_path, "rb");
    if (!f) {
        snprintf(binding->error_msg, sizeof(binding->error_msg),
                 "Failed to open struct meta: %s", meta_path);
        return 0;
    }

    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);

    char* content = (char*)malloc(size + 1);
    if (!content) {
        fclose(f);
        snprintf(binding->error_msg, sizeof(binding->error_msg), "Out of memory");
        return 0;
    }

    size_t read = fread(content, 1, size, f);
    fclose(f);
    content[read] = '\0';

    cJSON* root = cJSON_Parse(content);
    free(content);

    if (!root) {
        snprintf(binding->error_msg, sizeof(binding->error_msg),
                 "Failed to parse struct meta JSON");
        return 0;
    }

    cJSON* version = cJSON_GetObjectItem(root, "version");
    if (!cJSON_IsNumber(version) || version->valueint != 1) {
        snprintf(binding->error_msg, sizeof(binding->error_msg),
                 "Unsupported struct meta version");
        cJSON_Delete(root);
        return 0;
    }

    /* totalSize is in the metadata but we don't need it for symbol-based lookup */

    cJSON* fields = cJSON_GetObjectItem(root, "fields");
    if (!cJSON_IsArray(fields)) {
        snprintf(binding->error_msg, sizeof(binding->error_msg),
                 "No fields array in struct meta");
        cJSON_Delete(root);
        return 0;
    }

    binding->field_count = 0;
    cJSON* field;
    cJSON_ArrayForEach(field, fields) {
        if (binding->field_count >= MAX_FIELDS_PER_BINDING) break;

        FieldMeta* fm = &binding->fields[binding->field_count];

        cJSON* name = cJSON_GetObjectItem(field, "name");
        cJSON* json_path = cJSON_GetObjectItem(field, "jsonPath");
        cJSON* offset = cJSON_GetObjectItem(field, "offset");
        cJSON* size = cJSON_GetObjectItem(field, "size");
        cJSON* type = cJSON_GetObjectItem(field, "type");
        cJSON* array_count = cJSON_GetObjectItem(field, "arrayCount");

        if (cJSON_IsString(name)) {
            strncpy(fm->name, name->valuestring, sizeof(fm->name) - 1);
        }
        if (cJSON_IsString(json_path)) {
            strncpy(fm->json_path, json_path->valuestring, sizeof(fm->json_path) - 1);
        }
        if (cJSON_IsNumber(offset)) {
            fm->offset = offset->valueint;
        }
        if (cJSON_IsNumber(size)) {
            fm->size = size->valueint;
        }
        if (cJSON_IsString(type)) {
            fm->type = parse_field_type(type->valuestring);
        }
        if (cJSON_IsNumber(array_count)) {
            fm->array_count = array_count->valueint;
        } else {
            fm->array_count = 1;
        }

        binding->field_count++;
    }

    cJSON_Delete(root);
    strncpy(binding->struct_meta_path, meta_path, sizeof(binding->struct_meta_path) - 1);
    return 1;
}

/* Navigate JSON by dot-separated path */
static cJSON* json_get_by_path(cJSON* root, const char* path) {
    if (!root || !path || !*path) return NULL;

    char path_copy[256];
    strncpy(path_copy, path, sizeof(path_copy) - 1);
    path_copy[sizeof(path_copy) - 1] = '\0';

    cJSON* current = root;
    char* token = strtok(path_copy, ".");

    while (token && current) {
        current = cJSON_GetObjectItem(current, token);
        token = strtok(NULL, ".");
    }

    return current;
}

/* Get symbol address from current DLL */
static void* get_symbol_addr(const char* name) {
    if (!g_current_dll || !name) return NULL;
#ifdef _WIN32
    return (void*)GetProcAddress(g_current_dll, name);
#else
    return dlsym(g_current_dll, name);
#endif
}

static void apply_scalar_value_to_dest(FieldType type, void* dest, cJSON* value) {
    if (!dest || !value) return;

    switch (type) {
        case FIELD_TYPE_BOOL:
            if (cJSON_IsBool(value)) {
                *((uint8_t*)dest) = cJSON_IsTrue(value) ? 1 : 0;
            } else if (cJSON_IsNumber(value)) {
                *((uint8_t*)dest) = value->valueint ? 1 : 0;
            }
            break;

        case FIELD_TYPE_U8:
            if (cJSON_IsNumber(value)) {
                *((uint8_t*)dest) = (uint8_t)value->valueint;
            }
            break;

        case FIELD_TYPE_U16:
            if (cJSON_IsNumber(value)) {
                *((uint16_t*)dest) = (uint16_t)value->valueint;
            }
            break;

        case FIELD_TYPE_U32:
            if (cJSON_IsNumber(value)) {
                *((uint32_t*)dest) = (uint32_t)value->valueint;
            }
            break;

        case FIELD_TYPE_I32:
            if (cJSON_IsNumber(value)) {
                *((int32_t*)dest) = (int32_t)value->valueint;
            }
            break;

        case FIELD_TYPE_F32:
            if (cJSON_IsNumber(value)) {
                *((float*)dest) = (float)value->valuedouble;
            }
            break;

        case FIELD_TYPE_F64:
            if (cJSON_IsNumber(value)) {
                *((double*)dest) = value->valuedouble;
            }
            break;

        case FIELD_TYPE_STRING:
            /* String handling would need special care for buffer size */
            break;

        default:
            break;
    }
}

static int field_element_bytes(const FieldMeta* field) {
    if (!field) return 0;

    if (field->array_count > 1 && field->size > 0) {
        int elem = field->size / field->array_count;
        if (elem > 0 && (elem * field->array_count) == field->size) {
            return elem;
        }
    }

    switch (field->type) {
        case FIELD_TYPE_BOOL:
        case FIELD_TYPE_U8:
            return 1;
        case FIELD_TYPE_U16:
            return 2;
        case FIELD_TYPE_U32:
        case FIELD_TYPE_I32:
        case FIELD_TYPE_F32:
            return 4;
        case FIELD_TYPE_F64:
            return 8;
        default:
            return 0;
    }
}

/* Apply a single field value from JSON to memory via symbol lookup */
static void apply_field_value(FieldMeta* field, cJSON* value) {
    if (!field || !value) return;

    void* dest = get_symbol_addr(field->name);
    if (!dest) {
        /* Symbol not found - skip silently */
        return;
    }

    apply_scalar_value_to_dest(field->type, dest, value);
}

/* Load and apply JSON data file to bound memory via symbol lookup */
static int apply_data_file(DataBinding* binding) {
    if (!g_current_dll) {
        snprintf(binding->error_msg, sizeof(binding->error_msg),
                 "No DLL handle set - call stasis_data_set_dll first");
        binding->has_error = 1;
        return 0;
    }

    uint64_t t0 = now_ns();

    FILE* f = fopen(binding->json_file_path, "rb");
    if (!f) {
        snprintf(binding->error_msg, sizeof(binding->error_msg),
                 "Failed to open data file: %s", binding->json_file_path);
        binding->has_error = 1;
        return 0;
    }

    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);

    char* content = (char*)malloc(size + 1);
    if (!content) {
        fclose(f);
        snprintf(binding->error_msg, sizeof(binding->error_msg), "Out of memory");
        binding->has_error = 1;
        return 0;
    }

    size_t read = fread(content, 1, size, f);
    fclose(f);
    content[read] = '\0';

    cJSON* root = cJSON_Parse(content);
    free(content);

    if (!root) {
        const char* error_ptr = cJSON_GetErrorPtr();
        snprintf(binding->error_msg, sizeof(binding->error_msg),
                 "JSON parse error near: %.20s", error_ptr ? error_ptr : "unknown");
        binding->has_error = 1;
        binding->last_apply_ns = now_ns() - t0;
        return 0;
    }

    /* Apply each field via symbol lookup */
    for (int i = 0; i < binding->field_count; i++) {
        FieldMeta* field = &binding->fields[i];
        void* dest = get_symbol_addr(field->name);
        if (!dest) {
            continue;
        }

        /* Prefer non-lowered jsonPath; fall back to flattened symbol name as a key. */
        cJSON* value = json_get_by_path(root, field->json_path);
        if (!value && strcmp(field->json_path, field->name) != 0) {
            value = json_get_by_path(root, field->name);
        }

        if (!value) {
            /* AoS-style arrays: path like "asteroids.x" where "asteroids" is a JSON array. */
            if (field->array_count > 1) {
                const char* dot = strrchr(field->json_path, '.');
                if (dot && dot != field->json_path) {
                    char base_path[256];
                    size_t base_len = (size_t)(dot - field->json_path);
                    if (base_len < sizeof(base_path)) {
                        memcpy(base_path, field->json_path, base_len);
                        base_path[base_len] = '\0';
                        const char* leaf = dot + 1;
                        cJSON* base_value = json_get_by_path(root, base_path);
                        if (cJSON_IsArray(base_value) && leaf && *leaf) {
                            int elem_bytes = field_element_bytes(field);
                            if (elem_bytes > 0) {
                                int n = cJSON_GetArraySize(base_value);
                                int limit = n < field->array_count ? n : field->array_count;
                                for (int j = 0; j < limit; j++) {
                                    cJSON* obj = cJSON_GetArrayItem(base_value, j);
                                    if (!cJSON_IsObject(obj)) continue;
                                    cJSON* v = cJSON_GetObjectItem(obj, leaf);
                                    if (!v) continue;
                                    apply_scalar_value_to_dest(field->type, (uint8_t*)dest + (j * elem_bytes), v);
                                }
                            }
                        }
                    }
                }
            }

            continue;
        }

        if (field->array_count > 1 && cJSON_IsArray(value)) {
            int elem_bytes = field_element_bytes(field);
            if (elem_bytes > 0) {
                int n = cJSON_GetArraySize(value);
                int limit = n < field->array_count ? n : field->array_count;
                for (int j = 0; j < limit; j++) {
                    cJSON* v = cJSON_GetArrayItem(value, j);
                    if (!v) continue;
                    apply_scalar_value_to_dest(field->type, (uint8_t*)dest + (j * elem_bytes), v);
                }
            }
        } else {
            apply_scalar_value_to_dest(field->type, dest, value);
        }
    }

    cJSON_Delete(root);
    binding->has_error = 0;
    binding->error_msg[0] = '\0';
    binding->last_apply_ns = now_ns() - t0;
    return 1;
}

/* ============================================================
 * Public API
 * ============================================================ */

/*
 * Initialize the data binding system.
 * Called automatically on first use.
 */
void stasis_data_init(void) {
    if (g_initialized) return;
    memset(g_bindings, 0, sizeof(g_bindings));
    g_binding_count = 0;
    g_initialized = 1;
}

/*
 * Set the current DLL handle for symbol lookup.
 * Must be called before polling, and updated after each hot-swap.
 */
#ifdef _WIN32
void stasis_data_set_dll(HMODULE dll) {
    g_current_dll = dll;
    /* After hot-swap, addresses change even if the JSON file did not. Re-apply bindings. */
    if (g_current_dll)
    {
        for (int i = 0; i < MAX_DATA_BINDINGS; i++)
        {
            if (g_bindings[i].active)
            {
                (void)apply_data_file(&g_bindings[i]);
            }
        }
    }
}
#else
void stasis_data_set_dll(void* dll) {
    g_current_dll = dll;
    /* After hot-swap, addresses change even if the JSON file did not. Re-apply bindings. */
    if (g_current_dll)
    {
        for (int i = 0; i < MAX_DATA_BINDINGS; i++)
        {
            if (g_bindings[i].active)
            {
                (void)apply_data_file(&g_bindings[i]);
            }
        }
    }
}
#endif

/*
 * Register a data binding.
 *
 * @param json_file_path   Path to the JSON data file
 * @param struct_meta_path Path to the struct-meta.json file (compiler-generated)
 * @return Handle (1-based), or 0 on failure
 */
int stasis_data_bind(const char* json_file_path, const char* struct_meta_path) {
    if (!g_initialized) stasis_data_init();

    if (!json_file_path || !struct_meta_path) {
        fprintf(stderr, "data_bind: invalid arguments\n");
        return 0;
    }

    if (g_binding_count >= MAX_DATA_BINDINGS) {
        fprintf(stderr, "data_bind: max bindings reached\n");
        return 0;
    }

    /* Find free slot */
    int slot = -1;
    for (int i = 0; i < MAX_DATA_BINDINGS; i++) {
        if (!g_bindings[i].active) {
            slot = i;
            break;
        }
    }

    if (slot < 0) {
        fprintf(stderr, "data_bind: no free slots\n");
        return 0;
    }

    DataBinding* binding = &g_bindings[slot];
    memset(binding, 0, sizeof(*binding));

    strncpy(binding->json_file_path, json_file_path, sizeof(binding->json_file_path) - 1);

    /* Load struct metadata */
    if (!load_struct_meta(binding, struct_meta_path)) {
        fprintf(stderr, "data_bind: %s\n", binding->error_msg);
        return 0;
    }

    /* Initial load of data (if DLL is set) */
    binding->last_mtime = get_file_mtime(json_file_path);
    if (g_current_dll) {
        if (!apply_data_file(binding)) {
            fprintf(stderr, "data_bind: %s\n", binding->error_msg);
            /* Don't fail - file might not exist yet */
        }
    }

    binding->active = 1;
    g_binding_count++;

    fprintf(stderr, "DATABIND: registered %s (%d fields)\n",
            json_file_path, binding->field_count);

    return slot + 1; /* 1-based handle */
}

/*
 * Check for data file changes and reload if needed.
 * Returns 1 if data was reloaded, 0 otherwise.
 */
int stasis_data_poll(int handle) {
    if (!g_initialized) return 0;
    if (handle < 1 || handle > MAX_DATA_BINDINGS) return 0;

    DataBinding* binding = &g_bindings[handle - 1];
    if (!binding->active) return 0;

    int64_t current_mtime = get_file_mtime(binding->json_file_path);
    if (current_mtime < 0) {
        /* File doesn't exist or can't be read */
        return 0;
    }

    if (current_mtime != binding->last_mtime) {
        binding->last_mtime = current_mtime;
        if (apply_data_file(binding)) {
            double apply_ms = (double)binding->last_apply_ns / 1000000.0;
            fprintf(stderr, "DATABIND: reloaded %s apply_ms=%.1f fields=%d\n",
                    binding->json_file_path,
                    apply_ms,
                    binding->field_count);
            return 1;
        } else {
            fprintf(stderr, "DATABIND: reload failed %s: %s\n",
                    binding->json_file_path,
                    binding->error_msg[0] ? binding->error_msg : "unknown error");
        }
    }

    return 0;
}

/*
 * Poll all active bindings for changes.
 * Returns the number of bindings that were reloaded.
 */
int stasis_data_poll_all(void) {
    if (!g_initialized) return 0;

    int reloaded = 0;
    for (int i = 0; i < MAX_DATA_BINDINGS; i++) {
        if (g_bindings[i].active) {
            if (stasis_data_poll(i + 1)) {
                reloaded++;
            }
        }
    }
    return reloaded;
}

/*
 * Check if a binding has an error.
 */
int stasis_data_has_error(int handle) {
    if (!g_initialized) return 0;
    if (handle < 1 || handle > MAX_DATA_BINDINGS) return 0;
    return g_bindings[handle - 1].has_error;
}

/*
 * Get error message for a binding.
 */
const char* stasis_data_get_error(int handle) {
    if (!g_initialized) return "";
    if (handle < 1 || handle > MAX_DATA_BINDINGS) return "";
    return g_bindings[handle - 1].error_msg;
}

/*
 * Unbind a data binding.
 */
void stasis_data_unbind(int handle) {
    if (!g_initialized) return;
    if (handle < 1 || handle > MAX_DATA_BINDINGS) return;

    DataBinding* binding = &g_bindings[handle - 1];
    if (binding->active) {
        binding->active = 0;
        g_binding_count--;
    }
}

/*
 * Cleanup all bindings.
 */
void stasis_data_cleanup(void) {
    if (!g_initialized) return;
    memset(g_bindings, 0, sizeof(g_bindings));
    g_binding_count = 0;
}
