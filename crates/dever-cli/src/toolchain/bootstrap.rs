//! Explicit Linux first installation. The caller must already trust this installer.
//! Release keys come from an independent administrator input, never the release.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::release::{self, InstallLock};
use super::{Artifact, Layout, MachineManager, ReleaseManifest, Version};

mod sandbox;
mod service;
mod transaction;
pub use service::{RecoveryAction, recovery_actions};

pub(crate) const SERVICE: &str = "[Unit]\nDescription=Dever shared compiler service\nAfter=local-fs.target\n\n[Service]\nType=exec\nUser=root\nGroup=root\nExecStart=/opt/dever/bin/deverd --root /opt/dever\nRuntimeDirectory=dever\nRuntimeDirectoryMode=0755\nUMask=0077\nRestart=on-failure\nRestartSec=1\nKillMode=control-group\nTimeoutStopSec=30\nNoNewPrivileges=yes\nPrivateTmp=yes\nProtectHome=yes\nProtectSystem=strict\nReadWritePaths=/opt/dever/state /opt/dever/cache /run/dever\n\n[Install]\nWantedBy=multi-user.target\n";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    release: PathBuf,
    system_root: PathBuf,
    trusted_key: PathBuf,
    #[serde(default)]
    activate_service: bool,
}

/// Private distribution entry; ordinary version install/update remains unchanged.
pub fn install(installer_root: &Path) -> Result<Version, String> {
    require_root()?;
    absolute_root(installer_root)?;
    trusted_directory(installer_root)?;
    let config = installer_root.join("config/setting.json");
    trusted_file(&config)?;
    let settings: Settings =
        serde_json::from_slice(&read_bounded(&config, 1024 * 1024)?).map_err(|error| {
            format!(
                "invalid installer config at {}:{}",
                error.line(),
                error.column()
            )
        })?;
    absolute_root(&settings.system_root)?;
    trusted_directory(&settings.system_root)?;
    if settings.system_root == Path::new("/") {
        // Publishing an AppArmor file can trigger a later system-wide reload,
        // even when this installer does not load profiles or activate services.
        dever_sandbox::validate_host_policy()?;
    }
    if settings.activate_service && settings.system_root != Path::new("/") {
        return Err(
            "service activation requires the actual system root; image installs are offline".into(),
        );
    }
    let package = absolute_input(installer_root, &settings.release)?;
    let key_path = absolute_input(installer_root, &settings.trusted_key)?;
    trusted_file(&key_path)?;
    let package = fs::canonicalize(package).map_err(|error| error.to_string())?;
    if key_path.starts_with(&package) {
        return Err("trusted key must be supplied independently outside the release".into());
    }
    let key = release::decode_hex_file(&key_path, 32)?;
    create_directory(&settings.system_root, "opt")?;
    let layout = Layout::new(settings.system_root.join("opt/dever"));
    if layout.root().exists() {
        trusted_directory(layout.root())?;
    }
    for directory in [
        layout.bin(),
        layout.state(),
        layout.versions(),
        layout.downloads(),
        layout.staging(),
        layout.native_cache(),
    ] {
        let relative = directory
            .strip_prefix(&settings.system_root)
            .map_err(|error| error.to_string())?
            .to_str()
            .ok_or("installer directory is not UTF-8")?;
        create_directory(&settings.system_root, relative)?;
    }
    layout.validate_machine_permissions()?;
    let _lock = InstallLock::acquire(&layout)?;
    transaction::recover(&settings.system_root, &layout)?;
    let pinned = layout.state().join("trusted-release-key");
    if pinned.exists() {
        trusted_file(&pinned)?;
        if release::decode_hex_file(&pinned, 32)? != key {
            return Err("installer key differs from the machine's pinned release key".into());
        }
    }
    // The private copy is checked again by the version owner before any health
    // command executes. A writable download cannot race signed bytes into it.
    let manifest = MachineManager::verify_package(&package, None, &key)?;
    validate_payload(&package, &manifest.artifacts, true)?;
    pin_key(&layout, &key)?;
    let manager = MachineManager::new(layout.clone());
    if manager
        .active_version()?
        .is_some_and(|active| manifest.version < active)
    {
        return Err("bootstrap installation cannot downgrade the active compiler".into());
    }
    let installed = layout.versions().join(manifest.version.as_str());
    if installed.exists()
        && super::sha256_file(&installed.join("manifest.json"))?
            != super::sha256_file(&package.join("manifest.json"))?
    {
        return Err("installed version already has a different signed release identity".into());
    }
    manager.install_package_locked(&package, &manifest.version)?;
    transaction::install(
        &settings.system_root,
        &layout,
        &installed,
        &manifest,
        &key,
        settings.activate_service,
    )?;
    Ok(manifest.version)
}

/// Recover only this installer's fixed, recorded targets. Normal version
/// operations must finish a pending bootstrap transaction first.
pub fn recover(system_root: &Path) -> Result<(), String> {
    require_root()?;
    absolute_root(system_root)?;
    trusted_directory(system_root)?;
    let layout = Layout::new(system_root.join("opt/dever"));
    layout.validate_machine_permissions()?;
    let _lock = InstallLock::acquire(&layout)?;
    transaction::recover(system_root, &layout)
}

pub(crate) fn validate_payload(
    root: &Path,
    artifacts: &[Artifact],
    current_template: bool,
) -> Result<(), String> {
    let scoped = artifacts
        .iter()
        .filter_map(|artifact| {
            artifact
                .path
                .strip_prefix("bootstrap/")
                .map(|path| Artifact {
                    path: path.into(),
                    bytes: artifact.bytes,
                    sha256: artifact.sha256.clone(),
                })
        })
        .collect::<Vec<_>>();
    validate_files(&root.join("bootstrap"), &scoped, current_template)
}

fn validate_files(root: &Path, artifacts: &[Artifact], new_payload: bool) -> Result<(), String> {
    let bytes = artifacts
        .iter()
        .try_fold(0u64, |total, artifact| total.checked_add(artifact.bytes))
        .ok_or("bootstrap size overflow")?;
    if artifacts.len() > 64 || bytes > 64 * 1024 * 1024 {
        return Err("bootstrap closure exceeds its 64-file/64-MiB budget".into());
    }
    for artifact in artifacts {
        if !matches!(
            artifact.path.as_str(),
            "dever" | "deverd" | "deverd.service" | "dever-sandbox"
        ) && !artifact.path.starts_with("lib/")
        {
            return Err("bootstrap has an unsupported artifact".into());
        }
        let path = super::runtime_pack::checked_path(root, &artifact.path, true)?;
        let bytes = read_bounded(&path, artifact.bytes)?;
        if bytes.len() as u64 != artifact.bytes || digest(&bytes) != artifact.sha256 {
            return Err(format!(
                "bootstrap artifact '{}' differs from its signature",
                artifact.path
            ));
        }
    }
    if !artifacts
        .iter()
        .any(|artifact| artifact.path == "deverd.service")
        || new_payload && read_bounded(&root.join("deverd.service"), 8192)? != SERVICE.as_bytes()
    {
        return Err(
            "bootstrap service definition is absent or differs from the supported contract".into(),
        );
    }
    if new_payload
        && (!artifacts
            .iter()
            .any(|artifact| artifact.path == "dever-sandbox")
            || read_bounded(&root.join("dever-sandbox"), 8192)?
                != dever_sandbox::APPARMOR_PROFILE.as_bytes())
    {
        return Err(
            "bootstrap AppArmor profile is absent or differs from the supported contract".into(),
        );
    }
    super::core_libraries::validate_entries(root, artifacts, &["dever", "deverd"])
}

pub(super) fn health_check(path: &Path, flag: &str) -> Result<(), String> {
    if !service::run(
        std::process::Command::new(path).arg(flag),
        "health-check installed executable",
    )? {
        return Err(format!(
            "installed executable '{}' failed its health check",
            path.display()
        ));
    }
    Ok(())
}

pub(super) fn require_no_pending(layout: &Layout) -> Result<(), String> {
    if fs::symlink_metadata(layout.state().join("bootstrap-install.json")).is_ok() {
        return Err(
            "bootstrap installation is incomplete; rerun the explicit installer to recover it"
                .into(),
        );
    }
    Ok(())
}

fn validate_bin(root: &Path, key: &[u8]) -> Result<Vec<u8>, String> {
    trusted_directory(root)?;
    let manifest = MachineManager::signed_manifest(root, key)?;
    let artifacts = manifest
        .artifacts
        .iter()
        .filter_map(|artifact| {
            artifact
                .path
                .strip_prefix("bootstrap/")
                .map(|path| Artifact {
                    path: path.into(),
                    bytes: artifact.bytes,
                    sha256: artifact.sha256.clone(),
                })
        })
        .collect::<Vec<_>>();
    validate_files(root, &artifacts, false)?;
    let expected = artifacts
        .iter()
        .map(|artifact| artifact.path.as_str())
        .chain(["manifest.json", "manifest.sig"])
        .collect::<std::collections::BTreeSet<_>>();
    let found = transaction::files(root)?;
    if found
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>()
        != expected
    {
        return Err("installed bootstrap contains unsigned files".into());
    }
    read_bounded(&root.join("deverd.service"), 8192)
}

fn pin_key(layout: &Layout, key: &[u8]) -> Result<(), String> {
    let path = layout.state().join("trusted-release-key");
    let next = layout.state().join(".trusted-release-key.next");
    if fs::symlink_metadata(&next).is_ok() {
        trusted_file(&next)?;
        if fs::metadata(&next)
            .map_err(|error| error.to_string())?
            .len()
            > 65
        {
            return Err("temporary release key exceeds its fixed size".into());
        }
        fs::remove_file(&next).map_err(|error| error.to_string())?;
        release::sync_directory(&layout.state()).map_err(|error| error.to_string())?;
    }
    if fs::symlink_metadata(&path).is_ok() {
        trusted_file(&path)?;
        if release::decode_hex_file(&path, 32)? != key {
            return Err("installer key differs from the machine's pinned release key".into());
        }
        return Ok(());
    }
    write_new(&next, format!("{}\n", hex(key)).as_bytes(), 0o644)?;
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        &next,
        rustix::fs::CWD,
        &path,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|error| format!("cannot publish initial release key: {error}"))?;
    release::sync_directory(&layout.state()).map_err(|error| error.to_string())
}

fn absolute_root(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("installer roots must be absolute paths without '..'".into());
    }
    Ok(())
}

fn require_root() -> Result<(), String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("machine bootstrap installation requires root".into());
    }
    Ok(())
}

fn absolute_input(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("installer paths cannot contain '..'".into());
    }
    Ok(path)
}

fn trusted_directory(path: &Path) -> Result<(), String> {
    trusted_path(path, true)
}

fn trusted_file(path: &Path) -> Result<(), String> {
    trusted_path(path, false)
}

fn trusted_path(path: &Path, directory: bool) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "cannot inspect installer path '{}': {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
        || directory && !metadata.is_dir()
        || !directory && (!metadata.is_file() || metadata.nlink() != 1)
    {
        return Err(format!(
            "installer path '{}' must be a real root-owned {} without shared write permission",
            path.display(),
            if directory { "directory" } else { "file" }
        ));
    }
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let parent = fs::symlink_metadata(ancestor).map_err(|error| error.to_string())?;
        // A root-owned sticky temporary ancestor is safe for an owned fixture;
        // the actual installation root itself may never be shared-writable.
        if !parent.is_dir()
            || parent.file_type().is_symlink()
            || parent.uid() != 0
            || parent.mode() & 0o022 != 0 && parent.mode() & 0o1000 == 0
        {
            return Err("installer path crosses an untrusted ancestor".into());
        }
    }
    Ok(())
}

fn create_directory(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut path = root.to_owned();
    for part in Path::new(relative).components() {
        let Component::Normal(part) = part else {
            return Err("invalid installer directory".into());
        };
        path.push(part);
        match fs::create_dir(&path) {
            Ok(()) => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                    .map_err(|error| error.to_string())?;
                release::sync_directory(&path).map_err(|error| error.to_string())?;
                release::sync_directory(path.parent().ok_or("installer directory has no parent")?)
                    .map_err(|error| error.to_string())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
        trusted_directory(&path)?;
    }
    Ok(path)
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(maximum.saturating_add(1)).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > maximum {
        return Err("installer input exceeds its size bound".into());
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    release::sync_directory(path.parent().ok_or("installer file has no parent")?)
        .map_err(|error| error.to_string())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
