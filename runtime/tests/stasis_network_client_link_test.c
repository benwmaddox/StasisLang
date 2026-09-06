#include "stasis_network.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#if defined(_WIN32)
#include <windows.h>
#else
#include <time.h>
#endif

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "client check failed at line %d\n", __LINE__); \
        exit(1); \
    } \
} while (0)

static void pause_poll(void) {
#if defined(_WIN32)
    Sleep(5);
#else
    const struct timespec delay = {0, 5000000};
    nanosleep(&delay, NULL);
#endif
}

static stasis_network_event event;

static uint32_t wait_host(stasis_network_host *host, uint32_t kind) {
    for (int attempt = 0; attempt < 1000; ++attempt) {
        int32_t result = stasis_network_host_poll(host, &event);
        CHECK(result >= 0);
        if (result > 0 && event.kind == kind) return event.connection;
        pause_poll();
    }
    CHECK(0);
    return 0;
}

static void receive(stasis_network_client *client, const char *expected) {
    unsigned char payload[256];
    for (int attempt = 0; attempt < 1000; ++attempt) {
        int32_t result = stasis_network_client_poll(client, payload, sizeof(payload));
        if (result > 0) {
            CHECK((size_t)result == strlen(expected));
            CHECK(memcmp(payload, expected, (size_t)result) == 0);
            return;
        }
        pause_poll();
    }
    CHECK(0);
}

int main(void) {
    static const unsigned char bundle[] = {
        'S', 'G', 'B', '1', 0, 1, 0, 1,
        0, 10, 0, 9, 0, 0, 0, 4,
        'i', 'n', 'd', 'e', 'x', '.', 'h', 't', 'm', 'l',
        't', 'e', 'x', 't', '/', 'h', 't', 'm', 'l',
        '<', 'p', '/', '>'
    };
    static const char ack[] = "{\"kind\":\"join_ack\",\"seat\":0}";
    static const char command[] = "{\"kind\":\"guest_command\",\"command\":\"move\",\"sequence\":1}";
    static const char snapshot[] = "{\"kind\":\"snapshot\",\"seat\":0,\"tick\":42}";
    static const char other_snapshot[] = "{\"kind\":\"snapshot\",\"seat\":1,\"tick\":42}";
    uint16_t port = 0;
    char private_url[512];
    size_t length = 0;
    CHECK(stasis_network_client_abi_version() == STASIS_NETWORK_CLIENT_ABI_VERSION);
    stasis_network_host *host = stasis_network_host_start_bind(
        0, 0x7f000001u, bundle, sizeof(bundle), &port);
    CHECK(host != NULL && port != 0);
    CHECK(stasis_network_host_copy_join_url(host, private_url, sizeof(private_url), &length) == 0);
    stasis_network_client *client = stasis_network_client_create(private_url, length);
    stasis_network_client *other = stasis_network_client_create(private_url, length);
    memset(private_url, 0, sizeof(private_url));
    CHECK(client != NULL && other != NULL);
    CHECK(stasis_network_client_connect(client) == 0);
    uint32_t seat0 = wait_host(host, 1);
    CHECK(stasis_network_host_send(host, seat0, (const unsigned char *)ack, sizeof(ack) - 1) == 0);
    receive(client, ack);
    CHECK(stasis_network_client_send(client, (const unsigned char *)command, sizeof(command) - 1) == 0);
    CHECK(wait_host(host, 3) == seat0);
    CHECK(event.length == sizeof(command) - 1);
    CHECK(memcmp(event.payload, command, sizeof(command) - 1) == 0);

    CHECK(stasis_network_client_connect(other) == 0);
    uint32_t seat1 = wait_host(host, 1);
    CHECK(seat1 != seat0);
    CHECK(stasis_network_host_send(host, seat0, (const unsigned char *)snapshot, sizeof(snapshot) - 1) == 0);
    CHECK(stasis_network_host_send(host, seat1, (const unsigned char *)other_snapshot, sizeof(other_snapshot) - 1) == 0);
    receive(client, snapshot);
    receive(other, other_snapshot);
    CHECK(stasis_network_client_checkpoint(client, 0, 1) == 0);
    CHECK(stasis_network_client_set_background(client, 1) == 0);
    CHECK(wait_host(host, 4) == seat0);
    CHECK(stasis_network_client_set_background(client, 0) == 0);
    CHECK(wait_host(host, 1) == seat0);
    CHECK(stasis_network_client_resume_seat(client) == 0);
    CHECK(stasis_network_client_last_sequence(client) == 1);
    CHECK(stasis_network_host_send(host, seat0, (const unsigned char *)snapshot, sizeof(snapshot) - 1) == 0);
    receive(client, snapshot);
    CHECK(stasis_network_client_disconnect(client) == 0);
    CHECK(wait_host(host, 4) == seat0);
    stasis_network_client_destroy(client);
    stasis_network_client_destroy(other);
    stasis_network_host_stop(host);
    puts("native client ABI: join, command, per-seat snapshot, background resume passed");
    return 0;
}
