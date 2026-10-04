#include "runtime.h"
#include <limits.h>
#include <math.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>

#ifdef DEVER_LLVM_ABI
extern int32_t dever_abi_probe(void);
#endif

/* This harness consumes only the canonical ABI header, never Rust layouts. */
static int expect_buffer(DeverRtBuffer *buffer, const char *expected) {
    const size_t len = strlen(expected);
    const int okay = buffer->len == len &&
        (len == 0 || (buffer->ptr != NULL && memcmp(buffer->ptr, expected, len) == 0));
    dever_rt_v1_buffer_free(buffer->ptr, buffer->len);
    buffer->ptr = NULL;
    buffer->len = 0;
    return okay;
}

#define EXPECT(condition) do { \
    if (!(condition)) { fprintf(stderr, "ABI assertion failed at line %d: %s\n", __LINE__, #condition); return 1; } \
} while (0)

typedef struct {
    void *label;
    int64_t count;
} FixtureRecord;

static void text_clone(const void *source, void *destination) {
    *(void **)destination = dever_rt_v1_text_retain(*(void *const *)source);
}

static void text_drop(void *value) {
    dever_rt_v1_text_release(*(void **)value);
}

static uint8_t text_equal(const void *left, const void *right) {
    return dever_rt_v1_text_equal(*(void *const *)left, *(void *const *)right);
}

static uint64_t text_hash(const void *value) {
    return dever_rt_v1_text_hash(*(void *const *)value);
}

static void record_clone(const void *source, void *destination) {
    const FixtureRecord *record = source;
    *(FixtureRecord *)destination = (FixtureRecord){
        dever_rt_v1_text_retain(record->label), record->count
    };
}

static void record_drop(void *value) {
    dever_rt_v1_text_release(((FixtureRecord *)value)->label);
}

static uint8_t record_equal(const void *left, const void *right) {
    const FixtureRecord *first = left;
    const FixtureRecord *second = right;
    return first->count == second->count &&
        dever_rt_v1_text_equal(first->label, second->label);
}

static const DeverRtType TEXT_TYPE = {
    sizeof(void *), _Alignof(void *), text_clone, text_drop, text_equal, text_hash
};
static const DeverRtType RECORD_TYPE = {
    sizeof(FixtureRecord), _Alignof(FixtureRecord),
    record_clone, record_drop, record_equal, NULL
};

static int managed_values(void) {
    DeverRtBuffer error = {NULL, 0};
    void *label = NULL;
    void *key = NULL;
    void *second_key = NULL;
    EXPECT(dever_rt_v1_text_new((const uint8_t *)"label", 5, &label, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_text_new((const uint8_t *)"first", 5, &key, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_text_new((const uint8_t *)"second", 6, &second_key, &error) == DEVER_RT_OK);
    /* These source views are borrowed; only the ABI's clones own row fields. */
    const FixtureRecord source = {label, 42};
    const void *rows[] = {&source, &source};
    void *list = NULL;
    void *changed = NULL;
    int64_t size = 0;
    EXPECT(dever_rt_v1_list_new(&RECORD_TYPE, rows, 2, &list, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_list_append_take(dever_rt_v1_list_retain(list), &source, &changed, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_list_length(list, &size, &error) == DEVER_RT_OK && size == 2);
    EXPECT(dever_rt_v1_list_length(changed, &size, &error) == DEVER_RT_OK && size == 3);
    FixtureRecord extracted;
    uint8_t present = 0;
    EXPECT(dever_rt_v1_list_first_take(dever_rt_v1_list_retain(list), &extracted, &present, &error) == DEVER_RT_OK && present);
    EXPECT(record_equal(&source, &extracted));
    record_drop(&extracted);
    void *cursor = NULL;
    EXPECT(dever_rt_v1_list_cursor_take(changed, &cursor, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_list_cursor_next(cursor, &extracted, &present, &error) == DEVER_RT_OK && present);
    EXPECT(record_equal(&source, &extracted));
    record_drop(&extracted);
    /* Early cursor release must destroy the unvisited rows too. */
    dever_rt_v1_list_cursor_release(cursor);
    dever_rt_v1_list_release(list);

    const void *keys[] = {&key, &second_key};
    void *map = NULL;
    EXPECT(dever_rt_v1_map_new(&TEXT_TYPE, &RECORD_TYPE, keys, rows, 2, &map, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_map_get_take(dever_rt_v1_map_retain(map), &key, &extracted, &present, &error) == DEVER_RT_OK && present);
    EXPECT(record_equal(&source, &extracted));
    record_drop(&extracted);
    const FixtureRecord replacement = {label, 99};
    EXPECT(dever_rt_v1_map_put_take(dever_rt_v1_map_retain(map), &key, &replacement, &changed, &error) == DEVER_RT_OK);
    EXPECT(!dever_rt_v1_map_equal(map, changed));
    EXPECT(dever_rt_v1_map_get_take(changed, &key, &extracted, &present, &error) == DEVER_RT_OK && present);
    EXPECT(extracted.count == 99);
    record_drop(&extracted);
    EXPECT(dever_rt_v1_map_cursor_take(map, &cursor, &error) == DEVER_RT_OK);
    void *extracted_key = NULL;
    EXPECT(dever_rt_v1_map_cursor_next(cursor, &extracted_key, &extracted, &present, &error) == DEVER_RT_OK && present);
    EXPECT(dever_rt_v1_text_equal(key, extracted_key));
    text_drop(&extracted_key);
    record_drop(&extracted);
    dever_rt_v1_map_cursor_release(cursor);
    const void *duplicate_keys[] = {&key, &key};
    EXPECT(dever_rt_v1_map_new(&TEXT_TYPE, &RECORD_TYPE, duplicate_keys, rows, 2, &map, &error) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&error, "duplicate Map key"));

    void *text_list = NULL;
    void *separator = NULL;
    EXPECT(dever_rt_v1_text_new((const uint8_t *)"a", 1, &separator, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_text_split(label, separator, &TEXT_TYPE, &text_list, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_list_length(text_list, &size, &error) == DEVER_RT_OK && size == 2);
    dever_rt_v1_list_release(text_list);
    dever_rt_v1_text_release(separator);
    void *bytes = NULL;
    void *slice = NULL;
    void *decoded = NULL;
    EXPECT(dever_rt_v1_bytes_from_text(label, &bytes, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_bytes_slice(bytes, 1, 4, &slice, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_bytes_to_text_handle(slice, &decoded, &error) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_text_export(decoded, &error) == DEVER_RT_OK);
    EXPECT(expect_buffer(&error, "abe"));
    dever_rt_v1_text_release(decoded);
    dever_rt_v1_bytes_release(slice);
    dever_rt_v1_bytes_release(bytes);
    dever_rt_v1_text_release(label);
    dever_rt_v1_text_release(key);
    dever_rt_v1_text_release(second_key);
    return 0;
}

int main(void) {
#ifdef SIGPIPE
    EXPECT(signal(SIGPIPE, SIG_DFL) != SIG_ERR);
#endif
    EXPECT(dever_rt_v1_process_init() == DEVER_RT_OK);
#ifdef SIGPIPE
    EXPECT(raise(SIGPIPE) == 0);
#endif
    const char *arguments[] = {"suite", "1", NULL};
    uint64_t selected = 99;
    EXPECT(dever_rt_v1_process_test_index(2, arguments, 2, &selected) == DEVER_RT_OK);
    EXPECT(selected == 1);
    const char *invalid_indices[] = {"", "-1", "+1", " 1", "1x", "2", "18446744073709551616"};
    for (size_t index = 0; index < sizeof(invalid_indices) / sizeof(invalid_indices[0]); index++) {
        arguments[1] = invalid_indices[index];
        selected = 99;
        EXPECT(dever_rt_v1_process_test_index(2, arguments, 2, &selected) == DEVER_RT_INVALID_INPUT);
        EXPECT(selected == 99);
    }
    EXPECT(dever_rt_v1_process_test_index(1, arguments, 2, &selected) == DEVER_RT_INVALID_INPUT);
    EXPECT(dever_rt_v1_process_test_index(2, NULL, 2, &selected) == DEVER_RT_INVALID_INPUT);
    EXPECT(dever_rt_v1_process_stderr(NULL, 0) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_process_stderr(NULL, 1) == DEVER_RT_INVALID_INPUT);
    EXPECT(dever_rt_v1_process_error(NULL) == DEVER_RT_INVALID_INPUT);
    DeverRtBuffer buffer = {NULL, 0};
    int64_t integer = 0;
    double floating = 0.0;
    DeverRtDecimal seven, two, zero, decimal;
    EXPECT(dever_rt_v1_version() == 1);
    EXPECT(dever_rt_v1_int_add(40, 2, &integer, &buffer) == DEVER_RT_OK && integer == 42);
    EXPECT(buffer.ptr == NULL && buffer.len == 0);
    EXPECT(dever_rt_v1_int_sub(8, 3, &integer, &buffer) == DEVER_RT_OK && integer == 5);
    EXPECT(dever_rt_v1_int_mul(7, 6, &integer, &buffer) == DEVER_RT_OK && integer == 42);
    EXPECT(dever_rt_v1_int_div(-7, 3, &integer, &buffer) == DEVER_RT_OK && integer == -2);
    EXPECT(dever_rt_v1_int_rem(-7, 3, &integer, &buffer) == DEVER_RT_OK && integer == -1);
    EXPECT(dever_rt_v1_int_neg(-42, &integer, &buffer) == DEVER_RT_OK && integer == 42);
    EXPECT(dever_rt_v1_int_add(INT64_MAX, 1, &integer, &buffer) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&buffer, "Int overflow"));
    EXPECT(dever_rt_v1_int_div(1, 0, &integer, &buffer) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&buffer, "Int division by zero"));
    EXPECT(dever_rt_v1_int_neg(INT64_MIN, &integer, &buffer) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&buffer, "Int overflow"));
    EXPECT(dever_rt_v1_int_add(1, 2, NULL, &buffer) == DEVER_RT_INVALID_INPUT);
    EXPECT(buffer.ptr != NULL && buffer.len != 0);
    dever_rt_v1_buffer_free(buffer.ptr, buffer.len);
    buffer = (DeverRtBuffer){NULL, 0};
    EXPECT(dever_rt_v1_int_add(1, 2, &integer, NULL) == DEVER_RT_INVALID_INPUT);

    EXPECT(dever_rt_v1_float_div(0.0, 0.0, &floating, &buffer) == DEVER_RT_OK && isnan(floating));
    EXPECT(dever_rt_v1_float_div(1.0, 0.0, &floating, &buffer) == DEVER_RT_OK && isinf(floating));
    EXPECT(dever_rt_v1_float_add(1.5, 2.5, &floating, &buffer) == DEVER_RT_OK && floating == 4.0);
    EXPECT(dever_rt_v1_float_rem(7.0, 3.0, &floating, &buffer) == DEVER_RT_OK && floating == 1.0);
    EXPECT(dever_rt_v1_float_neg(0.0, &floating, &buffer) == DEVER_RT_OK && signbit(floating));
    EXPECT(dever_rt_v1_float_to_text(3.0, &buffer) == DEVER_RT_OK);
    EXPECT(expect_buffer(&buffer, "3"));

    EXPECT(dever_rt_v1_decimal_from_int(7, &seven, &buffer) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_decimal_from_int(2, &two, &buffer) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_decimal_from_int(0, &zero, &buffer) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_decimal_div(seven.low, seven.high, two.low, two.high, &decimal, &buffer) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_decimal_to_text(decimal.low, decimal.high, &buffer) == DEVER_RT_OK);
    EXPECT(expect_buffer(&buffer, "3.5"));
    EXPECT(dever_rt_v1_decimal_round(decimal.low, decimal.high, 0, &decimal, &buffer) == DEVER_RT_OK);
    EXPECT(dever_rt_v1_decimal_to_text(decimal.low, decimal.high, &buffer) == DEVER_RT_OK);
    EXPECT(expect_buffer(&buffer, "4"));
    EXPECT(dever_rt_v1_decimal_div(seven.low, seven.high, zero.low, zero.high, &decimal, &buffer) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(expect_buffer(&buffer, "Decimal division by zero"));
    EXPECT(dever_rt_v1_int_to_text(INT64_MIN, &buffer) == DEVER_RT_OK);
    EXPECT(expect_buffer(&buffer, "-9223372036854775808"));

    const uint8_t text[] = {0x61, 0xe4, 0xb8, 0xad};
    EXPECT(dever_rt_v1_text_copy_utf8(text, sizeof(text), &buffer) == DEVER_RT_OK);
    EXPECT(expect_buffer(&buffer, "a\xe4\xb8\xad"));
    const uint8_t bytes[] = {0, 255, 42};
    EXPECT(dever_rt_v1_bytes_copy(bytes, sizeof(bytes), &buffer) == DEVER_RT_OK);
    EXPECT(buffer.len == sizeof(bytes) && memcmp(buffer.ptr, bytes, sizeof(bytes)) == 0);
    dever_rt_v1_buffer_free(buffer.ptr, buffer.len);
    buffer = (DeverRtBuffer){NULL, 0};
    EXPECT(dever_rt_v1_bytes_to_text(text, sizeof(text), &buffer) == DEVER_RT_OK);
    EXPECT(expect_buffer(&buffer, "a\xe4\xb8\xad"));
    const uint8_t invalid[] = {255};
    EXPECT(dever_rt_v1_bytes_to_text(invalid, sizeof(invalid), &buffer) == DEVER_RT_RUNTIME_ERROR);
    EXPECT(buffer.ptr != NULL && buffer.len != 0);
    dever_rt_v1_buffer_free(buffer.ptr, buffer.len);
    buffer = (DeverRtBuffer){NULL, 0};
    EXPECT(dever_rt_v1_text_copy_utf8(NULL, 0, &buffer) == DEVER_RT_OK);
    EXPECT(buffer.ptr == NULL && buffer.len == 0);
    EXPECT(dever_rt_v1_bytes_copy(NULL, 1, &buffer) == DEVER_RT_INVALID_INPUT);
    EXPECT(buffer.ptr != NULL && buffer.len != 0);
    dever_rt_v1_buffer_free(buffer.ptr, buffer.len);
    dever_rt_v1_buffer_free(NULL, 0);
#ifdef DEVER_LLVM_ABI
    EXPECT(dever_abi_probe() == 0);
#endif
    EXPECT(managed_values() == 0);
    puts("native runtime ABI verified");
    return 0;
}
