#ifndef STASIS_NETWORK_JOIN_CARD_H
#define STASIS_NETWORK_JOIN_CARD_H

#include <stddef.h>
#include <stdint.h>

#define STASIS_NETWORK_JOIN_URL_CAPACITY 2048u

typedef int32_t (*StasisNetworkCopyJoinUrlFn)(char *out, size_t capacity);
typedef int (*StasisNetworkClipboardWriteFn)(const char *text);

/*
 * Copies the private join URL only after the native shell receives an explicit
 * copy action. The URL never enters guest state or visible join-card text, and
 * the temporary native buffer is erased before returning.
 */
static inline int stasis_network_copy_private_join_url(
    StasisNetworkCopyJoinUrlFn copy_join_url,
    StasisNetworkClipboardWriteFn write_clipboard
) {
    char url[STASIS_NETWORK_JOIN_URL_CAPACITY] = {0};
    int32_t length;
    int result = 0;

    if (copy_join_url == NULL || write_clipboard == NULL) {
        return 0;
    }
    length = copy_join_url(url, sizeof(url));
    if (length > 0 && (size_t)length < sizeof(url) && url[length] == '\0') {
        result = write_clipboard(url) != 0;
    }
    volatile char *wipe = url;
    for (size_t index = 0; index < sizeof(url); index += 1) {
        wipe[index] = '\0';
    }
    return result;
}

#endif
