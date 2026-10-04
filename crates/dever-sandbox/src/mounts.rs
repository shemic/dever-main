//! Pin deployment grants until bwrap has mounted the exact checked inode.

use std::path::Path;
use std::process::Command;

#[derive(Default)]
pub(super) struct Mounts {
    #[cfg(target_os = "linux")]
    descriptors: Vec<command_fds::FdMapping>,
}

impl Mounts {
    #[cfg(target_os = "linux")]
    pub(super) fn pass(&mut self, file: std::fs::File) -> Result<i32, String> {
        let child_fd =
            i32::try_from(self.descriptors.len()).map_err(|_| "too many sandbox descriptors")? + 3;
        self.descriptors.push(command_fds::FdMapping {
            parent_fd: file.into(),
            child_fd,
        });
        Ok(child_fd)
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn pass(&mut self, _: std::fs::File) -> Result<i32, String> {
        Err("sandbox descriptors are not supported on this platform".into())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn bind(
        &mut self,
        command: &mut Command,
        source: &Path,
        destination: &Path,
        writable: bool,
    ) -> Result<(), String> {
        use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
        let descriptor = openat2(
            rustix::fs::CWD,
            source,
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|error| format!("cannot pin sandbox mount without following links: {error}"))?;
        let child_fd =
            i32::try_from(self.descriptors.len()).map_err(|_| "too many sandbox mounts")? + 3;
        command
            .arg(if writable {
                "--bind-fd"
            } else {
                "--ro-bind-fd"
            })
            .arg(child_fd.to_string())
            .arg(destination);
        self.descriptors.push(command_fds::FdMapping {
            parent_fd: descriptor,
            child_fd,
        });
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn attach(self, command: &mut Command) -> Result<(), String> {
        use command_fds::CommandFdExt;
        command
            .fd_mappings(self.descriptors)
            .map_err(|error| format!("cannot pass sandbox mount descriptors: {error}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn bind(
        &mut self,
        _: &mut Command,
        _: &Path,
        _: &Path,
        _: bool,
    ) -> Result<(), String> {
        Err("sandbox mounts are not supported on this platform".into())
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn attach(self, _: &mut Command) -> Result<(), String> {
        Err("sandbox mounts are not supported on this platform".into())
    }
}
