#include "stasis_mobile_aot_runtime.h"

#include <math.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define STASIS_MOBILE_MAX_SCALARS 2048
#define STASIS_MOBILE_MAX_ARRAYS 512
#define STASIS_MOBILE_MAX_FUNCTIONS 1024
#define STASIS_MOBILE_MAX_STRINGS 512
#define STASIS_MOBILE_MAX_ARRAY_LENGTH 1048576

int stasis_audio_init(int sample_rate, int channels, int target_latency_frames);
void stasis_audio_shutdown(void);
int stasis_audio_is_available(void);
int stasis_audio_get_sample_rate(void);
int stasis_audio_get_channels(void);
int stasis_audio_get_queued_frames(void);
int stasis_audio_get_underruns(void);
int stasis_audio_push_f32_interleaved(const float *samples, int frames);
int stasis_audio_load_wav(const char *path);
void stasis_audio_release(int asset_handle);
int stasis_audio_play(int asset_handle, int loop, float volume, float pan);
void stasis_audio_stop(int voice_handle);
int stasis_audio_voice_is_playing(int voice_handle);
void stasis_audio_voice_set_paused(int voice_handle, int paused);
void stasis_audio_voice_set_volume_pan(int voice_handle, float volume, float pan);
int stasis_audio_load_music(const char *path);
int stasis_audio_load_effect(const char *path);
int stasis_audio_play_music(int asset_handle, int loop, float volume);
void stasis_audio_stop_music(int asset_handle);
void stasis_audio_pause_music(int asset_handle, int paused);
void stasis_audio_set_music_volume(int asset_handle, float volume);
int stasis_audio_play_effect(int asset_handle, float volume);
int stasis_gfx_load_sprite(const char *path, int max_w, int max_h);
void stasis_gfx_release_sprite(int handle);
int stasis_gfx_dump_bmp(const char *path);
int stasis_gfx_dump_png(const char *path);
int stasis_gfx_cache_text(int font, const char *text);
int stasis_gfx_poll_reload(int handle);
float stasis_gfx_measure_text_cached(int handle);
float stasis_gfx_measure_text_cached_height(int handle);
int stasis_load_font(const char *path, int size);
float stasis_measure_text(int font, const char *text);
void stasis_sleep_ms(int ms);
int stasis_storage_load_i32(const char *scope, const char *key, int fallback);
int stasis_storage_save_i32(const char *scope, const char *key, int value);
int stasis_storage_load_ascii(const char *scope, const char *key, char *out, int capacity);
int stasis_storage_save_ascii(const char *scope, const char *key, const char *value, int length);
int stasis_clipboard_load_ascii(char *out, int capacity);
int stasis_clipboard_save_ascii(const char *value, int length);

typedef union StasisScalarValue {
    int32_t i32_value;
    float f32_value;
    double f64_value;
    void *ptr;
} StasisScalarValue;

typedef struct StasisScalar {
    int32_t hash;
    int kind;
    int external;
    StasisScalarValue value;
} StasisScalar;

typedef struct StasisArray {
    int32_t collection_hash;
    int32_t field_hash;
    int kind;
    int external;
    size_t length;
    void *data;
} StasisArray;

typedef struct StasisCodePtr {
    int32_t fn_id;
    int64_t code_ptr;
} StasisCodePtr;

typedef struct StasisStringLiteral {
    int32_t id;
    const char *value;
} StasisStringLiteral;

enum {
    STASIS_VALUE_I32 = 1,
    STASIS_VALUE_F32 = 2,
    STASIS_VALUE_F64 = 3,
    STASIS_VALUE_U8 = 4,
    STASIS_VALUE_U16 = 5
};

static StasisScalar scalars[STASIS_MOBILE_MAX_SCALARS];
static size_t scalar_count;
static StasisArray arrays[STASIS_MOBILE_MAX_ARRAYS];
static size_t array_count;
static StasisCodePtr code_ptrs[STASIS_MOBILE_MAX_FUNCTIONS];
static size_t code_ptr_count;
static StasisStringLiteral strings[STASIS_MOBILE_MAX_STRINGS];
static size_t string_count;

int stasis_mobile_json_escape(const char *input, char *output, size_t capacity) {
    static const char hex[] = "0123456789abcdef";
    size_t out = 0;
    if (input == NULL || output == NULL || capacity == 0) return 0;
    output[0] = '\0';
    while (*input != '\0') {
        unsigned char value = (unsigned char)*input++;
        const char *escape = NULL;
        size_t escape_length = 0;
        switch (value) {
            case '"': escape = "\\\""; escape_length = 2; break;
            case '\\': escape = "\\\\"; escape_length = 2; break;
            case '\b': escape = "\\b"; escape_length = 2; break;
            case '\f': escape = "\\f"; escape_length = 2; break;
            case '\n': escape = "\\n"; escape_length = 2; break;
            case '\r': escape = "\\r"; escape_length = 2; break;
            case '\t': escape = "\\t"; escape_length = 2; break;
            default: break;
        }
        if (escape != NULL) {
            if (out + escape_length >= capacity) {
                output[0] = '\0';
                return 0;
            }
            memcpy(output + out, escape, escape_length);
            out += escape_length;
        } else if (value < 0x20) {
            if (out + 6 >= capacity) {
                output[0] = '\0';
                return 0;
            }
            output[out++] = '\\';
            output[out++] = 'u';
            output[out++] = '0';
            output[out++] = '0';
            output[out++] = hex[value >> 4];
            output[out++] = hex[value & 0x0f];
        } else {
            if (out + 1 >= capacity) {
                output[0] = '\0';
                return 0;
            }
            output[out++] = (char)value;
        }
    }
    output[out] = '\0';
    return 1;
}

static size_t value_size(int kind) {
    if (kind == STASIS_VALUE_I32) return sizeof(int32_t);
    if (kind == STASIS_VALUE_F32) return sizeof(float);
    if (kind == STASIS_VALUE_F64) return sizeof(double);
    if (kind == STASIS_VALUE_U8) return sizeof(uint8_t);
    if (kind == STASIS_VALUE_U16) return sizeof(uint16_t);
    return 0;
}

static StasisScalar *find_scalar(int32_t hash, int kind, int create) {
    size_t index;
    for (index = 0; index < scalar_count; index += 1) {
        if (scalars[index].hash == hash && scalars[index].kind == kind) {
            return &scalars[index];
        }
    }
    if (!create || scalar_count >= STASIS_MOBILE_MAX_SCALARS) return NULL;
    scalars[scalar_count].hash = hash;
    scalars[scalar_count].kind = kind;
    return &scalars[scalar_count++];
}

static StasisArray *find_array(
    int32_t collection_hash, int32_t field_hash, int kind, int create
) {
    size_t index;
    for (index = 0; index < array_count; index += 1) {
        StasisArray *entry = &arrays[index];
        if (entry->collection_hash == collection_hash &&
            entry->field_hash == field_hash && entry->kind == kind) {
            return entry;
        }
    }
    if (!create || array_count >= STASIS_MOBILE_MAX_ARRAYS) return NULL;
    arrays[array_count].collection_hash = collection_hash;
    arrays[array_count].field_hash = field_hash;
    arrays[array_count].kind = kind;
    return &arrays[array_count++];
}

static void *ensure_array(StasisArray *entry, size_t length) {
    size_t bytes;
    void *next;
    if (entry == NULL || length == 0 || length > STASIS_MOBILE_MAX_ARRAY_LENGTH) return NULL;
    if (entry->external) return length <= entry->length ? entry->data : NULL;
    if (length <= entry->length) return entry->data;
    bytes = length * value_size(entry->kind);
    next = realloc(entry->data, bytes);
    if (next == NULL) return NULL;
    memset((unsigned char *)next + entry->length * value_size(entry->kind),
        0, (length - entry->length) * value_size(entry->kind));
    entry->data = next;
    entry->length = length;
    return next;
}

static void register_scalar_ptr(int32_t hash, int kind, void *ptr) {
    StasisScalar *entry = find_scalar(hash, kind, 1);
    if (entry == NULL) return;
    entry->external = 1;
    entry->value.ptr = ptr;
}

static void register_array(
    int32_t collection_hash, int32_t field_hash, int kind, void *ptr, int32_t len
) {
    StasisArray *entry;
    if (ptr == NULL || len <= 0) return;
    entry = find_array(collection_hash, field_hash, kind, 1);
    if (entry == NULL) return;
    if (!entry->external) free(entry->data);
    entry->external = 1;
    entry->data = ptr;
    entry->length = (size_t)len;
}

void stasis_mobile_aot_reset(void) {
    size_t index;
    for (index = 0; index < array_count; index += 1) {
        if (!arrays[index].external) free(arrays[index].data);
    }
    memset(scalars, 0, sizeof(scalars));
    memset(arrays, 0, sizeof(arrays));
    memset(code_ptrs, 0, sizeof(code_ptrs));
    memset(strings, 0, sizeof(strings));
    scalar_count = 0;
    array_count = 0;
    code_ptr_count = 0;
    string_count = 0;
}

void stasis_jit_register_global_i32_ptr(int32_t hash, int32_t *ptr) {
    register_scalar_ptr(hash, STASIS_VALUE_I32, ptr);
}
void stasis_jit_register_global_f32_ptr(int32_t hash, float *ptr) {
    register_scalar_ptr(hash, STASIS_VALUE_F32, ptr);
}
void stasis_jit_register_global_f64_ptr(int32_t hash, double *ptr) {
    register_scalar_ptr(hash, STASIS_VALUE_F64, ptr);
}
void stasis_jit_register_global_i32_array(int32_t c, int32_t f, int32_t *p, int32_t n) {
    register_array(c, f, STASIS_VALUE_I32, p, n);
}
void stasis_jit_register_global_f32_array(int32_t c, int32_t f, float *p, int32_t n) {
    register_array(c, f, STASIS_VALUE_F32, p, n);
}
void stasis_jit_register_global_f64_array(int32_t c, int32_t f, double *p, int32_t n) {
    register_array(c, f, STASIS_VALUE_F64, p, n);
}
void stasis_jit_register_global_u8_array(int32_t c, int32_t f, uint8_t *p, int32_t n) {
    register_array(c, f, STASIS_VALUE_U8, p, n);
}
void stasis_jit_register_global_u16_array(int32_t c, int32_t f, uint16_t *p, int32_t n) {
    register_array(c, f, STASIS_VALUE_U16, p, n);
}

void stasis_jit_register_code_ptr(int32_t fn_id, int64_t code_ptr) {
    size_t index;
    for (index = 0; index < code_ptr_count; index += 1) {
        if (code_ptrs[index].fn_id == fn_id) {
            code_ptrs[index].code_ptr = code_ptr;
            return;
        }
    }
    if (code_ptr_count >= STASIS_MOBILE_MAX_FUNCTIONS) return;
    code_ptrs[code_ptr_count++] = (StasisCodePtr){fn_id, code_ptr};
}

int64_t stasis_jit_lookup_code_ptr(int32_t fn_id) {
    size_t index;
    for (index = 0; index < code_ptr_count; index += 1) {
        if (code_ptrs[index].fn_id == fn_id) return code_ptrs[index].code_ptr;
    }
    return 0;
}

#define DECLARE_I32_CALL_TYPE(name, ...) typedef int32_t (*name)(__VA_ARGS__)
#define DECLARE_F32_CALL_TYPE(name, ...) typedef float (*name)(__VA_ARGS__)
DECLARE_I32_CALL_TYPE(I32Call0, void);
DECLARE_I32_CALL_TYPE(I32Call1, int32_t);
DECLARE_I32_CALL_TYPE(I32Call2, int32_t, int32_t);
DECLARE_I32_CALL_TYPE(I32Call3, int32_t, int32_t, int32_t);
DECLARE_I32_CALL_TYPE(I32Call4, int32_t, int32_t, int32_t, int32_t);
DECLARE_I32_CALL_TYPE(I32Call5, int32_t, int32_t, int32_t, int32_t, int32_t);
DECLARE_I32_CALL_TYPE(I32Call6, int32_t, int32_t, int32_t, int32_t, int32_t, int32_t);
DECLARE_I32_CALL_TYPE(I32Call7, int32_t, int32_t, int32_t, int32_t, int32_t, int32_t, int32_t);
DECLARE_I32_CALL_TYPE(I32Call8, int32_t, int32_t, int32_t, int32_t, int32_t, int32_t, int32_t, int32_t);
DECLARE_I32_CALL_TYPE(I32F32Call1, float);
DECLARE_I32_CALL_TYPE(I32F32Call2, float, float);
DECLARE_I32_CALL_TYPE(I32F32Call3, float, float, float);
DECLARE_I32_CALL_TYPE(I32F32Call4, float, float, float, float);
DECLARE_I32_CALL_TYPE(I32F32Call5, float, float, float, float, float);
DECLARE_I32_CALL_TYPE(I32F32Call6, float, float, float, float, float, float);
DECLARE_I32_CALL_TYPE(I32F32Call7, float, float, float, float, float, float, float);
DECLARE_I32_CALL_TYPE(I32F32Call8, float, float, float, float, float, float, float, float);
DECLARE_F32_CALL_TYPE(F32Call0, void);
DECLARE_F32_CALL_TYPE(F32Call1, float);
DECLARE_F32_CALL_TYPE(F32Call2, float, float);
DECLARE_F32_CALL_TYPE(F32Call3, float, float, float);
DECLARE_F32_CALL_TYPE(F32Call4, float, float, float, float);
DECLARE_F32_CALL_TYPE(F32Call5, float, float, float, float, float);
DECLARE_F32_CALL_TYPE(F32Call6, float, float, float, float, float, float);
DECLARE_F32_CALL_TYPE(F32Call7, float, float, float, float, float, float, float);
DECLARE_F32_CALL_TYPE(F32Call8, float, float, float, float, float, float, float, float);
DECLARE_F32_CALL_TYPE(F32I32Call1, int32_t);

#define STASIS_CALL_PTR(fn, type) ((type)(uintptr_t)stasis_jit_lookup_code_ptr(fn))
#define CALL_OR_ZERO(fn, type, call) do { type target = STASIS_CALL_PTR(fn, type); return target ? (call) : 0; } while (0)

int32_t stasis_jit_call_i32_0(int32_t fn) { CALL_OR_ZERO(fn, I32Call0, target()); }
int32_t stasis_jit_call_i32_1(int32_t fn, int32_t a0) { CALL_OR_ZERO(fn, I32Call1, target(a0)); }
int32_t stasis_jit_call_i32_2(int32_t fn, int32_t a0, int32_t a1) { CALL_OR_ZERO(fn, I32Call2, target(a0,a1)); }
int32_t stasis_jit_call_i32_3(int32_t fn, int32_t a0, int32_t a1, int32_t a2) { CALL_OR_ZERO(fn, I32Call3, target(a0,a1,a2)); }
int32_t stasis_jit_call_i32_4(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3) { CALL_OR_ZERO(fn, I32Call4, target(a0,a1,a2,a3)); }
int32_t stasis_jit_call_i32_5(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3, int32_t a4) { CALL_OR_ZERO(fn, I32Call5, target(a0,a1,a2,a3,a4)); }
int32_t stasis_jit_call_i32_6(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3, int32_t a4, int32_t a5) { CALL_OR_ZERO(fn, I32Call6, target(a0,a1,a2,a3,a4,a5)); }
int32_t stasis_jit_call_i32_7(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3, int32_t a4, int32_t a5, int32_t a6) { CALL_OR_ZERO(fn, I32Call7, target(a0,a1,a2,a3,a4,a5,a6)); }
int32_t stasis_jit_call_i32_8(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3, int32_t a4, int32_t a5, int32_t a6, int32_t a7) { CALL_OR_ZERO(fn, I32Call8, target(a0,a1,a2,a3,a4,a5,a6,a7)); }

int32_t stasis_jit_call_i32_f32_1(int32_t fn, float a0) { CALL_OR_ZERO(fn, I32F32Call1, target(a0)); }
int32_t stasis_jit_call_i32_f32_2(int32_t fn, float a0, float a1) { CALL_OR_ZERO(fn, I32F32Call2, target(a0,a1)); }
int32_t stasis_jit_call_i32_f32_3(int32_t fn, float a0, float a1, float a2) { CALL_OR_ZERO(fn, I32F32Call3, target(a0,a1,a2)); }
int32_t stasis_jit_call_i32_f32_4(int32_t fn, float a0, float a1, float a2, float a3) { CALL_OR_ZERO(fn, I32F32Call4, target(a0,a1,a2,a3)); }
int32_t stasis_jit_call_i32_f32_5(int32_t fn, float a0, float a1, float a2, float a3, float a4) { CALL_OR_ZERO(fn, I32F32Call5, target(a0,a1,a2,a3,a4)); }
int32_t stasis_jit_call_i32_f32_6(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5) { CALL_OR_ZERO(fn, I32F32Call6, target(a0,a1,a2,a3,a4,a5)); }
int32_t stasis_jit_call_i32_f32_7(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5, float a6) { CALL_OR_ZERO(fn, I32F32Call7, target(a0,a1,a2,a3,a4,a5,a6)); }
int32_t stasis_jit_call_i32_f32_8(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5, float a6, float a7) { CALL_OR_ZERO(fn, I32F32Call8, target(a0,a1,a2,a3,a4,a5,a6,a7)); }

float stasis_jit_call_f32_0(int32_t fn) { CALL_OR_ZERO(fn, F32Call0, target()); }
float stasis_jit_call_f32_1(int32_t fn, float a0) { CALL_OR_ZERO(fn, F32Call1, target(a0)); }
float stasis_jit_call_f32_2(int32_t fn, float a0, float a1) { CALL_OR_ZERO(fn, F32Call2, target(a0,a1)); }
float stasis_jit_call_f32_3(int32_t fn, float a0, float a1, float a2) { CALL_OR_ZERO(fn, F32Call3, target(a0,a1,a2)); }
float stasis_jit_call_f32_4(int32_t fn, float a0, float a1, float a2, float a3) { CALL_OR_ZERO(fn, F32Call4, target(a0,a1,a2,a3)); }
float stasis_jit_call_f32_5(int32_t fn, float a0, float a1, float a2, float a3, float a4) { CALL_OR_ZERO(fn, F32Call5, target(a0,a1,a2,a3,a4)); }
float stasis_jit_call_f32_6(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5) { CALL_OR_ZERO(fn, F32Call6, target(a0,a1,a2,a3,a4,a5)); }
float stasis_jit_call_f32_7(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5, float a6) { CALL_OR_ZERO(fn, F32Call7, target(a0,a1,a2,a3,a4,a5,a6)); }
float stasis_jit_call_f32_8(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5, float a6, float a7) { CALL_OR_ZERO(fn, F32Call8, target(a0,a1,a2,a3,a4,a5,a6,a7)); }
float stasis_jit_call_f32_i32_1(int32_t fn, int32_t a0) { CALL_OR_ZERO(fn, F32I32Call1, target(a0)); }

void stasis_jit_clear_string_literal_table(void) {
    memset(strings, 0, sizeof(strings));
    string_count = 0;
}

void stasis_jit_upsert_string_literal(int32_t id, const char *value) {
    size_t index;
    for (index = 0; index < string_count; index += 1) {
        if (strings[index].id == id) {
            strings[index].value = value;
            return;
        }
    }
    if (string_count >= STASIS_MOBILE_MAX_STRINGS) return;
    strings[string_count++] = (StasisStringLiteral){id, value};
}

static const char *find_string(int32_t id) {
    size_t index;
    for (index = 0; index < string_count; index += 1) {
        if (strings[index].id == id) return strings[index].value;
    }
    return NULL;
}

static int32_t collection_meta_hash(int32_t hash, int32_t kind);

static char *resolve_text(int32_t id) {
    StasisArray *entry = find_array(id, 0, STASIS_VALUE_U8, 0);
    int32_t length;
    char *text;
    size_t index;
    const char *literal;

    if (entry == NULL) entry = find_array(id, 0, STASIS_VALUE_I32, 0);
    if (entry != NULL) {
        length = stasis_jit_global_i32_load(collection_meta_hash(id, 1));
        if (length < 0 || (size_t)length > entry->length) return NULL;
        text = (char *)malloc((size_t)length + 1);
        if (text == NULL) return NULL;
        for (index = 0; index < (size_t)length; index += 1) {
            int32_t value = stasis_jit_global_i32_array_load(id, 0, (int32_t)index);
            if (value <= 0 || value > 255) {
                free(text);
                return NULL;
            }
            text[index] = (char)value;
        }
        text[length] = '\0';
        return text;
    }

    literal = find_string(id);
    if (literal == NULL) return NULL;
    text = (char *)malloc(strlen(literal) + 1);
    if (text != NULL) strcpy(text, literal);
    return text;
}

static int stasis_valid_utf8(const unsigned char *text) {
    const unsigned char *cursor = text;
    if (cursor == NULL) return 0;
    while (*cursor != 0) {
        uint32_t codepoint;
        uint32_t minimum;
        int remaining;
        if (*cursor <= 0x7f) {
            cursor += 1;
            continue;
        }
        if (*cursor >= 0xc2 && *cursor <= 0xdf) {
            codepoint = (uint32_t)(*cursor & 0x1f);
            minimum = 0x80;
            remaining = 1;
        } else if (*cursor >= 0xe0 && *cursor <= 0xef) {
            codepoint = (uint32_t)(*cursor & 0x0f);
            minimum = 0x800;
            remaining = 2;
        } else if (*cursor >= 0xf0 && *cursor <= 0xf4) {
            codepoint = (uint32_t)(*cursor & 0x07);
            minimum = 0x10000;
            remaining = 3;
        } else {
            return 0;
        }
        cursor += 1;
        while (remaining > 0) {
            if (*cursor < 0x80 || *cursor > 0xbf) return 0;
            codepoint = (codepoint << 6) | (uint32_t)(*cursor & 0x3f);
            cursor += 1;
            remaining -= 1;
        }
        if (codepoint < minimum || (codepoint >= 0xd800 && codepoint <= 0xdfff) ||
            codepoint > 0x10ffff) return 0;
    }
    return 1;
}

int stasis_jit_audio_init(int32_t rate, int32_t channels, int32_t latency) {
    return stasis_audio_init(rate, channels, latency);
}
void stasis_jit_audio_shutdown(void) { stasis_audio_shutdown(); }
int stasis_jit_audio_is_available(void) { return stasis_audio_is_available(); }
int stasis_jit_audio_get_sample_rate(void) { return stasis_audio_get_sample_rate(); }
int stasis_jit_audio_get_channels(void) { return stasis_audio_get_channels(); }
int stasis_jit_audio_get_queued_frames(void) { return stasis_audio_get_queued_frames(); }
int stasis_jit_audio_get_underruns(void) { return stasis_audio_get_underruns(); }
int stasis_jit_audio_push_f32_interleaved(int32_t samples, int32_t frames) {
    int32_t channels = stasis_audio_get_channels();
    float *values;
    if (frames <= 0 || channels <= 0) return 0;
    values = stasis_jit_global_f32_array_ptr(samples, 0, frames * channels);
    return values == NULL ? 0 : stasis_audio_push_f32_interleaved(values, frames);
}
int stasis_jit_audio_load_wav(int32_t path) {
    char *value = resolve_text(path);
    int result = value == NULL ? 0 : stasis_audio_load_wav(value);
    free(value);
    return result;
}
void stasis_jit_audio_release(int32_t asset_handle) { stasis_audio_release(asset_handle); }
int stasis_jit_audio_play(int32_t asset_handle, int32_t loop, float volume, float pan) {
    return stasis_audio_play(asset_handle, loop, volume, pan);
}
void stasis_jit_audio_stop(int32_t voice_handle) { stasis_audio_stop(voice_handle); }
int stasis_jit_audio_voice_is_playing(int32_t voice_handle) {
    return stasis_audio_voice_is_playing(voice_handle);
}
void stasis_jit_audio_voice_set_paused(int32_t voice_handle, int32_t paused) {
    stasis_audio_voice_set_paused(voice_handle, paused);
}
void stasis_jit_audio_voice_set_volume_pan(int32_t voice_handle, float volume, float pan) {
    stasis_audio_voice_set_volume_pan(voice_handle, volume, pan);
}
int stasis_jit_audio_load_music(int32_t path) {
    char *value = resolve_text(path);
    int result = value == NULL ? 0 : stasis_audio_load_music(value);
    free(value);
    return result;
}
int stasis_jit_audio_load_effect(int32_t path) {
    char *value = resolve_text(path);
    int result = value == NULL ? 0 : stasis_audio_load_effect(value);
    free(value);
    return result;
}
int stasis_jit_audio_play_music(int32_t asset_handle, int32_t loop, float volume) {
    return stasis_audio_play_music(asset_handle, loop, volume);
}
void stasis_jit_audio_stop_music(int32_t asset_handle) { stasis_audio_stop_music(asset_handle); }
void stasis_jit_audio_pause_music(int32_t asset_handle, int32_t paused) {
    stasis_audio_pause_music(asset_handle, paused);
}
void stasis_jit_audio_set_music_volume(int32_t asset_handle, float volume) {
    stasis_audio_set_music_volume(asset_handle, volume);
}
int stasis_jit_audio_play_effect(int32_t asset_handle, float volume) {
    return stasis_audio_play_effect(asset_handle, volume);
}

int stasis_jit_gfx_load_sprite(int32_t path, int32_t max_w, int32_t max_h) {
    char *value = resolve_text(path);
    int result = value == NULL ? 0 : stasis_gfx_load_sprite(value, max_w, max_h);
    free(value);
    return result;
}
void stasis_jit_gfx_release_sprite(int32_t handle) { stasis_gfx_release_sprite(handle); }
int stasis_jit_gfx_dump_bmp(int32_t path) {
    char *value = resolve_text(path);
    int result = value == NULL ? 0 : stasis_gfx_dump_bmp(value);
    free(value);
    return result;
}
int stasis_jit_gfx_dump_png(int32_t path) {
    char *value = resolve_text(path);
    int result = value == NULL ? 0 : stasis_gfx_dump_png(value);
    free(value);
    return result;
}
int stasis_jit_gfx_cache_text(int32_t font, int32_t text) {
    char *value = resolve_text(text);
    int result = value == NULL || !stasis_valid_utf8((const unsigned char *)value)
        ? 0
        : stasis_gfx_cache_text(font, value);
    free(value);
    return result;
}
int stasis_jit_gfx_poll_reload(int32_t handle) { return stasis_gfx_poll_reload(handle); }
float stasis_jit_gfx_measure_text_cached(int32_t handle) {
    return stasis_gfx_measure_text_cached(handle);
}

float stasis_jit_gfx_measure_text_cached_height(int32_t handle) {
    return stasis_gfx_measure_text_cached_height(handle);
}

static int32_t stasis_struct_field_hash(int32_t base, const char *suffix) {
    uint32_t hash = (uint32_t)base;
    const unsigned char *cursor = (const unsigned char *)suffix;
    hash ^= (uint32_t)'.';
    hash *= 16777619u;
    while (*cursor != 0) {
        hash ^= (uint32_t)*cursor++;
        hash *= 16777619u;
    }
    return (int32_t)hash;
}

static int32_t stasis_suffix_hash(const char *suffix) {
    uint32_t hash = 2166136261u;
    const unsigned char *cursor = (const unsigned char *)suffix;
    while (*cursor != 0) {
        hash ^= (uint32_t)*cursor++;
        hash *= 16777619u;
    }
    return (int32_t)hash;
}

static int32_t stasis_struct_i32_load(int32_t base, int32_t index, const char *suffix) {
    if (index < 0) return stasis_jit_global_i32_load(stasis_struct_field_hash(base, suffix));
    return stasis_jit_global_i32_array_load(base, stasis_suffix_hash(suffix), index);
}

static void stasis_struct_i32_store(int32_t base, int32_t index, int32_t len, const char *suffix, int32_t value) {
    if (index < 0) stasis_jit_global_i32_store(stasis_struct_field_hash(base, suffix), value);
    else if (index < len) stasis_jit_global_i32_array_store(base, stasis_suffix_hash(suffix), index, value);
}

static void stasis_struct_f32_store(int32_t base, int32_t index, int32_t len, const char *suffix, float value) {
    if (index < 0) stasis_jit_global_f32_store(stasis_struct_field_hash(base, suffix), value);
    else if (index < len) stasis_jit_global_f32_array_store(base, stasis_suffix_hash(suffix), index, value);
}

int stasis_jit_sprite_load_from(int32_t base, int32_t index, int32_t len, int32_t path, int32_t width, int32_t height) {
    int32_t loaded_handle;
    int32_t old_handle;
    if (width <= 0 || height <= 0 || (index >= 0 && index >= len)) return 0;
    loaded_handle = stasis_jit_gfx_load_sprite(path, width, height);
    if (loaded_handle == 0) return 0;
    old_handle = stasis_struct_i32_load(base, index, "handle");
    stasis_struct_i32_store(base, index, len, "handle", loaded_handle);
    stasis_struct_i32_store(base, index, len, "width", width);
    stasis_struct_i32_store(base, index, len, "height", height);
    if (old_handle != 0 && old_handle != loaded_handle) stasis_jit_gfx_release_sprite(old_handle);
    return 1;
}

int stasis_jit_text_run_load_from(int32_t base, int32_t index, int32_t len, int32_t font, int32_t text) {
    int32_t loaded_handle;
    if (font <= 0 || (index >= 0 && index >= len)) return 0;
    loaded_handle = stasis_jit_gfx_cache_text(font, text);
    if (loaded_handle <= 0) return 0;
    stasis_struct_i32_store(base, index, len, "font", font);
    stasis_struct_i32_store(base, index, len, "handle", loaded_handle);
    stasis_struct_f32_store(base, index, len, "width", stasis_jit_gfx_measure_text_cached(loaded_handle));
    stasis_struct_f32_store(base, index, len, "height", stasis_jit_gfx_measure_text_cached_height(loaded_handle));
    return 1;
}
int stasis_jit_load_font(int32_t path, int32_t size) {
    char *value = resolve_text(path);
    int result = value == NULL ? 0 : stasis_load_font(value, size);
    free(value);
    return result;
}
float stasis_jit_measure_text(int32_t font, int32_t text) {
    char *value = resolve_text(text);
    float result = value == NULL ? 0.0f : stasis_measure_text(font, value);
    free(value);
    return result;
}
void stasis_jit_sleep_ms(int32_t ms) { stasis_sleep_ms(ms); }
static int stasis_write_ascii_result(int32_t out, const char *value, int length, int capacity) {
    int index;
    if (length < 0 || length > capacity) return -1;
    for (index = 0; index < length; index += 1) {
        unsigned char ch = (unsigned char)value[index];
        if (ch < 32 || ch > 126) return -1;
        stasis_jit_global_i32_array_store(out, 0, index, (int32_t)ch);
    }
    return length;
}
int stasis_jit_storage_load_ascii(int32_t scope, int32_t key, int32_t out, int32_t capacity) {
    char *scope_value;
    char *key_value;
    char *buffer;
    int loaded;
    if (capacity <= 0) return -1;
    scope_value = resolve_text(scope);
    key_value = resolve_text(key);
    buffer = (char *)malloc((size_t)capacity);
    if (scope_value == NULL || key_value == NULL || buffer == NULL) {
        free(scope_value);
        free(key_value);
        free(buffer);
        return -1;
    }
    loaded = stasis_storage_load_ascii(scope_value, key_value, buffer, capacity);
    loaded = stasis_write_ascii_result(out, buffer, loaded, capacity);
    free(scope_value);
    free(key_value);
    free(buffer);
    return loaded;
}
int stasis_jit_storage_save_ascii(int32_t scope, int32_t key, int32_t value, int32_t length) {
    char *scope_value = resolve_text(scope);
    char *key_value = resolve_text(key);
    char *text_value = resolve_text(value);
    int result = scope_value == NULL || key_value == NULL || text_value == NULL || length < 0
        ? 0
        : stasis_storage_save_ascii(scope_value, key_value, text_value, length);
    free(scope_value);
    free(key_value);
    free(text_value);
    return result;
}
int stasis_jit_clipboard_load_ascii(int32_t out, int32_t capacity) {
    char *buffer;
    int loaded;
    if (capacity <= 0) return -1;
    buffer = (char *)malloc((size_t)capacity);
    if (buffer == NULL) return -1;
    loaded = stasis_clipboard_load_ascii(buffer, capacity);
    loaded = stasis_write_ascii_result(out, buffer, loaded, capacity);
    free(buffer);
    return loaded;
}
int stasis_jit_clipboard_save_ascii(int32_t value, int32_t length) {
    char *text_value = resolve_text(value);
    int result = text_value == NULL || length < 0
        ? 0
        : stasis_clipboard_save_ascii(text_value, length);
    free(text_value);
    return result;
}
int stasis_jit_storage_load_i32(int32_t scope, int32_t key, int32_t fallback) {
    char *scope_value = resolve_text(scope);
    char *key_value = resolve_text(key);
    int result = scope_value == NULL || key_value == NULL
        ? fallback
        : stasis_storage_load_i32(scope_value, key_value, fallback);
    free(scope_value);
    free(key_value);
    return result;
}
int stasis_jit_storage_save_i32(int32_t scope, int32_t key, int32_t value) {
    char *scope_value = resolve_text(scope);
    char *key_value = resolve_text(key);
    int result = scope_value == NULL || key_value == NULL
        ? 0
        : stasis_storage_save_i32(scope_value, key_value, value);
    free(scope_value);
    free(key_value);
    return result;
}

#define DEFINE_SCALAR_ACCESSORS(name, type, kind, member) \
type stasis_jit_global_##name##_load(int32_t hash) { \
    StasisScalar *entry = find_scalar(hash, kind, 0); \
    if (entry == NULL) return (type)0; \
    return entry->external ? *(type *)entry->value.ptr : entry->value.member; \
} \
void stasis_jit_global_##name##_store(int32_t hash, type value) { \
    StasisScalar *entry = find_scalar(hash, kind, 1); \
    if (entry == NULL) return; \
    if (entry->external) *(type *)entry->value.ptr = value; \
    else entry->value.member = value; \
}

DEFINE_SCALAR_ACCESSORS(i32, int32_t, STASIS_VALUE_I32, i32_value)
DEFINE_SCALAR_ACCESSORS(f32, float, STASIS_VALUE_F32, f32_value)
DEFINE_SCALAR_ACCESSORS(f64, double, STASIS_VALUE_F64, f64_value)

#define DEFINE_ARRAY_ACCESSORS(name, type, kind) \
type stasis_jit_global_##name##_array_load(int32_t c, int32_t f, int32_t i) { \
    StasisArray *entry; \
    if (i < 0) return (type)0; \
    entry = find_array(c, f, kind, 0); \
    return entry != NULL && (size_t)i < entry->length ? ((type *)entry->data)[i] : (type)0; \
} \
void stasis_jit_global_##name##_array_store(int32_t c, int32_t f, int32_t i, type value) { \
    StasisArray *entry; \
    if (i < 0) return; \
    entry = find_array(c, f, kind, 1); \
    if (ensure_array(entry, (size_t)i + 1) != NULL) ((type *)entry->data)[i] = value; \
} \
type *stasis_jit_global_##name##_array_ptr(int32_t c, int32_t f, int32_t len) { \
    StasisArray *entry; \
    if (len <= 0) return NULL; \
    entry = find_array(c, f, kind, 1); \
    return (type *)ensure_array(entry, (size_t)len); \
}

DEFINE_ARRAY_ACCESSORS(f32, float, STASIS_VALUE_F32)
DEFINE_ARRAY_ACCESSORS(f64, double, STASIS_VALUE_F64)

int32_t stasis_jit_global_i32_array_load(int32_t c, int32_t f, int32_t i) {
    StasisArray *entry;
    if (i < 0) return 0;
    entry = find_array(c, f, STASIS_VALUE_I32, 0);
    if (entry != NULL && (size_t)i < entry->length) return ((int32_t *)entry->data)[i];
    entry = find_array(c, f, STASIS_VALUE_U8, 0);
    if (entry != NULL && (size_t)i < entry->length) return ((uint8_t *)entry->data)[i];
    entry = find_array(c, f, STASIS_VALUE_U16, 0);
    if (entry != NULL && (size_t)i < entry->length) return ((uint16_t *)entry->data)[i];
    return 0;
}

void stasis_jit_global_i32_array_store(
    int32_t c, int32_t f, int32_t i, int32_t value
) {
    StasisArray *entry;
    if (i < 0) return;
    entry = find_array(c, f, STASIS_VALUE_I32, 0);
    if (entry != NULL) {
        if (ensure_array(entry, (size_t)i + 1) != NULL) ((int32_t *)entry->data)[i] = value;
        return;
    }
    entry = find_array(c, f, STASIS_VALUE_U8, 0);
    if (entry != NULL) {
        if ((size_t)i < entry->length) ((uint8_t *)entry->data)[i] = (uint8_t)value;
        return;
    }
    entry = find_array(c, f, STASIS_VALUE_U16, 0);
    if (entry != NULL) {
        if ((size_t)i < entry->length) ((uint16_t *)entry->data)[i] = (uint16_t)value;
        return;
    }
    entry = find_array(c, f, STASIS_VALUE_I32, 1);
    if (ensure_array(entry, (size_t)i + 1) != NULL) ((int32_t *)entry->data)[i] = value;
}

int32_t *stasis_jit_global_i32_array_ptr(int32_t c, int32_t f, int32_t len) {
    StasisArray *entry;
    if (len <= 0) return NULL;
    entry = find_array(c, f, STASIS_VALUE_I32, 1);
    return (int32_t *)ensure_array(entry, (size_t)len);
}

uint8_t *stasis_jit_global_u8_array_ptr(int32_t c, int32_t f, int32_t len) {
    StasisArray *entry;
    if (len <= 0) return NULL;
    entry = find_array(c, f, STASIS_VALUE_U8, 1);
    return (uint8_t *)ensure_array(entry, (size_t)len);
}

static int32_t collection_meta_hash(int32_t hash, int32_t kind) {
    const char *suffix = kind == 1 ? ".length" : kind == 2 ? ".max_length" :
        kind == 3 ? ".char_length" : NULL;
    uint32_t value = (uint32_t)hash;
    if (suffix == NULL) return 0;
    while (*suffix != '\0') {
        value ^= (uint8_t)*suffix++;
        value *= 16777619U;
    }
    return (int32_t)value;
}

int32_t stasis_jit_collection_i32_load(int32_t hash, int32_t kind) {
    int32_t derived = collection_meta_hash(hash, kind);
    return derived == 0 ? 0 : stasis_jit_global_i32_load(derived);
}
void stasis_jit_collection_i32_store(int32_t hash, int32_t kind, int32_t value) {
    int32_t derived = collection_meta_hash(hash, kind);
    if (derived != 0) stasis_jit_global_i32_store(derived, value);
}

void stasis_jit_print_i32(int32_t value) { printf("%d", value); }
void stasis_jit_print_string(int32_t id) {
    char *value = resolve_text(id);
    if (value != NULL) fputs(value, stdout);
    free(value);
}
float stasis_jit_sin_fast(float value) { return sinf(value); }
float stasis_jit_cos_fast(float value) { return cosf(value); }

static void copy_i32_values(int32_t dst, int32_t di, int32_t src, int32_t si, int32_t count) {
    int32_t index;
    if (count <= 0 || di < 0 || si < 0) return;
    if (dst == src && di > si && di < si + count) {
        for (index = count; index > 0; index -= 1) {
            int32_t value = stasis_jit_global_i32_array_load(src, 0, si + index - 1);
            stasis_jit_global_i32_array_store(dst, 0, di + index - 1, value);
        }
        return;
    }
    for (index = 0; index < count; index += 1) {
        int32_t value = stasis_jit_global_i32_array_load(src, 0, si + index);
        stasis_jit_global_i32_array_store(dst, 0, di + index, value);
    }
}

static void copy_f32_values(int32_t dst, int32_t di, int32_t src, int32_t si, int32_t count) {
    int32_t index;
    if (count <= 0 || di < 0 || si < 0) return;
    if (dst == src && di > si && di < si + count) {
        for (index = count; index > 0; index -= 1) {
            float value = stasis_jit_global_f32_array_load(src, 0, si + index - 1);
            stasis_jit_global_f32_array_store(dst, 0, di + index - 1, value);
        }
        return;
    }
    for (index = 0; index < count; index += 1) {
        float value = stasis_jit_global_f32_array_load(src, 0, si + index);
        stasis_jit_global_f32_array_store(dst, 0, di + index, value);
    }
}

void stasis_jit_sys_memcpy_u8(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n) { copy_i32_values(d, di, s, si, n); }
void stasis_jit_sys_memcpy_i32(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n) { copy_i32_values(d, di, s, si, n); }
void stasis_jit_sys_memcpy_f32(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n) { copy_f32_values(d, di, s, si, n); }

/* Mobile AOT has no live swap coordinator; retain the shared import contract as a no-op. */
void stasis_jit_reject_code_swap(void) {}
void stasis_jit_sys_memmove_u8(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n) { copy_i32_values(d, di, s, si, n); }
void stasis_jit_sys_memmove_i32(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n) { copy_i32_values(d, di, s, si, n); }
void stasis_jit_sys_memmove_f32(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n) { copy_f32_values(d, di, s, si, n); }
