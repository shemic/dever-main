#define _POSIX_C_SOURCE 200809L
#include "runtime.h"

#include <stdint.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static _Atomic int unit_destroy_count;
static _Atomic int blocking_done_count;
static _Atomic int group_sync_done_count;

typedef struct {
    DeverRtPollState *poll;
    void *output;
    void *fault;
    DeverRtAsyncOp *operation;
    DeverRtAsyncOp *blocked_send;
    DeverRtAsyncTask *task;
    DeverRtAsyncGroup *group;
    DeverRtAsyncChannel *channel;
    uint32_t phase;
    uint8_t done;
    uint8_t pending;
    uint8_t group_pending;
} Frame;

static void move_int(void *source, void *destination) {
    *(int64_t *)destination = *(int64_t *)source;
    *(int64_t *)source = 0;
}

static void drop_int(void *value) { (void)value; }
static void move_unit(void *source, void *destination) { (void)source; (void)destination; }
static void drop_unit(void *value) { (void)value; }

typedef struct {
    uint8_t present;
    int64_t value;
} OptionalInt;

static void move_optional(void *source, void *destination) {
    OptionalInt *old = source;
    OptionalInt *new_value = destination;
    new_value->present = old->present;
    new_value->value = old->value;
    old->present = 0;
    old->value = 0;
}

static const DeverRtOwnedType integer = { 8, 8, move_int, drop_int, 1 };
static const DeverRtOwnedType unit = { 0, 1, move_unit, drop_unit, 1 };
static const DeverRtOwnedType optional_integer = {
    sizeof(OptionalInt), _Alignof(OptionalInt), move_optional, drop_int, 1
};

static void *create(DeverRtPollState *poll, const void *input, void *output, void *fault) {
    (void)input;
    Frame *frame = calloc(1, sizeof(*frame));
    if (frame) {
        frame->poll = poll;
        frame->output = output;
        frame->fault = fault;
    }
    return frame;
}

static void destroy(void *pointer) {
    Frame *frame = pointer;
    if (frame->operation) dever_rt_v1_async_op_release(frame->operation);
    if (frame->blocked_send) dever_rt_v1_async_op_release(frame->blocked_send);
    if (frame->task) dever_rt_v1_async_task_release(frame->task);
    if (frame->group) dever_rt_v1_async_group_release(frame->group);
    if (frame->channel) dever_rt_v1_async_channel_release(frame->channel);
    free(frame);
}

static void destroy_unit(void *pointer) {
    atomic_fetch_add(&unit_destroy_count, 1);
    destroy(pointer);
}

static uint8_t done(void *pointer) { return ((Frame *)pointer)->done; }

static void complete(Frame *frame, uint32_t status) {
    frame->done = 1;
    dever_rt_v1_async_complete(frame->poll, status);
}

static void resume_child(void *pointer, uint8_t has_value, int64_t delay) {
    Frame *frame = pointer;
    DeverRtBuffer error = { 0 };
    if (!frame->operation) frame->operation = dever_rt_v1_async_sleep(delay);
    uint32_t status = dever_rt_v1_async_op_poll(frame->poll, frame->operation,
                                                 NULL, NULL, NULL, &error);
    if (status == DEVER_RT_ASYNC_PENDING) return;
    if (status != DEVER_RT_ASYNC_READY_OK) abort();
    dever_rt_v1_async_op_release(frame->operation);
    frame->operation = NULL;
    if (has_value) *(int64_t *)frame->output = 7;
    complete(frame, DEVER_RT_ASYNC_READY_OK);
}

static void value_resume(void *pointer) { resume_child(pointer, 1, 2); }
static void unit_resume(void *pointer) { resume_child(pointer, 0, 1000); }
static void fault_resume(void *pointer) {
    Frame *frame = pointer;
    *(int64_t *)frame->fault = 99;
    complete(frame, DEVER_RT_ASYNC_READY_FAULT);
}

static const DeverRtAsyncFunction value_child = {
    &integer, &integer, create, value_resume, destroy, done, 1
};
static const DeverRtAsyncFunction unit_child = {
    &unit, &integer, create, unit_resume, destroy_unit, done, 1
};
static const DeverRtAsyncFunction fault_child = {
    &unit, &integer, create, fault_resume, destroy, done, 1
};

static uint32_t twice(void *input, void *output, void *fault) {
    (void)fault;
    struct timespec delay = { 0, 80000000 };
    nanosleep(&delay, NULL);
    *(int64_t *)output = *(int64_t *)input * 2;
    atomic_fetch_add(&blocking_done_count, 1);
    return 0;
}

static const DeverRtSyncFunction synchronous_twice = {
    &integer, &integer, &integer, twice, 1
};

static uint32_t group_sync(void *input, void *output, void *fault) {
    (void)output;
    (void)fault;
    if (*(int64_t *)input != 21) abort();
    atomic_fetch_add(&group_sync_done_count, 1);
    return 0;
}

static const DeverRtSyncFunction synchronous_unit = {
    &integer, &unit, &integer, group_sync, 1
};

static uint32_t poll(Frame *frame, DeverRtAsyncOp *operation, void *output,
                     uint8_t *present, void *fault) {
    DeverRtBuffer error = { 0 };
    uint32_t status = dever_rt_v1_async_op_poll(frame->poll, operation,
                                                 output, present, fault, &error);
    if (status == DEVER_RT_ASYNC_PENDING) frame->pending = 1;
    if (error.ptr) dever_rt_v1_buffer_free(error.ptr, error.len);
    return status;
}

static void root_resume(void *pointer) {
    Frame *frame = pointer;
    DeverRtBuffer error = { 0 };
    uint32_t status;
    int64_t item;
    int64_t fault_value;
    OptionalInt optional;
    uint8_t present;
    for (;;) {
        switch (frame->phase) {
        case 0:
            frame->operation = dever_rt_v1_async_task_run(&value_child, NULL, &error);
            if (!frame->operation) abort();
            frame->phase = 1;
            break;
        case 1:
            status = poll(frame, frame->operation, &frame->task, NULL, NULL);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || !frame->task) abort();
            dever_rt_v1_async_op_release(frame->operation);
            DeverRtAsyncTask *task_alias = dever_rt_v1_async_task_retain(frame->task);
            if (!task_alias) abort();
            frame->operation = dever_rt_v1_async_task_wait(frame->task);
            frame->task = NULL;
            dever_rt_v1_async_task_release(task_alias);
            frame->phase = 2;
            break;
        case 2:
            status = poll(frame, frame->operation, &item, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || item != 7 || !frame->pending) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = NULL;
            frame->operation = dever_rt_v1_async_task_run(&unit_child, NULL, &error);
            if (!frame->operation) abort();
            frame->phase = 12;
            break;
        case 12:
            status = poll(frame, frame->operation, &frame->task, NULL, NULL);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || !frame->task) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_task_stop(frame->task);
            frame->task = NULL;
            frame->phase = 13;
            break;
        case 13:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || atomic_load(&unit_destroy_count) < 1) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_task_run(&fault_child, NULL, &error);
            if (!frame->operation) abort();
            frame->phase = 14;
            break;
        case 14:
            status = poll(frame, frame->operation, &frame->task, NULL, NULL);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || !frame->task) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_task_wait(frame->task);
            frame->task = NULL;
            frame->phase = 15;
            break;
        case 15:
            status = poll(frame, frame->operation, NULL, NULL, &fault_value);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_FAULT || fault_value != 99) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = NULL;
            if (dever_rt_v1_async_group_new(1, &frame->group, &error) != DEVER_RT_ASYNC_PENDING) abort();
            frame->operation = dever_rt_v1_async_group_run(frame->group, &unit_child, NULL, &error);
            if (!frame->operation) abort();
            frame->phase = 3;
            break;
        case 3:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_group_run(frame->group, &unit_child, NULL, &error);
            if (!frame->operation) abort();
            frame->phase = 4;
            break;
        case 4:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status != DEVER_RT_ASYNC_PENDING) abort();
            frame->group_pending = 1;
            dever_rt_v1_async_op_release(frame->operation);
            DeverRtAsyncGroup *group_alias = dever_rt_v1_async_group_retain(frame->group);
            if (!group_alias) abort();
            frame->operation = dever_rt_v1_async_group_stop(frame->group);
            frame->group = NULL;
            dever_rt_v1_async_group_release(group_alias);
            frame->phase = 5;
            break;
        case 5:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || atomic_load(&unit_destroy_count) < 2) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = NULL;
            if (dever_rt_v1_async_group_new(1, &frame->group, &error) != DEVER_RT_ASYNC_PENDING) abort();
            frame->operation = dever_rt_v1_async_group_run(frame->group, &fault_child, NULL, &error);
            if (!frame->operation) abort();
            frame->phase = 18;
            break;
        case 18:
            status = poll(frame, frame->operation, NULL, NULL, &fault_value);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_group_wait(frame->group);
            frame->group = NULL;
            frame->phase = 19;
            break;
        case 19:
            status = poll(frame, frame->operation, NULL, NULL, &fault_value);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_FAULT || fault_value != 99) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = NULL;
            if (dever_rt_v1_async_channel_new(1, &frame->channel, &error) != DEVER_RT_ASYNC_PENDING) abort();
            item = 11;
            frame->operation = dever_rt_v1_async_channel_send(frame->channel, &integer, &item, &error);
            if (!frame->operation || item != 0) abort();
            frame->phase = 6;
            break;
        case 6:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->operation);
            item = 12;
            frame->blocked_send = dever_rt_v1_async_channel_send(frame->channel, &integer, &item, &error);
            if (!frame->blocked_send || item != 0) abort();
            if (poll(frame, frame->blocked_send, NULL, NULL, frame->fault) != DEVER_RT_ASYNC_PENDING) abort();
            frame->operation = dever_rt_v1_async_channel_receive(frame->channel);
            frame->phase = 7;
            break;
        case 7:
            status = poll(frame, frame->operation, &item, &present, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK || !present || item != 11) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = NULL;
            status = poll(frame, frame->blocked_send, NULL, NULL, frame->fault);
            frame->phase = 11;
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->blocked_send);
            frame->blocked_send = NULL;
            frame->operation = dever_rt_v1_async_channel_close(frame->channel);
            frame->phase = 8;
            break;
        case 11:
            status = poll(frame, frame->blocked_send, NULL, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->blocked_send);
            frame->blocked_send = NULL;
            frame->operation = dever_rt_v1_async_channel_close(frame->channel);
            frame->phase = 8;
            break;
        case 8:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_channel_receive(frame->channel);
            frame->phase = 9;
            break;
        case 9:
            status = poll(frame, frame->operation, &item, &present, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK || !present || item != 12) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_channel_receive(frame->channel);
            frame->phase = 10;
            break;
        case 10:
            status = poll(frame, frame->operation, &item, &present, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK || present) abort();
            if (!frame->group_pending) abort();
            dever_rt_v1_async_op_release(frame->operation);
            dever_rt_v1_async_channel_release(frame->channel);
            frame->channel = NULL;
            if (dever_rt_v1_async_channel_new(1, &frame->channel, &error) != DEVER_RT_ASYNC_PENDING) abort();
            optional.present = 0;
            optional.value = 0;
            frame->operation = dever_rt_v1_async_channel_send(frame->channel, &optional_integer, &optional, &error);
            if (!frame->operation) abort();
            frame->phase = 24;
            break;
        case 24:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_channel_close(frame->channel);
            frame->phase = 25;
            break;
        case 25:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_channel_receive(frame->channel);
            frame->phase = 26;
            break;
        case 26:
            status = poll(frame, frame->operation, &optional, &present, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK || !present || optional.present) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_channel_receive(frame->channel);
            frame->phase = 27;
            break;
        case 27:
            status = poll(frame, frame->operation, &optional, &present, frame->fault);
            if (status != DEVER_RT_ASYNC_READY_OK || present) abort();
            dever_rt_v1_async_op_release(frame->operation);
            dever_rt_v1_async_channel_release(frame->channel);
            frame->channel = NULL;
            item = 21;
            frame->operation = dever_rt_v1_async_parallel(&synchronous_twice, &item, &error);
            if (!frame->operation || item != 0) abort();
            frame->phase = 16;
            break;
        case 16:
            status = poll(frame, frame->operation, &item, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || item != 42) abort();
            dever_rt_v1_async_op_release(frame->operation);
            item = 21;
            frame->operation = dever_rt_v1_async_task_run_sync(&synchronous_twice, &item, &error);
            if (!frame->operation || item != 0) abort();
            frame->phase = 20;
            break;
        case 20:
            status = poll(frame, frame->operation, &frame->task, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || !frame->task) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_task_wait(frame->task);
            frame->task = NULL;
            frame->phase = 21;
            break;
        case 21:
            status = poll(frame, frame->operation, &item, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || item != 42) abort();
            dever_rt_v1_async_op_release(frame->operation);
            if (dever_rt_v1_async_group_new(1, &frame->group, &error) != DEVER_RT_ASYNC_PENDING) abort();
            item = 21;
            frame->operation = dever_rt_v1_async_group_run_sync(frame->group, &synchronous_unit, &item, &error);
            if (!frame->operation || item != 0) abort();
            frame->phase = 22;
            break;
        case 22:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = dever_rt_v1_async_group_wait(frame->group);
            frame->group = NULL;
            frame->phase = 23;
            break;
        case 23:
            status = poll(frame, frame->operation, NULL, NULL, frame->fault);
            if (status == DEVER_RT_ASYNC_PENDING) return;
            if (status != DEVER_RT_ASYNC_READY_OK || atomic_load(&group_sync_done_count) != 1) abort();
            dever_rt_v1_async_op_release(frame->operation);
            item = 21;
            frame->operation = dever_rt_v1_async_blocking(&synchronous_twice, &item, &error);
            if (!frame->operation || item != 0) abort();
            frame->phase = 17;
            break;
        case 17:
            status = poll(frame, frame->operation, &item, NULL, frame->fault);
            if (status != DEVER_RT_ASYNC_PENDING) abort();
            dever_rt_v1_async_op_release(frame->operation);
            frame->operation = NULL;
            *(int64_t *)frame->output = 42;
            complete(frame, DEVER_RT_ASYNC_READY_OK);
            return;
        default:
            abort();
        }
    }
}

static const DeverRtAsyncFunction root = {
    &integer, &integer, create, root_resume, destroy, done, 0
};

int main(void) {
    int64_t output = 0;
    int64_t fault = 0;
    DeverRtBuffer error = { 0 };
    if (dever_rt_v1_async_op_poll(NULL, NULL, NULL, NULL, NULL, &error)
        != DEVER_RT_ASYNC_INVALID_INPUT || !error.ptr) return 4;
    dever_rt_v1_buffer_free(error.ptr, error.len);
    error = (DeverRtBuffer){ 0 };
    uint8_t unit_output = 7;
    if (dever_rt_v1_time_sleep(0, &unit_output, &error) != DEVER_RT_OK
        || unit_output != 0 || error.ptr) return 2;
    unit_output = 7;
    if (dever_rt_v1_time_sleep(-1, &unit_output, &error) != DEVER_RT_RUNTIME_ERROR
        || unit_output != 7 || !error.ptr
        || error.len != sizeof("sleep duration must not be negative") - 1
        || memcmp(error.ptr, "sleep duration must not be negative", error.len)) return 3;
    dever_rt_v1_buffer_free(error.ptr, error.len);
    error = (DeverRtBuffer){ 0 };
    uint32_t status = dever_rt_v1_async_root(&root, NULL, &output, &fault, &error);
    if (error.ptr) dever_rt_v1_buffer_free(error.ptr, error.len);
    return status == DEVER_RT_ASYNC_READY_OK && output == 42
        && atomic_load(&blocking_done_count) == 3
        && atomic_load(&group_sync_done_count) == 1 ? 0 : 1;
}
