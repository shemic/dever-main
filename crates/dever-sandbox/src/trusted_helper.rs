//! The only host integration is a signed, administrator-installed static helper.
use std::path::{Path, PathBuf};

pub const TRUSTED_HELPER_ROOT: &str = "/opt/dever/sandbox";
pub const APPARMOR_PROFILE: &str = include_str!("../../../sdk/apparmor/dever-sandbox");

#[cfg(all(test, target_os = "linux"))]
#[path = "../../../test/dever-sandbox-tests/trusted_helper.rs"]
mod tests;

pub(super) fn executable(assets: &Path, expected: &str) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    if rustix::process::geteuid().as_raw() != 0 && validate_host_policy()? {
        let path = Path::new(TRUSTED_HELPER_ROOT).join(expected).join("bwrap");
        verify_helper(&path, expected)?;
        require_profiles()?;
        return Ok(path);
    }
    let _ = expected;
    Ok(assets.join("bin/bwrap"))
}

/// Validate the host prerequisite shared by non-root launch and real-host
/// policy installation. This only reads kernel state; administrators own it.
pub fn validate_host_policy() -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        host_policy(|path| std::fs::read_to_string(path))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(false)
    }
}

#[cfg(target_os = "linux")]
fn host_policy(read: impl Fn(&str) -> std::io::Result<String>) -> Result<bool, String> {
    let restricted = policy_flag(&read, "apparmor_restrict_unprivileged_userns")?.unwrap_or(false);
    if !restricted {
        return Ok(false);
    }
    if policy_flag(&read, "apparmor_restrict_unprivileged_unconfined")? != Some(true) {
        return Err("Dever AppArmor deployment requires kernel.apparmor_restrict_unprivileged_unconfined=1 while user namespace restriction is enabled; an administrator must approve strict profile transitions before installing or loading the Dever policy".into());
    }
    Ok(true)
}

#[cfg(target_os = "linux")]
fn policy_flag(
    read: &impl Fn(&str) -> std::io::Result<String>,
    name: &str,
) -> Result<Option<bool>, String> {
    match read(&format!("/proc/sys/kernel/{name}")) {
        Ok(value) => match value.trim() {
            "0" => Ok(Some(false)),
            "1" => Ok(Some(true)),
            _ => Err(format!(
                "invalid AppArmor kernel flag '{name}': expected 0 or 1"
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "cannot inspect AppArmor kernel flag '{name}': {error}"
        )),
    }
}

#[cfg(target_os = "linux")]
fn verify_helper(path: &Path, expected: &str) -> Result<(), String> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    for (index, ancestor) in path.ancestors().enumerate() {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(|error| format!(
            "trusted sandbox helper is missing or inaccessible at '{}': {error}; install the matching signed Dever sandbox assets", path.display()))?;
        if metadata.file_type().is_symlink()
            || metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
            || (index == 0
                && (!metadata.is_file()
                    || metadata.mode() & 0o6111 != 0o111
                    || metadata.nlink() != 1))
            || (index != 0 && !metadata.is_dir())
        {
            return Err("trusted sandbox helper requires real root-owned paths without shared writes, setuid or setgid".into());
        }
    }
    match rustix::fs::lgetxattr(path, "security.capability", &mut [0u8; 64]) {
        Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP) => {}
        Ok(_) => return Err("trusted sandbox helper must not have file capabilities".into()),
        Err(error) => {
            return Err(format!(
                "cannot inspect sandbox helper capabilities: {error}"
            ));
        }
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read trusted sandbox helper: {error}"))?;
    if super::validate_bwrap(&bytes)? != expected {
        return Err("trusted sandbox helper differs from the embedded signed asset".into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn require_profiles() -> Result<(), String> {
    // The profiles list requires policy-admin privilege; the policy filesystem
    // exposes each profile's name/mode to ordinary users without changing state.
    let entries = std::fs::read_dir("/sys/kernel/security/apparmor/policy/profiles")
        .map_err(|error| format!("cannot verify the Dever AppArmor profiles: {error}; an administrator must load /etc/apparmor.d/dever-sandbox"))?;
    let mut required = std::collections::BTreeSet::from(["dever-bwrap", "dever-bwrap-child"]);
    for entry in entries.take(4096) {
        let entry = entry.map_err(|error| format!("cannot inspect AppArmor profiles: {error}"))?;
        let name = std::fs::read_to_string(entry.path().join("name"))
            .map_err(|error| format!("cannot read AppArmor profile name: {error}"))?;
        if required.contains(name.trim()) {
            let mode = std::fs::read_to_string(entry.path().join("mode"))
                .map_err(|error| format!("cannot read AppArmor profile mode: {error}"))?;
            if mode.trim() != "enforce" {
                return Err("Dever AppArmor profiles must be loaded in enforce mode".into());
            }
            required.remove(name.trim());
        }
        if required.is_empty() {
            return Ok(());
        }
    }
    Err("Dever AppArmor profile is not loaded; an administrator must load /etc/apparmor.d/dever-sandbox".into())
}
