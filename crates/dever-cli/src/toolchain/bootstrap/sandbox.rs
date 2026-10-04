//! Preserve signed content-addressed helpers across compiler upgrades/removal.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::{
    MachineManager, ReleaseManifest, digest, read_bounded, release, trusted_directory, trusted_file,
};

const MAX_HELPERS: usize = 24;
const MAX_HELPER_BYTES: u64 = 32 * 1024 * 1024;

fn artifact_path() -> String {
    format!(
        "sandbox/{}/bin/bwrap",
        crate::toolchain::platform_identity()
    )
}

pub(super) fn stage(
    installed: &Path,
    next: &Path,
    source: &Path,
    manifest: &ReleaseManifest,
    key: &[u8],
) -> Result<(), String> {
    fs::create_dir(next).map_err(|error| error.to_string())?;
    fs::set_permissions(next, fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    let mut hashes = std::collections::BTreeSet::new();
    if installed.exists() {
        trusted_directory(installed)?;
        for entry in fs::read_dir(installed).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let hash = entry
                .file_name()
                .into_string()
                .map_err(|_| "sandbox helper identity is not UTF-8")?;
            if hashes.len() >= MAX_HELPERS || !valid_hash(&hash) {
                return Err("installed sandbox helper store has unsupported entries or exceeds its version bound".into());
            }
            validate_installed(&entry.path(), &hash, key)?;
            let destination = next.join(&hash);
            copy_helper(&entry.path().join("bwrap"), &entry.path(), &destination)?;
            hashes.insert(hash);
        }
    }
    if let Some(artifact) = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.path == artifact_path())
        && !hashes.contains(&artifact.sha256)
    {
        if hashes.len() >= MAX_HELPERS {
            return Err("sandbox helper version store is full; retained applications must be retired before removing old helpers".into());
        }
        let bytes = read_bounded(&source.join(&artifact.path), MAX_HELPER_BYTES)?;
        if bytes.len() as u64 != artifact.bytes
            || dever_sandbox::validate_bwrap(&bytes)? != artifact.sha256
        {
            return Err("sandbox helper differs from its signed release".into());
        }
        let destination = next.join(&artifact.sha256);
        copy_helper(&source.join(&artifact.path), source, &destination)?;
        validate_installed(&destination, &artifact.sha256, key)?;
    }
    release::sync_tree(next)
}

fn copy_helper(helper: &Path, signed_release: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    release::copy_file(helper, &destination.join("bwrap"), true)?;
    for name in ["manifest.json", "manifest.sig"] {
        release::copy_file(&signed_release.join(name), &destination.join(name), false)?;
    }
    Ok(())
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_installed(root: &Path, hash: &str, key: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    trusted_directory(root)?;
    let names = fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<std::collections::BTreeSet<_>, _>>()
        .map_err(|error| error.to_string())?;
    let expected = ["bwrap", "manifest.json", "manifest.sig"]
        .map(std::ffi::OsString::from)
        .into_iter()
        .collect();
    if names != expected {
        return Err("installed sandbox helper contains unsigned entries".into());
    }
    for name in ["bwrap", "manifest.json", "manifest.sig"] {
        trusted_file(&root.join(name))?;
    }
    let helper = root.join("bwrap");
    if fs::metadata(&helper)
        .map_err(|error| error.to_string())?
        .mode()
        & 0o6111
        != 0o111
    {
        return Err("installed sandbox helper has unsupported executable permissions".into());
    }
    match rustix::fs::lgetxattr(&helper, "security.capability", &mut [0u8; 64]) {
        Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP) => {}
        Ok(_) => return Err("installed sandbox helper must not have file capabilities".into()),
        Err(error) => {
            return Err(format!(
                "cannot inspect installed sandbox helper capabilities: {error}"
            ));
        }
    }
    let manifest = MachineManager::signed_manifest(root, key)?;
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.path == artifact_path())
        .ok_or("installed sandbox helper has no signed artifact")?;
    let bytes = read_bounded(&helper, MAX_HELPER_BYTES)?;
    if artifact.sha256 != hash || artifact.bytes != bytes.len() as u64 || digest(&bytes) != hash {
        return Err(
            "installed sandbox helper differs from its signature or directory identity".into(),
        );
    }
    dever_sandbox::validate_bwrap(&bytes)?;
    Ok(())
}
