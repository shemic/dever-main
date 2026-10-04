//! Test-only static entry for the already pivoted no-host acceptance namespace.
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rustix::mount::{MountFlags, mount};
    use rustix::thread::{self, CapabilitySet};
    use std::os::unix::process::CommandExt;

    let mut arguments = std::env::args_os().skip(1);
    let executable = arguments.next().ok_or("missing acceptance executable")?;
    if !std::path::Path::new(&executable).is_absolute() {
        return Err("acceptance executable must be absolute".into());
    }
    // The root-only harness keeps bwrap's original namespace capabilities.
    // Reject ordinary host execution before touching any mount.
    if !thread::no_new_privs()?
        || !thread::capabilities(None)?
            .effective
            .contains(CapabilitySet::SYS_ADMIN)
    {
        return Err("acceptance init requires its restricted fixture namespace".into());
    }
    // bwrap's readonly proc submounts otherwise prevent a nested user
    // namespace from mounting its own proc. Replace only this private proc.
    mount(
        "proc",
        "/proc",
        "proc",
        MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC,
        None,
    )?;
    Err(std::process::Command::new(executable)
        .args(arguments)
        .env_clear()
        .exec()
        .into())
}

#[cfg(not(target_os = "linux"))]
fn main() -> Result<(), &'static str> {
    Err("native acceptance isolation requires Linux")
}
