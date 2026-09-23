#include "stasis_replay_consumer.h"

#include "cJSON.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>
#include <string.h>

/*
 * This file is deliberately self-contained at the host boundary.  The game
 * never receives a JSON object or a replay-owned pointer: cJSON is retained
 * only by this bounded consumer, and every HostFrame write is projected from
 * validated compact slots.
 */

#define REPLAY_SHA256_BLOCK 64U

typedef struct ReplaySha256 {
    uint32_t state[8];
    uint8_t buffer[REPLAY_SHA256_BLOCK];
    size_t buffer_length;
    uint64_t bytes_hashed;
} ReplaySha256;

static void replay_update_u64_le(ReplaySha256 *hash, uint64_t value);

static const uint32_t replay_sha256_k[64] = {
    0x428a2f98U, 0x71374491U, 0xb5c0fbcfU, 0xe9b5dba5U,
    0x3956c25bU, 0x59f111f1U, 0x923f82a4U, 0xab1c5ed5U,
    0xd807aa98U, 0x12835b01U, 0x243185beU, 0x550c7dc3U,
    0x72be5d74U, 0x80deb1feU, 0x9bdc06a7U, 0xc19bf174U,
    0xe49b69c1U, 0xefbe4786U, 0x0fc19dc6U, 0x240ca1ccU,
    0x2de92c6fU, 0x4a7484aaU, 0x5cb0a9dcU, 0x76f988daU,
    0x983e5152U, 0xa831c66dU, 0xb00327c8U, 0xbf597fc7U,
    0xc6e00bf3U, 0xd5a79147U, 0x06ca6351U, 0x14292967U,
    0x27b70a85U, 0x2e1b2138U, 0x4d2c6dfcU, 0x53380d13U,
    0x650a7354U, 0x766a0abbU, 0x81c2c92eU, 0x92722c85U,
    0xa2bfe8a1U, 0xa81a664bU, 0xc24b8b70U, 0xc76c51a3U,
    0xd192e819U, 0xd6990624U, 0xf40e3585U, 0x106aa070U,
    0x19a4c116U, 0x1e376c08U, 0x2748774cU, 0x34b0bcb5U,
    0x391c0cb3U, 0x4ed8aa4aU, 0x5b9cca4fU, 0x682e6ff3U,
    0x748f82eeU, 0x78a5636fU, 0x84c87814U, 0x8cc70208U,
    0x90befffaU, 0xa4506cebU, 0xbef9a3f7U, 0xc67178f2U,
};

static uint32_t replay_rotr(uint32_t value, unsigned bits) {
    return (value >> bits) | (value << (32U - bits));
}

static void replay_sha256_compress(ReplaySha256 *hash, const uint8_t *block) {
    uint32_t schedule[64];
    for (size_t index = 0; index < 16U; ++index) {
        size_t base = index * 4U;
        schedule[index] = ((uint32_t)block[base] << 24U)
            | ((uint32_t)block[base + 1U] << 16U)
            | ((uint32_t)block[base + 2U] << 8U)
            | (uint32_t)block[base + 3U];
    }
    for (size_t index = 16U; index < 64U; ++index) {
        uint32_t x = schedule[index - 15U];
        uint32_t y = schedule[index - 2U];
        uint32_t sigma0 = replay_rotr(x, 7U) ^ replay_rotr(x, 18U) ^ (x >> 3U);
        uint32_t sigma1 = replay_rotr(y, 17U) ^ replay_rotr(y, 19U) ^ (y >> 10U);
        schedule[index] = schedule[index - 16U] + sigma0 + schedule[index - 7U] + sigma1;
    }
    uint32_t a = hash->state[0];
    uint32_t b = hash->state[1];
    uint32_t c = hash->state[2];
    uint32_t d = hash->state[3];
    uint32_t e = hash->state[4];
    uint32_t f = hash->state[5];
    uint32_t g = hash->state[6];
    uint32_t h = hash->state[7];
    for (size_t index = 0; index < 64U; ++index) {
        uint32_t sigma1 = replay_rotr(e, 6U) ^ replay_rotr(e, 11U) ^ replay_rotr(e, 25U);
        uint32_t choose = (e & f) ^ (~e & g);
        uint32_t temp1 = h + sigma1 + choose + replay_sha256_k[index] + schedule[index];
        uint32_t sigma0 = replay_rotr(a, 2U) ^ replay_rotr(a, 13U) ^ replay_rotr(a, 22U);
        uint32_t majority = (a & b) ^ (a & c) ^ (b & c);
        uint32_t temp2 = sigma0 + majority;
        h = g;
        g = f;
        f = e;
        e = d + temp1;
        d = c;
        c = b;
        b = a;
        a = temp1 + temp2;
    }
    hash->state[0] += a;
    hash->state[1] += b;
    hash->state[2] += c;
    hash->state[3] += d;
    hash->state[4] += e;
    hash->state[5] += f;
    hash->state[6] += g;
    hash->state[7] += h;
}

static void replay_sha256_init(ReplaySha256 *hash) {
    *hash = (ReplaySha256){
        .state = {
            0x6a09e667U, 0xbb67ae85U, 0x3c6ef372U, 0xa54ff53aU,
            0x510e527fU, 0x9b05688cU, 0x1f83d9abU, 0x5be0cd19U,
        },
    };
}

static void replay_sha256_update(ReplaySha256 *hash, const void *data, size_t length) {
    const uint8_t *bytes = (const uint8_t *)data;
    hash->bytes_hashed += (uint64_t)length;
    size_t offset = 0U;
    if (hash->buffer_length != 0U) {
        size_t copied = REPLAY_SHA256_BLOCK - hash->buffer_length;
        if (copied > length) copied = length;
        memcpy(hash->buffer + hash->buffer_length, bytes, copied);
        hash->buffer_length += copied;
        offset += copied;
        if (hash->buffer_length == REPLAY_SHA256_BLOCK) {
            replay_sha256_compress(hash, hash->buffer);
            hash->buffer_length = 0U;
        }
    }
    while (offset + REPLAY_SHA256_BLOCK <= length) {
        replay_sha256_compress(hash, bytes + offset);
        offset += REPLAY_SHA256_BLOCK;
    }
    if (offset < length) {
        hash->buffer_length = length - offset;
        memcpy(hash->buffer, bytes + offset, hash->buffer_length);
    }
}

static void replay_sha256_finish(ReplaySha256 *hash, uint8_t output[32]) {
    uint64_t bit_length = hash->bytes_hashed * 8U;
    hash->buffer[hash->buffer_length++] = 0x80U;
    if (hash->buffer_length > 56U) {
        while (hash->buffer_length < 64U) hash->buffer[hash->buffer_length++] = 0U;
        replay_sha256_compress(hash, hash->buffer);
        hash->buffer_length = 0U;
    }
    while (hash->buffer_length < 56U) hash->buffer[hash->buffer_length++] = 0U;
    for (size_t index = 0; index < 8U; ++index) {
        hash->buffer[63U - index] = (uint8_t)(bit_length >> (index * 8U));
    }
    replay_sha256_compress(hash, hash->buffer);
    for (size_t index = 0; index < 8U; ++index) {
        output[index * 4U] = (uint8_t)(hash->state[index] >> 24U);
        output[index * 4U + 1U] = (uint8_t)(hash->state[index] >> 16U);
        output[index * 4U + 2U] = (uint8_t)(hash->state[index] >> 8U);
        output[index * 4U + 3U] = (uint8_t)hash->state[index];
    }
}

static size_t replay_text_length(const char *text) {
    size_t length = 0U;
    if (text == NULL) return 0U;
    while (length <= STASIS_REPLAY_MAX_TEXT_BYTES && text[length] != '\0') ++length;
    return length;
}

static int replay_text_valid(const char *text, int allow_empty) {
    size_t length = replay_text_length(text);
    return text != NULL && (allow_empty || length != 0U) && length <= STASIS_REPLAY_MAX_TEXT_BYTES;
}

static int replay_hash_text_valid(const char *text) {
    if (text == NULL || strlen(text) != 64U) return 0;
    for (size_t index = 0; index < 64U; ++index) {
        char value = text[index];
        if (!((value >= '0' && value <= '9') || (value >= 'a' && value <= 'f'))) return 0;
    }
    return 1;
}

static void replay_receipt_clear(StasisReplayReceipt *receipt) {
    *receipt = (StasisReplayReceipt){.result = STASIS_REPLAY_OK};
}

static int32_t replay_fail(
    StasisReplayConsumer *consumer,
    StasisReplayResult result,
    const char *code,
    const char *format,
    ...
) {
    if (consumer == NULL) return result;
    consumer->receipt.result = result;
    consumer->receipt.verified = 0U;
    consumer->receipt.completed = 0U;
    if (code != NULL) {
        snprintf(consumer->receipt.code, sizeof(consumer->receipt.code), "%s", code);
    }
    va_list arguments;
    va_start(arguments, format);
    vsnprintf(consumer->receipt.diagnostic, sizeof(consumer->receipt.diagnostic), format, arguments);
    va_end(arguments);
    return result;
}

static int replay_is_object(const cJSON *value) {
    return value != NULL && cJSON_IsObject(value);
}

static int replay_key_allowed(const char *key, const char *const *allowed, size_t count) {
    if (key == NULL) return 0;
    for (size_t index = 0; index < count; ++index) {
        if (strcmp(key, allowed[index]) == 0) return 1;
    }
    return 0;
}

static StasisReplayResult replay_check_object(
    StasisReplayConsumer *consumer,
    const cJSON *object,
    const char *const *required,
    size_t required_count,
    const char *const *optional,
    size_t optional_count
) {
    if (!replay_is_object(object)) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_object", "replay value must be an object");
        return STASIS_REPLAY_INVALID_DOCUMENT;
    }
    for (const cJSON *child = object->child; child != NULL; child = child->next) {
        if (child->string == NULL) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_key", "replay object has a missing key");
            return STASIS_REPLAY_INVALID_DOCUMENT;
        }
        for (const cJSON *other = object->child; other != child; other = other->next) {
            if (other->string != NULL && strcmp(other->string, child->string) == 0) {
                replay_fail(consumer, STASIS_REPLAY_DUPLICATE_FIELD, "duplicate_field", "replay object contains duplicate field '%s'", child->string);
                return STASIS_REPLAY_DUPLICATE_FIELD;
            }
        }
        if (!replay_key_allowed(child->string, required, required_count)
            && !replay_key_allowed(child->string, optional, optional_count)) {
            replay_fail(consumer, STASIS_REPLAY_UNKNOWN_FIELD, "unknown_field", "replay object contains unknown field '%s'", child->string);
            return STASIS_REPLAY_UNKNOWN_FIELD;
        }
    }
    for (size_t index = 0; index < required_count; ++index) {
        if (cJSON_GetObjectItemCaseSensitive(object, required[index]) == NULL) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "missing_field", "replay object is missing field '%s'", required[index]);
            return STASIS_REPLAY_INVALID_DOCUMENT;
        }
    }
    return STASIS_REPLAY_OK;
}

static const cJSON *replay_required_number(
    StasisReplayConsumer *consumer,
    const cJSON *object,
    const char *name,
    uint64_t minimum,
    uint64_t maximum,
    uint64_t *output
) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(object, name);
    if (!cJSON_IsNumber(value) || !isfinite(value->valuedouble)
        || value->valuedouble < (double)minimum
        || value->valuedouble > (double)maximum
        || floor(value->valuedouble) != value->valuedouble) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_integer", "replay field '%s' is outside its integer bounds", name);
        return NULL;
    }
    *output = (uint64_t)value->valuedouble;
    return value;
}

static const char *replay_required_text(
    StasisReplayConsumer *consumer,
    const cJSON *object,
    const char *name,
    int allow_empty
) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(object, name);
    if (!cJSON_IsString(value) || !replay_text_valid(value->valuestring, allow_empty)) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_text", "replay field '%s' is not bounded text", name);
        return NULL;
    }
    return value->valuestring;
}

static int replay_hash_field(
    StasisReplayConsumer *consumer,
    const cJSON *object,
    const char *name,
    const char **output
) {
    const char *value = replay_required_text(consumer, object, name, 0);
    if (value == NULL || !replay_hash_text_valid(value)) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_hash", "replay field '%s' is not lowercase 64-hex", name);
        return 0;
    }
    *output = value;
    return 1;
}

static int replay_optional_hash(
    StasisReplayConsumer *consumer,
    const cJSON *object,
    const char *name,
    const char **output
) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(object, name);
    if (value == NULL || cJSON_IsNull(value)) {
        *output = NULL;
        return 1;
    }
    if (!cJSON_IsString(value) || !replay_hash_text_valid(value->valuestring)) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_hash", "replay optional field '%s' is not lowercase 64-hex or null", name);
        return 0;
    }
    *output = value->valuestring;
    return 1;
}

static int replay_optional_positive_number(
    StasisReplayConsumer *consumer,
    const cJSON *object,
    const char *name,
    uint32_t *output
) {
    const cJSON *value = cJSON_GetObjectItemCaseSensitive(object, name);
    if (value == NULL || cJSON_IsNull(value)) {
        *output = 0U;
        return 1;
    }
    uint64_t number = 0U;
    if (replay_required_number(consumer, object, name, 1U, UINT32_MAX, &number) == NULL) return 0;
    (void)number;
    *output = (uint32_t)number;
    return 1;
}

static int replay_descriptor_array(
    StasisReplayConsumer *consumer,
    const cJSON *compatibility,
    const char *name,
    uint32_t host_count,
    const StasisReplayInputDescriptor *expected,
    size_t expected_count,
    size_t *actual_count
) {
    const cJSON *array = cJSON_GetObjectItemCaseSensitive(compatibility, name);
    if (!cJSON_IsArray(array) || (size_t)cJSON_GetArraySize(array) > STASIS_REPLAY_MAX_HOST_VALUES) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_descriptor", "replay descriptor '%s' is not bounded", name);
        return 0;
    }
    size_t count = (size_t)cJSON_GetArraySize(array);
    if (count != expected_count) {
        replay_fail(consumer, STASIS_REPLAY_IDENTITY_MISMATCH, "identity_mismatch", "replay descriptor '%s' count differs from packaged identity", name);
        return 0;
    }
    int have_previous = 0;
    uint32_t previous_index = 0U;
    for (size_t index = 0; index < count; ++index) {
        const cJSON *field = cJSON_GetArrayItem(array, (int)index);
        static const char *const required[] = {"slot", "index", "path", "family"};
        if (replay_check_object(consumer, field, required, 4U, NULL, 0U) != STASIS_REPLAY_OK) return 0;
        uint64_t slot = 0U;
        uint64_t field_index = 0U;
        if (replay_required_number(consumer, field, "slot", 0U, STASIS_REPLAY_MAX_HOST_VALUES - 1U, &slot) == NULL
            || replay_required_number(consumer, field, "index", 0U, host_count - 1U, &field_index) == NULL) return 0;
        const char *path = replay_required_text(consumer, field, "path", 0);
        const char *family = replay_required_text(consumer, field, "family", 0);
        if (path == NULL || family == NULL) return 0;
        if (slot != index || (have_previous && field_index <= previous_index)) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "noncanonical_descriptor", "replay descriptor '%s' is not sorted and canonical", name);
            return 0;
        }
        if (expected != NULL && (expected[index].path == NULL || expected[index].family == NULL
            || expected[index].slot != slot || expected[index].index != field_index
            || strcmp(expected[index].path, path) != 0 || strcmp(expected[index].family, family) != 0)) {
            replay_fail(consumer, STASIS_REPLAY_IDENTITY_MISMATCH, "identity_mismatch", "replay descriptor '%s' differs from packaged identity", name);
            return 0;
        }
        previous_index = (uint32_t)field_index;
        have_previous = 1;
    }
    *actual_count = count;
    return 1;
}

static int replay_compatibility(
    StasisReplayConsumer *consumer,
    const cJSON *compatibility,
    const StasisReplayCompatibility *expected
) {
    static const char *const required[] = {
        "stasis_version", "release_id", "source_sha256", "state_layout_sha256",
        "compiler_layout_sha256", "host_schema_version", "host_i32_count", "host_f32_count",
        "input_usage_sha256", "tick_rate_hz", "hash_scope", "determinism_profile",
        "observed_i32", "observed_f32",
    };
    static const char *const optional[] = {"asset_manifest_sha256", "controller_schema_version"};
    if (replay_check_object(consumer, compatibility, required, 14U, optional, 2U) != STASIS_REPLAY_OK) return 0;
    const char *stasis_version = replay_required_text(consumer, compatibility, "stasis_version", 0);
    const char *release_id = replay_required_text(consumer, compatibility, "release_id", 0);
    const char *source_sha256 = NULL;
    const char *state_layout_sha256 = NULL;
    const char *compiler_layout_sha256 = NULL;
    const char *input_usage_sha256 = NULL;
    if (stasis_version == NULL || release_id == NULL
        || !replay_hash_field(consumer, compatibility, "source_sha256", &source_sha256)
        || !replay_hash_field(consumer, compatibility, "state_layout_sha256", &state_layout_sha256)
        || !replay_hash_field(consumer, compatibility, "compiler_layout_sha256", &compiler_layout_sha256)
        || !replay_hash_field(consumer, compatibility, "input_usage_sha256", &input_usage_sha256)) return 0;
    uint64_t host_schema = 0U;
    uint64_t host_i32 = 0U;
    uint64_t host_f32 = 0U;
    uint64_t tick_rate = 0U;
    if (replay_required_number(consumer, compatibility, "host_schema_version", 1U, UINT32_MAX, &host_schema) == NULL
        || replay_required_number(consumer, compatibility, "host_i32_count", 1U, STASIS_REPLAY_MAX_HOST_VALUES, &host_i32) == NULL
        || replay_required_number(consumer, compatibility, "host_f32_count", 1U, STASIS_REPLAY_MAX_HOST_VALUES, &host_f32) == NULL
        || replay_required_number(consumer, compatibility, "tick_rate_hz", 1U, UINT32_MAX, &tick_rate) == NULL) return 0;
    const char *hash_scope = replay_required_text(consumer, compatibility, "hash_scope", 0);
    const char *determinism_profile = replay_required_text(consumer, compatibility, "determinism_profile", 0);
    const char *asset_manifest_sha256 = NULL;
    uint32_t controller_schema_version = 0U;
    if (hash_scope == NULL || determinism_profile == NULL
        || !replay_optional_hash(consumer, compatibility, "asset_manifest_sha256", &asset_manifest_sha256)
        || !replay_optional_positive_number(consumer, compatibility, "controller_schema_version", &controller_schema_version)) return 0;
    if (strcmp(hash_scope, "simulation_after_tick") != 0) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_hash_scope", "replay hash_scope must be simulation_after_tick");
        return 0;
    }
    size_t i32_count = 0U;
    size_t f32_count = 0U;
    if (!replay_descriptor_array(consumer, compatibility, "observed_i32", (uint32_t)host_i32,
            expected == NULL ? NULL : expected->observed_i32,
            expected == NULL ? 0U : expected->observed_i32_count, &i32_count)
        || !replay_descriptor_array(consumer, compatibility, "observed_f32", (uint32_t)host_f32,
            expected == NULL ? NULL : expected->observed_f32,
            expected == NULL ? 0U : expected->observed_f32_count, &f32_count)) return 0;

    if (expected != NULL) {
        const char *expected_texts[] = {
            expected->stasis_version, expected->release_id, expected->source_sha256,
            expected->state_layout_sha256, expected->compiler_layout_sha256,
            expected->input_usage_sha256, expected->hash_scope, expected->determinism_profile,
        };
        const char *actual_texts[] = {
            stasis_version, release_id, source_sha256, state_layout_sha256,
            compiler_layout_sha256, input_usage_sha256, hash_scope, determinism_profile,
        };
        for (size_t index = 0; index < sizeof(expected_texts) / sizeof(expected_texts[0]); ++index) {
            if (expected_texts[index] == NULL || strcmp(expected_texts[index], actual_texts[index]) != 0) {
                replay_fail(consumer, STASIS_REPLAY_IDENTITY_MISMATCH, "identity_mismatch", "portable replay compatibility differs at field %zu", index);
                return 0;
            }
        }
        if (expected->asset_manifest_sha256 == NULL
            ? asset_manifest_sha256 != NULL
            : asset_manifest_sha256 == NULL || strcmp(expected->asset_manifest_sha256, asset_manifest_sha256) != 0) {
            replay_fail(consumer, STASIS_REPLAY_IDENTITY_MISMATCH, "identity_mismatch",
                "portable replay asset identity differs (package %.64s, recording %.64s)",
                expected->asset_manifest_sha256 == NULL ? "null" : expected->asset_manifest_sha256,
                asset_manifest_sha256 == NULL ? "null" : asset_manifest_sha256);
            return 0;
        }
        if ((expected->controller_schema_version == 0U
                ? controller_schema_version != 0U
                : controller_schema_version != expected->controller_schema_version)) {
            replay_fail(consumer, STASIS_REPLAY_IDENTITY_MISMATCH, "identity_mismatch", "portable replay controller schema differs");
            return 0;
        }
        if (expected->host_schema_version != host_schema || expected->host_i32_count != host_i32
            || expected->host_f32_count != host_f32 || expected->tick_rate_hz != tick_rate
            || expected->observed_i32_count != i32_count || expected->observed_f32_count != f32_count) {
            replay_fail(consumer, STASIS_REPLAY_IDENTITY_MISMATCH, "identity_mismatch", "portable replay host dimensions differ");
            return 0;
        }
    }
    consumer->host_i32_count = (uint32_t)host_i32;
    consumer->host_f32_count = (uint32_t)host_f32;
    consumer->input_i32_count = i32_count;
    consumer->input_f32_count = f32_count;
    return 1;
}

static int replay_producer(StasisReplayConsumer *consumer, const cJSON *producer) {
    static const char *const required[] = {"target", "runtime_sha256"};
    if (replay_check_object(consumer, producer, required, 2U, NULL, 0U) != STASIS_REPLAY_OK) return 0;
    const char *target = replay_required_text(consumer, producer, "target", 0);
    const char *runtime_sha256 = NULL;
    if (target == NULL || !replay_hash_field(consumer, producer, "runtime_sha256", &runtime_sha256)) return 0;
    (void)target;
    (void)runtime_sha256;
    return 1;
}

static int replay_hash_matches_descriptors(
    StasisReplayConsumer *consumer,
    const cJSON *compatibility,
    const char *expected_hash
) {
    ReplaySha256 hash;
    uint8_t digest[32];
    replay_sha256_init(&hash);
    replay_sha256_update(&hash, "stasis.replay.input-usage.v1\0", sizeof("stasis.replay.input-usage.v1\0") - 1U);
    const char *names[] = {"observed_i32", "observed_f32"};
    const uint8_t lanes[] = {'i', 'f'};
    for (size_t lane = 0; lane < 2U; ++lane) {
        const cJSON *array = cJSON_GetObjectItemCaseSensitive(compatibility, names[lane]);
        int count = cJSON_GetArraySize(array);
        uint64_t count64 = (uint64_t)count;
        replay_sha256_update(&hash, &lanes[lane], 1U);
        replay_update_u64_le(&hash, count64);
        for (int item_index = 0; item_index < count; ++item_index) {
            const cJSON *field = cJSON_GetArrayItem(array, item_index);
            uint64_t slot = 0U;
            uint64_t index = 0U;
            cJSON *slot_node = cJSON_GetObjectItemCaseSensitive(field, "slot");
            cJSON *index_node = cJSON_GetObjectItemCaseSensitive(field, "index");
            const char *path = cJSON_GetObjectItemCaseSensitive(field, "path")->valuestring;
            const char *family = cJSON_GetObjectItemCaseSensitive(field, "family")->valuestring;
            slot = (uint64_t)slot_node->valuedouble;
            index = (uint64_t)index_node->valuedouble;
            uint64_t path_length = (uint64_t)strlen(path);
            uint64_t family_length = (uint64_t)strlen(family);
            replay_update_u64_le(&hash, slot);
            replay_update_u64_le(&hash, index);
            replay_update_u64_le(&hash, path_length);
            replay_sha256_update(&hash, path, (size_t)path_length);
            replay_update_u64_le(&hash, family_length);
            replay_sha256_update(&hash, family, (size_t)family_length);
        }
    }
    replay_sha256_finish(&hash, digest);
    char encoded[65];
    for (size_t index = 0; index < 32U; ++index) snprintf(encoded + index * 2U, 3U, "%02x", digest[index]);
    encoded[64] = '\0';
    if (strcmp(encoded, expected_hash) != 0) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_input_usage_hash", "replay input_usage_sha256 does not match descriptors: got %s", encoded);
        return 0;
    }
    return 1;
}

static int replay_array_length(const cJSON *array, size_t maximum) {
    return cJSON_IsArray(array) && cJSON_GetArraySize(array) >= 0
        && (size_t)cJSON_GetArraySize(array) <= maximum;
}

static int replay_parse_baseline(StasisReplayConsumer *consumer, const cJSON *initial_input) {
    static const char *const required[] = {"i32_values", "f32_bits"};
    if (replay_check_object(consumer, initial_input, required, 2U, NULL, 0U) != STASIS_REPLAY_OK) return 0;
    const cJSON *i32 = cJSON_GetObjectItemCaseSensitive(initial_input, "i32_values");
    const cJSON *f32 = cJSON_GetObjectItemCaseSensitive(initial_input, "f32_bits");
    if (!replay_array_length(i32, STASIS_REPLAY_MAX_HOST_VALUES)
        || !replay_array_length(f32, STASIS_REPLAY_MAX_HOST_VALUES)
        || (size_t)cJSON_GetArraySize(i32) != consumer->input_i32_count
        || (size_t)cJSON_GetArraySize(f32) != consumer->input_f32_count) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "input_dimensions", "replay initial_input lengths do not match observed descriptors");
        return 0;
    }
    if (consumer->input_i32_count != 0U) {
        consumer->input_i32 = (int32_t *)calloc(consumer->input_i32_count, sizeof(int32_t));
        if (consumer->input_i32 == NULL) {
            replay_fail(consumer, STASIS_REPLAY_BOUNDS, "allocation_failed", "replay i32 baseline allocation failed");
            return 0;
        }
    }
    if (consumer->input_f32_count != 0U) {
        consumer->input_f32_bits = (uint32_t *)calloc(consumer->input_f32_count, sizeof(uint32_t));
        if (consumer->input_f32_bits == NULL) {
            replay_fail(consumer, STASIS_REPLAY_BOUNDS, "allocation_failed", "replay f32 baseline allocation failed");
            return 0;
        }
    }
    for (size_t index = 0; index < consumer->input_i32_count; ++index) {
        const cJSON *value = cJSON_GetArrayItem(i32, (int)index);
        if (!cJSON_IsNumber(value) || !isfinite(value->valuedouble)
            || value->valuedouble < (double)INT32_MIN || value->valuedouble > (double)INT32_MAX
            || floor(value->valuedouble) != value->valuedouble) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_input", "replay i32 baseline contains a non-i32 value");
            return 0;
        }
        consumer->input_i32[index] = (int32_t)value->valuedouble;
    }
    for (size_t index = 0; index < consumer->input_f32_count; ++index) {
        const cJSON *value = cJSON_GetArrayItem(f32, (int)index);
        if (!cJSON_IsNumber(value) || !isfinite(value->valuedouble)
            || value->valuedouble < 0.0 || value->valuedouble > 4294967295.0
            || floor(value->valuedouble) != value->valuedouble) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_input", "replay f32 baseline contains invalid bits");
            return 0;
        }
        consumer->input_f32_bits[index] = (uint32_t)value->valuedouble;
    }
    return 1;
}

static int replay_change_array(
    StasisReplayConsumer *consumer,
    const cJSON *segment,
    const char *name,
    uint32_t limit,
    int32_t *i32_values,
    uint32_t *f32_values,
    size_t count,
    int is_f32,
    size_t segment_index
) {
    const cJSON *changes = cJSON_GetObjectItemCaseSensitive(segment, name);
    if (!replay_array_length(changes, STASIS_REPLAY_MAX_CHANGES_PER_SEGMENT)) {
        replay_fail(consumer, STASIS_REPLAY_BOUNDS, "changes_too_large", "replay segment %zu changes are not bounded", segment_index);
        return 0;
    }
    int change_count = cJSON_GetArraySize(changes);
    if (change_count > 0 && limit == 0U) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "input_dimensions",
            "replay segment %zu contains changes for an empty input lane", segment_index);
        return 0;
    }
    uint64_t previous_slot = 0U;
    int have_previous = 0;
    for (int index = 0; index < change_count; ++index) {
        const cJSON *change = cJSON_GetArrayItem(changes, index);
        static const char *const i32_required[] = {"slot", "value"};
        static const char *const f32_required[] = {"slot", "bits"};
        const char *const *required = is_f32 ? f32_required : i32_required;
        if (replay_check_object(consumer, change, required, 2U, NULL, 0U) != STASIS_REPLAY_OK) return 0;
        uint64_t slot = 0U;
        if (replay_required_number(consumer, change, "slot", 0U, limit - 1U, &slot) == NULL) return 0;
        if (have_previous && slot <= previous_slot) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "noncanonical_change", "replay segment %zu changes are not sorted and unique", segment_index);
            return 0;
        }
        if (is_f32) {
            uint64_t bits = 0U;
            if (replay_required_number(consumer, change, "bits", 0U, UINT32_MAX, &bits) == NULL) return 0;
            if (f32_values[slot] == (uint32_t)bits) {
                replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "redundant_change", "replay segment %zu contains a redundant f32 change", segment_index);
                return 0;
            }
            f32_values[slot] = (uint32_t)bits;
        } else {
            const cJSON *value_node = cJSON_GetObjectItemCaseSensitive(change, "value");
            if (!cJSON_IsNumber(value_node) || !isfinite(value_node->valuedouble)
                || value_node->valuedouble < (double)INT32_MIN
                || value_node->valuedouble > (double)INT32_MAX
                || floor(value_node->valuedouble) != value_node->valuedouble) {
                replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_input", "replay i32 change is outside signed 32-bit bounds");
                return 0;
            }
            int32_t value = (int32_t)value_node->valuedouble;
            if (i32_values[slot] == value) {
                replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "redundant_change", "replay segment %zu contains a redundant i32 change", segment_index);
                return 0;
            }
            i32_values[slot] = value;
        }
        previous_slot = slot;
        have_previous = 1;
    }
    (void)count;
    return 1;
}

static int replay_validate_segments(StasisReplayConsumer *consumer, const cJSON *segments) {
    if (!replay_array_length(segments, STASIS_REPLAY_MAX_TICKS) || cJSON_GetArraySize(segments) == 0) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_segments", "replay must contain 1..%llu segments", (unsigned long long)STASIS_REPLAY_MAX_TICKS);
        return 0;
    }
    uint64_t cursor = 0U;
    for (int index = 0; index < cJSON_GetArraySize(segments); ++index) {
        const cJSON *segment = cJSON_GetArrayItem(segments, index);
        static const char *const required[] = {"tick_gap", "run_ticks", "i32_changes", "f32_changes"};
        if (replay_check_object(consumer, segment, required, 4U, NULL, 0U) != STASIS_REPLAY_OK) return 0;
        uint64_t tick_gap = 0U;
        uint64_t run_ticks = 0U;
        if (replay_required_number(consumer, segment, "tick_gap", 0U, STASIS_REPLAY_MAX_TICKS, &tick_gap) == NULL
            || replay_required_number(consumer, segment, "run_ticks", 1U, STASIS_REPLAY_MAX_TICKS, &run_ticks) == NULL) return 0;
        if (index == 0 && tick_gap != 0U) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "segment_coverage", "replay first segment must have tick_gap=0");
            return 0;
        }
        const cJSON *i32_changes = cJSON_GetObjectItemCaseSensitive(segment, "i32_changes");
        const cJSON *f32_changes = cJSON_GetObjectItemCaseSensitive(segment, "f32_changes");
        int i32_count = cJSON_GetArraySize(i32_changes);
        int f32_count = cJSON_GetArraySize(f32_changes);
        if (!replay_array_length(i32_changes, STASIS_REPLAY_MAX_CHANGES_PER_SEGMENT)
            || !replay_array_length(f32_changes, STASIS_REPLAY_MAX_CHANGES_PER_SEGMENT)
            || (uint64_t)i32_count + (uint64_t)f32_count > STASIS_REPLAY_MAX_CHANGES_PER_SEGMENT) {
            replay_fail(consumer, STASIS_REPLAY_BOUNDS, "changes_too_large", "replay segment %d has too many changes", index);
            return 0;
        }
        uint64_t gap_end = cursor + tick_gap;
        uint64_t run_end = gap_end + run_ticks;
        if (gap_end < cursor || run_end < gap_end || run_end > consumer->total_ticks) {
            replay_fail(consumer, STASIS_REPLAY_BOUNDS, "segment_coverage", "replay segment %d exceeds total_ticks", index);
            return 0;
        }
        if (!replay_change_array(consumer, segment, "i32_changes", (uint32_t)consumer->input_i32_count,
                consumer->input_i32, NULL, consumer->input_i32_count, 0, (size_t)index)
            || !replay_change_array(consumer, segment, "f32_changes", (uint32_t)consumer->input_f32_count,
                NULL, consumer->input_f32_bits, consumer->input_f32_count, 1, (size_t)index)) return 0;
        cursor = run_end;
    }
    if (cursor != consumer->total_ticks) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "segment_coverage", "replay segment coverage ends at %llu, expected %llu", (unsigned long long)cursor, (unsigned long long)consumer->total_ticks);
        return 0;
    }
    return 1;
}

static int replay_validate_checkpoints(StasisReplayConsumer *consumer, const cJSON *checkpoints, const cJSON *final_state) {
    if (!replay_array_length(checkpoints, STASIS_REPLAY_MAX_CHECKPOINTS)) {
        replay_fail(consumer, STASIS_REPLAY_BOUNDS, "checkpoints_too_large", "replay checkpoints exceed their bound");
        return 0;
    }
    uint64_t expected_count = consumer->total_ticks / 256U;
    if ((uint64_t)cJSON_GetArraySize(checkpoints) != expected_count) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "checkpoint_coverage", "replay must contain exactly %llu checkpoints", (unsigned long long)expected_count);
        return 0;
    }
    for (int index = 0; index < cJSON_GetArraySize(checkpoints); ++index) {
        const cJSON *checkpoint = cJSON_GetArrayItem(checkpoints, index);
        static const char *const required[] = {"tick", "state_sha256"};
        if (replay_check_object(consumer, checkpoint, required, 2U, NULL, 0U) != STASIS_REPLAY_OK) return 0;
        uint64_t tick = 0U;
        const char *state_hash = NULL;
        if (replay_required_number(consumer, checkpoint, "tick", 1U, consumer->total_ticks, &tick) == NULL
            || !replay_hash_field(consumer, checkpoint, "state_sha256", &state_hash)) return 0;
        if (tick != ((uint64_t)index + 1U) * 256U) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "checkpoint_coverage", "replay checkpoint %d has a noncanonical tick", index);
            return 0;
        }
        (void)state_hash;
    }
    static const char *const final_required[] = {"tick", "state_sha256"};
    if (replay_check_object(consumer, final_state, final_required, 2U, NULL, 0U) != STASIS_REPLAY_OK) return 0;
    uint64_t final_tick = 0U;
    const char *final_hash = NULL;
    if (replay_required_number(consumer, final_state, "tick", 1U, consumer->total_ticks, &final_tick) == NULL
        || !replay_hash_field(consumer, final_state, "state_sha256", &final_hash)) return 0;
    if (final_tick != consumer->total_ticks) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "checkpoint_coverage", "replay final tick does not equal total_ticks");
        return 0;
    }
    if (cJSON_GetArraySize(checkpoints) != 0) {
        const cJSON *last = cJSON_GetArrayItem(checkpoints, cJSON_GetArraySize(checkpoints) - 1);
        if (cJSON_GetObjectItemCaseSensitive(last, "tick")->valuedouble == (double)consumer->total_ticks
            && strcmp(cJSON_GetObjectItemCaseSensitive(last, "state_sha256")->valuestring, final_hash) != 0) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "checkpoint_coverage", "replay final checkpoint does not match final state");
            return 0;
        }
    }
    return 1;
}

static int replay_state_descriptor_valid(StasisReplayConsumer *consumer, const StasisReplayStateDescriptor *descriptor) {
    if (descriptor == NULL || descriptor->required_bytes > STASIS_REPLAY_MAX_FILE_BYTES
        || descriptor->entry_count > STASIS_REPLAY_MAX_TICKS
        || (descriptor->entry_count != 0U && descriptor->entries == NULL)) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay state descriptor is outside bounds");
        return 0;
    }
    uint64_t expected_offset = 0U;
    int saw_collection = 0;
    const char *previous_scalar_path = NULL;
    const char *previous_collection_path = NULL;
    const char *previous_collection_field = NULL;
    for (size_t index = 0; index < descriptor->entry_count; ++index) {
        const StasisReplayStateEntry *entry = &descriptor->entries[index];
        if (entry->kind == NULL || entry->path == NULL || entry->field == NULL
            || entry->storage_type == NULL
            || (strcmp(entry->kind, "scalar") != 0 && strcmp(entry->kind, "collection") != 0)
            || !replay_text_valid(entry->path, 0) || !replay_text_valid(entry->field, 1)
            || !replay_text_valid(entry->storage_type, 0)) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay state descriptor entry %zu is invalid", index);
            return 0;
        }
        uint8_t expected_width = 0U;
        if (strcmp(entry->storage_type, "bool") == 0 || strcmp(entry->storage_type, "u8") == 0) expected_width = 1U;
        else if (strcmp(entry->storage_type, "u16") == 0) expected_width = 2U;
        else if (strcmp(entry->storage_type, "i32") == 0 || strcmp(entry->storage_type, "f32") == 0 || strcmp(entry->storage_type, "u32") == 0) expected_width = 4U;
        else if (strcmp(entry->storage_type, "f64") == 0) expected_width = 8U;
        else {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay state descriptor entry %zu has unsupported type", index);
            return 0;
        }
        if (entry->element_bytes != expected_width || (strcmp(entry->kind, "scalar") == 0
                ? (entry->field[0] != '\0' || entry->element_count != 1U)
                : 0)) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay state descriptor entry %zu has invalid width or count", index);
            return 0;
        }
        if (strcmp(entry->kind, "scalar") == 0) {
            if (saw_collection || (previous_scalar_path != NULL
                    && strcmp(entry->path, previous_scalar_path) <= 0)) {
                replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay scalar descriptor entries are not sorted");
                return 0;
            }
            previous_scalar_path = entry->path;
        } else {
            saw_collection = 1;
            if (previous_collection_path != NULL
                && (strcmp(entry->path, previous_collection_path) < 0
                    || (strcmp(entry->path, previous_collection_path) == 0
                        && strcmp(entry->field, previous_collection_field) <= 0))) {
                replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay collection descriptor fields are not sorted");
                return 0;
            }
            previous_collection_path = entry->path;
            previous_collection_field = entry->field;
        }
        if (entry->offset != expected_offset || entry->element_count > STASIS_REPLAY_MAX_TICKS
            || entry->element_count > (UINT64_MAX / entry->element_bytes)) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay state descriptor entry %zu is not contiguous", index);
            return 0;
        }
        expected_offset += entry->element_count * entry->element_bytes;
        if (expected_offset > descriptor->required_bytes) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay state descriptor entry %zu exceeds required bytes", index);
            return 0;
        }
    }
    if (expected_offset != descriptor->required_bytes) {
        replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_state_descriptor", "replay state descriptor coverage differs from required bytes");
        return 0;
    }
    return 1;
}

static int replay_parse_scalar_bytes(
    StasisReplayConsumer *consumer,
    const cJSON *value,
    const char *expected_type,
    uint8_t *output,
    size_t width
) {
    static const char *const required[] = {"type_name", "bits"};
    if (replay_check_object(consumer, value, required, 2U, NULL, 0U) != STASIS_REPLAY_OK) return 0;
    const char *type_name = replay_required_text(consumer, value, "type_name", 0);
    const char *bits = replay_required_text(consumer, value, "bits", 0);
    if (type_name == NULL || bits == NULL || strcmp(type_name, expected_type) != 0 || strlen(bits) != width * 2U) {
        replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "state_type_mismatch", "replay initial state scalar type or width differs from descriptor");
        return 0;
    }
    for (size_t index = 0; index < width * 2U; ++index) {
        char digit = bits[index];
        if (!((digit >= '0' && digit <= '9') || (digit >= 'a' && digit <= 'f'))) {
            replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "state_type_mismatch", "replay initial state scalar bits are not lowercase hexadecimal");
            return 0;
        }
    }
    for (size_t index = 0; index < width; ++index) {
        size_t source = (width - index - 1U) * 2U;
        uint8_t high = (uint8_t)(bits[source] <= '9' ? bits[source] - '0' : bits[source] - 'a' + 10);
        uint8_t low = (uint8_t)(bits[source + 1U] <= '9' ? bits[source + 1U] - '0' : bits[source + 1U] - 'a' + 10);
        output[index] = (uint8_t)((high << 4U) | low);
    }
    if (strcmp(expected_type, "bool") == 0 && output[0] > 1U) {
        replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "invalid_state_value", "replay initial state bool is outside 0..1");
        return 0;
    }
    return 1;
}

static const StasisReplayStateEntry *replay_find_state_entry(
    const StasisReplayStateDescriptor *descriptor,
    const char *kind,
    const char *path,
    const char *field
) {
    for (size_t index = 0; index < descriptor->entry_count; ++index) {
        const StasisReplayStateEntry *entry = &descriptor->entries[index];
        if (strcmp(entry->kind, kind) == 0 && strcmp(entry->path, path) == 0
            && strcmp(entry->field, field) == 0) return entry;
    }
    return NULL;
}

static void replay_update_u64_le(ReplaySha256 *hash, uint64_t value) {
    uint8_t bytes[8];
    for (size_t index = 0; index < 8U; ++index) bytes[index] = (uint8_t)(value >> (index * 8U));
    replay_sha256_update(hash, bytes, sizeof(bytes));
}

static uint8_t replay_state_type_tag(const char *type) {
    if (strcmp(type, "i32") == 0) return 1U;
    if (strcmp(type, "f32") == 0) return 2U;
    if (strcmp(type, "f64") == 0) return 3U;
    if (strcmp(type, "bool") == 0) return 4U;
    if (strcmp(type, "u8") == 0) return 5U;
    if (strcmp(type, "u16") == 0) return 6U;
    return 7U;
}

static int replay_hash_snapshot(
    StasisReplayConsumer *consumer,
    const uint8_t *snapshot,
    char output[65]
) {
    ReplaySha256 hash;
    replay_sha256_init(&hash);
    replay_sha256_update(&hash, "stasis.simulation-state.v1\0", sizeof("stasis.simulation-state.v1\0") - 1U);
    for (size_t index = 0; index < consumer->state_descriptor.entry_count; ++index) {
        const StasisReplayStateEntry *entry = &consumer->state_descriptor.entries[index];
        for (uint64_t element = 0; element < entry->element_count; ++element) {
            size_t label_length = replay_text_length(entry->path);
            char decimal[32];
            if (strcmp(entry->kind, "collection") == 0) {
                int count = snprintf(decimal, sizeof(decimal), "%llu", (unsigned long long)element);
                if (count < 0) return 0;
                label_length += 3U + (size_t)count + replay_text_length(entry->field);
                replay_update_u64_le(&hash, (uint64_t)label_length);
                replay_sha256_update(&hash, entry->path, replay_text_length(entry->path));
                replay_sha256_update(&hash, "[", 1U);
                replay_sha256_update(&hash, decimal, (size_t)count);
                replay_sha256_update(&hash, "].", 2U);
                replay_sha256_update(&hash, entry->field, replay_text_length(entry->field));
            } else {
                replay_update_u64_le(&hash, (uint64_t)label_length);
                replay_sha256_update(&hash, entry->path, label_length);
            }
            uint64_t offset = entry->offset + element * entry->element_bytes;
            const uint8_t *value = snapshot + offset;
            if (strcmp(entry->storage_type, "bool") == 0 && value[0] > 1U) {
                replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "invalid_state_value", "replay state bool is outside 0..1");
                return 0;
            }
            uint8_t tag = replay_state_type_tag(entry->storage_type);
            replay_sha256_update(&hash, &tag, 1U);
            replay_sha256_update(&hash, value, entry->element_bytes);
        }
    }
    uint8_t digest[32];
    replay_sha256_finish(&hash, digest);
    for (size_t index = 0; index < 32U; ++index) snprintf(output + index * 2U, 3U, "%02x", digest[index]);
    output[64] = '\0';
    return 1;
}

static int replay_hash_state(
    StasisReplayConsumer *consumer,
    uint8_t *snapshot,
    char output[65]
) {
    if (consumer->state_ops.write == NULL) {
        replay_fail(consumer, STASIS_REPLAY_MISSING_STATE_ABI, "missing_snapshot_abi", "replay state snapshot write operation is unavailable");
        return 0;
    }
    int32_t written = consumer->state_ops.write(
        consumer->state_ops.context, snapshot, (int32_t)consumer->state_descriptor.required_bytes);
    if (written != (int32_t)consumer->state_descriptor.required_bytes) {
        replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "state_bytes_mismatch", "replay state snapshot write returned %d", written);
        return 0;
    }
    return replay_hash_snapshot(consumer, snapshot, output);
}

static int replay_restore_initial_state(StasisReplayConsumer *consumer) {
    if (consumer->state_ops.size == NULL || consumer->state_ops.write == NULL || consumer->state_ops.restore == NULL) {
        replay_fail(consumer, STASIS_REPLAY_MISSING_STATE_ABI, "missing_snapshot_abi", "replay requires size, write, and restore state snapshot operations");
        return 0;
    }
    int32_t size = consumer->state_ops.size(consumer->state_ops.context);
    if (size < 0 || (uint64_t)size != consumer->state_descriptor.required_bytes) {
        replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "state_bytes_mismatch", "replay state snapshot size %d differs from descriptor", size);
        return 0;
    }
    size_t bytes = (size_t)size;
    uint8_t *snapshot = bytes == 0U ? NULL : (uint8_t *)calloc(bytes, 1U);
    if (bytes != 0U && snapshot == NULL) {
        replay_fail(consumer, STASIS_REPLAY_BOUNDS, "allocation_failed", "replay state snapshot allocation failed");
        return 0;
    }
    const cJSON *values = cJSON_GetObjectItemCaseSensitive((const cJSON *)consumer->initial_state, "values");
    size_t previous_entry_index = SIZE_MAX;
    uint64_t previous_element = 0U;
    for (int index = 0; index < cJSON_GetArraySize(values); ++index) {
        const cJSON *entry = cJSON_GetArrayItem(values, index);
        static const char *const entry_required[] = {"location", "value"};
        if (replay_check_object(consumer, entry, entry_required, 2U, NULL, 0U) != STASIS_REPLAY_OK) goto fail;
        const cJSON *location = cJSON_GetObjectItemCaseSensitive(entry, "location");
        const cJSON *value = cJSON_GetObjectItemCaseSensitive(entry, "value");
        const cJSON *kind = cJSON_GetObjectItemCaseSensitive(location, "kind");
        if (!cJSON_IsString(kind)) {
            replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "invalid_initial_state", "replay initial state location kind is invalid");
            goto fail;
        }
        const char *kind_text = kind->valuestring;
        const char *path = NULL;
        const char *field = "";
        uint64_t element = 0U;
        size_t location_required_count = strcmp(kind_text, "scalar") == 0 ? 2U : 4U;
        static const char *const scalar_required[] = {"kind", "path"};
        static const char *const collection_required[] = {"kind", "path", "field", "index"};
        if (strcmp(kind_text, "scalar") == 0) {
            if (replay_check_object(consumer, location, scalar_required, 2U, NULL, 0U) != STASIS_REPLAY_OK) goto fail;
            path = replay_required_text(consumer, location, "path", 0);
        } else if (strcmp(kind_text, "collection") == 0) {
            if (replay_check_object(consumer, location, collection_required, 4U, NULL, 0U) != STASIS_REPLAY_OK) goto fail;
            path = replay_required_text(consumer, location, "path", 0);
            field = replay_required_text(consumer, location, "field", 1);
            if (replay_required_number(consumer, location, "index", 0U, STASIS_REPLAY_MAX_TICKS - 1U, &element) == NULL) goto fail;
        } else {
            replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "invalid_initial_state", "replay initial state location kind is unknown");
            goto fail;
        }
        (void)location_required_count;
        if (path == NULL || field == NULL) goto fail;
        const StasisReplayStateEntry *descriptor_entry = replay_find_state_entry(
            &consumer->state_descriptor, kind_text, path, field);
        if (descriptor_entry == NULL || element >= descriptor_entry->element_count) {
            replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "state_location_mismatch", "replay initial state location is absent or out of bounds");
            goto fail;
        }
        size_t entry_index = (size_t)(descriptor_entry - consumer->state_descriptor.entries);
        if (previous_entry_index != SIZE_MAX &&
            (entry_index < previous_entry_index ||
             (entry_index == previous_entry_index && element <= previous_element))) {
            replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "noncanonical_initial_state", "replay initial state locations must be strictly ordered and unique");
            goto fail;
        }
        previous_entry_index = entry_index;
        previous_element = element;
        uint8_t *destination = snapshot + descriptor_entry->offset + element * descriptor_entry->element_bytes;
        if (!replay_parse_scalar_bytes(consumer, value, descriptor_entry->storage_type, destination, descriptor_entry->element_bytes)) goto fail;
        int nondefault = 0;
        for (size_t byte = 0; byte < descriptor_entry->element_bytes; ++byte) {
            if (destination[byte] != 0U) nondefault = 1;
        }
        if (!nondefault) {
            replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "noncanonical_initial_state", "replay initial state must omit default values");
            goto fail;
        }
    }
    {
        char actual[65];
        const cJSON *state_hash = cJSON_GetObjectItemCaseSensitive((const cJSON *)consumer->initial_state, "state_sha256");
        if (!replay_hash_snapshot(consumer, snapshot, actual) || strcmp(actual, state_hash->valuestring) != 0) {
            if (consumer->receipt.result == STASIS_REPLAY_OK) replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "state_mismatch", "replay initial state hash differs");
            goto fail;
        }
        int32_t restored = consumer->state_ops.restore(
            consumer->state_ops.context, snapshot, size);
        if (restored != size) {
            replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "state_bytes_mismatch", "replay state snapshot restore returned %d", restored);
            goto fail;
        }
        if (!replay_hash_state(consumer, snapshot, actual) || strcmp(actual, state_hash->valuestring) != 0) {
            if (consumer->receipt.result == STASIS_REPLAY_OK) replay_fail(consumer, STASIS_REPLAY_STATE_MISMATCH, "state_mismatch", "replay restored initial state hash differs");
            goto fail;
        }
    }
    free(snapshot);
    consumer->initialized = 1U;
    consumer->receipt.result = STASIS_REPLAY_OK;
    consumer->receipt.verified = 1U;
    consumer->receipt.tick = 0U;
    return 1;
fail:
    free(snapshot);
    return 0;
}

static int replay_apply_segment_changes(StasisReplayConsumer *consumer, const cJSON *segment) {
    const cJSON *i32_changes = cJSON_GetObjectItemCaseSensitive(segment, "i32_changes");
    const cJSON *f32_changes = cJSON_GetObjectItemCaseSensitive(segment, "f32_changes");
    for (int index = 0; index < cJSON_GetArraySize(i32_changes); ++index) {
        const cJSON *change = cJSON_GetArrayItem(i32_changes, index);
        uint64_t slot = (uint64_t)cJSON_GetObjectItemCaseSensitive(change, "slot")->valuedouble;
        int32_t value = (int32_t)(uint32_t)cJSON_GetObjectItemCaseSensitive(change, "value")->valuedouble;
        consumer->input_i32[slot] = value;
    }
    for (int index = 0; index < cJSON_GetArraySize(f32_changes); ++index) {
        const cJSON *change = cJSON_GetArrayItem(f32_changes, index);
        uint64_t slot = (uint64_t)cJSON_GetObjectItemCaseSensitive(change, "slot")->valuedouble;
        uint32_t bits = (uint32_t)cJSON_GetObjectItemCaseSensitive(change, "bits")->valuedouble;
        consumer->input_f32_bits[slot] = bits;
    }
    return 1;
}

static int replay_advance_input(StasisReplayConsumer *consumer, uint64_t tick) {
    const cJSON *segments = (const cJSON *)consumer->segments;
    for (;;) {
        const cJSON *segment = cJSON_GetArrayItem(segments, (int)consumer->segment_index);
        if (segment == NULL) {
            replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "segment_coverage", "replay has no segment for tick %llu", (unsigned long long)tick);
            return 0;
        }
        uint64_t gap = (uint64_t)cJSON_GetObjectItemCaseSensitive(segment, "tick_gap")->valuedouble;
        uint64_t run = (uint64_t)cJSON_GetObjectItemCaseSensitive(segment, "run_ticks")->valuedouble;
        uint64_t gap_end = consumer->segment_cursor + gap;
        if (tick <= gap_end) return 1;
        uint64_t run_end = gap_end + run;
        if (tick <= run_end) {
            if (!consumer->segment_applied) {
                replay_apply_segment_changes(consumer, segment);
                consumer->segment_applied = 1U;
            }
            return 1;
        }
        consumer->segment_index += 1U;
        consumer->segment_cursor = run_end;
        consumer->segment_applied = 0U;
    }
}

static void replay_reset_baseline(StasisReplayConsumer *consumer) {
    const cJSON *initial_input = (const cJSON *)consumer->initial_input;
    const cJSON *i32 = cJSON_GetObjectItemCaseSensitive(initial_input, "i32_values");
    const cJSON *f32 = cJSON_GetObjectItemCaseSensitive(initial_input, "f32_bits");
    for (size_t index = 0; index < consumer->input_i32_count; ++index) {
        consumer->input_i32[index] = (int32_t)cJSON_GetArrayItem(i32, (int)index)->valuedouble;
    }
    for (size_t index = 0; index < consumer->input_f32_count; ++index) {
        consumer->input_f32_bits[index] = (uint32_t)cJSON_GetArrayItem(f32, (int)index)->valuedouble;
    }
}

void stasis_replay_consumer_init(StasisReplayConsumer *consumer) {
    if (consumer != NULL) *consumer = (StasisReplayConsumer){0};
}

int32_t stasis_replay_consumer_load(
    StasisReplayConsumer *consumer,
    const uint8_t *bytes,
    size_t byte_count,
    const StasisReplayCompatibility *expected,
    const StasisReplayStateDescriptor *state_descriptor
) {
    if (consumer == NULL || bytes == NULL || byte_count == 0U || byte_count > STASIS_REPLAY_MAX_FILE_BYTES
        || expected == NULL || state_descriptor == NULL) return STASIS_REPLAY_INVALID_ARGUMENT;
    /* Callers dispose before reusing storage; this assignment is safe for first load. */
    stasis_replay_consumer_init(consumer);
    replay_receipt_clear(&consumer->receipt);
    consumer->expected = *expected;
    consumer->state_descriptor = *state_descriptor;
    if (!replay_state_descriptor_valid(consumer, state_descriptor)) goto fail;
    char *source = (char *)malloc(byte_count + 1U);
    if (source == NULL) return replay_fail(consumer, STASIS_REPLAY_BOUNDS, "allocation_failed", "replay input allocation failed");
    memcpy(source, bytes, byte_count);
    source[byte_count] = '\0';
    const char *end = NULL;
    cJSON *root = cJSON_ParseWithLengthOpts(source, byte_count, &end, 0);
    if (root == NULL || end == NULL) {
        free(source);
        cJSON_Delete(root);
        return replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_json", "replay JSON could not be parsed");
    }
    while ((size_t)(end - source) < byte_count && (*end == ' ' || *end == '\t' || *end == '\r' || *end == '\n')) ++end;
    if ((size_t)(end - source) != byte_count) {
        free(source);
        cJSON_Delete(root);
        return replay_fail(consumer, STASIS_REPLAY_INVALID_DOCUMENT, "invalid_json", "replay JSON contains trailing data");
    }
    free(source);
    consumer->root = root;
    static const char *const root_required[] = {"schema_version", "identity", "initial_state", "initial_input", "segments", "checkpoints", "total_ticks", "final_state"};
    if (replay_check_object(consumer, root, root_required, 8U, NULL, 0U) != STASIS_REPLAY_OK) goto fail;
    uint64_t schema = 0U;
    if (replay_required_number(consumer, root, "schema_version", STASIS_REPLAY_SCHEMA_VERSION, STASIS_REPLAY_SCHEMA_VERSION, &schema) == NULL) goto fail;
    (void)schema;
    const cJSON *identity = cJSON_GetObjectItemCaseSensitive(root, "identity");
    static const char *const identity_required[] = {"compatibility", "producer"};
    if (replay_check_object(consumer, identity, identity_required, 2U, NULL, 0U) != STASIS_REPLAY_OK
        || !replay_compatibility(consumer, cJSON_GetObjectItemCaseSensitive(identity, "compatibility"), expected)
        || !replay_producer(consumer, cJSON_GetObjectItemCaseSensitive(identity, "producer"))) goto fail;
    const cJSON *compatibility = cJSON_GetObjectItemCaseSensitive(identity, "compatibility");
    const char *input_hash = cJSON_GetObjectItemCaseSensitive(compatibility, "input_usage_sha256")->valuestring;
    if (!replay_hash_matches_descriptors(consumer, compatibility, input_hash)) goto fail;
    consumer->initial_state = (void *)cJSON_GetObjectItemCaseSensitive(root, "initial_state");
    static const char *const initial_state_required[] = {"values", "state_sha256"};
    if (replay_check_object(consumer, (const cJSON *)consumer->initial_state, initial_state_required, 2U, NULL, 0U) != STASIS_REPLAY_OK
        || !replay_hash_text_valid(cJSON_GetObjectItemCaseSensitive((const cJSON *)consumer->initial_state, "state_sha256")->valuestring)
        || !replay_array_length(cJSON_GetObjectItemCaseSensitive((const cJSON *)consumer->initial_state, "values"), STASIS_REPLAY_MAX_TICKS)) goto fail;
    consumer->initial_input = (void *)cJSON_GetObjectItemCaseSensitive(root, "initial_input");
    if (!replay_parse_baseline(consumer, (const cJSON *)consumer->initial_input)) goto fail;
    if (replay_required_number(consumer, root, "total_ticks", 1U, STASIS_REPLAY_MAX_TICKS, &consumer->total_ticks) == NULL) goto fail;
    consumer->segments = (void *)cJSON_GetObjectItemCaseSensitive(root, "segments");
    consumer->checkpoints = (void *)cJSON_GetObjectItemCaseSensitive(root, "checkpoints");
    if (!replay_validate_segments(consumer, (const cJSON *)consumer->segments)) goto fail;
    replay_reset_baseline(consumer);
    if (!replay_validate_checkpoints(consumer, (const cJSON *)consumer->checkpoints, cJSON_GetObjectItemCaseSensitive(root, "final_state"))) goto fail;
    consumer->next_tick = 1U;
    consumer->receipt.result = STASIS_REPLAY_OK;
    return STASIS_REPLAY_OK;
fail:
    {
        int32_t result = consumer->receipt.result == STASIS_REPLAY_OK
            ? STASIS_REPLAY_INVALID_DOCUMENT : consumer->receipt.result;
        StasisReplayReceipt receipt = consumer->receipt;
        receipt.result = (StasisReplayResult)result;
        stasis_replay_consumer_dispose(consumer);
        consumer->receipt = receipt;
        return result;
    }
}

int32_t stasis_replay_consumer_initialize(
    StasisReplayConsumer *consumer,
    const StasisReplayStateOps *state_ops
) {
    if (consumer == NULL || consumer->root == NULL || state_ops == NULL) return STASIS_REPLAY_INVALID_ARGUMENT;
    consumer->state_ops = *state_ops;
    return replay_restore_initial_state(consumer) ? STASIS_REPLAY_OK : consumer->receipt.result;
}

int32_t stasis_replay_consumer_apply_host_frame(
    StasisReplayConsumer *consumer,
    uint64_t tick,
    int32_t *host_i32,
    size_t host_i32_count,
    float *host_f32,
    size_t host_f32_count
) {
    if (consumer == NULL || host_i32 == NULL || host_f32 == NULL) return STASIS_REPLAY_INVALID_ARGUMENT;
    if (!consumer->initialized || host_i32_count != consumer->host_i32_count || host_f32_count != consumer->host_f32_count) {
        return replay_fail(consumer, STASIS_REPLAY_INVALID_ARGUMENT, "input_dimensions", "replay HostFrame dimensions are not initialized");
    }
    if (tick != consumer->next_tick || tick == 0U || tick > consumer->total_ticks) {
        return replay_fail(consumer, STASIS_REPLAY_TICK_SEQUENCE, "tick_sequence", "replay expected tick %llu, found %llu", (unsigned long long)consumer->next_tick, (unsigned long long)tick);
    }
    if (!replay_advance_input(consumer, tick)) return consumer->receipt.result;
    memset(host_i32, 0, host_i32_count * sizeof(*host_i32));
    memset(host_f32, 0, host_f32_count * sizeof(*host_f32));
    const cJSON *compatibility = cJSON_GetObjectItemCaseSensitive(cJSON_GetObjectItemCaseSensitive((const cJSON *)consumer->root, "identity"), "compatibility");
    const cJSON *i32_fields = cJSON_GetObjectItemCaseSensitive(compatibility, "observed_i32");
    const cJSON *f32_fields = cJSON_GetObjectItemCaseSensitive(compatibility, "observed_f32");
    for (int index = 0; index < cJSON_GetArraySize(i32_fields); ++index) {
        const cJSON *field = cJSON_GetArrayItem(i32_fields, index);
        size_t slot = (size_t)cJSON_GetObjectItemCaseSensitive(field, "slot")->valuedouble;
        size_t target = (size_t)cJSON_GetObjectItemCaseSensitive(field, "index")->valuedouble;
        host_i32[target] = consumer->input_i32[slot];
    }
    for (int index = 0; index < cJSON_GetArraySize(f32_fields); ++index) {
        const cJSON *field = cJSON_GetArrayItem(f32_fields, index);
        size_t slot = (size_t)cJSON_GetObjectItemCaseSensitive(field, "slot")->valuedouble;
        size_t target = (size_t)cJSON_GetObjectItemCaseSensitive(field, "index")->valuedouble;
        uint32_t bits = consumer->input_f32_bits[slot];
        memcpy(&host_f32[target], &bits, sizeof(bits));
    }
    consumer->next_tick += 1U;
    consumer->receipt = (StasisReplayReceipt){.result = STASIS_REPLAY_OK, .tick = tick};
    return STASIS_REPLAY_OK;
}

int32_t stasis_replay_consumer_verify_tick(
    StasisReplayConsumer *consumer,
    uint64_t tick
) {
    if (consumer == NULL || consumer->root == NULL || !consumer->initialized) return STASIS_REPLAY_INVALID_ARGUMENT;
    if (tick != consumer->next_tick - 1U || tick == 0U) {
        return replay_fail(consumer, STASIS_REPLAY_TICK_SEQUENCE, "tick_sequence", "replay verification expected tick %llu, found %llu", (unsigned long long)(consumer->next_tick - 1U), (unsigned long long)tick);
    }
    const char *expected = NULL;
    const cJSON *checkpoints = (const cJSON *)consumer->checkpoints;
    if (tick == consumer->total_ticks) {
        const cJSON *final_state = cJSON_GetObjectItemCaseSensitive((const cJSON *)consumer->root, "final_state");
        expected = cJSON_GetObjectItemCaseSensitive(final_state, "state_sha256")->valuestring;
    } else if (consumer->next_checkpoint < (size_t)cJSON_GetArraySize(checkpoints)) {
        const cJSON *checkpoint = cJSON_GetArrayItem(checkpoints, (int)consumer->next_checkpoint);
        if ((uint64_t)cJSON_GetObjectItemCaseSensitive(checkpoint, "tick")->valuedouble == tick) {
            expected = cJSON_GetObjectItemCaseSensitive(checkpoint, "state_sha256")->valuestring;
            consumer->next_checkpoint += 1U;
        }
    }
    if (expected == NULL) {
        consumer->receipt = (StasisReplayReceipt){.result = STASIS_REPLAY_OK, .tick = tick};
        return STASIS_REPLAY_OK;
    }
    size_t bytes = (size_t)consumer->state_descriptor.required_bytes;
    uint8_t *snapshot = bytes == 0U ? NULL : (uint8_t *)malloc(bytes);
    if (bytes != 0U && snapshot == NULL) return replay_fail(consumer, STASIS_REPLAY_BOUNDS, "allocation_failed", "replay verification snapshot allocation failed");
    char actual[65];
    int valid = replay_hash_state(consumer, snapshot, actual);
    if (valid && strcmp(actual, expected) != 0) {
        uint64_t interval_start = consumer->last_verified_tick + 1U;
        consumer->receipt.interval_start = interval_start;
        replay_fail(consumer, STASIS_REPLAY_DIVERGED, "replay_diverged", "replay diverged within ticks %llu..=%llu; detected at checkpoint %llu (expected %.64s, actual %.64s)", (unsigned long long)interval_start, (unsigned long long)tick, (unsigned long long)tick, expected, actual);
        free(snapshot);
        return STASIS_REPLAY_DIVERGED;
    }
    free(snapshot);
    if (!valid) return consumer->receipt.result;
    consumer->last_verified_tick = tick;
    consumer->receipt = (StasisReplayReceipt){.result = STASIS_REPLAY_OK, .tick = tick, .verified = 1U};
    if (tick == consumer->total_ticks) {
        consumer->completed = 1U;
        consumer->receipt.result = STASIS_REPLAY_COMPLETE;
        consumer->receipt.completed = 1U;
        snprintf(consumer->receipt.code, sizeof(consumer->receipt.code), "replay_complete");
    }
    return consumer->receipt.result;
}

int32_t stasis_replay_consumer_is_complete(const StasisReplayConsumer *consumer) {
    return consumer != NULL && consumer->completed ? 1 : 0;
}

const StasisReplayReceipt *stasis_replay_consumer_receipt(const StasisReplayConsumer *consumer) {
    return consumer == NULL ? NULL : &consumer->receipt;
}

void stasis_replay_consumer_dispose(StasisReplayConsumer *consumer) {
    if (consumer == NULL) return;
    if (consumer->root != NULL) cJSON_Delete((cJSON *)consumer->root);
    free(consumer->input_i32);
    free(consumer->input_f32_bits);
    *consumer = (StasisReplayConsumer){0};
}
