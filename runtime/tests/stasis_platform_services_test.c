#include "stasis_platform_services.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)
#define URL_VALID(value) stasis_external_url_validate((value), (int32_t)sizeof(value) - 1)

static StasisPlatformServiceRequest deferred_request;
static int external_url_open_count;

static int count_external_url(const char *url, int32_t length, void *user_data) {
    int *result = (int *)user_data;
    CHECK(length == 27);
    CHECK(memcmp(url, "https://www.maddoxlabs.com/", 27) == 0);
    external_url_open_count += 1;
    return *result;
}

static int publish_price(
    const StasisPlatformServiceRequest *request,
    void *user_data
) {
    static const char price[] = "\xe2\x82\xac" "2.99";
    (void)user_data;
    CHECK(request != NULL);
    CHECK(request->key_length == 8);
    CHECK(memcmp(request->key, "power_up", 8) == 0);
    CHECK(stasis_platform_service_publish_response(
        request->dispatch_token,
        STASIS_PLATFORM_SERVICE_RESPONSE_OK,
        1,
        price,
        (int32_t)sizeof(price) - 1
    ) == STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    return STASIS_PLATFORM_SERVICE_DISPATCH_ACCEPTED;
}

static int defer_response(
    const StasisPlatformServiceRequest *request,
    void *user_data
) {
    (void)user_data;
    deferred_request = *request;
    return STASIS_PLATFORM_SERVICE_DISPATCH_ACCEPTED;
}

int main(void) {
    StasisPlatformServiceRequest stale_request;
    StasisPlatformServiceResponse response;
    int index;
    StasisExternalUrlActionState url_action = {0};
    int opener_result = 1;
    char oversized_url[STASIS_EXTERNAL_URL_MAX_BYTES + 2];
    char maximum_url[STASIS_EXTERNAL_URL_MAX_BYTES];
    const char malformed_utf8_url[] = "https://example.com/\xc3\x28";
    const char c1_url[] = "https://example.com/\xc2\x80";

    CHECK(URL_VALID("https://www.maddoxlabs.com/") == 1);
    CHECK(URL_VALID("http://localhost:8080/path?q=one%20two") == 1);
    CHECK(URL_VALID("https://[2001:db8::1]/") == 1);
    CHECK(URL_VALID("https://example.com/\xe2\x9c\x93") == 1);
    CHECK(URL_VALID("ftp://example.com") == 0);
    CHECK(URL_VALID("https://example.com/\nnext") == 0);
    CHECK(URL_VALID("https://example.com\\next") == 0);
    CHECK(URL_VALID("https://user@example.com") == 0);
    CHECK(URL_VALID("https://:443/") == 0);
    CHECK(URL_VALID("https://example.com:0/") == 0);
    CHECK(URL_VALID("https://999.1.1.1/") == 0);
    CHECK(URL_VALID("https://[:::]/") == 0);
    CHECK(URL_VALID("https://example.com/%zz") == 0);
    CHECK(stasis_external_url_validate(
        malformed_utf8_url, (int32_t)sizeof(malformed_utf8_url) - 1) == 0);
    CHECK(stasis_external_url_validate(c1_url, (int32_t)sizeof(c1_url) - 1) == 0);
    memset(maximum_url, 'a', sizeof(maximum_url));
    memcpy(maximum_url, "https://example.com/", sizeof("https://example.com/") - 1);
    CHECK(stasis_external_url_validate(maximum_url, sizeof(maximum_url)) == 1);
    memset(oversized_url, 'a', sizeof(oversized_url));
    memcpy(oversized_url, "https://", 8);
    CHECK(stasis_external_url_validate(
        oversized_url, STASIS_EXTERNAL_URL_MAX_BYTES + 1) == 0);

    stasis_external_url_action_begin_frame(&url_action, 1, 0);
    CHECK(stasis_external_url_action_request(
        &url_action, "javascript:alert(1)", 19,
        count_external_url, &opener_result) == -1);
    CHECK(stasis_external_url_action_request(
        &url_action, "https://www.maddoxlabs.com/", 27,
        count_external_url, &opener_result) == 1);
    CHECK(external_url_open_count == 1);
    CHECK(stasis_external_url_action_request(
        &url_action, "https://www.maddoxlabs.com/", 27,
        count_external_url, &opener_result) == 0);
    CHECK(external_url_open_count == 1);
    stasis_external_url_action_begin_frame(&url_action, 1, 0);
    CHECK(stasis_external_url_action_request(
        &url_action, "https://www.maddoxlabs.com/", 27, NULL, NULL) == 0);
    CHECK(stasis_external_url_action_request(
        &url_action, "https://www.maddoxlabs.com/", 27,
        count_external_url, &opener_result) == 0);
    stasis_external_url_action_begin_frame(&url_action, 1, 1);
    CHECK(stasis_external_url_action_request(
        &url_action, "https://www.maddoxlabs.com/", 27,
        count_external_url, &opener_result) == 0);
    CHECK(external_url_open_count == 1);

    stasis_platform_service_set_handler(NULL, NULL);
    stasis_platform_service_reset();
    CHECK(stasis_platform_service_submit(1, 1, 1, "power_up", 8) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    memset(&response, 0, sizeof(response));
    CHECK(stasis_platform_service_poll(&response, 0) == 1);
    CHECK(response.service == 1);
    CHECK(response.action == 1);
    CHECK(response.request_id == 1);
    CHECK(response.status == STASIS_PLATFORM_SERVICE_RESPONSE_UNSUPPORTED);
    CHECK(response.text_length == 0);
    CHECK(stasis_platform_service_poll(&response, 0) == 0);

    stasis_platform_service_set_handler(publish_price, NULL);
    CHECK(stasis_platform_service_submit(2, 3, 2, "power_up", 8) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    CHECK(stasis_platform_service_poll(&response, 6) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_INVALID);
    CHECK(stasis_platform_service_poll(&response, STASIS_PLATFORM_SERVICE_TEXT_CAPACITY) == 1);
    CHECK(response.status == STASIS_PLATFORM_SERVICE_RESPONSE_OK);
    CHECK(response.value == 1);
    CHECK(response.text_length == 7);
    CHECK(response.text_char_length == 5);
    CHECK(memcmp(response.text, "\xe2\x82\xac" "2.99", 7) == 0);

    stasis_platform_service_set_handler(defer_response, NULL);
    CHECK(stasis_platform_service_submit(3, 4, 3, "restore", 7) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    CHECK(stasis_platform_service_submit(9, 9, 3, "duplicate", 9) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_INVALID);
    CHECK(stasis_platform_service_poll(&response, STASIS_PLATFORM_SERVICE_TEXT_CAPACITY) == 0);
    CHECK(deferred_request.service == 3 && deferred_request.action == 4 &&
        deferred_request.request_id == 3 && deferred_request.dispatch_token != 0);
    CHECK(stasis_platform_service_publish_response(
        deferred_request.dispatch_token,
        STASIS_PLATFORM_SERVICE_RESPONSE_CANCELLED,
        0,
        NULL,
        0
    ) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    CHECK(stasis_platform_service_publish_response(
        deferred_request.dispatch_token,
        STASIS_PLATFORM_SERVICE_RESPONSE_CANCELLED,
        0,
        NULL,
        0
    ) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_INVALID);
    CHECK(stasis_platform_service_poll(&response, STASIS_PLATFORM_SERVICE_TEXT_CAPACITY) == 1);
    CHECK(response.status == STASIS_PLATFORM_SERVICE_RESPONSE_CANCELLED);

    stasis_platform_service_reset();
    for (index = 0; index < STASIS_PLATFORM_SERVICE_QUEUE_CAPACITY; index += 1) {
        CHECK(stasis_platform_service_submit(1, 1, 100 + index, "queued", 6) ==
            STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    }
    CHECK(stasis_platform_service_submit(1, 1, 999, "full", 4) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_BUSY);
    stale_request = deferred_request;
    stasis_platform_service_reset();
    CHECK(stasis_platform_service_submit(5, 6, stale_request.request_id, "still_set", 9) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    CHECK(deferred_request.service == 5 && deferred_request.action == 6 &&
        deferred_request.request_id == stale_request.request_id &&
        deferred_request.dispatch_token != stale_request.dispatch_token);
    CHECK(stasis_platform_service_publish_response(
        stale_request.dispatch_token,
        STASIS_PLATFORM_SERVICE_RESPONSE_OK,
        0,
        NULL,
        0
    ) == STASIS_PLATFORM_SERVICE_SUBMIT_INVALID);
    CHECK(stasis_platform_service_publish_response(
        deferred_request.dispatch_token,
        STASIS_PLATFORM_SERVICE_RESPONSE_OK,
        0,
        NULL,
        0
    ) == STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    CHECK(stasis_platform_service_poll(&response, STASIS_PLATFORM_SERVICE_TEXT_CAPACITY) == 1);
    CHECK(response.service == 5 && response.action == 6 &&
        response.request_id == stale_request.request_id);
    stasis_platform_service_reset();

    CHECK(stasis_platform_service_submit(0, 1, 1, "bad", 3) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_INVALID);
    CHECK(stasis_platform_service_submit(1, 1, 1, "bad\nkey", 7) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_INVALID);
    CHECK(stasis_platform_service_submit(1, 1, 2000, "bad_utf8", 8) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_ACCEPTED);
    CHECK(stasis_platform_service_publish_response(
        deferred_request.dispatch_token,
        STASIS_PLATFORM_SERVICE_RESPONSE_OK,
        0,
        "\xff",
        1
    ) ==
        STASIS_PLATFORM_SERVICE_SUBMIT_INVALID);

    stasis_platform_service_set_handler(NULL, NULL);
    stasis_platform_service_reset();
    puts("stasis_platform_services_test: ok");
    return 0;
}
