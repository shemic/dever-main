//! Deterministic, offline-first external library resolution.
//!
//! This module owns the project lock contract and resolver orchestration.  It
//! deliberately does not know how a machine cache is authenticated: released
//! clients will supply an implementation of `ArtifactStore` through `deverd`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod build;
mod mutation;
mod registry;
pub mod restore;
pub(crate) use mutation::ProjectMutation;
pub use registry::npm;
pub(crate) use registry::python_dependencies;
pub use registry::sumdb;
pub use registry::{HttpRegistry, RegistryResolver, RegistryRuntime, RegistryTransport};
pub(crate) use registry::{InstalledRegistryPack, archive_files};

const LOCK_FORMAT: &str = "dever-lock-v6";
const TOOLCHAIN: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Pip,
    Npm,
    Go,
}

impl Ecosystem {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pip => "pip",
            Self::Npm => "npm",
            Self::Go => "go",
        }
    }
}

impl FromStr for Ecosystem {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pip" => Ok(Self::Pip),
            "npm" => Ok(Self::Npm),
            "go" => Ok(Self::Go),
            _ => Err(format!("unsupported lib ecosystem '{value}'")),
        }
    }
}

/// The provider boundary is deliberately separate from lock-file handling.
/// A released client can install a real registry/runtime provider without
/// teaching the Dever language about pip, npm, or Go details.  The current
/// development distribution only ships the deterministic fixture provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderManifest {
    pub ecosystem: Ecosystem,
    pub resolver: String,
    pub runtime_pack: String,
    pub sdk: String,
    pub available: bool,
}

impl ProviderManifest {
    pub fn fixture(ecosystem: Ecosystem) -> Self {
        let (resolver, runtime_pack, sdk) = match ecosystem {
            Ecosystem::Pip => ("fixture-pip", "fixture-cpython", "fixture-python-worker"),
            Ecosystem::Npm => ("fixture-npm", "fixture-node", "fixture-javascript-worker"),
            Ecosystem::Go => ("fixture-go", "fixture-go", "fixture-go-worker"),
        };
        Self {
            ecosystem,
            resolver: resolver.into(),
            runtime_pack: runtime_pack.into(),
            sdk: sdk.into(),
            available: true,
        }
    }

    /// Metadata for a production provider whose signed runtime is not installed.
    pub fn unavailable(ecosystem: Ecosystem) -> Self {
        let (resolver, runtime_pack, sdk) = match ecosystem {
            Ecosystem::Pip => ("pip-registry", "managed-cpython", "python-worker-sdk"),
            Ecosystem::Npm => ("npm-registry", "managed-node", "javascript-worker-sdk"),
            Ecosystem::Go => ("go-module", "managed-go", "go-worker-sdk"),
        };
        Self {
            ecosystem,
            resolver: resolver.into(),
            runtime_pack: runtime_pack.into(),
            sdk: sdk.into(),
            available: false,
        }
    }

    pub fn blocked_message(&self, spec: &LibSpec) -> String {
        format!(
            "external Lib provider '{}' for {} requires a signed {} runtime pack and the managed artifact service; run/build never use PATH, environment variables, or host package managers",
            self.resolver,
            spec.key(),
            self.runtime_pack,
        )
    }
}

/// Resolver implementations own registry and runtime-specific behavior.
/// Lock normalization, dependency validation, and artifact verification stay
/// in this module and are shared by every provider.
pub trait LibResolver {
    fn manifest(&self) -> ProviderManifest;
    fn resolve(&self, requested: &[LibSpec]) -> Result<LockFile, String>;
    fn go_verifier(&self) -> sumdb::Verifier {
        sumdb::Verifier::official()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibSpec {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
}

impl LibSpec {
    pub fn key(&self) -> String {
        format!("{}:{}@{}", self.ecosystem.as_str(), self.name, self.version)
    }

    /// Extras select dependency edges, not a different installed distribution.
    pub fn distribution_name(&self) -> &str {
        distribution_name(&self.ecosystem, &self.name)
    }
}

fn distribution_name<'a>(ecosystem: &Ecosystem, name: &'a str) -> &'a str {
    if *ecosystem == Ecosystem::Pip {
        name.split_once('[').map_or(name, |(base, _)| base)
    } else {
        name
    }
}

fn python_name(name: &str) -> Result<(String, Vec<pep508_rs::ExtraName>), String> {
    let (base, extras) = match name.split_once('[') {
        Some((base, suffix)) => {
            let extras = suffix
                .strip_suffix(']')
                .ok_or("invalid pip extras suffix")?;
            let extras = extras
                .split(',')
                .map(python_extra)
                .collect::<Result<BTreeSet<_>, _>>()?;
            (base, extras.into_iter().collect::<Vec<_>>())
        }
        None => (name, Vec::new()),
    };
    if base.is_empty() {
        return Err("pip distribution name must not be empty".into());
    }
    let base = base
        .parse::<pep508_rs::PackageName>()
        .map_err(|error| format!("invalid pip lib name: {error}"))?
        .to_string();
    let canonical = if extras.is_empty() {
        base
    } else {
        format!(
            "{base}[{}]",
            extras
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    Ok((canonical, extras))
}

fn python_extra(extra: &str) -> Result<pep508_rs::ExtraName, String> {
    // pep508_rs normalization accepts an empty string; selection and metadata
    // still require a nonempty declared extra name.
    if extra.is_empty() {
        return Err("pip extra name must not be empty".into());
    }
    extra
        .parse()
        .map_err(|error| format!("invalid pip extra: {error}"))
}

impl FromStr for LibSpec {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (ecosystem, rest) = value
            .split_once(':')
            .ok_or("lib spec must be <pip|npm|go>:<name>@<version>")?;
        let ecosystem = ecosystem.parse()?;
        let (name, version) = rest
            .rsplit_once('@')
            .ok_or("lib spec must include an exact @version")?;
        let name = if ecosystem == Ecosystem::Pip {
            python_name(name)?.0
        } else {
            name.to_owned()
        };
        validate_name(&ecosystem, &name)?;
        validate_version(&ecosystem, version)?;
        let version = if ecosystem == Ecosystem::Go {
            version.strip_prefix('v').unwrap_or(version)
        } else {
            version
        };
        Ok(Self {
            ecosystem,
            name,
            version: version.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedDependency {
    pub spec: LibSpec,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedArtifact {
    pub target: String,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub source: Option<RegistrySource>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrySource {
    pub ecosystem: Ecosystem,
    pub locator: String,
    pub filename: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePack {
    pub name: String,
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedLib {
    pub spec: LibSpec,
    pub dependencies: Vec<LockedDependency>,
    pub runtime: RuntimePack,
    pub artifacts: Vec<LockedArtifact>,
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockFile {
    pub format: String,
    pub toolchain: String,
    pub libs: Vec<LockedLib>,
    pub workers: Vec<LockedWorker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub go_sumdb: Vec<sumdb::Evidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub npm: Vec<npm::Environment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builds: Vec<build::Receipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<crate::packages::LockedPackage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedWorker {
    pub port: String,
    pub adapter: String,
    pub ecosystem: String,
    pub runtime: Option<RuntimePack>,
    pub entry: String,
    pub schema: String,
    pub capabilities: Vec<String>,
    pub operations: Vec<String>,
    pub libs: Vec<LibSpec>,
}

impl LockedWorker {
    fn from_contract(contract: dever_core::hir::ExternalWorkerContract) -> Result<Self, String> {
        let mut libs = contract
            .libs
            .iter()
            .map(|request| request.parse())
            .collect::<Result<Vec<LibSpec>, _>>()?;
        libs.sort();
        libs.dedup();
        Ok(Self {
            port: contract.port,
            adapter: contract.adapter,
            ecosystem: contract.ecosystem,
            runtime: None,
            entry: contract.entry,
            schema: contract.schema,
            capabilities: contract.capabilities,
            operations: contract.operations,
            libs,
        })
    }
}

impl LockFile {
    pub fn new(mut libs: Vec<LockedLib>) -> Result<Self, String> {
        normalize_libs(&mut libs)?;
        Ok(Self {
            format: LOCK_FORMAT.into(),
            toolchain: TOOLCHAIN.into(),
            libs,
            workers: Vec::new(),
            go_sumdb: Vec::new(),
            npm: Vec::new(),
            builds: Vec::new(),
            packages: Vec::new(),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if self.format != LOCK_FORMAT {
            return Err("unsupported dever.lock format".into());
        }
        let mut normalized = self.clone();
        normalize_libs(&mut normalized.libs)?;
        normalize_workers(&mut normalized.workers)?;
        npm::normalize(&mut normalized.npm)?;
        build::normalize(&mut normalized.builds)?;
        crate::packages::normalize_lock(&mut normalized.packages)?;
        let mut bytes =
            serde_json::to_vec_pretty(&normalized).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        // Publication and offline decoding must accept exactly the same bounds.
        let text = std::str::from_utf8(&bytes).expect("JSON serialization is UTF-8");
        dever_runtime::wire::parse(text).map_err(|error| format!("invalid dever.lock: {error}"))?;
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "dever.lock must contain UTF-8 JSON")?;
        dever_runtime::wire::parse(text).map_err(|error| format!("invalid dever.lock: {error}"))?;
        let lock: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid dever.lock: {error}"))?;
        if lock.format != LOCK_FORMAT {
            return Err(format!(
                "unsupported dever.lock format '{}'; expected {LOCK_FORMAT}",
                lock.format
            ));
        }
        if lock.toolchain != TOOLCHAIN {
            return Err(format!(
                "dever.lock was created by toolchain {}, current toolchain is {TOOLCHAIN}",
                lock.toolchain
            ));
        }
        let canonical = lock.encode()?;
        if canonical != bytes {
            return Err(
                "dever.lock is not canonical; run 'dever lib update <project-root>'".into(),
            );
        }
        Ok(lock)
    }

    pub fn write_atomic(&self, project_root: &Path) -> Result<(), String> {
        let path = project_root.join("dever.lock");
        if path.exists()
            && fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err("dever.lock must not be a symbolic link".into());
        }
        let parent = path.parent().ok_or("project root has no parent")?;
        fs::create_dir_all(parent).map_err(|e| format!("cannot create project root: {e}"))?;
        let temporary = parent.join(format!(
            ".dever.lock.{}-{}.tmp",
            std::process::id(),
            unique_suffix()
        ));
        write_new_file(&temporary, &self.encode()?)
            .map_err(|e| format!("cannot stage dever.lock: {e}"))?;
        fs::rename(&temporary, &path).map_err(|e| {
            let _ = fs::remove_file(&temporary);
            format!("cannot replace dever.lock: {e}")
        })
    }
}

#[derive(Clone, Debug)]
pub struct FixturePackage {
    pub spec: LibSpec,
    pub dependencies: Vec<LibSpec>,
    pub runtime: RuntimePack,
    pub artifacts: Vec<FixtureArtifact>,
    pub schema: String,
}

impl FixturePackage {
    fn locked(&self) -> LockedLib {
        let mut artifacts = self
            .artifacts
            .iter()
            .map(|artifact| LockedArtifact {
                target: artifact.target.clone(),
                path: artifact.path.clone(),
                bytes: artifact.bytes.len() as u64,
                sha256: sha256(&artifact.bytes),
                source: None,
            })
            .collect::<Vec<_>>();
        artifacts.sort();
        let mut dependencies = self
            .dependencies
            .iter()
            .cloned()
            .map(|spec| LockedDependency { spec })
            .collect::<Vec<_>>();
        dependencies.sort();
        LockedLib {
            spec: self.spec.clone(),
            dependencies,
            runtime: self.runtime.clone(),
            artifacts,
            schema: self.schema.clone(),
            build: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FixtureArtifact {
    pub target: String,
    pub path: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct FixtureRegistry {
    packages: BTreeMap<LibSpec, FixturePackage>,
}

impl FixtureRegistry {
    pub fn register(&mut self, package: FixturePackage) -> Result<(), String> {
        if self
            .packages
            .insert(package.spec.clone(), package)
            .is_some()
        {
            return Err("duplicate fixture lib spec".into());
        }
        Ok(())
    }

    pub fn package(&self, spec: &LibSpec) -> Option<&FixturePackage> {
        self.packages.get(spec)
    }

    pub fn resolve(&self, requested: &[LibSpec]) -> Result<LockFile, String> {
        let mut queue = requested.iter().cloned().collect::<VecDeque<_>>();
        let mut resolved = BTreeMap::<LibSpec, FixturePackage>::new();
        let mut selected = BTreeMap::<(Ecosystem, String), String>::new();
        while let Some(spec) = queue.pop_front() {
            if resolved.contains_key(&spec) {
                continue;
            }
            let key = (spec.ecosystem.clone(), spec.distribution_name().to_owned());
            if let Some(version) = selected.get(&key)
                && version != &spec.version
            {
                return Err(format!(
                    "lib version conflict for {}:{}: {version} and {}",
                    spec.ecosystem.as_str(),
                    spec.name,
                    spec.version
                ));
            }
            selected.insert(key, spec.version.clone());
            let package = self
                .packages
                .get(&spec)
                .cloned()
                .ok_or_else(|| format!("fixture registry has no {}", spec.key()))?;
            queue.extend(package.dependencies.iter().cloned());
            resolved.insert(spec, package);
        }
        let libs = resolved
            .into_values()
            .map(|package| package.locked())
            .collect::<Vec<_>>();
        LockFile::new(libs)
    }

    pub fn builtin() -> Self {
        let mut registry = Self::default();
        // Built-in fixtures are deterministic and intentionally do not access a
        // registry, shell, PATH or language-specific environment variables.
        for (ecosystem, name) in [
            (Ecosystem::Pip, "fixture"),
            (Ecosystem::Npm, "fixture"),
            (Ecosystem::Go, "example.com/fixture/lib"),
        ] {
            let spec = LibSpec {
                ecosystem: ecosystem.clone(),
                name: name.into(),
                version: "1.0.0".into(),
            };
            let bytes = format!("dever fixture {}\n", spec.key()).into_bytes();
            let _ = registry.register(FixturePackage {
                spec: spec.clone(),
                dependencies: Vec::new(),
                runtime: RuntimePack {
                    name: match ecosystem {
                        Ecosystem::Pip => "cpython",
                        Ecosystem::Npm => "node",
                        Ecosystem::Go => "go",
                    }
                    .into(),
                    version: "managed-1".into(),
                    sha256: sha256(format!("runtime:{}", ecosystem.as_str()).as_bytes()),
                },
                artifacts: vec![FixtureArtifact {
                    target: "host".into(),
                    path: format!("{}/1.0.0/package.bin", ecosystem.as_str()),
                    bytes,
                }],
                schema: format!("fixture-schema-{}-v1", ecosystem.as_str()),
            });
        }
        registry
    }
}

impl LibResolver for FixtureRegistry {
    fn manifest(&self) -> ProviderManifest {
        // The fixture registry is intentionally multi-ecosystem, so expose a
        // generic manifest only for diagnostics.  Individual package specs
        // still select their ecosystem provider below.
        ProviderManifest {
            ecosystem: Ecosystem::Pip,
            resolver: "fixture-registry".into(),
            runtime_pack: "fixture-managed".into(),
            sdk: "fixture-worker".into(),
            available: true,
        }
    }

    fn resolve(&self, requested: &[LibSpec]) -> Result<LockFile, String> {
        FixtureRegistry::resolve(self, requested)
    }
}

pub fn provider_manifest(ecosystem: Ecosystem) -> ProviderManifest {
    ProviderManifest::unavailable(ecosystem)
}

pub fn resolve_workers(
    resolver: &impl LibResolver,
    declared: &[LibSpec],
    mut workers: Vec<LockedWorker>,
) -> Result<LockFile, String> {
    normalize_workers(&mut workers)?;
    let mut resolved = BTreeMap::<LibSpec, LockedLib>::new();
    let mut go_sumdb = Vec::new();
    let mut npm = Vec::new();
    let mut builds = Vec::new();
    let mut roots = vec![declared.to_vec()];
    roots.extend(workers.iter().map(|worker| worker.libs.clone()));
    for requested in roots {
        let result = resolver.resolve(&requested)?;
        go_sumdb.extend(result.go_sumdb);
        npm.extend(result.npm);
        builds.extend(result.builds);
        for lib in result.libs {
            if let Some(previous) = resolved.get(&lib.spec) {
                if previous != &lib {
                    return Err(format!(
                        "inconsistent resolved metadata for {}",
                        lib.spec.key()
                    ));
                }
            } else {
                resolved.insert(lib.spec.clone(), lib);
            }
        }
    }
    let mut lock = LockFile::new(resolved.into_values().collect())?;
    lock.workers = workers;
    lock.go_sumdb = go_sumdb;
    npm::normalize(&mut npm)?;
    lock.npm = npm;
    build::normalize(&mut builds)?;
    lock.builds = builds;
    doctor_with_go_verifier(&lock, &resolver.go_verifier())?;
    Ok(lock)
}

pub(crate) fn resolve_project(
    project_root: &Path,
    requested: &[LibSpec],
    workers: Vec<LockedWorker>,
    target: crate::toolchain::BuildTarget,
) -> Result<LockFile, String> {
    let prior_evidence = if project_root.join("dever.lock").exists() {
        let lock = read_lock(&project_root.join("dever.lock"))?;
        doctor(&lock)?;
        lock.go_sumdb
    } else {
        Vec::new()
    };
    let registry = FixtureRegistry::builtin();
    let all = requested
        .iter()
        .chain(workers.iter().flat_map(|worker| &worker.libs))
        .collect::<Vec<_>>();
    if all.iter().all(|spec| registry.package(spec).is_some())
        && workers.iter().all(|worker| worker.ecosystem == "exec")
    {
        let mut lock = resolve_workers(&registry, requested, workers)?;
        lock.go_sumdb = prior_evidence;
        lock.encode()?;
        return Ok(lock);
    }
    let managed = PreparingRegistry(InstalledRegistry::current(target)?);
    let mut runtimes = BTreeMap::new();
    let mut ecosystems = all
        .iter()
        .filter(|spec| registry.package(spec).is_none())
        .map(|spec| spec.ecosystem.clone())
        .collect::<BTreeSet<_>>();
    for worker in workers.iter().filter(|worker| worker.ecosystem != "exec") {
        ecosystems.insert(worker.ecosystem.parse()?);
    }
    for ecosystem in ecosystems {
        runtimes.insert(
            ecosystem.clone(),
            build::Inputs::runtime(&managed, ecosystem)?.0,
        );
    }
    for worker in workers.iter().filter(|worker| worker.ecosystem != "exec") {
        let ecosystem: Ecosystem = worker.ecosystem.parse()?;
        if worker.runtime.as_ref() != runtimes.get(&ecosystem).map(|runtime| &runtime.pack) {
            return Err(format!(
                "managed Worker '{}.{}' runtime identity differs from signed release",
                worker.port, worker.adapter
            ));
        }
    }
    let store = ManagedArtifactStore {
        layout: managed.0.layout.clone(),
    };
    let transport = HttpRegistry::official();
    let verified = sumdb::verify_chain(&prior_evidence, &sumdb::Verifier::official())?;
    let previous = (!verified.checkpoint.is_empty()).then_some(verified.checkpoint);
    let build = build::Session::new(&managed);
    let resolver = RegistryResolver {
        build: Some(&build),
        transport: &transport,
        store: &store,
        runtimes,
        go_sumdb: sumdb::ChecksumDatabase::official(previous)?,
    };
    let combined = ProjectResolver {
        fixtures: &registry,
        registry: &resolver,
    };
    let mut lock = resolve_workers(&combined, requested, workers)?;
    if lock.go_sumdb.is_empty() {
        lock.go_sumdb = prior_evidence;
    }
    lock.encode()?;
    Ok(lock)
}

struct ProjectResolver<'a> {
    fixtures: &'a FixtureRegistry,
    registry: &'a RegistryResolver<'a>,
}

impl LibResolver for ProjectResolver<'_> {
    fn manifest(&self) -> ProviderManifest {
        self.registry.manifest()
    }

    fn resolve(&self, requested: &[LibSpec]) -> Result<LockFile, String> {
        let (fixture, real): (Vec<_>, Vec<_>) = requested
            .iter()
            .cloned()
            .partition(|spec| self.fixtures.package(spec).is_some());
        let mut lock = self.registry.resolve(&real)?;
        lock.libs.extend(self.fixtures.resolve(&fixture)?.libs);
        normalize_libs(&mut lock.libs)?;
        Ok(lock)
    }
}

struct InstalledRegistry {
    layout: crate::toolchain::Layout,
    version: crate::toolchain::Version,
    resources: crate::toolchain::SignedResources,
    target: crate::toolchain::BuildTarget,
}

/// Only explicit dependency preparation constructs this capability. Offline
/// readers retain InstalledRegistry and cannot request an installation.
struct PreparingRegistry(InstalledRegistry);

impl PreparingRegistry {
    fn ensure(&self, kind: crate::toolchain::ExtensionKind) -> Result<(), String> {
        crate::toolchain::ensure_extension(&self.0.layout, &self.0.version, kind, self.0.target)
    }
}

impl build::Inputs for PreparingRegistry {
    fn runtime(&self, ecosystem: Ecosystem) -> Result<(RegistryRuntime, Vec<u8>), String> {
        self.ensure(crate::toolchain::ExtensionKind::Runtime(ecosystem.clone()))?;
        self.0.runtime(ecosystem)
    }
    fn tools(&self, ecosystem: Ecosystem) -> Result<(build::pack::Descriptor, Vec<u8>), String> {
        self.ensure(crate::toolchain::ExtensionKind::Build(ecosystem.clone()))?;
        build::Inputs::tools(&self.0, ecosystem)
    }
    fn assets(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        build::Inputs::assets(&self.0)
    }
    fn diagnostic(&self, output: &[u8]) {
        build::Inputs::diagnostic(&self.0, output);
    }
}

impl build::Inputs for InstalledRegistry {
    fn diagnostic(&self, output: &[u8]) {
        if !output.is_empty() {
            eprintln!("{}", String::from_utf8_lossy(output));
        }
    }

    fn runtime(&self, ecosystem: Ecosystem) -> Result<(RegistryRuntime, Vec<u8>), String> {
        self.runtime(ecosystem)
    }

    fn tools(&self, ecosystem: Ecosystem) -> Result<(build::pack::Descriptor, Vec<u8>), String> {
        let directory = PathBuf::from("build")
            .join(ecosystem.as_str())
            .join(self.target.platform());
        let metadata = self.signed_artifact(&directory.join("manifest.json"))?;
        let descriptor: build::pack::Descriptor = serde_json::from_slice(&metadata)
            .map_err(|error| format!("invalid signed build descriptor: {error}"))?;
        if descriptor.ecosystem != ecosystem {
            return Err("signed build pack ecosystem differs".into());
        }
        let bytes = self.signed_artifact(&directory.join("build.pack"))?;
        Ok((descriptor, bytes))
    }

    fn assets(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.sandbox_resources(crate::toolchain::BuildTarget::host()?)?
            .into_iter()
            .map(|resource| {
                let path = resource
                    .path
                    .strip_prefix("sandbox/")
                    .ok_or("invalid sandbox resource path")?
                    .to_owned();
                Ok((path, resource.bytes))
            })
            .collect()
    }
}

impl InstalledRegistry {
    fn current(target: crate::toolchain::BuildTarget) -> Result<Self, String> {
        let executable =
            fs::canonicalize(std::env::current_exe().map_err(|error| error.to_string())?)
                .map_err(|error| format!("cannot resolve Dever core executable: {error}"))?;
        let version_root = executable
            .parent()
            .ok_or("Dever core executable has no version directory")?;
        let version = version_root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Dever core executable is not in a managed version")?;
        let version = crate::toolchain::Version::parse(version)
            .map_err(|_| "real Lib providers require a signed managed Dever release".to_owned())?;
        let versions = version_root
            .parent()
            .ok_or("Dever version directory has no parent")?;
        if versions.file_name().and_then(|name| name.to_str()) != Some("versions") {
            return Err("real Lib providers require a signed managed Dever release".into());
        }
        let layout = crate::toolchain::Layout::new(
            versions.parent().ok_or("Dever machine root is missing")?,
        );
        layout.validate_machine_permissions()?;
        let installed =
            crate::toolchain::MachineManager::new(layout.clone()).resolve_core(&version)?;
        if fs::canonicalize(installed).map_err(|error| error.to_string())? != executable {
            return Err("running Dever core is not the verified managed release".into());
        }
        Ok(Self {
            resources: crate::toolchain::SignedResources::load(&layout, &version)?,
            layout,
            version,
            target,
        })
    }

    fn runtime(&self, ecosystem: Ecosystem) -> Result<(RegistryRuntime, Vec<u8>), String> {
        let directory = PathBuf::from("runtime")
            .join(ecosystem.as_str())
            .join(self.target.platform());
        let manifest_bytes = self.signed_artifact(&directory.join("manifest.json"))?;
        let manifest: registry::InstalledRegistryPack = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| {
                format!(
                    "invalid signed {} runtime manifest: {error}",
                    ecosystem.as_str()
                )
            })?;
        manifest.validate(&ecosystem, self.target.platform())?;
        let bytes = self.signed_artifact(&directory.join("runtime.pack"))?;
        if bytes.is_empty()
            || bytes.len() > 256 * 1024 * 1024
            || sha256(&bytes) != manifest.runtime.pack.sha256
        {
            return Err(format!(
                "signed {} runtime pack digest or size is invalid",
                ecosystem.as_str()
            ));
        }
        Ok((manifest.runtime, bytes))
    }

    fn signed_artifact(&self, relative: &Path) -> Result<Vec<u8>, String> {
        let name = relative.to_str().ok_or("runtime path is not UTF-8")?;
        self.resources.read(name)
    }

    fn sandbox_resources(
        &self,
        target: crate::toolchain::BuildTarget,
    ) -> Result<Vec<dever_core::native::EmbeddedResource>, String> {
        let prefix = format!("sandbox/{}/", target.platform());
        let mut resources = Vec::new();
        let mut files = Vec::new();
        for artifact in self.resources.artifacts(&prefix)? {
            let Some(relative) = artifact.path.strip_prefix(&prefix) else {
                continue;
            };
            let bytes = self.signed_artifact(Path::new(&artifact.path))?;
            files.push((relative.to_owned(), bytes.clone()));
            resources.push(dever_core::native::EmbeddedResource {
                path: format!("sandbox/{relative}"),
                sha256: artifact.sha256.clone(),
                bytes,
                executable: dever_sandbox::asset_executable(relative),
            });
        }
        dever_sandbox::validate_assets_for_target(&files, target.platform())?;
        Ok(resources)
    }
}

pub(crate) struct ManagedArtifactStore {
    layout: crate::toolchain::Layout,
}

pub(crate) fn managed_artifact_store() -> Result<ManagedArtifactStore, String> {
    Ok(ManagedArtifactStore {
        layout: InstalledRegistry::current(crate::toolchain::BuildTarget::host()?)?.layout,
    })
}

impl ArtifactStore for ManagedArtifactStore {
    fn has(&self, _sha256: &str, _target: &str) -> Result<bool, String> {
        Err("managed artifact lookup requires the locked byte length".into())
    }

    fn publish(&self, bytes: &[u8], _target: &str) -> Result<String, String> {
        let receipt = crate::toolchain::artifact_put(&self.layout, bytes)?;
        Ok(receipt.sha256)
    }

    fn get(&self, _sha256: &str, _target: &str) -> Result<Option<Vec<u8>>, String> {
        Err("managed artifact lookup requires the locked byte length".into())
    }

    fn get_exact(
        &self,
        sha256: &str,
        bytes: u64,
        _target: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        crate::toolchain::artifact_get_optional(&self.layout, sha256, bytes)
    }
}

pub trait ArtifactStore {
    fn has(&self, sha256: &str, target: &str) -> Result<bool, String>;
    fn publish(&self, bytes: &[u8], target: &str) -> Result<String, String>;
    fn get(&self, sha256: &str, target: &str) -> Result<Option<Vec<u8>>, String>;

    fn get_exact(
        &self,
        sha256: &str,
        _bytes: u64,
        target: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        self.get(sha256, target)
    }

    fn verify_exact(
        &self,
        sha256: &str,
        expected_bytes: u64,
        target: &str,
    ) -> Result<Vec<u8>, String> {
        let bytes = self
            .get_exact(sha256, expected_bytes, target)?
            .ok_or_else(|| format!("artifact {sha256} for target {target} is missing"))?;
        if bytes.len() as u64 != expected_bytes {
            return Err(format!(
                "artifact {sha256} for target {target} has size drift"
            ));
        }
        if sha256_bytes(&bytes) != sha256 {
            return Err(format!(
                "artifact {sha256} for target {target} failed SHA-256 verification"
            ));
        }
        Ok(bytes)
    }

    fn verify(&self, sha256: &str, target: &str) -> Result<Vec<u8>, String> {
        let bytes = self
            .get(sha256, target)?
            .ok_or_else(|| format!("artifact {sha256} for target {target} is missing"))?;
        if sha256_bytes(&bytes) != sha256 {
            return Err(format!(
                "artifact {sha256} for target {target} failed SHA-256 verification"
            ));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedArtifact {
    pub lib: LibSpec,
    pub target: String,
    pub path: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// Verify the exact immutable bytes selected by a lock file.  The caller
/// supplies the authenticated machine-cache implementation; no registry,
/// package manager, PATH lookup, or environment variable is consulted here.
pub fn verify_locked_artifacts(
    lock: &LockFile,
    store: &dyn ArtifactStore,
    target: &str,
) -> Result<Vec<VerifiedArtifact>, String> {
    verify_locked_artifacts_with_go_verifier(lock, store, target, &sumdb::Verifier::official())
}

pub fn verify_locked_artifacts_with_go_verifier(
    lock: &LockFile,
    store: &dyn ArtifactStore,
    target: &str,
    verifier: &sumdb::Verifier,
) -> Result<Vec<VerifiedArtifact>, String> {
    doctor_with_go_verifier(lock, verifier)?;
    let go = sumdb::verify_chain(&lock.go_sumdb, verifier)?;
    let mut verified = Vec::new();
    for lib in &lock.libs {
        for artifact in &lib.artifacts {
            if artifact.target != target && artifact.target != "host" {
                return Err(format!(
                    "locked artifact for {} targets {}, not {target}",
                    lib.spec.key(),
                    artifact.target
                ));
            }
            let bytes = store.verify_exact(&artifact.sha256, artifact.bytes, &artifact.target)?;
            if lib.spec.ecosystem == Ecosystem::Go && go.contains(&lib.spec.name, &lib.spec.version)
            {
                go.verify_zip(&lib.spec.name, &lib.spec.version, &bytes)?;
            }
            verified.push(VerifiedArtifact {
                lib: lib.spec.clone(),
                target: artifact.target.clone(),
                path: artifact.path.clone(),
                bytes,
                sha256: artifact.sha256.clone(),
            });
        }
    }
    for receipt in build::npm_outputs(lock) {
        let artifact = &receipt.output;
        if artifact.target != target && artifact.target != "host" {
            return Err(format!(
                "locked npm build output targets {}, not {target}",
                artifact.target
            ));
        }
        verified.push(VerifiedArtifact {
            lib: receipt
                .roots
                .first()
                .ok_or("npm build roots missing")?
                .clone(),
            target: artifact.target.clone(),
            path: artifact.path.clone(),
            bytes: store.verify_exact(&artifact.sha256, artifact.bytes, &artifact.target)?,
            sha256: artifact.sha256.clone(),
        });
    }
    verified.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.sha256.cmp(&right.sha256))
    });
    Ok(verified)
}

#[derive(Default)]
pub struct FixtureArtifactStore {
    entries: std::sync::Mutex<BTreeMap<(String, String), Vec<u8>>>,
}

impl ArtifactStore for FixtureArtifactStore {
    fn has(&self, sha256: &str, target: &str) -> Result<bool, String> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| "fixture cache poisoned")?
            .contains_key(&(sha256.into(), target.into())))
    }
    fn publish(&self, bytes: &[u8], target: &str) -> Result<String, String> {
        let digest = sha256(bytes);
        self.entries
            .lock()
            .map_err(|_| "fixture cache poisoned")?
            .insert((digest.clone(), target.into()), bytes.to_vec());
        Ok(digest)
    }

    fn get(&self, sha256: &str, target: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| "fixture cache poisoned")?
            .get(&(sha256.into(), target.into()))
            .cloned())
    }
}

/// Metadata-only SDK fixture used by offline tests.  Actual language SDKs are
/// Worker binaries and use the frozen Component Protocol; this type only checks
/// that their generated schema identity is the one recorded in `dever.lock`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdkFixture {
    pub ecosystem: Ecosystem,
    pub schema: String,
}

/// Offline preparation result consumed by `run` and `build`.  Resolution is
/// deliberately not performed here: project commands only accept a canonical
/// lock produced by an explicit `dever lib add/update`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PreparationReport {
    pub libs: usize,
    pub artifacts: usize,
    pub artifact_bytes: u64,
    pub runtimes: usize,
}

pub fn write_report(output: &Path, report: &PreparationReport) -> Result<(), String> {
    let mut path = output.to_path_buf();
    let name = output
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("build output must have a file name")?;
    path.set_file_name(format!("{name}.external.json"));
    let mut bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if path.exists()
        && fs::symlink_metadata(&path)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err(format!(
            "external build report '{}' must not be a symbolic link",
            path.display()
        ));
    }
    let temporary = path.with_file_name(format!(
        ".{}.{}-{}.tmp",
        name,
        std::process::id(),
        unique_suffix()
    ));
    write_new_file(&temporary, &bytes).map_err(|error| {
        format!(
            "cannot stage external build report '{}': {error}",
            path.display()
        )
    })?;
    fs::rename(&temporary, &path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!(
            "cannot publish external build report '{}': {error}",
            path.display()
        )
    })
}

impl PreparationReport {
    pub fn summary(&self) -> String {
        format!(
            "external libs: {} lib(s), {} artifact(s), {} runtime pack(s), {} artifact bytes",
            self.libs, self.artifacts, self.runtimes, self.artifact_bytes,
        )
    }
}

/// Validate the project declarations and lock without contacting a registry,
/// reading PATH, or consulting an environment variable.  Missing external
/// artifacts are reported by the authenticated machine cache owner in the
/// packaging build; this stage only proves that the project contract is
/// deterministic and complete.
pub fn prepare(
    project_root: &Path,
    source_requests: &[String],
) -> Result<Option<PreparationReport>, String> {
    let lock_path = project_root.join("dever.lock");
    let mut declarations = read_declarations(project_root)?;
    declarations.extend(crate::packages::declared_libs(project_root)?);
    if declarations.is_empty() && !lock_path.exists() && source_requests.is_empty() {
        return Ok(None);
    }
    if !lock_path.exists() {
        return Err(format!(
            "external libraries are declared but '{}' is missing; run 'dever lib add {} <pip|npm|go>:<name>@<version>'",
            lock_path.display(),
            project_root.display(),
        ));
    }
    let lock = read_lock(&lock_path)?;
    doctor(&lock)?;
    declarations.extend(
        source_requests
            .iter()
            .map(|request| request.parse())
            .collect::<Result<Vec<LibSpec>, _>>()?,
    );
    declarations.sort();
    declarations.dedup();
    for requested in declarations {
        if !lock.libs.iter().any(|lib| lib.spec == requested) {
            return Err(format!(
                "dever.lock does not contain declared {}; run 'dever lib update {}'",
                requested.key(),
                project_root.display(),
            ));
        }
    }
    let artifacts = lock
        .libs
        .iter()
        .flat_map(|lib| &lib.artifacts)
        .chain(build::npm_outputs(&lock).map(|receipt| &receipt.output))
        .collect::<BTreeSet<_>>();
    let artifact_bytes = artifacts.iter().map(|artifact| artifact.bytes).sum();
    let runtimes = lock
        .libs
        .iter()
        .map(|lib| (&lib.runtime.name, &lib.runtime.version))
        .chain(lock.workers.iter().filter_map(|worker| {
            worker
                .runtime
                .as_ref()
                .map(|runtime| (&runtime.name, &runtime.version))
        }))
        .collect::<BTreeSet<_>>()
        .len();
    Ok(Some(PreparationReport {
        libs: lock.libs.len(),
        artifacts: artifacts.len(),
        artifact_bytes,
        runtimes,
    }))
}

pub fn prepare_program(
    project_root: &Path,
    program: &dever_core::hir::Program,
    target: crate::toolchain::BuildTarget,
) -> Result<Option<PreparationReport>, String> {
    if program_workers(program)?
        .iter()
        .any(|worker| worker.ecosystem != "exec")
        && !project_root.join("dever.lock").exists()
    {
        return Err(format!(
            "managed external Worker requires dever.lock; run 'dever lib add {}'",
            project_root.display()
        ));
    }
    let report = prepare(project_root, &program.external_lib_requests())?;
    if report.is_some() {
        validate_worker_bindings(
            &read_lock(&project_root.join("dever.lock"))?,
            &bound_program_workers(program, target)?,
        )?;
    }
    Ok(report)
}

fn program_workers(program: &dever_core::hir::Program) -> Result<Vec<LockedWorker>, String> {
    program
        .external_worker_contracts()
        .into_iter()
        .filter(|worker| worker.ecosystem != "exec" || !worker.libs.is_empty())
        .map(LockedWorker::from_contract)
        .collect()
}

pub(crate) fn bound_program_workers(
    program: &dever_core::hir::Program,
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<LockedWorker>, String> {
    bind_workers(program_workers(program)?, target, false)
}

fn bind_workers(
    mut workers: Vec<LockedWorker>,
    target: crate::toolchain::BuildTarget,
    prepare: bool,
) -> Result<Vec<LockedWorker>, String> {
    if workers.iter().all(|worker| worker.ecosystem == "exec") {
        return Ok(workers);
    }
    let managed = InstalledRegistry::current(target)?;
    for worker in &mut workers {
        if worker.ecosystem != "exec" {
            if prepare {
                crate::toolchain::ensure_extension(
                    &managed.layout,
                    &managed.version,
                    crate::toolchain::ExtensionKind::Runtime(worker.ecosystem.parse()?),
                    target,
                )?;
            }
            worker.runtime = Some(managed.runtime(worker.ecosystem.parse()?)?.0.pack);
        }
    }
    Ok(workers)
}

fn validate_worker_bindings(lock: &LockFile, expected: &[LockedWorker]) -> Result<(), String> {
    if lock.workers != expected {
        return Err("dever.lock Worker schema, entry, capability or Lib binding changed; run 'dever lib update <project-root>'".into());
    }
    Ok(())
}

/// Materialize locked bytes into the compiler-owned resource table. Released
/// providers read the authenticated machine cache and signed runtime pack.
pub fn embedded_resources(
    project_root: &Path,
) -> Result<Vec<dever_core::native::EmbeddedResource>, String> {
    embedded_resources_for_target(project_root, crate::toolchain::BuildTarget::host()?)
}

pub fn embedded_resources_for_target(
    project_root: &Path,
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<dever_core::native::EmbeddedResource>, String> {
    let lock_path = project_root.join("dever.lock");
    let lock = read_lock(&lock_path)?;
    doctor(&lock)?;
    let registry = FixtureRegistry::builtin();
    let mut resources = Vec::new();
    let go = sumdb::verify_chain(&lock.go_sumdb, &sumdb::Verifier::official())?;
    let mut managed = None;
    let mut runtimes = BTreeMap::new();
    for lib in &lock.libs {
        if let Some(package) = registry.package(&lib.spec) {
            if &package.locked() != lib {
                return Err(format!(
                    "locked metadata for {} changed; run 'dever lib update {}'",
                    lib.spec.key(),
                    project_root.display()
                ));
            }
            for artifact in &package.artifacts {
                resources.push(dever_core::native::EmbeddedResource {
                    path: artifact.path.clone(),
                    bytes: artifact.bytes.clone(),
                    sha256: sha256(&artifact.bytes),
                    executable: artifact.path.ends_with(".bin")
                        || artifact.path.ends_with("/worker"),
                });
            }
            continue;
        }
        let installed = match &managed {
            Some(installed) => installed,
            None => managed.insert(InstalledRegistry::current(target)?),
        };
        if let Some(runtime) = runtimes.get(&lib.spec.ecosystem) {
            if runtime != &lib.runtime {
                return Err(format!(
                    "locked runtime pack for {} differs from the signed release",
                    lib.spec.key()
                ));
            }
        } else {
            let (runtime, bytes) = installed.runtime(lib.spec.ecosystem.clone())?;
            if runtime.pack != lib.runtime {
                return Err(format!(
                    "locked runtime pack for {} differs from the signed release",
                    lib.spec.key()
                ));
            }
            runtimes.insert(lib.spec.ecosystem.clone(), runtime.pack.clone());
            resources.push(dever_core::native::EmbeddedResource {
                path: format!(
                    "lib/runtime/{}/{}/runtime.pack",
                    lib.spec.ecosystem.as_str(),
                    runtime.target
                ),
                sha256: runtime.pack.sha256,
                bytes,
                executable: false,
            });
        }
        let store = ManagedArtifactStore {
            layout: installed.layout.clone(),
        };
        for artifact in &lib.artifacts {
            if artifact.target != target.platform() {
                return Err(format!(
                    "locked artifact for {} targets {}, not the selected build target",
                    lib.spec.key(),
                    artifact.target
                ));
            }
            let bytes = store.verify_exact(&artifact.sha256, artifact.bytes, &artifact.target)?;
            if lib.spec.ecosystem == Ecosystem::Go {
                go.verify_zip(&lib.spec.name, &lib.spec.version, &bytes)?;
            }
            resources.push(dever_core::native::EmbeddedResource {
                path: artifact.path.clone(),
                bytes,
                sha256: artifact.sha256.clone(),
                executable: false,
            });
        }
    }
    for receipt in build::npm_outputs(&lock) {
        let installed = match &managed {
            Some(installed) => installed,
            None => managed.insert(InstalledRegistry::current(target)?),
        };
        let artifact = &receipt.output;
        if artifact.target != target.platform() {
            return Err("locked npm build output differs from the selected build target".into());
        }
        let store = ManagedArtifactStore {
            layout: installed.layout.clone(),
        };
        resources.push(dever_core::native::EmbeddedResource {
            path: artifact.path.clone(),
            bytes: store.verify_exact(&artifact.sha256, artifact.bytes, &artifact.target)?,
            sha256: artifact.sha256.clone(),
            executable: false,
        });
    }
    for worker in lock
        .workers
        .iter()
        .filter(|worker| worker.ecosystem != "exec")
    {
        let ecosystem: Ecosystem = worker.ecosystem.parse()?;
        let installed = match &managed {
            Some(installed) => installed,
            None => managed.insert(InstalledRegistry::current(target)?),
        };
        if !runtimes.contains_key(&ecosystem) {
            let (runtime, bytes) = installed.runtime(ecosystem.clone())?;
            if worker.runtime.as_ref() != Some(&runtime.pack) {
                return Err(format!(
                    "locked Worker '{}.{}' runtime differs from signed release",
                    worker.port, worker.adapter
                ));
            }
            runtimes.insert(ecosystem.clone(), runtime.pack.clone());
            resources.push(dever_core::native::EmbeddedResource {
                path: format!(
                    "lib/runtime/{}/{}/runtime.pack",
                    ecosystem.as_str(),
                    runtime.target
                ),
                sha256: runtime.pack.sha256,
                bytes,
                executable: false,
            });
        } else if worker.runtime.as_ref() != runtimes.get(&ecosystem) {
            return Err(format!(
                "locked Worker '{}.{}' runtime differs from signed release",
                worker.port, worker.adapter
            ));
        }
    }
    resources.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.sha256.cmp(&right.sha256))
    });
    resources.dedup_by(|left, right| left.path == right.path && left.sha256 == right.sha256);
    Ok(resources)
}

pub fn prepare_resources(
    project_root: &Path,
    program: &dever_core::hir::Program,
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<dever_core::native::EmbeddedResource>, String> {
    let mut resources =
        if project_root.join("dever.lock").exists() && program.has_external_adapters() {
            embedded_resources_for_target(project_root, target)?
        } else {
            Vec::new()
        };
    for entry in program
        .external_worker_contracts()
        .into_iter()
        .filter(|worker| worker.ecosystem == "exec")
        .map(|worker| worker.entry)
    {
        let bytes = match crate::packages::owned_worker_bytes(project_root, &entry)? {
            Some(bytes) => bytes,
            None => {
                let path = owned_worker(project_root, &entry)?;
                fs::read(&path)
                    .map_err(|error| format!("cannot read external Worker '{entry}': {error}"))?
            }
        };
        if bytes.is_empty() {
            return Err(format!("external Worker '{entry}' is empty"));
        }
        resources.push(dever_core::native::EmbeddedResource {
            path: entry,
            sha256: sha256(&bytes),
            bytes,
            executable: true,
        });
    }
    let mut host_assets = Vec::new();
    if program.has_external_adapters() {
        let installed = InstalledRegistry::current(target)?;
        resources.extend(installed.sandbox_resources(target)?);
        if program
            .external_worker_contracts()
            .iter()
            .any(|worker| worker.ecosystem == "go")
        {
            host_assets = installed.sandbox_resources(crate::toolchain::BuildTarget::host()?)?;
        }
    }
    let packaged = crate::workers::prepare_for_target(
        project_root,
        program,
        &resources,
        target,
        &host_assets,
    )?;
    let exec_entries = program
        .external_worker_contracts()
        .into_iter()
        .filter(|worker| worker.ecosystem == "exec")
        .map(|worker| worker.entry)
        .collect::<BTreeSet<_>>();
    resources.retain(|resource| !exec_entries.contains(&resource.path));
    let workers = program_workers(program)?;
    if workers.iter().any(|worker| worker.ecosystem != "exec") {
        let lock = read_lock(&project_root.join("dever.lock"))?;
        prune_expanded_resources_for_target(
            &lock,
            &workers,
            &read_declarations(project_root)?,
            &mut resources,
            target,
        )?;
    }
    resources.extend(packaged);
    resources.sort_by(|left, right| left.path.cmp(&right.path));
    for pair in resources.windows(2) {
        if pair[0].path == pair[1].path
            && (pair[0].sha256 != pair[1].sha256 || pair[0].executable != pair[1].executable)
        {
            return Err(format!(
                "conflicting external resources for '{}'",
                pair[0].path
            ));
        }
    }
    resources.dedup_by(|left, right| left.path == right.path);
    Ok(resources)
}

/// The managed packager has already expanded these archives into verified
/// execution trees. Keep raw bytes if a declared Lib or exec Worker still uses
/// the same locked dependency closure.
pub fn prune_expanded_resources(
    lock: &LockFile,
    workers: &[LockedWorker],
    declared: &[LibSpec],
    resources: &mut Vec<dever_core::native::EmbeddedResource>,
) -> Result<(), String> {
    prune_expanded_resources_for_target(
        lock,
        workers,
        declared,
        resources,
        crate::toolchain::BuildTarget::host()?,
    )
}

pub fn prune_expanded_resources_for_target(
    lock: &LockFile,
    workers: &[LockedWorker],
    declared: &[LibSpec],
    resources: &mut Vec<dever_core::native::EmbeddedResource>,
    target: crate::toolchain::BuildTarget,
) -> Result<(), String> {
    let by_spec = lock
        .libs
        .iter()
        .map(|lib| (&lib.spec, lib))
        .collect::<BTreeMap<_, _>>();
    let closure = |roots: Vec<LibSpec>| -> Result<BTreeSet<LibSpec>, String> {
        let mut selected = BTreeSet::new();
        let npm_archives = npm::archive_roots(lock, &roots)?;
        let mut remaining = VecDeque::from(roots);
        remaining.extend(npm_archives);
        while let Some(spec) = remaining.pop_front() {
            if !selected.insert(spec.clone()) {
                continue;
            }
            let lib = by_spec
                .get(&spec)
                .ok_or_else(|| format!("locked Lib {} is missing", spec.key()))?;
            remaining.extend(
                lib.dependencies
                    .iter()
                    .map(|dependency| dependency.spec.clone()),
            );
        }
        Ok(selected)
    };
    let managed_workers = workers
        .iter()
        .filter(|worker| worker.ecosystem != "exec")
        .collect::<Vec<_>>();
    let npm_output = |roots: &[LibSpec]| -> Result<Option<&str>, String> {
        let roots = npm::roots(roots);
        let Some(environment) = lock
            .npm
            .iter()
            .find(|environment| environment.roots == roots)
        else {
            return Ok(None);
        };
        build::npm_receipt(lock, environment)
            .map(|receipt| receipt.map(|receipt| receipt.output.path.as_str()))
    };
    let mut managed = BTreeSet::new();
    let mut expanded_outputs = BTreeSet::new();
    for worker in &managed_workers {
        managed.extend(closure(worker.libs.clone())?);
        expanded_outputs.extend(npm_output(&worker.libs)?);
    }
    let mut retained = closure(declared.to_vec())?;
    let mut retained_outputs = BTreeSet::new();
    retained_outputs.extend(npm_output(declared)?);
    for worker in workers.iter().filter(|worker| worker.ecosystem == "exec") {
        retained.extend(closure(worker.libs.clone())?);
        retained_outputs.extend(npm_output(&worker.libs)?);
    }
    let managed_ecosystems = managed_workers
        .iter()
        .map(|worker| worker.ecosystem.as_str())
        .collect::<BTreeSet<_>>();
    let retained_ecosystems = retained
        .iter()
        .map(|spec| spec.ecosystem.as_str())
        .collect::<BTreeSet<_>>();
    let retained_artifacts = lock
        .libs
        .iter()
        .filter(|lib| retained.contains(&lib.spec))
        .flat_map(|lib| lib.artifacts.iter().map(|artifact| artifact.path.as_str()))
        .chain(retained_outputs)
        .collect::<BTreeSet<_>>();
    let expanded_artifacts = lock
        .libs
        .iter()
        .filter(|lib| managed.contains(&lib.spec) && !retained.contains(&lib.spec))
        .flat_map(|lib| lib.artifacts.iter().map(|artifact| artifact.path.as_str()))
        .chain(expanded_outputs)
        .filter(|path| !retained_artifacts.contains(path))
        .collect::<BTreeSet<_>>();
    let target = target.platform();
    let expanded_runtimes = managed_ecosystems
        .difference(&retained_ecosystems)
        .map(|ecosystem| format!("lib/runtime/{ecosystem}/{target}/runtime.pack"))
        .collect::<BTreeSet<_>>();
    resources.retain(|resource| {
        !expanded_artifacts.contains(resource.path.as_str())
            && !expanded_runtimes.contains(&resource.path)
    });
    Ok(())
}

pub(crate) fn owned_worker(project_root: &Path, entry: &str) -> Result<PathBuf, String> {
    let relative = Path::new(entry);
    if entry.contains(['\\', ':'])
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("external Worker entry must be a portable relative path".into());
    }
    let mut path = project_root.to_path_buf();
    if fs::symlink_metadata(&path)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("external Worker project root must not be a symbolic link".into());
    }
    for part in relative.components() {
        path.push(part);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect external Worker '{entry}': {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("external Worker entry crosses a symbolic link".into());
        }
    }
    if !fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("external Worker entry must be a regular file".into());
    }
    Ok(path)
}

impl SdkFixture {
    pub fn handshake(&self, expected_schema: &str) -> Result<(), String> {
        if self.schema != expected_schema {
            return Err("SDK fixture schema identity drift".into());
        }
        Ok(())
    }
}

pub fn doctor(lock: &LockFile) -> Result<(), String> {
    doctor_with_go_verifier(lock, &sumdb::Verifier::official())
}

pub fn doctor_with_go_verifier(lock: &LockFile, verifier: &sumdb::Verifier) -> Result<(), String> {
    lock.encode()?;
    crate::packages::doctor_lock(&lock.packages)?;
    npm::doctor(lock)?;
    build::doctor(lock)?;
    let go = sumdb::verify_chain(&lock.go_sumdb, verifier)?;
    let fixtures = FixtureRegistry::builtin();
    let mut workers = lock.workers.clone();
    normalize_workers(&mut workers)?;
    let mut seen = BTreeSet::new();
    let mut distributions = BTreeMap::<(&str, &str), &LockedLib>::new();
    let locked = lock
        .libs
        .iter()
        .map(|lib| lib.spec.clone())
        .collect::<BTreeSet<_>>();
    for lib in &lock.libs {
        if lib.spec.ecosystem == Ecosystem::Npm
            && (!lib.dependencies.is_empty()
                || !lock.npm.iter().any(|environment| {
                    environment
                        .instances
                        .iter()
                        .any(|node| node.source.archive == lib.spec)
                }))
            && !fixtures
                .package(&lib.spec)
                .is_some_and(|package| package.locked() == *lib)
        {
            return Err(
                "npm archive requires an exact instance graph, not flat dependencies".into(),
            );
        }
        if lib.spec.ecosystem == Ecosystem::Go
            && !go.contains(&lib.spec.name, &lib.spec.version)
            && !fixtures
                .package(&lib.spec)
                .is_some_and(|package| package.locked() == *lib)
        {
            return Err(format!(
                "{} lacks authenticated Go sumdb evidence",
                lib.spec.key()
            ));
        }
        validate_name(&lib.spec.ecosystem, &lib.spec.name)?;
        validate_version(&lib.spec.ecosystem, &lib.spec.version)?;
        if !seen.insert(lib.spec.key()) {
            return Err(format!("duplicate locked lib {}", lib.spec.key()));
        }
        if lib.schema.is_empty() {
            return Err(format!(
                "locked lib {} has empty adapter schema identity",
                lib.spec.key()
            ));
        }
        validate_runtime(&lib.runtime)?;
        if lib.spec.ecosystem == Ecosystem::Pip {
            let key = (lib.spec.distribution_name(), lib.spec.version.as_str());
            if let Some(previous) = distributions.insert(key, lib)
                && (previous.artifacts != lib.artifacts
                    || previous.runtime != lib.runtime
                    || previous.schema != lib.schema
                    || previous.build != lib.build)
            {
                return Err(format!(
                    "Python extra variants have inconsistent distribution artifacts for {}@{}",
                    key.0, key.1
                ));
            }
        }
        for artifact in &lib.artifacts {
            validate_artifact(artifact)?;
        }
        for dependency in &lib.dependencies {
            if !locked.contains(&dependency.spec) {
                return Err(format!(
                    "locked lib {} depends on missing {}",
                    lib.spec.key(),
                    dependency.spec.key()
                ));
            }
            if dependency.spec == lib.spec {
                return Err(format!("locked lib {} depends on itself", lib.spec.key()));
            }
        }
    }
    validate_dependency_cycles(&lock.libs)?;
    for worker in &workers {
        let mut selected = BTreeMap::new();
        let mut visited = BTreeSet::new();
        let mut remaining = worker.libs.iter().cloned().collect::<VecDeque<_>>();
        while let Some(spec) = remaining.pop_front() {
            if spec.ecosystem == Ecosystem::Npm {
                continue;
            }
            if !visited.insert(spec.clone()) {
                continue;
            }
            let key = (spec.ecosystem.clone(), spec.distribution_name().to_owned());
            if let Some(version) = selected.get(&key)
                && version != &spec.version
            {
                return Err(format!(
                    "Worker '{}.{}' has conflicting versions of {}:{}",
                    worker.port,
                    worker.adapter,
                    spec.ecosystem.as_str(),
                    spec.name
                ));
            }
            selected.insert(key, spec.version.clone());
            let lib = lock
                .libs
                .iter()
                .find(|lib| lib.spec == spec)
                .ok_or_else(|| {
                    format!(
                        "Worker '{}.{}' references missing {}",
                        worker.port,
                        worker.adapter,
                        spec.key()
                    )
                })?;
            remaining.extend(
                lib.dependencies
                    .iter()
                    .map(|dependency| dependency.spec.clone()),
            );
        }
    }
    Ok(())
}

fn validate_dependency_cycles(libs: &[LockedLib]) -> Result<(), String> {
    // Find strongly connected components of the exact request graph. Collapsing
    // all variants first would invent cycles between independent Workers.
    let mut graph = BTreeMap::<&LibSpec, Vec<&LibSpec>>::new();
    let mut reverse = BTreeMap::<&LibSpec, Vec<&LibSpec>>::new();
    for lib in libs {
        let dependencies = lib
            .dependencies
            .iter()
            .map(|dependency| &dependency.spec)
            .collect::<Vec<_>>();
        for dependency in &dependencies {
            reverse.entry(dependency).or_default().push(&lib.spec);
        }
        graph.insert(&lib.spec, dependencies);
    }
    fn visit<'a>(
        spec: &'a LibSpec,
        graph: &BTreeMap<&'a LibSpec, Vec<&'a LibSpec>>,
        visited: &mut BTreeSet<&'a LibSpec>,
        order: &mut Vec<&'a LibSpec>,
    ) {
        if !visited.insert(spec) {
            return;
        }
        for dependency in graph.get(spec).into_iter().flatten() {
            visit(dependency, graph, visited, order);
        }
        order.push(spec);
    }
    let mut order = Vec::new();
    let mut visited = BTreeSet::new();
    for spec in graph.keys() {
        visit(spec, &graph, &mut visited, &mut order);
    }
    visited.clear();
    for root in order.into_iter().rev() {
        let mut component = Vec::new();
        let mut remaining = VecDeque::from([root]);
        while let Some(spec) = remaining.pop_front() {
            if !visited.insert(spec) {
                continue;
            }
            component.push(spec);
            remaining.extend(reverse.get(spec).into_iter().flatten().copied());
        }
        if component.len() > 1
            && !component.iter().all(|spec| {
                spec.ecosystem == Ecosystem::Pip
                    && root.ecosystem == Ecosystem::Pip
                    && spec.distribution_name() == root.distribution_name()
                    && spec.version == root.version
            })
        {
            return Err(format!("cyclic locked lib dependency at {}", root.key()));
        }
    }
    Ok(())
}

pub fn execute(command: &str, project_root: &Path, specs: &[String]) -> Result<String, String> {
    execute_for_target(
        command,
        project_root,
        specs,
        crate::toolchain::BuildTarget::host()?,
    )
}

pub fn execute_for_target(
    command: &str,
    project_root: &Path,
    specs: &[String],
    target: crate::toolchain::BuildTarget,
) -> Result<String, String> {
    let _mutation = ProjectMutation::for_command(command, project_root)?;
    let path = project_root.join("dever.lock");
    match command {
        "install" => {
            if !specs.is_empty() {
                return Err(
                    "dever lib install accepts no Lib specs; it restores the existing dever.lock"
                        .into(),
                );
            }
            restore::install(project_root, target).map(|report| report.summary())
        }
        "add" => {
            let mut requested = read_declarations(project_root)?;
            let workers = prepare_source_workers_with_packages(
                project_root,
                &crate::packages::sources(project_root)?,
                target,
            )?;
            let package_libs = crate::packages::declared_libs(project_root)?;
            if specs.is_empty()
                && requested.is_empty()
                && workers.is_empty()
                && package_libs.is_empty()
            {
                return Err(
                    "dever lib add requires an external Adapter lib or at least one exact lib spec"
                        .into(),
                );
            }
            requested.extend(
                specs
                    .iter()
                    .map(|value| value.parse())
                    .collect::<Result<Vec<LibSpec>, _>>()?,
            );
            requested.sort();
            requested.dedup();
            let mut all = requested.clone();
            all.extend(package_libs);
            let mut lock = resolve_project(project_root, &all, workers, target)?;
            lock.packages = locked_packages(project_root)?;
            lock.encode()?;
            write_declarations(project_root, &requested)?;
            lock.write_atomic(project_root)?;
            Ok(format!(
                "resolved {} lib(s) into {}",
                lock.libs.len(),
                path.display()
            ))
        }
        "update" => {
            let requested = if specs.is_empty() {
                read_declarations(project_root)?
            } else {
                specs
                    .iter()
                    .map(|value| value.parse())
                    .collect::<Result<Vec<LibSpec>, _>>()?
            };
            let workers = prepare_source_workers_with_packages(
                project_root,
                &crate::packages::sources(project_root)?,
                target,
            )?;
            let package_libs = crate::packages::declared_libs(project_root)?;
            if requested.is_empty() && workers.is_empty() && package_libs.is_empty() {
                return Err("dever lib update requires declared libs or exact lib specs".into());
            }
            let mut all = requested.clone();
            all.extend(package_libs);
            let mut lock = resolve_project(project_root, &all, workers, target)?;
            lock.packages = locked_packages(project_root)?;
            lock.encode()?;
            if !specs.is_empty() {
                write_declarations(project_root, &requested)?;
            }
            lock.write_atomic(project_root)?;
            Ok(format!(
                "updated {} lib(s) in {}",
                lock.libs.len(),
                path.display()
            ))
        }
        "list" => {
            let lock = read_lock(&path)?;
            Ok(lock
                .libs
                .iter()
                .map(|lib| lib.spec.key())
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "doctor" => {
            let lock = read_lock(&path)?;
            doctor(&lock)?;
            validate_worker_bindings(&lock, &source_workers(project_root, target)?)?;
            prepare(
                project_root,
                &lock
                    .workers
                    .iter()
                    .flat_map(|worker| worker.libs.iter().map(LibSpec::key))
                    .collect::<Vec<_>>(),
            )?;
            embedded_resources_for_target(project_root, target)?;
            Ok(format!("{} is valid", path.display()))
        }
        "remove" => {
            if specs.is_empty() {
                return Err("dever lib remove requires at least one exact lib spec".into());
            }
            let removals = specs
                .iter()
                .map(|value| value.parse::<LibSpec>())
                .collect::<Result<BTreeSet<_>, _>>()?;
            let package_libs = crate::packages::declared_libs(project_root)?;
            if removals.iter().any(|spec| package_libs.contains(spec)) {
                return Err(
                    "cannot remove a Lib declared by a Package; remove the Package first".into(),
                );
            }
            let previous_lock = if path.exists() {
                Some(read_lock(&path)?)
            } else {
                None
            };
            let mut requested = read_declarations(project_root)?;
            requested.retain(|spec| !removals.contains(spec));
            let workers = source_workers(project_root, target)?;
            if workers
                .iter()
                .any(|worker| worker.libs.iter().any(|spec| removals.contains(spec)))
            {
                return Err("cannot remove a Lib declared by an external Adapter; remove the source declaration first".into());
            }
            if let Some(lock) = &previous_lock {
                let mut roots = requested.clone();
                roots.extend(package_libs.iter().cloned());
                let mut groups = vec![roots];
                groups.extend(workers.iter().map(|worker| worker.libs.clone()));
                // An orphan cycle is removed together. Only consumers reachable
                // from the remaining project/Worker roots can block removal.
                for environment in npm::retain(lock, &groups)? {
                    for node in environment
                        .instances
                        .iter()
                        .filter(|node| !removals.contains(&node.spec))
                    {
                        for target in node.edges.iter().filter_map(|edge| edge.target.as_ref()) {
                            if let Some(required) = environment
                                .instances
                                .iter()
                                .find(|node| &node.path == target && removals.contains(&node.spec))
                            {
                                return Err(format!(
                                    "cannot remove {}: it is required by {}",
                                    required.spec.key(),
                                    node.spec.key()
                                ));
                            }
                        }
                    }
                }
                for lib in &lock.libs {
                    if !removals.contains(&lib.spec)
                        && lib
                            .dependencies
                            .iter()
                            .any(|dependency| removals.contains(&dependency.spec))
                    {
                        let dependency = lib
                            .dependencies
                            .iter()
                            .find(|dependency| removals.contains(&dependency.spec))
                            .unwrap();
                        return Err(format!(
                            "cannot remove {}: it is required by {}",
                            dependency.spec.key(),
                            lib.spec.key()
                        ));
                    }
                }
            }
            if requested.is_empty()
                && workers.is_empty()
                && package_libs.is_empty()
                && locked_packages(project_root)?.is_empty()
                && previous_lock
                    .as_ref()
                    .is_none_or(|lock| lock.go_sumdb.is_empty())
            {
                write_declarations(project_root, &requested)?;
                if path.exists() {
                    fs::remove_file(&path)
                        .map_err(|e| format!("cannot remove '{}': {e}", path.display()))?;
                }
            } else {
                let old = previous_lock
                    .ok_or("dever.lock is required to remove a Lib without registry access")?;
                let mut all = requested.clone();
                all.extend(package_libs);
                let lock = retain_locked_libs(&old, &all, workers)?;
                lock.encode()?;
                write_declarations(project_root, &requested)?;
                lock.write_atomic(project_root)?;
            }
            Ok(format!("removed {} lib(s)", removals.len()))
        }
        _ => Err("Usage: dever lib add|list|update|remove|doctor <project-root> [spec ...]".into()),
    }
}

pub(crate) fn retain_locked_libs(
    old: &LockFile,
    requested: &[LibSpec],
    workers: Vec<LockedWorker>,
) -> Result<LockFile, String> {
    let mut groups = vec![requested.to_vec()];
    groups.extend(workers.iter().map(|worker| worker.libs.clone()));
    let npm = npm::retain(old, &groups)?;
    let by_spec = old
        .libs
        .iter()
        .map(|lib| (lib.spec.clone(), lib))
        .collect::<BTreeMap<_, _>>();
    let mut queue = requested
        .iter()
        .cloned()
        .chain(
            workers
                .iter()
                .flat_map(|worker| worker.libs.iter().cloned()),
        )
        .collect::<VecDeque<_>>();
    queue.extend(npm.iter().flat_map(|environment| {
        environment
            .instances
            .iter()
            .map(|node| node.source.archive.clone())
    }));
    let mut retained = BTreeMap::new();
    while let Some(spec) = queue.pop_front() {
        if retained.contains_key(&spec) {
            continue;
        }
        let lib = by_spec.get(&spec).ok_or_else(|| {
            format!(
                "dever.lock lacks {}; run 'dever lib update <project-root>'",
                spec.key()
            )
        })?;
        queue.extend(
            lib.dependencies
                .iter()
                .map(|dependency| dependency.spec.clone()),
        );
        retained.insert(spec, (*lib).clone());
    }
    let mut lock = LockFile::new(retained.into_values().collect())?;
    lock.workers = workers;
    lock.packages = old.packages.clone();
    lock.go_sumdb = old.go_sumdb.clone();
    lock.npm = npm;
    let retained_builds = lock
        .libs
        .iter()
        .filter_map(|lib| lib.build.as_deref())
        .chain(
            lock.npm
                .iter()
                .filter_map(|environment| environment.build.as_deref()),
        )
        .collect::<BTreeSet<_>>();
    lock.builds = old
        .builds
        .iter()
        .filter_map(|receipt| match receipt.identity() {
            Ok(identity) if retained_builds.contains(identity.as_str()) => {
                Some(Ok(receipt.clone()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, String>>()?;
    doctor(&lock)?;
    Ok(lock)
}

fn locked_packages(project_root: &Path) -> Result<Vec<crate::packages::LockedPackage>, String> {
    crate::packages::owned_files(project_root)?;
    let path = project_root.join("dever.lock");
    if path.exists() {
        Ok(read_lock(&path)?.packages)
    } else {
        Ok(Vec::new())
    }
}

pub(crate) fn read_lock(path: &Path) -> Result<LockFile, String> {
    LockFile::decode(&fs::read(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?)
}

fn source_workers(
    project_root: &Path,
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<LockedWorker>, String> {
    source_workers_with_packages(
        project_root,
        &crate::packages::sources(project_root)?,
        target,
    )
}

pub(crate) fn source_workers_with_packages(
    project_root: &Path,
    packages: &[dever_core::source::PackageSource],
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<LockedWorker>, String> {
    bind_workers(
        unbound_source_workers(project_root, packages)?,
        target,
        false,
    )
}

pub(crate) fn prepare_source_workers_with_packages(
    project_root: &Path,
    packages: &[dever_core::source::PackageSource],
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<LockedWorker>, String> {
    bind_workers(
        unbound_source_workers(project_root, packages)?,
        target,
        true,
    )
}

fn unbound_source_workers(
    project_root: &Path,
    packages: &[dever_core::source::PackageSource],
) -> Result<Vec<LockedWorker>, String> {
    let source_root = project_root.join("module");
    if !source_root.exists() && packages.is_empty() {
        return Ok(Vec::new());
    }
    let sources = dever_core::source::SourceMap::load_with_packages(&source_root, None, packages)
        .map_err(|errors| {
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let program = dever_core::check(&sources).map_err(|errors| {
        errors
            .iter()
            .map(|error| error.render(&sources))
            .collect::<String>()
    })?;
    program_workers(&program)
}

fn declaration_path(project_root: &Path) -> PathBuf {
    project_root.join("config/setting.json")
}

pub(crate) fn read_declarations(project_root: &Path) -> Result<Vec<LibSpec>, String> {
    if project_root.join("config/lib.json").exists() {
        return Err(
            "config/lib.json is not supported; declare Libs in config/setting.json 'lib'".into(),
        );
    }
    let path = declaration_path(project_root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    reject_declaration_symlinks(project_root)?;
    let settings = dever_runtime::config::Settings::load_project(project_root)?;
    let mut specs = settings
        .external_libs()
        .iter()
        .map(|value| value.parse::<LibSpec>())
        .collect::<Result<Vec<_>, _>>()?;
    specs.sort();
    specs.dedup();
    for spec in &specs {
        validate_name(&spec.ecosystem, &spec.name)?;
        validate_version(&spec.ecosystem, &spec.version)?;
    }
    Ok(specs)
}

fn write_declarations(project_root: &Path, specs: &[LibSpec]) -> Result<(), String> {
    reject_declaration_symlinks(project_root)?;
    let path = declaration_path(project_root);
    let parent = path
        .parent()
        .ok_or("project root has no config directory")?;
    fs::create_dir_all(parent).map_err(|e| format!("cannot create config directory: {e}"))?;
    let mut specs = specs.to_vec();
    specs.sort();
    specs.dedup();
    let mut document = if path.exists() {
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        dever_runtime::wire::parse(&text)
            .map_err(|error| format!("invalid setting.json: {error}"))?;
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text)
            .map_err(|error| error.to_string())?
    } else {
        serde_json::Map::new()
    };
    if specs.is_empty() {
        document.remove("lib");
    } else {
        document.insert(
            "lib".into(),
            serde_json::json!(specs.iter().map(LibSpec::key).collect::<Vec<_>>()),
        );
    }
    let mut bytes = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let temporary = parent.join(format!(
        ".lib.json.{}-{}.tmp",
        std::process::id(),
        unique_suffix()
    ));
    write_new_file(&temporary, &bytes)
        .map_err(|e| format!("cannot stage '{}': {e}", path.display()))?;
    fs::rename(&temporary, &path).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        format!("cannot replace '{}': {e}", path.display())
    })
}

pub(crate) fn reject_declaration_symlinks(project_root: &Path) -> Result<(), String> {
    for path in [
        project_root.to_path_buf(),
        project_root.join("config"),
        declaration_path(project_root),
    ] {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "Lib configuration '{}' must not be a symbolic link",
                    path.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot inspect Lib configuration '{}': {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn normalize_libs(libs: &mut [LockedLib]) -> Result<(), String> {
    for lib in libs.iter_mut() {
        lib.dependencies.sort();
        lib.dependencies.dedup();
        for dependency in &lib.dependencies {
            validate_name(&dependency.spec.ecosystem, &dependency.spec.name)?;
            validate_version(&dependency.spec.ecosystem, &dependency.spec.version)?;
        }
        lib.artifacts.sort();
        validate_name(&lib.spec.ecosystem, &lib.spec.name)?;
        validate_version(&lib.spec.ecosystem, &lib.spec.version)?;
        validate_runtime(&lib.runtime)?;
        for artifact in &lib.artifacts {
            validate_artifact(artifact)?;
        }
    }
    libs.sort_by(|a, b| a.spec.cmp(&b.spec));
    for pair in libs.windows(2) {
        if pair[0].spec == pair[1].spec {
            return Err(format!("duplicate locked lib {}", pair[0].spec.key()));
        }
    }
    Ok(())
}

fn normalize_workers(workers: &mut [LockedWorker]) -> Result<(), String> {
    for worker in workers.iter_mut() {
        if worker.port.is_empty()
            || worker.adapter.is_empty()
            || worker.schema.is_empty()
            || worker.operations.is_empty()
        {
            return Err("locked Worker identity and operations must be complete".into());
        }
        let entry = Path::new(&worker.entry);
        if worker.entry.is_empty()
            || worker.entry.contains(['\\', ':'])
            || entry.is_absolute()
            || entry
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err("locked Worker entry must be a portable relative path".into());
        }
        worker.libs.sort();
        worker.libs.dedup();
        for spec in &worker.libs {
            validate_name(&spec.ecosystem, &spec.name)?;
            validate_version(&spec.ecosystem, &spec.version)?;
        }
        worker.operations.sort();
        worker.capabilities.sort();
        if !["exec", "pip", "npm", "go"].contains(&worker.ecosystem.as_str()) {
            return Err("locked Worker ecosystem is unknown".into());
        }
        if worker.ecosystem == "exec" {
            if worker.runtime.is_some() {
                return Err("exec Worker must not select a managed runtime".into());
            }
        } else {
            let runtime = worker
                .runtime
                .as_ref()
                .ok_or("managed Worker lacks a runtime pack identity")?;
            validate_runtime(runtime)?;
        }
        if worker.operations.iter().any(|name| name.is_empty())
            || worker.operations.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err("locked Worker operations must be unique and nonempty".into());
        }
        if worker
            .capabilities
            .iter()
            .any(|name| !["network", "file", "process", "gpu"].contains(&name.as_str()))
            || worker
                .capabilities
                .windows(2)
                .any(|pair| pair[0] == pair[1])
        {
            return Err("locked Worker capabilities are unknown or duplicated".into());
        }
    }
    workers.sort_by(|left, right| {
        left.port
            .cmp(&right.port)
            .then(left.adapter.cmp(&right.adapter))
    });
    if workers
        .windows(2)
        .any(|pair| pair[0].port == pair[1].port && pair[0].adapter == pair[1].adapter)
    {
        return Err("duplicate locked Worker identity".into());
    }
    Ok(())
}

fn validate_name(ecosystem: &Ecosystem, name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 240
        || name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(format!("invalid {} lib name", ecosystem.as_str()));
    }
    match ecosystem {
        Ecosystem::Pip => {
            let canonical = python_name(name)?.0;
            if canonical != name {
                return Err(format!(
                    "pip lib name '{name}' is not canonical; use '{canonical}'"
                ));
            }
            Ok(())
        }
        Ecosystem::Npm => {
            let valid_atom = |atom: &str| {
                !atom.is_empty()
                    && atom != "."
                    && atom != ".."
                    && atom
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
            };
            if let Some((scope, package)) = name
                .strip_prefix('@')
                .and_then(|value| value.split_once('/'))
            {
                if !valid_atom(scope) || !valid_atom(package) {
                    return Err("invalid npm scoped lib name".into());
                }
            } else if !valid_atom(name) {
                return Err("invalid npm lib name".into());
            }
            Ok(())
        }
        Ecosystem::Go => {
            if name.split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || !part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '~'))
            }) {
                return Err("invalid go module name".into());
            }
            Ok(())
        }
    }
}

fn validate_version(ecosystem: &Ecosystem, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-' | b'_' | b'!')
        })
    {
        return Err(format!("lib version '{value}' must be exact and path-safe"));
    }
    let valid = match ecosystem {
        Ecosystem::Pip => value.parse::<pep440_rs::Version>().is_ok(),
        Ecosystem::Npm => value.parse::<node_semver::Version>().is_ok(),
        Ecosystem::Go => value
            .trim_start_matches('v')
            .parse::<node_semver::Version>()
            .is_ok(),
    };
    if !valid {
        return Err(format!(
            "invalid exact {} version '{value}'",
            ecosystem.as_str()
        ));
    }
    Ok(())
}

fn validate_runtime(runtime: &RuntimePack) -> Result<(), String> {
    if runtime.name.is_empty() || runtime.version.is_empty() {
        return Err("runtime pack identity is incomplete".into());
    }
    validate_digest(&runtime.sha256)
}

fn validate_artifact(artifact: &LockedArtifact) -> Result<(), String> {
    if let Some(source) = &artifact.source {
        crate::workers::valid_path(&source.filename)?;
        if source.filename.contains('/')
            || source.locator.is_empty()
            || source.locator.len() > 8192
            || source.locator.chars().any(char::is_control)
        {
            return Err("invalid locked registry source locator or filename".into());
        }
    }
    if artifact.target.is_empty() || artifact.bytes == 0 {
        return Err("artifact target/size is invalid".into());
    }
    let path = Path::new(&artifact.path);
    if path.is_absolute()
        || artifact.path.contains(['\\', ':'])
        || artifact
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "artifact path '{}' is not a safe relative archive path",
            artifact.path
        ));
    }
    validate_digest(&artifact.sha256)
}

fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("artifact digest must be lowercase SHA-256".into());
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

fn sha256_bytes(bytes: &[u8]) -> String {
    sha256(bytes)
}

fn write_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
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
    fn spec_requires_exact_version_and_known_ecosystem() {
        assert!("pip:fixture@1.0.0".parse::<LibSpec>().is_ok());
        assert!("pip:fixture@latest".parse::<LibSpec>().is_err());
        assert!("ruby:fixture@1.0.0".parse::<LibSpec>().is_err());
    }

    #[test]
    fn builtin_resolution_is_deterministic() {
        let requested = vec![
            "npm:fixture@1.0.0".parse().unwrap(),
            "pip:fixture@1.0.0".parse().unwrap(),
        ];
        let first = FixtureRegistry::builtin()
            .resolve(&requested)
            .unwrap()
            .encode()
            .unwrap();
        let second = FixtureRegistry::builtin()
            .resolve(&requested)
            .unwrap()
            .encode()
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn lock_rejects_noncanonical_or_unsafe_artifact() {
        let mut lock = FixtureRegistry::builtin()
            .resolve(&["pip:fixture@1.0.0".parse().unwrap()])
            .unwrap();
        let bytes = lock.encode().unwrap();
        assert!(LockFile::decode(&bytes).is_ok());
        lock.libs[0].artifacts[0].path = "../escape".into();
        assert!(lock.encode().is_err());
    }

    #[test]
    fn conflicts_are_explicit() {
        let mut registry = FixtureRegistry::default();
        let a: LibSpec = "pip:a@1.0.0".parse().unwrap();
        let b: LibSpec = "pip:b@1.0.0".parse().unwrap();
        let dep: LibSpec = "pip:a@2.0.0".parse().unwrap();
        for spec in [a.clone(), dep.clone(), b.clone()] {
            registry
                .register(FixturePackage {
                    spec: spec.clone(),
                    dependencies: if spec == b { vec![dep.clone()] } else { vec![] },
                    runtime: RuntimePack {
                        name: "cpython".into(),
                        version: "1".into(),
                        sha256: "00".repeat(32),
                    },
                    artifacts: vec![FixtureArtifact {
                        target: "host".into(),
                        path: "a.bin".into(),
                        bytes: vec![1],
                    }],
                    schema: "s".into(),
                })
                .unwrap();
        }
        assert!(registry.resolve(std::slice::from_ref(&a)).is_ok());
        assert!(registry.resolve(&[a, b]).is_err());
    }
}
