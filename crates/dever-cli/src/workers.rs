//! Offline, per-Adapter execution trees for managed external Workers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use dever_core::hir::{ExternalWorkerContract, Program};
use dever_core::native::EmbeddedResource;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::libs::{self, Ecosystem, LibSpec, LockFile, LockedLib};

mod command;
mod elf;
mod go_build;
pub mod python_wheel;
pub(crate) mod runtime;

pub(crate) const MAX_TREE_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_TREE_FILES: usize = 65_536;
const IGNORED_SOURCE_DIRECTORIES: &[&str] = &["node_modules", "__pycache__", ".git", ".dever"];
const PYTHON_SDK: &[u8] = include_bytes!("../../../sdk/python/dever_component.py");
const JAVASCRIPT_SDK: &[u8] = include_bytes!("../../../sdk/javascript/dever_component.mjs");

const PYTHON_RUNNER: &str = r#"import importlib.util
import json
from pathlib import Path
import sys

base = Path(__file__).resolve().parent
sys.stdout = sys.stderr
sys.path.insert(0, str(base))
from dever_component import Worker, main

sys.path.insert(0, str(base / "runtime" / "lib" / "python__PYTHON_VERSION__" / "site-packages"))
sys.path.insert(0, str(base / "source"))
spec = importlib.util.spec_from_file_location("dever_worker_entry", base / "source" / "__ENTRY__")
if spec is None or spec.loader is None:
    raise RuntimeError("managed Worker entry cannot be loaded")
entry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(entry)
manifest = json.loads((base / "contract.json").read_text(encoding="utf-8"))
handlers = {operation: getattr(entry, operation) for operation in manifest["operations"]}
main(Worker.from_manifest(manifest, handlers))
"#;

const JAVASCRIPT_RUNNER: &str = r#"import { Console } from 'node:console';
import { readFileSync } from 'node:fs';
import { Worker, main } from './dever_component.mjs';

globalThis.console = new Console({ stdout: process.stderr, stderr: process.stderr });
const manifest = JSON.parse(readFileSync(new URL('./contract.json', import.meta.url), 'utf8'));
const entry = await import(new URL('./source/__ENTRY__', import.meta.url));
const handlers = Object.fromEntries(manifest.operations.map(operation => [operation, entry[operation] ?? entry.default?.[operation]]));
await main(Worker.fromManifest(manifest, handlers));
"#;

pub fn prepare(
    project_root: &Path,
    program: &Program,
    resources: &[EmbeddedResource],
) -> Result<Vec<EmbeddedResource>, String> {
    prepare_for_target(
        project_root,
        program,
        resources,
        crate::toolchain::BuildTarget::host()?,
        resources,
    )
}

pub fn prepare_for_target(
    project_root: &Path,
    program: &Program,
    resources: &[EmbeddedResource],
    target: crate::toolchain::BuildTarget,
    build_assets: &[EmbeddedResource],
) -> Result<Vec<EmbeddedResource>, String> {
    let workers = program.external_worker_contracts();
    let commands = program.external_command_contracts();
    if workers.is_empty() && commands.is_empty() {
        return Ok(Vec::new());
    }
    let lock = if workers.iter().any(|worker| worker.ecosystem != "exec") {
        let lock_path = project_root.join("dever.lock");
        if fs::symlink_metadata(&lock_path)
            .map_err(|error| format!("cannot inspect dever.lock: {error}"))?
            .file_type()
            .is_symlink()
        {
            return Err("managed Worker lock file must not be a symbolic link".into());
        }
        let lock = LockFile::decode(
            &fs::read(&lock_path).map_err(|error| format!("cannot read dever.lock: {error}"))?,
        )?;
        libs::doctor(&lock)?;
        Some(lock)
    } else {
        None
    };
    let mut supplied = BTreeMap::new();
    for resource in resources {
        if supplied.insert(resource.path.as_str(), resource).is_some() {
            return Err(format!(
                "duplicate locked Worker resource '{}'",
                resource.path
            ));
        }
        if digest(&resource.bytes) != resource.sha256 {
            return Err(format!(
                "locked Worker resource '{}' has a digest mismatch",
                resource.path
            ));
        }
    }
    let mut output = Vec::new();
    let package_files = crate::packages::owned_files(project_root)?;
    for contract in commands {
        output.extend(command::prepare(
            project_root,
            &contract,
            &package_files,
            &supplied,
            target,
        )?);
    }
    for contract in workers {
        if contract.ecosystem == "exec" {
            output.extend(prepare_exec(&contract, &supplied, target)?);
            continue;
        }
        let lock = lock.as_ref().expect("managed Worker has a lock");
        let locked = lock
            .workers
            .iter()
            .find(|worker| worker.port == contract.port && worker.adapter == contract.adapter)
            .ok_or_else(|| {
                format!(
                    "managed Worker '{}.{}' is not locked",
                    contract.port, contract.adapter
                )
            })?;
        if locked.entry != contract.entry
            || locked.schema != contract.schema
            || locked.ecosystem != contract.ecosystem
            || locked.operations != contract.operations
            || locked.capabilities != contract.capabilities
        {
            return Err(format!(
                "managed Worker '{}.{}' differs from its checked Port",
                contract.port, contract.adapter
            ));
        }
        let requested = contract
            .libs
            .iter()
            .map(|value| value.parse::<LibSpec>())
            .collect::<Result<BTreeSet<_>, _>>()?;
        if requested != locked.libs.iter().cloned().collect() {
            return Err("managed Worker Lib roots differ from its checked Adapter".into());
        }
        output.extend(prepare_worker(
            project_root,
            &contract,
            lock,
            &supplied,
            &package_files,
            target,
            build_assets,
        )?);
    }
    Ok(output)
}

fn prepare_exec(
    contract: &ExternalWorkerContract,
    supplied: &BTreeMap<&str, &EmbeddedResource>,
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<EmbeddedResource>, String> {
    let entry = supplied
        .get(contract.entry.as_str())
        .ok_or("exec Worker is missing from verified resources")?;
    if !entry.executable || entry.bytes.is_empty() {
        return Err("exec Worker entry must be a nonempty executable resource".into());
    }
    runtime::validate_binary(&contract.entry, &entry.bytes, target.platform())?;
    let identity =
        serde_json::to_vec(&contract.sdk_manifest()).map_err(|error| error.to_string())?;
    let tree = format!("workers/{}", digest(&identity));
    let executable = format!("{tree}/worker");
    let manifest = json!({
        "format": "dever-worker-launch-v1", "ecosystem": "exec", "entry": contract.entry,
        "executable": executable, "arguments": [], "working_directory": tree,
    });
    Ok(vec![
        resource(executable, entry.bytes.clone(), true),
        resource(
            format!("{}.dever-worker.json", contract.entry),
            serde_json::to_vec(&manifest).map_err(|error| error.to_string())?,
            false,
        ),
    ])
}

fn prepare_worker(
    project_root: &Path,
    contract: &ExternalWorkerContract,
    lock: &LockFile,
    supplied: &BTreeMap<&str, &EmbeddedResource>,
    package_files: &BTreeMap<String, Vec<u8>>,
    target: crate::toolchain::BuildTarget,
    build_assets: &[EmbeddedResource],
) -> Result<Vec<EmbeddedResource>, String> {
    let ecosystem: Ecosystem = contract.ecosystem.parse()?;
    let identity =
        serde_json::to_vec(&contract.sdk_manifest()).map_err(|error| error.to_string())?;
    let tree = format!("workers/{}", digest(&identity));
    let target = target.platform();
    let runtime_path = format!("lib/runtime/{}/{target}/runtime.pack", ecosystem.as_str());
    let runtime = supplied
        .get(runtime_path.as_str())
        .ok_or_else(|| format!("managed {} runtime pack is missing", ecosystem.as_str()))?;
    let locked_worker = lock
        .workers
        .iter()
        .find(|worker| worker.port == contract.port && worker.adapter == contract.adapter)
        .expect("validated Worker lock identity");
    let locked_runtime = locked_worker
        .runtime
        .as_ref()
        .ok_or("managed Worker lacks a locked runtime pack")?;
    if runtime.sha256 != locked_runtime.sha256 {
        return Err("managed Worker runtime pack differs from its lock".into());
    }
    let files = libs::archive_files("tgz", &runtime.bytes)?;
    let manifest = runtime::validate(&files, &ecosystem, target)?;
    let mut tree_files = Tree::new(tree.clone());
    let runtime_executable = if ecosystem == Ecosystem::Go {
        None
    } else {
        let executable = manifest
            .executable
            .as_deref()
            .expect("validated interpreter executable");
        for (path, bytes) in &files {
            tree_files.add(
                &format!("runtime/{path}"),
                bytes.clone(),
                path == executable,
            )?;
        }
        let path = format!("{tree}/runtime/{executable}");
        Some(path)
    };
    add_adapter_source(project_root, contract, package_files, &mut tree_files)?;
    let runner = match ecosystem {
        Ecosystem::Pip => {
            add_python_dependencies(contract, lock, supplied, &manifest, &mut tree_files)?;
            tree_files.add("dever_component.py", PYTHON_SDK.to_vec(), false)?;
            tree_files.add(
                "runner.py",
                PYTHON_RUNNER
                    .replace("__ENTRY__", entry_name(&contract.entry)?)
                    .replace(
                        "__PYTHON_VERSION__",
                        &manifest
                            .python_markers
                            .as_ref()
                            .expect("validated Python markers")
                            .python_version()
                            .to_string(),
                    )
                    .into_bytes(),
                false,
            )?;
            "runner.py"
        }
        Ecosystem::Npm => {
            ensure_javascript_format(&contract.entry, &mut tree_files)?;
            add_javascript_dependencies(contract, lock, supplied, &manifest, &mut tree_files)?;
            tree_files.add("dever_component.mjs", JAVASCRIPT_SDK.to_vec(), false)?;
            tree_files.add(
                "runner.mjs",
                JAVASCRIPT_RUNNER
                    .replace("__ENTRY__", entry_name(&contract.entry)?)
                    .into_bytes(),
                false,
            )?;
            "runner.mjs"
        }
        Ecosystem::Go => {
            tree_files.add("contract.json", identity.clone(), false)?;
            let binary = go_build::compile_worker(
                contract,
                lock,
                supplied,
                go_build::Inputs {
                    files: &files,
                    manifest: manifest
                        .build
                        .as_ref()
                        .expect("validated Go build descriptor"),
                    sandbox: build_assets,
                },
                &mut tree_files,
                target,
            )?;
            tree_files.remove_prefix("source/");
            let launch = json!({
                "format": "dever-worker-launch-v1", "ecosystem": contract.ecosystem,
                "entry": contract.entry, "executable": format!("{tree}/worker{}", std::env::consts::EXE_SUFFIX),
                "arguments": [], "working_directory": tree,
            });
            let mut output = tree_files.finish();
            output.push(resource(
                format!("{}.dever-worker.json", contract.entry),
                serde_json::to_vec(&launch).map_err(|error| error.to_string())?,
                false,
            ));
            debug_assert_eq!(binary, format!("worker{}", std::env::consts::EXE_SUFFIX));
            return Ok(output);
        }
    };
    tree_files.add("contract.json", identity, false)?;
    let runner_path = format!("{tree}/{runner}");
    let arguments = manifest
        .arguments
        .iter()
        .map(|value| json!({"literal": value}))
        .chain(std::iter::once(json!({"resource": runner_path})))
        .collect::<Vec<_>>();
    let launch = json!({
        "format": "dever-worker-launch-v1", "ecosystem": contract.ecosystem,
        "entry": contract.entry, "executable": runtime_executable.expect("interpreter branch"),
        "arguments": arguments, "working_directory": tree,
    });
    let mut output = tree_files.finish();
    output.push(resource(
        format!("{}.dever-worker.json", contract.entry),
        serde_json::to_vec(&launch).map_err(|error| error.to_string())?,
        false,
    ));
    Ok(output)
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn resource(path: String, bytes: Vec<u8>, executable: bool) -> EmbeddedResource {
    EmbeddedResource {
        sha256: digest(&bytes),
        path,
        bytes,
        executable,
    }
}

pub(crate) fn valid_path(path: &str) -> Result<(), String> {
    let relative = Path::new(path);
    if path.is_empty()
        || path.contains(['\\', ':', '\0'])
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path.split('/').any(str::is_empty)
    {
        return Err(format!(
            "managed Worker path '{path}' is not portable and relative"
        ));
    }
    Ok(())
}

fn entry_name(entry: &str) -> Result<&str, String> {
    valid_path(entry)?;
    let name = Path::new(entry)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid managed Worker entry name")?;
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("managed Worker entry name contains an unsafe character".into());
    }
    Ok(name)
}

fn ensure_javascript_format(entry: &str, tree: &mut Tree) -> Result<(), String> {
    if !entry_name(entry)?.ends_with(".js") {
        return Ok(());
    }
    let path = format!("{}/source/package.json", tree.prefix);
    if let Some(package) = tree.files.get(&path) {
        let metadata: serde_json::Value = serde_json::from_slice(&package.bytes)
            .map_err(|error| format!("invalid Adapter package.json: {error}"))?;
        if !matches!(
            metadata.get("type").and_then(|value| value.as_str()),
            Some("module" | "commonjs")
        ) {
            return Err(
                "managed .js Worker entry requires package.json type module or commonjs".into(),
            );
        }
    } else {
        tree.add(
            "source/package.json",
            b"{\"type\":\"module\"}\n".to_vec(),
            false,
        )?;
    }
    Ok(())
}

fn add_adapter_source(
    project_root: &Path,
    contract: &ExternalWorkerContract,
    package_files: &BTreeMap<String, Vec<u8>>,
    tree: &mut Tree,
) -> Result<(), String> {
    let name = entry_name(&contract.entry)?;
    let allowed: &[&str] = match contract.ecosystem.as_str() {
        "pip" if name.ends_with(".py") => &["py", "pyi", "json"],
        "npm" if name.ends_with(".js") || name.ends_with(".mjs") || name.ends_with(".cjs") => {
            &["js", "mjs", "cjs", "json"]
        }
        "go" if name.ends_with(".go") => &[],
        _ => return Err("managed Worker entry has an unsupported language extension".into()),
    };
    let package_owned = crate::packages::owns_worker(project_root, &contract.entry)?;
    if add_package_adapter_source(&contract.entry, allowed, package_files, tree, package_owned)? {
        return Ok(());
    }
    let entry = libs::owned_worker(project_root, &contract.entry)?;
    let parent = entry
        .parent()
        .ok_or("managed Worker entry has no source directory")?;
    let mut pending = vec![parent.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut children = fs::read_dir(&directory)
            .map_err(|error| format!("cannot read Adapter source: {error}"))?
            .map(|result| {
                result
                    .map(|entry| entry.path())
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        children.sort();
        for path in children {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot inspect Adapter source: {error}"))?;
            if metadata.file_type().is_symlink() {
                return Err("Adapter source contains a symbolic link".into());
            }
            let relative = path
                .strip_prefix(parent)
                .map_err(|error| error.to_string())?
                .to_str()
                .ok_or("Adapter source path is not UTF-8")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            valid_path(&relative)?;
            if metadata.is_dir() {
                if !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| IGNORED_SOURCE_DIRECTORIES.contains(&name))
                {
                    pending.push(path);
                }
            } else if metadata.is_file() {
                if allowed.is_empty()
                    || path
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|extension| allowed.contains(&extension))
                {
                    tree.add(
                        &format!("source/{relative}"),
                        fs::read(&path)
                            .map_err(|error| format!("cannot read Adapter source: {error}"))?,
                        false,
                    )?;
                }
            } else {
                return Err("Adapter source contains a non-regular file".into());
            }
        }
    }
    Ok(())
}

fn add_package_adapter_source(
    entry: &str,
    allowed: &[&str],
    package_files: &BTreeMap<String, Vec<u8>>,
    tree: &mut Tree,
    package_owned: bool,
) -> Result<bool, String> {
    // Ownership comes from the checked Package lock, not from file presence:
    // a missing entire source directory must never enable a local fallback.
    if !package_owned {
        return Ok(false);
    }
    if !package_files.contains_key(entry) {
        return Err(format!("locked Package Worker entry '{entry}' is missing"));
    }
    let parent = Path::new(entry)
        .parent()
        .and_then(|path| path.to_str())
        .ok_or("Package Worker entry has no source directory")?;
    let prefix = format!("{parent}/");
    for (path, bytes) in package_files {
        let Some(relative) = path.strip_prefix(&prefix) else {
            continue;
        };
        valid_path(relative)?;
        if relative
            .split('/')
            .any(|part| IGNORED_SOURCE_DIRECTORIES.contains(&part))
        {
            continue;
        }
        if allowed.is_empty()
            || Path::new(relative)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| allowed.contains(&extension))
        {
            tree.add(&format!("source/{relative}"), bytes.clone(), false)?;
        }
    }
    Ok(true)
}

fn dependency_closure<'a>(
    contract: &ExternalWorkerContract,
    lock: &'a LockFile,
) -> Result<Vec<&'a LockedLib>, String> {
    let mut remaining = contract
        .libs
        .iter()
        .map(|value| value.parse::<LibSpec>())
        .collect::<Result<Vec<_>, _>>()?;
    let mut visited = BTreeSet::new();
    let mut closure = Vec::new();
    while let Some(spec) = remaining.pop() {
        if !visited.insert(spec.clone()) {
            continue;
        }
        if spec.ecosystem.as_str() != contract.ecosystem {
            return Err("Worker Lib ecosystem differs from Adapter".into());
        }
        let lib = lock
            .libs
            .iter()
            .find(|lib| lib.spec == spec)
            .ok_or_else(|| format!("locked Worker Lib {} is missing", spec.key()))?;
        remaining.extend(
            lib.dependencies
                .iter()
                .map(|dependency| dependency.spec.clone()),
        );
        closure.push(lib);
    }
    closure.sort_by(|left, right| left.spec.cmp(&right.spec));
    Ok(closure)
}

fn locked_archive<'a>(
    lib: &LockedLib,
    extension: &str,
    supplied: &'a BTreeMap<&str, &EmbeddedResource>,
    target: &str,
) -> Result<&'a [u8], String> {
    let artifact = lib
        .artifacts
        .iter()
        .find(|artifact| {
            artifact.target == target && artifact.path.ends_with(&format!(".{extension}"))
        })
        .ok_or_else(|| format!("locked {} archive for {target} is missing", lib.spec.key()))?;
    let path = format!(
        "lib/{}/{}/{}/archive.{extension}",
        lib.spec.ecosystem.as_str(),
        digest(lib.spec.distribution_name().as_bytes()),
        lib.spec.version
    );
    if artifact.path != path {
        return Err(format!(
            "locked Worker archive path differs from the {} installation layout",
            lib.spec.key()
        ));
    }
    let resource = supplied
        .get(path.as_str())
        .ok_or_else(|| format!("locked Worker archive '{path}' is missing"))?;
    if resource.sha256 != artifact.sha256 || resource.bytes.len() as u64 != artifact.bytes {
        return Err(format!(
            "locked Worker archive '{path}' differs from its lock"
        ));
    }
    Ok(&resource.bytes)
}

fn add_python_dependencies(
    contract: &ExternalWorkerContract,
    lock: &LockFile,
    supplied: &BTreeMap<&str, &EmbeddedResource>,
    runtime: &runtime::Manifest,
    tree: &mut Tree,
) -> Result<(), String> {
    let mut installed_distributions = BTreeMap::<&str, &LockedLib>::new();
    let mut wheel_files = BTreeSet::new();
    for lib in dependency_closure(contract, lock)? {
        let wheel = python_wheel::Wheel::parse(
            &lib.spec,
            None,
            &runtime.python_wheel_tags,
            locked_archive(lib, "whl", supplied, &runtime.target)?,
        )?;
        wheel.validate_dependencies(
            lib,
            runtime
                .python_markers
                .as_ref()
                .expect("validated Python markers"),
        )?;
        libs::build::validate_python_policy(
            lock,
            lib,
            runtime
                .python_markers
                .as_ref()
                .expect("validated Python markers"),
        )?;
        if let Some(previous) = installed_distributions.insert(lib.spec.distribution_name(), lib) {
            if previous.spec.version != lib.spec.version || previous.artifacts != lib.artifacts {
                return Err("Python extras refer to conflicting distribution artifacts".into());
            }
            continue;
        }
        for file in wheel.install(
            &format!(
                "runtime/{}",
                runtime
                    .executable
                    .as_deref()
                    .expect("validated Python interpreter")
            ),
            &runtime
                .python_markers
                .as_ref()
                .expect("validated Python markers")
                .python_version()
                .to_string(),
        )? {
            wheel_files.insert(file.path.clone());
            tree.add(&file.path, file.bytes, file.executable)?;
        }
    }
    validate_worker_native(
        runtime,
        tree,
        supplied,
        &wheel_files,
        &runtime.python_extension_suffixes,
    )
}

fn validate_worker_native(
    runtime: &runtime::Manifest,
    tree: &Tree,
    supplied: &BTreeMap<&str, &EmbeddedResource>,
    entries: &BTreeSet<String>,
    suffixes: &[String],
) -> Result<(), String> {
    let prefix = format!("{}/", tree.prefix);
    let mut files = tree
        .files
        .iter()
        .map(|(path, resource)| {
            (
                path.strip_prefix(&prefix)
                    .expect("owned Worker tree")
                    .to_owned(),
                resource.bytes.as_slice(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    files.extend(
        supplied
            .iter()
            .filter(|(path, _)| path.starts_with("sandbox/lib/"))
            .map(|(path, resource)| ((*path).to_owned(), resource.bytes.as_slice())),
    );
    python_wheel::validate_native(
        &runtime.target,
        suffixes,
        &format!(
            "runtime/{}",
            runtime
                .executable
                .as_deref()
                .expect("validated interpreter")
        ),
        &files,
        entries,
    )?;
    Ok(())
}

fn add_javascript_dependencies(
    contract: &ExternalWorkerContract,
    lock: &LockFile,
    supplied: &BTreeMap<&str, &EmbeddedResource>,
    runtime: &runtime::Manifest,
    tree: &mut Tree,
) -> Result<(), String> {
    let requested = contract
        .libs
        .iter()
        .map(|value| value.parse())
        .collect::<Result<Vec<LibSpec>, _>>()?;
    let Some(environment) = libs::npm::environment(lock, &requested)? else {
        return Ok(());
    };
    if let Some(receipt) = libs::build::npm_receipt(lock, environment)? {
        if receipt.output.target != runtime.target || receipt.tools.target != runtime.target {
            return Err("locked npm build receipt differs from the selected Worker target".into());
        }
        let resource = supplied
            .get(receipt.output.path.as_str())
            .ok_or("locked npm installation resource is missing")?;
        for (path, bytes, executable) in
            libs::build::npm_installation(lock, environment, &resource.bytes)?
        {
            tree.add(&path, bytes, executable)?;
        }
        let entries = receipt
            .native_entries
            .iter()
            .filter(|path| {
                libs::npm::installed_owner(path)
                    .is_ok_and(|owner| environment.instances.iter().any(|node| node.path == owner))
            })
            .cloned()
            .collect();
        return validate_worker_native(runtime, tree, supplied, &entries, &[]);
    }
    for (path, bytes) in libs::npm::files(environment, lock, |lib| {
        locked_archive(lib, "tgz", supplied, &runtime.target)
    })? {
        tree.add(&path, bytes, false)?;
    }
    Ok(())
}

struct Tree {
    prefix: String,
    files: BTreeMap<String, EmbeddedResource>,
    bytes: usize,
}

impl Tree {
    fn new(prefix: String) -> Self {
        Self {
            prefix,
            files: BTreeMap::new(),
            bytes: 0,
        }
    }

    fn add(&mut self, relative: &str, bytes: Vec<u8>, executable: bool) -> Result<(), String> {
        valid_path(relative)?;
        let total = self
            .bytes
            .checked_add(bytes.len())
            .ok_or("managed Worker tree size overflow")?;
        if total > MAX_TREE_BYTES || self.files.len() >= MAX_TREE_FILES {
            return Err("managed Worker tree exceeds file or byte budget".into());
        }
        let path = format!("{}/{relative}", self.prefix);
        if self.files.contains_key(&path) {
            return Err(format!("managed Worker tree repeats '{path}'"));
        }
        let mut ancestor = path.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if self.files.contains_key(parent) {
                return Err(format!(
                    "managed Worker tree has a file/directory conflict at '{parent}'"
                ));
            }
            ancestor = parent;
        }
        let descendant_prefix = format!("{path}/");
        if self
            .files
            .range(descendant_prefix.clone()..)
            .next()
            .is_some_and(|(existing, _)| existing.starts_with(&descendant_prefix))
        {
            return Err(format!(
                "managed Worker tree has a file/directory conflict at '{path}'"
            ));
        }
        self.bytes = total;
        self.files
            .insert(path.clone(), resource(path, bytes, executable));
        Ok(())
    }

    fn finish(self) -> Vec<EmbeddedResource> {
        self.files.into_values().collect()
    }

    fn remove_prefix(&mut self, relative: &str) {
        let prefix = format!("{}/{relative}", self.prefix);
        self.files.retain(|path, resource| {
            if path.starts_with(&prefix) {
                self.bytes -= resource.bytes.len();
                false
            } else {
                true
            }
        });
    }
}
