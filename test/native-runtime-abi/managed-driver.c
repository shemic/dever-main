#include "../../crates/dever-backend-bridge/include/runtime.h"
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>
#include <pthread.h>
#include <string.h>

extern int dever_test_run(void);

#ifndef DEVER_TEST_ITERATIONS
#define DEVER_TEST_ITERATIONS 64
#endif

/* Link-time wrappers observe allocations in this fixture's static runtime.
 * This is a bounded ownership regression, not a general memory profiler. */
static _Atomic long long live_allocations;

void *__real_malloc(size_t size);
void *__real_calloc(size_t count, size_t size);
void *__real_realloc(void *pointer, size_t size);
void __real_free(void *pointer);
int __real_posix_memalign(void **pointer, size_t alignment, size_t size);
char *__real_realpath(const char *path, char *resolved);

/* glibc allocates this returned owner inside its DSO, bypassing --wrap=malloc.
 * Rust canonicalize frees it through the executable's wrapped free. */
char *__wrap_realpath(const char *path, char *resolved) {
    char *pointer = __real_realpath(path, resolved);
    atomic_fetch_add(&live_allocations, pointer != NULL && resolved == NULL);
    return pointer;
}

void *__wrap_malloc(size_t size) {
    void *pointer = __real_malloc(size);
    atomic_fetch_add(&live_allocations, pointer != NULL);
    return pointer;
}

void *__wrap_calloc(size_t count, size_t size) {
    void *pointer = __real_calloc(count, size);
    atomic_fetch_add(&live_allocations, pointer != NULL);
    return pointer;
}

void *__wrap_realloc(void *pointer, size_t size) {
    int had_pointer = pointer != NULL;
    void *resized = __real_realloc(pointer, size);
    if (!had_pointer && resized != NULL) {
        atomic_fetch_add(&live_allocations, 1);
    } else if (had_pointer && resized == NULL && size == 0) {
        atomic_fetch_sub(&live_allocations, 1);
    }
    return resized;
}

void __wrap_free(void *pointer) {
    atomic_fetch_sub(&live_allocations, pointer != NULL);
    __real_free(pointer);
}

int __wrap_posix_memalign(void **pointer, size_t alignment, size_t size) {
    int status = __real_posix_memalign(pointer, alignment, size);
    atomic_fetch_add(&live_allocations, status == 0 && *pointer != NULL);
    return status;
}

/* Standard-library queue caches live until the host thread exits. Join before
 * measuring so lazy TLS initialization cannot masquerade as a per-call leak. */
static void *invoke_kernel(void *result) {
    *(int *)result = dever_test_run();
    return NULL;
}

static int run_kernel(void) {
    pthread_t thread;
    int result = 0;
    int status = pthread_create(&thread, NULL, invoke_kernel, &result);
    if (status == 0) {
        status = pthread_join(thread, NULL);
    }
    if (status != 0) {
        fprintf(stderr, "managed kernel thread failed: %s\n", strerror(status));
        exit(EXIT_FAILURE);
    }
    return result;
}

int main(void) {
    /* This C process owns signal policy just like the generated process main. */
    if (dever_rt_v1_process_init() != DEVER_RT_OK) {
        fprintf(stderr, "could not initialize the managed kernel process\n");
        return 1;
    }
    /* Permit process-wide lazy initialization, then require no per-call leak. */
    int result = run_kernel();
    if (result != 0) {
        fprintf(stderr, "managed kernel returned an unexpected result: %d\n", result);
        return 1;
    }
    long long baseline = atomic_load(&live_allocations);
    for (size_t remaining = DEVER_TEST_ITERATIONS; remaining != 0; --remaining) {
        result = run_kernel();
        if (result != 0) {
            fprintf(stderr, "managed kernel changed its result on repeated execution: %d\n", result);
            return 1;
        }
        long long current = atomic_load(&live_allocations);
        if (current != baseline) {
            fprintf(stderr, "managed kernel allocation imbalance: %lld\n",
                    current - baseline);
            return 1;
        }
    }
    return 0;
}
