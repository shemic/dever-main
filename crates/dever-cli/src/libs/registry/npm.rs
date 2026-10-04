//! npm package instances follow Node's ancestor lookup, not one global version
//! per name. Archives remain content-addressed and shared between instances.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use node_semver::{Range, Version};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Ecosystem, LibSpec, LockFile, LockedLib, RegistryRuntime, archive_files};

mod resolve;
pub(super) use resolve::resolve;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Dependency,
    Optional,
    Peer,
    OptionalPeer,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Omission {
    Target,
    Unavailable,
    Conflict,
    OptionalPeerAbsent,
    BuildFailure,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub name: String,
    pub requirement: String,
    pub kind: Kind,
    pub target: Option<String>,
    pub omission: Option<Omission>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub archive: LibSpec,
    pub prefix: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub path: String,
    pub spec: LibSpec,
    pub source: Source,
    /// Exact bundled layout of a root archive, including hoisted transitives.
    pub bundled: BTreeMap<String, LibSpec>,
    pub edges: Vec<Edge>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub roots: Vec<LibSpec>,
    pub instances: Vec<Instance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

impl Environment {
    /// Explicit single-package graph for author fixtures with no dependencies.
    /// Materialization still checks package.json against this graph.
    pub fn single(spec: LibSpec) -> Self {
        Self {
            build: None,
            roots: vec![spec.clone()],
            instances: vec![Instance {
                path: child("", &spec.name),
                spec: spec.clone(),
                source: Source {
                    archive: spec,
                    prefix: "package/".into(),
                },
                bundled: BTreeMap::new(),
                edges: Vec::new(),
            }],
        }
    }
}

pub fn roots(specs: &[LibSpec]) -> Vec<LibSpec> {
    let mut roots = specs
        .iter()
        .filter(|spec| spec.ecosystem == Ecosystem::Npm)
        .cloned()
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}

pub fn environment<'a>(
    lock: &'a LockFile,
    requested: &[LibSpec],
) -> Result<Option<&'a Environment>, String> {
    let requested = roots(requested);
    if requested.is_empty() {
        return Ok(None);
    }
    lock.npm
        .iter()
        .find(|env| env.roots == requested)
        .map(Some)
        .ok_or("npm Worker roots have no locked installation environment".into())
}

pub fn archive_roots(lock: &LockFile, requested: &[LibSpec]) -> Result<BTreeSet<LibSpec>, String> {
    let requested = roots(requested);
    if requested.is_empty() {
        return Ok(BTreeSet::new());
    }
    if let Some(environment) = lock
        .npm
        .iter()
        .find(|environment| environment.roots == requested)
    {
        return Ok(environment
            .instances
            .iter()
            .map(|node| node.source.archive.clone())
            .collect());
    }
    // Built-in metadata fixtures are not registry packages and have no tree.
    let fixtures = super::super::FixtureRegistry::builtin();
    if requested
        .iter()
        .all(|spec| fixtures.package(spec).is_some())
    {
        return Ok(requested.into_iter().collect());
    }
    Err("npm roots have no exact locked installation environment".into())
}

pub fn retain(lock: &LockFile, groups: &[Vec<LibSpec>]) -> Result<Vec<Environment>, String> {
    let mut environments = Vec::new();
    for group in groups {
        let requested = roots(group);
        if requested.is_empty() {
            continue;
        }
        let original = lock
            .npm
            .iter()
            .find(|env| env.roots == requested)
            .or_else(|| {
                lock.npm
                    .iter()
                    .find(|env| requested.iter().all(|root| env.roots.contains(root)))
            });
        let Some(original) = original else {
            archive_roots(lock, group)?;
            continue;
        };
        let mut queue = requested
            .iter()
            .map(|spec| child("", &spec.name))
            .collect::<VecDeque<_>>();
        let mut paths = BTreeSet::new();
        while let Some(path) = queue.pop_front() {
            if !paths.insert(path.clone()) {
                continue;
            }
            let node = original
                .instances
                .iter()
                .find(|node| node.path == path)
                .ok_or("npm retained graph lacks a dependency instance")?;
            queue.extend(node.edges.iter().filter_map(|edge| edge.target.clone()));
        }
        environments.push(Environment {
            build: original.build.clone(),
            roots: requested,
            instances: original
                .instances
                .iter()
                .filter(|node| paths.contains(&node.path))
                .cloned()
                .collect(),
        });
    }
    normalize(&mut environments)?;
    Ok(environments)
}

pub(super) fn child(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        format!("node_modules/{name}")
    } else {
        format!("{parent}/node_modules/{name}")
    }
}

pub(super) fn parent(path: &str) -> &str {
    path.rsplit_once("/node_modules/")
        .map_or("", |(parent, _)| parent)
}

fn validate_path(path: &str) -> Result<(), String> {
    if path.len() > 4096 {
        return Err("npm instance path exceeds its bound".into());
    }
    let path = path
        .strip_prefix("node_modules/")
        .ok_or("invalid npm instance path")?;
    for name in path.split("/node_modules/") {
        super::super::validate_name(&Ecosystem::Npm, name)?;
    }
    Ok(())
}

pub(super) fn visible<'a>(
    instances: &'a BTreeMap<String, Instance>,
    consumer: &str,
    name: &str,
    peer: bool,
) -> Option<&'a Instance> {
    let mut scope = if peer { parent(consumer) } else { consumer };
    loop {
        if let Some(found) = instances.get(&child(scope, name)) {
            return Some(found);
        }
        if scope.is_empty() {
            return None;
        }
        scope = parent(scope);
    }
}

#[derive(Clone, Debug)]
pub(super) struct Metadata {
    pub spec: LibSpec,
    pub edges: Vec<Edge>,
    pub bundles: BTreeSet<String>,
    pub document: Value,
}

fn requirements(document: &Value, field: &str) -> Result<BTreeMap<String, String>, String> {
    let Some(value) = document.get(field) else {
        return Ok(BTreeMap::new());
    };
    value
        .as_object()
        .ok_or_else(|| format!("npm {field} must be an object"))?
        .iter()
        .map(|(name, value)| {
            super::super::validate_name(&Ecosystem::Npm, name)?;
            let rule = value.as_str().ok_or("npm dependency range must be text")?;
            rule.parse::<Range>()
                .map_err(|error| format!("invalid npm range for {name}: {error}"))?;
            Ok((name.clone(), rule.into()))
        })
        .collect()
}

impl Metadata {
    pub(super) fn parse(document: Value) -> Result<Self, String> {
        let spec: LibSpec = format!(
            "npm:{}@{}",
            super::field(&document, "name")?,
            super::field(&document, "version")?
        )
        .parse()?;
        let mut regular = requirements(&document, "dependencies")?;
        let optional = requirements(&document, "optionalDependencies")?;
        regular.retain(|name, _| !optional.contains_key(name));
        let mut peers = requirements(&document, "peerDependencies")?;
        let peer_meta = document
            .get("peerDependenciesMeta")
            .map(|value| {
                value
                    .as_object()
                    .ok_or("npm peerDependenciesMeta must be an object")
            })
            .transpose()?;
        if let Some(meta) = peer_meta {
            for (name, value) in meta {
                if !peers.contains_key(name)
                    || !value.is_object()
                    || value.get("optional").is_some_and(|flag| !flag.is_boolean())
                {
                    return Err("invalid npm optional peer metadata".into());
                }
            }
        }
        // npm has one outgoing edge per name: optional > production > peer.
        peers.retain(|name, _| !regular.contains_key(name) && !optional.contains_key(name));
        let mut edges = Vec::new();
        for (entries, kind) in [
            (regular.clone(), Kind::Dependency),
            (optional.clone(), Kind::Optional),
            (peers, Kind::Peer),
        ] {
            for (name, requirement) in entries {
                let kind = if kind == Kind::Peer
                    && peer_meta
                        .and_then(|meta| meta.get(&name))
                        .and_then(|value| value.get("optional"))
                        .and_then(Value::as_bool)
                        == Some(true)
                {
                    Kind::OptionalPeer
                } else {
                    kind.clone()
                };
                edges.push(Edge {
                    name,
                    requirement,
                    kind,
                    target: None,
                    omission: None,
                });
            }
        }
        edges.sort();
        let bundles = |value: &Value| -> Result<BTreeSet<String>, String> {
            match value {
                Value::Bool(true) => Ok(regular.keys().chain(optional.keys()).cloned().collect()),
                Value::Bool(false) => Ok(BTreeSet::new()),
                Value::Array(values) => values
                    .iter()
                    .map(|value| {
                        let name = value
                            .as_str()
                            .ok_or("npm bundled dependency name must be text")?;
                        super::super::validate_name(&Ecosystem::Npm, name)?;
                        if !regular.contains_key(name) && !optional.contains_key(name) {
                            return Err(
                                "npm bundled dependency is not a declared dependency".into()
                            );
                        }
                        Ok(name.to_owned())
                    })
                    .collect(),
                _ => Err("npm bundleDependencies must be an array or boolean".into()),
            }
        };
        let first = document
            .get("bundleDependencies")
            .map(bundles)
            .transpose()?;
        let second = document
            .get("bundledDependencies")
            .map(bundles)
            .transpose()?;
        if first.is_some() && second.is_some() && first != second {
            return Err("npm bundled dependency aliases disagree".into());
        }
        Ok(Self {
            spec,
            edges,
            bundles: first.or(second).unwrap_or_default(),
            document,
        })
    }

    pub(super) fn eligible(&self, runtime: &RegistryRuntime) -> Result<bool, String> {
        let (os, cpu) = runtime
            .target
            .split_once('-')
            .ok_or("npm runtime target lacks platform identity")?;
        let os = match os {
            "windows" => "win32",
            "macos" => "darwin",
            other => other,
        };
        let cpu = match cpu {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => other,
        };
        for (field, selected) in [
            ("os", Some(os)),
            ("cpu", Some(cpu)),
            ("libc", runtime.npm_libc.as_deref()),
        ] {
            if field == "libc" && os != "linux" {
                continue;
            }
            let Some(value) = self.document.get(field) else {
                continue;
            };
            let values = value
                .as_array()
                .ok_or_else(|| format!("npm {field} must be an array"))?;
            let values = values
                .iter()
                .map(|value| value.as_str().ok_or("npm platform selector must be text"))
                .collect::<Result<Vec<_>, _>>()?;
            let Some(selected) = selected else {
                return Ok(false);
            };
            if values
                .iter()
                .any(|value| value.strip_prefix('!') == Some(selected))
                || (values.iter().any(|value| !value.starts_with('!'))
                    && !values.contains(&selected)
                    && !values.contains(&"any"))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

pub(super) struct Archive {
    pub files: Vec<(String, Vec<u8>)>,
    pub packages: BTreeMap<String, Metadata>,
}

impl Archive {
    pub(super) fn bundled(&self) -> BTreeMap<String, LibSpec> {
        self.packages
            .iter()
            .filter(|(prefix, _)| prefix.as_str() != "package/")
            .map(|(prefix, metadata)| {
                (
                    prefix
                        .strip_prefix("package/")
                        .unwrap()
                        .trim_end_matches('/')
                        .into(),
                    metadata.spec.clone(),
                )
            })
            .collect()
    }

    pub(super) fn parse(spec: &LibSpec, bytes: &[u8]) -> Result<Self, String> {
        let files = archive_files("tgz", bytes)?;
        let mut packages = BTreeMap::<String, Metadata>::new();
        for (path, bytes) in &files {
            let relative = path
                .strip_prefix("package/")
                .ok_or("npm archive must have a package/ root")?;
            if relative == "package.json" || relative.ends_with("/package.json") {
                let prefix = path.strip_suffix("package.json").unwrap();
                if prefix != "package/" {
                    let nested = prefix
                        .strip_prefix("package/")
                        .unwrap()
                        .trim_end_matches('/');
                    if validate_path(nested).is_err() {
                        continue;
                    }
                }
                let document = super::parse_json(bytes)?;
                let metadata = Metadata::parse(document)?;
                if prefix == "package/" {
                    if metadata.spec != *spec {
                        return Err(
                            "npm package.json identity differs from its registry archive".into(),
                        );
                    }
                } else if !prefix.ends_with(&format!("node_modules/{}/", metadata.spec.name)) {
                    return Err("npm bundled package identity differs from its path".into());
                }
                packages.insert(prefix.into(), metadata);
            }
        }
        let root = packages
            .get("package/")
            .ok_or("npm archive lacks package.json")?;
        for name in &root.bundles {
            if !packages.contains_key(&format!("package/node_modules/{name}/")) {
                return Err("npm declared bundled dependency is absent from its archive".into());
            }
        }
        for (path, _) in &files {
            if !packages.contains_key(&source_owner(path)?) {
                return Err("npm archive has unowned node_modules content".into());
            }
        }
        let nodes = packages
            .iter()
            .filter(|(prefix, _)| prefix.as_str() != "package/")
            .map(|(prefix, metadata)| {
                let path = prefix
                    .strip_prefix("package/")
                    .unwrap()
                    .trim_end_matches('/')
                    .to_owned();
                (
                    path.clone(),
                    Instance {
                        path,
                        spec: metadata.spec.clone(),
                        source: Source {
                            archive: spec.clone(),
                            prefix: prefix.clone(),
                        },
                        bundled: BTreeMap::new(),
                        edges: Vec::new(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut pending = root
            .bundles
            .iter()
            .map(|name| child("", name))
            .collect::<VecDeque<_>>();
        let mut reached = BTreeSet::new();
        while let Some(path) = pending.pop_front() {
            if !reached.insert(path.clone()) {
                continue;
            }
            let node = nodes
                .get(&path)
                .ok_or("npm bundle references a missing package")?;
            for edge in &packages[&node.source.prefix].edges {
                let peer = matches!(edge.kind, Kind::Peer | Kind::OptionalPeer);
                if let Some(found) = visible(&nodes, &path, &edge.name, peer) {
                    if !Version::from_str(&found.spec.version)
                        .map_err(|error| error.to_string())?
                        .satisfies(
                            &edge
                                .requirement
                                .parse::<Range>()
                                .map_err(|error| error.to_string())?,
                        )
                    {
                        return Err("npm bundled dependency has an incompatible version".into());
                    }
                    pending.push_back(found.path.clone());
                } else if edge.kind == Kind::Dependency
                    && edge.name != spec.name
                    && !root.edges.iter().any(|root| root.name == edge.name)
                {
                    return Err("npm bundled dependency closure is incomplete".into());
                }
            }
        }
        if reached.len() != nodes.len() {
            return Err("npm archive contains undeclared or unreachable bundled packages".into());
        }
        Ok(Self { files, packages })
    }
}

fn source_owner(path: &str) -> Result<String, String> {
    let Some((parent, tail)) = path.rsplit_once("/node_modules/") else {
        return Ok("package/".into());
    };
    let mut parts = tail.split('/');
    let first = parts.next().ok_or("npm archive has invalid package path")?;
    let name = if first.starts_with('@') {
        format!(
            "{first}/{}",
            parts
                .next()
                .ok_or("npm scoped package path is incomplete")?
        )
    } else {
        first.into()
    };
    let prefix = format!("{parent}/node_modules/{name}");
    validate_path(
        prefix
            .strip_prefix("package/")
            .ok_or("npm bundled archive has invalid root")?,
    )?;
    Ok(format!("{prefix}/"))
}

fn validate_source_owner(environment: &Environment, node: &Instance) -> Result<(), String> {
    for owner in &environment.instances {
        for (relative, spec) in &owner.bundled {
            if node.path == format!("{}/{relative}", owner.path)
                && (node.spec != *spec
                    || node.source.archive != owner.source.archive
                    || node.source.prefix != format!("package/{relative}/"))
            {
                return Err(
                    "npm bundled instance is not owned by its parent archive installation".into(),
                );
            }
        }
    }
    if node.source.prefix == "package/" {
        for (relative, spec) in &node.bundled {
            validate_path(relative)?;
            if spec.ecosystem != Ecosystem::Npm
                || !relative.ends_with(&format!("node_modules/{}", spec.name))
            {
                return Err("invalid npm bundled layout identity".into());
            }
            super::super::validate_name(&Ecosystem::Npm, &spec.name)?;
            super::super::validate_version(&Ecosystem::Npm, &spec.version)?;
        }
        return if node.source.archive == node.spec {
            Ok(())
        } else {
            Err("npm root archive identity differs from its instance".into())
        };
    }
    let relative = node
        .source
        .prefix
        .strip_prefix("package/")
        .and_then(|prefix| prefix.strip_suffix('/'))
        .ok_or("invalid npm bundled source prefix")?;
    validate_path(relative)?;
    if !node.bundled.is_empty()
        || !environment.instances.iter().any(|owner| {
            owner.source.prefix == "package/"
                && owner.source.archive == node.source.archive
                && owner.spec == node.source.archive
                && node.path == format!("{}/{relative}", owner.path)
                && owner.bundled.get(relative) == Some(&node.spec)
        })
    {
        return Err("npm bundled instance is not owned by its parent archive installation".into());
    }
    Ok(())
}

/// Validate immutable manifests and plan borrowed files before cloning any
/// installation output. Reused archives must not multiply past the tree budget.
pub fn files<'a>(
    environment: &Environment,
    lock: &LockFile,
    archive: impl FnMut(&LockedLib) -> Result<&'a [u8], String>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    source_files(environment, lock, archive, false)
}

pub fn build_files<'a>(
    environment: &Environment,
    lock: &LockFile,
    archive: impl FnMut(&LockedLib) -> Result<&'a [u8], String>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    source_files(environment, lock, archive, true)
}

pub(crate) fn build_executables(
    environment: &Environment,
    archives: &BTreeMap<LibSpec, Vec<u8>>,
) -> Result<BTreeSet<String>, String> {
    let modes = archives
        .iter()
        .map(|(spec, bytes)| Ok((spec, super::archive_executables("tgz", bytes)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let mut installed = BTreeSet::new();
    for node in &environment.instances {
        for path in modes
            .get(&node.source.archive)
            .ok_or("npm source modes missing")?
        {
            if source_owner(path)? == node.source.prefix {
                let relative = path
                    .strip_prefix(&node.source.prefix)
                    .ok_or("npm source mode outside owner")?;
                installed.insert(format!("{}/{relative}", node.path));
            }
        }
    }
    Ok(installed)
}

pub(crate) fn installed_owner(path: &str) -> Result<String, String> {
    source_owner(&format!("package/{path}"))?
        .strip_prefix("package/")
        .map(|owner| owner.trim_end_matches('/').into())
        .ok_or("npm output is outside its installation".into())
}

fn source_files<'a>(
    environment: &Environment,
    lock: &LockFile,
    mut archive: impl FnMut(&LockedLib) -> Result<&'a [u8], String>,
    preparing: bool,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut archives = BTreeMap::new();
    let mut archive_bytes = 0usize;
    for node in &environment.instances {
        validate_source_owner(environment, node)?;
        if archives.contains_key(&node.source.archive) {
            continue;
        }
        let lib = lock
            .libs
            .iter()
            .find(|lib| lib.spec == node.source.archive)
            .ok_or("npm graph source archive is missing")?;
        let parsed = Archive::parse(&lib.spec, archive(lib)?)?;
        for (_, bytes) in &parsed.files {
            archive_bytes = archive_bytes
                .checked_add(bytes.len())
                .ok_or("npm archive size overflow")?;
            if archive_bytes > crate::workers::MAX_TREE_BYTES {
                return Err("npm archive cache exceeds its tree byte budget".into());
            }
        }
        archives.insert(lib.spec.clone(), parsed);
    }
    let mut files = BTreeMap::new();
    let mut installed_bytes = 0usize;
    for node in &environment.instances {
        let source = &archives[&node.source.archive];
        if node.source.prefix == "package/" && node.bundled != source.bundled() {
            return Err("npm bundled layout differs from its parent archive".into());
        }
        let metadata = source
            .packages
            .get(&node.source.prefix)
            .ok_or("npm instance source package is absent")?;
        if metadata.spec != node.spec
            || metadata.edges.len() != node.edges.len()
            || metadata.edges.iter().any(|expected| {
                !node.edges.iter().any(|edge| {
                    edge.name == expected.name
                        && edge.requirement == expected.requirement
                        && edge.kind == expected.kind
                })
            })
        {
            return Err("npm locked dependency graph differs from package.json".into());
        }
        if !preparing
            && (metadata.document.get("gypfile").and_then(Value::as_bool) == Some(true)
                || metadata
                    .document
                    .get("scripts")
                    .and_then(Value::as_object)
                    .is_some_and(|scripts| {
                        ["preinstall", "install", "postinstall"]
                            .iter()
                            .any(|name| scripts.contains_key(*name))
                    }))
        {
            return Err(
                "managed npm Worker rejects lifecycle scripts and native build hooks".into(),
            );
        }
        for (path, bytes) in &source.files {
            if source_owner(path)? != node.source.prefix {
                continue;
            }
            let relative = path
                .strip_prefix(&node.source.prefix)
                .ok_or("npm file is outside its instance source")?;
            if !preparing && (relative.ends_with(".node") || relative == "binding.gyp") {
                return Err("managed npm Worker rejects native package content".into());
            }
            let installed = format!("{}/{relative}", node.path);
            installed_bytes = installed_bytes
                .checked_add(bytes.len())
                .ok_or("npm installed size overflow")?;
            if installed_bytes > crate::workers::MAX_TREE_BYTES
                || files.len() >= crate::workers::MAX_TREE_FILES
            {
                return Err("npm installation exceeds its tree file/byte budget".into());
            }
            if files.insert(installed, bytes).is_some() {
                return Err("npm installed files have conflicting owners".into());
            }
        }
    }
    Ok(files
        .into_iter()
        .map(|(path, bytes)| (path, bytes.clone()))
        .collect())
}

pub fn normalize(environments: &mut Vec<Environment>) -> Result<(), String> {
    for env in environments.iter_mut() {
        env.roots.sort();
        env.roots.dedup();
        for node in &mut env.instances {
            node.edges.sort();
        }
        env.instances
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
    environments.sort_by(|left, right| left.roots.cmp(&right.roots));
    for pair in environments.windows(2) {
        if pair[0].roots == pair[1].roots && pair[0] != pair[1] {
            return Err("npm roots have inconsistent installation environments".into());
        }
    }
    environments.dedup();
    Ok(())
}

pub fn requires_build(files: &[(String, Vec<u8>)]) -> Result<bool, String> {
    for (path, bytes) in files {
        if path.ends_with(".node") || path.ends_with("/binding.gyp") {
            return Ok(true);
        }
        if path.ends_with("/package.json") {
            let metadata: Value = serde_json::from_slice(bytes)
                .map_err(|error| format!("invalid npm installation metadata: {error}"))?;
            if metadata.get("gypfile").and_then(Value::as_bool) == Some(true)
                || metadata
                    .get("scripts")
                    .and_then(Value::as_object)
                    .is_some_and(|scripts| {
                        ["preinstall", "install", "postinstall"]
                            .iter()
                            .any(|name| scripts.contains_key(*name))
                    })
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Lifecycle output may add generated files, but cannot rewrite dependency
/// identities, create undeclared package instances, or escape the locked tree.
pub fn validate_built_files(
    environment: &Environment,
    files: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let nodes = environment
        .instances
        .iter()
        .map(|node| (node.path.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let mut manifests = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut total = 0usize;
    for (path, bytes) in files {
        crate::workers::valid_path(path)?;
        let owner = source_owner(&format!("package/{path}"))?;
        let owner = owner
            .strip_prefix("package/")
            .ok_or("npm output is outside its installation")?
            .trim_end_matches('/');
        let node = nodes
            .get(owner)
            .ok_or("npm build output introduces an unlocked instance")?;
        if !names.insert(path) {
            return Err("npm build output repeats a file".into());
        }
        total = total
            .checked_add(bytes.len())
            .ok_or("npm output byte overflow")?;
        if total > crate::workers::MAX_TREE_BYTES || names.len() > crate::workers::MAX_TREE_FILES {
            return Err("npm output exceeds installation budget".into());
        }
        if path == &format!("{owner}/package.json") {
            let metadata =
                Metadata::parse(serde_json::from_slice(bytes).map_err(|error| error.to_string())?)?;
            if metadata.spec != node.spec
                || metadata.edges.len() != node.edges.len()
                || metadata.edges.iter().any(|expected| {
                    !node.edges.iter().any(|edge| {
                        edge.name == expected.name
                            && edge.requirement == expected.requirement
                            && edge.kind == expected.kind
                    })
                })
            {
                return Err("npm lifecycle changed locked package identity or dependencies".into());
            }
            manifests.insert(owner.to_owned());
        }
    }
    if manifests.len() != nodes.len() {
        return Err("npm lifecycle removed a locked package manifest".into());
    }
    Ok(())
}

pub fn doctor(lock: &LockFile) -> Result<(), String> {
    for env in &lock.npm {
        if env.roots.is_empty()
            || env.roots != roots(&env.roots)
            || env.instances.len() > super::MAX_PACKAGES
        {
            return Err("invalid npm installation environment roots or size".into());
        }
        let nodes = env
            .instances
            .iter()
            .map(|node| (node.path.clone(), node.clone()))
            .collect::<BTreeMap<_, _>>();
        if nodes.len() != env.instances.len() {
            return Err("duplicate npm instance path".into());
        }
        let mut pending = VecDeque::new();
        for root in &env.roots {
            let path = child("", &root.name);
            if nodes.get(&path).map(|node| &node.spec) != Some(root) {
                return Err("npm root is not installed at its exact path".into());
            }
            pending.push_back(path);
        }
        let mut reached = BTreeSet::new();
        while let Some(path) = pending.pop_front() {
            if !reached.insert(path.clone()) {
                continue;
            }
            let node = nodes
                .get(&path)
                .ok_or("npm edge references a missing instance")?;
            validate_source_owner(env, node)?;
            validate_path(&node.path)?;
            super::super::validate_name(&Ecosystem::Npm, &node.spec.name)?;
            super::super::validate_version(&Ecosystem::Npm, &node.spec.version)?;
            if node.spec.ecosystem != Ecosystem::Npm
                || !node
                    .path
                    .ends_with(&format!("node_modules/{}", node.spec.name))
            {
                return Err("npm instance identity differs from its path".into());
            }
            if !lock
                .libs
                .iter()
                .any(|lib| lib.spec == node.source.archive && lib.spec.ecosystem == Ecosystem::Npm)
            {
                return Err("npm instance source archive is not locked".into());
            }
            if node.source.prefix != "package/" {
                validate_path(
                    node.source
                        .prefix
                        .strip_prefix("package/")
                        .and_then(|path| path.strip_suffix('/'))
                        .ok_or("invalid npm bundled source prefix")?,
                )?;
            } else if node.source.archive != node.spec {
                return Err("npm root archive identity differs from its instance".into());
            }
            let mut unique = BTreeSet::new();
            for edge in &node.edges {
                if !unique.insert((&edge.name, &edge.kind)) {
                    return Err("duplicate npm dependency edge".into());
                }
                let range = edge
                    .requirement
                    .parse::<Range>()
                    .map_err(|error| error.to_string())?;
                let actual = visible(
                    &nodes,
                    &path,
                    &edge.name,
                    matches!(edge.kind, Kind::Peer | Kind::OptionalPeer),
                );
                match (&edge.target, &edge.omission) {
                    (Some(target), None)
                        if actual.is_some_and(|found| {
                            found.path == *target
                                && Version::from_str(&found.spec.version)
                                    .is_ok_and(|version| version.satisfies(&range))
                        }) =>
                    {
                        pending.push_back(target.clone());
                    }
                    (None, Some(reason))
                        if actual.is_none()
                            && (edge.kind == Kind::Optional
                                && *reason != Omission::OptionalPeerAbsent
                                || edge.kind == Kind::OptionalPeer
                                    && matches!(
                                        reason,
                                        Omission::OptionalPeerAbsent | Omission::BuildFailure
                                    ))
                            && (*reason != Omission::BuildFailure || env.build.is_some()) => {}
                    _ => {
                        return Err(
                            "npm edge differs from Node lookup or its required range".into()
                        );
                    }
                }
            }
        }
        if reached.len() != nodes.len() {
            return Err("npm environment contains unreachable package instances".into());
        }
    }
    for worker in &lock.workers {
        if worker.ecosystem == "npm" {
            environment(lock, &worker.libs)?;
        }
    }
    Ok(())
}

use std::str::FromStr;
