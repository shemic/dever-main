//! A pinned namespace init is the completion boundary for all descendants.
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, RawFd};

pub struct CommandNamespace {
    #[cfg(target_os = "linux")]
    descriptor: std::fs::File,
}

impl CommandNamespace {
    #[cfg(target_os = "linux")]
    pub fn pin(pid: i32, wrapper: u32) -> Result<Self, String> {
        use rustix::process::{Pid, PidfdFlags, pidfd_open};
        let pid = Pid::from_raw(pid).ok_or("invalid sandbox namespace PID")?;
        let descriptor = pidfd_open(pid, PidfdFlags::NONBLOCK)
            .map_err(|error| format!("cannot pin sandbox namespace: {error}"))?;
        // --block-fd holds the namespace before guard/target execution. If
        // setup failed and its PID was reused, do not acquire authority over
        // that process: only this bwrap's direct child belongs to this launch.
        let status = std::fs::read_to_string(format!("/proc/{}/status", pid.as_raw_pid()))
            .map_err(|error| format!("cannot verify sandbox namespace parent: {error}"))?;
        let parent = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))
            .and_then(|value| value.trim().parse::<u32>().ok());
        if parent != Some(wrapper) {
            return Err("sandbox namespace no longer belongs to its launcher".into());
        }
        Ok(Self {
            descriptor: descriptor.into(),
        })
    }

    #[cfg(target_os = "linux")]
    pub fn terminate(&self) -> Result<(), String> {
        match rustix::process::pidfd_send_signal(&self.descriptor, rustix::process::Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(error) => Err(format!("cannot stop owned command namespace: {error}")),
        }
    }
}

#[cfg(target_os = "linux")]
impl AsRawFd for CommandNamespace {
    fn as_raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }
}
