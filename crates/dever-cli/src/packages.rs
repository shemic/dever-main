//! Explicit Package resolution and offline, lock-bound source loading.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use dever_core::source::PackageSource;
use node_semver::{Range, Version};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::libs::{self, ArtifactStore, LibSpec};

const INDEX_FORMAT: &str = "dever-package-index-v1";
const MANIFEST_FORMAT: &str = "dever-package-v1";
const MAX_INDEX_BYTES: usize = 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PACKAGES: usize = 256;
const MAX_SEARCH_STEPS: usize = 4096;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRequest {
    pub name: String,
    pub requirement: String,
}

impl FromStr for PackageRequest {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (name, requirement) = value
            .rsplit_once('@')
            .ok_or("Package must be <name>@<version-range>")?;
        valid_name(name)?;
        requirement
            .parse::<Range>()
            .map_err(|error| format!("invalid Package version range: {error}"))?;
        Ok(Self {
            name: name.into(),
            requirement: requirement.into(),
        })
    }
}

impl PackageRequest {
    pub fn key(&self) -> String {
        format!("{}@{}", self.name, self.requirement)
    }

    fn matches(&self, version: &str) -> Result<bool, String> {
        let range = self
            .requirement
            .parse::<Range>()
            .map_err(|error| error.to_string())?;
        let version = version
            .parse::<Version>()
            .map_err(|error| error.to_string())?;
        Ok(version.satisfies(&range))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDependency {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub bytes: u64,
    pub dependencies: Vec<PackageDependency>,
    pub manifest_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PackageManifest {
    format: String,
    name: String,
    version: String,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    lib: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageIndex {
    format: String,
    name: String,
    versions: Vec<IndexVersion>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexVersion {
    version: String,
    sha256: String,
    bytes: u64,
}

pub trait PackageTransport {
    fn get(&self, path: &str, limit: usize) -> Result<Vec<u8>, String>;
}

pub struct HttpPackageTransport {
    origin: Url,
    agent: ureq::Agent,
}

impl HttpPackageTransport {
    pub fn new(origin: &str) -> Result<Self, String> {
        let origin = Url::parse(origin).map_err(|_| "invalid Package registry URL")?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || origin.username() != ""
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err("Package registry must be an HTTPS origin".into());
        }
        Ok(Self::at(origin))
    }

    fn at(origin: Url) -> Self {
        let config = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .build();
        Self {
            origin,
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// Only a caller-owned loopback peer may replace the HTTPS origin in tests.
    pub fn loopback(origin: &str) -> Result<Self, String> {
        let origin = Url::parse(origin).map_err(|_| "invalid loopback Package registry")?;
        if origin.scheme() != "http"
            || !matches!(origin.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            || origin.username() != ""
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err("Package test registry must be an owned IPv4 loopback origin".into());
        }
        Ok(Self::at(origin))
    }
}

impl PackageTransport for HttpPackageTransport {
    fn get(&self, path: &str, limit: usize) -> Result<Vec<u8>, String> {
        if !path.starts_with('/')
            || path.contains(['?', '#', '\\'])
            || path
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err("invalid Package registry path".into());
        }
        let url = self
            .origin
            .join(&path[1..])
            .map_err(|_| "invalid Package registry path")?;
        let mut response = self
            .agent
            .get(url.as_str())
            .call()
            .map_err(|_| "Package registry request failed")?;
        if !response.status().is_success() {
            return Err("Package registry returned a non-success response".into());
        }
        response
            .body_mut()
            .with_config()
            .limit(limit as u64 + 1)
            .read_to_vec()
            .map_err(|_| "cannot read Package registry response".into())
            .and_then(|bytes| {
                (bytes.len() <= limit)
                    .then_some(bytes)
                    .ok_or("Package registry response exceeds limit".into())
            })
    }
}

fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("Package name must use lower_snake_case".into());
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("Package SHA-256 must be lowercase hexadecimal".into());
    }
    Ok(())
}

struct ManifestArchive {
    manifest: PackageManifest,
    files: Vec<(String, Vec<u8>)>,
    manifest_sha256: String,
}

fn manifest(
    archive: &[u8],
    expected_name: &str,
    expected_version: &str,
) -> Result<ManifestArchive, String> {
    let files = libs::archive_files("zip", archive)?;
    let manifest_bytes = files
        .iter()
        .find(|(path, _)| path == "dever-package.json")
        .map(|(_, bytes)| bytes)
        .ok_or("Package archive lacks dever-package.json")?;
    let text = std::str::from_utf8(manifest_bytes).map_err(|_| "Package manifest is not UTF-8")?;
    dever_runtime::wire::parse(text)
        .map_err(|error| format!("invalid Package manifest: {error}"))?;
    let manifest: PackageManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("invalid Package manifest: {error}"))?;
    if manifest.format != MANIFEST_FORMAT
        || manifest.name != expected_name
        || manifest.version != expected_version
    {
        return Err("Package archive manifest identity differs from registry metadata".into());
    }
    valid_name(&manifest.name)?;
    manifest
        .version
        .parse::<Version>()
        .map_err(|_| "invalid Package version")?;
    for (name, requirement) in &manifest.dependencies {
        valid_name(name)?;
        requirement
            .parse::<Range>()
            .map_err(|_| "invalid Package dependency range")?;
    }
    for request in &manifest.lib {
        request.parse::<LibSpec>()?;
    }
    let mut source_count = 0;
    for (path, _) in &files {
        if path == "dever-package.json" {
            continue;
        }
        let valid_owner = path.starts_with(&format!("module/{expected_name}/"))
            || path.starts_with(&format!("worker/{expected_name}/"));
        if !valid_owner {
            return Err(format!("Package archive contains unowned path '{path}'"));
        }
        if path.starts_with("module/") && (path.ends_with(".dever") || path.ends_with(".dever.md"))
        {
            source_count += 1;
        }
    }
    if source_count == 0 {
        return Err("Package archive has no Dever source".into());
    }
    let manifest_sha256 = digest(manifest_bytes);
    Ok(ManifestArchive {
        manifest,
        files,
        manifest_sha256,
    })
}

pub fn normalize_lock(packages: &mut Vec<LockedPackage>) -> Result<(), String> {
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    let mut names = BTreeSet::new();
    for package in packages {
        valid_name(&package.name)?;
        package
            .version
            .parse::<Version>()
            .map_err(|_| "invalid locked Package version")?;
        validate_digest(&package.sha256)?;
        validate_digest(&package.manifest_sha256)?;
        if package.bytes == 0
            || package.bytes > MAX_ARCHIVE_BYTES
            || !names.insert(package.name.clone())
        {
            return Err("invalid or duplicate locked Package".into());
        }
        package
            .dependencies
            .sort_by(|left, right| left.name.cmp(&right.name));
        for dependency in &package.dependencies {
            valid_name(&dependency.name)?;
            dependency
                .version
                .parse::<Version>()
                .map_err(|_| "invalid locked Package dependency")?;
        }
    }
    Ok(())
}

pub fn doctor_lock(packages: &[LockedPackage]) -> Result<(), String> {
    let mut canonical = packages.to_vec();
    normalize_lock(&mut canonical)?;
    if canonical != packages {
        return Err("Package lock is not canonical".into());
    }
    let by_name = packages
        .iter()
        .map(|package| (package.name.as_str(), package.version.as_str()))
        .collect::<BTreeMap<_, _>>();
    for package in packages {
        let mut dependencies = BTreeSet::new();
        for dependency in &package.dependencies {
            if !dependencies.insert(&dependency.name) || dependency.name == package.name {
                return Err(format!(
                    "locked Package {} has duplicate or self dependency",
                    package.name
                ));
            }
            if by_name.get(dependency.name.as_str()) != Some(&dependency.version.as_str()) {
                return Err(format!(
                    "locked Package {} has a missing or mismatched dependency {}",
                    package.name, dependency.name
                ));
            }
        }
    }
    let mut complete = BTreeSet::new();
    while complete.len() < packages.len() {
        let before = complete.len();
        for package in packages {
            if package
                .dependencies
                .iter()
                .all(|dependency| complete.contains(&dependency.name))
            {
                complete.insert(package.name.clone());
            }
        }
        if complete.len() == before {
            return Err("cyclic locked Package dependencies".into());
        }
    }
    Ok(())
}

fn settings(project_root: &Path) -> Result<(String, Vec<PackageRequest>), String> {
    if !project_root.join("config/setting.json").exists() {
        return Ok((String::new(), Vec::new()));
    }
    let settings = dever_runtime::config::Settings::load_project(project_root)?;
    match settings.packages() {
        Some(package) => {
            HttpPackageTransport::new(package.registry())?;
            let mut roots = package
                .roots()
                .iter()
                .map(|root| root.parse::<PackageRequest>())
                .collect::<Result<Vec<_>, _>>()?;
            roots.sort();
            roots.dedup();
            if roots
                .iter()
                .map(|root| &root.name)
                .collect::<BTreeSet<_>>()
                .len()
                != roots.len()
            {
                return Err("setting.json package.use repeats a Package name".into());
            }
            Ok((package.registry().into(), roots))
        }
        None => Ok((String::new(), Vec::new())),
    }
}

#[derive(Clone)]
struct SelectedVersion {
    index: IndexVersion,
    manifest: PackageManifest,
    manifest_sha256: String,
}

struct PackageResolver<'a> {
    transport: &'a dyn PackageTransport,
    store: &'a dyn ArtifactStore,
    indexes: BTreeMap<String, Vec<IndexVersion>>,
    steps: usize,
}

impl PackageResolver<'_> {
    fn index(&mut self, name: &str) -> Result<Vec<IndexVersion>, String> {
        if let Some(index) = self.indexes.get(name) {
            return Ok(index.clone());
        }
        let bytes = self
            .transport
            .get(&format!("/v1/packages/{name}/index.json"), MAX_INDEX_BYTES)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| "Package index is not UTF-8")?;
        dever_runtime::wire::parse(text)
            .map_err(|error| format!("invalid Package index: {error}"))?;
        let index: PackageIndex = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid Package index: {error}"))?;
        if index.format != INDEX_FORMAT || index.name != name {
            return Err(format!("Package index identity differs for {name}"));
        }
        let mut seen = BTreeSet::new();
        for version in &index.versions {
            version
                .version
                .parse::<Version>()
                .map_err(|_| "invalid indexed Package version")?;
            validate_digest(&version.sha256)?;
            if version.bytes == 0
                || version.bytes > MAX_ARCHIVE_BYTES
                || !seen.insert(version.version.clone())
            {
                return Err(format!(
                    "invalid or duplicate Package index version for {name}"
                ));
            }
        }
        let mut versions = index.versions;
        versions.sort_by(|left, right| {
            right
                .version
                .parse::<Version>()
                .expect("validated version")
                .cmp(&left.version.parse::<Version>().expect("validated version"))
        });
        self.indexes.insert(name.into(), versions.clone());
        Ok(versions)
    }

    fn archive(&self, name: &str, index: &IndexVersion) -> Result<Vec<u8>, String> {
        let bytes = self.transport.get(
            &format!("/v1/packages/{name}/{}.zip", index.version),
            index.bytes as usize,
        )?;
        if bytes.len() as u64 != index.bytes || digest(&bytes) != index.sha256 {
            return Err(format!(
                "Package {name}@{} differs from registry index",
                index.version
            ));
        }
        let published = self.store.publish(&bytes, "package")?;
        if published != index.sha256 {
            return Err("Package cache receipt differs from registry index".into());
        }
        Ok(bytes)
    }

    fn select(
        &mut self,
        constraints: BTreeMap<String, Vec<PackageRequest>>,
        selected: BTreeMap<String, SelectedVersion>,
    ) -> Result<BTreeMap<String, SelectedVersion>, String> {
        self.steps += 1;
        if self.steps > MAX_SEARCH_STEPS || constraints.len() > MAX_PACKAGES {
            return Err("Package resolution budget exceeded".into());
        }
        for (name, requests) in &constraints {
            if let Some(chosen) = selected.get(name)
                && !requests
                    .iter()
                    .all(|request| request.matches(&chosen.index.version).unwrap_or(false))
            {
                return Err(format!(
                    "Package {name} has incompatible version constraints"
                ));
            }
        }
        let Some((name, requests)) = constraints
            .iter()
            .find(|(name, _)| !selected.contains_key(*name))
        else {
            return Ok(selected);
        };
        let name = name.clone();
        let requests = requests.clone();
        let mut conflict = None;
        for index in self.index(&name)? {
            if !requests
                .iter()
                .all(|request| request.matches(&index.version).unwrap_or(false))
            {
                continue;
            }
            let archive = self.archive(&name, &index)?;
            let ManifestArchive {
                manifest,
                manifest_sha256,
                ..
            } = manifest(&archive, &name, &index.version)?;
            let mut next_constraints = constraints.clone();
            for (dependency, requirement) in &manifest.dependencies {
                next_constraints
                    .entry(dependency.clone())
                    .or_default()
                    .push(PackageRequest {
                        name: dependency.clone(),
                        requirement: requirement.clone(),
                    });
            }
            let mut next_selected = selected.clone();
            next_selected.insert(
                name.clone(),
                SelectedVersion {
                    index,
                    manifest,
                    manifest_sha256,
                },
            );
            match self.select(next_constraints, next_selected) {
                Ok(result) => return Ok(result),
                Err(error)
                    if error.contains("incompatible version constraints")
                        || error.contains("no version satisfies") =>
                {
                    conflict = Some(error)
                }
                Err(error) => return Err(error),
            }
        }
        Err(conflict
            .unwrap_or_else(|| format!("no Package version satisfies constraints for {name}")))
    }
}

struct ResolvedClosure {
    packages: Vec<LockedPackage>,
    libs: Vec<LibSpec>,
    sources: Vec<PackageSource>,
}

fn resolve_with(
    roots: &[PackageRequest],
    transport: &dyn PackageTransport,
    store: &dyn ArtifactStore,
) -> Result<ResolvedClosure, String> {
    let mut constraints = BTreeMap::<String, Vec<PackageRequest>>::new();
    for request in roots {
        constraints
            .entry(request.name.clone())
            .or_default()
            .push(request.clone());
    }
    let selected = PackageResolver {
        transport,
        store,
        indexes: BTreeMap::new(),
        steps: 0,
    }
    .select(constraints, BTreeMap::new())?;
    let mut packages = Vec::new();
    let mut libs = BTreeSet::new();
    let mut sources = Vec::new();
    for (name, chosen) in &selected {
        let archive = store.verify_exact(&chosen.index.sha256, chosen.index.bytes, "package")?;
        let ManifestArchive {
            files,
            manifest_sha256,
            ..
        } = manifest(&archive, name, &chosen.index.version)?;
        if manifest_sha256 != chosen.manifest_sha256 {
            return Err(format!("Package {name} manifest changed"));
        }
        for request in &chosen.manifest.lib {
            libs.insert(request.parse::<LibSpec>()?);
        }
        sources.extend(package_sources(files)?);
        let dependencies = chosen
            .manifest
            .dependencies
            .keys()
            .map(|dependency| PackageDependency {
                name: dependency.clone(),
                version: selected[dependency].index.version.clone(),
            })
            .collect();
        packages.push(LockedPackage {
            name: name.clone(),
            version: chosen.index.version.clone(),
            sha256: chosen.index.sha256.clone(),
            bytes: chosen.index.bytes,
            dependencies,
            manifest_sha256: chosen.manifest_sha256.clone(),
        });
    }
    normalize_lock(&mut packages)?;
    Ok(ResolvedClosure {
        packages,
        libs: libs.into_iter().collect(),
        sources,
    })
}

fn retain_locked_closure(
    project_root: &Path,
    roots: &[PackageRequest],
    lock: &libs::LockFile,
    store: &dyn ArtifactStore,
) -> Result<ResolvedClosure, String> {
    let files = owned_files_with(project_root, store)?;
    let by_name = lock
        .packages
        .iter()
        .map(|package| (package.name.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let mut names = BTreeSet::new();
    let mut pending = roots
        .iter()
        .map(|root| root.name.as_str())
        .collect::<Vec<_>>();
    while let Some(name) = pending.pop() {
        if !names.insert(name) {
            continue;
        }
        let package = by_name
            .get(name)
            .ok_or_else(|| format!("Package {name} is missing from dever.lock"))?;
        pending.extend(
            package
                .dependencies
                .iter()
                .map(|dependency| dependency.name.as_str()),
        );
    }
    let packages = lock
        .packages
        .iter()
        .filter(|package| names.contains(package.name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut libs = BTreeSet::new();
    for package in &packages {
        let archive = store.verify_exact(&package.sha256, package.bytes, "package")?;
        let ManifestArchive { manifest, .. } = manifest(&archive, &package.name, &package.version)?;
        for spec in manifest.lib {
            libs.insert(spec.parse::<LibSpec>()?);
        }
    }
    let sources = package_sources(
        files
            .into_iter()
            .filter(|(path, _)| {
                path.strip_prefix("module/")
                    .and_then(|path| path.split('/').next())
                    .is_some_and(|name| names.contains(name))
            })
            .collect(),
    )?;
    Ok(ResolvedClosure {
        packages,
        libs: libs.into_iter().collect(),
        sources,
    })
}

fn package_sources(files: Vec<(String, Vec<u8>)>) -> Result<Vec<PackageSource>, String> {
    files
        .into_iter()
        .filter(|(path, _)| {
            path.starts_with("module/") && (path.ends_with(".dever") || path.ends_with(".dever.md"))
        })
        .map(|(path, bytes)| {
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("Package source '{path}' is not UTF-8"))?;
            Ok(PackageSource {
                path: PathBuf::from(path.strip_prefix("module/").expect("filtered module path")),
                display: PathBuf::from(format!("package/{path}")),
                text,
            })
        })
        .collect()
}

pub fn owned_files(project_root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let (_, roots) = settings(project_root)?;
    if roots.is_empty() {
        if project_root.join("dever.lock").exists()
            && !libs::read_lock(&project_root.join("dever.lock"))?
                .packages
                .is_empty()
        {
            return Err("Package lock has no setting.json roots".into());
        }
        return Ok(BTreeMap::new());
    }
    let store = libs::managed_artifact_store()?;
    owned_files_with(project_root, &store)
}

pub fn owned_files_with(
    project_root: &Path,
    store: &dyn ArtifactStore,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let (_, roots) = settings(project_root)?;
    let lock_path = project_root.join("dever.lock");
    if roots.is_empty() && !lock_path.exists() {
        return Ok(BTreeMap::new());
    }
    if !lock_path.exists() {
        return Err("Package declarations require dever.lock; run 'dever package update'".into());
    }
    let lock = libs::read_lock(&lock_path)?;
    let packages = &lock.packages;
    validate_locked_roots(&roots, packages)?;
    let mut files = BTreeMap::new();
    for package in packages {
        if fs::symlink_metadata(project_root.join("module").join(&package.name)).is_ok() {
            return Err(format!(
                "Package '{}' collides with a local component",
                package.name
            ));
        }
        let archive = store.verify_exact(&package.sha256, package.bytes, "package")?;
        let ManifestArchive {
            manifest,
            files: entries,
            manifest_sha256,
        } = manifest(&archive, &package.name, &package.version)?;
        if manifest_sha256 != package.manifest_sha256 {
            return Err(format!("Package {} manifest changed", package.name));
        }
        let dependencies = manifest
            .dependencies
            .iter()
            .map(|(name, requirement)| {
                let dependency = packages
                    .iter()
                    .find(|package| &package.name == name)
                    .ok_or_else(|| {
                        format!("Package {} dependency {name} is missing", package.name)
                    })?;
                if !(PackageRequest {
                    name: name.clone(),
                    requirement: requirement.clone(),
                })
                .matches(&dependency.version)?
                {
                    return Err(format!(
                        "Package {} dependency {name} lock is stale",
                        package.name
                    ));
                }
                Ok(PackageDependency {
                    name: name.clone(),
                    version: dependency.version.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if dependencies != package.dependencies {
            return Err(format!("Package {} dependencies changed", package.name));
        }
        for (path, bytes) in entries {
            if path == "dever-package.json" {
                continue;
            }
            let owned_path = project_root.join(&path);
            match fs::symlink_metadata(&owned_path) {
                Ok(_) => {
                    return Err(format!(
                        "Package '{}' collides with project path '{}'",
                        package.name, path
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("cannot inspect Package path '{path}': {error}")),
            }
            if files.insert(path.clone(), bytes).is_some() {
                return Err(format!("Package path '{path}' is duplicated"));
            }
        }
    }
    Ok(files)
}

fn validate_locked_roots(
    roots: &[PackageRequest],
    packages: &[LockedPackage],
) -> Result<(), String> {
    doctor_lock(packages)?;
    for root in roots {
        let package = packages
            .iter()
            .find(|package| package.name == root.name)
            .ok_or_else(|| format!("Package {} is missing from dever.lock", root.name))?;
        if !root.matches(&package.version)? {
            return Err(format!("Package {} lock is stale", root.name));
        }
    }
    if roots.is_empty() && !packages.is_empty() {
        return Err("Package lock has no setting.json roots".into());
    }
    let by_name = packages
        .iter()
        .map(|package| (package.name.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let mut reachable = BTreeSet::new();
    let mut pending = roots
        .iter()
        .map(|root| root.name.as_str())
        .collect::<Vec<_>>();
    while let Some(name) = pending.pop() {
        if !reachable.insert(name) {
            continue;
        }
        pending.extend(
            by_name[name]
                .dependencies
                .iter()
                .map(|dependency| dependency.name.as_str()),
        );
    }
    if reachable.len() != packages.len() {
        return Err("Package lock contains unreferenced Package entries".into());
    }
    Ok(())
}

pub fn sources(project_root: &Path) -> Result<Vec<PackageSource>, String> {
    if settings(project_root)?.1.is_empty() {
        return package_sources(owned_files(project_root)?.into_iter().collect());
    }
    let store = libs::managed_artifact_store()?;
    sources_with(project_root, &store)
}

pub fn sources_with(
    project_root: &Path,
    store: &dyn ArtifactStore,
) -> Result<Vec<PackageSource>, String> {
    package_sources(owned_files_with(project_root, store)?.into_iter().collect())
}

pub fn owned_worker_bytes(project_root: &Path, entry: &str) -> Result<Option<Vec<u8>>, String> {
    if !owns_worker(project_root, entry)? {
        return Ok(None);
    }
    let store = libs::managed_artifact_store()?;
    locked_worker_bytes(project_root, entry, &store)
}

pub fn owned_worker_bytes_with(
    project_root: &Path,
    entry: &str,
    store: &dyn ArtifactStore,
) -> Result<Option<Vec<u8>>, String> {
    if !owns_worker(project_root, entry)? {
        return Ok(None);
    }
    locked_worker_bytes(project_root, entry, store)
}

fn locked_worker_bytes(
    project_root: &Path,
    entry: &str,
    store: &dyn ArtifactStore,
) -> Result<Option<Vec<u8>>, String> {
    owned_files_with(project_root, store)?
        .remove(entry)
        .map(Some)
        .ok_or_else(|| format!("Package Worker entry '{entry}' is missing from its locked archive"))
}

pub fn owns_worker(project_root: &Path, entry: &str) -> Result<bool, String> {
    let Some((name, relative_entry)) = entry
        .strip_prefix("worker/")
        .or_else(|| entry.strip_prefix("module/"))
        .and_then(|path| path.split_once('/'))
    else {
        return Ok(false);
    };
    if name.is_empty() || relative_entry.is_empty() {
        return Ok(false);
    }
    let (_, roots) = settings(project_root)?;
    let lock_path = project_root.join("dever.lock");
    if roots.is_empty() && !lock_path.exists() {
        return Ok(false);
    }
    if !lock_path.exists() {
        return Err("Package declarations require dever.lock; run 'dever package update'".into());
    }
    let lock = libs::read_lock(&lock_path)?;
    validate_locked_roots(&roots, &lock.packages)?;
    Ok(lock.packages.iter().any(|package| package.name == name))
}

pub fn declared_libs(project_root: &Path) -> Result<Vec<LibSpec>, String> {
    if settings(project_root)?.1.is_empty() {
        return Ok(Vec::new());
    }
    let store = libs::managed_artifact_store()?;
    declared_libs_with(project_root, &store)
}

pub fn declared_libs_with(
    project_root: &Path,
    store: &dyn ArtifactStore,
) -> Result<Vec<LibSpec>, String> {
    let (_, roots) = settings(project_root)?;
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    owned_files_with(project_root, store)?;
    let lock = libs::read_lock(&project_root.join("dever.lock"))?;
    let mut libs = BTreeSet::new();
    for package in lock.packages {
        let archive = store.verify_exact(&package.sha256, package.bytes, "package")?;
        let ManifestArchive {
            manifest,
            manifest_sha256,
            ..
        } = manifest(&archive, &package.name, &package.version)?;
        if manifest_sha256 != package.manifest_sha256 {
            return Err(format!("Package {} manifest changed", package.name));
        }
        for request in manifest.lib {
            libs.insert(request.parse::<LibSpec>()?);
        }
    }
    Ok(libs.into_iter().collect())
}

fn write_roots(
    project_root: &Path,
    registry: &str,
    roots: &[PackageRequest],
) -> Result<(), String> {
    libs::reject_declaration_symlinks(project_root)?;
    let path = project_root.join("config/setting.json");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
    dever_runtime::wire::parse(&text).map_err(|error| format!("invalid setting.json: {error}"))?;
    let mut document: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&text).map_err(|error| format!("invalid setting.json: {error}"))?;
    document.insert(
        "package".into(),
        serde_json::json!({
            "registry": registry,
            "use": roots.iter().map(PackageRequest::key).collect::<Vec<_>>(),
        }),
    );
    let mut bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    replace_settings(&path, &bytes)
}

fn replace_settings(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary =
        path.with_file_name(format!(".setting.json.package.{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("cannot stage setting.json: {error}"))?;
    use std::io::Write;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot stage setting.json: {error}"));
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("cannot replace setting.json: {error}")
    })
}

fn selected_roots(
    current: &[PackageRequest],
    command: &str,
    specs: &[String],
) -> Result<Vec<PackageRequest>, String> {
    let mut roots = current.to_vec();
    match command {
        "add" | "update" => {
            for spec in specs {
                let request = spec.parse::<PackageRequest>()?;
                roots.retain(|root| root.name != request.name);
                roots.push(request);
            }
        }
        "remove" => {
            if specs.is_empty() {
                return Err("dever package remove requires a Package name".into());
            }
            for name in specs {
                valid_name(name)?;
                if !roots.iter().any(|root| &root.name == name) {
                    return Err(format!(
                        "Package {name} is not a direct setting.json dependency"
                    ));
                }
                roots.retain(|root| &root.name != name);
            }
        }
        _ => unreachable!("command checked by caller"),
    }
    roots.sort();
    if roots
        .iter()
        .map(|root| &root.name)
        .collect::<BTreeSet<_>>()
        .len()
        != roots.len()
    {
        return Err("duplicate Package root".into());
    }
    Ok(roots)
}

fn check_project_collisions(
    project_root: &Path,
    packages: &[LockedPackage],
    store: &dyn ArtifactStore,
) -> Result<(), String> {
    for package in packages {
        if fs::symlink_metadata(project_root.join("module").join(&package.name)).is_ok() {
            return Err(format!(
                "Package '{}' collides with a local component",
                package.name
            ));
        }
        let archive = store.verify_exact(&package.sha256, package.bytes, "package")?;
        let ManifestArchive { files: entries, .. } =
            manifest(&archive, &package.name, &package.version)?;
        for (path, _) in entries {
            if path == "dever-package.json" {
                continue;
            }
            match fs::symlink_metadata(project_root.join(&path)) {
                Ok(_) => {
                    return Err(format!(
                        "Package '{}' collides with project path '{path}'",
                        package.name
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("cannot inspect Package path '{path}': {error}")),
            }
        }
    }
    Ok(())
}

pub fn execute(command: &str, project_root: &Path, specs: &[String]) -> Result<String, String> {
    let _mutation = libs::ProjectMutation::for_command(command, project_root)?;
    let (registry, _) = settings(project_root)?;
    if command == "list" {
        let lock = libs::read_lock(&project_root.join("dever.lock"))?;
        return Ok(lock
            .packages
            .iter()
            .map(|package| format!("{}@{}", package.name, package.version))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let transport = HttpPackageTransport::new(&registry)?;
    let store = libs::managed_artifact_store()?;
    execute_locked(command, project_root, specs, &transport, &store)
}

pub fn execute_with(
    command: &str,
    project_root: &Path,
    specs: &[String],
    transport: &dyn PackageTransport,
    store: &dyn ArtifactStore,
) -> Result<String, String> {
    let _mutation = libs::ProjectMutation::for_command(command, project_root)?;
    execute_locked(command, project_root, specs, transport, store)
}

fn execute_locked(
    command: &str,
    project_root: &Path,
    specs: &[String],
    transport: &dyn PackageTransport,
    store: &dyn ArtifactStore,
) -> Result<String, String> {
    let (registry, current) = settings(project_root)?;
    let lock_path = project_root.join("dever.lock");
    match command {
        "list" => {
            let lock = libs::read_lock(&lock_path)?;
            Ok(lock
                .packages
                .iter()
                .map(|package| format!("{}@{}", package.name, package.version))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "doctor" => {
            if !specs.is_empty() {
                return Err("dever package doctor takes no specs".into());
            }
            let sources =
                package_sources(owned_files_with(project_root, store)?.into_iter().collect())?;
            let lock = libs::read_lock(&lock_path)?;
            libs::doctor(&lock)?;
            let workers = libs::source_workers_with_packages(
                project_root,
                &sources,
                crate::toolchain::BuildTarget::host()?,
            )?;
            if workers != lock.workers {
                return Err("Package Worker binding differs from dever.lock".into());
            }
            Ok(format!("{} is valid", lock_path.display()))
        }
        "add" | "update" | "remove" => {
            let roots = selected_roots(&current, command, specs)?;
            if command == "add" && specs.is_empty() {
                return Err("dever package add requires <name>@<version-range>".into());
            }
            if command == "update" && roots.is_empty() {
                return Err("dever package update requires Package roots".into());
            }
            let previous = if lock_path.exists() {
                Some(libs::read_lock(&lock_path)?)
            } else {
                None
            };
            let closure = if command == "remove" {
                retain_locked_closure(
                    project_root,
                    &roots,
                    previous
                        .as_ref()
                        .ok_or("dever.lock is required for offline Package removal")?,
                    store,
                )?
            } else {
                resolve_with(&roots, transport, store)?
            };
            check_project_collisions(project_root, &closure.packages, store)?;
            let workers = libs::source_workers_with_packages(
                project_root,
                &closure.sources,
                crate::toolchain::BuildTarget::host()?,
            )?;
            let mut requested = libs::read_declarations(project_root)?;
            requested.extend(closure.libs);
            requested.sort();
            requested.dedup();
            let mut lock = if command == "remove" {
                libs::retain_locked_libs(
                    previous.as_ref().expect("remove requires lock"),
                    &requested,
                    workers,
                )?
            } else {
                libs::resolve_project(
                    project_root,
                    &requested,
                    workers,
                    crate::toolchain::BuildTarget::host()?,
                )?
            };
            lock.packages = closure.packages;
            libs::doctor(&lock)?;
            lock.encode()?;
            let previous_settings = fs::read(project_root.join("config/setting.json"))
                .map_err(|error| format!("cannot save setting.json for rollback: {error}"))?;
            write_roots(project_root, &registry, &roots)?;
            if let Err(error) = lock.write_atomic(project_root) {
                let settings_path = project_root.join("config/setting.json");
                return match replace_settings(&settings_path, &previous_settings) {
                    Ok(()) => Err(error),
                    Err(rollback) => {
                        Err(format!("{error}; setting.json rollback failed: {rollback}"))
                    }
                };
            }
            Ok(format!(
                "locked {} Package(s) in {}",
                lock.packages.len(),
                lock_path.display()
            ))
        }
        _ => Err(
            "Usage: dever package add|update|remove|list|doctor <project-root> [spec ...]".into(),
        ),
    }
}
