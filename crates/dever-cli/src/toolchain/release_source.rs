//! Explicit installation/update downloads. Project commands never enter here.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::extensions::Extension;
#[cfg(test)]
use super::extensions::PAYLOAD_LIMIT as RELEASE_LIMIT;
#[cfg(test)]
use super::release::ReleaseManifest;
use super::release::{
    InstallLock, Layout, MachineManager, Version, decode_hex_file, ensure_real_directory,
    platform_identity, reject_symlink_ancestors, set_shared_file_permissions, sync_directory,
    sync_tree, validate_shared_file,
};
use super::validate_catalog;

const OFFICIAL_RELEASES: &str = "https://github.com/shemic/dever-main/releases";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const METADATA_LIMIT: u64 = 2 * 1024 * 1024;
static NEXT_DOWNLOAD: AtomicU64 = AtomicU64::new(0);

pub(super) struct ReleaseSource {
    base: String,
    agent: ureq::Agent,
}

impl ReleaseSource {
    pub(super) fn official() -> Self {
        Self::new(OFFICIAL_RELEASES.to_owned())
    }

    fn new(base: String) -> Self {
        Self {
            base,
            agent: ureq::Agent::new_with_config(
                ureq::Agent::config_builder()
                    .proxy(None)
                    .max_redirects(0)
                    .timeout_global(Some(DOWNLOAD_TIMEOUT))
                    .build(),
            ),
        }
    }

    fn asset(&self, requested: &str, suffix: &str) -> String {
        let selection = if requested == "latest" {
            "latest/download".to_owned()
        } else {
            format!("download/v{requested}")
        };
        format!(
            "{}/{selection}/dever-{}.{suffix}",
            self.base,
            platform_identity()
        )
    }

    fn response(
        &self,
        mut address: String,
        started: Instant,
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        for _ in 0..=5 {
            let remaining = DOWNLOAD_TIMEOUT
                .checked_sub(started.elapsed())
                .filter(|duration| !duration.is_zero())
                .ok_or("Dever release download exceeded its five-minute deadline")?;
            let response = self
                .agent
                .get(&address)
                .config()
                .timeout_global(Some(remaining))
                .build()
                .call()
                .map_err(|error| match error {
                    ureq::Error::StatusCode(404) => {
                        "the official Dever release is not published for this version/platform; no installed version was changed".to_owned()
                    }
                    ureq::Error::StatusCode(code) => {
                        format!("official Dever release download returned HTTP {code}")
                    }
                    _ => "cannot download the official Dever release".to_owned(),
                })?;
            if response.status().is_success() {
                return Ok(response);
            }
            if !response.status().is_redirection() {
                return Err("unexpected official release HTTP status".into());
            }
            let mut locations = response.headers().get_all("location").iter();
            let location = locations
                .next()
                .and_then(|value| value.to_str().ok())
                .ok_or("release redirect is missing a valid Location")?;
            if locations.next().is_some() {
                return Err("release redirect has multiple Locations".into());
            }
            address = redirect(&address, location)?;
        }
        Err("official release download exceeded its redirect limit".into())
    }

    fn metadata(
        &self,
        requested: &str,
        suffix: &str,
        path: &Path,
        started: Instant,
    ) -> Result<(), String> {
        let mut response = self.response(self.asset(requested, suffix), started)?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(METADATA_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read official release metadata")?;
        if bytes.len() as u64 > METADATA_LIMIT {
            return Err("official release metadata exceeds its size limit".into());
        }
        write_new(path, &bytes)
    }
}

impl super::ExtensionSource for ReleaseSource {
    fn open(
        &self,
        version: &Version,
        extension: &super::ExtensionId,
    ) -> Result<Box<dyn Read>, String> {
        Ok(Box::new(
            self.response(
                self.asset(version.as_str(), &extension.asset_suffix()),
                Instant::now(),
            )?
            .into_body()
            .into_reader(),
        ))
    }
}

fn redirect(current: &str, location: &str) -> Result<String, String> {
    let next = url::Url::parse(current)
        .and_then(|base| base.join(location))
        .map_err(|_| "invalid release redirect")?;
    if next.scheme() != "https"
        || !next.username().is_empty()
        || next.password().is_some()
        || next.port_or_known_default() != Some(443)
        || next.fragment().is_some()
        || !matches!(
            next.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
    {
        return Err("release redirect must remain on the official HTTPS asset hosts".into());
    }
    Ok(next.into())
}

pub(super) fn prepare(layout: &Layout, requested: &str) -> Result<Version, String> {
    prepare_from(layout, requested, &ReleaseSource::official())
}

fn prepare_from(
    layout: &Layout,
    requested: &str,
    source: &ReleaseSource,
) -> Result<Version, String> {
    if requested != "latest" {
        Version::parse(requested)?;
    }
    layout.validate_machine_permissions()?;
    for directory in [layout.downloads(), layout.staging()] {
        reject_symlink_ancestors(&directory)?;
        ensure_real_directory(&directory, "release download directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(&directory).map_err(|error| error.to_string())?;
            if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                return Err(
                    "release download directories must be root-owned and not publicly writable"
                        .into(),
                );
            }
        }
    }
    let _lock = InstallLock::acquire(layout)?;
    #[cfg(target_os = "linux")]
    super::bootstrap::require_no_pending(layout)?;
    let key_path = layout.state().join("trusted-release-key");
    validate_shared_file(&key_path, &layout.state(), "trusted release key", false)?;
    let key = decode_hex_file(&key_path, 32)?;
    let staging = Download::new(&layout.staging())?;
    let started = Instant::now();
    source.metadata(
        requested,
        "manifest.json",
        &staging.0.join("manifest.json"),
        started,
    )?;
    source.metadata(
        requested,
        "manifest.sig",
        &staging.0.join("manifest.sig"),
        started,
    )?;
    let manifest = MachineManager::signed_manifest(&staging.0, &key)?;
    if !manifest
        .artifacts
        .iter()
        .any(|artifact| artifact.path == "skills/dever-language/SKILL.md")
    {
        return Err("official releases must include the matching dever-language skill".into());
    }
    if requested != "latest" && manifest.version.as_str() != requested {
        return Err("downloaded release version does not match the requested version".into());
    }
    validate_catalog(&manifest)?;
    let version = manifest.version.clone();
    let destination = layout.downloads().join(version.as_str());
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            MachineManager::verify_package(&destination, Some(&version), &key)?;
            if fs::read(destination.join("manifest.json")).map_err(|error| error.to_string())?
                != fs::read(staging.0.join("manifest.json")).map_err(|error| error.to_string())?
            {
                return Err("published Dever version changed its signed manifest".into());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut response =
                source.response(source.asset(version.as_str(), "tar.zst"), started)?;
            super::release_archive::extract(
                response.body_mut().as_reader(),
                &staging.0,
                &manifest.artifacts,
            )?;
            MachineManager::verify_package(&staging.0, Some(&version), &key)?;
            sync_tree(&staging.0)?;
            publish(&staging.0, &destination)?;
            sync_directory(&layout.downloads()).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(format!("cannot inspect downloaded release: {error}")),
    }
    // Latest is only a catalog hint; the signed version and active journal own trust.
    if requested == "latest" {
        let latest = layout.downloads().join("latest");
        match fs::symlink_metadata(&latest) {
            Ok(_) => validate_shared_file(&latest, &layout.downloads(), "release catalog", false)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect release catalog: {error}")),
        }
        let temporary = Download::new(&layout.staging())?;
        let catalog = temporary.0.join("latest");
        write_new(&catalog, format!("{version}\n").as_bytes())?;
        fs::rename(catalog, latest)
            .map_err(|error| format!("cannot publish release catalog: {error}"))?;
        sync_directory(&layout.downloads()).map_err(|error| error.to_string())?;
    }
    Ok(version)
}

#[cfg(test)]
fn unpack(reader: impl Read, root: &Path, manifest: &ReleaseManifest) -> Result<(), String> {
    validate_catalog(manifest)?;
    super::release_archive::unpack(reader, root, &manifest.artifacts)
}

pub(super) fn prepare_extension(
    layout: &Layout,
    version: &Version,
    extension: &Extension,
    destination: &Path,
    source: &dyn super::ExtensionSource,
) -> Result<(), String> {
    let staging = Download::new(&layout.staging())?;
    super::release_archive::extract(
        source.open(version, &extension.id())?,
        &staging.0,
        &extension.artifacts,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging.0, fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
    }
    super::extensions::verify_extension(&staging.0, &extension.artifacts)?;
    if matches!(extension.kind, super::ExtensionKind::Target) {
        super::runtime_pack::validate(&staging.0, version.as_str(), extension.target)?;
    }
    sync_tree(&staging.0)?;
    publish(&staging.0, destination)?;
    sync_directory(destination.parent().ok_or("extension has no parent")?)
        .map_err(|error| error.to_string())
}

/// Private, independently authenticated installer entry; never accepted over IPC.
pub fn extract_config(config: &Path) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Extraction {
        release: PathBuf,
        archive: PathBuf,
        trusted_key: PathBuf,
    }
    trusted_input(config, false)?;
    let mut bytes = Vec::new();
    File::open(config)
        .map_err(|error| error.to_string())?
        .take(METADATA_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > METADATA_LIMIT {
        return Err("extraction config exceeds its size limit".into());
    }
    let settings: Extraction = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid extraction config: {error}"))?;
    trusted_input(&settings.release, true)?;
    trusted_input(&settings.archive, false)?;
    trusted_input(&settings.trusted_key, false)?;
    if settings.trusted_key.starts_with(&settings.release) {
        return Err("trusted key must be supplied independently outside the release".into());
    }
    let key = decode_hex_file(&settings.trusted_key, 32)?;
    let manifest = MachineManager::signed_manifest(&settings.release, &key)?;
    let archive = File::open(&settings.archive).map_err(|error| error.to_string())?;
    super::release_archive::extract(archive, &settings.release, &manifest.artifacts)?;
    MachineManager::verify_package(&settings.release, Some(&manifest.version), &key)?;
    sync_tree(&settings.release)
}

fn trusted_input(path: &Path, directory: bool) -> Result<(), String> {
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err("extraction paths must be absolute and normalized".into());
    }
    reject_symlink_ancestors(path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect extraction input: {error}"))?;
    if directory && !metadata.is_dir() || !directory && !metadata.is_file() {
        return Err("extraction input has the wrong file type".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
            || !directory && metadata.nlink() != 1
        {
            return Err(
                "extraction inputs must be root-owned and not writable by other users".into(),
            );
        }
        for ancestor in path.ancestors().skip(1) {
            let metadata = fs::symlink_metadata(ancestor).map_err(|error| error.to_string())?;
            if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0
            {
                return Err("extraction input ancestor is not protected by root".into());
            }
        }
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create release metadata: {error}"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    set_shared_file_permissions(path, false)
}

fn publish(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE)
            .map_err(|error| format!("cannot publish downloaded release: {error}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, destination);
        Err("official release downloads currently require Linux".into())
    }
}

struct Download(PathBuf);

impl Download {
    fn new(parent: &Path) -> Result<Self, String> {
        let path = parent.join(format!(
            "download-{}-{}",
            std::process::id(),
            NEXT_DOWNLOAD.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|error| format!("cannot create release download staging: {error}"))?;
        Ok(Self(path))
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
#[path = "../../../../test/dever-cli-tests/release_source.rs"]
mod tests;
