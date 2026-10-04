//! One lifecycle for every managed compiler and package build hook.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(300);
const MAX_OUTPUT: u64 = 16 * 1024 * 1024;
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
mod budget;

pub(crate) struct Stage(pub(crate) PathBuf);

impl Stage {
    pub(crate) fn new() -> Result<Self, String> {
        for _ in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "dever-lib-build-{}-{}",
                std::process::id(),
                NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
            ));
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder.create(&path)
            };
            #[cfg(not(unix))]
            let created = fs::create_dir(&path);
            match created {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("cannot create private build staging: {error}")),
            }
        }
        Err("cannot allocate private build staging".into())
    }

    pub(crate) fn write(
        &self,
        relative: &str,
        bytes: &[u8],
        executable: bool,
    ) -> Result<PathBuf, String> {
        crate::workers::valid_path(relative)?;
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().expect("validated relative path"))
            .map_err(|error| format!("cannot create build input directory: {error}"))?;
        // This tree is private and never a writable child mount. A second write
        // must not silently replace a previously verified input.
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("cannot stage build input: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("cannot write build input: {error}"))?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("cannot mark build tool executable: {error}"))?;
        }
        Ok(path)
    }

    pub(crate) fn run(
        &self,
        command: Command,
        operation: &str,
        writable: &Path,
    ) -> Result<Vec<u8>, String> {
        let status = self.execute(command, operation, Some(writable))?;
        if !status.success() {
            return Err(format!(
                "{operation} failed: {}",
                String::from_utf8_lossy(&self.diagnostics()?)
            ));
        }
        fs::read(self.0.join("tool-stdout"))
            .map_err(|error| format!("cannot read {operation} output: {error}"))
    }

    pub(crate) fn probe(&self, command: Command) -> Result<bool, String> {
        match self
            .execute(command, "isolated Node addon probe", None)?
            .code()
        {
            Some(0) => Ok(true),
            Some(10) => Ok(false),
            _ => Err(format!(
                "Node addon probe failed outside its loader check: {}",
                String::from_utf8_lossy(&self.diagnostics()?)
            )),
        }
    }

    fn execute(
        &self,
        mut command: Command,
        operation: &str,
        writable: Option<&Path>,
    ) -> Result<std::process::ExitStatus, String> {
        // Captures are outside every child-writable grant. Namespace teardown
        // reaps descendants when the owned bootstrap is killed on any exit path.
        let stdout = self.0.join("tool-stdout");
        let stderr = self.0.join("tool-stderr");
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                File::create(&stdout).map_err(|error| error.to_string())?,
            ))
            .stderr(Stdio::from(
                File::create(&stderr).map_err(|error| error.to_string())?,
            ))
            .spawn()
            .map_err(|error| format!("cannot execute {operation}: {error}"))?;
        let mut child = OwnedChild(child);
        let deadline = Instant::now() + DEADLINE;
        let mut next_scratch_check = Instant::now();
        let within_budget = || -> Result<(), String> {
            for path in [&stdout, &stderr] {
                if fs::metadata(path).map_err(|error| error.to_string())?.len() > MAX_OUTPUT {
                    return Err(format!("{operation} output exceeds byte limit"));
                }
            }
            Ok(())
        };
        let status = loop {
            within_budget()?;
            if Instant::now() >= next_scratch_check {
                if let Some(path) = writable {
                    budget::check(path)?;
                }
                next_scratch_check = Instant::now() + Duration::from_millis(250);
            }
            if let Some(status) = child
                .0
                .try_wait()
                .map_err(|error| format!("cannot wait for {operation}: {error}"))?
            {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(format!("{operation} exceeded its five-minute deadline"));
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        within_budget()?;
        if let Some(path) = writable {
            budget::check(path)?;
        }
        Ok(status)
    }

    pub(crate) fn diagnostics(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        for name in ["tool-stdout", "tool-stderr"] {
            File::open(self.0.join(name))
                .and_then(|file| file.take(MAX_OUTPUT).read_to_end(&mut bytes))
                .map_err(|error| format!("cannot read build diagnostic: {error}"))?;
        }
        Ok(bytes)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
