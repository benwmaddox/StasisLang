#include "stasis_network.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

int main(void) {
    static const unsigned char bundle[] = {
        'S', 'G', 'B', '1', 0, 1, 0, 1,
        0, 10, 0, 9, 0, 0, 0, 4,
        'i', 'n', 'd', 'e', 'x', '.', 'h', 't', 'm', 'l',
        't', 'e', 'x', 't', '/', 'h', 't', 'm', 'l',
        '<', 'p', '/', '>'
    };
    static const unsigned char invalid_bundle[] = {'b', 'a', 'd'};
    char join_card[512];
    size_t join_card_length = 0;
    uint16_t port = 0;

    CHECK(stasis_network_abi_version() == STASIS_NETWORK_ABI_VERSION);
    CHECK(stasis_network_supported() == 1);
    CHECK(stasis_network_host_start_bind(
        0, 0x7f000001u, invalid_bundle, sizeof(invalid_bundle), &port) == NULL);

    stasis_network_host *host = stasis_network_host_start_bind(
        0, 0x7f000001u, bundle, sizeof(bundle), &port);
    CHECK(host != NULL);
    CHECK(port != 0);
    CHECK(stasis_network_host_port(host) == port);
    CHECK(stasis_network_host_copy_join_card(
        host, join_card, sizeof(join_card), &join_card_length) == 0);
    CHECK(join_card_length == strlen(join_card));
    CHECK(strstr(join_card, "http://") == join_card);
    CHECK(strstr(join_card, "pair=") == NULL);
    CHECK(strstr(join_card, "secret=") == NULL);
    CHECK(strstr(join_card, "resume=") == NULL);
    CHECK(strchr(join_card, '#') == NULL);
    CHECK(stasis_network_host_copy_join_card(host, join_card, 1, &join_card_length) == -2);
    stasis_network_host_stop(host);

    puts("stasis_network native static-link contract passed");
    return 0;
}
