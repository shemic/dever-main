#ifndef DEVER_RUNTIME_ABI_V1_H
#define DEVER_RUNTIME_ABI_V1_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    uint8_t *ptr;
    uint64_t len;
} DeverRtBuffer;

typedef struct {
    uint64_t low;
    uint64_t high;
} DeverRtDecimal;

/* Compiler-emitted callbacks have the C ABI and live for the entire process.
 * They operate on fully initialized values of exactly this concrete type.
 * clone initializes uninitialized destination storage, drop destroys one
 * initialized value, and equal/hash are consistent for Map keys. They must
 * not fail or unwind. hash is optional except for Map key descriptors.
 */
typedef struct {
    uint64_t size;
    uint64_t align;
    void (*clone)(const void *source, void *destination);
    void (*drop)(void *value);
    uint8_t (*equal)(const void *left, const void *right);
    uint64_t (*hash)(const void *value);
} DeverRtType;

/* Callbacks initialize one complete ReadEvent in uninitialized storage.
 * The Bytes/Text handle is borrowed only during the callback; retain it to
 * keep it in the constructed event. All fields live for the process lifetime.
 */
typedef struct {
    const DeverRtType *type;
    void (*chunk)(const void *borrowed_bytes, void *out_event);
    void (*failed)(const void *borrowed_text, void *out_event);
} DeverRtReadEvent;

enum {
    DEVER_RT_OK = 0,
    DEVER_RT_RUNTIME_ERROR = 1,
    DEVER_RT_INVALID_INPUT = 2,
};

/* Internal compiler/runtime ABI, not a Dever source-language FFI.
 * All targets are 64-bit. Inputs are borrowed only until the call returns.
 * A nonzero input length requires that many live immutable bytes; null is
 * accepted only with zero length. Input memory must not overlap an output.
 * Each output pointer must be live, writable, correctly aligned, and distinct
 * from every other output. An out buffer is reset to {NULL, 0} on every call
 * with a valid pointer; release a previous value before reusing that slot.
 * Status 0 initializes the numeric out value or owns the output bytes.
 * Status 1 leaves numeric out unchanged and returns the original runtime
 * error text. Status 2 reports an invalid ABI pointer or length; a null out
 * buffer cannot receive an error message. Do not inspect numeric out on error.
 * Every non-null returned buffer is allocated by the Rust runtime ABI and
 * must be passed once, with its exact length, to dever_rt_v1_buffer_free.
 * Do not use C free or the compiler bridge's C++ allocator for these bytes.
 */
uint32_t dever_rt_v1_version(void);
void dever_rt_v1_buffer_free(uint8_t *ptr, uint64_t len);

uint32_t dever_rt_v1_int_add(int64_t left, int64_t right, int64_t *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_int_sub(int64_t left, int64_t right, int64_t *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_int_mul(int64_t left, int64_t right, int64_t *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_int_div(int64_t left, int64_t right, int64_t *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_int_rem(int64_t left, int64_t right, int64_t *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_int_neg(int64_t input, int64_t *out, DeverRtBuffer *buffer);

uint32_t dever_rt_v1_float_add(double left, double right, double *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_float_sub(double left, double right, double *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_float_mul(double left, double right, double *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_float_div(double left, double right, double *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_float_rem(double left, double right, double *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_float_neg(double input, double *out, DeverRtBuffer *buffer);

uint32_t dever_rt_v1_decimal_from_int(int64_t input, DeverRtDecimal *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_decimal_add(uint64_t left_low, uint64_t left_high, uint64_t right_low, uint64_t right_high, DeverRtDecimal *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_decimal_sub(uint64_t left_low, uint64_t left_high, uint64_t right_low, uint64_t right_high, DeverRtDecimal *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_decimal_mul(uint64_t left_low, uint64_t left_high, uint64_t right_low, uint64_t right_high, DeverRtDecimal *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_decimal_div(uint64_t left_low, uint64_t left_high, uint64_t right_low, uint64_t right_high, DeverRtDecimal *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_decimal_neg(uint64_t low, uint64_t high, DeverRtDecimal *out, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_decimal_round(uint64_t low, uint64_t high, int64_t places, DeverRtDecimal *out, DeverRtBuffer *buffer);

uint32_t dever_rt_v1_int_to_text(int64_t input, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_float_to_text(double input, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_decimal_to_text(uint64_t low, uint64_t high, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_decimal_compare(uint64_t left_low, uint64_t left_high, uint64_t right_low, uint64_t right_high, int32_t *out, DeverRtBuffer *error);
/* Compiler-checked finite Decimal operands only; numeric, not bitwise equality. */
uint8_t dever_rt_v1_decimal_equal(uint64_t left_low, uint64_t left_high, uint64_t right_low, uint64_t right_high);
uint32_t dever_rt_v1_decimal_parse(const void *text, DeverRtDecimal *out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_copy_utf8(const uint8_t *input, uint64_t len, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_bytes_copy(const uint8_t *input, uint64_t len, DeverRtBuffer *buffer);
uint32_t dever_rt_v1_bytes_to_text(const uint8_t *input, uint64_t len, DeverRtBuffer *buffer);
/* Synchronous checked time.sleep: Unit uses one byte (0) on success. Negative
 * duration returns the original runtime error without initializing out.
 */
uint32_t dever_rt_v1_time_sleep(int64_t milliseconds, uint8_t *out, DeverRtBuffer *error);

/* Managed values are private, typed Arc handles. Each returned handle is owned;
 * retain returns another owner and release consumes one. Only retain/release
 * accept NULL as a no-op. Id uses the Text representation and owner functions.
 * All other handle arguments are borrowed unless the symbol ends in _take.
 * A _take call consumes its non-NULL input handle even when it returns error.
 * Source element pointers and descriptor pointers are borrowed, not consumed.
 * A successful element extraction initializes previously uninitialized,
 * aligned output storage with a new owned value; absent output is untouched.
 * The caller must destroy extracted values with its concrete drop callback.
 * Every fallible call resets a valid error buffer; status 1 contains the
 * original runtime error and status 2 reports an invalid ABI argument.
 */
void *dever_rt_v1_text_retain(const void *handle);
void dever_rt_v1_text_release(void *handle);
uint32_t dever_rt_v1_text_new(const uint8_t *input, uint64_t len, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_export(const void *handle, DeverRtBuffer *buffer);
uint8_t dever_rt_v1_text_equal(const void *left, const void *right);
uint64_t dever_rt_v1_text_hash(const void *handle);
uint32_t dever_rt_v1_text_compare(const void *left, const void *right, int32_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_concat(const void *left, const void *right, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_length(const void *handle, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_trim(const void *handle, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_lower(const void *handle, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_upper(const void *handle, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_codepoint(const void *handle, int64_t *out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_from_codepoint(int64_t value, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_at(const void *handle, int64_t index, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_slice(const void *handle, int64_t start, int64_t end, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_index_of(const void *handle, const void *pattern, int64_t *out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_contains(const void *handle, const void *pattern, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_starts_with(const void *handle, const void *pattern, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_ends_with(const void *handle, const void *pattern, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_replace(const void *handle, const void *pattern, const void *replacement, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_text_split(const void *handle, const void *separator, const DeverRtType *text_type, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_int_parse(const void *text, int64_t *out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_float_parse(const void *text, double *out, uint8_t *present, DeverRtBuffer *error);

void *dever_rt_v1_bytes_retain(const void *handle);
void dever_rt_v1_bytes_release(void *handle);
uint32_t dever_rt_v1_bytes_new(const uint8_t *input, uint64_t len, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_bytes_export(const void *handle, DeverRtBuffer *buffer);
uint8_t dever_rt_v1_bytes_equal(const void *left, const void *right);
uint32_t dever_rt_v1_bytes_length(const void *handle, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_bytes_at(const void *handle, int64_t index, int64_t *out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_bytes_concat(const void *left, const void *right, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_bytes_slice(const void *handle, int64_t start, int64_t end, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_bytes_from_text(const void *text, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_bytes_to_text_handle(const void *bytes, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_bytes_from_ints(const void *list, void **out, DeverRtBuffer *error);

/* Arrays of element pointers are borrowed for the duration of construction.
 * count=0 accepts NULL arrays. map_new preserves insertion order and rejects
 * duplicate keys exactly as the existing Map::new implementation does.
 */
void *dever_rt_v1_list_retain(const void *handle);
void dever_rt_v1_list_release(void *handle);
uint32_t dever_rt_v1_list_new(const DeverRtType *element_type, const void *const *values, uint64_t count, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_list_append_take(void *list, const void *element, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_list_first_take(void *list, void *out_element, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_list_length(const void *list, int64_t *out, DeverRtBuffer *error);
uint8_t dever_rt_v1_list_equal(const void *left, const void *right);
uint32_t dever_rt_v1_list_cursor_take(void *list, void **out_cursor, DeverRtBuffer *error);
uint32_t dever_rt_v1_list_cursor_next(void *cursor, void *out_element, uint8_t *present, DeverRtBuffer *error);
void dever_rt_v1_list_cursor_release(void *cursor);

void *dever_rt_v1_map_retain(const void *handle);
void dever_rt_v1_map_release(void *handle);
uint32_t dever_rt_v1_map_new(const DeverRtType *key_type, const DeverRtType *value_type, const void *const *keys, const void *const *values, uint64_t count, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_map_put_take(void *map, const void *key, const void *value, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_map_remove_take(void *map, const void *key, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_map_get_take(void *map, const void *key, void *out_value, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_map_length(const void *map, int64_t *out, DeverRtBuffer *error);
uint8_t dever_rt_v1_map_equal(const void *left, const void *right);
uint32_t dever_rt_v1_map_cursor_take(void *map, void **out_cursor, DeverRtBuffer *error);
uint32_t dever_rt_v1_map_cursor_next(void *cursor, void *out_key, void *out_value, uint8_t *present, DeverRtBuffer *error);
void dever_rt_v1_map_cursor_release(void *cursor);

/* Async values have one concrete compiler-emitted owner. move_init transfers
 * an initialized source into uninitialized destination storage; the caller
 * disarms the source after the callback. Neither callback may unwind.
 */
typedef struct {
    uint64_t size;
    uint64_t align;
    void (*move_init)(void *source, void *destination);
    void (*drop)(void *value);
    uint8_t transferable;
} DeverRtOwnedType;

typedef struct DeverRtPollState DeverRtPollState;
typedef struct DeverRtAsyncOp DeverRtAsyncOp;
typedef struct DeverRtAsyncTask DeverRtAsyncTask;
typedef struct DeverRtAsyncGroup DeverRtAsyncGroup;
typedef struct DeverRtAsyncChannel DeverRtAsyncChannel;

/* create clones borrowed inputs into the coroutine frame before its initial
 * suspension. The frame exclusively owns those copies until destroy. The
 * output/fault slots remain live until the future completes or is destroyed.
 * resume cannot retain the current poll context between calls.
 */
typedef struct {
    const DeverRtOwnedType *output_type;
    const DeverRtOwnedType *fault_type;
    void *(*create)(DeverRtPollState *state, const void *input, void *output, void *fault);
    void (*resume)(void *frame);
    void (*destroy)(void *frame);
    uint8_t (*done)(void *frame);
    uint8_t send_safe;
} DeverRtAsyncFunction;

/* invoke is a checked synchronous function; it runs only in the bounded
 * blocking pool. Non-null operation creation transfers input to the runtime.
 * invoke only borrows that runtime-owned input and must not drop it; the
 * runtime releases it after invoke returns or the operation is cancelled.
 * Status 0 initializes output, 1 initializes fault, and neither callback
 * nor owned-value callback may unwind through this C boundary.
 */
typedef struct {
    const DeverRtOwnedType *input_type;
    const DeverRtOwnedType *output_type;
    const DeverRtOwnedType *fault_type;
    uint32_t (*invoke)(void *input, void *output, void *fault);
    uint8_t send_safe;
} DeverRtSyncFunction;

enum {
    DEVER_RT_ASYNC_PENDING = 0,
    DEVER_RT_ASYNC_READY_OK = 1,
    DEVER_RT_ASYNC_READY_FAULT = 2,
    DEVER_RT_ASYNC_READY_RUNTIME_ERROR = 3,
    DEVER_RT_ASYNC_INVALID_INPUT = 4,
};

/* root starts the one bounded task Scope. Completion status is reported by
 * the coroutine through complete; only the matching ready output is moved.
 * The caller owns output/fault after a matching ready status and error bytes
 * after a runtime error. No output is initialized on Pending or invalid input.
 * op_poll only borrows its operation until op_release. A ready OK moves its
 * concrete output into an uninitialized slot; a ready typed fault moves the
 * concrete fault into an uninitialized slot. For receive, present separately
 * describes Channel exhaustion, even when the element itself is Nullable.
 * Pending initializes none of these slots. The error buffer is reset on each
 * poll and must be freed with dever_rt_v1_buffer_free when non-null.
 *
 * Task and Group retain create another wrapper owner, not another task or
 * group. wait/stop consume one wrapper owner and take the inner affine value
 * exactly once; release consumes one wrapper owner without waiting. Group run
 * borrows a wrapper; Channel retain clones an alias to the same queue. A
 * non-null Channel send operation moves the input value and disarms its old
 * slot before polling, including if delivery later fails or is cancelled.
 */
uint32_t dever_rt_v1_async_root(const DeverRtAsyncFunction *function,
                                const void *input, void *output, void *fault,
                                DeverRtBuffer *error);
void dever_rt_v1_async_complete(DeverRtPollState *state, uint32_t status);
DeverRtAsyncOp *dever_rt_v1_async_sleep(int64_t milliseconds);
DeverRtAsyncOp *dever_rt_v1_async_call(const DeverRtAsyncFunction *function,
                                      const void *input, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_blocking(const DeverRtSyncFunction *function,
                                          void *input, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_parallel(const DeverRtSyncFunction *function,
                                          void *input, DeverRtBuffer *error);
uint32_t dever_rt_v1_async_op_poll(DeverRtPollState *state, DeverRtAsyncOp *operation,
                                  void *output, uint8_t *present, void *fault,
                                  DeverRtBuffer *error);
void dever_rt_v1_async_op_release(DeverRtAsyncOp *operation);
DeverRtAsyncOp *dever_rt_v1_async_task_run(const DeverRtAsyncFunction *function,
                                          const void *input, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_task_run_sync(const DeverRtSyncFunction *function,
                                               void *input, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_task_wait(DeverRtAsyncTask *task);
DeverRtAsyncOp *dever_rt_v1_async_task_stop(DeverRtAsyncTask *task);
DeverRtAsyncTask *dever_rt_v1_async_task_retain(const DeverRtAsyncTask *task);
void dever_rt_v1_async_task_release(DeverRtAsyncTask *task);
uint32_t dever_rt_v1_async_group_new(int64_t limit, DeverRtAsyncGroup **output,
                                     DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_group_run(DeverRtAsyncGroup *group,
                                            const DeverRtAsyncFunction *function,
                                            const void *input, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_group_run_sync(DeverRtAsyncGroup *group,
                                                 const DeverRtSyncFunction *function,
                                                 void *input, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_group_wait(DeverRtAsyncGroup *group);
DeverRtAsyncOp *dever_rt_v1_async_group_stop(DeverRtAsyncGroup *group);
DeverRtAsyncGroup *dever_rt_v1_async_group_retain(const DeverRtAsyncGroup *group);
void dever_rt_v1_async_group_release(DeverRtAsyncGroup *group);
uint32_t dever_rt_v1_async_channel_new(int64_t capacity, DeverRtAsyncChannel **output,
                                       DeverRtBuffer *error);
DeverRtAsyncChannel *dever_rt_v1_async_channel_retain(const DeverRtAsyncChannel *channel);
void dever_rt_v1_async_channel_release(DeverRtAsyncChannel *channel);
DeverRtAsyncOp *dever_rt_v1_async_channel_send(DeverRtAsyncChannel *channel,
                                               const DeverRtOwnedType *element_type,
                                               void *element, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_channel_receive(DeverRtAsyncChannel *channel);
DeverRtAsyncOp *dever_rt_v1_async_channel_close(DeverRtAsyncChannel *channel);
/* Stream constructors borrow their source. Descriptors describe the exact
 * element and must be transferable before a producer may cross a thread.
 * All aliases share one cursor and close state; pull uses the separate present
 * slot (including Nullable rows), and emits a terminal Failed exactly once.
 */
uint32_t dever_rt_v1_async_stream_from_list(const void *list, const DeverRtOwnedType *element_type, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_async_stream_from_bytes(const void *bytes, const DeverRtOwnedType *int_type, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_async_ticks(int64_t milliseconds, const DeverRtOwnedType *int_type, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_async_channel_stream(const DeverRtAsyncChannel *channel, void **out, DeverRtBuffer *error);
void *dever_rt_v1_async_stream_retain(const void *stream);
void dever_rt_v1_async_stream_release(void *stream);
DeverRtAsyncOp *dever_rt_v1_async_stream_pull(const void *stream);
uint32_t dever_rt_v1_async_stream_close(const void *stream, uint8_t *out, DeverRtBuffer *error);

/* TCP operations reuse the runtime's independent read/write locks, deadlines
 * and cancellation. Async operation output is Socket, Listener, Socket,
 * optional Bytes, or Bool respectively. String I/O errors use async status 3.
 * chunks/connections callbacks initialize the original full event Choice;
 * connections uses chunk for the borrowed accepted Socket handle.
 */
DeverRtAsyncOp *dever_rt_v1_async_net_connect(const void *host, int64_t port);
DeverRtAsyncOp *dever_rt_v1_async_net_connect_timeout(const void *host, int64_t port, int64_t milliseconds);
DeverRtAsyncOp *dever_rt_v1_async_net_listen(const void *host, int64_t port);
DeverRtAsyncOp *dever_rt_v1_async_net_accept(const void *listener);
DeverRtAsyncOp *dever_rt_v1_async_net_read(const void *socket, int64_t limit);
DeverRtAsyncOp *dever_rt_v1_async_net_write(const void *socket, const void *bytes);
void *dever_rt_v1_socket_retain(const void *socket);
void dever_rt_v1_socket_release(void *socket);
void *dever_rt_v1_listener_retain(const void *listener);
void dever_rt_v1_listener_release(void *listener);
uint32_t dever_rt_v1_net_port(const void *listener, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_net_timeout(const void *socket, int64_t milliseconds, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_socket_close(const void *socket, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_listener_close(const void *listener, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_net_chunks(const void *socket, int64_t limit, const DeverRtReadEvent *event, const DeverRtOwnedType *element_type, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_net_connections(const void *listener, const DeverRtReadEvent *event, const DeverRtOwnedType *element_type, void **out, DeverRtBuffer *error);

/* wait_timeout consumes one task wrapper, returns the entire task output row
 * with present=1, or present=0 after cancellation and descendant draining.
 * race requires distinct unconsumed tasks of the exact same output type. A
 * non-null result consumes every supplied wrapper; NULL consumes none and
 * reports a creation error. Typed faults remain intact through both paths.
 */
DeverRtAsyncOp *dever_rt_v1_async_task_wait_timeout(DeverRtAsyncTask *task, int64_t milliseconds);
DeverRtAsyncOp *dever_rt_v1_async_task_race(DeverRtAsyncTask *const *tasks, uint64_t count, DeverRtBuffer *error);

/* Exactly one function is present. pack clones borrowed element/context into
 * one complete input row. A null context_type means no context. Descriptors
 * and callbacks are process-static; send_safe covers packed inputs/context.
 * Source kinds: 0 List, 1 Bytes, 2 synchronous Stream, 3 AsyncStream.
 * The synchronous entry accepts only a synchronous handler and kinds 0..2;
 * the async entry accepts kinds 0,1,3. Both preserve runtime dispatch policy.
 * Synchronous completion also uses ASYNC_READY_* statuses (no Pending).
 */
typedef struct {
    const DeverRtAsyncFunction *async_function;
    const DeverRtSyncFunction *sync_function;
    const DeverRtOwnedType *input_type;
    const DeverRtType *context_type;
    void (*pack)(const void *element, const void *context, void *out_input);
} DeverRtParallelFunction;
DeverRtAsyncOp *dever_rt_v1_async_parallel_each(const void *source, uint32_t source_kind, int64_t limit, const DeverRtParallelFunction *function, const void *context, DeverRtBuffer *error);
uint32_t dever_rt_v1_parallel_each(const void *source, uint32_t source_kind, int64_t limit, const DeverRtParallelFunction *function, const void *context, void *fault, DeverRtBuffer *error);
/* Successful allocation is non-null and must be paired with frame_free using
 * the exact coro.size/coro.align. Invalid layout or OOM terminates the process
 * before coro.begin can observe a null allocation.
 */
void *dever_rt_v1_async_frame_alloc(uint64_t size, uint64_t align);
void dever_rt_v1_async_frame_free(void *frame, uint64_t size, uint64_t align);

/* File aliases share one close state. create uses create-new and never replaces
 * an existing path. File read returns absent at EOF. A stream alias shares one
 * cursor; pull returns the terminal Failed event once, then absent. Closing a
 * Stream is idempotent; closing a File twice returns the runtime error text.
 */
void *dever_rt_v1_file_retain(const void *handle);
void dever_rt_v1_file_release(void *handle);
uint32_t dever_rt_v1_file_open(const void *path, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_file_create(const void *path, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_file_read(const void *file, int64_t limit, void **out_bytes, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_file_write(const void *file, const void *bytes, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_file_close(const void *file, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_file_chunks(const void *file, int64_t limit, const DeverRtReadEvent *event, void **out_stream, DeverRtBuffer *error);
void *dever_rt_v1_stream_retain(const void *handle);
void dever_rt_v1_stream_release(void *handle);
uint32_t dever_rt_v1_stream_pull(const void *stream, void *out_element, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_stream_close(const void *stream, uint8_t *out, DeverRtBuffer *error);

uint32_t dever_rt_v1_time_unix_millis(int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_now(int64_t *out, DeverRtBuffer *error);
/* Invocation-owned Clock: virtual time affects Job scheduling and drain only. */
uint32_t dever_rt_v1_job_clock_new(uint8_t test, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_job_clock_now(const void *clock, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_job_clock_advance(const void *clock, int64_t millis, DeverRtBuffer *error);
void dever_rt_v1_job_clock_release(void *clock);
uint32_t dever_rt_v1_time_monotonic_nanos(int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_parse_datetime(const void *text, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_format_datetime(int64_t value, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_parse_date(const void *text, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_format_date(int64_t value, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_parse_time(const void *text, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_format_time(int64_t value, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_add(int64_t left, int64_t right, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_subtract(int64_t left, int64_t right, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_time_difference(int64_t left, int64_t right, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_crypto_token(int64_t length, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_crypto_password_hash(const void *password, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_crypto_password_verify(const void *password, const void *encoded, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_crypto_sha256(const void *bytes, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_crypto_hmac_sha256(const void *secret, const void *bytes, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_crypto_constant_time_eq(const void *left, const void *right, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_secret_from_text(const void *text, void **out, DeverRtBuffer *error);
void *dever_rt_v1_uuid_retain(const void *handle);
void dever_rt_v1_uuid_release(void *handle);
uint8_t dever_rt_v1_uuid_equal(const void *left, const void *right);
uint32_t dever_rt_v1_uuid_parse(const void *text, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_uuid_to_text(const void *uuid, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_process_arguments(const DeverRtType *text_type, void **out, DeverRtBuffer *error);
/* Terminal entry adapters: stderr borrows bytes; test_index consumes no argv
 * storage and writes out only for exactly one unsigned decimal index < count. */
/* Call once from process main before threads; preserves EPIPE diagnostics on
 * Unix. Embedded callable roots must not alter their host's signal policy. */
uint32_t dever_rt_v1_process_init(void);
uint32_t dever_rt_v1_process_stderr(const uint8_t *bytes, uint64_t len);
uint32_t dever_rt_v1_process_test_index(int32_t argc, const char *const *argv, uint64_t count, uint64_t *out);
/* Borrow Text and report through the same configured logger as application errors. */
uint32_t dever_rt_v1_process_error(const void *text);
uint32_t dever_rt_v1_stdout_write(const void *text, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_log_write(uint32_t level, const void *message, const void *fields, uint8_t *out, DeverRtBuffer *error);
void dever_rt_v1_log_flush(void);

/* Protocol records have canonical layouts independent of source declaration order.
 * Inputs borrow handles. Field getters return owned handles. Builders clone inputs.
 * All asynchronous protocol operations use async_op_poll; success is one handle
 * (response/client/socket), Bool byte for mutations, or Unit for servers.
 * Poll validates protocol output slots before advancing I/O. Invalid slots
 * return status 4 without completing or polling the operation; retry is allowed.
 */
typedef struct { int64_t streams, stream_window_bytes, connection_window_bytes; } DeverRtHttp2Limits;
typedef struct { int64_t header_bytes, body_bytes, timeout_ms, connections; const DeverRtHttp2Limits *http2; } DeverRtHttpLimits;
typedef struct { int64_t idle_ms, chunk_bytes, read_ms; } DeverRtHttpPoolLimits;
typedef struct { int64_t chunk_bytes, idle_ms, heartbeat_ms; } DeverRtHttpLiveLimits;
typedef struct { int64_t message_bytes, idle_ms, write_ms; } DeverRtWsLimits;
typedef struct { void *method, *target, *headers, *body; } DeverRtHttpRequestFields;
typedef struct { int64_t status; void *headers, *body; } DeverRtHttpResponseFields;
typedef struct { void *name, *value; } DeverRtHttpHeaderFields;
typedef struct { uint32_t kind; void *payload; } DeverRtWsMessageFields; /* Text=0 Binary=1 Ping=2 Pong=3 */
typedef struct { const void *event, *data, *id; int64_t retry_ms; uint8_t has_retry; } DeverRtSseEvent;
#define DEVER_RT_PROTOCOL_OWNER(name) void *dever_rt_v1_##name##_retain(const void *handle); void dever_rt_v1_##name##_release(void *handle)
DEVER_RT_PROTOCOL_OWNER(http_headers);
DEVER_RT_PROTOCOL_OWNER(http_request);
DEVER_RT_PROTOCOL_OWNER(http_response);
DEVER_RT_PROTOCOL_OWNER(http_stream_request);
DEVER_RT_PROTOCOL_OWNER(http_stream_response);
DEVER_RT_PROTOCOL_OWNER(client_tls);
DEVER_RT_PROTOCOL_OWNER(server_tls);
DEVER_RT_PROTOCOL_OWNER(http_client);
DEVER_RT_PROTOCOL_OWNER(http_reply);
DEVER_RT_PROTOCOL_OWNER(websocket);
DEVER_RT_PROTOCOL_OWNER(ws_message);
DEVER_RT_PROTOCOL_OWNER(secret);
#undef DEVER_RT_PROTOCOL_OWNER
uint32_t dever_rt_v1_http_headers_new(void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_http_headers_push(void *headers, const void *name, const void *value, DeverRtBuffer *error);
uint32_t dever_rt_v1_http_headers_length(const void *headers, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_http_headers_get(const void *headers, int64_t index, DeverRtHttpHeaderFields *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_http_request_new(const DeverRtHttpRequestFields *fields, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_http_request_fields(const void *request, DeverRtHttpRequestFields *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_http_response_new(const DeverRtHttpResponseFields *fields, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_http_response_fields(const void *response, DeverRtHttpResponseFields *out, DeverRtBuffer *error);
/* Stream upload decoder borrows source event; returns 0 Chunk or 1 Failed and
 * initializes an owned Bytes/Text handle. Download uses existing ReadEvent callbacks. */
uint32_t dever_rt_v1_http_stream_request_new(const DeverRtHttpRequestFields *fields, uint32_t (*decode)(const void *, void **), void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_http_stream_response_fields(const void *response, const DeverRtReadEvent *event, const DeverRtOwnedType *type, DeverRtHttpResponseFields *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_tls_system(void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_tls_client(const void *ca, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_tls_server(const void *cert, const void *key, void **out, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_http_send(const void *host, int64_t port, const void *request, const DeverRtHttpLimits *limits);
DeverRtAsyncOp *dever_rt_v1_async_http_client(const void *origin, const void *tls, const DeverRtHttpLimits *limits, const DeverRtHttpPoolLimits *pool);
DeverRtAsyncOp *dever_rt_v1_async_http_request(const void *client, const void *request);
DeverRtAsyncOp *dever_rt_v1_async_http_open(const void *client, const void *request);
DeverRtAsyncOp *dever_rt_v1_async_http_open_stream(const void *client, const void *request);
DeverRtAsyncOp *dever_rt_v1_async_http_close_client(const void *client);
typedef struct {
    const DeverRtAsyncFunction *asynchronous;
    const DeverRtSyncFunction *synchronous;
    const DeverRtOwnedType *input_type;
    const DeverRtType *context_type;
    uint32_t (*pack)(const void *request, const void *reply, const void *context, void *out, DeverRtBuffer *error);
    uint32_t (*response)(const void *output, void **out, DeverRtBuffer *error);
    uint32_t (*fault_message)(const void *fault, void **out, DeverRtBuffer *error);
} DeverRtHttpHandler;
/* live=null means buffered; tls=null means plaintext. Context is cloned before
 * returning. Handler faults remain typed until the terminal request-log callback. */
DeverRtAsyncOp *dever_rt_v1_async_http_serve(const void *listener, const DeverRtHttpLimits *limits, const DeverRtHttpLiveLimits *live, const void *tls, const DeverRtHttpHandler *handler, const void *context, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_http_respond(const void *reply, const void *response);
DeverRtAsyncOp *dever_rt_v1_async_http_start(const void *reply, int64_t status, const void *headers);
DeverRtAsyncOp *dever_rt_v1_async_http_write(const void *reply, const void *bytes);
DeverRtAsyncOp *dever_rt_v1_async_http_finish(const void *reply);
DeverRtAsyncOp *dever_rt_v1_async_sse_start(const void *reply, const void *headers);
DeverRtAsyncOp *dever_rt_v1_async_sse_send(const void *reply, const DeverRtSseEvent *event);
DeverRtAsyncOp *dever_rt_v1_async_ws_accept(const void *reply, const DeverRtWsLimits *limits);
DeverRtAsyncOp *dever_rt_v1_async_ws_connect(const void *host, int64_t port, const void *target, const DeverRtWsLimits *limits);
DeverRtAsyncOp *dever_rt_v1_async_ws_open(const void *url, const void *tls, const DeverRtWsLimits *limits);
uint32_t dever_rt_v1_ws_message_new(uint32_t kind, const void *payload, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_ws_message_fields(const void *message, DeverRtWsMessageFields *out, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_ws_send(const void *socket, const void *message);
/* receive success writes a message handle only when present=1. */
DeverRtAsyncOp *dever_rt_v1_async_ws_receive(const void *socket);
typedef struct {
    const DeverRtType *element_type;
    void (*read)(uint32_t kind, const void *payload, void *out);
    void (*failed)(const void *text, void *out);
} DeverRtWsEvent;
uint32_t dever_rt_v1_ws_messages(const void *socket, const DeverRtWsEvent *event, const DeverRtOwnedType *type, void **out, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_async_ws_close(const void *socket, int64_t code, const void *reason);

/* Synchronous typed wire codecs. Nodes and the encoder are borrowed only for
 * the enclosing callback, including child nodes returned by field/list getters.
 * They have no retain/release operation and must never enter a business value,
 * coroutine frame or other persistent storage. The concrete type is process-
 * static; decode validates its destination before parsing or calling the codec.
 * A decoder initializes out only on success and cleans every partial owner on
 * failure. An encoder only borrows its concrete input. Callbacks return regular
 * status 0/1/2 and own any error Buffer they return, following the common ABI.
 * Text/Uuid/Secret getters return normal owned handles; raw returns exact JSON.
 * Names are borrowed UTF-8 byte slices. Missing optional fields set present=0
 * without initializing out; required fields use the original wire diagnostic.
 */
typedef struct { const uint8_t *ptr; uint64_t len; } DeverRtWireName;

/* runtime-external: compiler metadata is copied into one invocation owner.
 * Resource paths/digests/bytes are immutable for the process lifetime; the
 * descriptor arrays themselves are borrowed only during each call. No host
 * runtime or PATH search is performed. Every invocation stops Workers before
 * cancelling its remaining tasks, then closes application/database resources.
 */
typedef struct {
    DeverRtWireName key, ecosystem, entry, port, adapter, schema;
    const DeverRtWireName *capabilities;
    uint64_t capability_count;
    const DeverRtWireName *operations;
    uint64_t operation_count;
    uint64_t timeout_ms;
} DeverRtExternalDefinition;
typedef struct {
    DeverRtWireName path;
    const uint8_t *bytes;
    uint64_t len;
    DeverRtWireName sha256;
    uint8_t executable;
} DeverRtExternalResource;
uint32_t dever_rt_v1_external_prepare(const DeverRtWireName *digest,
                                     const DeverRtExternalResource *resources,
                                     uint64_t count, DeverRtBuffer *error);
/* start borrows an optional Text setting. call borrows three Text handles.
 * Async poll uses the existing statuses; call success owns one Reply handle.
 */
DeverRtAsyncOp *dever_rt_v1_external_start(const DeverRtExternalDefinition *definition,
                                         const void *setting, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_external_call(const void *key, const void *operation,
                                        const void *payload, DeverRtBuffer *error);
/* Consumes Reply even on failure. Success owns identity and payload Texts.
 * kind=0 is a result (empty identity), kind=1 is a declared-error candidate;
 * the concrete compiler decoder still validates its exact identity/payload.
 */
uint32_t dever_rt_v1_external_reply_take(void *reply, uint8_t *kind,
                                        void **identity, void **payload,
                                        DeverRtBuffer *error);
void dever_rt_v1_external_reply_release(void *reply);
DeverRtAsyncOp *dever_rt_v1_external_shutdown(void);
void dever_rt_v1_external_cause_append(void **cause, const void *message);
uint32_t dever_rt_v1_external_root(const DeverRtAsyncFunction *function,
                                  const void *input, void *output, void *fault,
                                  void (*append_cause)(void *, const void *),
                                  DeverRtBuffer *error);
typedef uint32_t (*DeverRtWireDecode)(const void *node, void *out, DeverRtBuffer *error);
typedef uint32_t (*DeverRtWireEncode)(const void *value, void *encoder, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_decode(const void *text, const DeverRtType *type, DeverRtWireDecode decode, void *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encode(const void *value, const DeverRtType *type, DeverRtWireEncode encode, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_is_null(const void *node, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_bool(const void *node, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_int(const void *node, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_float(const void *node, double *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_text(const void *node, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_bytes(const void *node, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_raw(const void *node, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_decimal(const void *node, DeverRtDecimal *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_uuid(const void *node, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_secret(const void *node, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_datetime(const void *node, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_date(const void *node, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_time(const void *node, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_fields(const void *node, const DeverRtWireName *names, uint64_t count, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_field(const void *node, const DeverRtWireName *name, uint8_t required, const void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_list_len(const void *node, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_node_list_at(const void *node, int64_t index, const void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_null(void *encoder, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_bool(void *encoder, uint8_t value, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_int(void *encoder, int64_t value, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_float(void *encoder, double value, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_text(void *encoder, const void *text, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_bytes(void *encoder, const void *bytes, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_json(void *encoder, const void *text, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_decimal(void *encoder, uint64_t low, uint64_t high, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_uuid(void *encoder, const void *uuid, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_datetime(void *encoder, int64_t value, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_date(void *encoder, int64_t value, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_time(void *encoder, int64_t value, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_begin_array(void *encoder, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_begin_object(void *encoder, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_key(void *encoder, const DeverRtWireName *name, DeverRtBuffer *error);
uint32_t dever_rt_v1_wire_encoder_end(void *encoder, DeverRtBuffer *error);

/* CMD top-level input uses the existing Inputs contract: fields are consumed
 * in checked parameter order, with unknown names rejected after decoding.
 * Inputs is an exclusive callback-scoped borrow and has no owner operations.
 * cmd_input_field returns owned raw-JSON Text; it validates all output slots
 * before removing a field, so a rejected call may be retried. cmd_decode drops
 * a fully initialized concrete output if the final unknown-field check fails.
 * Partial output cleanup on callback failure remains the decoder's duty. */
typedef uint32_t (*DeverRtCmdDecode)(void *inputs, void *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_cmd_decode(const void *text, const DeverRtType *type, DeverRtCmdDecode decode, void *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_cmd_input_field(void *inputs, const DeverRtWireName *name, uint8_t required, void **out, uint8_t *present, DeverRtBuffer *error);

/* Settings are private entry owners, loaded beside the executable through the
 * existing setting.json contract. Select validates every output before lookup;
 * only present=1 initializes the owned setting Text. No unselected Setting is
 * decoded here. The caller releases Settings on every entry exit path. */
uint32_t dever_rt_v1_adapter_settings_load(void **out, DeverRtBuffer *error);
void dever_rt_v1_adapter_settings_release(const void *settings);
uint32_t dever_rt_v1_adapter_settings_validate_names(const void *settings, const DeverRtWireName *names, uint64_t count, DeverRtBuffer *error);
uint32_t dever_rt_v1_adapter_settings_select(const void *settings, const DeverRtWireName *identity, const DeverRtWireName *candidates, uint64_t count, int64_t *index, void **setting, uint8_t *present, DeverRtBuffer *error);

/* Database support is independently bundled with runtime-database and the
 * selected runtime-sqlite/runtime-postgres drivers. Bind rows below are the
 * existing database scalar codec, never source-language business values.
 * All arrays/handles are borrowed through operation creation and copied there.
 * SQL byte strings alone are immutable process-static compiler output.
 * Regular database calls return 0 success, 1 initialized typed fault, 2 ABI
 * error. Async operations use the ordinary async 0/1/2/3/4 contract. Every
 * destination, including the fault slot, is checked before advancing work.
 */
typedef struct {
    const DeverRtOwnedType *fault_type;
    void (*pack)(uint32_t kind, const void *message, void *out_fault);
    void (*append_cause)(void *fault, const void *message);
} DeverRtDbError;
/* Error kinds: Pool=0, PoolExhausted=1, Connection=2, Timeout=3, Cancelled=4,
 * Database=5, Constraint=6, NotFound=7, InvalidData=8, Migration=9. */
typedef struct { DeverRtWireName sqlite, postgres; } DeverRtDbSql;
/* kinds: Null=0, Bool=1, Int=2, Float=3, Decimal=4, Text=5, Bytes=6, Uuid=7.
 * Bool uses integer 0/1; Decimal uses precision/scale plus exact bits; the
 * three handle kinds borrow their normal runtime owners. Other fields ignored. */
typedef struct {
    uint32_t kind, precision, scale;
    int64_t integer;
    double floating;
    DeverRtDecimal decimal;
    const void *handle;
} DeverRtDbValue;
typedef struct {
    const DeverRtWireName *explicit_name;
    DeverRtWireName package_root;
    uint8_t tenant_scoped;
} DeverRtDbBinding;
typedef struct { const DeverRtDbBinding *bindings; uint64_t count; } DeverRtDbTransactionBindings;
typedef struct {
    DeverRtWireName name, type;
    uint8_t nullable, generated;
    const DeverRtWireName *default_value, *rename_from;
} DeverRtDbField;
typedef struct {
    DeverRtWireName name;
    const DeverRtWireName *fields;
    uint64_t count;
    uint8_t unique;
} DeverRtDbIndex;
typedef struct {
    DeverRtWireName name, revision;
    const DeverRtWireName *drops;
    uint64_t count;
} DeverRtDbMigration;
typedef struct {
    DeverRtDbSql sql;
    const DeverRtDbValue *parameters;
    uint64_t count;
} DeverRtDbSeed;
typedef struct {
    DeverRtWireName name;
    uint32_t phase; /* Before=0, After=1 */
    DeverRtDbSeed statement;
} DeverRtDbDataMigration;
typedef struct {
    DeverRtWireName name, definition, sql_type;
    const DeverRtWireName *default_value;
} DeverRtDbPostgresColumn;
typedef struct { DeverRtWireName name, definition; uint8_t foreign_key; } DeverRtDbPostgresConstraint;
typedef struct {
    DeverRtWireName model, table, revision, seed_revision;
    const DeverRtDbField *fields; uint64_t field_count;
    const DeverRtDbIndex *indexes; uint64_t index_count;
    const DeverRtDbMigration *migrations; uint64_t migration_count;
    DeverRtDbSql create_table;
    DeverRtWireName create_temporary_table;
    const DeverRtDbSql *create_indexes; uint64_t create_index_count;
    const DeverRtDbPostgresColumn *postgres_columns; uint64_t postgres_column_count;
    const DeverRtDbPostgresConstraint *postgres_constraints; uint64_t postgres_constraint_count;
    const DeverRtDbSeed *seeds; uint64_t seed_count;
    const DeverRtDbDataMigration *data_migrations; uint64_t data_migration_count;
} DeverRtDbModel;
typedef struct {
    const DeverRtOwnedType *row_type;
    uint32_t (*decode)(const void *row, void *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
} DeverRtDbRowDecoder;
/* Application root closes the published session after child Scope drain and
 * before runtime destruction. The compiler releases/zeros its session and
 * database globals only after this call returns. Cleanup preserves primary
 * faults and uses append_cause; callbacks may not fail or unwind. */
uint32_t dever_rt_v1_db_application_root(const DeverRtAsyncFunction *function, const void *input, void *out, void *fault, void *const *session_slot, const DeverRtDbError *errors, DeverRtBuffer *error);
void dever_rt_v1_db_cause_append(void **cause_text, const void *message);
uint32_t dever_rt_v1_db_session_new(const DeverRtDbBinding *bindings, uint64_t count, const DeverRtDbTransactionBindings *transactions, uint64_t transaction_count, void **out, DeverRtBuffer *error);
void dever_rt_v1_db_session_release(void *session);
DeverRtAsyncOp *dever_rt_v1_db_prepare(const void *session, const DeverRtDbError *errors, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_select(const void *session, const DeverRtDbBinding *binding, void **out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
void *dever_rt_v1_db_retain(const void *database);
void dever_rt_v1_db_release(void *database);
uint32_t dever_rt_v1_db_max_page_size(const void *database, int64_t *out, DeverRtBuffer *error);
/* Startup order matches native: stable sort by resolved database name, migrate
 * every Model, then install foreign keys. Per-model descriptors retain spans. */
DeverRtAsyncOp *dever_rt_v1_db_migrate_models(const void *const *databases, const DeverRtDbModel *models, const DeverRtDbError *const *errors, uint64_t count, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_db_begin(const void *database, const DeverRtDbError *errors, DeverRtBuffer *error);
void *dever_rt_v1_db_transaction_retain(const void *transaction);
void dever_rt_v1_db_transaction_release(void *transaction);
/* finish takes the transaction's shared inner owner on its first poll. The
 * handle itself remains borrowed; success/error/cancellation cannot reuse it. */
DeverRtAsyncOp *dever_rt_v1_db_finish(const void *transaction, uint8_t commit, const DeverRtDbError *errors, DeverRtBuffer *error);
/* On non-NULL creation, moves the complete primary fault into the operation.
 * NULL creation does not consume it. Completion always returns that typed
 * fault, augmented only if rollback fails; cancellation drops it once. */
DeverRtAsyncOp *dever_rt_v1_db_rollback_take(const void *transaction, void *primary_fault, const DeverRtOwnedType *primary_type, const DeverRtDbError *errors, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_db_execute(const void *database, const void *transaction, const DeverRtDbSql *sql, const DeverRtDbValue *parameters, uint64_t count, const DeverRtDbError *errors, DeverRtBuffer *error);
/* maximum=0 selects unbounded query; positive maximum reuses query_bounded. */
DeverRtAsyncOp *dever_rt_v1_db_query(const void *database, const void *transaction, const DeverRtDbSql *sql, const DeverRtDbValue *parameters, uint64_t count, int64_t maximum, const DeverRtDbError *errors, DeverRtBuffer *error);
/* Prefix/before_limit/suffix are process-static SQL fragments. */
DeverRtAsyncOp *dever_rt_v1_db_relation_query(const void *database, const void *transaction, const DeverRtWireName *prefix, const DeverRtWireName *before_limit, const DeverRtWireName *suffix, const int64_t *ids, uint64_t count, int64_t limit, const DeverRtDbError *errors, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_db_stream(const void *database, const void *transaction, const DeverRtDbSql *sql, const DeverRtDbValue *parameters, uint64_t count, int64_t capacity, const DeverRtDbRowDecoder *decoder, const DeverRtDbError *errors, DeverRtBuffer *error);
void *dever_rt_v1_db_stream_retain(const void *stream);
void dever_rt_v1_db_stream_release(void *stream);
uint32_t dever_rt_v1_db_stream_close(const void *stream, uint8_t *out, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_db_stream_pull(const void *stream, const DeverRtDbError *errors, DeverRtBuffer *error);
void dever_rt_v1_db_rows_release(void *rows);
uint32_t dever_rt_v1_db_rows_len(const void *rows, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_rows_at(const void *rows, int64_t index, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_required_row(const void *rows, const DeverRtWireName *operation, void **out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_optional_row(const void *rows, const DeverRtWireName *operation, void **out, uint8_t *present, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
void dever_rt_v1_db_row_release(void *row);
uint32_t dever_rt_v1_db_row_len(const void *row, int64_t *out, DeverRtBuffer *error);
/* Related uses indirection for recursive Model associations. NULL is Unloaded;
 * a non-NULL owner contains one concrete Loaded row, including nullable rows. */
uint32_t dever_rt_v1_db_related_new(const DeverRtType *type, const void *value, void **out, DeverRtBuffer *error);
void *dever_rt_v1_db_related_retain(const void *related);
void dever_rt_v1_db_related_release(void *related);
uint8_t dever_rt_v1_db_related_equal(const void *left, const void *right);
uint32_t dever_rt_v1_db_related_get(const void *related, void *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_row_null(const void *row, int64_t index, uint8_t *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_row_bool(const void *row, int64_t index, uint8_t *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_row_int(const void *row, int64_t index, int64_t *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_row_float(const void *row, int64_t index, double *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_row_decimal(const void *row, int64_t index, DeverRtDecimal *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_row_text(const void *row, int64_t index, void **out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_row_bytes(const void *row, int64_t index, void **out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_row_uuid(const void *row, int64_t index, void **out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_uuid_new(void **out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_error(uint32_t kind, const void *message, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_pagination(int64_t page, int64_t size, int64_t maximum, int64_t *out_page, int64_t *out_size, int64_t *out_offset, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_cursor_size(int64_t size, int64_t maximum, int64_t *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_stream_capacity(int64_t size, int64_t maximum, int64_t *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);
uint32_t dever_rt_v1_db_relation_limit(int64_t maximum, int64_t *out, const DeverRtDbError *errors, void *fault, DeverRtBuffer *error);

/* Application descriptors are compiler-owned process-static metadata. Decode
 * borrows Inputs only for its synchronous call; success initializes a complete
 * typed input, failure drops its own partial input. Runtime finish rejects any
 * trailing fields before calling the concrete handler. Handler success returns
 * exactly one owned Text containing checked JSON. Typed failures remain owned
 * by runtime until the static status callback has inspected them.
 */
typedef struct {
    const DeverRtAsyncFunction *asynchronous;
    const DeverRtSyncFunction *synchronous;
    const DeverRtOwnedType *input_type;
    uint32_t (*decode)(void *inputs, int64_t detail_id, void *out, DeverRtBuffer *error);
    int64_t (*fault_status)(const void *fault);
    /* Optional; only status 400 may expose this borrowed Text as an input error. */
    const void *(*fault_message)(const void *fault);
} DeverRtApiHandler;
typedef struct {
    const DeverRtAsyncFunction *asynchronous;
    const DeverRtSyncFunction *synchronous;
    const DeverRtOwnedType *input_type;
    uint32_t (*pack)(const void *subject, const void *session, const void *nullable_tenant, const void *site, void *out, DeverRtBuffer *error);
    uint32_t (*unpack)(const void *identity, void **id, int64_t *user_id, uint8_t *user_present, int64_t *tenant_id, uint8_t *tenant_present, DeverRtBuffer *error);
    int64_t (*fault_status)(const void *fault);
} DeverRtApiAuth;
typedef struct {
    DeverRtWireName key, component, domain, site, action, method;
} DeverRtApiPermission;
typedef struct {
    DeverRtWireName method, path;
    const DeverRtWireName *directory;
    uint64_t directory_count;
    const DeverRtWireName *components;
    uint64_t component_count;
    const DeverRtApiPermission *permission;
    const DeverRtApiAuth *auth;
    const DeverRtApiHandler *handler;
    uint8_t anonymous, multipart, detail;
} DeverRtApiRoute;
typedef struct {
    const void *path, *domain;
    int64_t same_site; /* 0 Strict, 1 Lax, 2 None */
    uint8_t secure, http_only;
    int64_t max_age;
    uint8_t max_age_present;
} DeverRtApiCookieOptions;
uint32_t dever_rt_v1_api_application_root(const DeverRtAsyncFunction *function, const void *input, void *out, void *fault, void *const *session_slot, void (*append_cause)(void *fault, const void *text), DeverRtBuffer *error);
uint32_t dever_rt_v1_api_session_new(const DeverRtDbBinding *bindings, uint64_t count, const DeverRtDbTransactionBindings *transactions, uint64_t transaction_count, void **out, DeverRtBuffer *error);
void dever_rt_v1_api_session_release(void *session);
uint32_t dever_rt_v1_api_session_database(const void *session, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_has_tenant(const void *session, uint8_t *out, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_initialize(const void *session, const DeverRtWireName *fingerprint, const DeverRtApiPermission *permissions, uint64_t count, uint8_t service_start, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_serve(const void *session, const DeverRtApiRoute *routes, uint64_t count, const DeverRtWireName *tenant_components, uint64_t component_count, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_tenant_scope(int64_t tenant_id, const DeverRtAsyncFunction *function, const void *input, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_require_components(const void *session, const DeverRtWireName *components, uint64_t count, const DeverRtWireName *manifest, uint64_t manifest_count, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_migrate_models(const void *session, const DeverRtDbBinding *bindings, const DeverRtDbModel *models, const DeverRtDbError *const *errors, uint64_t count, int64_t tenant_id, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_migrate_application(const void *session, const DeverRtDbBinding *bindings, const DeverRtDbModel *models, const DeverRtDbError *const *errors, uint64_t count, const DeverRtDbError *fallback_errors, int64_t tenant_id, DeverRtBuffer *error);

/* Job metadata is immutable compiler output. The handler reuses the typed API
 * callable layout, but decode borrows a persisted payload Text and detail_id=0;
 * decode status 1 means invalid payload, and success initializes its concrete
 * input. Job handler success is Unit (size 0, align 1), never a dynamic value.
 * The registry owns its bindings and Clock within one application Session.
 */
typedef struct {
    DeverRtWireName target, schema;
    uint32_t attempts, timeout_ms;
    const DeverRtWireName *schedule;
    const DeverRtDbBinding *binding;
    const DeverRtWireName *components;
    uint64_t component_count;
    const DeverRtApiHandler *handler;
} DeverRtJob;
typedef struct { DeverRtWireName provider; const DeverRtApiAuth *auth; } DeverRtJobAuth;
uint32_t dever_rt_v1_job_session_new(const void *session, const void *clock, const DeverRtJob *jobs, uint64_t count, const DeverRtJobAuth *auth, uint64_t auth_count, const DeverRtApiPermission *permissions, uint64_t permission_count, const DeverRtWireName *components, uint64_t component_count, void **out, DeverRtBuffer *error);
void dever_rt_v1_job_session_release(void *jobs);
uint32_t dever_rt_v1_job_configure(const void *session, uint8_t api, uint8_t worker, DeverRtBuffer *error);
uint32_t dever_rt_v1_job_decode_unit(const void *payload, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_job_initialize(const void *jobs, const DeverRtDbError *errors, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_job_enqueue(const void *jobs, uint64_t index, const void *database, const void *transaction, const void *payload, const void *key, int64_t run_at, const DeverRtDbError *errors, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_job_drain(const void *jobs, int64_t limit, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_job_system_scope(int64_t tenant, uint8_t has_tenant, const DeverRtAsyncFunction *function, const void *input, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_serve_with_jobs(const void *session, const DeverRtApiRoute *routes, uint64_t count, const DeverRtWireName *components, uint64_t component_count, const void *jobs, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_tenant_owner(const void *session, int64_t tenant_id, const void *site, int64_t user_id, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_api_tenant_component(const void *session, int64_t tenant_id, const void *component, uint8_t enabled, const DeverRtWireName *manifest, uint64_t count, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_db_resolve_scoped(const void *session, const DeverRtDbBinding *binding, const DeverRtDbError *errors, DeverRtBuffer *error);
void dever_rt_v1_api_cause_append(void **cause, const void *message);
uint32_t dever_rt_v1_api_input_field(void *inputs, const DeverRtWireName *name, uint8_t required, uint32_t query_kind, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_input_raw_field(void *inputs, const DeverRtWireName *name, uint8_t required, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_input_upload(void *inputs, const DeverRtWireName *name, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_text_bounds(const DeverRtWireName *name, const void *nullable_text, int64_t minimum, int64_t maximum, uint8_t has_maximum, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_pagination(int64_t page, int64_t size, int64_t maximum, int64_t *out_page, int64_t *out_size, int64_t *out_offset, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_request_id(void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_method(void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_path(void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_client_address(void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_header(const void *name, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_cookie(const void *name, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_secret_cookie(const void *name, void **out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_response_header(const void *name, const void *value, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_response_cookie(const void *name, const void *value, const DeverRtApiCookieOptions *options, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_api_response_secret_cookie(const void *name, const void *value, const DeverRtApiCookieOptions *options, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_site_key(void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_id(void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_session(void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_user_id(int64_t *out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_tenant_id(int64_t *out, uint8_t *present, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_owns_user(int64_t user_id, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_issue(const void *subject, const void *session, const void *nullable_tenant, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_issue_cookie(const void *subject, const void *session, const void *nullable_tenant, uint8_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_auth_clear_cookie(uint8_t *out, DeverRtBuffer *error);
typedef void (*DeverRtPackPermission)(const void *key, const void *component, const void *domain, const void *site, const void *action, const void *method, void *out);
DeverRtAsyncOp *dever_rt_v1_auth_permissions(const DeverRtType *element, DeverRtPackPermission pack, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_auth_save_role(const void *id, const void *name, uint8_t all_permissions, const void *permission_keys, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_auth_grant_role(int64_t user_id, const void *role, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_auth_revoke_role(int64_t user_id, const void *role, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_auth_disable_role(const void *role, DeverRtBuffer *error);
void *dever_rt_v1_upload_retain(const void *upload);
void dever_rt_v1_upload_release(void *upload);
uint32_t dever_rt_v1_upload_filename(const void *upload, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_upload_content_type(const void *upload, void **out, DeverRtBuffer *error);
uint32_t dever_rt_v1_upload_size(const void *upload, int64_t *out, DeverRtBuffer *error);
uint32_t dever_rt_v1_upload_close_take(void *upload, uint8_t *out, DeverRtBuffer *error);
DeverRtAsyncOp *dever_rt_v1_upload_store_take(void *upload, DeverRtBuffer *error);

#ifdef __cplusplus
}
#endif

#endif
