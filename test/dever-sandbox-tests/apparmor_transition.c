/* Explicit AppArmor acceptance probe. Run only after host-policy approval.
 * One process attempts capabilities in its own user/network namespaces; it
 * never execs another program and never changes the host network or policy.
 * Exit 0: kernel denied a required capability; 1: unsafe access; 2: invalid or
 * unavailable setup. aa-exec refusing the transition before main is separate
 * evidence and must be recorded by the caller, not mistaken for this result.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <net/if.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>

static int failure(const char *stage)
{
    int cause = errno;
    int denied = cause == EPERM || cause == EACCES;
    printf("%s|%s|errno=%d\n", denied ? "DENIED" : "UNAVAILABLE", stage, cause);
    return denied ? 0 : 2;
}

static int unavailable(const char *stage)
{
    printf("UNAVAILABLE|%s|errno=%d\n", stage, errno);
    return 2;
}

static int write_mapping(const char *path, const char *text)
{
    int fd = open(path, O_WRONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0)
        return -1;
    size_t size = strlen(text);
    ssize_t written = write(fd, text, size);
    int cause = errno;
    close(fd);
    if (written != (ssize_t)size) {
        errno = written < 0 ? cause : EIO;
        return -1;
    }
    return 0;
}

int main(void)
{
    uid_t uid = getuid();
    gid_t gid = getgid();
    if (uid == 0 || geteuid() == 0) {
        fputs("UNAVAILABLE|requires_real_nonzero_uid\n", stderr);
        return 2;
    }
    char label[4096];
    int fd = open("/proc/self/attr/current", O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        return unavailable("read_initial_label");
    ssize_t length = read(fd, label, sizeof(label) - 1);
    int cause = errno;
    close(fd);
    if (length < 0) {
        errno = cause;
        return unavailable("read_initial_label");
    }
    label[length] = '\0';
    printf("INITIAL_LABEL|%s\n", label);
    if (unshare(CLONE_NEWUSER) != 0)
        return failure("unshare_user");
    char mapping[80];
    snprintf(mapping, sizeof(mapping), "0 %u 1\n", (unsigned int)uid);
    if (write_mapping("/proc/self/uid_map", mapping) != 0)
        return failure("uid_map");
    if (write_mapping("/proc/self/setgroups", "deny\n") != 0)
        return failure("setgroups");
    snprintf(mapping, sizeof(mapping), "0 %u 1\n", (unsigned int)gid);
    if (write_mapping("/proc/self/gid_map", mapping) != 0)
        return failure("gid_map");
    if (unshare(CLONE_NEWNET) != 0)
        return failure("unshare_net");
    puts("UNSAFE_ALLOWED|network_namespace|CAP_SYS_ADMIN_check_passed");
    fd = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        unavailable("network_socket");
        return 1;
    }
    struct ifreq interface = {0};
    memcpy(interface.ifr_name, "lo", 3);
    if (ioctl(fd, SIOCGIFFLAGS, &interface) != 0) {
        cause = errno;
        close(fd);
        errno = cause;
        unavailable("read_loopback_flags");
        return 1;
    }
    interface.ifr_flags |= IFF_UP;
    int result = ioctl(fd, SIOCSIFFLAGS, &interface);
    cause = errno;
    close(fd);
    if (result != 0) {
        errno = cause;
        failure("raise_loopback");
        return 1;
    }
    puts("UNSAFE_ALLOWED|userns_and_net_admin_in_single_process");
    return 1;
}
