//! The fixed Linux execution boundary shared by Lib runtime and build owners.
//! Callers verify signed assets and own every child; this crate never discovers tools.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

mod assets;
mod mounts;
mod observation;
mod trusted_helper;
pub use assets::{
    validate_assets, validate_assets_for_target, validate_bwrap, validate_bwrap_for_target,
};
pub use observation::CommandNamespace;
pub use trusted_helper::{APPARMOR_PROFILE, TRUSTED_HELPER_ROOT, validate_host_policy};

/// Executable modes are part of the bootstrap resource contract. The private
/// loader is needed by the guard and packaged language runtimes. Authoring and
/// embedding can run on a different host; target validation belongs to assets.
pub fn asset_executable(path: &str) -> bool {
    matches!(
        path,
        "bin/bwrap" | "bin/guard" | "lib/ld-linux-x86-64.so.2" | "lib/ld-linux-aarch64.so.1"
    )
}

#[cfg(target_os = "linux")]
mod filter;

#[derive(Clone, Copy, Debug, Default)]
pub struct Capabilities {
    pub network: bool,
    pub process: bool,
}

/// The source is a parent-validated deployment grant, never a Worker request.
#[derive(Debug)]
pub struct Grant {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub writable: bool,
}

pub struct Launch<'a> {
    pub assets: &'a Path,
    pub worker: &'a Path,
    pub executable: &'a Path,
    pub arguments: &'a [OsString],
    pub working_directory: &'a Path,
    pub capabilities: Capabilities,
    pub grants: &'a [Grant],
}

/// Guard exec confirmation and PID-namespace lifetime, separate from user I/O.
pub struct CommandObservation {
    pub startup: std::fs::File,
    pub namespace: std::fs::File,
    pub release: std::fs::File,
}

/// A private ELF closure cannot honor additional host preload libraries.
/// Applies to compiler launches as well as the sandbox bootstrap.
pub fn require_no_loader_preload() -> Result<(), String> {
    match std::fs::symlink_metadata("/etc/ld.so.preload") {
        Ok(_) => {
            Err("private executable does not support host loader preload configuration".into())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect host loader preload configuration: {error}"
        )),
    }
}

impl Launch<'_> {
    pub fn command(&self) -> Result<Command, String> {
        self.command_with_tools(None, None)
    }

    #[cfg(target_os = "linux")]
    pub fn observed_command(&self) -> Result<(Command, CommandObservation), String> {
        use std::os::fd::OwnedFd;
        let (startup, startup_writer) = std::io::pipe().map_err(|error| error.to_string())?;
        let (namespace, namespace_writer) = std::io::pipe().map_err(|error| error.to_string())?;
        let (blocked, release) = std::io::pipe().map_err(|error| error.to_string())?;
        let command = self.command_with_tools(
            None,
            Some([
                std::fs::File::from(OwnedFd::from(startup_writer)),
                std::fs::File::from(OwnedFd::from(namespace_writer)),
                std::fs::File::from(OwnedFd::from(blocked)),
            ]),
        )?;
        Ok((
            command,
            CommandObservation {
                startup: OwnedFd::from(startup).into(),
                namespace: OwnedFd::from(namespace).into(),
                release: OwnedFd::from(release).into(),
            },
        ))
    }

    #[cfg(not(target_os = "linux"))]
    pub fn observed_command(&self) -> Result<(Command, CommandObservation), String> {
        Err("external command sandbox is not supported on this platform".into())
    }

    /// Only the explicit package-build owner may supply this verified signed
    /// root. Ordinary Worker launches never acquire a compiler or /usr mount.
    pub fn command_for_build(&self, tool_root: &Path) -> Result<Command, String> {
        if self.capabilities.network || !self.capabilities.process {
            return Err(
                "package builds require isolated network and child-process capability".into(),
            );
        }
        self.command_with_tools(Some(tool_root), None)
    }

    fn command_with_tools(
        &self,
        tool_root: Option<&Path>,
        observation: Option<[std::fs::File; 3]>,
    ) -> Result<Command, String> {
        if !cfg!(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )) {
            return Err("external Worker sandbox is not supported on this platform".into());
        }
        let assets = real_path(self.assets)?;
        let bwrap_hash = assets::verify_directory(&assets)?;
        require_no_loader_preload()?;
        let worker = real_path(self.worker)?;
        let tool_root = tool_root.map(real_path).transpose()?;
        let executable = mapped_path(&worker, self.executable)?;
        let working_directory = mapped_path(&worker, self.working_directory)?;
        let loader = assets.join("lib").join(loader_name());
        for file in [
            &loader,
            &assets.join("bin/bwrap"),
            &assets.join("bin/guard"),
        ] {
            if !real_path(file)?.is_file() {
                return Err("sandbox executable asset is not a regular file".into());
            }
        }
        // A direct static exec is required for the trusted AppArmor attachment.
        let bwrap = trusted_helper::executable(&assets, &bwrap_hash)?;
        let mut command = Command::new(bwrap);
        command.env_clear().args([
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
        ]);
        if self.capabilities.network {
            command.arg("--share-net");
        }
        let mut mounts = mounts::Mounts::default();
        let startup_fd = if let Some([startup, namespace, blocked]) = observation {
            let startup = mounts.pass(startup)?;
            let namespace = mounts.pass(namespace)?;
            let blocked = mounts.pass(blocked)?;
            command
                .arg("--info-fd")
                .arg(namespace.to_string())
                .arg("--block-fd")
                .arg(blocked.to_string());
            Some(startup)
        } else {
            None
        };
        mounts.bind(&mut command, &assets, Path::new("/sandbox"), false)?;
        mounts.bind(&mut command, &assets.join("lib"), Path::new("/lib"), false)?;
        command.args(["--symlink", "lib", "/lib64"]);
        mounts.bind(&mut command, &worker, Path::new("/worker"), false)?;
        if let Some(root) = &tool_root {
            mounts.bind(&mut command, &root.join("usr"), Path::new("/usr"), false)?;
            command.args(["--symlink", "usr/bin", "/bin"]);
        }
        command.args([
            "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--dir", "/data",
        ]);
        if self.capabilities.network {
            // DNS is an OS service input, never application configuration.
            // Expose only these bounded resolver files, not the host's /etc.
            for name in ["resolv.conf", "hosts"] {
                let source = Path::new("/etc")
                    .join(name)
                    .canonicalize()
                    .map_err(|error| format!("cannot resolve network resolver input: {error}"))?;
                let source = real_path(&source)?;
                let metadata = std::fs::metadata(&source).map_err(|error| error.to_string())?;
                if !metadata.is_file() || metadata.len() > 65_536 {
                    return Err("network resolver input is not a bounded regular file".into());
                }
                mounts.bind(&mut command, &source, &Path::new("/etc").join(name), false)?;
            }
        }
        let mut destinations = BTreeSet::new();
        for grant in self.grants {
            let source = real_path(&grant.source)?;
            if grant.writable
                && [&assets, &worker]
                    .into_iter()
                    .chain(tool_root.iter())
                    .any(|protected| {
                        source.starts_with(protected) || protected.starts_with(&source)
                    })
            {
                return Err("sandbox writable grant overlaps executable inputs".into());
            }
            let destination = &grant.destination;
            normalized_absolute(destination)?;
            if !destination.starts_with("/data") || destination == Path::new("/data") {
                return Err("sandbox file grant must have a path below /data".into());
            }
            if destinations.iter().any(|prior: &&Path| {
                prior.starts_with(destination) || destination.starts_with(prior)
            }) {
                return Err("sandbox file grants overlap".into());
            }
            destinations.insert(destination.as_path());
            mounts.bind(&mut command, &source, destination, grant.writable)?;
        }
        let flags =
            u8::from(self.capabilities.network) | (u8::from(self.capabilities.process) << 1);
        command
            .arg("--chdir")
            .arg(working_directory)
            .args(["--", "/sandbox/bin/guard"]);
        if let Some(fd) = startup_fd {
            command.arg("--observe").arg(fd.to_string());
        }
        command.arg(flags.to_string()).arg(executable);
        for argument in self.arguments {
            // Only resource arguments are absolute; fixed flags stay literal.
            // Other absolute arguments cannot silently expose a host pathname.
            let path = Path::new(argument);
            command.arg(if path.is_absolute() {
                self.argument_path(path)?.into_os_string()
            } else {
                argument.clone()
            });
        }
        mounts.attach(&mut command)?;
        Ok(command)
    }

    /// Build manifests need the same names as command arguments. Output files
    /// may not exist yet; their owning mount was already validated at launch.
    pub fn argument_path(&self, path: &Path) -> Result<PathBuf, String> {
        normalized_absolute(path)?;
        for grant in self.grants {
            if let Ok(relative) = path.strip_prefix(&grant.source) {
                return Ok(grant.destination.join(relative));
            }
        }
        let relative = path
            .strip_prefix(self.worker)
            .map_err(|_| "sandbox argument escapes its owned mounts")?;
        Ok(Path::new("/worker").join(relative))
    }
}

pub fn loader_name() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "ld-linux-aarch64.so.1"
    } else {
        "ld-linux-x86-64.so.2"
    }
}

fn normalized_absolute(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err("sandbox path must be absolute and normalized".into());
    }
    Ok(())
}

fn real_path(path: &Path) -> Result<PathBuf, String> {
    normalized_absolute(path)?;
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)
            .map_err(|error| format!("cannot inspect sandbox input: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("sandbox input must not contain symbolic links".into());
        }
    }
    Ok(path.to_owned())
}

fn mapped_path(worker: &Path, path: &Path) -> Result<PathBuf, String> {
    let path = real_path(path)?;
    let relative = path
        .strip_prefix(worker)
        .map_err(|_| "sandbox execution path escapes the Worker tree")?;
    Ok(Path::new("/worker").join(relative))
}

/// Called only by the single-threaded guard immediately before exec. The filter
/// is inherited by exec and every subsequently created thread or child.
pub fn restrict(capabilities: Capabilities) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        filter::apply(capabilities)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = capabilities;
        Err("sandbox is not supported on this platform".into())
    }
}
