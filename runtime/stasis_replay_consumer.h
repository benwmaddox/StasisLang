#ifndef STASIS_REPLAY_CONSUMER_H
#define STASIS_REPLAY_CONSUMER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Schema-v3 replay is intentionally bounded before any guest code runs. */
#define STASIS_REPLAY_SCHEMA_VERSION 3U
#define STASIS_REPLAY_MAX_FILE_BYTES (256U * 1024U * 1024U)
#define STASIS_REPLAY_MAX_TICKS 1000000ULL
#define STASIS_REPLAY_MAX_HOST_VALUES 4096U
#define STASIS_REPLAY_MAX_CHECKPOINTS 4096U
#define STASIS_REPLAY_MAX_TEXT_BYTES 4096U
#define STASIS_REPLAY_MAX_CHANGES_PER_SEGMENT 8192U
#define STASIS_REPLAY_MAX_DIAGNOSTIC_BYTES 512U

typedef enum StasisReplayResult {
    STASIS_REPLAY_OK = 0,
    STASIS_REPLAY_COMPLETE = 1,
    STASIS_REPLAY_INVALID_ARGUMENT = -1,
    STASIS_REPLAY_INVALID_DOCUMENT = -2,
    STASIS_REPLAY_BOUNDS = -3,
    STASIS_REPLAY_DUPLICATE_FIELD = -4,
    STASIS_REPLAY_UNKNOWN_FIELD = -5,
    STASIS_REPLAY_IDENTITY_MISMATCH = -6,
    STASIS_REPLAY_MISSING_STATE_ABI = -7,
    STASIS_REPLAY_STATE_MISMATCH = -8,
    STASIS_REPLAY_DIVERGED = -9,
    STASIS_REPLAY_TICK_SEQUENCE = -10,
    STASIS_REPLAY_IO = -11
} StasisReplayResult;

typedef struct StasisReplayInputDescriptor {
    uint32_t slot;
    uint32_t index;
    const char *path;
    const char *family;
} StasisReplayInputDescriptor;

typedef struct StasisReplayCompatibility {
    const char *stasis_version;
    const char *release_id;
    const char *source_sha256;
    const char *state_layout_sha256;
    const char *compiler_layout_sha256;
    const char *asset_manifest_sha256;
    uint32_t host_schema_version;
    uint32_t host_i32_count;
    uint32_t host_f32_count;
    const char *input_usage_sha256;
    uint32_t tick_rate_hz;
    const char *hash_scope;
    const char *determinism_profile;
    /* Zero means the optional controller schema field is absent. */
    uint32_t controller_schema_version;
    const StasisReplayInputDescriptor *observed_i32;
    size_t observed_i32_count;
    const StasisReplayInputDescriptor *observed_f32;
    size_t observed_f32_count;
} StasisReplayCompatibility;

typedef struct StasisReplayProducer {
    const char *target;
    const char *runtime_sha256;
} StasisReplayProducer;

typedef struct StasisReplayStateEntry {
    const char *kind;
    const char *path;
    const char *field;
    const char *storage_type;
    uint64_t offset;
    uint64_t element_count;
    uint8_t element_bytes;
} StasisReplayStateEntry;

typedef struct StasisReplayStateDescriptor {
    uint64_t required_bytes;
    const StasisReplayStateEntry *entries;
    size_t entry_count;
} StasisReplayStateDescriptor;

typedef int32_t (*StasisReplaySnapshotSize)(void *context);
typedef int32_t (*StasisReplaySnapshotWrite)(void *context, uint8_t *output, int32_t capacity);
typedef int32_t (*StasisReplaySnapshotRestore)(void *context, const uint8_t *input, int32_t bytes);

typedef struct StasisReplayStateOps {
    void *context;
    StasisReplaySnapshotSize size;
    StasisReplaySnapshotWrite write;
    StasisReplaySnapshotRestore restore;
} StasisReplayStateOps;

typedef struct StasisReplayReceipt {
    StasisReplayResult result;
    uint64_t tick;
    uint64_t interval_start;
    uint8_t verified;
    uint8_t completed;
    char code[32];
    char diagnostic[STASIS_REPLAY_MAX_DIAGNOSTIC_BYTES + 1U];
} StasisReplayReceipt;

/* Opaque storage is exposed for stack allocation by C hosts. */
typedef struct StasisReplayConsumer {
    void *root;
    void *segments;
    void *checkpoints;
    void *initial_state;
    void *initial_input;
    uint64_t total_ticks;
    uint64_t next_tick;
    uint64_t segment_cursor;
    size_t segment_index;
    size_t next_checkpoint;
    uint64_t last_verified_tick;
    uint8_t segment_applied;
    uint8_t completed;
    uint8_t initialized;
    uint32_t host_i32_count;
    uint32_t host_f32_count;
    int32_t *input_i32;
    uint32_t *input_f32_bits;
    size_t input_i32_count;
    size_t input_f32_count;
    StasisReplayCompatibility expected;
    StasisReplayStateDescriptor state_descriptor;
    StasisReplayStateOps state_ops;
    StasisReplayReceipt receipt;
} StasisReplayConsumer;

/* Initialize stack storage before the first load; dispose before reusing it. */
void stasis_replay_consumer_init(StasisReplayConsumer *consumer);

/* Parse, strictly validate, and identity-check a bounded schema-v3 document. */
int32_t stasis_replay_consumer_load(
    StasisReplayConsumer *consumer,
    const uint8_t *bytes,
    size_t byte_count,
    const StasisReplayCompatibility *expected,
    const StasisReplayStateDescriptor *state_descriptor
);

/* Restore the sparse initial state after guest main() and verify tick zero. */
int32_t stasis_replay_consumer_initialize(
    StasisReplayConsumer *consumer,
    const StasisReplayStateOps *state_ops
);

/* Project one replay tick into HostFrame; physical input must not be read by the caller. */
int32_t stasis_replay_consumer_apply_host_frame(
    StasisReplayConsumer *consumer,
    uint64_t tick,
    int32_t *host_i32,
    size_t host_i32_count,
    float *host_f32,
    size_t host_f32_count
);

/* Verify the canonical post-tick state before render construction. */
int32_t stasis_replay_consumer_verify_tick(
    StasisReplayConsumer *consumer,
    uint64_t tick
);

int32_t stasis_replay_consumer_is_complete(const StasisReplayConsumer *consumer);
const StasisReplayReceipt *stasis_replay_consumer_receipt(const StasisReplayConsumer *consumer);
void stasis_replay_consumer_dispose(StasisReplayConsumer *consumer);

#ifdef __cplusplus
}
#endif

#endif
