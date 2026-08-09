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
