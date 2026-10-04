//! Fixed allowlist for packaged Linux runtimes. Mounts own file visibility;
//! this filter owns syscall categories that could bypass that visibility.

use super::Capabilities;
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule,
};
use std::collections::BTreeMap;

fn condition(argument: u8, op: SeccompCmpOp, value: u64) -> Result<SeccompCondition, String> {
    SeccompCondition::new(argument, SeccompCmpArgLen::Qword, op, value)
        .map_err(|error| error.to_string())
}

fn rule(conditions: Vec<SeccompCondition>) -> Result<SeccompRule, String> {
    SeccompRule::new(conditions).map_err(|error| error.to_string())
}

fn install(
    rules: BTreeMap<i64, Vec<SeccompRule>>,
    default: SeccompAction,
    matched: SeccompAction,
) -> Result<(), String> {
    let architecture = std::env::consts::ARCH
        .try_into()
        .map_err(|error: seccompiler::BackendError| error.to_string())?;
    let filter = SeccompFilter::new(rules, default, matched, architecture)
        .map_err(|error| error.to_string())?;
    let program: BpfProgram = filter
        .try_into()
        .map_err(|error: seccompiler::BackendError| error.to_string())?;
    seccompiler::apply_filter(&program)
        .map_err(|error| format!("cannot enforce Worker syscall policy: {error}"))
}

pub(super) fn apply(capabilities: Capabilities) -> Result<(), String> {
    // clone3 carries flags behind a pointer. ENOSYS makes supported libc use
    // clone, whose scalar flags can be checked without user-memory races.
    install(
        BTreeMap::from([(libc::SYS_clone3, vec![])]),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
    )?;
    let mut rules = BTreeMap::new();
    for syscall in [
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_pread64,
        libc::SYS_pwrite64,
        libc::SYS_preadv,
        libc::SYS_pwritev,
        libc::SYS_close,
        libc::SYS_close_range,
        libc::SYS_lseek,
        libc::SYS_fcntl,
        libc::SYS_dup,
        libc::SYS_dup3,
        libc::SYS_pipe2,
        libc::SYS_openat,
        libc::SYS_fstat,
        libc::SYS_statx,
        libc::SYS_faccessat,
        libc::SYS_faccessat2,
        libc::SYS_readlinkat,
        // copy2/copytree inspect extended metadata on their visible source.
        // These calls only read mounted files; writes remain independently
        // governed by the read-only inputs and explicit writable grants.
        libc::SYS_listxattr,
        libc::SYS_llistxattr,
        libc::SYS_flistxattr,
        libc::SYS_getxattr,
        libc::SYS_lgetxattr,
        libc::SYS_fgetxattr,
        libc::SYS_getdents64,
        libc::SYS_getcwd,
        libc::SYS_chdir,
        libc::SYS_fchdir,
        libc::SYS_mkdirat,
        libc::SYS_unlinkat,
        libc::SYS_renameat,
        libc::SYS_renameat2,
        libc::SYS_linkat,
        libc::SYS_symlinkat,
        libc::SYS_ftruncate,
        libc::SYS_truncate,
        libc::SYS_fchmod,
        libc::SYS_fchmodat,
        libc::SYS_fchown,
        libc::SYS_fchownat,
        libc::SYS_flock,
        libc::SYS_fsync,
        libc::SYS_fdatasync,
        libc::SYS_sync_file_range,
        libc::SYS_fallocate,
        libc::SYS_fstatfs,
        libc::SYS_statfs,
        libc::SYS_umask,
        libc::SYS_utimensat,
        libc::SYS_sendfile,
        libc::SYS_copy_file_range,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_brk,
        libc::SYS_msync,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_rt_sigsuspend,
        libc::SYS_rt_sigtimedwait,
        libc::SYS_sigaltstack,
        libc::SYS_tgkill,
        libc::SYS_kill,
        libc::SYS_getpid,
        libc::SYS_getppid,
        libc::SYS_gettid,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        libc::SYS_getgroups,
        libc::SYS_getpgid,
        libc::SYS_getsid,
        libc::SYS_setpgid,
        libc::SYS_setsid,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_get_robust_list,
        libc::SYS_futex,
        libc::SYS_rseq,
        libc::SYS_sched_yield,
        libc::SYS_sched_getaffinity,
        libc::SYS_sched_getparam,
        libc::SYS_sched_getscheduler,
        libc::SYS_clock_gettime,
        libc::SYS_clock_getres,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
        libc::SYS_gettimeofday,
        libc::SYS_times,
        libc::SYS_getrusage,
        libc::SYS_getrlimit,
        libc::SYS_prlimit64,
        libc::SYS_uname,
        libc::SYS_sysinfo,
        libc::SYS_getrandom,
        libc::SYS_epoll_create1,
        libc::SYS_epoll_ctl,
        libc::SYS_epoll_pwait,
        libc::SYS_epoll_pwait2,
        libc::SYS_ppoll,
        libc::SYS_pselect6,
        libc::SYS_eventfd2,
        libc::SYS_timerfd_create,
        libc::SYS_timerfd_settime,
        libc::SYS_timerfd_gettime,
        libc::SYS_inotify_init1,
        libc::SYS_inotify_add_watch,
        libc::SYS_inotify_rm_watch,
        libc::SYS_recvfrom,
        libc::SYS_sendto,
        libc::SYS_recvmsg,
        libc::SYS_sendmsg,
        libc::SYS_recvmmsg,
        libc::SYS_sendmmsg,
        libc::SYS_shutdown,
        libc::SYS_getsockopt,
        libc::SYS_setsockopt,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_execve,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_wait4,
        libc::SYS_waitid,
        libc::SYS_clone3,
    ] {
        rules.insert(syscall, vec![]);
    }
    #[cfg(target_arch = "x86_64")]
    for syscall in [
        libc::SYS_arch_prctl,
        libc::SYS_open,
        libc::SYS_stat,
        libc::SYS_lstat,
        libc::SYS_newfstatat,
        libc::SYS_access,
        libc::SYS_readlink,
        libc::SYS_pipe,
        libc::SYS_dup2,
        libc::SYS_poll,
        libc::SYS_select,
        libc::SYS_epoll_wait,
        libc::SYS_epoll_create,
        libc::SYS_mkdir,
        libc::SYS_rmdir,
        libc::SYS_unlink,
        libc::SYS_rename,
        libc::SYS_link,
        libc::SYS_symlink,
        libc::SYS_chmod,
        libc::SYS_chown,
        libc::SYS_time,
        libc::SYS_alarm,
        libc::SYS_getpgrp,
    ] {
        rules.insert(syscall, vec![]);
    }
    #[cfg(target_arch = "aarch64")]
    {
        rules.insert(libc::SYS_newfstatat, vec![]);
    }

    // New namespaces and CLONE_PARENT could escape the owned PID lifecycle.
    let forbidden_clone = (libc::CLONE_NEWCGROUP
        | libc::CLONE_NEWIPC
        | libc::CLONE_NEWNET
        | libc::CLONE_NEWNS
        | libc::CLONE_NEWPID
        | libc::CLONE_NEWUSER
        | libc::CLONE_NEWUTS
        | libc::CLONE_PARENT
        | libc::CLONE_UNTRACED) as u64;
    let mut clone = vec![condition(0, SeccompCmpOp::MaskedEq(forbidden_clone), 0)?];
    if !capabilities.process {
        let required = (libc::CLONE_THREAD | libc::CLONE_VM | libc::CLONE_SIGHAND) as u64;
        clone.push(condition(
            0,
            SeccompCmpOp::MaskedEq(required | 255),
            required,
        )?);
    }
    rules.insert(libc::SYS_clone, vec![rule(clone)?]);
    if capabilities.process {
        // POSIX_SPAWN_RESETIDS restores the caller's real identity before exec.
        // Permit that exact reset only; real/saved IDs and group membership
        // cannot change. Linux uid_t/gid_t consume the low 32 argument bits.
        for (syscall, identity) in [
            (libc::SYS_setresuid, rustix::process::getuid().as_raw()),
            (libc::SYS_setresgid, rustix::process::getgid().as_raw()),
        ] {
            let conditions = [u32::MAX, identity, u32::MAX]
                .into_iter()
                .enumerate()
                .map(|(argument, value)| {
                    SeccompCondition::new(
                        argument as u8,
                        SeccompCmpArgLen::Dword,
                        SeccompCmpOp::Eq,
                        u64::from(value),
                    )
                    .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            rules.insert(syscall, vec![rule(conditions)?]);
        }
    }
    #[cfg(target_arch = "x86_64")]
    if capabilities.process {
        rules.insert(libc::SYS_fork, vec![]);
        rules.insert(libc::SYS_vfork, vec![]);
    }
    // Anonymous socket pairs cannot reach another process. They are needed by
    // language runtime event loops even without the network capability.
    rules.insert(
        libc::SYS_socketpair,
        vec![rule(vec![condition(
            0,
            SeccompCmpOp::Eq,
            libc::AF_UNIX as u64,
        )?])?],
    );
    if capabilities.network {
        let mut sockets = Vec::new();
        for domain in [libc::AF_INET, libc::AF_INET6] {
            for kind in [libc::SOCK_STREAM, libc::SOCK_DGRAM] {
                sockets.push(rule(vec![
                    condition(0, SeccompCmpOp::Eq, domain as u64)?,
                    condition(
                        1,
                        SeccompCmpOp::MaskedEq(
                            !(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC) as u32 as u64,
                        ),
                        kind as u64,
                    )?,
                ])?);
            }
        }
        rules.insert(libc::SYS_socket, sockets);
        for syscall in [
            libc::SYS_connect,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept,
            libc::SYS_accept4,
        ] {
            rules.insert(syscall, vec![]);
        }
    }
    // ioctl truncates request to unsigned int. Compare that same width so a
    // caller cannot hide a forbidden request behind nonzero upper bits.
    let terminal_requests = [libc::TIOCSTI, libc::TIOCLINUX]
        .into_iter()
        .map(|request| {
            SeccompCondition::new(1, SeccompCmpArgLen::Dword, SeccompCmpOp::Ne, request)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    rules.insert(libc::SYS_ioctl, vec![rule(terminal_requests)?]);
    // The allowlist excludes namespace, mount, ptrace/process_vm, io_uring,
    // device creation, kernel administration, BPF and all x32 syscall numbers.
    install(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
    )
}
