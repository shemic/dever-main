use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static NEXT_BUILD: AtomicU64 = AtomicU64::new(0);
const BUILD_DIRECTORY_PREFIX: &str = "dever-build-";
const BUILD_MARKER: &str = ".dever-owner";
const BUILD_MARKER_VERSION: &str = "dever-build-owner-v1";
const STALE_BUILD_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CleanupSummary {
    pub directories: u64,
    pub files: u64,
    pub bytes: u64,
}

pub(super) struct BuildDirectory(pub(super) PathBuf);

impl Drop for BuildDirectory {
    fn drop(&mut self) {
        // Cleanup cannot replace a compilation/runtime result with a second error.
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn build_directory() -> io::Result<PathBuf> {
    let path = build_directory_at(&std::env::temp_dir())?;
    if let Err(error) = write_build_marker(&path, SystemTime::now()) {
        let _ = fs::remove_dir_all(&path);
        return Err(error);
    }
    Ok(path)
}

pub(super) fn build_directory_at(parent: &Path) -> io::Result<PathBuf> {
    loop {
        let id = NEXT_BUILD.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            "{BUILD_DIRECTORY_PREFIX}{}-{id}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

pub fn clean_project_artifacts(project_root: &Path) -> io::Result<CleanupSummary> {
    let now = SystemTime::now();
    let mut summary = clean_stale_build_directories(now)?;
    let owner = current_owner_id(&std::env::temp_dir())?;
    let entries = match fs::read_dir(project_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(summary),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(legacy_run_pid) else {
            continue;
        };
        if pid == u64::from(std::process::id()) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || !owner_matches(&metadata, owner)
        {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if !is_stale(modified, now) {
            continue;
        }
        if process_is_active(pid) {
            continue;
        }
        let bytes = metadata.len();
        fs::remove_file(entry.path())?;
        summary.files += 1;
        summary.bytes += bytes;
    }
    Ok(summary)
}

pub(super) fn clean_stale_build_directories(now: SystemTime) -> io::Result<CleanupSummary> {
    let root = std::env::temp_dir();
    let owner = current_owner_id(&root)?;
    let mut summary = CleanupSummary::default();
    for entry in fs::read_dir(&root)? {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(build_directory_pid) else {
            continue;
        };
        if pid == u64::from(std::process::id()) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || !owner_matches(&metadata, owner)
        {
            continue;
        }
        let Ok(Some(marker)) = read_build_marker(&path) else {
            continue;
        };
        if marker.pid != pid
            || marker.owner != owner
            || !is_stale(marker.created, now)
            || process_is_active(pid)
        {
            continue;
        }
        let Ok(Some((files, bytes))) = directory_usage(&path) else {
            continue;
        };
        fs::remove_dir_all(&path)?;
        summary.directories += 1;
        summary.files += files;
        summary.bytes += bytes;
    }
    Ok(summary)
}

struct BuildMarker {
    created: SystemTime,
    pid: u64,
    owner: u64,
}

fn write_build_marker(path: &Path, created: SystemTime) -> io::Result<()> {
    let seconds = created
        .duration_since(UNIX_EPOCH)
        .map_err(|_| io::Error::other("system clock is before the Unix epoch"))?
        .as_secs();
    let owner = metadata_owner(&fs::metadata(path)?);
    fs::write(
        path.join(BUILD_MARKER),
        format!(
            "{BUILD_MARKER_VERSION}\ncreated={seconds}\npid={}\nowner={owner}\ncompiler={}\n",
            std::process::id(),
            env!("CARGO_PKG_VERSION"),
        ),
    )
}

fn read_build_marker(path: &Path) -> io::Result<Option<BuildMarker>> {
    let marker_path = path.join(BUILD_MARKER);
    let metadata = match fs::symlink_metadata(&marker_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(marker_path)?;
    let mut lines = text.lines();
    if lines.next() != Some(BUILD_MARKER_VERSION) {
        return Ok(None);
    }
    let Some(created) = parse_marker_number(lines.next(), "created") else {
        return Ok(None);
    };
    let Some(pid) = parse_marker_number(lines.next(), "pid") else {
        return Ok(None);
    };
    let Some(owner) = parse_marker_number(lines.next(), "owner") else {
        return Ok(None);
    };
    let compiler = lines.next().and_then(|line| line.strip_prefix("compiler="));
    if compiler != Some(env!("CARGO_PKG_VERSION")) || lines.next().is_some() {
        return Ok(None);
    }
    Ok(Some(BuildMarker {
        created: UNIX_EPOCH + Duration::from_secs(created),
        pid,
        owner,
    }))
}

fn parse_marker_number(line: Option<&str>, key: &str) -> Option<u64> {
    line.and_then(|line| line.strip_prefix(key))
        .and_then(|line| line.strip_prefix('='))
        .and_then(|value| value.parse().ok())
}

fn build_directory_pid(name: &str) -> Option<u64> {
    let suffix = name.strip_prefix(BUILD_DIRECTORY_PREFIX)?;
    let (pid, id) = suffix.split_once('-')?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    pid.parse().ok()
}

fn legacy_run_pid(name: &str) -> Option<u64> {
    let pid = name.strip_prefix(".dever-run-")?;
    if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    pid.parse().ok()
}

fn is_stale(created: SystemTime, now: SystemTime) -> bool {
    now.duration_since(created)
        .is_ok_and(|age| age >= STALE_BUILD_AGE)
}

#[cfg(target_os = "linux")]
fn process_is_active(pid: u64) -> bool {
    match fs::symlink_metadata(Path::new("/proc").join(pid.to_string())) {
        Ok(metadata) => metadata.is_dir() && !metadata.file_type().is_symlink(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_is_active(pid: u64) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_or(true, |status| status.success())
}

#[cfg(windows)]
fn process_is_active(pid: u64) -> bool {
    let filter = format!("PID eq {pid}");
    let Ok(output) = std::process::Command::new("tasklist.exe")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let expected = format!("\"{pid}\"");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.split(',').nth(1) == Some(expected.as_str()))
}

#[cfg(not(any(unix, windows)))]
fn process_is_active(_pid: u64) -> bool {
    true
}

fn directory_usage(path: &Path) -> io::Result<Option<(u64, u64)>> {
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            return Ok(None);
        }
        if metadata.is_dir() {
            let Some((child_files, child_bytes)) = directory_usage(&entry.path())? else {
                return Ok(None);
            };
            files += child_files;
            bytes += child_bytes;
        } else if metadata.is_file() {
            files += 1;
            bytes += metadata.len();
        } else {
            return Ok(None);
        }
    }
    Ok(Some((files, bytes)))
}

#[cfg(unix)]
fn metadata_owner(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    u64::from(metadata.uid())
}

#[cfg(not(unix))]
fn metadata_owner(_metadata: &fs::Metadata) -> u64 {
    0
}

fn current_owner_id(parent: &Path) -> io::Result<u64> {
    loop {
        let path = parent.join(format!(
            ".dever-owner-probe-{}-{}",
            std::process::id(),
            NEXT_BUILD.fetch_add(1, Ordering::Relaxed),
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => {
                let result = file.metadata().map(|metadata| metadata_owner(&metadata));
                drop(file);
                let _ = fs::remove_file(path);
                return result;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn owner_matches(metadata: &fs::Metadata, owner: u64) -> bool {
    metadata_owner(metadata) == owner
}
