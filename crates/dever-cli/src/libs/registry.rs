//! Registry protocol adapters used only by explicit `lib add` and `lib update`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Cursor, Read};
use std::path::{Component, Path};
use std::str::FromStr;
use std::time::{Duration, Instant};

use node_semver::Version as NpmVersion;
use pep440_rs::{Version as PythonVersion, VersionSpecifiers};
use pep508_rs::{MarkerEnvironment, Requirement, VersionOrUrl};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha512};
use url::{Host, Url};

pub mod npm;
pub mod sumdb;

use super::{
    ArtifactStore, Ecosystem, LibResolver, LibSpec, LockFile, LockedArtifact, LockedDependency,
    LockedLib, ProviderManifest, RuntimePack, sha256, validate_artifact,
};

const MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
const MAX_PACKAGES: usize = 1024;
const MAX_VERSIONS_PER_PACKAGE: usize = 10_000;
const MAX_RESOLUTION_ATTEMPTS: usize = 4096;
const REGISTRY_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_GO_ARCHIVE_REDIRECTS: usize = 2;
const GO_ARCHIVE_BUCKET: &str = "/proxy-golang-org-prod/";

/// The immutable runtime identity is supplied by the verified release pack.
/// Python marker values come from that pack, not the host interpreter.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryRuntime {
    pub pack: RuntimePack,
    pub target: String,
    pub python_markers: Option<MarkerEnvironment>,
    pub python_wheel_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_libc: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledRegistryPack {
    pub format: String,
    pub runtime: RegistryRuntime,
}

impl InstalledRegistryPack {
    pub(crate) fn validate(&self, ecosystem: &Ecosystem, target: &str) -> Result<(), String> {
        if self.format != "dever-registry-runtime-v1"
            || self.runtime.target != target
            || (ecosystem == &Ecosystem::Pip) != self.runtime.python_markers.is_some()
            || (ecosystem == &Ecosystem::Pip) != !self.runtime.python_wheel_tags.is_empty()
        {
            return Err(format!(
                "signed {} runtime manifest does not match this target",
                ecosystem.as_str()
            ));
        }
        super::validate_runtime(&self.runtime.pack)?;
        super::validate_version(ecosystem, &self.runtime.pack.version)?;
        if !self
            .runtime
            .pack
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        {
            return Err("runtime pack name is invalid".into());
        }
        if ecosystem == &Ecosystem::Pip {
            crate::workers::python_wheel::validate_target(
                target,
                &self.runtime.python_wheel_tags,
                self.runtime
                    .python_markers
                    .as_ref()
                    .expect("validated Python markers"),
            )?;
        }
        Ok(())
    }
}

/// A transport only fetches registry data; it cannot choose a package version.
/// Production uses fixed official origins. Tests can bind the same protocols to
/// a loopback registry without invoking host package managers.
pub trait RegistryTransport {
    fn get(&self, ecosystem: Ecosystem, path_or_url: &str, limit: usize)
    -> Result<Vec<u8>, String>;

    /// Only an explicit registry 404 is absence. Transport, parsing and
    /// integrity failures remain errors, including for optional dependencies.
    fn get_optional(
        &self,
        ecosystem: Ecosystem,
        path: &str,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        self.get(ecosystem, path, limit).map(Some)
    }

    fn get_sumdb(&self, path: &str, limit: usize) -> Result<Vec<u8>, String> {
        self.get(
            Ecosystem::Go,
            &format!("https://sum.golang.org{path}"),
            limit,
        )
    }
}

pub struct HttpRegistry {
    agent: ureq::Agent,
    origins: BTreeMap<Ecosystem, String>,
    artifact_origins: BTreeMap<Ecosystem, Vec<String>>,
    go_archive_mirror: Option<Url>,
}

impl HttpRegistry {
    pub fn official() -> Self {
        Self::new(
            [
                (Ecosystem::Pip, "https://pypi.org".into()),
                (Ecosystem::Npm, "https://registry.npmjs.org".into()),
                (Ecosystem::Go, "https://proxy.golang.org".into()),
            ],
            [
                (
                    Ecosystem::Pip,
                    vec!["https://files.pythonhosted.org".into()],
                ),
                (Ecosystem::Npm, vec!["https://registry.npmjs.org".into()]),
                (
                    Ecosystem::Go,
                    vec![
                        "https://proxy.golang.org".into(),
                        "https://sum.golang.org".into(),
                    ],
                ),
            ],
        )
        .with_go_archive_mirror("https://storage.googleapis.com/proxy-golang-org-prod/")
        .expect("fixed official Go archive mirror")
    }

    pub fn new(
        origins: impl IntoIterator<Item = (Ecosystem, String)>,
        artifact_origins: impl IntoIterator<Item = (Ecosystem, Vec<String>)>,
    ) -> Self {
        let config = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(REGISTRY_REQUEST_TIMEOUT))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            origins: origins
                .into_iter()
                .map(|(ecosystem, origin)| (ecosystem, origin.trim_end_matches('/').into()))
                .collect(),
            artifact_origins: artifact_origins
                .into_iter()
                .map(|(ecosystem, origins)| {
                    (
                        ecosystem,
                        origins
                            .into_iter()
                            .map(|origin| origin.trim_end_matches('/').into())
                            .collect(),
                    )
                })
                .collect(),
            go_archive_mirror: None,
        }
    }

    /// Explicit loopback mirrors let protocol tests exercise the same policy
    /// without granting production projects a configurable registry origin.
    pub fn with_go_archive_mirror(mut self, mirror: &str) -> Result<Self, String> {
        let mirror_has_userinfo = has_userinfo(mirror)?;
        let mirror =
            Url::parse(mirror).map_err(|error| format!("invalid Go archive mirror: {error}"))?;
        let go_origin = self
            .origins
            .get(&Ecosystem::Go)
            .ok_or("Go registry origin is missing")?;
        let go_origin = Url::parse(go_origin)
            .map_err(|error| format!("invalid Go registry origin: {error}"))?;
        let official = mirror.scheme() == "https"
            && mirror.host_str() == Some("storage.googleapis.com")
            && mirror.port().is_none();
        let local = mirror.scheme() == "http"
            && matches!(mirror.host(), Some(Host::Ipv4(address)) if address.is_loopback())
            && go_origin.scheme() == "http"
            && matches!(go_origin.host(), Some(Host::Ipv4(address)) if address.is_loopback());
        if (!official && !local)
            || mirror.path() != GO_ARCHIVE_BUCKET
            || mirror_has_userinfo
            || mirror.query().is_some()
            || mirror.fragment().is_some()
        {
            return Err("Go archive mirror must be the fixed official HTTPS bucket or an explicit loopback test mirror".into());
        }
        self.go_archive_mirror = Some(mirror);
        Ok(self)
    }

    fn go_redirect(&self, location: &str) -> Result<String, String> {
        let mirror = self
            .go_archive_mirror
            .as_ref()
            .ok_or("Go archive mirror is not configured")?;
        let raw_userinfo = has_userinfo(location)?;
        let target = Url::parse(location)
            .map_err(|error| format!("invalid Go archive redirect URL: {error}"))?;
        if target.scheme() != mirror.scheme()
            || target.host() != mirror.host()
            || target.port_or_known_default() != mirror.port_or_known_default()
            || raw_userinfo
            || target.fragment().is_some()
            || !target.path().starts_with(GO_ARCHIVE_BUCKET)
            || target.path().len() == GO_ARCHIVE_BUCKET.len()
        {
            return Err("Go archive redirect is outside its trusted mirror bucket".into());
        }
        Ok(target.into())
    }
}

fn has_userinfo(raw_url: &str) -> Result<bool, String> {
    let uri: ureq::http::Uri = raw_url.parse().map_err(|_| "invalid Go archive URL")?;
    Ok(uri
        .authority()
        .is_some_and(|authority| authority.as_str().contains('@')))
}

impl RegistryTransport for HttpRegistry {
    fn get(
        &self,
        ecosystem: Ecosystem,
        path_or_url: &str,
        limit: usize,
    ) -> Result<Vec<u8>, String> {
        self.fetch(ecosystem, path_or_url, limit, false)?
            .ok_or("registry returned HTTP 404".into())
    }

    fn get_optional(
        &self,
        ecosystem: Ecosystem,
        path: &str,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        self.fetch(ecosystem, path, limit, true)
    }
}

impl HttpRegistry {
    fn fetch(
        &self,
        ecosystem: Ecosystem,
        path_or_url: &str,
        limit: usize,
        allow_missing: bool,
    ) -> Result<Option<Vec<u8>>, String> {
        let origin = self
            .origins
            .get(&ecosystem)
            .ok_or("registry origin is missing")?;
        let url = if path_or_url.starts_with('/') {
            format!("{origin}{path_or_url}")
        } else {
            let allowed = self
                .artifact_origins
                .get(&ecosystem)
                .ok_or("artifact origin is missing")?;
            if !allowed
                .iter()
                .any(|origin| path_or_url.starts_with(&format!("{origin}/")))
            {
                return Err("registry artifact URL has an untrusted origin".into());
            }
            path_or_url.to_owned()
        };
        let go_archive = ecosystem == Ecosystem::Go
            && path_or_url.starts_with('/')
            && path_or_url.contains("/@v/")
            && path_or_url.ends_with(".zip");
        let mut current = url;
        let mut visited = BTreeSet::new();
        visited.insert(current.clone());
        let mut redirects = 0;
        let started = Instant::now();
        let mut response = loop {
            let remaining = REGISTRY_REQUEST_TIMEOUT
                .checked_sub(started.elapsed())
                .filter(|remaining| !remaining.is_zero())
                .ok_or("registry request exceeded the 30-second deadline")?;
            let response = self
                .agent
                .get(&current)
                .config()
                .timeout_global(Some(remaining))
                .build()
                .call();
            if allow_missing && matches!(&response, Err(ureq::Error::StatusCode(404))) {
                return Ok(None);
            }
            let response = response.map_err(|error| match error {
                ureq::Error::StatusCode(code) => {
                    format!("{} registry returned HTTP {code}", ecosystem.as_str())
                }
                _ => format!("{} registry request failed", ecosystem.as_str()),
            })?;
            if !response.status().is_redirection() {
                break response;
            }
            if !go_archive || redirects >= MAX_GO_ARCHIVE_REDIRECTS {
                return Err("registry redirect is not allowed for this endpoint".into());
            }
            let mut locations = response.headers().get_all("location").iter();
            let location = locations
                .next()
                .ok_or("Go archive redirect lacks Location")?
                .to_str()
                .map_err(|_| "Go archive redirect has invalid Location")?;
            if locations.next().is_some() {
                return Err("Go archive redirect has multiple Locations".into());
            }
            let next = self.go_redirect(location)?;
            if !visited.insert(next.clone()) {
                return Err("Go archive redirect loop detected".into());
            }
            redirects += 1;
            current = next;
        };
        if !response.status().is_success() {
            return Err(format!(
                "{} registry returned HTTP {}",
                ecosystem.as_str(),
                response.status()
            ));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read registry response: {error}"))?;
        if bytes.len() > limit {
            return Err(format!("registry response exceeds {limit} bytes"));
        }
        Ok(Some(bytes))
    }
}

pub struct RegistryResolver<'a> {
    pub transport: &'a dyn RegistryTransport,
    pub store: &'a dyn ArtifactStore,
    pub runtimes: BTreeMap<Ecosystem, RegistryRuntime>,
    pub go_sumdb: sumdb::ChecksumDatabase,
    pub build: Option<&'a super::build::Session<'a>>,
}

impl LibResolver for RegistryResolver<'_> {
    fn go_verifier(&self) -> sumdb::Verifier {
        self.go_sumdb.verifier().clone()
    }

    fn manifest(&self) -> ProviderManifest {
        ProviderManifest {
            ecosystem: Ecosystem::Pip,
            resolver: "official-registry-protocols-v1".into(),
            runtime_pack: "signed-release-runtime".into(),
            sdk: "component-protocol".into(),
            available: true,
        }
    }

    fn resolve(&self, requested: &[LibSpec]) -> Result<LockFile, String> {
        let mut libs = Vec::new();
        let mut go_sumdb = Vec::new();
        let mut npm = Vec::new();
        for ecosystem in [Ecosystem::Pip, Ecosystem::Npm, Ecosystem::Go] {
            let roots = requested
                .iter()
                .filter(|spec| spec.ecosystem == ecosystem)
                .cloned()
                .collect::<Vec<_>>();
            if roots.is_empty() {
                continue;
            }
            let runtime = self
                .runtimes
                .get(&ecosystem)
                .ok_or_else(|| format!("signed {} runtime pack is missing", ecosystem.as_str()))?;
            let packages = match ecosystem {
                Ecosystem::Pip => self.resolve_python(&roots, runtime)?,
                Ecosystem::Npm => {
                    let (mut packages, mut environment) = npm::resolve(self, &roots, runtime)?;
                    if let Some(build) = self.build {
                        build.npm(self, &mut packages, &mut environment, runtime)?;
                    }
                    npm.push(environment);
                    packages
                }
                Ecosystem::Go => {
                    let (packages, evidence) = self.resolve_go(&roots, runtime)?;
                    go_sumdb.push(evidence);
                    packages
                }
            };
            libs.extend(packages);
        }
        let mut lock = LockFile::new(libs)?;
        lock.go_sumdb = go_sumdb;
        lock.npm = npm;
        self.attach_builds(&mut lock)?;
        Ok(lock)
    }
}

impl RegistryResolver<'_> {
    fn resolve_python(
        &self,
        roots: &[LibSpec],
        runtime: &RegistryRuntime,
    ) -> Result<Vec<LockedLib>, String> {
        let mut constraints = BTreeMap::<String, Vec<Constraint>>::new();
        for spec in roots {
            constraints
                .entry(spec.name.clone())
                .or_default()
                .push(Constraint::Exact(spec.version.clone()));
        }
        self.python_constraints(constraints, runtime)
    }

    pub(crate) fn build_requirements(
        &self,
        requirements: &[String],
        runtime: &RegistryRuntime,
    ) -> Result<LockFile, String> {
        let document = serde_json::json!({"info":{"requires_dist":requirements}});
        let markers = runtime
            .python_markers
            .as_ref()
            .ok_or("Python build runtime lacks markers")?;
        let mut constraints = BTreeMap::<String, Vec<Constraint>>::new();
        for (name, rule) in python_dependencies(&document, markers, &[])? {
            constraints
                .entry(name)
                .or_default()
                .push(Constraint::Python(rule));
        }
        let mut lock = LockFile::new(self.python_constraints(constraints, runtime)?)?;
        self.attach_builds(&mut lock)?;
        super::doctor(&lock)?;
        Ok(lock)
    }

    fn attach_builds(&self, lock: &mut LockFile) -> Result<(), String> {
        let identities = lock
            .libs
            .iter()
            .filter_map(|lib| lib.build.clone())
            .chain(
                lock.npm
                    .iter()
                    .filter_map(|environment| environment.build.clone()),
            )
            .collect::<BTreeSet<_>>();
        if !identities.is_empty() {
            lock.builds = self
                .build
                .ok_or("build receipts lack their preparation session")?
                .receipts(&identities)?;
        }
        Ok(())
    }

    fn python_constraints(
        &self,
        constraints: BTreeMap<String, Vec<Constraint>>,
        runtime: &RegistryRuntime,
    ) -> Result<Vec<LockedLib>, String> {
        let markers = runtime
            .python_markers
            .as_ref()
            .ok_or("Python runtime pack has no marker environment")?;
        let mut remaining = constraints.keys().cloned().collect::<VecDeque<_>>();
        let mut metadata = BTreeMap::<(String, String), Value>::new();
        let mut wheels = BTreeMap::new();
        let selected = self.solve_registry(constraints, &mut metadata, &mut wheels, runtime)?;
        let mut visited = BTreeSet::new();
        let mut locked = Vec::new();
        let mut distributions = BTreeMap::<String, LockedLib>::new();
        while let Some(name) = remaining.pop_front() {
            if !visited.insert(name.clone()) {
                continue;
            }
            let base = super::distribution_name(&Ecosystem::Pip, &name);
            let version = selected
                .get(base)
                .ok_or("resolved Python version is missing")?;
            let spec = LibSpec {
                ecosystem: Ecosystem::Pip,
                name: name.clone(),
                version: version.clone(),
            };
            let document = metadata
                .get(&(base.to_owned(), version.clone()))
                .ok_or("resolved Python metadata is missing")?;
            let dependencies =
                python_dependencies(document, markers, &super::python_name(&name)?.1)?
                    .into_iter()
                    .map(|(name, _)| {
                        let version = selected
                            .get(super::distribution_name(&Ecosystem::Pip, &name))
                            .ok_or_else(|| format!("Python dependency {name} was not selected"))?;
                        selected_spec(Ecosystem::Pip, &name, version)
                    })
                    .collect::<Result<Vec<_>, String>>()?;
            let dependencies = dependencies
                .into_iter()
                .filter(|dependency| dependency != &spec)
                .collect::<Vec<_>>();
            remaining.extend(dependencies.iter().map(|spec| spec.name.clone()));
            let lib = if let Some(distribution) = distributions.get(base) {
                LockedLib {
                    spec,
                    dependencies: dependencies
                        .into_iter()
                        .map(|spec| LockedDependency { spec })
                        .collect(),
                    ..distribution.clone()
                }
            } else {
                let candidate = wheels
                    .get(&(base.to_owned(), version.clone()))
                    .and_then(Option::as_ref)
                    .ok_or("resolved Python wheel is missing")?;
                let mut lib = self.lock_archive(
                    &spec,
                    dependencies,
                    runtime,
                    "whl",
                    &candidate.bytes,
                    "pypi-wheel-v1",
                )?;
                lib.build = candidate.build.clone();
                distributions.insert(base.to_owned(), lib.clone());
                lib
            };
            locked.push(lib);
        }
        Ok(locked)
    }

    fn solve_registry(
        &self,
        constraints: BTreeMap<String, Vec<Constraint>>,
        metadata: &mut BTreeMap<(String, String), Value>,
        wheels: &mut BTreeMap<(String, String), Option<PythonCandidate>>,
        runtime: &RegistryRuntime,
    ) -> Result<BTreeMap<String, String>, String> {
        if constraints.len() > MAX_PACKAGES {
            return Err("registry dependency graph exceeds the package limit".into());
        }
        let mut selected = BTreeMap::new();
        let mut state = SearchState {
            metadata,
            markers: runtime.python_markers.as_ref(),
            runtime,
            wheels,
            budget: SearchBudget::default(),
        };
        self.search_registry(
            Ecosystem::Pip,
            constraints,
            BTreeSet::new(),
            &mut selected,
            &mut state,
        )
        .map_err(SearchError::message)?;
        Ok(selected)
    }

    fn expand_selected(
        &self,
        ecosystem: &Ecosystem,
        constraints: &mut BTreeMap<String, Vec<Constraint>>,
        expanded: &mut BTreeSet<String>,
        selected: &BTreeMap<String, String>,
        state: &SearchState<'_>,
    ) -> Result<(), SearchError> {
        // A later edge may activate extras on an already selected distribution.
        // Expand exact request variants to a fixed point inside this branch only.
        loop {
            if constraints.len() > MAX_PACKAGES {
                return Err(SearchError::Fatal(
                    "registry dependency graph exceeds the package limit".into(),
                ));
            }
            let request = constraints
                .keys()
                .find(|name| {
                    !expanded.contains(*name)
                        && selected.contains_key(super::distribution_name(ecosystem, name))
                })
                .cloned();
            let Some(name) = request else { break };
            let base = super::distribution_name(ecosystem, &name);
            let version = &selected[base];
            let document = state
                .metadata
                .get(&(base.to_owned(), version.clone()))
                .ok_or_else(|| {
                    SearchError::Fatal("selected registry metadata is missing".into())
                })?;
            let dependencies = match ecosystem {
                Ecosystem::Pip => {
                    let (_, extras) = super::python_name(&name).map_err(SearchError::Fatal)?;
                    validate_python_extras(document, &extras)?;
                    python_dependencies(
                        document,
                        state.markers.ok_or_else(|| {
                            SearchError::Fatal("Python marker environment is missing".into())
                        })?,
                        &extras,
                    )
                    .map_err(SearchError::Fatal)?
                    .into_iter()
                    .map(|(name, rule)| (name, Constraint::Python(rule)))
                    .collect::<Vec<_>>()
                }
                Ecosystem::Npm | Ecosystem::Go => unreachable!(),
            };
            expanded.insert(name);
            for (dependency, rule) in dependencies {
                constraints.entry(dependency).or_default().push(rule);
            }
        }
        Ok(())
    }

    fn search_registry(
        &self,
        ecosystem: Ecosystem,
        mut constraints: BTreeMap<String, Vec<Constraint>>,
        mut expanded: BTreeSet<String>,
        selected: &mut BTreeMap<String, String>,
        state: &mut SearchState<'_>,
    ) -> Result<(), SearchError> {
        self.expand_selected(&ecosystem, &mut constraints, &mut expanded, selected, state)?;
        if selected.len() > MAX_PACKAGES || constraints.len() > MAX_PACKAGES {
            return Err(SearchError::Fatal(
                "registry dependency graph exceeds the package limit".into(),
            ));
        }
        for (name, version) in selected.iter() {
            if !constraints
                .iter()
                .filter(|(request, _)| super::distribution_name(&ecosystem, request) == name)
                .flat_map(|(_, rules)| rules)
                .all(|rule| rule.matches(version))
            {
                return Err(SearchError::Conflict(format!(
                    "{} version conflict for {name}@{version}",
                    ecosystem.as_str()
                )));
            }
        }
        let Some((name, _)) = constraints
            .iter()
            .find(|(name, _)| !selected.contains_key(super::distribution_name(&ecosystem, name)))
        else {
            return Ok(());
        };
        let name = super::distribution_name(&ecosystem, name).to_owned();
        let rules = constraints
            .iter()
            .filter(|(request, _)| super::distribution_name(&ecosystem, request) == name)
            .flat_map(|(_, rules)| rules)
            .cloned()
            .collect::<Vec<_>>();
        let key = (ecosystem.clone(), name.clone());
        let versions = if let Some(versions) = state.budget.indexes.get(&key) {
            versions.clone()
        } else {
            let versions = self
                .registry_versions(ecosystem.clone(), &name)
                .map_err(SearchError::Fatal)?;
            if versions.len() > MAX_VERSIONS_PER_PACKAGE {
                return Err(SearchError::Fatal(format!(
                    "registry index for {name} exceeds the version limit"
                )));
            }
            state.budget.indexes.insert(key, versions.clone());
            versions
        };
        let mut last_conflict = None;
        for version in versions
            .into_iter()
            .filter(|version| rules.iter().all(|rule| rule.matches(version)))
        {
            state.budget.attempts += 1;
            if state.budget.attempts > MAX_RESOLUTION_ATTEMPTS {
                return Err(SearchError::Fatal(
                    "registry dependency search exceeds the attempt limit".into(),
                ));
            }
            self.registry_metadata(ecosystem.clone(), &name, &version, state.metadata)
                .map_err(SearchError::Fatal)?;
            let key = (name.clone(), version.clone());
            if !state.wheels.contains_key(&key) {
                let spec =
                    selected_spec(Ecosystem::Pip, &name, &version).map_err(SearchError::Fatal)?;
                let candidate = self
                    .python_candidate(&spec, &state.metadata[&key], state.runtime)
                    .map_err(SearchError::Fatal)?;
                if let Some(candidate) = &candidate {
                    state.budget.archive_bytes = state
                        .budget
                        .archive_bytes
                        .checked_add(candidate.bytes.len())
                        .ok_or_else(|| {
                            SearchError::Fatal("Python candidate cache size overflow".into())
                        })?;
                    if state.budget.archive_bytes > crate::workers::MAX_TREE_BYTES {
                        return Err(SearchError::Fatal(
                            "Python candidate cache exceeds its 256 MiB budget".into(),
                        ));
                    }
                    // Release JSON is upload-time metadata and can differ per
                    // platform. Only this verified wheel defines solver edges.
                    state.metadata.get_mut(&key).unwrap()["info"] = candidate.info.clone();
                }
                state.wheels.insert(key.clone(), candidate);
            }
            if state.wheels[&key].is_none() {
                last_conflict = Some(format!(
                    "pip:{name}@{version} has no compatible wheel for the signed Python runtime"
                ));
                continue;
            }
            let mut branch_selected = selected.clone();
            branch_selected.insert(name.clone(), version.clone());
            match self.search_registry(
                ecosystem.clone(),
                constraints.clone(),
                expanded.clone(),
                &mut branch_selected,
                state,
            ) {
                Ok(()) => {
                    *selected = branch_selected;
                    return Ok(());
                }
                Err(SearchError::Conflict(error)) => last_conflict = Some(error),
                Err(fatal) => return Err(fatal),
            }
        }
        Err(SearchError::Conflict(last_conflict.unwrap_or_else(|| {
            format!(
                "no {} version satisfies constraints for {name}",
                ecosystem.as_str()
            )
        })))
    }

    fn registry_versions(&self, ecosystem: Ecosystem, name: &str) -> Result<Vec<String>, String> {
        let path = match ecosystem {
            Ecosystem::Pip => format!("/pypi/{name}/json"),
            Ecosystem::Npm => format!("/{}", npm_path(name)),
            Ecosystem::Go => unreachable!(),
        };
        let bytes = if ecosystem == Ecosystem::Npm {
            let Some(bytes) =
                self.transport
                    .get_optional(ecosystem.clone(), &path, MAX_METADATA_BYTES)?
            else {
                return Ok(Vec::new());
            };
            bytes
        } else {
            self.transport
                .get(ecosystem.clone(), &path, MAX_METADATA_BYTES)?
        };
        let document = parse_json(&bytes)?;
        let versions = match ecosystem {
            Ecosystem::Pip => document.get("releases"),
            Ecosystem::Npm => document.get("versions"),
            Ecosystem::Go => unreachable!(),
        }
        .and_then(Value::as_object)
        .ok_or("registry version index is invalid")?;
        if versions.len() > MAX_VERSIONS_PER_PACKAGE {
            return Err("registry version index exceeds its version budget".into());
        }
        if ecosystem == Ecosystem::Npm
            && versions
                .keys()
                .any(|version| super::validate_version(&ecosystem, version).is_err())
        {
            return Err("npm registry index contains an invalid version".into());
        }
        let mut versions = versions
            .keys()
            .filter(|version| super::validate_version(&ecosystem, version).is_ok())
            .cloned()
            .collect::<Vec<_>>();
        match ecosystem {
            Ecosystem::Pip => {
                versions.retain(|version| PythonVersion::from_str(version).is_ok());
                versions.sort_by(|left, right| {
                    PythonVersion::from_str(right)
                        .unwrap()
                        .cmp(&PythonVersion::from_str(left).unwrap())
                });
            }
            Ecosystem::Npm => {
                versions.retain(|version| NpmVersion::from_str(version).is_ok());
                versions.sort_by(|left, right| {
                    NpmVersion::from_str(right)
                        .unwrap()
                        .cmp(&NpmVersion::from_str(left).unwrap())
                });
            }
            Ecosystem::Go => unreachable!(),
        }
        Ok(versions)
    }

    fn registry_metadata(
        &self,
        ecosystem: Ecosystem,
        name: &str,
        version: &str,
        cache: &mut BTreeMap<(String, String), Value>,
    ) -> Result<Value, String> {
        let key = (name.to_owned(), version.to_owned());
        if let Some(document) = cache.get(&key) {
            return Ok(document.clone());
        }
        let path = match ecosystem {
            Ecosystem::Pip => format!("/pypi/{name}/{version}/json"),
            Ecosystem::Npm => format!("/{}/{}", npm_path(name), version),
            Ecosystem::Go => unreachable!(),
        };
        let document = parse_json(&self.transport.get(ecosystem, &path, MAX_METADATA_BYTES)?)?;
        cache.insert(key, document.clone());
        Ok(document)
    }

    fn python_candidate(
        &self,
        spec: &LibSpec,
        document: &Value,
        runtime: &RegistryRuntime,
    ) -> Result<Option<PythonCandidate>, String> {
        let files = document
            .get("urls")
            .and_then(Value::as_array)
            .ok_or("PyPI release has no files")?;
        let python_version = runtime
            .python_markers
            .as_ref()
            .ok_or("Python marker environment is missing")?
            .python_full_version()
            .to_string()
            .parse::<PythonVersion>()
            .map_err(|error| error.to_string())?;
        let file = runtime.python_wheel_tags.iter().find_map(|tag| {
            files.iter().find(|file| {
                file.get("packagetype").and_then(Value::as_str) == Some("bdist_wheel")
                    && file
                        .get("filename")
                        .and_then(Value::as_str)
                        .is_some_and(|name| crate::workers::python_wheel::matches(name, tag))
                    && file
                        .get("requires_python")
                        .and_then(Value::as_str)
                        .is_none_or(|rule| {
                            rule.parse::<VersionSpecifiers>()
                                .is_ok_and(|rule| rule.contains(&python_version))
                        })
            })
        });
        let Some(file) = file else {
            return self.python_source_candidate(spec, files, runtime);
        };
        let expected = file
            .get("digests")
            .and_then(|digests| digests.get("sha256"))
            .and_then(Value::as_str)
            .ok_or("PyPI file lacks SHA-256")?;
        let url = field(file, "url")?;
        let bytes = self
            .transport
            .get(Ecosystem::Pip, url, MAX_ARTIFACT_BYTES)?;
        if sha256(&bytes) != expected {
            return Err(format!("PyPI wheel SHA-256 mismatch for {}", spec.key()));
        }
        let wheel = crate::workers::python_wheel::Wheel::parse(
            spec,
            Some(field(file, "filename")?),
            &runtime.python_wheel_tags,
            &bytes,
        )?;
        if let Some(rule) = &wheel.requires_python
            && !rule
                .parse::<VersionSpecifiers>()
                .map_err(|error| error.to_string())?
                .contains(&python_version)
        {
            return Ok(None);
        }
        Ok(Some(PythonCandidate {
            build: None,
            bytes,
            info: serde_json::json!({"requires_dist":wheel.requirements,"provides_extra":wheel.extras}),
        }))
    }

    fn python_source_candidate(
        &self,
        spec: &LibSpec,
        files: &[Value],
        runtime: &RegistryRuntime,
    ) -> Result<Option<PythonCandidate>, String> {
        let python = runtime
            .python_markers
            .as_ref()
            .ok_or("Python marker environment is missing")?
            .python_full_version();
        let Some(file) = files.iter().find(|file| {
            file.get("packagetype").and_then(Value::as_str) == Some("sdist")
                && file
                    .get("requires_python")
                    .and_then(Value::as_str)
                    .is_none_or(|rule| {
                        rule.parse::<VersionSpecifiers>()
                            .is_ok_and(|rule| rule.contains(python))
                    })
        }) else {
            return Ok(None);
        };
        let session = self.build.ok_or("Python source distribution requires a signed build pack during explicit lib add/update")?;
        let expected = file
            .get("digests")
            .and_then(|value| value.get("sha256"))
            .and_then(Value::as_str)
            .ok_or("PyPI source lacks SHA-256")?;
        let bytes = self
            .transport
            .get(Ecosystem::Pip, field(file, "url")?, MAX_ARTIFACT_BYTES)?;
        if sha256(&bytes) != expected {
            return Err("PyPI source SHA-256 mismatch".into());
        }
        let product = session.python(self, spec, runtime, field(file, "filename")?, &bytes)?;
        let wheel = crate::workers::python_wheel::Wheel::parse(
            spec,
            Some(&product.filename),
            &runtime.python_wheel_tags,
            &product.bytes,
        )?;
        if wheel.requires_python.as_ref().is_some_and(|rule| {
            rule.parse::<VersionSpecifiers>()
                .is_ok_and(|rule| !rule.contains(python))
        }) {
            return Ok(None);
        }
        Ok(Some(PythonCandidate {
            bytes: product.bytes,
            info: serde_json::json!({"requires_dist":wheel.requirements,"provides_extra":wheel.extras}),
            build: Some(product.receipt.identity()?),
        }))
    }

    fn npm_archive(&self, spec: &LibSpec, document: &Value) -> Result<Vec<u8>, String> {
        let dist = document
            .get("dist")
            .ok_or("npm version has no dist metadata")?;
        let integrity = field(dist, "integrity")?;
        let expected = integrity
            .strip_prefix("sha512-")
            .ok_or("npm dist.integrity must use SHA-512")?;
        let url = field(dist, "tarball")?;
        let bytes = self
            .transport
            .get(Ecosystem::Npm, url, MAX_ARTIFACT_BYTES)?;
        let digest = Sha512::digest(&bytes);
        use base64::Engine;
        if base64::engine::general_purpose::STANDARD.encode(digest) != expected {
            return Err(format!("npm tarball integrity mismatch for {}", spec.key()));
        }
        Ok(bytes)
    }

    fn lock_archive(
        &self,
        spec: &LibSpec,
        dependencies: Vec<LibSpec>,
        runtime: &RegistryRuntime,
        extension: &str,
        bytes: &[u8],
        schema: &str,
    ) -> Result<LockedLib, String> {
        let files = archive_files(extension, bytes)?;
        if spec.ecosystem == Ecosystem::Go {
            let prefix = format!("{}@v{}/", spec.name, spec.version);
            if files.iter().any(|(path, _)| !path.starts_with(&prefix)) {
                return Err(format!(
                    "Go module archive for {} has a mismatched module root",
                    spec.key()
                ));
            }
        }
        let path = format!(
            "lib/{}/{}/{}/archive.{extension}",
            spec.ecosystem.as_str(),
            sha256(spec.distribution_name().as_bytes()),
            spec.version
        );
        let artifact = LockedArtifact {
            target: runtime.target.clone(),
            path,
            bytes: bytes.len() as u64,
            sha256: sha256(bytes),
        };
        validate_artifact(&artifact)?;
        let published = self.store.publish(bytes, &runtime.target)?;
        if published != artifact.sha256 {
            return Err(format!("artifact cache digest mismatch for {}", spec.key()));
        }
        let mut dependencies = dependencies
            .into_iter()
            .map(|spec| LockedDependency { spec })
            .collect::<Vec<_>>();
        dependencies.sort();
        dependencies.dedup();
        Ok(LockedLib {
            spec: spec.clone(),
            dependencies,
            runtime: runtime.pack.clone(),
            artifacts: vec![artifact],
            schema: schema.into(),
            build: None,
        })
    }

    fn resolve_go(
        &self,
        roots: &[LibSpec],
        runtime: &RegistryRuntime,
    ) -> Result<(Vec<LockedLib>, sumdb::Evidence), String> {
        let mut sumdb = self.go_sumdb.begin(self.transport)?;
        let roots = roots
            .iter()
            .map(|spec| (spec.name.clone(), spec.version.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut selected = roots.clone();
        let mut metadata = BTreeMap::<(String, String), Vec<u8>>::new();
        for _ in 0..MAX_PACKAGES {
            let mut next = roots.clone();
            let mut queue = roots.keys().cloned().collect::<VecDeque<_>>();
            let mut visited = BTreeSet::new();
            while let Some(name) = queue.pop_front() {
                if !visited.insert(name.clone()) {
                    continue;
                }
                let version = selected
                    .get(&name)
                    .or_else(|| next.get(&name))
                    .ok_or("Go module has no selected version")?
                    .clone();
                let source = self.go_mod(&name, &version, &mut metadata, &mut sumdb)?;
                for (dependency, required) in
                    go_requirements(&source, &name, &runtime.pack.version)?
                {
                    let old = next
                        .entry(dependency.clone())
                        .or_insert_with(|| required.clone());
                    if go_version_key(&required) > go_version_key(old) {
                        *old = required;
                    }
                    queue.push_back(dependency);
                }
                if visited.len() > MAX_PACKAGES {
                    return Err("Go module graph exceeds the package limit".into());
                }
            }
            if next == selected {
                let libs = selected
                    .iter()
                    .map(|(name, version)| {
                        let spec = LibSpec {
                            ecosystem: Ecosystem::Go,
                            name: name.clone(),
                            version: version.clone(),
                        };
                        let source = self.go_mod(name, version, &mut metadata, &mut sumdb)?;
                        let dependencies = go_requirements(&source, name, &runtime.pack.version)?
                            .into_keys()
                            .map(|name| {
                                let version =
                                    selected.get(&name).ok_or("Go MVS dependency is missing")?;
                                selected_spec(Ecosystem::Go, &name, version)
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        let zip = self.transport.get(
                            Ecosystem::Go,
                            &format!(
                                "/{}/@v/v{}.zip",
                                go_path(name),
                                version.trim_start_matches('v')
                            ),
                            MAX_ARTIFACT_BYTES,
                        )?;
                        sumdb.verify_zip(name, version, &zip)?;
                        self.lock_archive(&spec, dependencies, runtime, "zip", &zip, "go-sumdb-v1")
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                return Ok((libs, sumdb.finish()));
            }
            selected = next;
        }
        Err("Go MVS selection did not converge".into())
    }

    fn go_mod(
        &self,
        name: &str,
        version: &str,
        cache: &mut BTreeMap<(String, String), Vec<u8>>,
        sumdb: &mut sumdb::Session<'_>,
    ) -> Result<Vec<u8>, String> {
        let key = (name.to_owned(), version.to_owned());
        if let Some(source) = cache.get(&key) {
            return Ok(source.clone());
        }
        let source = self.transport.get(
            Ecosystem::Go,
            &format!(
                "/{}/@v/v{}.mod",
                go_path(name),
                version.trim_start_matches('v')
            ),
            MAX_METADATA_BYTES,
        )?;
        sumdb.verify_mod(name, version, &source)?;
        cache.insert(key, source.clone());
        Ok(source)
    }
}

#[derive(Clone)]
enum Constraint {
    Exact(String),
    Python(VersionSpecifiers),
}

#[derive(Default)]
struct SearchBudget {
    attempts: usize,
    indexes: BTreeMap<(Ecosystem, String), Vec<String>>,
    archive_bytes: usize,
}

struct PythonCandidate {
    bytes: Vec<u8>,
    info: Value,
    build: Option<String>,
}

struct SearchState<'a> {
    metadata: &'a mut BTreeMap<(String, String), Value>,
    markers: Option<&'a MarkerEnvironment>,
    runtime: &'a RegistryRuntime,
    wheels: &'a mut BTreeMap<(String, String), Option<PythonCandidate>>,
    budget: SearchBudget,
}

enum SearchError {
    Conflict(String),
    Fatal(String),
}

impl SearchError {
    fn message(self) -> String {
        match self {
            Self::Conflict(message) | Self::Fatal(message) => message,
        }
    }
}

impl Constraint {
    fn matches(&self, version: &str) -> bool {
        match self {
            Self::Exact(exact) => exact == version,
            Self::Python(rule) => {
                PythonVersion::from_str(version).is_ok_and(|version| rule.contains(&version))
            }
        }
    }
}

pub(crate) fn python_dependencies(
    document: &Value,
    markers: &MarkerEnvironment,
    extras: &[pep508_rs::ExtraName],
) -> Result<Vec<(String, VersionSpecifiers)>, String> {
    let dependencies = document
        .get("info")
        .ok_or("PyPI release lacks info metadata")?
        .get("requires_dist")
        .ok_or("PyPI release lacks requires_dist metadata")?;
    if dependencies.is_null() {
        return Ok(Vec::new());
    }
    let dependencies = dependencies
        .as_array()
        .ok_or("PyPI requires_dist must be an array or null")?;
    dependencies
        .iter()
        .map(|dependency| {
            let requirement: Requirement = field_value(dependency)?
                .parse()
                .map_err(|error| format!("invalid Requires-Dist: {error}"))?;
            if !requirement.evaluate_markers(markers, extras) {
                return Ok(None);
            }
            let name = if requirement.extras.is_empty() {
                requirement.name.to_string()
            } else {
                super::python_name(&format!(
                    "{}[{}]",
                    requirement.name,
                    requirement
                        .extras
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ))?
                .0
            };
            let specifiers = match requirement.version_or_url {
                Some(VersionOrUrl::VersionSpecifier(specifiers)) => specifiers,
                None => VersionSpecifiers::empty(),
                Some(VersionOrUrl::Url(_)) => {
                    return Err("PyPI dependency URL is not supported".into());
                }
            };
            Ok(Some((name, specifiers)))
        })
        .collect::<Result<Vec<_>, String>>()
        .map(|entries| entries.into_iter().flatten().collect())
}

fn validate_python_extras(
    document: &Value,
    extras: &[pep508_rs::ExtraName],
) -> Result<(), SearchError> {
    let declared = document
        .get("info")
        .and_then(|info| info.get("provides_extra"));
    let declared = match declared {
        None | Some(Value::Null) => BTreeSet::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                super::python_extra(field_value(value)?)
                    .map_err(|error| format!("invalid PyPI Provides-Extra: {error}"))
            })
            .collect::<Result<BTreeSet<_>, String>>()
            .map_err(SearchError::Fatal)?,
        _ => {
            return Err(SearchError::Fatal(
                "PyPI provides_extra must be an array or null".into(),
            ));
        }
    };
    for extra in extras {
        if !declared.contains(extra) {
            return Err(SearchError::Conflict(format!(
                "PyPI release does not provide requested extra '{extra}'"
            )));
        }
    }
    Ok(())
}

fn go_requirements(
    bytes: &[u8],
    module: &str,
    runtime_version: &str,
) -> Result<BTreeMap<String, String>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Go mod file is not UTF-8")?;
    let mut dependencies = BTreeMap::new();
    let mut in_require = false;
    let mut declared_module = false;
    let runtime_version = NpmVersion::from_str(runtime_version)
        .map_err(|error| format!("managed Go runtime has an invalid version: {error}"))?;
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "require (" {
            in_require = true;
            continue;
        }
        if in_require && line == ")" {
            in_require = false;
            continue;
        }
        if line.starts_with("replace ")
            || line.starts_with("exclude ")
            || line.starts_with("replace(")
            || line.starts_with("exclude(")
        {
            return Err("Go replace/exclude directives require a managed module policy".into());
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if !in_require {
            match fields.as_slice() {
                ["module", declared] => {
                    if declared_module || *declared != module {
                        return Err(format!("Go module declaration does not match {module}"));
                    }
                    declared_module = true;
                    continue;
                }
                ["go", version] | ["toolchain", version] => {
                    let minimum = go_toolchain_version(version)?;
                    if minimum > runtime_version {
                        return Err(format!(
                            "Go module requires toolchain {minimum}, managed runtime is {runtime_version}"
                        ));
                    }
                    continue;
                }
                ["require", ..] => {}
                _ => return Err(format!("unsupported Go module directive '{line}'")),
            }
        }
        let pair = if in_require {
            fields.as_slice()
        } else {
            &fields[1..]
        };
        if pair.len() != 2 {
            return Err("invalid Go require directive".into());
        }
        let spec =
            format!("go:{}@{}", pair[0], pair[1].trim_start_matches('v')).parse::<LibSpec>()?;
        if dependencies.insert(spec.name, spec.version).is_some() {
            return Err("duplicate Go require directive".into());
        }
    }
    if in_require {
        return Err("unclosed Go require block".into());
    }
    if !declared_module {
        return Err(format!("Go module declaration for {module} is missing"));
    }
    Ok(dependencies)
}

fn go_toolchain_version(value: &str) -> Result<NpmVersion, String> {
    let value = value.strip_prefix("go").unwrap_or(value);
    let version = if value.split('.').count() == 2 {
        format!("{value}.0")
    } else {
        value.to_owned()
    };
    NpmVersion::from_str(&version)
        .map_err(|error| format!("invalid Go toolchain directive: {error}"))
}

fn go_version_key(version: &str) -> NpmVersion {
    NpmVersion::from_str(version).expect("Go version was validated when parsed")
}

fn go_path(name: &str) -> String {
    name.chars()
        .flat_map(|character| {
            if character.is_ascii_uppercase() {
                vec!['!', character.to_ascii_lowercase()]
            } else {
                vec![character]
            }
        })
        .collect()
}

fn npm_path(name: &str) -> String {
    name.replace('/', "%2f")
}

fn selected_spec(ecosystem: Ecosystem, name: &str, version: &str) -> Result<LibSpec, String> {
    format!("{}:{name}@{version}", ecosystem.as_str()).parse()
}

fn field<'a>(document: &'a Value, name: &str) -> Result<&'a str, String> {
    document
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("registry metadata lacks {name}"))
}

fn field_value(value: &Value) -> Result<&str, String> {
    value.as_str().ok_or("registry field must be text".into())
}

fn parse_json(bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("invalid registry JSON: {error}"))
}

fn safe_archive_path(path: &Path) -> Result<(), String> {
    let text = path.to_str().ok_or("archive path is not UTF-8")?;
    if text.is_empty()
        || text.contains(['\\', ':'])
        || text
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("archive path '{text}' escapes its package"));
    }
    Ok(())
}

/// The same bounded extraction contract is used by registry admission and
/// managed Worker packaging. Files retain their archive-relative names.
pub(crate) fn archive_files(
    extension: &str,
    bytes: &[u8],
) -> Result<Vec<(String, Vec<u8>)>, String> {
    if matches!(extension, "whl" | "zip") {
        return Ok(zip_entries(bytes)?
            .into_iter()
            .filter(|(name, _)| !name.ends_with('/'))
            .collect());
    }
    tar_files(bytes, extension, 256 * 1024 * 1024)
}

/// Read only permission metadata from an archive already admitted by
/// `archive_files`. Set-id bits are never propagated to an installed tree.
pub(crate) fn archive_executables(
    extension: &str,
    bytes: &[u8],
) -> Result<BTreeSet<String>, String> {
    let mut paths = BTreeSet::new();
    if matches!(extension, "zip" | "whl") {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
        for index in 0..archive.len() {
            let entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if !entry.is_dir() && entry.unix_mode().unwrap_or(0) & 0o111 != 0 {
                paths.insert(entry.name().into());
            }
        }
    } else if extension == "tgz" {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(bytes)));
        for entry in archive.entries().map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.header().entry_type().is_file()
                && entry.header().mode().map_err(|error| error.to_string())? & 0o111 != 0
            {
                paths.insert(
                    entry
                        .path()
                        .map_err(|error| error.to_string())?
                        .to_str()
                        .ok_or("archive path is not UTF-8")?
                        .into(),
                );
            }
        }
    } else {
        return Err("unsupported archive permissions format".into());
    }
    Ok(paths)
}

/// Build-tool packs have a separate expansion budget; normal packages retain
/// the Worker limit. Both paths use the same entry and traversal validation.
pub(crate) fn tar_files(
    bytes: &[u8],
    extension: &str,
    max_bytes: u64,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut total = 0u64;
    let mut names = BTreeSet::new();
    let mut files = Vec::new();
    match extension {
        "tgz" => {
            let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
            let mut archive = tar::Archive::new(decoder);
            for (index, entry) in archive
                .entries()
                .map_err(|error| format!("invalid tar archive: {error}"))?
                .enumerate()
            {
                if index >= 65_536 {
                    return Err("archive has too many entries".into());
                }
                let mut entry = entry.map_err(|error| format!("invalid tar entry: {error}"))?;
                if !entry.header().entry_type().is_file() && !entry.header().entry_type().is_dir() {
                    return Err("archive contains a link or unsupported entry".into());
                }
                let path = entry
                    .path()
                    .map_err(|error| format!("invalid tar path: {error}"))?
                    .into_owned();
                let name = path
                    .to_str()
                    .ok_or("archive path is not UTF-8")?
                    .trim_end_matches('/')
                    .to_owned();
                safe_archive_path(Path::new(&name))?;
                if !names.insert(name.clone()) {
                    return Err(format!("archive repeats path '{name}'"));
                }
                if entry.header().entry_type().is_dir() {
                    continue;
                }
                total = total
                    .checked_add(entry.size())
                    .ok_or("archive expanded size overflow")?;
                if total > max_bytes {
                    return Err("archive expanded size exceeds its byte budget".into());
                }
                let mut output = Vec::new();
                let expected_size = entry.size();
                let remaining = max_bytes - total + expected_size + 1;
                entry
                    .by_ref()
                    .take(remaining)
                    .read_to_end(&mut output)
                    .map_err(|error| format!("corrupt tar entry: {error}"))?;
                if output.len() as u64 != expected_size {
                    return Err("tar entry size drift".into());
                }
                files.push((name, output));
            }
        }
        _ => return Err("unsupported registry archive format".into()),
    }
    if total == 0 {
        return Err("registry archive is empty".into());
    }
    Ok(files)
}

/// Preserve exact ZIP names, including directory entries: Go's HashZip hashes
/// those entries too. Extraction consumers explicitly filter directories.
fn zip_entries(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("invalid ZIP archive: {error}"))?;
    if archive.len() > 65_536 {
        return Err("archive has too many entries".into());
    }
    let mut total = 0u64;
    let mut names = BTreeSet::new();
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("invalid ZIP entry: {error}"))?;
        let name = entry.name().to_owned();
        let path = name.strip_suffix('/').unwrap_or(&name);
        safe_archive_path(Path::new(path))?;
        if !names.insert(path.to_owned()) {
            return Err(format!("archive repeats path '{name}'"));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000 | 0o040000))
        {
            return Err("archive contains a link or unsupported entry".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("archive expanded size overflow")?;
        if total > 256 * 1024 * 1024 {
            return Err("archive expanded size exceeds 256 MiB".into());
        }
        let expected_size = entry.size();
        let mut output = Vec::new();
        entry
            .by_ref()
            .take(256 * 1024 * 1024 - total + expected_size + 1)
            .read_to_end(&mut output)
            .map_err(|error| format!("corrupt ZIP entry: {error}"))?;
        if output.len() as u64 != expected_size {
            return Err("ZIP entry size drift".into());
        }
        entries.push((name, output));
    }
    if total == 0 {
        return Err("registry archive is empty".into());
    }
    Ok(entries)
}
