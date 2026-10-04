/* Canonical wire callbacks; repeated by the existing atomic allocation ledger. */
#include "runtime.h"
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct { void *title; int64_t count; } Row;
static const DeverRtWireName fields[] = {
    { (const uint8_t *)"title", 5 },
    { (const uint8_t *)"count", 5 }
};
static uint32_t calls;

static void row_clone(const void *source, void *target) {
    const Row *from = source;
    Row *to = target;
    to->title = dever_rt_v1_text_retain(from->title);
    to->count = from->count;
}

static void row_drop(void *value) {
    Row *row = value;
    dever_rt_v1_text_release(row->title);
    row->title = NULL;
}

static uint8_t row_equal(const void *left, const void *right) {
    const Row *a = left;
    const Row *b = right;
    return a->count == b->count && dever_rt_v1_text_equal(a->title, b->title);
}

static const DeverRtType row_type = {
    sizeof(Row), _Alignof(Row), row_clone, row_drop, row_equal, NULL
};

static void error_release(DeverRtBuffer *error) {
    dever_rt_v1_buffer_free(error->ptr, error->len);
    *error = (DeverRtBuffer){0};
}

static uint32_t row_decode(const void *node, void *output, DeverRtBuffer *error) {
    ++calls;
    Row *row = output;
    *row = (Row){0};
    uint32_t status = dever_rt_v1_wire_node_fields(node, fields, 2, error);
    const void *child = NULL;
    uint8_t present = 0;
    if (status == 0) status = dever_rt_v1_wire_node_field(node, &fields[0], 1, &child, &present, error);
    if (status == 0) status = dever_rt_v1_wire_node_text(child, &row->title, error);
    if (status == 0) status = dever_rt_v1_wire_node_field(node, &fields[1], 1, &child, &present, error);
    if (status == 0) status = dever_rt_v1_wire_node_int(child, &row->count, error);
    if (status != 0) row_drop(row);
    return status;
}

static uint32_t row_encode(const void *value, void *encoder, DeverRtBuffer *error) {
    ++calls;
    const Row *row = value;
    uint32_t status = dever_rt_v1_wire_encoder_begin_object(encoder, error);
    if (status == 0) status = dever_rt_v1_wire_encoder_key(encoder, &fields[0], error);
    if (status == 0) status = dever_rt_v1_wire_encoder_text(encoder, row->title, error);
    if (status == 0) status = dever_rt_v1_wire_encoder_key(encoder, &fields[1], error);
    if (status == 0) status = dever_rt_v1_wire_encoder_int(encoder, row->count, error);
    if (status == 0) status = dever_rt_v1_wire_encoder_end(encoder, error);
    return status;
}

static uint32_t bad_success_decode(const void *node, void *output, DeverRtBuffer *error) {
    uint32_t status = row_decode(node, output, error);
    if (status != 0) return status;
    int64_t ignored;
    (void)dever_rt_v1_int_div(1, 0, &ignored, error);
    /* A successful codec must not leave an error Buffer alongside its owner. */
    return 0;
}

/* Invalid destinations must not inspect a Node or advance a scoped encoder. */
static uint32_t destination_probe(const void *node, void *output, DeverRtBuffer *error) {
    const void *child = (const void *)(uintptr_t)17;
    uint8_t present = 91;
    if (dever_rt_v1_wire_node_field(NULL, &fields[0], 1, NULL, &present, error) != 2 || present != 91) return 2;
    error_release(error);
    if (dever_rt_v1_wire_node_field(NULL, &fields[0], 1, &child, NULL, error) != 2 || child != (const void *)(uintptr_t)17) return 2;
    error_release(error);
    unsigned char unaligned[sizeof(int64_t) + _Alignof(int64_t)];
    uintptr_t base = (uintptr_t)unaligned;
    size_t offset = (_Alignof(int64_t) - base % _Alignof(int64_t)) % _Alignof(int64_t);
    int64_t *bad = (int64_t *)(unaligned + offset + 1);
    if (dever_rt_v1_wire_node_int(NULL, bad, error) != 2) return 2;
    error_release(error);
    /* Correcting the destination still sees the original parsed object. */
    return row_decode(node, output, error);
}

static uint32_t command_decode(void *inputs, void *output, DeverRtBuffer *error) {
    const DeverRtWireName field = { (const uint8_t *)"record", 6 };
    void *raw = NULL;
    uint8_t present = 91;
    *(Row *)output = (Row){0};
    if (dever_rt_v1_cmd_input_field(inputs, &field, 1, NULL, &present, error) != 2 || present != 91) return 2;
    error_release(error);
    uint32_t status = dever_rt_v1_cmd_input_field(inputs, &field, 1, &raw, &present, error);
    if (status == 0) status = dever_rt_v1_wire_decode(raw, &row_type, row_decode, output, error);
    dever_rt_v1_text_release(raw);
    return status;
}

static int text_equals(void *text, const char *expected) {
    DeverRtBuffer bytes = {0};
    uint32_t status = dever_rt_v1_text_export(text, &bytes);
    int equal = status == 0 && bytes.len == strlen(expected) && memcmp(bytes.ptr, expected, (size_t)bytes.len) == 0;
    error_release(&bytes);
    return equal;
}

static void bytes_clone(const void *source, void *target) {
    *(void **)target = dever_rt_v1_bytes_retain(*(void *const *)source);
}

static void bytes_drop(void *value) {
    dever_rt_v1_bytes_release(*(void **)value);
    *(void **)value = NULL;
}

static uint8_t bytes_equal(const void *left, const void *right) {
    return dever_rt_v1_bytes_equal(*(void *const *)left, *(void *const *)right);
}

static const DeverRtType bytes_type = {
    sizeof(void *), _Alignof(void *), bytes_clone, bytes_drop, bytes_equal, NULL
};

static uint32_t bytes_decode(const void *node, void *output, DeverRtBuffer *error) {
    if (dever_rt_v1_wire_node_bytes(NULL, NULL, error) != 2) return 2;
    error_release(error);
    return dever_rt_v1_wire_node_bytes(node, output, error);
}

static uint32_t bytes_encode(const void *value, void *encoder, DeverRtBuffer *error) {
    return dever_rt_v1_wire_encoder_bytes(encoder, *(void *const *)value, error);
}

static int bytes_round_trip(void) {
    DeverRtBuffer error = {0};
    void *text = NULL, *bytes = NULL, *encoded = NULL;
    const char *json = "\"AP+ACg==\"";
    const uint8_t expected[] = {0, 255, 128, 10};
    if (dever_rt_v1_text_new((const uint8_t *)json, strlen(json), &text, &error) != 0) return 20;
    if (dever_rt_v1_wire_decode(text, &bytes_type, bytes_decode, &bytes, &error) != 0) return 21;
    DeverRtBuffer actual = {0};
    if (dever_rt_v1_bytes_export(bytes, &actual) != 0 || actual.len != sizeof(expected) || memcmp(actual.ptr, expected, sizeof(expected)) != 0) return 22;
    error_release(&actual);
    if (dever_rt_v1_wire_encode(&bytes, &bytes_type, bytes_encode, &encoded, &error) != 0 || !text_equals(encoded, json)) return 23;
    dever_rt_v1_text_release(encoded);
    dever_rt_v1_text_release(text);
    bytes_drop(&bytes);
    const char *invalid[] = {"null", "[0,255]", "\"/w\"", "\"/x==\"", "\"_w==\""};
    for (size_t i = 0; i < sizeof(invalid) / sizeof(invalid[0]); ++i) {
        if (dever_rt_v1_text_new((const uint8_t *)invalid[i], strlen(invalid[i]), &text, &error) != 0) return 24;
        if (dever_rt_v1_wire_decode(text, &bytes_type, bytes_decode, &bytes, &error) != 1 || bytes != NULL || error.len == 0) return 25;
        error_release(&error);
        dever_rt_v1_text_release(text);
    }
    return 0;
}

int32_t dever_test_run(void) {
    int bytes_status = bytes_round_trip();
    if (bytes_status != 0) return bytes_status;
    DeverRtBuffer error = {0};
    void *text = NULL;
    Row row = {0};
    void *encoded = NULL;
    const char *json = "{\"title\":\"owned\",\"count\":9223372036854775807}";
    if (dever_rt_v1_text_new((const uint8_t *)json, strlen(json), &text, &error) != 0) return 1;
    calls = 0;
    if (dever_rt_v1_wire_decode(text, &row_type, row_decode, NULL, &error) != 2 || calls != 0) return 2;
    error_release(&error);
    if (dever_rt_v1_wire_decode(text, &row_type, NULL, &row, &error) != 2 || calls != 0) return 3;
    error_release(&error);
    if (dever_rt_v1_wire_decode(text, &row_type, destination_probe, &row, &error) != 0 || row.count != INT64_MAX || !text_equals(row.title, "owned")) return 4;
    calls = 0;
    if (dever_rt_v1_wire_encode(&row, &row_type, row_encode, NULL, &error) != 2 || calls != 0) return 5;
    error_release(&error);
    if (dever_rt_v1_wire_encode(&row, &row_type, row_encode, &encoded, &error) != 0 || calls != 1 || !text_equals(encoded, json)) return 6;
    dever_rt_v1_text_release(encoded);
    row_drop(&row);
    if (dever_rt_v1_wire_decode(text, &row_type, bad_success_decode, &row, &error) != 2 || row.title != NULL) return 9;
    error_release(&error);
    dever_rt_v1_text_release(text);

    const char *invalid[] = {
        "{\"title\":\"partially-owned\",\"count\":\"wrong\"}",
        "{\"title\":\"partially-owned\"}",
        "{\"title\":\"partially-owned\",\"count\":9223372036854775808}",
        "{\"title\":\"one\",\"title\":\"duplicate\",\"count\":1}",
        "{\"title\":\"owned\",\"count\":1,\"extra\":false}",
        "[]",
        "{broken"
    };
    for (size_t i = 0; i < sizeof(invalid) / sizeof(invalid[0]); ++i) {
        if (dever_rt_v1_text_new((const uint8_t *)invalid[i], strlen(invalid[i]), &text, &error) != 0) return 7;
        row = (Row){0};
        if (dever_rt_v1_wire_decode(text, &row_type, row_decode, &row, &error) != 1 || row.title != NULL || error.len == 0) return 8;
        error_release(&error);
        dever_rt_v1_text_release(text);
    }

    const char *command = "{\"record\":{\"title\":\"command-owned\",\"count\":42}}";
    if (dever_rt_v1_text_new((const uint8_t *)command, strlen(command), &text, &error) != 0) return 10;
    if (dever_rt_v1_cmd_decode(text, &row_type, command_decode, &row, &error) != 0 || row.count != 42 || !text_equals(row.title, "command-owned")) return 11;
    row_drop(&row);
    dever_rt_v1_text_release(text);

    const char *bad_commands[] = {
        "{\"record\":{\"title\":\"complete-owned\",\"count\":42},\"unknown\":false}",
        "{\"record\":{\"title\":\"partial-owned\",\"count\":\"bad\"},\"unknown\":false}",
        "{}"
    };
    const char *messages[] = {
        "unknown input 'unknown'", "expected wire integer", "input 'record' is required"
    };
    for (size_t i = 0; i < sizeof(bad_commands) / sizeof(bad_commands[0]); ++i) {
        if (dever_rt_v1_text_new((const uint8_t *)bad_commands[i], strlen(bad_commands[i]), &text, &error) != 0) return 12;
        row = (Row){0};
        if (dever_rt_v1_cmd_decode(text, &row_type, command_decode, &row, &error) != 1 || row.title != NULL || error.len != strlen(messages[i]) || memcmp(error.ptr, messages[i], (size_t)error.len) != 0) return 13;
        error_release(&error);
        dever_rt_v1_text_release(text);
    }
    return 0;
}
