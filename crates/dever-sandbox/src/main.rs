//! Private signed helper. It accepts a fixed capability mask, never filter code.

#[cfg(target_os = "linux")]
fn execute() -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    let mut arguments = std::env::args_os().skip(1);
    let first = arguments.next();
    let mut observation = if first.as_deref() == Some(std::ffi::OsStr::new("--observe")) {
        let fd = arguments
            .next()
            .and_then(|value| value.to_str().and_then(|value| value.parse::<i32>().ok()))
            .filter(|fd| *fd >= 3)
            .ok_or("invalid sandbox observation descriptor")?;
        // Opening proc creates a safe owned CLOEXEC duplicate. Close the
        // inherited raw descriptor before exec so neither channel leaks.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/self/fd/{fd}"))
            .map_err(|error| format!("cannot open sandbox observation: {error}"))?;
        nix::unistd::close(fd)
            .map_err(|error| format!("cannot close inherited observation: {error}"))?;
        Some(file)
    } else {
        None
    };
    let first = if observation.is_some() {
        arguments.next()
    } else {
        first
    };
    let result = (|| {
        let flags = first
            .and_then(|value| value.to_str().and_then(|value| value.parse::<u8>().ok()))
            .filter(|value| *value <= 3)
            .ok_or("invalid sandbox capability mask")?;
        let executable = arguments.next().ok_or("missing sandbox executable")?;
        if !std::path::Path::new(&executable).starts_with("/worker") {
            return Err("sandbox executable must be inside its Worker tree".into());
        }
        let mut command = std::process::Command::new(executable);
        command.args(arguments).env_clear();
        dever_sandbox::restrict(dever_sandbox::Capabilities {
            network: flags & 1 != 0,
            process: flags & 2 != 0,
        })?;
        if let Some(observation) = &mut observation {
            observation
                .write_all(b"R")
                .map_err(|error| format!("cannot confirm sandbox guard: {error}"))?;
        }
        Err(format!(
            "cannot execute sandboxed Worker: {}",
            command.exec()
        ))
    })();
    if let (Some(observation), Err(error)) = (&mut observation, &result) {
        observation
            .write_all(b"E")
            .and_then(|_| observation.write_all(error.as_bytes()))
            .map_err(|write| format!("{error}; cannot report sandbox startup: {write}"))?;
    }
    result
}

#[cfg(not(target_os = "linux"))]
fn execute() -> Result<(), String> {
    Err("sandbox is not supported on this platform".into())
}

fn main() -> std::process::ExitCode {
    match execute() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
