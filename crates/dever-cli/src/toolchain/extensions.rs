//! One signed catalog owns both the base release and immutable optional resources.
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::BuildTarget;
use super::release::{
    self, Artifact, InstallLock, Layout, MachineManager, ReleaseManifest, Version,
};
use crate::libs::Ecosystem;

/// Explicit author/test transport. Product IPC always uses the compiled official source.
pub trait ExtensionSource {
    fn open(&self, version: &Version, extension: &ExtensionId)
    -> Result<Box<dyn io::Read>, String>;
}

pub(super) const PAYLOAD_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
pub(super) const ARTIFACT_LIMIT: usize = 4096;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "ecosystem",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExtensionKind {
    Runtime(Ecosystem),
    Build(Ecosystem),
    Target,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionId {
    pub kind: ExtensionKind,
    pub target: BuildTarget,
}

impl ExtensionId {
    pub fn name(&self) -> String {
        let kind = match &self.kind {
            ExtensionKind::Runtime(ecosystem) => format!("runtime-{}", ecosystem.as_str()),
            ExtensionKind::Build(ecosystem) => format!("build-{}", ecosystem.as_str()),
            ExtensionKind::Target => "target".into(),
        };
        format!("{kind}-{}", self.target.platform())
    }

    pub fn asset_suffix(&self) -> String {
        format!("ext-{}.tar.zst", self.name())
    }

    fn permits(&self, path: &str) -> bool {
        let target = self.target.platform();
        match &self.kind {
            ExtensionKind::Runtime(ecosystem) => {
                path.starts_with(&format!("runtime/{}/{target}/", ecosystem.as_str()))
            }
            ExtensionKind::Build(ecosystem) => {
                path.starts_with(&format!("build/{}/{target}/", ecosystem.as_str()))
            }
            ExtensionKind::Target => {
                path.starts_with(&format!("runtime/{target}/"))
                    || path.starts_with(&format!("sandbox/{target}/"))
            }
        }
    }

    fn missing(&self) -> String {
        match self.kind {
            ExtensionKind::Target => format!(
                "target resources are not installed; run 'dever target add {}'",
                self.target.platform()
            ),
            _ => format!(
                "{} resources are not installed; run 'dever lib install <project-root> --target {}' (or 'dever lib add' for new dependencies)",
                self.name(),
                self.target.platform()
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    pub kind: ExtensionKind,
    pub target: BuildTarget,
    pub artifacts: Vec<Artifact>,
}

impl Extension {
    pub fn id(&self) -> ExtensionId {
        ExtensionId {
            kind: self.kind.clone(),
            target: self.target,
        }
    }
}

pub fn validate_catalog(manifest: &ReleaseManifest) -> Result<(), String> {
    Version::parse(manifest.version.as_str())?;
    if manifest.format != "dever-release-v2" || manifest.platform != release::platform_identity() {
        return Err("unsupported release format or host platform".into());
    }
    if manifest.extensions.len() > 12 {
        return Err("release extension count exceeds its limit".into());
    }
    let mut paths = BTreeSet::new();
    validate_artifacts(&manifest.artifacts, &mut paths)?;
    let mut ids = BTreeSet::new();
    for extension in &manifest.extensions {
        let id = extension.id();
        if !ids.insert(id.clone())
            || matches!(id.kind, ExtensionKind::Build(Ecosystem::Go))
            || matches!(id.kind, ExtensionKind::Target) && id.target == BuildTarget::host()?
        {
            return Err("release has a duplicate or unsupported extension identity".into());
        }
        validate_artifacts(&extension.artifacts, &mut paths)?;
        if extension
            .artifacts
            .iter()
            .any(|artifact| !id.permits(&artifact.path))
        {
            return Err(format!(
                "extension '{}' contains a file outside its resource namespace",
                id.name()
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_artifacts<'a>(
    artifacts: &'a [Artifact],
    paths: &mut BTreeSet<&'a str>,
) -> Result<(), String> {
    if artifacts.is_empty() || artifacts.len() > ARTIFACT_LIMIT {
        return Err("release artifact count is outside 1..=4096".into());
    }
    let mut bytes = 0_u64;
    for artifact in artifacts {
        if artifact.path.is_empty()
            || artifact.path.len() > 1024
            || artifact
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || artifact.path.contains(['\\', ':', '\0'])
            || matches!(artifact.path.as_str(), "manifest.json" | "manifest.sig")
            || !paths.insert(&artifact.path)
        {
            return Err("release has an unsafe or duplicate artifact path".into());
        }
        if artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("release artifact has an invalid SHA-256".into());
        }
        bytes = bytes
            .checked_add(artifact.bytes)
            .ok_or("release size overflow")?;
        if bytes > PAYLOAD_LIMIT {
            return Err("release exceeds its two-GiB unpacked size limit".into());
        }
    }
    // A signed file cannot also be the parent directory of another file.
    for path in paths.iter() {
        for (index, _) in path.match_indices('/') {
            if paths.contains(&path[..index]) {
                return Err("release artifact paths overlap as file and directory".into());
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct SignedResources {
    layout: Layout,
    base: PathBuf,
    manifest: ReleaseManifest,
    catalog: String,
}

impl SignedResources {
    pub fn load(layout: &Layout, version: &Version) -> Result<Self, String> {
        Version::parse(version.as_str())?;
        layout.validate_machine_permissions()?;
        let base = layout.versions().join(version.as_str());
        let manifest = MachineManager::new(layout.clone()).verify_release(&base, Some(version))?;
        release::validate_installed_release(layout, &base, &manifest)?;
        let catalog = release::sha256_file(&base.join("manifest.json"))?;
        Ok(Self {
            layout: layout.clone(),
            base,
            manifest,
            catalog,
        })
    }

    pub fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }

    pub fn root(&self, logical_path: &str) -> Result<PathBuf, String> {
        if let Some(artifact) = self
            .manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.path == logical_path)
        {
            verify_file(&self.base, artifact)?;
            return Ok(self.base.clone());
        }
        let extension = self
            .manifest
            .extensions
            .iter()
            .find(|extension| {
                extension
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.path == logical_path)
            })
            .ok_or_else(|| format!("signed release does not declare resource '{logical_path}'"))?;
        self.extension_root(extension)
    }

    pub fn path(&self, logical_path: &str) -> Result<PathBuf, String> {
        Ok(self.root(logical_path)?.join(logical_path))
    }

    pub fn read(&self, logical_path: &str) -> Result<Vec<u8>, String> {
        let path = self.path(logical_path)?;
        let artifact = self
            .manifest
            .artifacts
            .iter()
            .chain(
                self.manifest
                    .extensions
                    .iter()
                    .flat_map(|extension| &extension.artifacts),
            )
            .find(|artifact| artifact.path == logical_path)
            .expect("root required signed artifact");
        let bytes =
            fs::read(path).map_err(|error| format!("cannot read signed resource: {error}"))?;
        use sha2::{Digest, Sha256};
        if bytes.len() as u64 != artifact.bytes
            || release::hex(&Sha256::digest(&bytes)) != artifact.sha256
        {
            return Err("signed resource changed while reading".into());
        }
        Ok(bytes)
    }

    pub fn artifacts(&self, prefix: &str) -> Result<Vec<Artifact>, String> {
        let mut artifacts = Vec::new();
        for artifact in self
            .manifest
            .artifacts
            .iter()
            .filter(|artifact| artifact.path.starts_with(prefix))
        {
            verify_file(&self.base, artifact)?;
            artifacts.push(artifact.clone());
        }
        for extension in &self.manifest.extensions {
            if extension
                .artifacts
                .iter()
                .any(|artifact| artifact.path.starts_with(prefix))
            {
                self.extension_root(extension)?;
                artifacts.extend(
                    extension
                        .artifacts
                        .iter()
                        .filter(|artifact| artifact.path.starts_with(prefix))
                        .cloned(),
                );
            }
        }
        Ok(artifacts)
    }

    fn store(&self) -> PathBuf {
        self.layout
            .extensions()
            .join(self.manifest.version.as_str())
            .join(&self.catalog)
    }

    fn destination(&self, id: &ExtensionId) -> PathBuf {
        self.store().join(id.name())
    }

    fn extension(&self, id: &ExtensionId) -> Result<&Extension, String> {
        self.manifest
            .extensions
            .iter()
            .find(|extension| extension.id() == *id)
            .ok_or_else(|| {
                format!(
                    "Dever {} does not declare extension '{}'",
                    self.manifest.version,
                    id.name()
                )
            })
    }

    fn extension_root(&self, extension: &Extension) -> Result<PathBuf, String> {
        let id = extension.id();
        let root = self.destination(&id);
        if !self.store_present()? || !exists(&root)? {
            return Err(id.missing());
        }
        verify_extension(&root, &extension.artifacts)?;
        Ok(root)
    }

    fn store_present(&self) -> Result<bool, String> {
        let mut parent = self.layout.cache();
        for directory in [
            self.layout.extensions(),
            self.layout
                .extensions()
                .join(self.manifest.version.as_str()),
            self.store(),
        ] {
            if !exists(&directory)? {
                return Ok(false);
            }
            release::validate_shared_directory(&directory, &parent, "extension store")?;
            parent = directory;
        }
        Ok(true)
    }

    pub(super) fn installed_extensions(&self) -> Result<Vec<ExtensionId>, String> {
        if !self.store_present()? {
            return Ok(Vec::new());
        }
        let mut installed = Vec::new();
        for entry in fs::read_dir(self.store()).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let extension = self
                .manifest
                .extensions
                .iter()
                .find(|extension| entry.file_name() == extension.id().name().as_str())
                .ok_or("extension store contains an undeclared entry")?;
            self.extension_root(extension)?;
            installed.push(extension.id());
        }
        Ok(installed)
    }
}

pub(super) fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "cannot inspect extension path '{}': {error}",
            path.display()
        )),
    }
}

fn verify_file(root: &Path, artifact: &Artifact) -> Result<(), String> {
    let mut parent = root.to_path_buf();
    let relative = Path::new(&artifact.path);
    for component in relative.parent().into_iter().flat_map(Path::components) {
        let directory = parent.join(component);
        release::validate_shared_directory(&directory, &parent, "signed resource directory")?;
        parent = directory;
    }
    let path = root.join(relative);
    release::validate_shared_file(&path, &parent, "signed resource", false)?;
    if fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .len()
        != artifact.bytes
        || release::sha256_file(&path)? != artifact.sha256
    {
        return Err(format!(
            "signed resource '{}' failed integrity verification",
            artifact.path
        ));
    }
    Ok(())
}

pub(super) fn verify_extension(root: &Path, artifacts: &[Artifact]) -> Result<(), String> {
    release::validate_shared_directory(
        root,
        root.parent().ok_or("extension root has no parent")?,
        "installed extension",
    )?;
    for artifact in artifacts {
        verify_file(root, artifact)?;
    }
    fn entries(root: &Path, directory: &Path, expected: &BTreeSet<&str>) -> Result<(), String> {
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_str()
                .ok_or("extension path is not UTF-8")?;
            if kind.is_dir() {
                release::validate_shared_directory(&path, directory, "extension directory")?;
                if !expected
                    .iter()
                    .any(|artifact| artifact.starts_with(&format!("{relative}/")))
                {
                    return Err("installed extension contains an unsigned directory".into());
                }
                entries(root, &path, expected)?;
            } else if !kind.is_file() || !expected.contains(relative) {
                return Err("installed extension contains an unsigned or non-regular file".into());
            }
        }
        Ok(())
    }
    entries(
        root,
        root,
        &artifacts
            .iter()
            .map(|artifact| artifact.path.as_str())
            .collect(),
    )
}

pub(super) fn ensure(layout: &Layout, version: &Version, id: &ExtensionId) -> Result<(), String> {
    prepare_extension_with(
        layout,
        version,
        id,
        &super::release_source::ReleaseSource::official(),
    )
}

pub fn prepare_extension_with(
    layout: &Layout,
    version: &Version,
    id: &ExtensionId,
    source: &dyn ExtensionSource,
) -> Result<(), String> {
    Version::parse(version.as_str())?;
    let _lock = InstallLock::acquire_wait(layout)?;
    ensure_locked(layout, version, id, source)
}

pub(super) fn ensure_locked(
    layout: &Layout,
    version: &Version,
    id: &ExtensionId,
    source: &dyn ExtensionSource,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    super::bootstrap::require_no_pending(layout)?;
    let resources = SignedResources::load(layout, version)?;
    let extension = resources.extension(id)?;
    let destination = resources.destination(id);
    if exists(&destination)? {
        resources.extension_root(extension)?;
        return Ok(());
    }
    let mut parent = layout.cache();
    for directory in [
        layout.extensions(),
        layout.extensions().join(version.as_str()),
        resources.store(),
    ] {
        if !exists(&directory)? {
            fs::create_dir(&directory)
                .map_err(|error| format!("cannot create extension store: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
                    .map_err(|error| error.to_string())?;
            }
            release::sync_directory(&parent).map_err(|error| error.to_string())?;
        }
        release::validate_shared_directory(&directory, &parent, "extension store")?;
        parent = directory;
    }
    super::release_source::prepare_extension(layout, version, extension, &destination, source)
}
