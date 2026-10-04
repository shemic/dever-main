/* Native author fixture for compile cancellation and deadline supervision. */
#include <stdio.h>
#include <errno.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--dever-health") == 0) return 0;
    FILE *pid = fopen("worker.pid", "w");
    if (!pid) return 2;
    if (fprintf(pid, "%ld", (long)getpid()) < 0 || fclose(pid) != 0) return 3;
    struct timespec remaining = {300, 0};
    while (nanosleep(&remaining, &remaining) != 0) {
        if (errno != EINTR) return 4;
    }
    return 0;
}
