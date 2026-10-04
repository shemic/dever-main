//! Hidden source compiler. It never prepares dependencies or executes a project.

use dever_cli::toolchain::compilation::{
    CompileKind, CompileRequest, MAX_COMPILE_BYTES, WORKER_OUTPUT_FILE, WORKER_REQUEST_FILE,
    worker_directory,
};
use std::fs;
use std::io::Read;

pub(super) fn execute() -> Result<(), String> {
    resource_limits()?;
    let (layout, version) = crate::compile::managed_compiler()?
        .ok_or("compile workers require an installed managed compiler")?;
    let directory = worker_directory(&layout)?;
    let path = directory.join(WORKER_REQUEST_FILE);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect compile request: {error}"))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_COMPILE_BYTES as u64
    {
        return Err("compile request must be a bounded regular file".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| {
            file.take(MAX_COMPILE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("cannot read compile request: {error}"))?;
    let request = CompileRequest::decode(&bytes)?;
    drop(bytes);
    if request.version != version {
        return Err("compile request version does not match worker".into());
    }
    let sources = request.source_map()?;
    let checked = match &request.bindings {
        Some(bindings) => dever_core::check_with_bindings(&sources, bindings),
        None => dever_core::check(&sources),
    };
    let render = |errors: Vec<dever_core::diagnostic::Diagnostic>| {
        errors
            .iter()
            .map(|error| error.render(&sources))
            .collect::<String>()
    };
    let program = checked.map_err(render)?;
    if request.kind == CompileKind::Application {
        if request.sources.iter().any(|source| {
            source
                .logical_path
                .rsplit('/')
                .next()
                .is_some_and(|name| name == "main.dever" || name == "main.dever.md")
        }) {
            return Err("application entry is generated from API, CMD, and Job declarations; remove module/main.dever".into());
        }
        let errors = program.application_errors()?;
        if !errors.is_empty() {
            return Err(render(errors));
        }
    }
    crate::compile::worker_compile(
        &program,
        &sources,
        &request,
        &directory,
        &directory.join(WORKER_OUTPUT_FILE),
    )
}

#[cfg(unix)]
fn resource_limits() -> Result<(), String> {
    use rustix::process::{Resource, Rlimit, setrlimit};
    for (resource, limit) in [
        (Resource::Cpu, 120),
        (Resource::As, 2 * 1024 * 1024 * 1024),
        (Resource::Fsize, 1024 * 1024 * 1024),
        (Resource::Nofile, 128),
        (Resource::Core, 0),
    ] {
        setrlimit(
            resource,
            Rlimit {
                current: Some(limit),
                maximum: Some(limit),
            },
        )
        .map_err(|error| format!("cannot restrict compiler worker resources: {error}"))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn resource_limits() -> Result<(), String> {
    Err("managed compiler worker resource limits are unavailable on this platform".into())
}
