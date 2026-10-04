//! Bounded Linux platform actions. Image installation never calls systemctl.
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn run(command: &mut Command, operation: &str) -> Result<bool, String> {
    let mut child = OwnedChild(
        command
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("cannot {operation}: {error}"))?,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child
            .0
            .try_wait()
            .map_err(|error| format!("cannot wait for {operation}: {error}"))?
        {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            return Err(format!("{operation} exceeded 30 seconds"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn active() -> Result<bool, String> {
    if !loaded()? {
        return Ok(false);
    }
    match property("ActiveState")?.as_str() {
        "inactive" | "failed" => Ok(false),
        "active" | "activating" | "deactivating" | "reloading" => Ok(true),
        _ => Err("Dever service has an unsupported active state".into()),
    }
}

pub(super) fn loaded() -> Result<bool, String> {
    match property("LoadState")?.as_str() {
        "loaded" => Ok(true),
        "not-found" => Ok(false),
        _ => Err("Dever service has an unsupported load state".into()),
    }
}

fn property(name: &str) -> Result<String, String> {
    let mut command = Command::new("/usr/bin/systemctl");
    command
        .env_clear()
        .args(["show", "--value", "--property", name, "deverd.service"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = OwnedChild(
        command
            .spawn()
            .map_err(|error| format!("cannot inspect Dever service: {error}"))?,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.0.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err("service state inspection exceeded 30 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut bytes = Vec::new();
    child
        .0
        .stdout
        .take()
        .ok_or("service state output missing")?
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 4096 {
        return Err("service state output exceeded its bound".into());
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| "service state output is not UTF-8")?
        .trim()
        .to_owned();
    if !status.success()
        && !(status.code() == Some(1) && name == "LoadState" && value == "not-found")
    {
        return Err(format!("cannot inspect Dever service {name}: {status}"));
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    Stop,
    Reload,
    Restart,
}

/// Files are restored between these two action lists. A service that never
/// existed is not stopped after its new unit has already been removed.
pub fn recovery_actions(
    start_requested: bool,
    was_active: bool,
    currently_loaded: bool,
) -> (Vec<RecoveryAction>, Vec<RecoveryAction>) {
    if !start_requested {
        return (vec![], vec![]);
    }
    let before = if currently_loaded {
        vec![RecoveryAction::Stop]
    } else {
        vec![]
    };
    let mut after = vec![RecoveryAction::Reload];
    if was_active {
        after.push(RecoveryAction::Restart);
    }
    (before, after)
}

pub(super) fn execute(actions: &[RecoveryAction]) -> Result<(), String> {
    for action in actions {
        let arguments = match action {
            RecoveryAction::Reload => vec!["daemon-reload"],
            RecoveryAction::Stop => vec!["stop", "deverd.service"],
            RecoveryAction::Restart => vec!["restart", "deverd.service"],
        };
        if !run(
            Command::new("/usr/bin/systemctl").args(&arguments),
            "restore Dever service",
        )? {
            return Err(format!("systemctl {} failed", arguments.join(" ")));
        }
    }
    Ok(())
}

pub(super) fn apply(restart: bool) -> Result<(), String> {
    execute(&[
        RecoveryAction::Reload,
        if restart {
            RecoveryAction::Restart
        } else {
            RecoveryAction::Stop
        },
    ])
}

pub(super) fn ready(layout: &crate::toolchain::Layout) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match crate::toolchain::cache_status(layout) {
            Ok(_) => return Ok(()),
            Err(error) if Instant::now() >= deadline => {
                return Err(format!("Dever service did not become ready: {error}"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}
