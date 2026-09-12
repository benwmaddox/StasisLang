#include "stasis_platform_services.h"

#include <stddef.h>
#include <string.h>

#if defined(_WIN32)
#include <windows.h>
static SRWLOCK service_lock = SRWLOCK_INIT;
#define SERVICE_LOCK() AcquireSRWLockExclusive(&service_lock)
#define SERVICE_UNLOCK() ReleaseSRWLockExclusive(&service_lock)
#else
#include <pthread.h>
static pthread_mutex_t service_lock = PTHREAD_MUTEX_INITIALIZER;
#define SERVICE_LOCK() pthread_mutex_lock(&service_lock)
#define SERVICE_UNLOCK() pthread_mutex_unlock(&service_lock)
#endif

typedef struct StasisPlatformServiceState {
    StasisPlatformServiceRequest pending[STASIS_PLATFORM_SERVICE_QUEUE_CAPACITY];
    StasisPlatformServiceResponse responses[STASIS_PLATFORM_SERVICE_QUEUE_CAPACITY];
    int pending_count;
    int response_count;
    uint64_t next_dispatch_token;
    StasisPlatformServiceHandler handler;
    void *handler_user_data;
} StasisPlatformServiceState;

static StasisPlatformServiceState service_state;

static int external_url_hex(unsigned char byte) {
    return (byte >= '0' && byte <= '9') ||
        (byte >= 'a' && byte <= 'f') || (byte >= 'A' && byte <= 'F');
}

static int external_url_port(const unsigned char *value, int32_t length) {
    int32_t index;
    int32_t port = 0;
    if (length <= 0 || length > 5) return 0;
    for (index = 0; index < length; index += 1) {
        if (value[index] < '0' || value[index] > '9') return 0;
        port = port * 10 + (int32_t)(value[index] - '0');
    }
    return port >= 1 && port <= 65535;
}

static int external_url_dns_host(const unsigned char *host, int32_t length) {
    int32_t index;
    int32_t label_start = 0;
    int32_t numeric_labels = 0;
    int32_t label_count = 0;
    int32_t ipv4_valid = 1;
    if (length <= 0 || length > 253 || host[0] == '.' || host[length - 1] == '.') return 0;
    for (index = 0; index <= length; index += 1) {
        if (index == length || host[index] == '.') {
            int32_t label_length = index - label_start;
            int32_t digit_index;
            int32_t numeric = 1;
            int32_t numeric_value = 0;
            if (label_length <= 0 || label_length > 63 ||
                host[label_start] == '-' || host[index - 1] == '-') return 0;
            label_count += 1;
            for (digit_index = label_start; digit_index < index; digit_index += 1) {
                if (host[digit_index] < '0' || host[digit_index] > '9') numeric = 0;
                else if (digit_index - label_start < 3) {
                    numeric_value = numeric_value * 10 + (int32_t)(host[digit_index] - '0');
                }
            }
            if (numeric) {
                if ((label_length > 1 && host[label_start] == '0') ||
                    label_length > 3 || numeric_value > 255) ipv4_valid = 0;
                numeric_labels += 1;
            }
            label_start = index + 1;
        } else if (!((host[index] >= 'a' && host[index] <= 'z') ||
                     (host[index] >= 'A' && host[index] <= 'Z') ||
                     (host[index] >= '0' && host[index] <= '9') ||
                     host[index] == '-')) {
            return 0;
        }
    }
    if (numeric_labels == label_count) return label_count == 4 && ipv4_valid;
    return 1;
}

static int external_url_ipv6(const unsigned char *host, int32_t length) {
    int32_t index = 0;
    int32_t groups = 0;
    int32_t compressed = 0;
    if (length < 2) return 0;
    while (index < length) {
        int32_t digits = 0;
        if (host[index] == ':') {
            if (index + 1 >= length || host[index + 1] != ':' || compressed) return 0;
            compressed = 1;
            index += 2;
            if (index == length) break;
            continue;
        }
        while (index < length && host[index] != ':') {
            if (!external_url_hex(host[index]) || digits >= 4) return 0;
            digits += 1;
            index += 1;
        }
        if (digits == 0) return 0;
        groups += 1;
        if (groups > 8) return 0;
        if (index < length) {
            if (index + 1 < length && host[index + 1] == ':') {
                if (compressed) return 0;
                compressed = 1;
                index += 2;
                if (index == length) break;
            } else {
                if (index + 1 == length) return 0;
                index += 1;
            }
        }
    }
    return compressed ? groups < 8 : groups == 8;
}

static int external_url_authority(const unsigned char *value, int32_t length) {
    int32_t index;
    int32_t host_length = length;
    if (length <= 0) return 0;
    for (index = 0; index < length; index += 1) {
        if (value[index] == '@' || value[index] >= 0x80) return 0;
    }
    if (value[0] == '[') {
        int32_t close = 1;
        while (close < length && value[close] != ']') {
            close += 1;
        }
        if (close <= 1 || close >= length || !external_url_ipv6(value + 1, close - 1)) return 0;
        if (close + 1 == length) return 1;
        return value[close + 1] == ':' &&
            external_url_port(value + close + 2, length - close - 2);
    }
    for (index = 0; index < length; index += 1) {
        if (value[index] == ':') {
            if (host_length != length) return 0;
            host_length = index;
        }
    }
    if (!external_url_dns_host(value, host_length)) return 0;
    return host_length == length ||
        external_url_port(value + host_length + 1, length - host_length - 1);
}

int stasis_external_url_validate(const char *url, int32_t length) {
    const unsigned char *bytes = (const unsigned char *)url;
    int32_t index;
    int32_t authority_start;
    int32_t authority_end;
    if (url == NULL || length <= 0 || length > STASIS_EXTERNAL_URL_MAX_BYTES) return 0;
    if (length >= 8 && memcmp(url, "https://", 8) == 0) authority_start = 8;
    else if (length >= 7 && memcmp(url, "http://", 7) == 0) authority_start = 7;
    else return 0;

    authority_end = length;
    for (index = authority_start; index < length; index += 1) {
        unsigned char byte = bytes[index];
        if (byte == '/' || byte == '?' || byte == '#') {
            authority_end = index;
            break;
        }
    }
    if (!external_url_authority(bytes + authority_start, authority_end - authority_start)) return 0;

    for (index = 0; index < length; index += 1) {
        uint32_t codepoint;
        uint32_t minimum;
        int32_t remaining;
        unsigned char first = bytes[index];
        if (first == 0 || first == '\\' || first <= 0x20 || first == 0x7f) return 0;
        if (first == '%') {
            if (index + 2 >= length || !external_url_hex(bytes[index + 1]) ||
                !external_url_hex(bytes[index + 2])) return 0;
        }
        if (first <= 0x7f) continue;
        if (first >= 0xc2 && first <= 0xdf) {
            codepoint = (uint32_t)(first & 0x1f);
            minimum = 0x80;
            remaining = 1;
        } else if (first >= 0xe0 && first <= 0xef) {
            codepoint = (uint32_t)(first & 0x0f);
            minimum = 0x800;
            remaining = 2;
        } else if (first >= 0xf0 && first <= 0xf4) {
            codepoint = (uint32_t)(first & 0x07);
            minimum = 0x10000;
            remaining = 3;
        } else return 0;
        if (index + remaining >= length) return 0;
        while (remaining > 0) {
            unsigned char continuation = bytes[++index];
            if (continuation < 0x80 || continuation > 0xbf) return 0;
            codepoint = (codepoint << 6) | (uint32_t)(continuation & 0x3f);
            remaining -= 1;
        }
        if (codepoint < minimum || (codepoint >= 0xd800 && codepoint <= 0xdfff) ||
            codepoint > 0x10ffff || (codepoint >= 0x80 && codepoint <= 0x9f)) return 0;
    }
    return 1;
}

void stasis_external_url_action_begin_frame(
    StasisExternalUrlActionState *state,
    int32_t has_input_edge,
    int32_t disabled
) {
    if (state == NULL) return;
    state->gesture_available = has_input_edge != 0 ? 1 : 0;
    state->disabled = disabled != 0 ? 1 : 0;
}

void stasis_external_url_action_clear(StasisExternalUrlActionState *state) {
    if (state != NULL) memset(state, 0, sizeof(*state));
}

int stasis_external_url_action_request(
    StasisExternalUrlActionState *state,
    const char *url,
    int32_t length,
    StasisExternalUrlOpener opener,
    void *user_data
) {
    int result;
    if (!stasis_external_url_validate(url, length)) return -1;
    if (state == NULL || state->disabled || !state->gesture_available) return 0;
    state->gesture_available = 0;
    if (opener == NULL) return 0;
    result = opener(url, length, user_data);
    return result > 0 ? 1 : 0;
}

static int printable_ascii(const char *value, int32_t length) {
    int32_t index;
    if (value == NULL || length <= 0 || length > STASIS_PLATFORM_SERVICE_KEY_CAPACITY) return 0;
    for (index = 0; index < length; index += 1) {
        unsigned char byte = (unsigned char)value[index];
        if (byte < 32 || byte > 126) return 0;
    }
    return 1;
}

static int valid_response_status(int32_t status) {
    return status >= STASIS_PLATFORM_SERVICE_RESPONSE_OK &&
        status <= STASIS_PLATFORM_SERVICE_RESPONSE_FAILED;
}

static int utf8_char_count(const char *text, int32_t length, int32_t *out_count) {
    int32_t index = 0;
    int32_t count = 0;
    if (length < 0 || length > STASIS_PLATFORM_SERVICE_TEXT_CAPACITY || out_count == NULL) return 0;
    if (length > 0 && text == NULL) return 0;
    while (index < length) {
        uint32_t codepoint;
        uint32_t minimum;
        int32_t remaining;
        unsigned char first = (unsigned char)text[index++];
        if (first == 0) return 0;
        if (first <= 0x7f) {
            count += 1;
            continue;
        }
        if (first >= 0xc2 && first <= 0xdf) {
            codepoint = (uint32_t)(first & 0x1f);
            minimum = 0x80;
            remaining = 1;
        } else if (first >= 0xe0 && first <= 0xef) {
            codepoint = (uint32_t)(first & 0x0f);
            minimum = 0x800;
            remaining = 2;
        } else if (first >= 0xf0 && first <= 0xf4) {
            codepoint = (uint32_t)(first & 0x07);
            minimum = 0x10000;
            remaining = 3;
        } else {
            return 0;
        }
        if (index + remaining > length) return 0;
        while (remaining > 0) {
            unsigned char continuation = (unsigned char)text[index++];
            if (continuation < 0x80 || continuation > 0xbf) return 0;
            codepoint = (codepoint << 6) | (uint32_t)(continuation & 0x3f);
            remaining -= 1;
        }
        if (codepoint < minimum || (codepoint >= 0xd800 && codepoint <= 0xdfff) ||
            codepoint > 0x10ffff) return 0;
        count += 1;
    }
    *out_count = count;
    return 1;
}

static int pending_index(int32_t service, int32_t action, int32_t request_id) {
    int index;
    for (index = 0; index < service_state.pending_count; index += 1) {
        StasisPlatformServiceRequest *request = &service_state.pending[index];
        if (request->service == service && request->action == action &&
            request->request_id == request_id) return index;
    }
    return -1;
}

static int pending_request_id_index(int32_t request_id) {
    int index;
    for (index = 0; index < service_state.pending_count; index += 1) {
        if (service_state.pending[index].request_id == request_id) return index;
    }
    return -1;
}

static int pending_dispatch_token_index(uint64_t dispatch_token) {
    int index;
    for (index = 0; index < service_state.pending_count; index += 1) {
        if (service_state.pending[index].dispatch_token == dispatch_token) return index;
    }
    return -1;
}

static int response_index(int32_t service, int32_t action, int32_t request_id) {
    int index;
    for (index = 0; index < service_state.response_count; index += 1) {
        StasisPlatformServiceResponse *response = &service_state.responses[index];
        if (response->service == service && response->action == action &&
            response->request_id == request_id) return index;
    }
    return -1;
}

static void remove_pending_at(int index) {
    if (index < 0 || index >= service_state.pending_count) return;
    if (index + 1 < service_state.pending_count) {
        memmove(
            &service_state.pending[index],
            &service_state.pending[index + 1],
            (size_t)(service_state.pending_count - index - 1) * sizeof(service_state.pending[0])
        );
    }
    service_state.pending_count -= 1;
}

void stasis_platform_service_set_handler(
    StasisPlatformServiceHandler handler,
    void *user_data
) {
    SERVICE_LOCK();
    service_state.handler = handler;
    service_state.handler_user_data = user_data;
    SERVICE_UNLOCK();
}

void stasis_platform_service_reset(void) {
    SERVICE_LOCK();
    memset(service_state.pending, 0, sizeof(service_state.pending));
    memset(service_state.responses, 0, sizeof(service_state.responses));
    service_state.pending_count = 0;
    service_state.response_count = 0;
    SERVICE_UNLOCK();
}

int stasis_platform_service_publish_response(
    uint64_t dispatch_token,
    int32_t status,
    int32_t value,
    const char *text,
    int32_t text_length
) {
    int32_t char_length = 0;
    int pending_at;
    StasisPlatformServiceRequest *request;
    StasisPlatformServiceResponse *response;
    if (dispatch_token == 0 || !valid_response_status(status) ||
        !utf8_char_count(text, text_length, &char_length)) return STASIS_PLATFORM_SERVICE_SUBMIT_INVALID;

    SERVICE_LOCK();
    pending_at = pending_dispatch_token_index(dispatch_token);
    if (pending_at < 0) {
        SERVICE_UNLOCK();
        return STASIS_PLATFORM_SERVICE_SUBMIT_INVALID;
    }
    request = &service_state.pending[pending_at];
    if (response_index(request->service, request->action, request->request_id) >= 0) {
        SERVICE_UNLOCK();
        return STASIS_PLATFORM_SERVICE_SUBMIT_INVALID;
    }
    if (service_state.response_count >= STASIS_PLATFORM_SERVICE_QUEUE_CAPACITY) {
        SERVICE_UNLOCK();
        return STASIS_PLATFORM_SERVICE_SUBMIT_BUSY;
    }
    response = &service_state.responses[service_state.response_count++];
    memset(response, 0, sizeof(*response));
    response->service = request->service;
    response->action = request->action;
    response->request_id = request->request_id;
    response->status = status;
    response->value = value;
    response->text_length = text_length;
    response->text_char_length = char_length;
    if (text_length > 0) memcpy(response->text, text, (size_t)text_length);
    response->text[text_length] = '\0';
    SERVICE_UNLOCK();
    return STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED;
}

int stasis_platform_service_submit(
    int32_t service,
    int32_t action,
    int32_t request_id,
    const char *key,
    int32_t key_length
) {
    StasisPlatformServiceRequest request;
    StasisPlatformServiceHandler handler;
    void *user_data;
    int dispatch;
    if (service <= 0 || action <= 0 || request_id <= 0 || !printable_ascii(key, key_length)) {
        return STASIS_PLATFORM_SERVICE_SUBMIT_INVALID;
    }

    memset(&request, 0, sizeof(request));
    request.service = service;
    request.action = action;
    request.request_id = request_id;
    request.key_length = key_length;
    memcpy(request.key, key, (size_t)key_length);

    SERVICE_LOCK();
    if (pending_request_id_index(request_id) >= 0) {
        SERVICE_UNLOCK();
        return STASIS_PLATFORM_SERVICE_SUBMIT_INVALID;
    }
    if (service_state.pending_count >= STASIS_PLATFORM_SERVICE_QUEUE_CAPACITY) {
        SERVICE_UNLOCK();
        return STASIS_PLATFORM_SERVICE_SUBMIT_BUSY;
    }
    do {
        service_state.next_dispatch_token += 1;
        if (service_state.next_dispatch_token == 0) service_state.next_dispatch_token += 1;
    } while (pending_dispatch_token_index(service_state.next_dispatch_token) >= 0);
    request.dispatch_token = service_state.next_dispatch_token;
    service_state.pending[service_state.pending_count++] = request;
    handler = service_state.handler;
    user_data = service_state.handler_user_data;
    SERVICE_UNLOCK();

    if (handler == NULL) {
        (void)stasis_platform_service_publish_response(
            request.dispatch_token, STASIS_PLATFORM_SERVICE_RESPONSE_UNSUPPORTED, 0, NULL, 0);
        return STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED;
    }

    dispatch = handler(&request, user_data);
    if (dispatch == STASIS_PLATFORM_SERVICE_DISPATCH_UNSUPPORTED) {
        (void)stasis_platform_service_publish_response(
            request.dispatch_token, STASIS_PLATFORM_SERVICE_RESPONSE_UNSUPPORTED, 0, NULL, 0);
    } else if (dispatch != STASIS_PLATFORM_SERVICE_DISPATCH_ACCEPTED) {
        (void)stasis_platform_service_publish_response(
            request.dispatch_token, STASIS_PLATFORM_SERVICE_RESPONSE_FAILED, 0, NULL, 0);
    }
    return STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED;
}

int stasis_platform_service_poll(
    StasisPlatformServiceResponse *response,
    int32_t text_capacity
) {
    int pending;
    if (response == NULL || text_capacity < 0) return STASIS_PLATFORM_SERVICE_SUBMIT_INVALID;
    SERVICE_LOCK();
    if (service_state.response_count == 0) {
        SERVICE_UNLOCK();
        return 0;
    }
    if (service_state.responses[0].text_length > text_capacity) {
        SERVICE_UNLOCK();
        return STASIS_PLATFORM_SERVICE_SUBMIT_INVALID;
    }
    *response = service_state.responses[0];
    pending = pending_index(response->service, response->action, response->request_id);
    remove_pending_at(pending);
    if (service_state.response_count > 1) {
        memmove(
            &service_state.responses[0],
            &service_state.responses[1],
            (size_t)(service_state.response_count - 1) * sizeof(service_state.responses[0])
        );
    }
    service_state.response_count -= 1;
    SERVICE_UNLOCK();
    return 1;
}
