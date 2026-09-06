#ifndef STASIS_NETWORK_H
#define STASIS_NETWORK_H

#include <stddef.h>
#include <stdint.h>

#define STASIS_NETWORK_ABI_VERSION 1u
#define STASIS_NETWORK_CLIENT_ABI_VERSION 1u
#define STASIS_NETWORK_MAX_MESSAGE_BYTES (64u * 1024u)
#define STASIS_NETWORK_ADVERTISE_IPV4_ENV "STASIS_NETWORK_ADVERTISE_IPV4"

typedef struct stasis_network_host stasis_network_host;
typedef struct stasis_network_client stasis_network_client;
typedef struct stasis_network_event {
    uint32_t kind;
    uint32_t connection;
    uint32_t length;
    unsigned char payload[STASIS_NETWORK_MAX_MESSAGE_BYTES];
} stasis_network_event;

int32_t stasis_realtime_start(int32_t simulation_hz, int32_t presentation_hz,
    int32_t control_hz, int32_t input_delay_ticks, int32_t seats);
int32_t stasis_realtime_stop(void);
int32_t stasis_realtime_submit_payload(const int32_t *payload, int32_t length);
int32_t stasis_realtime_build_payload(int32_t *out_payload, int32_t capacity,
    int32_t seat, int32_t epoch, int32_t sequence, int32_t apply_tick,
    int32_t buttons, int32_t axis_x, int32_t axis_y);
int32_t stasis_realtime_resync_required(void);
int32_t stasis_realtime_record_hash(int32_t tick, int32_t hash_low, int32_t hash_high);
int32_t stasis_realtime_apply_snapshot(int32_t revision, int32_t tick, int32_t seat_count,
    const int32_t *buttons, const int32_t *axis_x, const int32_t *axis_y,
    const int32_t *sequences, const int32_t *epochs, const int32_t *active);
int32_t stasis_realtime_current_tick(void);
int32_t stasis_realtime_current_epoch(int32_t seat);
int32_t stasis_realtime_schedule(int32_t seat, int32_t epoch, int32_t sequence,
    int32_t apply_tick, int32_t buttons, int32_t axis_x, int32_t axis_y);
int32_t stasis_realtime_advance(void);
int32_t stasis_realtime_read_control(int32_t seat, int32_t *out_buttons,
    int32_t *out_axis_x, int32_t *out_axis_y);
int32_t stasis_realtime_disconnect(int32_t seat);
int32_t stasis_realtime_reconnect(int32_t seat);
int32_t stasis_realtime_pause(void);
int32_t stasis_realtime_focus_lost(void);
int32_t stasis_realtime_rematch(void);

uint32_t stasis_network_abi_version(void);
const char *stasis_network_release_id(void);
int32_t stasis_network_supported(void);
int32_t stasis_network_random_seed(void);
stasis_network_host *stasis_network_host_start_bind(uint16_t port, uint32_t bind_ipv4,
    const unsigned char *bundle, size_t bundle_length, uint16_t *out_port);
int32_t stasis_network_host_poll(stasis_network_host *host, stasis_network_event *event);
int32_t stasis_network_host_send(stasis_network_host *host, uint32_t connection,
    const unsigned char *payload, size_t length);
int32_t stasis_network_host_status(stasis_network_host *host);
uint32_t stasis_network_host_overflow_count(stasis_network_host *host);
uint16_t stasis_network_host_port(stasis_network_host *host);
int32_t stasis_network_host_copy_join_url(stasis_network_host *host, char *out,
    size_t capacity, size_t *out_length);
/* Display-safe URL without pairing or resume credentials. */
int32_t stasis_network_host_copy_join_card(stasis_network_host *host, char *out,
    size_t capacity, size_t *out_length);
void stasis_network_host_stop(stasis_network_host *host);

#define STASIS_NETWORK_CLIENT_STATUS_DISCONNECTED 0
#define STASIS_NETWORK_CLIENT_STATUS_CONNECTED 1
#define STASIS_NETWORK_CLIENT_STATUS_CONNECTING 2

uint32_t stasis_network_client_abi_version(void);
/* Client calls are thread-safe. The owner must serialize destroy against all
 * other calls and keep each caller buffer valid for its declared length.
 * Create returns NULL on failure. Integer results use -1 invalid argument,
 * -2 transport/disconnected, -3 bounded queue
 * full, and -4 invalid credentials or protocol. A -4 status is terminal until
 * an explicit connect call. */
stasis_network_client *stasis_network_client_create(const char *join_url, size_t length);
int32_t stasis_network_client_connect(stasis_network_client *client);
int32_t stasis_network_client_disconnect(stasis_network_client *client);
int32_t stasis_network_client_set_background(stasis_network_client *client, int32_t background);
/* Retry backoff reports CONNECTING; background suspension reports DISCONNECTED. */
int32_t stasis_network_client_status(stasis_network_client *client);
/* Returns bytes copied, zero when idle, or a negative error. An undersized output
 * returns -1 and preserves the queued message for a later poll. */
int32_t stasis_network_client_poll(stasis_network_client *client, unsigned char *out,
    size_t capacity);
int32_t stasis_network_client_send(stasis_network_client *client,
    const unsigned char *payload, size_t length);
int32_t stasis_network_client_checkpoint(stasis_network_client *client, int32_t seat,
    int32_t sequence);
int32_t stasis_network_client_resume_seat(stasis_network_client *client);
int32_t stasis_network_client_last_sequence(stasis_network_client *client);
void stasis_network_client_destroy(stasis_network_client *client);

#endif
