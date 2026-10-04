#include "../../crates/dever-backend-bridge/include/runtime.h"
#include <pthread.h>
#include <stdlib.h>

static int initialized;
static size_t calls;
static pthread_key_t cache_key;
static pthread_once_t cache_once = PTHREAD_ONCE_INIT;

#if DEVER_LEDGER_SCENARIO == 1
static void *volatile leaked;
#endif

static void create_cache_key(void) {
    if (pthread_key_create(&cache_key, free) != 0) abort();
}

uint32_t dever_rt_v1_process_init(void) {
    initialized = 1;
    return DEVER_RT_OK;
}

int dever_test_run(void) {
    if (!initialized) return 9;
    ++calls;
    /* Deterministically initialize TLS after warmup, as a later full queue can. */
    if (calls > 1) {
        if (pthread_once(&cache_once, create_cache_key) != 0) abort();
        if (pthread_getspecific(cache_key) == NULL) {
            void *cache = malloc(48);
            if (cache == NULL || pthread_setspecific(cache_key, cache) != 0) abort();
        }
    }
#if DEVER_LEDGER_SCENARIO == 1
    /* Keep the allocation observable to the optimizer and deliberately leak it. */
    leaked = malloc(1);
    if (leaked == NULL) abort();
#elif DEVER_LEDGER_SCENARIO == 2
    if (calls > 1) return 7;
#endif
    return 0;
}
