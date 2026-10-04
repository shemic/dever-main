#include "runtime.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* These deliberately invalid domain inputs fail before opening a connection.
 * Retrying the same operation after invalid output slots proves poll did not
 * advance the future or mark it completed on the rejected call. */
static unsigned frames_created;
static unsigned frames_destroyed;

static void release_buffer(DeverRtBuffer *buffer) {
    dever_rt_v1_buffer_free(buffer->ptr, buffer->len);
    buffer->ptr = NULL;
    buffer->len = 0;
}

static int message_is(const DeverRtBuffer *buffer, const char *expected) {
    size_t length = strlen(expected);
    return buffer->len == length && buffer->ptr &&
           memcmp(buffer->ptr, expected, length) == 0;
}

static int text(const char *value, void **out) {
    DeverRtBuffer error = {0};
    uint32_t status = dever_rt_v1_text_new((const uint8_t *)value,
                                         (uint64_t)strlen(value), out, &error);
    release_buffer(&error);
    return status == DEVER_RT_OK;
}

static int probe_slots(DeverRtPollState *state, DeverRtAsyncOp *operation,
                       const char *runtime_message) {
    if (!operation) return 1;
    DeverRtBuffer error = {0};
    uint8_t present = 37;
    int64_t fault = 41;
    void *output = (void *)(uintptr_t)47;
    uint32_t status = dever_rt_v1_async_op_poll(state, operation, NULL, &present,
                                               &fault, &error);
    int valid = status == DEVER_RT_ASYNC_INVALID_INPUT &&
                message_is(&error, "invalid protocol handle output") &&
                present == 37 && fault == 41;
    release_buffer(&error);
    if (!valid) return 2;

    _Alignas(void *) unsigned char unaligned[sizeof(void *) + 1];
    memset(unaligned, 53, sizeof(unaligned));
    status = dever_rt_v1_async_op_poll(state, operation, unaligned + 1,
                                       &present, &fault, &error);
    valid = status == DEVER_RT_ASYNC_INVALID_INPUT &&
            message_is(&error, "invalid protocol handle output") &&
            present == 37 && fault == 41;
    for (size_t index = 0; index < sizeof(unaligned); ++index) {
        valid = valid && unaligned[index] == 53;
    }
    release_buffer(&error);
    if (!valid) return 3;

    status = dever_rt_v1_async_op_poll(state, operation, &output, &present,
                                       &fault, &error);
    valid = status == DEVER_RT_ASYNC_READY_RUNTIME_ERROR &&
            message_is(&error, runtime_message) &&
            output == (void *)(uintptr_t)47 && present == 37 && fault == 41;
    release_buffer(&error);
    if (!valid) return 4;

    status = dever_rt_v1_async_op_poll(state, operation, &output, &present,
                                       &fault, &error);
    valid = status == DEVER_RT_ASYNC_INVALID_INPUT &&
            message_is(&error, "async operation is not polling");
    release_buffer(&error);
    return valid ? 0 : 5;
}

static int probe_http(DeverRtPollState *state) {
    void *host = NULL, *method = NULL, *target = NULL, *empty = NULL;
    void *headers = NULL, *body = NULL, *request = NULL;
    DeverRtAsyncOp *operation = NULL;
    DeverRtBuffer error = {0};
    int result = 10;
    if (!text("127.0.0.1", &host) || !text("GET", &method) ||
        !text("/", &target) || !text("", &empty)) goto cleanup;
    if (dever_rt_v1_http_headers_new(&headers, &error) != DEVER_RT_OK) goto cleanup;
    if (dever_rt_v1_bytes_from_text(empty, &body, &error) != DEVER_RT_OK) goto cleanup;
    DeverRtHttpRequestFields fields = {method, target, headers, body};
    if (dever_rt_v1_http_request_new(&fields, &request, &error) != DEVER_RT_OK) goto cleanup;
    DeverRtHttpLimits limits = {1, 0, 100, 1, NULL};
    operation = dever_rt_v1_async_http_send(host, 0, request, &limits);
    /* Constructor ownership survives releasing every borrowed input. */
    dever_rt_v1_http_request_release(request);
    request = NULL;
    dever_rt_v1_text_release(host);
    host = NULL;
    result = probe_slots(state, operation, "HTTP header_bytes must be at least 8192");
cleanup:
    dever_rt_v1_async_op_release(operation);
    dever_rt_v1_http_request_release(request);
    dever_rt_v1_http_headers_release(headers);
    dever_rt_v1_bytes_release(body);
    dever_rt_v1_text_release(host);
    dever_rt_v1_text_release(method);
    dever_rt_v1_text_release(target);
    dever_rt_v1_text_release(empty);
    release_buffer(&error);
    return result;
}

static int probe_websocket_open(DeverRtPollState *state) {
    void *url = NULL, *tls = NULL;
    DeverRtAsyncOp *operation = NULL;
    DeverRtBuffer error = {0};
    int result = 20;
    if (!text("http://fixture.invalid/", &url)) goto cleanup;
    if (dever_rt_v1_tls_system(&tls, &error) != DEVER_RT_OK) goto cleanup;
    DeverRtWsLimits limits = {128, 100, 100};
    operation = dever_rt_v1_async_ws_open(url, tls, &limits);
    dever_rt_v1_text_release(url);
    url = NULL;
    dever_rt_v1_client_tls_release(tls);
    tls = NULL;
    result = probe_slots(state, operation, "WebSocket URL must use ws or wss");
cleanup:
    dever_rt_v1_async_op_release(operation);
    dever_rt_v1_text_release(url);
    dever_rt_v1_client_tls_release(tls);
    release_buffer(&error);
    return result;
}

typedef struct {
    DeverRtPollState *state;
    int64_t *output;
    uint8_t completed;
} ProbeFrame;

static void move_int(void *source, void *destination) {
    *(int64_t *)destination = *(int64_t *)source;
    *(int64_t *)source = 0;
}

static void drop_int(void *value) { *(int64_t *)value = 0; }

static void *create(DeverRtPollState *state, const void *input, void *output, void *fault) {
    (void)input;
    (void)fault;
    ProbeFrame *frame = calloc(1, sizeof(*frame));
    if (!frame) return NULL;
    frame->state = state;
    frame->output = output;
    ++frames_created;
    return frame;
}

static void resume(void *raw) {
    ProbeFrame *frame = raw;
    int result = probe_http(frame->state);
    if (result == 0) result = probe_websocket_open(frame->state);
    *frame->output = result;
    frame->completed = 1;
    dever_rt_v1_async_complete(frame->state, DEVER_RT_ASYNC_READY_OK);
}

static void destroy(void *raw) { ++frames_destroyed; free(raw); }
static uint8_t done(void *raw) { return ((ProbeFrame *)raw)->completed; }

int main(void) {
    static const DeverRtOwnedType integer = {
        sizeof(int64_t), _Alignof(int64_t), move_int, drop_int, 1
    };
    static const DeverRtAsyncFunction function = {
        &integer, &integer, create, resume, destroy, done, 1
    };
    int64_t output = -1, fault = -1;
    DeverRtBuffer error = {0};
    uint32_t status = dever_rt_v1_async_root(&function, NULL, &output, &fault, &error);
    int valid = status == DEVER_RT_ASYNC_READY_OK && output == 0 &&
                fault == -1 && error.ptr == NULL && error.len == 0 &&
                frames_created == 1 && frames_destroyed == 1;
    release_buffer(&error);
    return valid ? 0 : 1;
}
