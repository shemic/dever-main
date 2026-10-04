#include "runtime.h"
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define EXPECT(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "resource ABI assertion failed at line %d: %s\n", __LINE__, #condition); \
        return 1; \
    } \
} while (0)

typedef struct {
    uint64_t kind;
    void *value;
} ReadEvent;

static unsigned event_retains;
static unsigned event_drops;

static void event_clone(const void *source, void *destination) {
    const ReadEvent *event = source;
    ReadEvent *copy = destination;
    copy->kind = event->kind;
    copy->value = event->kind == 1
        ? dever_rt_v1_bytes_retain(event->value)
        : dever_rt_v1_text_retain(event->value);
    ++event_retains;
}

static void event_drop(void *value) {
    ReadEvent *event = value;
    if (event->kind == 1) {
        dever_rt_v1_bytes_release(event->value);
    } else {
        dever_rt_v1_text_release(event->value);
    }
    ++event_drops;
}

static uint8_t event_equal(const void *left, const void *right) {
    const ReadEvent *first = left;
    const ReadEvent *second = right;
    if (first->kind != second->kind) return 0;
    return first->kind == 1
        ? dever_rt_v1_bytes_equal(first->value, second->value)
        : dever_rt_v1_text_equal(first->value, second->value);
}

static void event_chunk(const void *bytes, void *out) {
    *(ReadEvent *)out = (ReadEvent){1, dever_rt_v1_bytes_retain(bytes)};
    ++event_retains;
}

static void event_failed(const void *text, void *out) {
    *(ReadEvent *)out = (ReadEvent){2, dever_rt_v1_text_retain(text)};
    ++event_retains;
}

static const DeverRtType EVENT_TYPE = {
    sizeof(ReadEvent), _Alignof(ReadEvent), event_clone, event_drop, event_equal, NULL
};
static const DeverRtReadEvent READ_EVENT = {
    &EVENT_TYPE, event_chunk, event_failed
};

static int expect_buffer(DeverRtBuffer *buffer, const char *expected) {
    size_t length = strlen(expected);
    int equal = buffer->len == length &&
        (length == 0 || (buffer->ptr && memcmp(buffer->ptr, expected, length) == 0));
    dever_rt_v1_buffer_free(buffer->ptr, buffer->len);
    *buffer = (DeverRtBuffer){NULL, 0};
    return equal;
}

static int expect_nonempty(DeverRtBuffer *buffer) {
    int nonempty = buffer->ptr != NULL && buffer->len != 0;
    dever_rt_v1_buffer_free(buffer->ptr, buffer->len);
    *buffer = (DeverRtBuffer){NULL, 0};
    return nonempty;
}

static int expect_event_bytes(ReadEvent *event, const char *expected) {
    DeverRtBuffer bytes = {NULL, 0};
    if (event->kind != 1 || dever_rt_v1_bytes_export(event->value, &bytes) != DEVER_RT_OK) {
        return 0;
    }
    return expect_buffer(&bytes, expected);
}

int main(int argc, char **argv) {
    EXPECT(argc == 2);
    DeverRtBuffer error = {NULL, 0};
    void *path = NULL;
    void *file = NULL;
    void *bytes = NULL;
    void *stream = NULL;
    uint8_t done = 0;
    uint8_t present = 0;
    EXPECT(dever_rt_v1_file_retain(NULL) == NULL);
    dever_rt_v1_file_release(NULL);
    EXPECT(dever_rt_v1_stream_retain(NULL) == NULL);
    dever_rt_v1_stream_release(NULL);
    EXPECT(dever_rt_v1_text_new((const uint8_t *)argv[1], strlen(argv[1]), &path, &error) == DEVER_RT_OK);
    char missing_path[4096];
    int missing_length = snprintf(missing_path, sizeof(missing_path), "%s.missing", argv[1]);
    EXPECT(missing_length > 0 && (size_t)missing_length < sizeof(missing_path));
    void *missing = NULL;
    EXPECT(dever_rt_v1_text_new((const uint8_t *)missing_path, (uint64_t)missing_length, &missing, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_file_open(missing, &file, &error) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_nonempty(&error));
    dever_rt_v1_text_release(missing);

    /* Invalid output must not create the file. */
    EXPECT(dever_rt_v1_file_create(path, NULL, &error) == DEVER_RT_INVALID_INPUT);
    EXPECT(expect_buffer(&error, "invalid ABI output pointer"));
    EXPECT(dever_rt_v1_file_create(path, &file, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_file_create(path, &bytes, &error) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_nonempty(&error));
    EXPECT(dever_rt_v1_bytes_new((const uint8_t *)"abcd", 4, &bytes, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_file_write(file, bytes, &done, &error) == DEVER_RT_OK && done == 1);
    void *alias = dever_rt_v1_file_retain(file);
    EXPECT(dever_rt_v1_file_close(file, &done, &error) == DEVER_RT_OK && done == 1);
    EXPECT(dever_rt_v1_file_write(alias, bytes, &done, &error) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&error, "resource is closed"));
    EXPECT(dever_rt_v1_file_close(alias, &done, &error) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&error, "resource is closed"));
    dever_rt_v1_file_release(alias);
    dever_rt_v1_file_release(file);
    dever_rt_v1_bytes_release(bytes);

    EXPECT(dever_rt_v1_file_open(path, &file, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_file_read(file, 0, &bytes, &present, &error) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&error, "read size must be a positive Int"));
    EXPECT(dever_rt_v1_file_read(file, 4, NULL, &present, &error) == DEVER_RT_INVALID_INPUT);
    EXPECT(expect_buffer(&error, "invalid ABI output pointer"));
    EXPECT(dever_rt_v1_file_read(file, 4, &bytes, &present, &error) == DEVER_RT_OK && present);
    EXPECT(dever_rt_v1_bytes_export(bytes, &error) == DEVER_RT_OK);
    EXPECT(expect_buffer(&error, "abcd"));
    dever_rt_v1_bytes_release(bytes);
    bytes = NULL;
    EXPECT(dever_rt_v1_file_read(file, 4, &bytes, &present, &error) == DEVER_RT_OK && !present);
    EXPECT(bytes == NULL);
    EXPECT(dever_rt_v1_file_close(file, &done, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_file_chunks(file, 2, &READ_EVENT, &stream, &error) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&error, "resource is closed"));
    dever_rt_v1_file_release(file);

    EXPECT(dever_rt_v1_file_open(path, &file, &error) == DEVER_RT_OK);
    DeverRtReadEvent invalid_event = {&EVENT_TYPE, event_chunk, NULL};
    EXPECT(dever_rt_v1_file_chunks(file, 2, &invalid_event, &stream, &error) == DEVER_RT_INVALID_INPUT);
    EXPECT(expect_buffer(&error, "incomplete ABI read event descriptor"));
    EXPECT(dever_rt_v1_file_chunks(file, 0, &READ_EVENT, &stream, &error) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&error, "read size must be a positive Int"));
    EXPECT(dever_rt_v1_file_chunks(file, 2, &READ_EVENT, &stream, &error) == DEVER_RT_OK);
    alias = dever_rt_v1_stream_retain(stream);
    ReadEvent event;
    EXPECT(dever_rt_v1_stream_pull(stream, NULL, &present, &error) == DEVER_RT_INVALID_INPUT);
    EXPECT(expect_buffer(&error, "invalid ABI output pointer"));
    EXPECT(dever_rt_v1_stream_pull(alias, &event, &present, &error) == DEVER_RT_OK && present);
    EXPECT(expect_event_bytes(&event, "ab"));
    event_drop(&event);
    EXPECT(dever_rt_v1_stream_pull(stream, &event, &present, &error) == DEVER_RT_OK && present);
    EXPECT(expect_event_bytes(&event, "cd"));
    event_drop(&event);
    EXPECT(dever_rt_v1_stream_pull(alias, &event, &present, &error) == DEVER_RT_OK && !present);
    EXPECT(dever_rt_v1_stream_close(stream, &done, &error) == DEVER_RT_OK && done == 1);
    EXPECT(dever_rt_v1_stream_close(alias, &done, &error) == DEVER_RT_OK && done == 1);
    dever_rt_v1_stream_release(alias);
    dever_rt_v1_stream_release(stream);
    EXPECT(dever_rt_v1_file_close(file, &done, &error) == DEVER_RT_OK);
    dever_rt_v1_file_release(file);

    EXPECT(dever_rt_v1_file_open(path, &file, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_file_chunks(file, 1, &READ_EVENT, &stream, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_stream_pull(stream, &event, &present, &error) == DEVER_RT_OK && present);
    EXPECT(expect_event_bytes(&event, "a"));
    event_drop(&event);
    EXPECT(dever_rt_v1_stream_close(stream, &done, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_stream_pull(stream, &event, &present, &error) == DEVER_RT_OK && !present);
    dever_rt_v1_stream_release(stream);
    EXPECT(dever_rt_v1_file_close(file, &done, &error) == DEVER_RT_OK);
    dever_rt_v1_file_release(file);

    EXPECT(dever_rt_v1_file_open(path, &file, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_file_chunks(file, 2, &READ_EVENT, &stream, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_file_close(file, &done, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_stream_pull(stream, &event, &present, &error) == DEVER_RT_OK && present);
    EXPECT(event.kind == 2);
    EXPECT(dever_rt_v1_text_export(event.value, &error) == DEVER_RT_OK);
    EXPECT(expect_buffer(&error, "resource is closed"));
    event_drop(&event);
    EXPECT(dever_rt_v1_stream_pull(stream, &event, &present, &error) == DEVER_RT_OK && !present);
    dever_rt_v1_stream_release(stream);
    dever_rt_v1_file_release(file);

    EXPECT(dever_rt_v1_file_chunks(NULL, 2, &READ_EVENT, &stream, &error) == DEVER_RT_INVALID_INPUT);
    EXPECT(expect_buffer(&error, "invalid ABI handle"));
    EXPECT(dever_rt_v1_file_open(NULL, &file, &error) == DEVER_RT_INVALID_INPUT);
    EXPECT(expect_buffer(&error, "invalid ABI handle"));
    dever_rt_v1_text_release(path);
    EXPECT(event_retains == event_drops);
    return 0;
}
