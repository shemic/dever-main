//! Create complete starter projects without replacing caller-owned paths.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

macro_rules! template {
    ($path:literal) => {
        include_str!(concat!("../../../skills/assets/template/", $path))
    };
}

const FILES: &[(&str, &str)] = &[
    ("config/setting.json", template!("config/setting.json")),
    ("AGENTS.md", template!("AGENTS.md")),
    ("README.md", template!("README.md")),
    (".gitignore", template!(".gitignore")),
];

const SOURCES: &[(&str, &str, &str)] = &[
    (
        "module/hello/greeting/app",
        template!("module/hello/greeting/app.dever"),
        template!("module/hello/greeting/app.md"),
    ),
    (
        "module/hello/greeting/api",
        template!("module/hello/greeting/api.dever"),
        template!("module/hello/greeting/api.md"),
    ),
    (
        "test/hello/greeting/greet",
        template!("test/hello/greeting/greet.dever"),
        template!("test/hello/greeting/greet.md"),
    ),
];

/// Publish a new project beside its private staging directory. Existing targets,
/// including empty directories and dangling symlinks, are never replaced.
pub fn create(destination: &Path, markdown: bool) -> Result<(), String> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .ok_or_else(|| "new project requires a new directory name".to_owned())?;
    let parent = fs::canonicalize(parent)
        .map_err(|error| format!("cannot resolve new project parent: {error}"))?;
    let destination = parent.join(name);
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            return Err(format!(
                "project target already exists: {}",
                destination.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect project target: {error}")),
    }
    let staging = Staging::create(&parent)?;
    for (path, content) in FILES {
        write(&staging.0, path, content)?;
    }
    for (path, source, document) in SOURCES {
        let (suffix, content) = if markdown {
            ("dever.md", document.replace("{{source}}", source))
        } else {
            ("dever", (*source).to_owned())
        };
        write(&staging.0, &format!("{path}.{suffix}"), &content)?;
    }
    publish(&staging.0, &destination)
}

fn write(root: &Path, path: &str, content: &str) -> Result<(), String> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create template directory: {error}"))?;
    }
    fs::write(&path, content)
        .map_err(|error| format!("cannot write template {}: {error}", path.display()))
}

struct Staging(PathBuf);

impl Staging {
    fn create(parent: &Path) -> Result<Self, String> {
        loop {
            let sequence = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".dever-new-{}-{sequence}", std::process::id()));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("cannot stage new project: {error}")),
            }
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn publish(staging: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        renameat_with(CWD, staging, CWD, destination, RenameFlags::NOREPLACE)
            .map_err(|error| format!("cannot publish new project: {error}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (staging, destination);
        Err("new project publication is not available for this platform".into())
    }
}
