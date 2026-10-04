#include "../../crates/dever-backend-bridge/include/runtime.h"
#include <errno.h>

/* Keep the same allocation oracle as application/kernel probes. Only case
 * selection and the suite's indexed fault ownership differ. */
#define main managed_main
#include "managed-driver.c"
#undef main

extern int32_t dever_test_entry(int64_t index, void *out, void *fault);
extern uint64_t dever_test_fault_size(int64_t index);
extern void dever_test_fault_release(int64_t index, void *fault);
extern int32_t dever_test_fault_render(int64_t index, const void *fault,
                                     void **out, DeverRtBuffer *error);
extern int64_t dever_test_count(void);

static int64_t case_index;
static int32_t expected_status;

int dever_test_run(void) {
    uint64_t size = dever_test_fault_size(case_index);
    if (size == 0 || size > 1024 * 1024) return 2;
    void *fault = calloc(1, (size_t)size);
    if (fault == NULL) return 3;
    uint8_t unit = 0;
    int32_t status = dever_test_entry(case_index, &unit, fault);
    int failed = status != expected_status;
    if (status != 0) {
        void *rendered = NULL;
        DeverRtBuffer error = {0};
        int32_t render_status = dever_test_fault_render(case_index, fault, &rendered, &error);
        if (render_status == 0) {
            DeverRtBuffer text = {0};
            if (dever_rt_v1_text_export(rendered, &text) != 0) {
                failed = 1;
            } else {
                fwrite(text.ptr, 1, (size_t)text.len, stderr);
                fputc('\n', stderr);
            }
            dever_rt_v1_buffer_free(text.ptr, text.len);
            dever_rt_v1_text_release(rendered);
        } else {
            failed = 1;
            fwrite(error.ptr, 1, (size_t)error.len, stderr);
        }
        dever_rt_v1_buffer_free(error.ptr, error.len);
    }
    dever_test_fault_release(case_index, fault);
    free(fault);
    return failed;
}

int main(int argc, char **argv) {
    if (argc != 3) return 4;
    char *end = NULL;
    errno = 0;
    long long index = strtoll(argv[1], &end, 10);
    if (errno != 0 || end == argv[1] || *end != '\0' || index < 0 || index >= dever_test_count()) return 5;
    case_index = (int64_t)index;
    if (argv[2][0] == '0' && argv[2][1] == '\0') expected_status = 0;
    else if (argv[2][0] == '1' && argv[2][1] == '\0') expected_status = 1;
    else return 6;
    return managed_main();
}
