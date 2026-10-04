use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use dever_core::hir::Program;

pub(super) fn check_baseline(program: &Program, root: &Path) -> Result<(), String> {
    let path = root.join("dever.api");
    let baseline = match fs::read_to_string(&path) {
        Ok(baseline) => baseline,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot read API baseline '{}': {error}",
                path.display()
            ));
        }
    };
    program
        .check_api(&baseline)
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub(super) fn write(program: &Program, output: Option<&Path>) -> Result<ExitCode, String> {
    let snapshot = program.api_snapshot();
    match output {
        None => io::stdout()
            .lock()
            .write_all(snapshot.as_bytes())
            .map_err(|error| error.to_string())?,
        Some(path) => {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|error| {
                    format!("cannot create API snapshot '{}': {error}", path.display())
                })?;
            file.write_all(snapshot.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|error| {
                    format!("cannot write API snapshot '{}': {error}", path.display())
                })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
