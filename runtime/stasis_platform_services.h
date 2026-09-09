#ifndef STASIS_PLATFORM_SERVICES_H
#define STASIS_PLATFORM_SERVICES_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define STASIS_PLATFORM_SERVICE_QUEUE_CAPACITY 16
#define STASIS_PLATFORM_SERVICE_KEY_CAPACITY 128
#define STASIS_PLATFORM_SERVICE_TEXT_CAPACITY 512
#define STASIS_EXTERNAL_URL_MAX_BYTES 2048

typedef int (*StasisExternalUrlOpener)(
    const char *url,
    int32_t length,
    void *user_data
);

typedef struct StasisExternalUrlActionState {
    int32_t gesture_available;
    int32_t disabled;
} StasisExternalUrlActionState;

/* Returns 1 only for a bounded, well-formed http:// or https:// URL. */
int stasis_external_url_validate(const char *url, int32_t length);

/* Replaces the previous frame's authority with one real input-edge budget. */
void stasis_external_url_action_begin_frame(
    StasisExternalUrlActionState *state,
    int32_t has_input_edge,
    int32_t disabled
);

/* Clears authority before lifecycle transitions and non-guest callbacks. */
void stasis_external_url_action_clear(StasisExternalUrlActionState *state);

/* Returns -1 invalid, 0 ignored/unavailable, or 1 accepted by the host. */
int stasis_external_url_action_request(
    StasisExternalUrlActionState *state,
    const char *url,
    int32_t length,
    StasisExternalUrlOpener opener,
    void *user_data
);

enum StasisPlatformServiceSubmitResult {
    STASIS_PLATFORM_SERVICE_SUBMIT_INVALID = -1,
    STASIS_PLATFORM_SERVICE_SUBMIT_BUSY = 0,
    STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED = 1
};

enum StasisPlatformServiceResponseStatus {
    STASIS_PLATFORM_SERVICE_RESPONSE_OK = 1,
    STASIS_PLATFORM_SERVICE_RESPONSE_CANCELLED = 2,
    STASIS_PLATFORM_SERVICE_RESPONSE_UNAVAILABLE = 3,
    STASIS_PLATFORM_SERVICE_RESPONSE_UNSUPPORTED = 4,
    STASIS_PLATFORM_SERVICE_RESPONSE_FAILED = 5
};

enum StasisPlatformServiceDispatchResult {
    STASIS_PLATFORM_SERVICE_DISPATCH_FAILED = -1,
    STASIS_PLATFORM_SERVICE_DISPATCH_UNSUPPORTED = 0,
    STASIS_PLATFORM_SERVICE_DISPATCH_ACCEPTED = 1
};

typedef struct StasisPlatformServiceRequest {
    int32_t service;
    int32_t action;
    int32_t request_id;
    uint64_t dispatch_token;
    int32_t key_length;
    char key[STASIS_PLATFORM_SERVICE_KEY_CAPACITY + 1];
} StasisPlatformServiceRequest;

typedef struct StasisPlatformServiceResponse {
    int32_t service;
    int32_t action;
    int32_t request_id;
    int32_t status;
    int32_t value;
    int32_t text_length;
    int32_t text_char_length;
    char text[STASIS_PLATFORM_SERVICE_TEXT_CAPACITY + 1];
} StasisPlatformServiceResponse;

/* The request pointer is valid only during the call; deferred handlers must copy it. */
typedef int (*StasisPlatformServiceHandler)(
    const StasisPlatformServiceRequest *request,
    void *user_data
);

/* Registration persists across queue resets. Adapters must marshal any UI work. */
void stasis_platform_service_set_handler(
    StasisPlatformServiceHandler handler,
    void *user_data
);

/* Clears pending requests and responses without unregistering the host adapter. */
void stasis_platform_service_reset(void);

/* Submits one request. Keys are non-empty printable ASCII and request IDs are positive. */
int stasis_platform_service_submit(
    int32_t service,
    int32_t action,
    int32_t request_id,
    const char *key,
    int32_t key_length
);

/* Publishes one response for an outstanding dispatch token. Text must be valid UTF-8. */
int stasis_platform_service_publish_response(
    uint64_t dispatch_token,
    int32_t status,
    int32_t value,
    const char *text,
    int32_t text_length
);

/* Returns 1 for a response, 0 when empty, or -1 when the output capacity is too small. */
int stasis_platform_service_poll(
    StasisPlatformServiceResponse *response,
    int32_t text_capacity
);

#ifdef __cplusplus
}
#endif

#endif
