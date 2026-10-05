//! Serialize dependency mutations from the first project read to publication.
//! The guard file is stable: replacing dever.lock must not replace its mutex.

use std::fs::{self, File, Metadata, OpenOptions};
use std::path::Path;

pub(crate) struct ProjectMutation {
    _file: File,
}

impl ProjectMutation {
    pub(crate) fn for_command(command: &str, root: &Path) -> Result<Option<Self>, String> {
        if !matches!(command, "add" | "update" | "remove" | "install") {
            return Ok(None);
        }
        let directory = fs::symlink_metadata(root)
            .map_err(|error| format!("cannot inspect project mutation directory: {error}"))?;
        validate(&directory, true)?;
        let path = root.join(".dever-project.lock");
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            validate(&metadata, false)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        let file = options
            .open(&path)
            .map_err(|error| format!("cannot open project mutation lock: {error}"))?;
        let opened = file.metadata().map_err(|error| error.to_string())?;
        validate(&opened, false)?;
        let current = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        validate(&current, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if opened.dev() != current.dev() || opened.ino() != current.ino() {
                return Err("project mutation lock changed while opening".into());
            }
        }
        fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                "project dependency mutation is busy; retry after the current command finishes"
                    .into()
            } else {
                format!("cannot acquire project mutation lock: {error}")
            }
        })?;
        Ok(Some(Self { _file: file }))
    }
}

fn validate(metadata: &Metadata, directory: bool) -> Result<(), String> {
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(
            "project mutation path must be a real directory or regular lock file, without symlinks"
                .into(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let forbidden = if directory { 0o022 } else { 0o077 };
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & forbidden != 0
            || (!directory && metadata.nlink() != 1)
        {
            return Err("project mutation directory must be owned and not group/other writable; lock must be owned, private and unlinked elsewhere".into());
        }
    }
    Ok(())
}
