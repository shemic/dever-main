/* Kernel-level probes for argument truncation and fixed escape denials. */
#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/sched.h>
#include <stdint.h>
#include <sys/ioctl.h>
#include <sys/ptrace.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/xattr.h>
#include <unistd.h>

int main(int argc, char **argv) {
    (void)argv;
    /* Spawn may reset only the effective ID to this sandbox's real identity. */
    const long identity_calls[] = {SYS_setresuid, SYS_setresgid};
    const uint32_t identities[] = {getuid(), getgid()};
    for (unsigned i = 0; i < sizeof(identity_calls) / sizeof(identity_calls[0]); i++) {
        const long call = identity_calls[i];
        const uint32_t identity = identities[i];
        errno = 0;
        const long reset = syscall(call, UINT32_MAX, identity, UINT32_MAX);
        assert(argc == 1 ? reset == 0 : (reset == -1 && errno == EPERM));
        if (argc == 1) {
            assert(syscall(call, UINT64_MAX, UINT64_C(0x100000000) | identity, UINT64_MAX) == 0);
        }
        const uint64_t denied[][3] = {
            {UINT32_MAX, identity ^ 1U, UINT32_MAX},
            {identity, identity, UINT32_MAX},
            {UINT32_MAX, identity, identity},
        };
        for (unsigned j = 0; j < sizeof(denied) / sizeof(denied[0]); j++) {
            errno = 0;
            assert(syscall(call, denied[j][0], denied[j][1], denied[j][2]) == -1 && errno == EPERM);
        }
    }
    /* Python copy2 must be able to inspect metadata of its mounted source. */
    assert(listxattr("/worker/syscalls", NULL, 0) >= 0);
    int fd = open("/dev/null", O_RDONLY);
    assert(fd >= 0);
    const uint64_t forbidden[] = {TIOCSTI, TIOCLINUX, UINT64_C(0x100000000) | TIOCSTI, UINT64_C(0x100000000) | TIOCLINUX};
    for (unsigned i = 0; i < sizeof(forbidden) / sizeof(forbidden[0]); i++) {
        errno = 0;
        assert(syscall(SYS_ioctl, fd, forbidden[i], 0) == -1 && errno == EPERM);
    }
    errno = 0;
    assert(ioctl(fd, TCGETS, 0) == -1 && errno == ENOTTY);
    assert(close(fd) == 0);
    errno = 0;
    assert(syscall(SYS_unshare, CLONE_NEWUSER) == -1 && errno == EPERM);
    errno = 0;
    assert(ptrace(PTRACE_TRACEME, 0, 0, 0) == -1 && errno == EPERM);
    errno = 0;
    assert(socket(AF_UNIX, SOCK_STREAM, 0) == -1 && errno == EPERM);
    errno = 0;
    assert(syscall(SYS_clone3, 0, 0) == -1 && errno == ENOSYS);
    return 0;
}
