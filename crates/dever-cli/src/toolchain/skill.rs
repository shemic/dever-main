//! A stable AI entry resolves the same active-version journal as the launcher.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::release::MachineManager;

static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

pub(super) fn path(manager: &MachineManager) -> Result<PathBuf, String> {
    let version = manager
        .active_version()?
        .ok_or("no active Dever version; install Dever first")?;
    // Like launcher dispatch, this is read-only and available to every machine user.
    manager.resolve_skill(&version)
}

pub(super) fn install(manager: &MachineManager, destination: &Path) -> Result<(), String> {
    path(manager)?;
    let launcher = manager.layout().bin().join("dever");
    let launcher = launcher
        .to_str()
        .ok_or("launcher path must be UTF-8 for an AI skill")?;
    if launcher.chars().any(char::is_control) || launcher.contains('`') {
        return Err("launcher path cannot contain control characters or backticks".into());
    }
    let quoted = format!("'{}'", launcher.replace('\'', "'\\''"));
    let content = format!(
        "---\nname: dever-language\ndescription: Develop, review and debug Dever language applications (.dever and .dever.md), manage Dever Packages and Libs, and install or update Dever. Not the Go Dever framework.\n---\n\n# Dever language\n\nBefore every Dever task, run this read-only command:\n\n```sh\n{quoted} skill path\n```\n\nRead `SKILL.md` in the returned directory completely, then follow its instructions. Resolve its reference and asset paths against that directory. This entry follows the machine's active Dever version, so `dever update` and `dever use` update the compiler and development guidance together. If resolution fails, report the error; do not use stale guidance or infer language syntax. This entry grants no permission to install, update or change a project.\n"
    );
    let name = destination
        .file_name()
        .ok_or("skill destination requires a new directory name")?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent)
        .map_err(|error| format!("cannot resolve skill destination parent: {error}"))?;
    let destination = parent.join(name);
    let staging = stage(&parent)?;
    let entry = staging.join("SKILL.md");
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&entry)
            .map_err(|error| format!("cannot create skill entry: {error}"))?;
        file.write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot persist skill entry: {error}"))?;
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{CWD, RenameFlags, renameat_with};
            renameat_with(CWD, &staging, CWD, &destination, RenameFlags::NOREPLACE)
                .map_err(|error| format!("cannot publish new skill directory: {error}"))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("skill installation is not available for this platform".into())
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&entry);
        let _ = fs::remove_dir(&staging);
    }
    result
}

fn stage(parent: &Path) -> Result<PathBuf, String> {
    loop {
        let path = parent.join(format!(
            ".dever-skill-{}-{}",
            std::process::id(),
            NEXT_STAGING.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot stage skill entry: {error}")),
        }
    }
}
