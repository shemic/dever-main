#include "runtime.h"

/* Link with managed-driver.c: the first flush initializes the logger; every
 * later flush must finish without outstanding per-call or deferred owners. */
int dever_test_run(void) {
    dever_rt_v1_log_flush();
    return 0;
}
