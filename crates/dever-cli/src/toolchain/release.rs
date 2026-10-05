use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
#[cfg(not(target_os = "linux"))]
use std::process::Command;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

static NEXT_TRANSACTION: AtomicU64 = AtomicU64::new(0);

const MANIFEST_NAME: &str = "manifest.json";
const SIGNATURE_NAME: &str = "manifest.sig";
const TRUSTED_KEY_NAME: &str = "trusted-release-key";

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Version(String);

impl Version {
    pub fn parse(value: &str) -> Result<Self, String> {
        dever_runtime::config::validate_dever_version(value)?;
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        version_components(self.as_str()).cmp(&version_components(other.as_str()))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for Version {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub format: String,
    pub version: Version,
    pub platform: String,
    pub artifacts: Vec<Artifact>,
    pub extensions: Vec<super::extensions::Extension>,
}

impl ReleaseManifest {
    pub fn new(version: Version, artifacts: Vec<Artifact>) -> Self {
        Self {
            format: "dever-release-v2".into(),
            version,
            platform: platform_identity(),
            artifacts,
            extensions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_launcher(executable: &Path) -> Result<Self, String> {
        let executable = fs::canonicalize(executable).map_err(|error| {
            format!(
                "cannot resolve launcher '{}': {error}",
                executable.display()
            )
        })?;
        let bin = executable
            .parent()
            .ok_or("dever launcher has no bin directory")?;
        if bin.file_name().and_then(|name| name.to_str()) != Some("bin") {
            return Err("dever launcher must be installed under <toolchain>/bin".into());
        }
        let root = bin.parent().ok_or("dever launcher has no toolchain root")?;
        Ok(Self::new(root))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    pub fn extensions(&self) -> PathBuf {
        self.cache().join("extensions")
    }
    pub fn downloads(&self) -> PathBuf {
        self.cache().join("downloads")
    }
    pub fn staging(&self) -> PathBuf {
        self.cache().join("staging")
    }
    pub fn native_cache(&self) -> PathBuf {
        self.cache().join("native")
    }
    /// The local control socket is kept inside the machine state directory for
    /// development fixtures. Platform installers may bind the documented
    /// system endpoint to this service, but callers never fall back to the
    /// cache directory itself.
    pub fn service_socket(&self) -> PathBuf {
        if self.root
            == Path::new(
                platform_contract(std::env::consts::OS)
                    .map(|contract| contract.machine_root)
                    .unwrap_or(""),
            )
        {
            return PathBuf::from(
                platform_contract(std::env::consts::OS)
                    .expect("current platform contract")
                    .ipc_endpoint,
            );
        }
        self.state().join("deverd.sock")
    }
    pub fn service_token(&self) -> PathBuf {
        self.state().join("deverd.token")
    }

    pub fn initialize(&self) -> Result<(), String> {
        reject_symlink_ancestors(&self.root)?;
        for directory in [
            self.bin(),
            self.state(),
            self.versions(),
            self.downloads(),
            self.staging(),
            self.native_cache(),
        ] {
            create_private_directory(&directory)?;
        }
        Ok(())
    }

    pub fn validate_machine_permissions(&self) -> Result<(), String> {
        for path in [self.root.clone(), self.bin(), self.state(), self.versions()] {
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!("cannot inspect machine path '{}': {error}", path.display())
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "machine path '{}' must be a real directory",
                    path.display()
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                    return Err(format!(
                        "machine path '{}' must be owned by root and not writable by group or other users",
                        path.display()
                    ));
                }
            }
        }
        let cache = fs::symlink_metadata(self.cache())
            .map_err(|error| format!("cannot inspect machine cache: {error}"))?;
        if cache.file_type().is_symlink() || !cache.is_dir() {
            return Err("machine cache must be a real directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if cache.mode() & 0o002 != 0 {
                return Err("machine cache must not be writable by other users".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformContract {
    pub machine_root: &'static str,
    pub public_entry: &'static str,
    pub service_definition: &'static str,
    pub ipc_endpoint: &'static str,
}

pub fn platform_contract(os: &str) -> Option<PlatformContract> {
    match os {
        "linux" => Some(PlatformContract {
            machine_root: "/opt/dever",
            public_entry: "/usr/local/bin/dever",
            service_definition: "/etc/systemd/system/deverd.service",
            ipc_endpoint: "/run/dever/deverd.sock",
        }),
        "macos" => Some(PlatformContract {
            machine_root: "/Library/Application Support/Dever",
            public_entry: "/usr/local/bin/dever",
            service_definition: "/Library/LaunchDaemons/dev.dever.deverd.plist",
            ipc_endpoint: "/var/run/dever/deverd.sock",
        }),
        "windows" => Some(PlatformContract {
            machine_root: r"C:\Program Files\Dever",
            public_entry: r"C:\Program Files\Dever\bin\dever.exe",
            service_definition: "DeverBuildService",
            ipc_endpoint: r"\\.\pipe\deverd",
        }),
        _ => None,
    }
}

pub fn platform_identity() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[derive(Clone, Debug)]
pub struct DownloadedReleases {
    root: PathBuf,
}

impl DownloadedReleases {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn package(&self, version: &Version) -> PathBuf {
        self.root.join(version.as_str())
    }

    pub fn latest(&self) -> Result<Version, String> {
        let path = self.root.join("latest");
        let value = String::from_utf8(read_regular_file(&path)?)
            .map_err(|_| format!("release catalog '{}' must be UTF-8", path.display()))?;
        Version::parse(value.trim())
            .map_err(|error| format!("invalid latest release in '{}': {error}", path.display()))
    }
}

pub struct MachineManager {
    layout: Layout,
    releases: DownloadedReleases,
}

impl MachineManager {
    pub fn new(layout: Layout) -> Self {
        let releases = DownloadedReleases::new(layout.downloads());
        Self { layout, releases }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn install_requested(&self, requested: &str) -> Result<Version, String> {
        let version = if requested == "latest" {
            self.releases.latest()?
        } else {
            Version::parse(requested)?
        };
        let _lock = InstallLock::acquire(&self.layout)?;
        #[cfg(target_os = "linux")]
        super::bootstrap::require_no_pending(&self.layout)?;
        self.cleanup_staging()?;
        self.install_locked(&version)?;
        if self.active_version()?.is_none() {
            self.activate_locked(&version)?;
        }
        Ok(version)
    }

    pub fn update(&self) -> Result<Version, String> {
        self.update_with(&super::release_source::ReleaseSource::official())
    }

    /// Uses the ordinary install transaction with an explicitly supplied author transport.
    pub fn update_with(&self, source: &dyn super::ExtensionSource) -> Result<Version, String> {
        let version = self.releases.latest()?;
        let _lock = InstallLock::acquire(&self.layout)?;
        #[cfg(target_os = "linux")]
        super::bootstrap::require_no_pending(&self.layout)?;
        if self
            .active_version()?
            .is_some_and(|active| version < active)
        {
            return Err(format!(
                "refusing to downgrade active Dever with 'dever update' to {version}; use 'dever use {version}' for an explicit installed-version switch"
            ));
        }
        self.cleanup_staging()?;
        self.install_locked(&version)?;
        if let Some(active) = self.active_version()? {
            let resources = super::extensions::SignedResources::load(&self.layout, &active)?;
            for extension in resources.installed_extensions()? {
                super::extensions::ensure_locked(&self.layout, &version, &extension, source)?;
            }
        }
        self.activate_locked(&version)?;
        Ok(version)
    }

    pub fn activate(&self, version: &Version) -> Result<(), String> {
        let _lock = InstallLock::acquire(&self.layout)?;
        #[cfg(target_os = "linux")]
        super::bootstrap::require_no_pending(&self.layout)?;
        self.activate_locked(version)
    }

    pub fn uninstall(&self, version: &Version) -> Result<(), String> {
        let _lock = InstallLock::acquire(&self.layout)?;
        #[cfg(target_os = "linux")]
        super::bootstrap::require_no_pending(&self.layout)?;
        if self.active_version()?.as_ref() == Some(version) {
            return Err(format!(
                "cannot uninstall active Dever {version}; run 'dever use <other-version>' first"
            ));
        }
        let installed = self.layout.versions().join(version.as_str());
        ensure_real_directory(&installed, "installed version")?;
        let staging = self.transaction_path("uninstall");
        fs::rename(&installed, &staging)
            .map_err(|error| format!("cannot stage Dever {version} for removal: {error}"))?;
        if let Err(error) = sync_directory(&self.layout.versions()) {
            let rollback = fs::rename(&staging, &installed)
                .and_then(|()| sync_directory(&self.layout.versions()));
            return match rollback {
                Ok(()) => Err(format!(
                    "cannot persist Dever {version} uninstallation: {error}"
                )),
                Err(rollback_error) => Err(format!(
                    "cannot persist Dever {version} uninstallation ({error}) or restore it ({rollback_error})"
                )),
            };
        }
        // The durable rename is the logical uninstall commit. Physical removal is
        // recoverable staging cleanup and must not report failure after that commit.
        let _ = fs::remove_dir_all(&staging);
        Ok(())
    }

    pub fn active_version(&self) -> Result<Option<Version>, String> {
        let path = self.layout.state().join("active-version");
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                validate_shared_file(&path, &self.layout.state(), "active Dever state", false)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot inspect '{}': {error}", path.display())),
        }
        let value = match fs::read_to_string(&path) {
            Ok(value) => value,
            Err(error) => return Err(format!("cannot read '{}': {error}", path.display())),
        };
        let mut lines = value.lines();
        if lines.next() != Some("dever-active-version-v1") {
            return Err(format!(
                "invalid active Dever state in '{}'",
                path.display()
            ));
        }
        let mut selected = None;
        for line in lines {
            let Some((version, checksum)) = line.split_once(' ') else {
                continue;
            };
            if checksum == active_checksum(version) {
                selected = Some(Version::parse(version).map_err(|error| {
                    format!(
                        "invalid active Dever version in '{}': {error}",
                        path.display()
                    )
                })?);
            }
        }
        Ok(selected)
    }

    pub fn installed_versions(&self) -> Result<Vec<Version>, String> {
        let mut versions = Vec::new();
        for entry in fs::read_dir(self.layout.versions())
            .map_err(|error| format!("cannot list installed Dever versions: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("cannot list installed Dever versions: {error}"))?;
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "installed Dever version name is not UTF-8".to_owned())?;
                versions.push(
                    Version::parse(&name).map_err(|_| {
                        format!("invalid directory in machine version store: {name}")
                    })?,
                );
            }
        }
        versions.sort();
        Ok(versions)
    }

    pub fn resolve_core(&self, version: &Version) -> Result<PathBuf, String> {
        let root = self.layout.versions().join(version.as_str());
        if !root.exists() {
            return Err(format!(
                "Dever {version} is not installed; run 'dever install {version}'"
            ));
        }
        let manifest = self.verify_release(&root, Some(version))?;
        validate_installed_release(&self.layout, &root, &manifest)?;
        Ok(root.join(core_path(&manifest)?))
    }

    pub(super) fn resolve_skill(&self, version: &Version) -> Result<PathBuf, String> {
        let root = self.layout.versions().join(version.as_str());
        let manifest = self.verify_release(&root, Some(version))?;
        validate_installed_release(&self.layout, &root, &manifest)?;
        if !manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "skills/dever-language/SKILL.md")
        {
            return Err(format!(
                "Dever {version} has no signed dever-language skill; install a complete release"
            ));
        }
        Ok(root.join("skills/dever-language"))
    }

    /// The lease prevents installation/removal while a trusted worker uses its
    /// compiler and link inputs. Callers cannot select a compiler filesystem path.
    pub(crate) fn compilation(&self, version: &Version) -> Result<InstalledCompiler, String> {
        let lease = InstallLock::shared(&self.layout)?;
        let root = self.layout.versions().join(version.as_str());
        let manifest = self.verify_release(&root, Some(version))?;
        validate_installed_release(&self.layout, &root, &manifest)?;
        let core = fs::canonicalize(root.join(core_path(&manifest)?))
            .map_err(|error| format!("cannot resolve installed compiler: {error}"))?;
        Ok(InstalledCompiler {
            core,
            manifest,
            resources: super::SignedResources::load(&self.layout, version)?,
            identity: sha256_file(&root.join(MANIFEST_NAME))?,
            _lease: lease,
        })
    }

    fn install_locked(&self, version: &Version) -> Result<(), String> {
        self.install_package_locked(&self.releases.package(version), version)
    }

    pub(super) fn install_package_locked(
        &self,
        package: &Path,
        version: &Version,
    ) -> Result<(), String> {
        let destination = self.layout.versions().join(version.as_str());
        if destination.exists() {
            let manifest = self.verify_release(&destination, Some(version))?;
            validate_installed_release(&self.layout, &destination, &manifest)?;
            if package.exists() {
                self.verify_release(package, Some(version))?;
                if read_regular_file(&destination.join(MANIFEST_NAME))?
                    != read_regular_file(&package.join(MANIFEST_NAME))?
                {
                    return Err("published Dever version changed its signed manifest".into());
                }
            }
            return Ok(());
        }
        let manifest = self.verify_release(package, Some(version))?;
        let core_relative = core_path(&manifest)?.to_path_buf();
        let staging = self.transaction_path("install");
        create_private_directory(&staging)?;
        let result = (|| {
            copy_file(
                &package.join(MANIFEST_NAME),
                &staging.join(MANIFEST_NAME),
                false,
            )?;
            copy_file(
                &package.join(SIGNATURE_NAME),
                &staging.join(SIGNATURE_NAME),
                false,
            )?;
            for artifact in &manifest.artifacts {
                let relative = safe_relative(&artifact.path)?;
                copy_file(
                    &artifact_path(package, relative)?,
                    &staging.join(relative),
                    relative == core_relative || bootstrap_executable(relative),
                )?;
            }
            self.verify_release(&staging, Some(version))?;
            validate_installed_release(&self.layout, &staging, &manifest)?;
            let core = staging.join(core_path(&manifest)?);
            #[cfg(target_os = "linux")]
            super::bootstrap::health_check(&core, "--dever-health")?;
            #[cfg(not(target_os = "linux"))]
            {
                let status = Command::new(&core)
                    .arg("--dever-health")
                    .env_clear()
                    .status()
                    .map_err(|error| format!("cannot health-check Dever {version}: {error}"))?;
                if !status.success() {
                    return Err(format!(
                        "Dever {version} failed its installation health check"
                    ));
                }
            }
            sync_tree(&staging)?;
            fs::rename(&staging, &destination)
                .map_err(|error| format!("cannot publish Dever {version}: {error}"))?;
            sync_directory(&self.layout.versions())
                .map_err(|error| format!("cannot persist Dever {version} installation: {error}"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    fn activate_locked(&self, version: &Version) -> Result<(), String> {
        self.resolve_core(version)?;
        append_active_version(&self.layout.state().join("active-version"), version)
            .map_err(|error| format!("cannot activate Dever {version}: {error}"))
    }

    pub(super) fn verify_release(
        &self,
        root: &Path,
        expected: Option<&Version>,
    ) -> Result<ReleaseManifest, String> {
        let trusted_key_path = self.layout.state().join(TRUSTED_KEY_NAME);
        validate_shared_file(
            &trusted_key_path,
            &self.layout.state(),
            "trusted release key",
            false,
        )?;
        let trusted_key = decode_hex_file(&trusted_key_path, 32)?;
        Self::verify_package(root, expected, &trusted_key)
    }

    pub(super) fn verify_package(
        root: &Path,
        expected: Option<&Version>,
        trusted_key: &[u8],
    ) -> Result<ReleaseManifest, String> {
        let manifest = Self::signed_manifest(root, trusted_key)?;
        if expected.is_some_and(|expected| expected != &manifest.version) {
            return Err(format!(
                "release version does not match requested {expected:?}"
            ));
        }
        Self::verify_artifacts(root, manifest)
    }

    pub(super) fn signed_manifest(
        root: &Path,
        trusted_key: &[u8],
    ) -> Result<ReleaseManifest, String> {
        ensure_real_directory(root, "release")?;
        let manifest_path = root.join(MANIFEST_NAME);
        let manifest_bytes = read_regular_file(&manifest_path)?;
        let signature = decode_hex_file(&root.join(SIGNATURE_NAME), 64)?;
        UnparsedPublicKey::new(&ED25519, trusted_key)
            .verify(&manifest_bytes, &signature)
            .map_err(|_| {
                format!(
                    "release signature verification failed for '{}'",
                    root.display()
                )
            })?;
        let manifest: ReleaseManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|error| {
                format!(
                    "invalid release manifest '{}': {error}",
                    manifest_path.display()
                )
            })?;
        if manifest.format != "dever-release-v2" {
            return Err("unsupported Dever release manifest format".into());
        }
        if manifest.platform != platform_identity() {
            return Err(format!(
                "release platform '{}' does not match '{}'",
                manifest.platform,
                platform_identity()
            ));
        }
        super::extensions::validate_catalog(&manifest)?;
        Ok(manifest)
    }

    fn verify_artifacts(root: &Path, manifest: ReleaseManifest) -> Result<ReleaseManifest, String> {
        let mut paths = BTreeSet::new();
        for artifact in &manifest.artifacts {
            let relative = safe_relative(&artifact.path)?;
            if !paths.insert(relative.to_path_buf()) {
                return Err(format!("duplicate release artifact '{}'", artifact.path));
            }
            if artifact.sha256.len() != 64
                || !artifact
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(format!(
                    "invalid SHA-256 for release artifact '{}'",
                    artifact.path
                ));
            }
            let path = artifact_path(root, relative)?;
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!(
                    "cannot inspect release artifact '{}': {error}",
                    path.display()
                )
            })?;
            if !metadata.file_type().is_file() {
                return Err(format!(
                    "release artifact '{}' is not a regular file",
                    path.display()
                ));
            }
            if metadata.len() != artifact.bytes || sha256_file(&path)? != artifact.sha256 {
                return Err(format!(
                    "release artifact '{}' failed integrity verification",
                    artifact.path
                ));
            }
        }
        core_path(&manifest)?;
        let native_manifest = format!("runtime/{}/manifest.json", platform_identity());
        if manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.path == native_manifest)
        {
            super::core_libraries::validate(root, &manifest.artifacts)?;
        }
        #[cfg(target_os = "linux")]
        if manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.path.starts_with("bootstrap/"))
        {
            // Older installed versions retain their own signed service template.
            // Only the explicit bootstrap installer applies the current template.
            super::bootstrap::validate_payload(root, &manifest.artifacts, false)?;
        }
        Ok(manifest)
    }

    fn cleanup_staging(&self) -> Result<(), String> {
        for entry in fs::read_dir(self.layout.staging())
            .map_err(|error| format!("cannot inspect toolchain staging: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("cannot inspect toolchain staging: {error}"))?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("cannot inspect toolchain staging entry: {error}"))?;
            if metadata.file_type().is_symlink() {
                fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
            } else if metadata.is_dir() {
                fs::remove_dir_all(entry.path()).map_err(|error| error.to_string())?;
            } else {
                fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    fn transaction_path(&self, operation: &str) -> PathBuf {
        self.layout.staging().join(format!(
            "{operation}-{}-{}",
            std::process::id(),
            NEXT_TRANSACTION.fetch_add(1, Ordering::Relaxed),
        ))
    }
}

pub(crate) struct InstalledCompiler {
    pub(crate) core: PathBuf,
    pub(crate) manifest: ReleaseManifest,
    pub(crate) identity: String,
    pub(crate) resources: super::SignedResources,
    _lease: InstallLock,
}

pub(super) struct InstallLock(File);

impl InstallLock {
    pub(super) fn acquire(layout: &Layout) -> Result<Self, String> {
        Self::open(layout, false, false)
    }

    pub(super) fn acquire_wait(layout: &Layout) -> Result<Self, String> {
        Self::open(layout, false, true)
    }

    fn shared(layout: &Layout) -> Result<Self, String> {
        Self::open(layout, true, false)
    }

    fn open(layout: &Layout, shared: bool, wait: bool) -> Result<Self, String> {
        let path = layout.state().join("install.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                validate_install_lock(layout, &path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect installation lock: {error}")),
        }
        let file = options
            .open(&path)
            .map_err(|error| format!("cannot open Dever installation lock: {error}"))?;
        let current = validate_install_lock(layout, &path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let opened = file.metadata().map_err(|error| error.to_string())?;
            if current.dev() != opened.dev() || current.ino() != opened.ino() {
                return Err("installation lock changed while opening".into());
            }
        }
        #[cfg(not(unix))]
        let _ = current;
        let started = std::time::Instant::now();
        loop {
            let locked = if shared {
                fs2::FileExt::try_lock_shared(&file)
            } else {
                fs2::FileExt::try_lock_exclusive(&file)
            };
            match locked {
                Ok(()) => break,
                Err(error)
                    if wait
                        && error.kind() == io::ErrorKind::WouldBlock
                        && started.elapsed() < std::time::Duration::from_secs(300) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                Err(error) => {
                    return Err(format!(
                        "another Dever installation operation or compilation is active: {error}"
                    ));
                }
            }
        }
        Ok(Self(file))
    }
}

fn validate_install_lock(layout: &Layout, path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect installation lock: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("installation lock must be a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let state = fs::symlink_metadata(layout.state()).map_err(|error| error.to_string())?;
        if metadata.uid() != state.uid() || metadata.mode() & 0o077 != 0 {
            return Err("installation lock must be private to the service owner".into());
        }
    }
    #[cfg(not(unix))]
    let _ = layout;
    Ok(metadata)
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        // Keep the inode: unlinking a locked file permits a second independent lock.
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn core_path(manifest: &ReleaseManifest) -> Result<&Path, String> {
    let expected = format!("dever-core{}", std::env::consts::EXE_SUFFIX);
    let mut matching = manifest
        .artifacts
        .iter()
        .filter(|artifact| artifact.path == expected);
    let core = matching
        .next()
        .ok_or("release manifest is missing dever-core")?;
    if matching.next().is_some() {
        return Err("release manifest contains multiple dever-core artifacts".into());
    }
    Ok(Path::new(&core.path))
}

fn bootstrap_executable(path: &Path) -> bool {
    matches!(path.to_str(), Some("bootstrap/dever" | "bootstrap/deverd"))
}

fn version_components(version: &str) -> [u64; 3] {
    let mut parts = version.split('.').map(|part| {
        part.parse::<u64>()
            .expect("validated Dever version component")
    });
    [
        parts.next().expect("validated Dever major version"),
        parts.next().expect("validated Dever minor version"),
        parts.next().expect("validated Dever patch version"),
    ]
}

fn safe_relative(path: &str) -> Result<&Path, String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "release artifact path '{path:?}' must be a normalized relative path"
        ));
    }
    Ok(path)
}

fn artifact_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    let count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        let Component::Normal(component) = component else {
            return Err("release artifact path must be normalized".into());
        };
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "cannot inspect release artifact '{}': {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink()
            || (index + 1 < count && !metadata.is_dir())
            || (index + 1 == count && !metadata.file_type().is_file())
        {
            return Err(format!(
                "release artifact '{}' crosses a symlink or invalid path",
                path.display()
            ));
        }
    }
    Ok(path)
}

pub(super) fn reject_symlink_ancestors(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "machine path '{}' must not be a symlink",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(format!("cannot inspect '{}': {error}", current.display())),
        }
    }
    Ok(())
}

pub(super) fn ensure_real_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label} '{}': {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "{label} '{}' must be a real directory",
            path.display()
        ));
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o755);
    }
    builder
        .create(path)
        .map_err(|error| format!("cannot create '{}': {error}", path.display()))?;
    ensure_real_directory(path, "machine directory")
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("'{}' must be a regular file", path.display()));
    }
    const LIMIT: u64 = 2 * 1024 * 1024;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| format!("cannot read '{}': {error}", path.display()))?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("release metadata exceeds its size limit".into());
    }
    Ok(bytes)
}

pub(super) fn copy_file(source: &Path, destination: &Path, executable: bool) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        create_private_directory(parent)?;
    }
    let mut source_file = File::open(source)
        .map_err(|error| format!("cannot read '{}': {error}", source.display()))?;
    if !source_file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(format!("'{}' must be a regular file", source.display()));
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut destination_file = options
        .open(destination)
        .map_err(|error| format!("cannot create '{}': {error}", destination.display()))?;
    io::copy(&mut source_file, &mut destination_file)
        .and_then(|_| destination_file.sync_all())
        .map_err(|error| format!("cannot copy '{}': {error}", source.display()))?;
    set_shared_file_permissions(destination, executable)?;
    destination_file.sync_all().map_err(|error| {
        format!(
            "cannot persist final file mode '{}': {error}",
            destination.display()
        )
    })
}

/// Persist every child directory after its leaves, before publishing a tree.
pub(super) fn sync_tree(root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            sync_tree(&entry.path())?;
        } else if !kind.is_file() {
            return Err("installation tree contains a non-regular entry".into());
        }
    }
    sync_directory(root).map_err(|error| error.to_string())
}

pub(super) fn validate_installed_release(
    layout: &Layout,
    root: &Path,
    manifest: &ReleaseManifest,
) -> Result<(), String> {
    validate_shared_directory(root, &layout.versions(), "installed release")?;
    validate_shared_file(&root.join(MANIFEST_NAME), root, "release manifest", false)?;
    validate_shared_file(&root.join(SIGNATURE_NAME), root, "release signature", false)?;
    let core = core_path(manifest)?;
    for artifact in &manifest.artifacts {
        let relative = safe_relative(&artifact.path)?;
        let mut parent = root.to_path_buf();
        let components: Vec<_> = relative.components().collect();
        for component in &components[..components.len().saturating_sub(1)] {
            let Component::Normal(component) = component else {
                return Err("release artifact path must be normalized".into());
            };
            let directory = parent.join(component);
            validate_shared_directory(&directory, &parent, "release artifact directory")?;
            parent = directory;
        }
        validate_shared_file(
            &root.join(relative),
            &parent,
            "installed release artifact",
            relative == core || bootstrap_executable(relative),
        )?;
    }
    Ok(())
}

pub(super) fn validate_shared_directory(
    path: &Path,
    parent: &Path,
    label: &str,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label} '{}': {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "{label} '{}' must be a real directory",
            path.display()
        ));
    }
    validate_shared_permissions(path, parent, &metadata, label, true, false)
}

pub(super) fn validate_shared_file(
    path: &Path,
    parent: &Path,
    label: &str,
    executable: bool,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label} '{}': {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "{label} '{}' must be a regular file",
            path.display()
        ));
    }
    validate_shared_permissions(path, parent, &metadata, label, false, executable)
}

fn validate_shared_permissions(
    path: &Path,
    parent: &Path,
    metadata: &fs::Metadata,
    label: &str,
    directory: bool,
    executable: bool,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
            format!(
                "cannot inspect {label} parent '{}': {error}",
                parent.display()
            )
        })?;
        let required = if directory || executable {
            0o005
        } else {
            0o004
        };
        if metadata.uid() != parent_metadata.uid()
            || metadata.mode() & 0o022 != 0
            || metadata.mode() & required != required
        {
            return Err(format!(
                "{label} '{}' must be owned by the machine store owner, shared read-only, and not writable by group or other users",
                path.display()
            ));
        }
    }
    #[cfg(windows)]
    {
        let _ = (path, parent, metadata, label, directory, executable);
    }
    Ok(())
}

pub(super) fn set_shared_file_permissions(path: &Path, executable: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?;
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("cannot protect '{}': {error}", path.display()))?;
        let protected = fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?;
        if protected.uid() != metadata.uid() || protected.mode() & 0o777 != mode {
            return Err(format!(
                "cannot protect '{}': permissions did not persist",
                path.display()
            ));
        }
    }
    #[cfg(windows)]
    {
        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?
            .permissions();
        let _ = executable;
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("cannot protect '{}': {error}", path.display()))?;
    }
    Ok(())
}

pub(super) fn decode_hex_file(path: &Path, expected_bytes: usize) -> Result<Vec<u8>, String> {
    let text = String::from_utf8(read_regular_file(path)?)
        .map_err(|_| format!("'{}' must contain UTF-8 hexadecimal data", path.display()))?;
    let text = text.trim();
    if text.len() != expected_bytes * 2 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "'{}' must contain {} hexadecimal bytes",
            path.display(),
            expected_bytes
        ));
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digits = std::str::from_utf8(pair).expect("hexadecimal bytes are UTF-8");
            u8::from_str_radix(digits, 16)
                .map_err(|_| format!("invalid hexadecimal data in '{}'", path.display()))
        })
        .collect()
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
    // Runtime packs are checked on every compilation, including cache hits.
    // Reuse the existing cryptographic backend's optimized streaming SHA-256.
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let length = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash '{}': {error}", path.display()))?;
        if length == 0 {
            break;
        }
        digest.update(&buffer[..length]);
    }
    Ok(hex(digest.finish().as_ref()))
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

pub(super) fn append_active_version(path: &Path, version: &Version) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("active state has no parent"))?;
    match fs::symlink_metadata(path) {
        Ok(_) => validate_shared_file(path, parent, "active Dever state", false)
            .map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    if file.metadata()?.len() == 0 {
        file.write_all(b"dever-active-version-v1\n")?;
    }
    // A leading newline keeps the next record recoverable even if a previous
    // append stopped halfway. Readers use only complete checksum-valid records.
    writeln!(file, "\n{} {}", version, active_checksum(version.as_str()))?;
    file.sync_all()?;
    set_shared_file_permissions(path, false).map_err(io::Error::other)?;
    sync_directory(parent)
}

fn active_checksum(version: &str) -> String {
    hex(&Sha256::digest(
        [b"dever-active-version-v1\0".as_slice(), version.as_bytes()].concat(),
    ))
}

pub(super) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}
