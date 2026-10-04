//! Explicit installation/update downloads. Project commands never enter here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::release::{
    InstallLock, Layout, MachineManager, ReleaseManifest, Version, decode_hex_file,
    ensure_real_directory, platform_identity, reject_symlink_ancestors,
    set_shared_file_permissions, sync_directory, sync_tree, validate_shared_file,
};

const OFFICIAL_RELEASES: &str = "https://github.com/shemic/dever-main/releases";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const METADATA_LIMIT: u64 = 2 * 1024 * 1024;
const RELEASE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
static NEXT_DOWNLOAD: AtomicU64 = AtomicU64::new(0);

struct ReleaseSource {
    base: String,
    agent: ureq::Agent,
}

impl ReleaseSource {
    fn official() -> Self {
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
                source.response(source.asset(version.as_str(), "tar.gz"), started)?;
            let compressed = response.body_mut().as_reader().take(RELEASE_LIMIT + 1);
            unpack(
                flate2::read::GzDecoder::new(compressed),
                &staging.0,
                &manifest,
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

fn validate_catalog(manifest: &ReleaseManifest) -> Result<(), String> {
    if manifest.artifacts.is_empty() || manifest.artifacts.len() > 4096 {
        return Err("release artifact count is outside 1..=4096".into());
    }
    let mut paths = BTreeSet::new();
    let mut size = 0_u64;
    for artifact in &manifest.artifacts {
        if artifact
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
            || artifact.path.contains(['\\', ':', '\0'])
            || matches!(artifact.path.as_str(), "manifest.json" | "manifest.sig")
            || !paths.insert(&artifact.path)
        {
            return Err("release has an unsafe or duplicate artifact path".into());
        }
        size = size
            .checked_add(artifact.bytes)
            .ok_or("release size overflow")?;
        if size > RELEASE_LIMIT {
            return Err("release exceeds its two-GiB unpacked size limit".into());
        }
    }
    Ok(())
}

fn unpack(reader: impl Read, root: &Path, manifest: &ReleaseManifest) -> Result<(), String> {
    validate_catalog(manifest)?;
    let expected: BTreeMap<_, _> = manifest
        .artifacts
        .iter()
        .map(|artifact| (artifact.path.as_str(), artifact))
        .collect();
    let mut seen = BTreeSet::new();
    // Raw entries reject PAX/GNU extensions before tar can buffer arbitrary metadata.
    let mut archive = tar::Archive::new(reader.take(RELEASE_LIMIT + 4096 * 1024));
    for entry in archive
        .entries()
        .map_err(|error| format!("invalid release archive: {error}"))?
        .raw(true)
    {
        let mut entry = entry.map_err(|error| format!("invalid release archive entry: {error}"))?;
        if !entry.header().entry_type().is_file() {
            return Err("release archives may contain only regular files".into());
        }
        let bytes = entry.path_bytes();
        let name = std::str::from_utf8(&bytes)
            .map_err(|_| "release archive path is not UTF-8")?
            .to_owned();
        let artifact = expected
            .get(name.as_str())
            .ok_or("release archive contains an unsigned file")?;
        if !seen.insert(name.clone()) || entry.size() != artifact.bytes {
            return Err("release archive contains a duplicate or wrong-sized file".into());
        }
        let path = root.join(&name);
        fs::create_dir_all(path.parent().expect("artifact has a parent"))
            .map_err(|error| format!("cannot create release artifact directory: {error}"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("cannot create release artifact: {error}"))?;
        let bytes = io::copy(&mut entry, &mut file)
            .map_err(|error| format!("cannot extract release artifact: {error}"))?;
        if bytes != artifact.bytes {
            return Err("release archive contains a truncated file".into());
        }
        set_shared_file_permissions(&path, false)?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    if seen.len() != expected.len() {
        return Err("release archive is missing signed artifacts".into());
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
