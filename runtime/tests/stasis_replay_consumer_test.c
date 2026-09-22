#include "stasis_replay_consumer.h"

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *fixture =
    "{\"schema_version\":3,\"identity\":{\"compatibility\":{"
    "\"stasis_version\":\"0.1.0\",\"release_id\":\"development\","
    "\"source_sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\","
    "\"state_layout_sha256\":\"1111111111111111111111111111111111111111111111111111111111111111\","
    "\"compiler_layout_sha256\":\"2222222222222222222222222222222222222222222222222222222222222222\","
    "\"asset_manifest_sha256\":null,\"host_schema_version\":4,\"host_i32_count\":1,"
    "\"host_f32_count\":1,\"input_usage_sha256\":\"056929fcb5907821547caaecfc192cd116379ae13730bf60739c5def076fc8f0\","
    "\"tick_rate_hz\":60,\"hash_scope\":\"simulation_after_tick\","
    "\"determinism_profile\":\"input_only_no_external_observations\","
    "\"controller_schema_version\":null,\"observed_i32\":[],\"observed_f32\":[]},"
    "\"producer\":{\"target\":\"jit-test\",\"runtime_sha256\":\"3333333333333333333333333333333333333333333333333333333333333333\"}},"
    "\"initial_state\":{\"values\":[],\"state_sha256\":\"1509bbe5b0b410b7a03af43d8fb37b6ec5f22b11e620e37ec126e46cdab18e64\"},"
    "\"initial_input\":{\"i32_values\":[],\"f32_bits\":[]},"
    "\"segments\":[{\"tick_gap\":0,\"run_ticks\":1,\"i32_changes\":[],\"f32_changes\":[]}],"
    "\"checkpoints\":[],\"total_ticks\":1,"
    "\"final_state\":{\"tick\":1,\"state_sha256\":\"1509bbe5b0b410b7a03af43d8fb37b6ec5f22b11e620e37ec126e46cdab18e64\"}}";

static const char *nonempty_fixture =
    "{\"schema_version\":3,\"identity\":{\"compatibility\":{"
    "\"stasis_version\":\"0.1.0\",\"release_id\":\"development\","
    "\"source_sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\","
    "\"state_layout_sha256\":\"1111111111111111111111111111111111111111111111111111111111111111\","
    "\"compiler_layout_sha256\":\"2222222222222222222222222222222222222222222222222222222222222222\","
    "\"asset_manifest_sha256\":null,\"host_schema_version\":4,\"host_i32_count\":3,"
    "\"host_f32_count\":3,\"input_usage_sha256\":\"fc21c652cb8d5c6800c8393e644cab27ac081bcdc885cd39477db71b3790f3f0\","
    "\"tick_rate_hz\":60,\"hash_scope\":\"simulation_after_tick\","
    "\"determinism_profile\":\"input_only_no_external_observations\","
    "\"controller_schema_version\":null,"
    "\"observed_i32\":[{\"slot\":0,\"index\":1,\"path\":\"input.i32\",\"family\":\"raw\"}],"
    "\"observed_f32\":[{\"slot\":0,\"index\":2,\"path\":\"input.f32\",\"family\":\"raw\"}]},"
    "\"producer\":{\"target\":\"jit-test\",\"runtime_sha256\":\"3333333333333333333333333333333333333333333333333333333333333333\"}},"
    "\"initial_state\":{\"values\":[{\"location\":{\"kind\":\"scalar\",\"path\":\"z\"},"
    "\"value\":{\"type_name\":\"i32\",\"bits\":\"0000002a\"}},"
    "{\"location\":{\"kind\":\"collection\",\"path\":\"a\",\"field\":\"value\",\"index\":0},"
    "\"value\":{\"type_name\":\"i32\",\"bits\":\"00000007\"}}],"
    "\"state_sha256\":\"cea6ddbb15f5284d09b3eb704f3f7a9cfe2f5b5f80c8a8c218a0281e49df3dd8\"},"
    "\"initial_input\":{\"i32_values\":[11],\"f32_bits\":[2143289345]},"
    "\"segments\":["
    "{\"tick_gap\":0,\"run_ticks\":2,\"i32_changes\":[{\"slot\":0,\"value\":20}],"
    "\"f32_changes\":[{\"slot\":0,\"bits\":1065353216}]},"
    "{\"tick_gap\":1,\"run_ticks\":2,\"i32_changes\":[{\"slot\":0,\"value\":99}],"
    "\"f32_changes\":[{\"slot\":0,\"bits\":1}]},"
    "{\"tick_gap\":0,\"run_ticks\":507,\"i32_changes\":[],\"f32_changes\":[]}],"
    "\"checkpoints\":["
    "{\"tick\":256,\"state_sha256\":\"cea6ddbb15f5284d09b3eb704f3f7a9cfe2f5b5f80c8a8c218a0281e49df3dd8\"},"
    "{\"tick\":512,\"state_sha256\":\"cea6ddbb15f5284d09b3eb704f3f7a9cfe2f5b5f80c8a8c218a0281e49df3dd8\"}],"
    "\"total_ticks\":512,"
    "\"final_state\":{\"tick\":512,\"state_sha256\":\"cea6ddbb15f5284d09b3eb704f3f7a9cfe2f5b5f80c8a8c218a0281e49df3dd8\"}}";

static int32_t snapshot_size(void *context) {
    (void)context;
    return 0;
}

static int32_t snapshot_write(void *context, uint8_t *output, int32_t capacity) {
    (void)context;
    (void)output;
    return capacity == 0 ? 0 : -1;
}

static int32_t snapshot_restore(void *context, const uint8_t *input, int32_t bytes) {
    (void)context;
    (void)input;
    return bytes == 0 ? 0 : -1;
}

static StasisReplayCompatibility expected_identity(void) {
    return (StasisReplayCompatibility){
        .stasis_version = "0.1.0",
        .release_id = "development",
        .source_sha256 = "0000000000000000000000000000000000000000000000000000000000000000",
        .state_layout_sha256 = "1111111111111111111111111111111111111111111111111111111111111111",
        .compiler_layout_sha256 = "2222222222222222222222222222222222222222222222222222222222222222",
        .host_schema_version = 4,
        .host_i32_count = 1,
        .host_f32_count = 1,
        .input_usage_sha256 = "056929fcb5907821547caaecfc192cd116379ae13730bf60739c5def076fc8f0",
        .tick_rate_hz = 60,
        .hash_scope = "simulation_after_tick",
        .determinism_profile = "input_only_no_external_observations",
    };
}

static StasisReplayStateOps state_ops(void) {
    return (StasisReplayStateOps){
        .size = snapshot_size,
        .write = snapshot_write,
        .restore = snapshot_restore,
    };
}

static uint8_t nonempty_state[8] = {42, 0, 0, 0, 7, 0, 0, 0};

static int32_t nonempty_snapshot_size(void *context) {
    (void)context;
    return 8;
}

static int32_t nonempty_snapshot_write(void *context, uint8_t *output, int32_t capacity) {
    (void)context;
    if (capacity != 8 || output == NULL) return -1;
    memcpy(output, nonempty_state, sizeof(nonempty_state));
    return 8;
}

static int32_t nonempty_snapshot_restore(void *context, const uint8_t *input, int32_t bytes) {
    (void)context;
    if (bytes != 8 || input == NULL) return -1;
    memcpy(nonempty_state, input, sizeof(nonempty_state));
    return 8;
}


static void test_fixture_completes(void) {
    StasisReplayConsumer consumer;
    stasis_replay_consumer_init(&consumer);
    StasisReplayCompatibility expected = expected_identity();
    StasisReplayStateDescriptor descriptor = {0};
    int32_t load_result = stasis_replay_consumer_load(
        &consumer, (const uint8_t *)fixture, strlen(fixture), &expected, &descriptor);
    assert(load_result == STASIS_REPLAY_OK);
    StasisReplayStateOps ops = state_ops();
    assert(stasis_replay_consumer_initialize(&consumer, &ops) == STASIS_REPLAY_OK);
    int32_t host_i32[1] = {99};
    float host_f32[1] = {99.0f};
    assert(stasis_replay_consumer_apply_host_frame(&consumer, 1, host_i32, 1, host_f32, 1) == STASIS_REPLAY_OK);
    assert(host_i32[0] == 0 && host_f32[0] == 0.0f);
    assert(stasis_replay_consumer_verify_tick(&consumer, 1) == STASIS_REPLAY_COMPLETE);
    assert(stasis_replay_consumer_is_complete(&consumer));
    assert(strcmp(stasis_replay_consumer_receipt(&consumer)->code, "replay_complete") == 0);
    stasis_replay_consumer_dispose(&consumer);

}

static void test_duplicate_and_identity_rejected(void) {
    StasisReplayCompatibility expected = expected_identity();
    StasisReplayStateDescriptor descriptor = {0};
    char *duplicate = (char *)malloc(strlen(fixture) + 32U);
    assert(duplicate != NULL);
    memcpy(duplicate, fixture, strlen(fixture) + 1U);
    char *schema = strstr(duplicate, "\"schema_version\":3");
    assert(schema != NULL);
    memmove(schema + 37, schema + 18, strlen(schema + 18) + 1U);
    memcpy(schema + 18, ",\"schema_version\":3", 19U);
    StasisReplayConsumer consumer;
    stasis_replay_consumer_init(&consumer);
    assert(stasis_replay_consumer_load(
        &consumer, (const uint8_t *)duplicate, strlen(duplicate), &expected, &descriptor)
        == STASIS_REPLAY_DUPLICATE_FIELD);
    free(duplicate);

    expected.release_id = "other";
    assert(stasis_replay_consumer_load(
        &consumer, (const uint8_t *)fixture, strlen(fixture), &expected, &descriptor)
        == STASIS_REPLAY_IDENTITY_MISMATCH);
    stasis_replay_consumer_dispose(&consumer);
}

static void test_corrupted_final_state_diverges(void) {
    StasisReplayCompatibility expected = expected_identity();
    StasisReplayStateDescriptor descriptor = {0};
    char *corrupt = (char *)malloc(strlen(fixture) + 1U);
    assert(corrupt != NULL);
    memcpy(corrupt, fixture, strlen(fixture) + 1U);
    char *final_hash = strrchr(corrupt, '3');
    assert(final_hash != NULL);
    *final_hash = '4';
    StasisReplayConsumer consumer;
    stasis_replay_consumer_init(&consumer);
    assert(stasis_replay_consumer_load(
        &consumer, (const uint8_t *)corrupt, strlen(corrupt), &expected, &descriptor) == STASIS_REPLAY_OK);
    StasisReplayStateOps ops = state_ops();
    assert(stasis_replay_consumer_initialize(&consumer, &ops) == STASIS_REPLAY_OK);
    int32_t host_i32[1] = {0};
    float host_f32[1] = {0.0f};
    assert(stasis_replay_consumer_apply_host_frame(&consumer, 1, host_i32, 1, host_f32, 1) == STASIS_REPLAY_OK);
    assert(stasis_replay_consumer_verify_tick(&consumer, 1) == STASIS_REPLAY_DIVERGED);
    stasis_replay_consumer_dispose(&consumer);
    free(corrupt);
}

static void test_nonempty_rle_checkpoint_and_descriptor_order(void) {
    static const StasisReplayStateEntry entries[] = {
        {"scalar", "z", "", "i32", 0, 1, 4},
        {"collection", "a", "value", "i32", 4, 1, 4},
    };
    static const StasisReplayInputDescriptor observed_i32[] = {
        {0, 1, "input.i32", "raw"},
    };
    static const StasisReplayInputDescriptor observed_f32[] = {
        {0, 2, "input.f32", "raw"},
    };
    StasisReplayCompatibility expected = expected_identity();
    expected.host_i32_count = 3;
    expected.host_f32_count = 3;
    expected.input_usage_sha256 = "fc21c652cb8d5c6800c8393e644cab27ac081bcdc885cd39477db71b3790f3f0";
    expected.observed_i32 = observed_i32;
    expected.observed_i32_count = 1;
    expected.observed_f32 = observed_f32;
    expected.observed_f32_count = 1;
    StasisReplayStateDescriptor descriptor = {
        .required_bytes = 8,
        .entries = entries,
        .entry_count = 2,
    };
    StasisReplayConsumer consumer;
    stasis_replay_consumer_init(&consumer);
    int32_t load_result = stasis_replay_consumer_load(
        &consumer,
        (const uint8_t *)nonempty_fixture,
        strlen(nonempty_fixture),
        &expected,
        &descriptor);
    assert(load_result == STASIS_REPLAY_OK);
    StasisReplayStateOps ops = {
        .size = nonempty_snapshot_size,
        .write = nonempty_snapshot_write,
        .restore = nonempty_snapshot_restore,
    };
    nonempty_state[0] = 0;
    assert(stasis_replay_consumer_initialize(&consumer, &ops) == STASIS_REPLAY_OK);
    assert(nonempty_state[0] == 42 && nonempty_state[4] == 7);
    int32_t host_i32[3] = {77, 88, 99};
    float host_f32[3] = {4.0f, 5.0f, 6.0f};
    for (uint64_t tick = 1; tick <= 512; ++tick) {
        assert(stasis_replay_consumer_apply_host_frame(
            &consumer, tick, host_i32, 3, host_f32, 3) == STASIS_REPLAY_OK);
        if (tick == 1) {
            assert(host_i32[0] == 0 && host_i32[1] == 20 && host_i32[2] == 0);
            assert(host_f32[0] == 0.0f && host_f32[2] == 1.0f);
        } else if (tick == 3) {
            assert(host_i32[1] == 20);
        } else if (tick == 4) {
            assert(host_i32[1] == 99);
        }
        int32_t result = stasis_replay_consumer_verify_tick(&consumer, tick);
        assert(result == (tick == 512 ? STASIS_REPLAY_COMPLETE : STASIS_REPLAY_OK));
    }
    assert(stasis_replay_consumer_is_complete(&consumer));
    assert(strcmp(stasis_replay_consumer_receipt(&consumer)->code, "replay_complete") == 0);
    stasis_replay_consumer_dispose(&consumer);

    const char *state_hash =
        "cea6ddbb15f5284d09b3eb704f3f7a9cfe2f5b5f80c8a8c218a0281e49df3dd8";
    char *corrupt = (char *)malloc(strlen(nonempty_fixture) + 1U);
    assert(corrupt != NULL);
    memcpy(corrupt, nonempty_fixture, strlen(nonempty_fixture) + 1U);
    char *cursor = corrupt;
    size_t hash_occurrences = 0U;
    while ((cursor = strstr(cursor, state_hash)) != NULL) {
        hash_occurrences += 1U;
        if (hash_occurrences >= 3U) cursor[strlen(state_hash) - 1U] = '9';
        cursor += strlen(state_hash);
    }
    assert(hash_occurrences == 4U);
    StasisReplayConsumer divergent;
    stasis_replay_consumer_init(&divergent);
    assert(stasis_replay_consumer_load(
        &divergent,
        (const uint8_t *)corrupt,
        strlen(corrupt),
        &expected,
        &descriptor) == STASIS_REPLAY_OK);
    nonempty_state[0] = 0;
    assert(stasis_replay_consumer_initialize(&divergent, &ops) == STASIS_REPLAY_OK);
    for (uint64_t tick = 1; tick <= 512; ++tick) {
        assert(stasis_replay_consumer_apply_host_frame(
            &divergent, tick, host_i32, 3, host_f32, 3) == STASIS_REPLAY_OK);
        int32_t result = stasis_replay_consumer_verify_tick(&divergent, tick);
        assert(result == (tick == 512 ? STASIS_REPLAY_DIVERGED : STASIS_REPLAY_OK));
    }
    assert(strcmp(stasis_replay_consumer_receipt(&divergent)->code, "replay_diverged") == 0);
    assert(stasis_replay_consumer_receipt(&divergent)->interval_start == 257U);
    stasis_replay_consumer_dispose(&divergent);
    free(corrupt);
}

int main(void) {
    test_fixture_completes();
    test_duplicate_and_identity_rejected();
    test_corrupted_final_state_diverges();
    test_nonempty_rle_checkpoint_and_descriptor_order();
    puts("stasis_replay_consumer_test: ok");
    return 0;
}
