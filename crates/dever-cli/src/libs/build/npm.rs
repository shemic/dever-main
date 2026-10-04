//! Registry lifecycle scripts use the signed npm frontend and exact instance graph.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Cursor;

use super::{Product, Receipt, Session};
use crate::libs::{
    Ecosystem, LockFile, LockedArtifact, LockedLib, RegistryResolver, RegistryRuntime, npm, sha256,
};

mod environment;
use environment::{Environment, Output};

impl Session<'_> {
    pub(crate) fn npm(
        &self,
        resolver: &RegistryResolver<'_>,
        libraries: &mut Vec<LockedLib>,
        graph: &mut npm::Environment,
        runtime: &RegistryRuntime,
    ) -> Result<(), String> {
        let mut lock = LockFile::new(libraries.clone())?;
        lock.npm.push(graph.clone());
        let archives = libraries
            .iter()
            .map(|lib| {
                let artifact = lib.artifacts.first().ok_or("npm source archive missing")?;
                Ok((
                    lib.spec.clone(),
                    resolver.store.verify_exact(
                        &artifact.sha256,
                        artifact.bytes,
                        &artifact.target,
                    )?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let files = npm::build_files(graph, &lock, |lib| {
            archives
                .get(&lib.spec)
                .map(Vec::as_slice)
                .ok_or("npm source archive missing".into())
        })?;
        if !npm::requires_build(&files)? {
            return Ok(());
        }
        let mut sources = libraries
            .iter()
            .flat_map(|lib| lib.artifacts.clone())
            .collect::<Vec<_>>();
        sources.sort();
        sources.dedup();
        let key = format!(
            "npm:{}:{}:{}",
            sha256(&serde_json::to_vec(graph).map_err(|error| error.to_string())?),
            runtime.pack.sha256,
            sha256(&serde_json::to_vec(&sources).map_err(|error| error.to_string())?)
        );
        if let Some(product) = self.products.borrow().get(&key) {
            *graph = product
                .receipt
                .graph
                .clone()
                .ok_or("cached npm graph missing")?;
            graph.build = Some(product.receipt.identity()?);
            retain_archives(libraries, graph);
            return Ok(());
        }
        let executables = npm::build_executables(graph, &archives)?;
        let environment = Environment::new(self.inputs, runtime)?;
        let output = environment.run(self.inputs, graph, &files, &executables)?;
        prune_failed(graph, &output.failed)?;
        retain_archives(libraries, graph);
        npm::validate_built_files(graph, &output.files)?;
        let bytes = archive(&output)?;
        let digest = resolver.store.publish(&bytes, &runtime.target)?;
        if digest != sha256(&bytes) {
            return Err("built npm cache identity mismatch".into());
        }
        let output_artifact = LockedArtifact {
            target: runtime.target.clone(),
            path: format!("build/npm/{digest}/installation.tgz"),
            bytes: bytes.len() as u64,
            sha256: digest,
        };
        let receipt = Receipt {
            ecosystem: Ecosystem::Npm,
            roots: graph.roots.clone(),
            sources,
            graph: Some(graph.clone()),
            runtime: runtime.pack.clone(),
            auxiliary_runtime: Some(environment.python_runtime),
            native_entries: output.native,
            native_rejections: output.rejections,
            python_markers: None,
            tools: environment.descriptor,
            frontend: super::frontend(&Ecosystem::Npm)?,
            python: None,
            dynamic_requires: vec![],
            dependencies: Box::new(LockFile::new(vec![])?),
            config_settings: BTreeMap::new(),
            output: output_artifact,
        };
        graph.build = Some(receipt.identity()?);
        self.remember(
            key,
            Product {
                bytes,
                filename: "installation.tgz".into(),
                receipt,
            },
        )?;
        Ok(())
    }
}

fn archive(output: &Output) -> Result<Vec<u8>, String> {
    let encoder = flate2::GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (path, bytes) in &output.files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(if output.executables.contains(path) {
            0o755
        } else {
            0o644
        });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive
            .append_data(&mut header, path, Cursor::new(bytes))
            .map_err(|error| error.to_string())?;
    }
    archive
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(|error| error.to_string())
}

fn retain_archives(libraries: &mut Vec<LockedLib>, graph: &npm::Environment) {
    let used = graph
        .instances
        .iter()
        .map(|node| &node.source.archive)
        .collect::<BTreeSet<_>>();
    libraries.retain(|lib| used.contains(&lib.spec));
}

fn required_nodes(graph: &npm::Environment) -> Result<BTreeSet<String>, String> {
    reachable(graph, true)
}

fn reachable(graph: &npm::Environment, mandatory_only: bool) -> Result<BTreeSet<String>, String> {
    let mut remaining = graph
        .roots
        .iter()
        .map(|root| format!("node_modules/{}", root.name))
        .collect::<VecDeque<_>>();
    let mut reached = BTreeSet::new();
    while let Some(path) = remaining.pop_front() {
        if !reached.insert(path.clone()) {
            continue;
        }
        let node = graph
            .instances
            .iter()
            .find(|node| node.path == path)
            .ok_or("npm reachable instance missing")?;
        remaining.extend(
            node.edges
                .iter()
                .filter(|edge| {
                    !mandatory_only || matches!(edge.kind, npm::Kind::Dependency | npm::Kind::Peer)
                })
                .filter_map(|edge| edge.target.clone()),
        );
    }
    Ok(reached)
}

fn prune_failed(graph: &mut npm::Environment, failed: &BTreeSet<String>) -> Result<(), String> {
    if failed
        .iter()
        .any(|path| !graph.instances.iter().any(|node| &node.path == path))
    {
        return Err("npm build reported an unknown failed instance".into());
    }
    let mut failed = failed.clone();
    loop {
        let before = failed.len();
        for node in &graph.instances {
            if node.edges.iter().any(|edge| {
                matches!(edge.kind, npm::Kind::Dependency | npm::Kind::Peer)
                    && edge
                        .target
                        .as_ref()
                        .is_some_and(|path| failed.contains(path))
            }) {
                failed.insert(node.path.clone());
            }
        }
        if before == failed.len() {
            break;
        }
    }
    if !required_nodes(graph)?.is_disjoint(&failed) {
        return Err("required npm lifecycle failed".into());
    }
    graph.instances.retain(|node| !failed.contains(&node.path));
    for node in &mut graph.instances {
        for edge in &mut node.edges {
            if edge
                .target
                .as_ref()
                .is_some_and(|path| failed.contains(path))
            {
                edge.target = None;
                edge.omission = Some(npm::Omission::BuildFailure);
            }
        }
    }
    let reached = reachable(graph, false)?;
    graph.instances.retain(|node| reached.contains(&node.path));
    Ok(())
}
