#include <stdint.h>

extern uint32_t dever_rt_v1_version(void);
extern void *dever_coro_probe_create(void);
extern int32_t dever_coro_probe_progress(void);
extern void dever_coro_probe_resume(void *frame);
extern uint8_t dever_coro_probe_done(void *frame);
extern void dever_coro_probe_destroy(void *frame);
extern int32_t dever_coro_root_run(void);

int main(void) {
    if (dever_rt_v1_version() != 1) return 1;
    void *frame = dever_coro_probe_create();
    if (!frame || dever_coro_probe_progress() != 0 || dever_coro_probe_done(frame)) return 2;
    dever_coro_probe_resume(frame);
    if (dever_coro_probe_progress() != 42 || !dever_coro_probe_done(frame)) return 3;
    dever_coro_probe_destroy(frame);
    frame = dever_coro_probe_create();
    if (!frame || dever_coro_probe_done(frame)) return 4;
    dever_coro_probe_destroy(frame);
    if (dever_coro_root_run() != 0) return 5;
    return 0;
}
