//! Safe extraction primitives for compiler-owned external Worker resources.
//!
//! The packager supplies verified bytes; this module never downloads, searches
//! PATH, follows links, or executes a resource.  A caller can therefore use
//! the same primitive for an embedded resource table and for a machine-cache
//! hit without creating a writable shared execution directory.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug)]
pub struct Resource<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub sha256: &'a str,
    pub executable: bool,
}

pub(crate) struct PreparedBundle {
    directory: PathBuf,
    resources: Vec<Resource<'static>>,
}

pub fn prepare(root: &Path, digest: &str, resources: &[Resource<'static>]) -> Result<(), String> {
    let session =
        crate::component::current().ok_or("external resources require a Dever invocation")?;
    let directory = extract(root, digest, resources)?;
    session
        .settings
        .get_or_init(|| {
            crate::config::Settings::load_adapter_settings_at(root).map(std::sync::Arc::new)
        })
        .as_ref()
        .map_err(Clone::clone)?;
    session
        .resources
        .set(PreparedBundle {
            directory,
            resources: resources.to_vec(),
        })
        .map_err(|_| "external resources were initialized twice".into())
}

/// A managed Worker never obtains an interpreter from PATH. Its executable,
/// arguments and working directory all come from its verified resource tree.
#[derive(Debug)]
pub struct WorkerLaunch {
    pub executable: PathBuf,
    pub arguments: Vec<std::ffi::OsString>,
    pub working_directory: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchManifest {
    format: String,
    ecosystem: String,
    entry: String,
    executable: String,
    arguments: Vec<LaunchArgument>,
    working_directory: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LaunchArgument {
    Resource(ResourceArgument),
    Literal(LiteralArgument),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceArgument {
    resource: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiteralArgument {
    literal: String,
}

pub fn worker_command(
    definition: &crate::component::Definition,
) -> Result<std::process::Command, String> {
    sandbox_launch(definition, |launch| launch.command())
}

pub(crate) fn observed_command(
    definition: &crate::component::Definition,
) -> Result<(std::process::Command, dever_sandbox::CommandObservation), String> {
    sandbox_launch(definition, |launch| launch.observed_command())
}

fn sandbox_launch<T>(
    definition: &crate::component::Definition,
    build: impl FnOnce(dever_sandbox::Launch<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let session =
        crate::component::current().ok_or("external resources require a Dever invocation")?;
    let bundle = session
        .resources
        .get()
        .ok_or("external Worker resources and sandbox were not prepared")?;
    let launch = verified_worker_launch(
        &bundle.directory,
        &bundle.resources,
        &definition.entry,
        &definition.ecosystem,
    )?;
    let settings = crate::component::adapter_settings()?
        .external_grants(&definition.port, &definition.capabilities)?;
    if definition
        .capabilities
        .iter()
        .any(|capability| capability == "gpu")
    {
        return Err("GPU Worker requires a supported device isolation backend".into());
    }
    let arguments = launch.arguments;
    build(dever_sandbox::Launch {
        assets: &bundle.directory.join("sandbox"),
        worker: &launch.working_directory,
        executable: &launch.executable,
        arguments: &arguments,
        working_directory: &launch.working_directory,
        capabilities: dever_sandbox::Capabilities {
            network: definition
                .capabilities
                .iter()
                .any(|capability| capability == "network"),
            process: definition
                .capabilities
                .iter()
                .any(|capability| capability == "process"),
        },
        grants: &settings,
    })
}

pub fn verified_worker_launch(
    directory: &Path,
    resources: &[Resource<'_>],
    entry: &str,
    ecosystem: &str,
) -> Result<WorkerLaunch, String> {
    let (suffix, format, tree) = match ecosystem {
        "exec" | "pip" | "npm" | "go" => ("worker", "dever-worker-launch-v1", "workers/"),
        "command" => ("command", "dever-command-launch-v1", "commands/"),
        _ => return Err("invalid managed Worker ecosystem".into()),
    };
    let manifest_path = format!("{entry}.dever-{suffix}.json");
    let manifest = resources
        .iter()
        .find(|resource| resource.path == manifest_path)
        .ok_or("managed Worker launch manifest is missing")?;
    if manifest.executable {
        return Err("managed Worker manifest must not be executable".into());
    }
    let text = std::str::from_utf8(manifest.bytes)
        .map_err(|_| "managed Worker manifest must be UTF-8 JSON")?;
    crate::wire::parse(text)
        .map_err(|error| format!("invalid managed Worker manifest: {error}"))?;
    let manifest: LaunchManifest = serde_json::from_str(text)
        .map_err(|error| format!("invalid managed Worker manifest: {error}"))?;
    if manifest.format != format || manifest.ecosystem != ecosystem || manifest.entry != entry {
        return Err("managed Worker launch identity differs from its checked Adapter".into());
    }
    validate_relative_path(&manifest.working_directory)?;
    if !manifest.working_directory.starts_with(tree) || manifest.arguments.len() > 64 {
        return Err("invalid managed Worker execution tree or argument count".into());
    }
    let prefix = format!("{}/", manifest.working_directory);
    let resource_path = |path: &str| -> Result<PathBuf, String> {
        validate_relative_path(path)?;
        if !path.starts_with(&prefix) || !resources.iter().any(|resource| resource.path == path) {
            return Err("managed Worker argument escapes its verified execution tree".into());
        }
        Ok(directory.join(path))
    };
    resource_path(&manifest.executable)?;
    // Recheck the complete tree once, including executable bits, at every
    // Worker start/restart before any launch path is exposed to the supervisor.
    let executable = verified_entry(directory, resources, &manifest.executable)?;
    let mut total_bytes = 0_usize;
    let mut arguments = Vec::with_capacity(manifest.arguments.len());
    for argument in manifest.arguments {
        let argument = match argument {
            LaunchArgument::Resource(argument) => {
                resource_path(&argument.resource)?.into_os_string()
            }
            LaunchArgument::Literal(argument) => {
                if argument.literal.contains('\0') {
                    return Err("managed Worker argument contains NUL".into());
                }
                argument.literal.into()
            }
        };
        total_bytes = total_bytes.saturating_add(argument.as_encoded_bytes().len());
        if total_bytes > 65_536 {
            return Err("managed Worker arguments exceed the byte budget".into());
        }
        arguments.push(argument);
    }
    let working_directory = directory.join(manifest.working_directory);
    if !working_directory.is_dir() {
        return Err("managed Worker execution directory is missing".into());
    }
    reject_symlink_ancestors(&working_directory)?;
    Ok(WorkerLaunch {
        executable,
        arguments,
        working_directory,
    })
}

pub fn verified_entry(
    directory: &Path,
    resources: &[Resource<'_>],
    entry: &str,
) -> Result<PathBuf, String> {
    let resource = resources
        .iter()
        .find(|resource| resource.path == entry)
        .ok_or("external Adapter entry is not in the verified manifest")?;
    if !resource.executable {
        return Err("external Adapter entry is not executable in the verified manifest".into());
    }
    verify_directory(directory, resources)?;
    let path = directory.join(entry);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .permissions()
            .mode()
            & 0o100
            == 0
        {
            return Err("external Adapter entry has no owner execute permission".into());
        }
    }
    path.canonicalize()
        .map_err(|error| format!("cannot resolve verified external Adapter entry: {error}"))
}

pub fn bundle_digest(resources: &[Resource<'_>]) -> String {
    let mut digest = Sha256::new();
    for resource in resources {
        digest.update(resource.path.as_bytes());
        digest.update([0]);
        digest.update(resource.sha256.as_bytes());
        digest.update([0]);
        digest.update((resource.bytes.len() as u64).to_le_bytes());
        digest.update([u8::from(resource.executable)]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn extract(root: &Path, digest: &str, resources: &[Resource<'_>]) -> Result<PathBuf, String> {
    validate_digest(digest)?;
    let mut paths = std::collections::BTreeSet::new();
    for resource in resources {
        validate_resource(resource)?;
        if !paths.insert(resource.path) {
            return Err(format!(
                "duplicate external resource path '{}'",
                resource.path
            ));
        }
    }
    if bundle_digest(resources) != digest {
        return Err("external resource bundle identity does not match its manifest".into());
    }
    ensure_cache_directory(root)?;
    let target = root.join("data/cache/lib").join(digest);
    if target.exists() {
        verify_directory(&target, resources)?;
        return Ok(target);
    }
    let parent = target.parent().ok_or("external resource has no parent")?;
    let staging = parent.join(format!(
        ".staging-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    create_private_directory(&staging)?;
    let result = (|| {
        let mut paths = std::collections::BTreeSet::new();
        for resource in resources {
            validate_resource(resource)?;
            if !paths.insert(resource.path) {
                return Err(format!(
                    "duplicate external resource path '{}'",
                    resource.path
                ));
            }
            let path = staging.join(resource.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&path).map_err(|error| error.to_string())?;
            file.write_all(resource.bytes)
                .map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            #[cfg(unix)]
            if resource.executable {
                set_executable(&file)?;
            }
        }
        sync_directory(&staging)?;
        match fs::rename(&staging, &target) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                verify_directory(&target, resources)?;
                let _ = fs::remove_dir_all(&staging);
            }
            Err(error) => return Err(error.to_string()),
        }
        sync_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
        .map(|()| target)
        .map_err(|error: String| format!("cannot publish external resources: {error}"))
}

fn verify_directory(root: &Path, resources: &[Resource<'_>]) -> Result<(), String> {
    reject_symlink_ancestors(root)?;
    let metadata = fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if !metadata.is_dir() {
        return Err("external resource cache entry is not a directory".into());
    }
    let parent = root
        .parent()
        .ok_or("external resource cache has no parent")?;
    validate_owner(
        root,
        &fs::metadata(parent).map_err(|error| error.to_string())?,
        true,
    )?;
    let mut expected = std::collections::BTreeSet::new();
    for resource in resources {
        validate_resource(resource)?;
        if !expected.insert(resource.path) {
            return Err("duplicate external resource path".into());
        }
        let path = root.join(resource.path);
        reject_symlink_ancestors(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "cannot inspect external resource '{}': {error}",
                resource.path
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "external resource '{}' is not a regular file",
                resource.path
            ));
        }
        validate_owner(
            &path,
            &fs::metadata(root).map_err(|error| error.to_string())?,
            true,
        )?;
        if !verify_resource_file(&path, resource)? {
            return Err(format!(
                "external resource '{}' failed integrity verification",
                resource.path
            ));
        }
    }
    let mut actual = Vec::new();
    collect_files(root, root, &mut actual)?;
    if actual.iter().any(|path| !expected.contains(path.as_str())) || actual.len() != expected.len()
    {
        return Err("external resource cache contains files outside the verified manifest".into());
    }
    Ok(())
}

fn verify_resource_file(path: &Path, resource: &Resource<'_>) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() != resource.bytes.len() as u64 {
        return Ok(false);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut offset = 0;
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        let Some(expected) = resource.bytes.get(offset..offset + read) else {
            return Ok(false);
        };
        if expected != &buffer[..read] {
            return Ok(false);
        }
        digest.update(&buffer[..read]);
        offset += read;
    }
    let actual = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(offset == resource.bytes.len() && actual == resource.sha256)
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<String>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "external resource cache contains a symbolic link: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            validate_owner(
                &path,
                &fs::metadata(root).map_err(|error| error.to_string())?,
                false,
            )?;
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_str()
                .ok_or("external resource path is not UTF-8")?;
            files.push(relative.replace(std::path::MAIN_SEPARATOR, "/"));
        } else {
            return Err(format!(
                "external resource cache contains a non-regular entry: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn ensure_cache_directory(root: &Path) -> Result<(), String> {
    reject_symlink_ancestors(root)?;
    if !root.exists() {
        create_private_directory(root)?;
    }
    if !fs::symlink_metadata(root)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("external application root must be a directory".into());
    }
    let owner = fs::metadata(root).map_err(|error| error.to_string())?;
    validate_owner(root, &owner, false)?;
    let data = root.join("data");
    let cache = data.join("cache");
    let lib = cache.join("lib");
    for path in [&data, &cache, &lib] {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "external resource directory '{}' is a symbolic link",
                    path.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "external resource directory '{}' is not a directory",
                    path.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                match builder.create(path) {
                    Ok(()) => {}
                    Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {
                        let metadata =
                            fs::symlink_metadata(path).map_err(|error| error.to_string())?;
                        if !metadata.is_dir() || metadata.file_type().is_symlink() {
                            return Err(
                                "external resource directory changed during creation".into()
                            );
                        }
                    }
                    Err(create_error) => {
                        return Err(format!(
                            "cannot create external resource cache '{}': {create_error}",
                            path.display()
                        ));
                    }
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                        .map_err(|permission_error| permission_error.to_string())?;
                }
            }
            Err(error) => {
                return Err(format!(
                    "cannot inspect external resource directory '{}': {error}",
                    path.display()
                ));
            }
        }
        validate_owner(path, &owner, path == &lib)?;
    }
    Ok(())
}

fn validate_resource(resource: &Resource<'_>) -> Result<(), String> {
    validate_relative_path(resource.path)?;
    if sha256(resource.bytes) != resource.sha256 {
        return Err(format!(
            "external resource '{}' has an invalid digest",
            resource.path
        ));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains(['\\', ':', '\0'])
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "external resource path '{value}' is not safely relative"
        ));
    }
    Ok(())
}

fn reject_symlink_ancestors(path: &Path) -> Result<(), String> {
    let mut ancestor = PathBuf::new();
    for component in path.components() {
        ancestor.push(component);
        match fs::symlink_metadata(&ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("external resource path crosses a symbolic link".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(format!("cannot inspect external resource path: {error}")),
        }
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("external resource digest must be lowercase SHA-256".into());
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|error| error.to_string())
}

fn validate_owner(path: &Path, owner: &fs::Metadata, private: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        let prohibited = if private { 0o077 } else { 0o022 };
        if metadata.uid() != owner.uid() || metadata.mode() & prohibited != 0 {
            return Err(format!(
                "external resource path '{}' has an untrusted owner or permissions",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, owner, private);
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .map_err(|error| error.to_string())?
        .sync_all()
        .map_err(|error| error.to_string())
}

#[cfg(unix)]
fn set_executable(file: &File) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = file
        .metadata()
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o700);
    file.set_permissions(permissions)
        .map_err(|error| error.to_string())
}

fn unique_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_is_verified_and_rejects_path_escape() {
        let root = std::env::temp_dir().join(format!("dever-external-{}", std::process::id()));
        let bytes = b"worker";
        let digest = sha256(bytes);
        let resources = [Resource {
            path: "worker.bin",
            bytes,
            sha256: &digest,
            executable: false,
        }];
        let bundle = bundle_digest(&resources);
        let directory = extract(&root, &bundle, &resources).unwrap();
        assert_eq!(fs::read(directory.join("worker.bin")).unwrap(), bytes);
        fs::write(directory.join("unexpected.bin"), b"unexpected").unwrap();
        assert!(extract(&root, &bundle, &resources).is_err());
        assert!(
            extract(
                &root,
                &bundle,
                &[Resource {
                    path: "../escape",
                    bytes,
                    sha256: &digest,
                    executable: false
                }]
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(root);
    }
}
