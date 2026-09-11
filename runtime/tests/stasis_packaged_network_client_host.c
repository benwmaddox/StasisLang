#include "stasis_network.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "packaged client host check failed at line %d\n", __LINE__); \
        exit(1); \
    } \
} while (0)

static const unsigned char bundle[] = {
    'S', 'G', 'B', '1', 0, 1, 0, 1,
    0, 10, 0, 9, 0, 0, 0, 4,
    'i', 'n', 'd', 'e', 'x', '.', 'h', 't', 'm', 'l',
    't', 'e', 'x', 't', '/', 'h', 't', 'm', 'l',
    '<', 'p', '/', '>'
};

int main(int argc, char **argv) {
    static const unsigned char ping[] = {'P', 'I', 'N', 'G'};
    static const unsigned char pong[] = {'P', 'O', 'N', 'G'};
    static const unsigned char done[] = {'D', 'O', 'N', 'E'};
    static const unsigned char bye[] = {'B', 'Y', 'E', '!'};
    stasis_network_host *host;
    stasis_network_event event;
    char private_url[512];
    size_t private_url_length = 0;
    uint16_t port = 0;
    uint32_t connection = 0;
    int received_ping = 0;
    int received_done = 0;
    int disconnected = 0;
    FILE *join_file;

    CHECK(argc == 2);
    host = stasis_network_host_start_bind(0, 0x7f000001u, bundle, sizeof(bundle), &port);
    CHECK(host != NULL && port != 0);
    CHECK(stasis_network_host_copy_join_url(
        host, private_url, sizeof(private_url), &private_url_length) == 0);
    CHECK(fopen_s(&join_file, argv[1], "wb") == 0 && join_file != NULL);
    CHECK(fwrite(private_url, 1, private_url_length, join_file) == private_url_length);
    CHECK(fclose(join_file) == 0);
    memset(private_url, 0, sizeof(private_url));

    for (int attempt = 0; attempt < 3000 && !disconnected; ++attempt) {
        int32_t result = stasis_network_host_poll(host, &event);
        CHECK(result >= 0);
        if (result == 0) {
            Sleep(5);
            continue;
        }
        if (event.kind == 1) {
            connection = event.connection;
        } else if (event.kind == 3) {
            CHECK(event.connection == connection && event.length == 4);
            if (!received_ping) {
                CHECK(memcmp(event.payload, ping, sizeof(ping)) == 0);
                CHECK(stasis_network_host_send(host, connection, pong, sizeof(pong)) == 0);
                received_ping = 1;
            } else {
                CHECK(memcmp(event.payload, done, sizeof(done)) == 0);
                CHECK(stasis_network_host_send(host, connection, bye, sizeof(bye)) == 0);
                received_done = 1;
            }
        } else if (event.kind == 4) {
            CHECK(event.connection == connection);
            disconnected = received_done;
        }
    }
    CHECK(received_ping && received_done && disconnected);
    stasis_network_host_stop(host);
    puts("packaged native client: join, send/poll, checkpoint/resume, shutdown passed");
    return 0;
}
