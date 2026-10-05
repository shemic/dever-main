//! Exact restoration is a preparation operation, never a dependency resolver.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use super::{
    ArtifactStore, FixtureRegistry, LockFile, LockedArtifact, RegistryTransport, build, sumdb,
};

pub(super) fn install(
    project_root: &Path,
    target: crate::toolchain::BuildTarget,
) -> Result<super::PreparationReport, String> {
    super::reject_declaration_symlinks(project_root)?;
    let lock_bytes = fs::read(project_root.join("dever.lock"))
        .map_err(|error| format!("dever lib install requires an existing dever.lock: {error}"))?;
    let setting_bytes = setting_snapshot(project_root)?;
    let lock = LockFile::decode(&lock_bytes)?;
    super::doctor(&lock)?;
    validate_target(&lock, target.platform())?;
    let fixtures = FixtureRegistry::builtin();
    if lock.packages.is_empty()
        && lock
            .libs
            .iter()
            .all(|lib| fixtures.package(&lib.spec).is_some())
        && lock.workers.iter().all(|worker| worker.ecosystem == "exec")
    {
        let store = super::FixtureArtifactStore::default();
        validate_project(project_root, &lock, &store, target, false)?;
        restore_fixtures(&lock, &store)?;
    } else {
        let inputs = super::PreparingRegistry(super::InstalledRegistry::current(target)?);
        let store = super::ManagedArtifactStore {
            layout: inputs.0.layout.clone(),
        };
        crate::packages::restore_locked(project_root, &lock, &store)?;
        validate_project(project_root, &lock, &store, target, true)?;
        for worker in lock
            .workers
            .iter()
            .filter(|worker| worker.ecosystem != "exec")
        {
            let (runtime, _) = build::Inputs::runtime(&inputs, worker.ecosystem.parse()?)?;
            if worker.runtime.as_ref() != Some(&runtime.pack) {
                return Err("locked Worker runtime differs from signed release".into());
            }
        }
        if target != crate::toolchain::BuildTarget::host()? && !lock.workers.is_empty() {
            inputs.ensure(crate::toolchain::ExtensionKind::Target)?;
        }
        restore_artifacts(
            &lock,
            &super::HttpRegistry::official(),
            &store,
            &inputs,
            target.platform(),
            &sumdb::Verifier::official(),
        )?;
        super::embedded_resources_for_target(project_root, target)?;
    }
    if fs::read(project_root.join("dever.lock")).map_err(|error| error.to_string())? != lock_bytes
        || setting_snapshot(project_root)? != setting_bytes
    {
        return Err("project setting or lock changed during exact restoration".into());
    }
    let artifacts = lock
        .libs
        .iter()
        .flat_map(|lib| &lib.artifacts)
        .chain(build::npm_outputs(&lock).map(|receipt| &receipt.output))
        .collect::<BTreeSet<_>>();
    Ok(super::PreparationReport {
        libs: lock.libs.len(),
        artifacts: artifacts.len(),
        runtimes: lock
            .libs
            .iter()
            .map(|lib| &lib.runtime)
            .chain(
                lock.workers
                    .iter()
                    .filter_map(|worker| worker.runtime.as_ref()),
            )
            .collect::<BTreeSet<_>>()
            .len(),
        artifact_bytes: artifacts.iter().map(|artifact| artifact.bytes).sum(),
    })
}

fn setting_snapshot(project_root: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(project_root.join("config/setting.json")) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read project setting snapshot: {error}")),
    }
}

fn validate_project(
    project_root: &Path,
    lock: &LockFile,
    store: &dyn ArtifactStore,
    target: crate::toolchain::BuildTarget,
    prepare: bool,
) -> Result<(), String> {
    let sources = crate::packages::sources_with(project_root, store)?;
    let workers = super::unbound_source_workers(project_root, &sources)?;
    let mut unbound = lock.clone();
    for worker in &mut unbound.workers {
        worker.runtime = None;
    }
    super::validate_worker_bindings(&unbound, &workers)?;
    let mut roots = super::read_declarations(project_root)?;
    roots.extend(crate::packages::declared_libs_with(project_root, store)?);
    if super::retain_locked_libs(lock, &roots, lock.workers.clone())? != *lock {
        return Err("dever.lock contains Lib entries or environments outside the declared closure; run 'dever lib update <project-root>'".into());
    }
    let workers = super::bind_workers(workers, target, prepare)?;
    super::validate_worker_bindings(lock, &workers)?;
    Ok(())
}

fn validate_target(lock: &LockFile, target: &str) -> Result<(), String> {
    for artifact in lock.libs.iter().flat_map(|lib| &lib.artifacts).chain(
        lock.builds
            .iter()
            .flat_map(|receipt| receipt.sources.iter().chain([&receipt.output])),
    ) {
        if artifact.target != target && artifact.target != "host" {
            return Err(format!(
                "locked artifact targets {}, not {target}; install never switches lock targets",
                artifact.target
            ));
        }
    }
    for receipt in &lock.builds {
        validate_target(&receipt.inputs, target)?;
    }
    Ok(())
}

fn restore_fixtures(lock: &LockFile, store: &dyn ArtifactStore) -> Result<(), String> {
    let fixtures = FixtureRegistry::builtin();
    for lib in &lock.libs {
        if let Some(package) = fixtures.package(&lib.spec) {
            if package.locked() != *lib {
                return Err("locked fixture metadata differs from builtin package".into());
            }
            for artifact in &package.artifacts {
                let expected = super::sha256(&artifact.bytes);
                if store.publish(&artifact.bytes, &artifact.target)? != expected {
                    return Err("fixture artifact cache identity mismatch".into());
                }
            }
        }
    }
    Ok(())
}

/// Shared by production preparation and isolated registry fixtures. Every
/// network request names the recorded archive; no metadata/version query runs.
pub fn restore_artifacts(
    lock: &LockFile,
    transport: &dyn RegistryTransport,
    store: &dyn ArtifactStore,
    inputs: &dyn build::Inputs,
    target: &str,
    verifier: &sumdb::Verifier,
) -> Result<(), String> {
    super::doctor_with_go_verifier(lock, verifier)?;
    validate_target(lock, target)?;
    let session = build::Session::new(inputs);
    restore_at(lock, transport, store, inputs, &session)?;
    super::verify_locked_artifacts_with_go_verifier(lock, store, target, verifier)?;
    Ok(())
}

fn restore_at(
    lock: &LockFile,
    transport: &dyn RegistryTransport,
    store: &dyn ArtifactStore,
    inputs: &dyn build::Inputs,
    session: &build::Session<'_>,
) -> Result<(), String> {
    restore_fixtures(lock, store)?;
    let fixtures = FixtureRegistry::builtin();
    for lib in &lock.libs {
        if fixtures.package(&lib.spec).is_some() {
            continue;
        }
        let (runtime, _) = inputs.runtime(lib.spec.ecosystem.clone())?;
        if runtime.pack != lib.runtime
            || lib
                .artifacts
                .iter()
                .any(|artifact| artifact.target != runtime.target)
        {
            return Err(format!(
                "{} locked runtime differs from signed release",
                lib.spec.key()
            ));
        }
        if let Some(markers) = &runtime.python_markers {
            build::validate_python_policy(lock, lib, markers)?;
        }
        for artifact in &lib.artifacts {
            if artifact
                .source
                .as_ref()
                .is_some_and(|source| source.ecosystem != lib.spec.ecosystem)
            {
                return Err("locked registry source ecosystem differs from its Lib".into());
            }
            if artifact.source.is_some() {
                restore_archive(artifact, transport, store)?;
            } else if lib.build.is_none() {
                return Err(format!("{} lacks an exact registry source", lib.spec.key()));
            }
        }
    }
    for receipt in &lock.builds {
        if cached(&receipt.output, store)? {
            continue;
        }
        restore_at(&receipt.inputs, transport, store, inputs, session)?;
        for source in &receipt.sources {
            restore_archive(source, transport, store)?;
        }
        session.replay(receipt, store)?;
    }
    Ok(())
}

fn cached(artifact: &LockedArtifact, store: &dyn ArtifactStore) -> Result<bool, String> {
    match store.get_exact(&artifact.sha256, artifact.bytes, &artifact.target)? {
        Some(bytes) => {
            validate_bytes(artifact, &bytes)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn validate_bytes(artifact: &LockedArtifact, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != artifact.bytes || super::sha256(bytes) != artifact.sha256 {
        return Err(format!(
            "locked archive '{}' failed exact byte/hash verification",
            artifact.path
        ));
    }
    Ok(())
}

fn restore_archive(
    artifact: &LockedArtifact,
    transport: &dyn RegistryTransport,
    store: &dyn ArtifactStore,
) -> Result<(), String> {
    if cached(artifact, store)? {
        return Ok(());
    }
    let source = artifact
        .source
        .as_ref()
        .ok_or("locked archive source is absent")?;
    let limit =
        usize::try_from(artifact.bytes).map_err(|_| "locked archive exceeds address space")?;
    if limit > 64 * 1024 * 1024 {
        return Err("locked registry archive exceeds byte budget".into());
    }
    let bytes = transport.get(source.ecosystem.clone(), &source.locator, limit)?;
    validate_bytes(artifact, &bytes)?;
    if store.publish(&bytes, &artifact.target)? != artifact.sha256 {
        return Err("restored archive cache identity mismatch".into());
    }
    Ok(())
}
