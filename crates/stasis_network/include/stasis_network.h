#ifndef STASIS_NETWORK_H
#define STASIS_NETWORK_H

#include <stddef.h>
#include <stdint.h>

#define STASIS_NETWORK_ABI_VERSION 1u
#define STASIS_NETWORK_MAX_MESSAGE_BYTES (64u * 1024u)

typedef struct stasis_network_host stasis_network_host;
typedef struct stasis_network_event {
    uint32_t kind;
    uint32_t connection;
    uint32_t length;
    unsigned char payload[STASIS_NETWORK_MAX_MESSAGE_BYTES];
} stasis_network_event;

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
void stasis_network_host_stop(stasis_network_host *host);

#endif
