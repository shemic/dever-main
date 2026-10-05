//! Explicit Lib preparation: shared owned staging and bounded tool execution.

mod npm;
pub mod pack;
pub(crate) mod process;
mod python;

use super::{Ecosystem, LibSpec, LockFile, LockedArtifact, RegistryRuntime, RuntimePack};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

fn require_execution_target(target: &str, operation: &str) -> Result<(), String> {
    if target != crate::toolchain::BuildTarget::host()?.platform() {
        return Err(format!(
            "{operation} for {target} requires prepared target output and a complete build receipt; prepare the signed target inputs in a matching isolated execution environment before the offline build"
        ));
    }
    Ok(())
}

/// Production supplies only authenticated release resources. Explicit author
/// tests inject the same immutable inputs without creating a host-tool fallback.
pub trait Inputs {
    fn runtime(&self, ecosystem: Ecosystem) -> Result<(RegistryRuntime, Vec<u8>), String>;
    fn tools(&self, ecosystem: Ecosystem) -> Result<(pack::Descriptor, Vec<u8>), String>;
    fn assets(&self) -> Result<Vec<(String, Vec<u8>)>, String>;
    /// Author tools may display bounded hook output without retaining temporary
    /// staging trees or putting diagnostics in reproducible build identities.
    fn diagnostic(&self, _output: &[u8]) {}
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonSystem {
    pub requires: Vec<String>,
    pub backend: String,
    pub backend_path: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRejection {
    Format,
    Target,
    Closure,
    Unloadable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub ecosystem: Ecosystem,
    pub roots: Vec<LibSpec>,
    pub sources: Vec<LockedArtifact>,
    pub graph: Option<super::npm::Environment>,
    pub runtime: RuntimePack,
    pub auxiliary_runtime: Option<RuntimePack>,
    pub native_entries: BTreeSet<String>,
    pub native_rejections: BTreeMap<String, NativeRejection>,
    pub python_markers: Option<pep508_rs::MarkerEnvironment>,
    pub tools: pack::Descriptor,
    pub frontend: String,
    pub python: Option<PythonSystem>,
    pub dynamic_requires: Vec<String>,
    pub inputs: Box<LockFile>,
    pub config_settings: BTreeMap<String, serde_json::Value>,
    pub output: LockedArtifact,
}

impl Receipt {
    pub fn identity(&self) -> Result<String, String> {
        serde_json::to_vec(self)
            .map(|bytes| super::sha256(&bytes))
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct Product {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub receipt: Receipt,
}

pub struct Session<'a> {
    inputs: &'a dyn Inputs,
    active: RefCell<BTreeSet<String>>,
    products: RefCell<BTreeMap<String, Product>>,
}

impl<'a> Session<'a> {
    pub(crate) fn replay(
        &self,
        receipt: &Receipt,
        store: &dyn super::ArtifactStore,
    ) -> Result<(), String> {
        if receipt.frontend != frontend(&receipt.ecosystem)? {
            return Err("locked build frontend differs from this toolchain".into());
        }
        let bytes = match receipt.ecosystem {
            Ecosystem::Pip => self.replay_python(receipt, store)?,
            Ecosystem::Npm => self.replay_npm(receipt, store)?,
            Ecosystem::Go => return Err("Go source build receipts are unsupported".into()),
        };
        if bytes.len() as u64 != receipt.output.bytes
            || super::sha256(&bytes) != receipt.output.sha256
        {
            return Err("fixed build replay is not byte reproducible: output differs from dever.lock; lock was not changed".into());
        }
        if store.publish(&bytes, &receipt.output.target)? != receipt.output.sha256 {
            return Err("restored build cache identity mismatch".into());
        }
        Ok(())
    }
    pub fn new(inputs: &'a dyn Inputs) -> Self {
        Self {
            inputs,
            active: RefCell::new(BTreeSet::new()),
            products: RefCell::new(BTreeMap::new()),
        }
    }

    pub(crate) fn receipts(&self, identities: &BTreeSet<String>) -> Result<Vec<Receipt>, String> {
        let receipts = self
            .products
            .borrow()
            .values()
            .map(|product| Ok((product.receipt.identity()?, product.receipt.clone())))
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        identities
            .iter()
            .map(|identity| {
                receipts
                    .get(identity)
                    .cloned()
                    .ok_or("prepared build receipt is missing".into())
            })
            .collect()
    }

    fn remember(&self, key: String, product: Product) -> Result<(), String> {
        let total = self
            .products
            .borrow()
            .values()
            .try_fold(product.bytes.len(), |total, cached| {
                total.checked_add(cached.bytes.len())
            })
            .ok_or("build candidate cache byte overflow")?;
        if total > crate::workers::MAX_TREE_BYTES {
            return Err("build candidate cache exceeds byte budget".into());
        }
        self.products.borrow_mut().insert(key, product);
        Ok(())
    }
}

pub fn normalize(receipts: &mut Vec<Receipt>) -> Result<(), String> {
    let mut indexed = BTreeMap::new();
    for receipt in receipts.drain(..) {
        indexed.insert(receipt.identity()?, receipt);
    }
    *receipts = indexed.into_values().collect();
    Ok(())
}

pub fn npm_outputs(lock: &LockFile) -> impl Iterator<Item = &Receipt> {
    lock.builds
        .iter()
        .filter(|receipt| receipt.ecosystem == Ecosystem::Npm)
}

pub(crate) fn npm_receipt<'a>(
    lock: &'a LockFile,
    environment: &super::npm::Environment,
) -> Result<Option<&'a Receipt>, String> {
    let Some(identity) = &environment.build else {
        return Ok(None);
    };
    lock.builds
        .iter()
        .find(|receipt| receipt.identity().as_ref() == Ok(identity))
        .map(Some)
        .ok_or("npm installation receipt is missing".into())
}

/// Offline consumption never invokes a frontend. Removing roots projects the
/// already built graph and output; it does not rerun lifecycle side effects.
pub fn npm_installation(
    lock: &LockFile,
    environment: &super::npm::Environment,
    bytes: &[u8],
) -> Result<Vec<(String, Vec<u8>, bool)>, String> {
    let receipt = npm_receipt(lock, environment)?.ok_or("npm installation has no build receipt")?;
    if bytes.len() as u64 != receipt.output.bytes || super::sha256(bytes) != receipt.output.sha256 {
        return Err("npm built installation differs from its locked artifact".into());
    }
    let graph = receipt
        .graph
        .as_ref()
        .ok_or("npm receipt lacks its exact graph")?;
    let files = super::archive_files("tgz", bytes)?;
    super::npm::validate_built_files(graph, &files)?;
    let candidates = files
        .iter()
        .filter(|(path, _)| path.ends_with(".node"))
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    let accounted = receipt
        .native_entries
        .iter()
        .chain(receipt.native_rejections.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    if candidates != accounted
        || receipt
            .native_rejections
            .keys()
            .any(|path| receipt.native_entries.contains(path))
    {
        return Err("npm native admission does not cover its exact addon files".into());
    }
    for path in &receipt.native_entries {
        if !files.iter().any(|(name, bytes)| {
            name == path && path.ends_with(".node") && bytes.starts_with(b"\x7fELF")
        }) {
            return Err("npm locked native entry point is absent from output".into());
        }
    }
    let executable = super::registry::archive_executables("tgz", bytes)?;
    let retained = environment
        .instances
        .iter()
        .map(|node| node.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut output = Vec::new();
    for (path, bytes) in files {
        if path.ends_with(".node") && !receipt.native_entries.contains(&path) {
            continue;
        }
        if retained.contains(super::npm::installed_owner(&path)?.as_str()) {
            let mode = executable.contains(&path);
            output.push((path, bytes, mode));
        }
    }
    Ok(output)
}

pub fn doctor(lock: &LockFile) -> Result<(), String> {
    doctor_at(lock, 0)
}

fn doctor_at(lock: &LockFile, depth: usize) -> Result<(), String> {
    if lock.format != super::LOCK_FORMAT || lock.toolchain != super::TOOLCHAIN {
        return Err("build input lock format or toolchain differs from this release".into());
    }
    if depth > 16 {
        return Err("build dependency depth exceeds limit".into());
    }
    let records = lock
        .builds
        .iter()
        .map(|receipt| Ok((receipt.identity()?, receipt)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    if records.len() != lock.builds.len() {
        return Err("duplicate build receipt".into());
    }
    let mut used = BTreeSet::new();
    for lib in &lock.libs {
        if let Some(identity) = &lib.build {
            let receipt = records
                .get(identity)
                .ok_or("Python Lib has no exact build receipt")?;
            if lib.spec.ecosystem != Ecosystem::Pip
                || receipt.ecosystem != Ecosystem::Pip
                || receipt.runtime != lib.runtime
                || !lib.artifacts.contains(&receipt.output)
                || receipt.roots.len() != 1
                || receipt.roots[0].distribution_name() != lib.spec.distribution_name()
                || receipt.roots[0].version != lib.spec.version
            {
                return Err(
                    "Python Lib build receipt differs from locked artifact identity".into(),
                );
            }
            used.insert(identity.clone());
        }
    }
    for environment in &lock.npm {
        if let Some(identity) = &environment.build {
            let receipt = records
                .get(identity)
                .ok_or("npm environment has no exact build receipt")?;
            let graph = receipt
                .graph
                .as_ref()
                .ok_or("npm build receipt lacks its graph")?;
            if receipt.ecosystem != Ecosystem::Npm
                || receipt.roots != graph.roots
                || graph.build.is_some()
                || !environment
                    .roots
                    .iter()
                    .all(|root| graph.roots.contains(root))
                || !environment
                    .instances
                    .iter()
                    .all(|node| graph.instances.contains(node))
                || !environment.instances.iter().all(|node| {
                    lock.libs.iter().any(|lib| {
                        lib.spec == node.source.archive
                            && lib.runtime == receipt.runtime
                            && lib
                                .artifacts
                                .iter()
                                .all(|artifact| receipt.sources.contains(artifact))
                    })
                })
            {
                return Err("npm installation build receipt differs from locked graph".into());
            }
            used.insert(identity.clone());
        }
    }
    if used.len() != records.len() {
        return Err("lock contains an unreferenced build receipt".into());
    }
    for receipt in records.values() {
        super::validate_runtime(&receipt.runtime)?;
        if receipt.tools.runtime_sha256 != receipt.runtime.sha256
            || receipt.tools.ecosystem != receipt.ecosystem
            || receipt.sources.is_empty()
            || receipt.output.target != receipt.tools.target
            || receipt.roots.is_empty()
            || receipt.frontend != frontend(&receipt.ecosystem)?
        {
            return Err("build receipt has inconsistent tools, frontend, or target".into());
        }
        for artifact in receipt.sources.iter().chain([&receipt.output]) {
            super::validate_artifact(artifact)?;
            if artifact.target != receipt.tools.target {
                return Err("build artifact target differs from its tools".into());
            }
        }
        if receipt.ecosystem == Ecosystem::Pip
            && (receipt.python.is_none()
                || receipt.graph.is_some()
                || receipt.auxiliary_runtime.is_some())
        {
            return Err("Python build receipt lacks its backend configuration".into());
        }
        if receipt.ecosystem == Ecosystem::Pip
            && (!receipt.native_entries.is_empty() || !receipt.native_rejections.is_empty())
        {
            return Err("Python receipt cannot declare Node native entry points".into());
        }
        for path in receipt
            .native_entries
            .iter()
            .chain(receipt.native_rejections.keys())
        {
            crate::workers::valid_path(path)?;
        }
        let policy = RegistryRuntime {
            pack: receipt.runtime.clone(),
            target: receipt.tools.target.clone(),
            python_markers: receipt.python_markers.clone(),
            python_wheel_tags: vec![],
            npm_libc: None,
        };
        let auxiliary = receipt
            .auxiliary_runtime
            .as_ref()
            .map(|runtime| RegistryRuntime {
                pack: runtime.clone(),
                target: receipt.tools.target.clone(),
                python_markers: None,
                python_wheel_tags: vec![],
                npm_libc: None,
            });
        if let Some(runtime) = &receipt.auxiliary_runtime {
            super::validate_runtime(runtime)?;
        }
        receipt.tools.validate(&policy, auxiliary.as_ref())?;
        if receipt.ecosystem == Ecosystem::Npm
            && (receipt.python.is_some()
                || receipt.python_markers.is_some()
                || !receipt.dynamic_requires.is_empty()
                || receipt.graph.is_none())
        {
            return Err("npm build receipt contains Python backend state".into());
        }
        if !receipt.config_settings.is_empty() {
            return Err("unsupported source-build configuration settings".into());
        }
        if let Some(system) = &receipt.python {
            validate_requirements(receipt, system)?;
        }
        if !receipt.inputs.workers.is_empty()
            || !receipt.inputs.packages.is_empty()
            || receipt
                .inputs
                .libs
                .iter()
                .any(|lib| lib.spec.ecosystem != receipt.ecosystem)
        {
            return Err("build input closure contains unrelated environments".into());
        }
        if receipt.output.source.is_some()
            || receipt.sources.iter().any(|source| {
                source
                    .source
                    .as_ref()
                    .is_none_or(|origin| origin.ecosystem != receipt.ecosystem)
            })
        {
            return Err("build receipt lacks exact registry source identity".into());
        }
        if receipt.ecosystem == Ecosystem::Npm {
            let mut sources = receipt
                .inputs
                .libs
                .iter()
                .flat_map(|lib| lib.artifacts.clone())
                .collect::<Vec<_>>();
            sources.sort();
            sources.dedup();
            if sources != receipt.sources
                || receipt.inputs.npm.len() != 1
                || receipt.inputs.npm[0].roots != receipt.roots
                || receipt.inputs.npm[0].build.is_some()
                || !receipt.inputs.builds.is_empty()
            {
                return Err("npm build input graph differs from its original sources".into());
            }
        } else if receipt.sources.len() != 1 || !receipt.inputs.npm.is_empty() {
            return Err("Python build receipt has invalid source inputs".into());
        }
        doctor_at(&receipt.inputs, depth + 1)?;
        // Full graph checks belong to the ordinary lock owner; recursion is
        // bounded above before invoking that owner on a dependency lock.
        let mut dependencies = (*receipt.inputs).clone();
        dependencies.builds.clear();
        for lib in &mut dependencies.libs {
            lib.build = None;
        }
        super::doctor(&dependencies)?;
    }
    Ok(())
}

fn validate_requirements(receipt: &Receipt, system: &PythonSystem) -> Result<(), String> {
    let identifier = |path: &str| {
        !path.is_empty()
            && path.split('.').all(|part| {
                let mut bytes = part.bytes();
                bytes
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                    && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
    };
    let (module, function) = system
        .backend
        .split_once(':')
        .map(|(module, function)| (module, Some(function)))
        .unwrap_or((&system.backend, None));
    if !identifier(module)
        || function.is_some_and(|function| !identifier(function))
        || system.requires.len() > 1024
        || receipt.dynamic_requires.len() > 1024
        || system.backend_path.len() > 64
    {
        return Err("build receipt has an invalid backend or requirement set".into());
    }
    for path in &system.backend_path {
        if path.is_empty() || path.starts_with('/') || path.contains(['\\', '\0']) {
            return Err("invalid backend-path in receipt".into());
        }
        let mut depth = 0usize;
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth = depth.checked_sub(1).ok_or("backend-path escapes source")?;
                }
                _ => depth += 1,
            }
        }
    }
    let markers = receipt
        .python_markers
        .as_ref()
        .ok_or("Python build receipt lacks marker policy")?;
    let requirements = system
        .requires
        .iter()
        .chain(&receipt.dynamic_requires)
        .cloned()
        .collect::<Vec<_>>();
    let document = serde_json::json!({"info":{"requires_dist":requirements}});
    let expected = super::python_dependencies(&document, markers, &[])?;
    let libraries = receipt
        .inputs
        .libs
        .iter()
        .map(|lib| (lib.spec.name.as_str(), lib))
        .collect::<BTreeMap<_, _>>();
    let mut remaining = Vec::new();
    for (name, rule) in expected {
        let lib = libraries
            .get(name.as_str())
            .ok_or("Python build receipt omits a required dependency")?;
        let version = lib
            .spec
            .version
            .parse::<pep440_rs::Version>()
            .map_err(|error| error.to_string())?;
        if !rule.contains(&version) {
            return Err("Python build dependency does not satisfy recorded requirements".into());
        }
        remaining.push(lib.spec.name.as_str());
    }
    let mut reached = BTreeSet::new();
    while let Some(name) = remaining.pop() {
        if !reached.insert(name) {
            continue;
        }
        let lib = libraries
            .get(name)
            .ok_or("build dependency closure is incomplete")?;
        if lib.runtime != receipt.runtime
            || lib
                .artifacts
                .iter()
                .any(|artifact| artifact.target != receipt.tools.target)
        {
            return Err("build dependency runtime or target differs from its receipt".into());
        }
        remaining.extend(lib.dependencies.iter().map(|edge| edge.spec.name.as_str()));
    }
    if reached.len() != libraries.len() {
        return Err("build dependency lock contains unrelated libraries".into());
    }
    Ok(())
}

/// Offline consumers bind recorded marker evaluation to the immutable runtime
/// manifest they have just verified, never to a lock's self-declared policy.
pub fn validate_python_policy(
    lock: &LockFile,
    lib: &super::LockedLib,
    markers: &pep508_rs::MarkerEnvironment,
) -> Result<(), String> {
    if let Some(identity) = &lib.build {
        let receipt = lock
            .builds
            .iter()
            .find(|receipt| receipt.identity().as_ref() == Ok(identity))
            .ok_or("Python build receipt missing")?;
        if receipt.python_markers.as_ref() != Some(markers) {
            return Err("Python build marker policy differs from signed runtime".into());
        }
    }
    Ok(())
}

fn frontend(ecosystem: &Ecosystem) -> Result<String, String> {
    match ecosystem {
        Ecosystem::Pip => Ok(super::sha256(include_bytes!("build/python.py"))),
        Ecosystem::Npm => Ok(super::sha256(
            &[
                include_bytes!("build/npm.js").as_slice(),
                include_bytes!("build/npm/probe.cjs").as_slice(),
            ]
            .concat(),
        )),
        _ => Err("source-build frontend is not available for this ecosystem".into()),
    }
}
