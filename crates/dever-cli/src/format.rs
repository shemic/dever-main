use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use dever_core::source::SourceMap;

pub(super) fn execute(root: &Path, sources: &SourceMap, check: bool) -> Result<ExitCode, String> {
    let mut changes = Vec::new();
    let mut diagnostics = Vec::new();
    for source in sources.files() {
        match dever_core::format::format(source) {
            Ok(formatted) if formatted != source.text() => {
                changes.push((source.path(), destination(root, source), formatted));
            }
            Ok(_) => {}
            Err(errors) => diagnostics.extend(errors),
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics
            .iter()
            .map(|error| error.render(sources))
            .collect());
    }
    if check {
        for (path, _, _) in &changes {
            writeln!(std::io::stdout().lock(), "{}", path.display())
                .map_err(|error| error.to_string())?;
        }
        return Ok(if changes.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }

    // Prepare all replacements before changing any source. A syntax, permission or staging
    // failure leaves the original files intact; each final replacement is an atomic rename.
    validate_snapshot(root, sources)?;
    let mut staged = Vec::new();
    for (_, destination, formatted) in changes {
        staged.push(Replacement::prepare(destination, &formatted)?);
    }
    validate_snapshot(root, sources)?;
    for replacement in &mut staged {
        replacement.commit()?;
    }
    Ok(ExitCode::SUCCESS)
}

fn validate_snapshot(root: &Path, sources: &SourceMap) -> Result<(), String> {
    // Reuse loading rules so added/deleted paths and new symlinks also invalidate the snapshot.
    let current =
        SourceMap::load_project(&root.join("module"), &root.join("test")).map_err(|errors| {
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })?;
    if !current
        .files()
        .iter()
        .map(|source| source.path())
        .eq(sources.files().iter().map(|source| source.path()))
    {
        return Err(format!(
            "{}: source file set changed during formatting",
            root.display()
        ));
    }
    for (current, source) in current.files().iter().zip(sources.files()) {
        if current.text() != source.text() {
            return Err(format!(
                "{}: source changed during formatting",
                destination(root, source).display()
            ));
        }
    }
    Ok(())
}

fn destination(root: &Path, source: &dever_core::source::SourceFile) -> PathBuf {
    if source.is_test() {
        root.join(source.path())
    } else {
        root.join("module").join(source.path())
    }
}

struct Replacement {
    destination: PathBuf,
    temporary: Option<PathBuf>,
}

impl Replacement {
    fn prepare(destination: PathBuf, formatted: &str) -> Result<Self, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let metadata =
            fs::metadata(&destination).map_err(|error| file_error(&destination, error))?;
        if metadata.permissions().readonly() {
            return Err(format!("{}: source is read-only", destination.display()));
        }
        let parent = destination.parent().expect("source files have a parent");
        let (temporary, mut file) = loop {
            let temporary = parent.join(format!(
                ".dever-fmt-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(file_error(&destination, error)),
            }
        };
        let replacement = Self {
            destination,
            temporary: Some(temporary),
        };
        file.write_all(formatted.as_bytes())
            .and_then(|()| file.set_permissions(metadata.permissions()))
            .and_then(|()| file.sync_all())
            .map_err(|error| file_error(&replacement.destination, error))?;
        Ok(replacement)
    }

    fn commit(&mut self) -> Result<(), String> {
        let temporary = self.temporary.as_ref().expect("replacement is pending");
        fs::rename(temporary, &self.destination)
            .map_err(|error| file_error(&self.destination, error))?;
        self.temporary = None;
        Ok(())
    }
}

impl Drop for Replacement {
    fn drop(&mut self) {
        if let Some(temporary) = &self.temporary {
            let _ = fs::remove_file(temporary);
        }
    }
}

fn file_error(path: &Path, error: std::io::Error) -> String {
    format!("{}: cannot format source: {error}", path.display())
}
