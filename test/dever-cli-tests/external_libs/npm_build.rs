use super::*;
use dever_cli::libs::build::{Inputs, NativeRejection, Receipt, Session, pack};
use dever_cli::libs::{LibResolver, npm};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    runtime: RegistryRuntime,
    runtime_file: String,
    python: RegistryRuntime,
    python_file: String,
    build: pack::Descriptor,
    build_file: String,
}

struct AuthorInputs {
    root: PathBuf,
    settings: Settings,
}

#[test]
fn npm_lifecycle_output_preserves_locked_identity_and_instance_ownership() {
    let environment = npm::Environment::single("npm:app@1.0.0".parse().unwrap());
    let files = vec![
        (
            "node_modules/app/package.json".into(),
            br#"{"name":"app","version":"1.0.0"}"#.to_vec(),
        ),
        (
            "node_modules/app/generated.js".into(),
            b"module.exports=42;".to_vec(),
        ),
    ];
    npm::validate_built_files(&environment, &files).unwrap();
    let mut changed = files.clone();
    changed[0].1 =
        br#"{"name":"app","version":"1.0.0","dependencies":{"injected":"1.0.0"}}"#.to_vec();
    assert!(npm::validate_built_files(&environment, &changed).is_err());
    let mut changed = files.clone();
    changed.push(("node_modules/injected/index.js".into(), vec![]));
    assert!(npm::validate_built_files(&environment, &changed).is_err());
    let mut changed = files.clone();
    changed.push(("node_modules/app/../escape".into(), vec![]));
    assert!(npm::validate_built_files(&environment, &changed).is_err());
    assert!(npm::validate_built_files(&environment, &files[1..]).is_err());
}

#[test]
fn npm_registry_install_hooks_do_not_run_publishing_lifecycle() {
    let metadata = |script: &str| {
        vec![(
            "node_modules/app/package.json".into(),
            serde_json::to_vec(&serde_json::json!({"scripts":{script:"node command.js"}})).unwrap(),
        )]
    };
    for script in ["preinstall", "install", "postinstall"] {
        assert!(npm::requires_build(&metadata(script)).unwrap());
    }
    for script in ["prepare", "prepack", "prepublish"] {
        assert!(!npm::requires_build(&metadata(script)).unwrap());
    }
    assert!(
        npm::requires_build(&[("node_modules/app/binding.gyp".into(), b"{}".to_vec())]).unwrap()
    );
}

fn locked_installation_fixture() -> (LockFile, Vec<u8>, FixtureArtifactStore) {
    let mut registry = LocalRegistry(BTreeMap::new());
    npm_tests::release(
        &mut registry,
        "app",
        "1.0.0",
        serde_json::json!({}),
        &[(
            "package/prebuilds/foreign/addon.node",
            b"foreign-object".to_vec(),
        )],
    );
    npm_tests::release(&mut registry, "other", "1.0.0", serde_json::json!({}), &[]);
    let store = FixtureArtifactStore::default();
    let mut lock = local_resolver(&registry, &store)
        .resolve(&[
            "npm:app@1.0.0".parse().unwrap(),
            "npm:other@1.0.0".parse().unwrap(),
        ])
        .unwrap();
    let mut sources: Vec<_> = lock
        .libs
        .iter()
        .flat_map(|lib| lib.artifacts.clone())
        .collect();
    sources.sort();
    sources.dedup();
    let archives = lock
        .libs
        .iter()
        .map(|lib| {
            (
                lib.spec.clone(),
                store
                    .verify_exact(
                        &lib.artifacts[0].sha256,
                        lib.artifacts[0].bytes,
                        &lib.artifacts[0].target,
                    )
                    .unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let files = npm::build_files(
        &lock.npm[0],
        &lock,
        |lib| Ok(archives[&lib.spec].as_slice()),
    )
    .unwrap();
    let mut writer = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        writer
            .append_data(&mut header, path, bytes.as_slice())
            .unwrap();
    }
    let bytes = writer.into_inner().unwrap().finish().unwrap();
    let runtime = registry_runtime(Ecosystem::Npm);
    let python = registry_runtime(Ecosystem::Pip);
    let hash = digest(&bytes);
    let receipt = Receipt {
        ecosystem: Ecosystem::Npm,
        roots: lock.npm[0].roots.clone(),
        sources,
        graph: Some(lock.npm[0].clone()),
        runtime: runtime.pack.clone(),
        auxiliary_runtime: Some(python.pack.clone()),
        native_entries: Default::default(),
        native_rejections: BTreeMap::from([(
            "node_modules/app/prebuilds/foreign/addon.node".into(),
            NativeRejection::Format,
        )]),
        python_markers: None,
        tools: pack::Descriptor {
            format: "dever-registry-build-v1".into(),
            ecosystem: Ecosystem::Npm,
            target: runtime.target.clone(),
            runtime_sha256: runtime.pack.sha256.clone(),
            python_runtime_sha256: Some(python.pack.sha256),
            sha256: "a".repeat(64),
        },
        frontend: digest(
            &[
                include_bytes!("../../../crates/dever-cli/src/libs/build/npm.js").as_slice(),
                include_bytes!("../../../crates/dever-cli/src/libs/build/npm/probe.cjs").as_slice(),
            ]
            .concat(),
        ),
        python: None,
        dynamic_requires: vec![],
        inputs: Box::new(lock.clone()),
        config_settings: BTreeMap::new(),
        output: dever_cli::libs::LockedArtifact {
            source: None,
            target: runtime.target,
            path: format!("build/npm/{hash}/installation.tgz"),
            bytes: bytes.len() as u64,
            sha256: hash,
        },
    };
    lock.npm[0].build = Some(receipt.identity().unwrap());
    lock.builds.push(receipt);
    doctor(&lock).unwrap();
    (lock, bytes, store)
}

#[test]
fn cached_npm_build_restore_does_not_load_build_tools_or_auxiliary_runtime() {
    let (lock, bytes, store) = locked_installation_fixture();
    store
        .publish(&bytes, &lock.builds[0].output.target)
        .unwrap();
    dever_cli::libs::restore::restore_artifacts(
        &lock,
        &LocalRegistry(BTreeMap::new()),
        &store,
        &super::restore_tests::RuntimeOnly,
        "linux-x86_64",
        &sumdb_tests::verifier(),
    )
    .unwrap();
}

#[test]
fn npm_expanded_outputs_are_pruned_only_when_no_raw_consumer_remains() {
    use dever_cli::libs::{LockedWorker, prune_expanded_resources};
    use dever_core::native::EmbeddedResource;

    let (mut lock, bytes, _) = locked_installation_fixture();
    let roots = lock.npm[0].roots.clone();
    let other = vec!["npm:other@1.0.0".parse().unwrap()];
    lock.npm = npm::retain(&lock, &[roots.clone(), other.clone()]).unwrap();
    let output = &lock.builds[0].output;
    let managed = LockedWorker {
        port: "sample.worker".into(),
        adapter: "default".into(),
        ecosystem: "npm".into(),
        runtime: Some(registry_runtime(Ecosystem::Npm).pack),
        entry: "worker/main.cjs".into(),
        schema: "checked".into(),
        capabilities: vec![],
        operations: vec!["probe".into()],
        libs: roots.clone(),
    };
    let resource = EmbeddedResource {
        path: output.path.clone(),
        bytes,
        sha256: output.sha256.clone(),
        executable: false,
    };
    let mut resources = vec![resource.clone()];
    prune_expanded_resources(&lock, std::slice::from_ref(&managed), &[], &mut resources).unwrap();
    assert!(
        resources.is_empty(),
        "expanded installation archive leaked into program"
    );

    // A projected environment shares its original build output with the full
    // Worker. Retention follows current consumers, not the receipt's old roots.
    for declared in [roots, other] {
        let mut resources = vec![resource.clone()];
        prune_expanded_resources(
            &lock,
            std::slice::from_ref(&managed),
            &declared,
            &mut resources,
        )
        .unwrap();
        assert_eq!(resources.len(), 1);
    }
    let mut exec = managed.clone();
    exec.ecosystem = "exec".into();
    exec.runtime = None;
    let mut resources = vec![resource];
    prune_expanded_resources(&lock, &[managed, exec], &[], &mut resources).unwrap();
    assert_eq!(resources.len(), 1);
}

#[test]
fn npm_native_admission_accounts_every_addon_and_prunes_unsupported_objects() {
    let (lock, bytes, _) = locked_installation_fixture();
    let files = dever_cli::libs::build::npm_installation(&lock, &lock.npm[0], &bytes).unwrap();
    assert!(files.iter().all(|(path, _, _)| !path.ends_with(".node")));
    let mut missing = lock.clone();
    missing.builds[0].native_rejections.clear();
    missing.npm[0].build = Some(missing.builds[0].identity().unwrap());
    assert!(
        dever_cli::libs::build::npm_installation(&missing, &missing.npm[0], &bytes)
            .unwrap_err()
            .contains("exact addon files")
    );
    let mut corrupted = bytes.clone();
    corrupted[10] ^= 1;
    assert!(dever_cli::libs::build::npm_installation(&lock, &lock.npm[0], &corrupted).is_err());
    for change in ["tools", "graph", "runtime"] {
        let mut changed = lock.clone();
        match change {
            "tools" => changed.builds[0].tools.sha256 = "not-a-hash".into(),
            "graph" => changed.builds[0].graph.as_mut().unwrap().instances.clear(),
            _ => changed.builds[0].runtime.sha256 = "b".repeat(64),
        }
        changed.npm[0].build = Some(changed.builds[0].identity().unwrap());
        assert!(
            doctor(&changed).is_err(),
            "accepted internally inconsistent {change}"
        );
    }
}

#[test]
fn npm_locked_output_projects_retained_roots_without_rerunning_hooks() {
    let (lock, bytes, _) = locked_installation_fixture();
    let project = TemporaryDirectory::new();
    fs::create_dir(project.path().join("config")).unwrap();
    fs::write(
        project.path().join("config/setting.json"),
        br#"{"lib":["npm:app@1.0.0","npm:other@1.0.0"]}"#,
    )
    .unwrap();
    lock.write_atomic(project.path()).unwrap();
    dever_cli::libs::execute("remove", project.path(), &["npm:app@1.0.0".into()]).unwrap();
    let retained = LockFile::decode(&fs::read(project.path().join("dever.lock")).unwrap()).unwrap();
    doctor(&retained).unwrap();
    let files =
        dever_cli::libs::build::npm_installation(&retained, &retained.npm[0], &bytes).unwrap();
    assert_eq!(retained.builds, lock.builds);
    assert!(
        files
            .iter()
            .all(|(path, _, _)| path.starts_with("node_modules/other/"))
    );
}

impl AuthorInputs {
    fn load() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/npm-build");
        let settings =
            serde_json::from_slice(&fs::read(root.join("config/setting.json")).unwrap()).unwrap();
        Self { root, settings }
    }
}

impl Inputs for AuthorInputs {
    fn runtime(&self, ecosystem: Ecosystem) -> Result<(RegistryRuntime, Vec<u8>), String> {
        let (runtime, path) = match ecosystem {
            Ecosystem::Pip => (&self.settings.python, &self.settings.python_file),
            Ecosystem::Npm => (&self.settings.runtime, &self.settings.runtime_file),
            _ => return Err("fixture only supplies Node and auxiliary Python".into()),
        };
        let bytes = fs::read(self.root.join(path)).map_err(|error| error.to_string())?;
        if digest(&bytes) != runtime.pack.sha256 {
            return Err("author runtime drift".into());
        }
        Ok((runtime.clone(), bytes))
    }
    fn tools(&self, ecosystem: Ecosystem) -> Result<(pack::Descriptor, Vec<u8>), String> {
        if ecosystem != Ecosystem::Npm {
            return Err("fixture only supplies npm tools".into());
        }
        Ok((
            self.settings.build.clone(),
            fs::read(self.root.join(&self.settings.build_file))
                .map_err(|error| error.to_string())?,
        ))
    }
    fn assets(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        Ok(build_tests::sandbox::assets(&workspace)
            .into_iter()
            .map(|asset| {
                (
                    asset.path.trim_start_matches("sandbox/").into(),
                    asset.bytes,
                )
            })
            .collect())
    }
    fn diagnostic(&self, output: &[u8]) {
        eprintln!("{}", String::from_utf8_lossy(output));
    }
}

#[test]
#[ignore = "explicit author operation: assembles hashed npm/compiler/header inputs; no network or host execution"]
fn prepare_npm_build_tool_fixture() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime = |name: &str| {
        let root = workspace.join(format!("target/{name}-managed"));
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("config/setting.json")).unwrap()).unwrap();
        let runtime: RegistryRuntime = serde_json::from_value(config["runtime"].clone()).unwrap();
        assert_eq!(
            digest(&fs::read(root.join("runtime.pack")).unwrap()),
            runtime.pack.sha256
        );
        runtime
    };
    let node = runtime("npm");
    let python = runtime("python");
    let root = workspace.join("target/npm-build");
    let build = build_tests::prepare_tool_pack(Ecosystem::Npm, &node, Some(&python), &root);
    let settings = Settings {
        runtime: node,
        runtime_file: "../npm-managed/runtime.pack".into(),
        python,
        python_file: "../python-managed/runtime.pack".into(),
        build,
        build_file: "build.pack".into(),
    };
    fs::write(
        root.join("config/setting.json"),
        serde_json::to_vec_pretty(&settings).unwrap(),
    )
    .unwrap();
}

fn resolver<'a>(
    inputs: &AuthorInputs,
    session: &'a Session<'a>,
    transport: &'a dyn RegistryTransport,
    store: &'a FixtureArtifactStore,
) -> RegistryResolver<'a> {
    RegistryResolver {
        transport,
        store,
        runtimes: BTreeMap::from([(Ecosystem::Npm, inputs.settings.runtime.clone())]),
        go_sumdb: dever_cli::libs::sumdb::ChecksumDatabase::official(None).unwrap(),
        build: Some(session),
    }
}

#[test]
#[ignore = "explicit official npm native source build; network only in registry resolver, never in lifecycle sandbox"]
fn builds_official_npm_addon_with_locked_lifecycle() {
    let inputs = AuthorInputs::load();
    let session = Session::new(&inputs);
    let transport = dever_cli::libs::HttpRegistry::official();
    let store = FixtureArtifactStore::default();
    let resolver = resolver(&inputs, &session, &transport, &store);
    let roots = vec!["npm:bufferutil@4.0.9".parse().unwrap()];
    let lock = resolver.resolve(&roots).unwrap();
    doctor(&lock).unwrap();
    assert_eq!(lock.builds.len(), 1);
    let receipt = &lock.builds[0];
    assert!(
        receipt
            .native_entries
            .contains("node_modules/bufferutil/build/Release/bufferutil.node")
    );
    let bytes = store
        .verify_exact(
            &receipt.output.sha256,
            receipt.output.bytes,
            &receipt.output.target,
        )
        .unwrap();
    let files = dever_cli::libs::build::npm_installation(&lock, &lock.npm[0], &bytes).unwrap();
    assert!(files.iter().any(
        |(path, bytes, _)| path.ends_with("/build/Release/bufferutil.node")
            && bytes.starts_with(b"\x7fELF")
    ));
    assert_eq!(lock, resolver.resolve(&roots).unwrap());
    publish_fixture_artifacts(&inputs.root.join("dependency"), &lock, &store);
}

#[test]
#[ignore = "explicit managed npm hooks; uses local registry and real signed author runtime/tools"]
fn npm_lifecycle_preserves_optional_failures_bins_and_offline_remove() {
    let inputs = AuthorInputs::load();
    let session = Session::new(&inputs);
    let mut transport = LocalRegistry(BTreeMap::new());
    npm_tests::release(&mut transport, "helper", "1.0.0", serde_json::json!({"bin":{"build-probe":"cli.js"}}), &[("package/cli.js", b"#!/usr/bin/env node\nrequire('fs').writeFileSync('generated.js','module.exports = 42;');\n".to_vec())]);
    npm_tests::release(
        &mut transport,
        "broken",
        "1.0.0",
        serde_json::json!({"dependencies":{"cycle":"1.0.0"},"scripts":{"install":"node -e 'process.exit(7)'"}}),
        &[],
    );
    npm_tests::release(
        &mut transport,
        "cycle",
        "1.0.0",
        serde_json::json!({"dependencies":{"broken":"1.0.0"}}),
        &[],
    );
    npm_tests::release(
        &mut transport,
        "app",
        "1.0.0",
        serde_json::json!({"dependencies":{"helper":"1.0.0"},"optionalDependencies":{"broken":"1.0.0"},"scripts":{"postinstall":"build-probe"}}),
        &[],
    );
    npm_tests::release(&mut transport, "other", "1.0.0", serde_json::json!({}), &[]);
    let store = FixtureArtifactStore::default();
    let resolver = resolver(&inputs, &session, &transport, &store);
    let lock = resolver
        .resolve(&[
            "npm:app@1.0.0".parse().unwrap(),
            "npm:other@1.0.0".parse().unwrap(),
        ])
        .unwrap();
    doctor(&lock).unwrap();
    assert!(
        !lock
            .libs
            .iter()
            .any(|lib| matches!(lib.spec.name.as_str(), "broken" | "cycle"))
    );
    assert!(
        lock.npm[0]
            .instances
            .iter()
            .flat_map(|node| &node.edges)
            .any(|edge| edge.omission == Some(npm::Omission::BuildFailure))
    );
    let receipt = &lock.builds[0];
    assert!(
        receipt
            .inputs
            .libs
            .iter()
            .any(|lib| lib.spec.name == "broken")
    );
    assert!(
        receipt
            .inputs
            .libs
            .iter()
            .any(|lib| lib.spec.name == "cycle")
    );
    assert!(receipt.inputs.npm[0].build.is_none());
    let cold = FixtureArtifactStore::default();
    let before = lock.encode().unwrap();
    dever_cli::libs::restore::restore_artifacts(
        &lock,
        &super::restore_tests::archives_only(&lock, &transport),
        &cold,
        &inputs,
        &inputs.settings.runtime.target,
        &dever_cli::libs::sumdb::Verifier::official(),
    )
    .unwrap();
    assert_eq!(lock.encode().unwrap(), before);
    let bytes = store
        .verify_exact(
            &receipt.output.sha256,
            receipt.output.bytes,
            &receipt.output.target,
        )
        .unwrap();
    let files = dever_cli::libs::build::npm_installation(&lock, &lock.npm[0], &bytes).unwrap();
    assert!(
        files
            .iter()
            .any(|(path, bytes, _)| path == "node_modules/app/generated.js"
                && bytes == b"module.exports = 42;")
    );
    assert!(
        resolver
            .resolve(&["npm:broken@1.0.0".parse().unwrap()])
            .unwrap_err()
            .contains("failed")
    );
    let project = TemporaryDirectory::new();
    fs::create_dir(project.path().join("config")).unwrap();
    fs::write(
        project.path().join("config/setting.json"),
        br#"{"lib":["npm:app@1.0.0","npm:other@1.0.0"]}"#,
    )
    .unwrap();
    lock.write_atomic(project.path()).unwrap();
    dever_cli::libs::execute("remove", project.path(), &["npm:app@1.0.0".into()]).unwrap();
    let retained = LockFile::decode(&fs::read(project.path().join("dever.lock")).unwrap()).unwrap();
    doctor(&retained).unwrap();
    let files =
        dever_cli::libs::build::npm_installation(&retained, &retained.npm[0], &bytes).unwrap();
    assert!(
        files
            .iter()
            .all(|(path, _, _)| path.starts_with("node_modules/other/"))
    );
}

#[test]
#[ignore = "explicit managed npm hook: sparse scratch overflow and child cleanup; no network"]
fn npm_build_scratch_budget_kills_owned_hook_and_cleans_stage() {
    let inputs = AuthorInputs::load();
    let session = Session::new(&inputs);
    let mut transport = LocalRegistry(BTreeMap::new());
    npm_tests::release(&mut transport,"oversized","1.0.0",serde_json::json!({"scripts":{"install":"node fill.js"}}),&[("package/fill.js",b"const fs=require('fs');fs.writeFileSync('large',Buffer.alloc(1));fs.truncateSync('large',600*1024*1024);setInterval(()=>{},1000);".to_vec())]);
    let store = FixtureArtifactStore::default();
    let resolver = resolver(&inputs, &session, &transport, &store);
    let owned_stages = || {
        fs::read_dir(std::env::temp_dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&format!("dever-lib-build-{}-", std::process::id())))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = owned_stages();
    let error = resolver
        .resolve(&["npm:oversized@1.0.0".parse().unwrap()])
        .unwrap_err();
    assert!(error.contains("scratch budget"), "{error}");
    assert_eq!(
        owned_stages(),
        before,
        "failed hook leaked its owned staging tree"
    );
}

#[test]
#[ignore = "explicit managed npm hook: internal output aliases are flattened; aliases outside output are rejected"]
fn npm_build_output_hard_links_stay_inside_the_exported_tree() {
    let inputs = AuthorInputs::load();
    let session = Session::new(&inputs);
    for (name, source, accepted) in [
        (
            "insidealias",
            "const fs=require('fs');fs.writeFileSync('original.txt','same-bytes');fs.linkSync('original.txt','alias.txt');",
            true,
        ),
        (
            "outsidealias",
            "const fs=require('fs');fs.writeFileSync('/data/build/outside.txt','outside');fs.linkSync('/data/build/outside.txt','alias.txt');",
            false,
        ),
    ] {
        let mut transport = LocalRegistry(BTreeMap::new());
        npm_tests::release(
            &mut transport,
            name,
            "1.0.0",
            serde_json::json!({"scripts":{"install":"node aliases.js"}}),
            &[("package/aliases.js", source.as_bytes().to_vec())],
        );
        let store = FixtureArtifactStore::default();
        let result =
            resolver(&inputs, &session, &transport, &store)
                .resolve(&[format!("npm:{name}@1.0.0").parse().unwrap()]);
        if accepted {
            let lock = result.unwrap();
            doctor(&lock).unwrap();
            let receipt = &lock.builds[0];
            let bytes = store
                .verify_exact(
                    &receipt.output.sha256,
                    receipt.output.bytes,
                    &receipt.output.target,
                )
                .unwrap();
            let files =
                dever_cli::libs::build::npm_installation(&lock, &lock.npm[0], &bytes).unwrap();
            for leaf in ["original.txt", "alias.txt"] {
                assert!(files.iter().any(|(path, bytes, _)| path
                    == &format!("node_modules/{name}/{leaf}")
                    && bytes == b"same-bytes"));
            }
        } else {
            assert!(result.unwrap_err().contains("hard-link alias outside"));
        }
    }
}
