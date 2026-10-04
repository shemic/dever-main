//! Periodic scratch accounting is separate from immutable runtime/tool inputs.

use std::fs;
use std::path::Path;

const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 65_536;
const MAX_DEPTH: usize = 128;

#[derive(Default)]
struct Budget {
    bytes: u64,
    entries: usize,
}

pub(super) fn check(root: &Path) -> Result<(), String> {
    visit(root, 0, &mut Budget::default()).map_err(|error| format!("build scratch budget: {error}"))
}

#[cfg(unix)]
fn visit(path: &Path, depth: usize, budget: &mut Budget) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    if depth > MAX_DEPTH {
        return Err("directory depth exceeds limit".into());
    }
    // An open directory pins the inode throughout enumeration. Child-created
    // symlinks and rename races cannot redirect accounting outside this tree.
    let directory = match fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::DIRECTORY).bits() as i32)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot pin scratch directory: {error}")),
    };
    let pinned = format!("/proc/self/fd/{}", directory.as_raw_fd());
    for entry in fs::read_dir(pinned).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        budget.entries += 1;
        if budget.entries > MAX_ENTRIES {
            return Err("entry count exceeds limit".into());
        }
        if metadata.is_dir() {
            visit(&entry.path(), depth + 1, budget)?;
        } else {
            budget.bytes = budget
                .bytes
                .checked_add(metadata.len())
                .ok_or("byte count overflow")?;
            if budget.bytes > MAX_BYTES {
                return Err("bytes exceed 512 MiB limit".into());
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn visit(_path: &Path, _depth: usize, _budget: &mut Budget) -> Result<(), String> {
    Err("managed build scratch accounting requires its Linux sandbox".into())
}
