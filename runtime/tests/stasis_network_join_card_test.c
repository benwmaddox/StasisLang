#include "stasis_network_join_card.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

static const char private_url[] =
    "http://192.0.2.20:4312/session?pair=private-fixture-secret";
static char clipboard[256];
static int copy_calls;
static int clipboard_calls;

static int32_t copy_join_url(char *out, size_t capacity) {
    size_t length = strlen(private_url);
    copy_calls += 1;
    CHECK(capacity > length);
    memcpy(out, private_url, length + 1);
    return (int32_t)length;
}

static int write_clipboard(const char *text) {
    clipboard_calls += 1;
    snprintf(clipboard, sizeof(clipboard), "%s", text);
    return 1;
}

static int32_t reject_join_url(char *out, size_t capacity) {
    copy_calls += 1;
    if (capacity > 0) out[0] = '\0';
    return -1;
}

int main(void) {
    CHECK(stasis_network_copy_private_join_url(NULL, write_clipboard) == 0);
    CHECK(copy_calls == 0 && clipboard_calls == 0);

    CHECK(stasis_network_copy_private_join_url(copy_join_url, write_clipboard) == 1);
    CHECK(copy_calls == 1 && clipboard_calls == 1);
    CHECK(strcmp(clipboard, private_url) == 0);

    clipboard[0] = '\0';
    CHECK(stasis_network_copy_private_join_url(reject_join_url, write_clipboard) == 0);
    CHECK(copy_calls == 2 && clipboard_calls == 1);
    CHECK(clipboard[0] == '\0');
    puts("stasis_network_join_card_test: ok");
    return 0;
}
