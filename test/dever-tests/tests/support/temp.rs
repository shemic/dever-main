use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    pub fn new() -> Self {
        Self::new_in(&std::env::temp_dir())
    }

    pub fn new_in(parent: &Path) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = parent.join(format!(
                "dever-core-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        // This object owns only the directory it exclusively created above.
        let _ = fs::remove_dir_all(&self.0);
    }
}
