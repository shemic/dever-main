/* Database ABI ownership without settings, pools or database connections.
 * The existing managed-driver supplies warm 1 + 64 and the strict ledger. */
#include "runtime.h"

#include <stdint.h>
#include <string.h>

typedef struct {
    uint32_t kind;
    void *message;
    void *cause;
} Fault;

typedef struct {
    uint8_t present;
    void *text;
} OptionalText;

static unsigned packs;
static unsigned moves;
static unsigned drops;

static void release_buffer(DeverRtBuffer *buffer) {
    dever_rt_v1_buffer_free(buffer->ptr, buffer->len);
    *buffer = (DeverRtBuffer){0};
}

static int buffer_is(const DeverRtBuffer *buffer, const char *expected) {
    size_t length = strlen(expected);
    return buffer->ptr && buffer->len == length &&
           memcmp(buffer->ptr, expected, length) == 0;
}

static int text_is(const void *text, const char *expected) {
    DeverRtBuffer buffer = {0};
    uint32_t status = dever_rt_v1_text_export(text, &buffer);
    int matches = status == 0 && buffer_is(&buffer, expected);
    release_buffer(&buffer);
    return matches;
}

static int new_text(const char *text, void **out) {
    DeverRtBuffer buffer = {0};
    uint32_t status = dever_rt_v1_text_new(
        (const uint8_t *)text, strlen(text), out, &buffer);
    release_buffer(&buffer);
    return status == 0;
}

static void move_fault(void *source, void *destination) {
    *(Fault *)destination = *(Fault *)source;
    *(Fault *)source = (Fault){0};
    ++moves;
}

static void drop_fault(void *value) {
    Fault *fault = value;
    dever_rt_v1_text_release(fault->message);
    dever_rt_v1_text_release(fault->cause);
    *fault = (Fault){0};
    ++drops;
}

static void pack_fault(uint32_t kind, const void *message, void *out) {
    *(Fault *)out = (Fault){kind, dever_rt_v1_text_retain(message), NULL};
    ++packs;
}

static void append_cause(void *value, const void *message) {
    dever_rt_v1_db_cause_append(&((Fault *)value)->cause, message);
}

static const DeverRtOwnedType fault_type = {
    sizeof(Fault), _Alignof(Fault), move_fault, drop_fault, 1
};
static const DeverRtDbError errors = {&fault_type, pack_fault, append_cause};

static void clone_optional(const void *source, void *destination) {
    const OptionalText *from = source;
    *(OptionalText *)destination = (OptionalText){
        from->present, from->present ? dever_rt_v1_text_retain(from->text) : NULL
    };
}

static void drop_optional(void *value) {
    OptionalText *text = value;
    if (text->present) dever_rt_v1_text_release(text->text);
    *text = (OptionalText){0};
}

static uint8_t equal_optional(const void *left, const void *right) {
    const OptionalText *a = left;
    const OptionalText *b = right;
    return a->present == b->present &&
           (!a->present || dever_rt_v1_text_equal(a->text, b->text));
}

static const DeverRtType optional_type = {
    sizeof(OptionalText), _Alignof(OptionalText),
    clone_optional, drop_optional, equal_optional, NULL
};

/* Preserve cleanup on an assertion failure so the first diagnostic identifies
 * the broken contract, not a second error caused by the test's own resources. */
#define CHECK(condition) do { if (!(condition)) { result = __LINE__; goto cleanup; } } while (0)

static int error_kinds(void) {
    int result = 0;
    DeverRtBuffer buffer = {0};
    Fault fault = {0};
    void *message = NULL;
    unsigned before_packs = packs;
    unsigned before_moves = moves;
    unsigned before_drops = drops;
    for (uint32_t kind = 0; kind < 10; ++kind) {
        CHECK(new_text("original database diagnostic: \xe6\x95\xb0\xe6\x8d\xae", &message));
        CHECK(dever_rt_v1_db_error(kind, message, &errors, &fault, &buffer) == 1);
        CHECK(buffer.ptr == NULL && buffer.len == 0 && fault.kind == kind);
        dever_rt_v1_text_release(message);
        message = NULL;
        /* pack's borrowed handle is retainable after the bridge returns. */
        CHECK(text_is(fault.message, "original database diagnostic: \xe6\x95\xb0\xe6\x8d\xae"));
        CHECK(new_text("rollback failed", &message));
        append_cause(&fault, message);
        append_cause(&fault, message);
        dever_rt_v1_text_release(message);
        message = NULL;
        CHECK(fault.kind == kind && text_is(fault.cause,
            "caused by: rollback failed; caused by: rollback failed"));
        drop_fault(&fault);
    }
    CHECK(packs == before_packs + 10 && moves == before_moves + 10 &&
          drops == before_drops + 10);
cleanup:
    dever_rt_v1_text_release(message);
    if (fault.message || fault.cause) drop_fault(&fault);
    release_buffer(&buffer);
    return result;
}

static int related_owners(void) {
    int result = 0;
    DeverRtBuffer buffer = {0};
    OptionalText source = {1, NULL}, extracted = {0}, absent = {0};
    void *related = NULL, *alias = NULL, *loaded_null = NULL, *second_null = NULL;
    CHECK(new_text("related-owned", &source.text));
    CHECK(dever_rt_v1_db_related_new(&optional_type, &source, &related, &buffer) == 0);
    alias = dever_rt_v1_db_related_retain(related);
    drop_optional(&source);
    dever_rt_v1_db_related_release(related);
    related = NULL;
    CHECK(dever_rt_v1_db_related_get(alias, &extracted, &buffer) == 0);
    dever_rt_v1_db_related_release(alias);
    alias = NULL;
    CHECK(extracted.present == 1 && text_is(extracted.text, "related-owned"));
    CHECK(dever_rt_v1_db_related_new(&optional_type, &absent, &loaded_null, &buffer) == 0);
    CHECK(dever_rt_v1_db_related_new(&optional_type, &absent, &second_null, &buffer) == 0);
    CHECK(loaded_null != NULL && dever_rt_v1_db_related_equal(loaded_null, second_null) == 1);
    CHECK(dever_rt_v1_db_related_equal(NULL, loaded_null) == 0);
    CHECK(dever_rt_v1_db_related_equal(NULL, NULL) == 1);
    drop_optional(&extracted);
    CHECK(dever_rt_v1_db_related_get(loaded_null, &extracted, &buffer) == 0);
    CHECK(extracted.present == 0 && extracted.text == NULL);
cleanup:
    drop_optional(&source);
    drop_optional(&extracted);
    dever_rt_v1_db_related_release(related);
    dever_rt_v1_db_related_release(alias);
    dever_rt_v1_db_related_release(loaded_null);
    dever_rt_v1_db_related_release(second_null);
    release_buffer(&buffer);
    return result;
}

static int invalid_destinations(void) {
    int result = 0;
    DeverRtBuffer buffer = {0};
    Fault fault = {37, NULL, NULL};
    int64_t output = 41;
    unsigned before = packs;
    CHECK(dever_rt_v1_db_cursor_size(0, 7, NULL, &errors, &fault, &buffer) == 2);
    CHECK(buffer_is(&buffer, "invalid ABI output pointer") && packs == before && fault.kind == 37);
    release_buffer(&buffer);
    _Alignas(int64_t) unsigned char storage[sizeof(int64_t) + 1];
    memset(storage, 53, sizeof(storage));
    CHECK(dever_rt_v1_db_stream_capacity(0, 7, (int64_t *)(storage + 1), &errors, &fault, &buffer) == 2);
    CHECK(packs == before && fault.kind == 37);
    for (size_t i = 0; i < sizeof(storage); ++i) CHECK(storage[i] == 53);
    release_buffer(&buffer);
    CHECK(dever_rt_v1_db_cursor_size(0, 7, &output, &errors, NULL, &buffer) == 2);
    CHECK(output == 41 && packs == before && buffer_is(&buffer, "invalid database fault output"));
    release_buffer(&buffer);
    CHECK(dever_rt_v1_db_cursor_size(0, 7, &output, &errors, &fault, &buffer) == 1);
    CHECK(output == 41 && fault.kind == 8 && text_is(fault.message, "cursor size must be between 1 and 7"));
    drop_fault(&fault);
    CHECK(dever_rt_v1_db_stream_capacity(0, 7, &output, &errors, &fault, &buffer) == 1);
    CHECK(output == 41 && fault.kind == 8 && text_is(fault.message, "stream buffer size must be between 1 and 7"));
    drop_fault(&fault);
    CHECK(dever_rt_v1_db_cursor_size(3, 7, &output, &errors, &fault, &buffer) == 0 && output == 3);
    CHECK(packs == before + 2 && buffer.ptr == NULL && buffer.len == 0);
cleanup:
    if (fault.message || fault.cause) drop_fault(&fault);
    release_buffer(&buffer);
    return result;
}

static int invalid_constructors(void) {
    int result = 0;
    DeverRtBuffer buffer = {0};
    Fault primary = {6, NULL, NULL};
    DeverRtAsyncOp *operation = NULL;
    void *session = (void *)(uintptr_t)47;
    unsigned before_moves = moves, before_drops = drops;
    CHECK(new_text("primary-owned", &primary.message));
    operation = dever_rt_v1_db_rollback_take(NULL, &primary, &fault_type, &errors, &buffer);
    CHECK(operation == NULL && buffer_is(&buffer, "invalid ABI handle"));
    CHECK(moves == before_moves && drops == before_drops && primary.kind == 6 && text_is(primary.message, "primary-owned"));
    release_buffer(&buffer);

    const DeverRtDbBinding bad_flag = {NULL, {(const uint8_t *)"fixture", 7}, 2};
    CHECK(dever_rt_v1_db_session_new(&bad_flag, 1, NULL, 0, NULL, &buffer) == 2);
    CHECK(buffer_is(&buffer, "invalid ABI output pointer"));
    release_buffer(&buffer);
    /* Metadata rejects before looking for config/setting.json or opening pools. */
    CHECK(dever_rt_v1_db_session_new(&bad_flag, 1, NULL, 0, &session, &buffer) == 2);
    CHECK(session == (void *)(uintptr_t)47 && buffer_is(&buffer, "invalid database ABI boolean"));
    release_buffer(&buffer);
    _Alignas(DeverRtDbBinding) unsigned char unaligned[sizeof(DeverRtDbBinding) + 1];
    CHECK(dever_rt_v1_db_session_new((const DeverRtDbBinding *)(unaligned + 1), 1, NULL, 0, &session, &buffer) == 2);
    CHECK(session == (void *)(uintptr_t)47 && buffer_is(&buffer, "invalid database ABI array"));
cleanup:
    dever_rt_v1_async_op_release(operation);
    drop_fault(&primary);
    release_buffer(&buffer);
    return result;
}

int32_t dever_test_run(void) {
    packs = moves = drops = 0;
    int result = error_kinds();
    if (result == 0) result = related_owners();
    if (result == 0) result = invalid_destinations();
    if (result == 0) result = invalid_constructors();
    return result;
}
