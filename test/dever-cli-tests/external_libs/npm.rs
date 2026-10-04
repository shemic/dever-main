use super::*;
use dever_cli::libs::{LibResolver, LockedWorker, npm, resolve_workers};

pub(super) fn archive(
    name: &str,
    version: &str,
    mut manifest: serde_json::Value,
    extra: &[(&str, Vec<u8>)],
) -> Vec<u8> {
    manifest["name"] = name.into();
    manifest["version"] = version.into();
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut writer = tar::Builder::new(encoder);
    for (name, bytes) in [
        (
            "package/package.json",
            serde_json::to_vec(&manifest).unwrap(),
        ),
        (
            "package/index.js",
            format!("module.exports = '{name}/{version}';\n").into_bytes(),
        ),
    ]
    .into_iter()
    .chain(extra.iter().map(|(name, bytes)| (*name, bytes.clone())))
    {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        writer
            .append_data(&mut header, name, bytes.as_slice())
            .unwrap();
    }
    writer.into_inner().unwrap().finish().unwrap()
}

pub(super) fn release(
    registry: &mut LocalRegistry,
    name: &str,
    version: &str,
    manifest: serde_json::Value,
    extra: &[(&str, Vec<u8>)],
) {
    let archive = archive(name, version, manifest.clone(), extra);
    let index_path = (Ecosystem::Npm, format!("/{name}"));
    let mut index = registry
        .0
        .get(&index_path)
        .map(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).unwrap())
        .unwrap_or(serde_json::json!({"versions":{}}));
    index["versions"][version] = serde_json::json!({});
    registry
        .0
        .insert(index_path, serde_json::to_vec(&index).unwrap());
    let mut document = manifest;
    document["name"] = name.into();
    document["version"] = version.into();
    let url = format!("https://mock/{name}/{version}.tgz");
    document["dist"] = serde_json::json!({"tarball":url,"integrity":format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&archive)))});
    registry.0.insert(
        (Ecosystem::Npm, format!("/{name}/{version}")),
        serde_json::to_vec(&document).unwrap(),
    );
    registry.0.insert((Ecosystem::Npm, url), archive);
}

fn resolved(registry: &LocalRegistry, requested: &[&str]) -> (LockFile, FixtureArtifactStore) {
    let store = FixtureArtifactStore::default();
    let lock = local_resolver(registry, &store)
        .resolve(
            &requested
                .iter()
                .map(|spec| spec.parse().unwrap())
                .collect::<Vec<_>>(),
        )
        .unwrap();
    doctor(&lock).unwrap();
    assert_eq!(LockFile::decode(&lock.encode().unwrap()).unwrap(), lock);
    (lock, store)
}

fn files(lock: &LockFile, store: &FixtureArtifactStore, index: usize) -> BTreeMap<String, Vec<u8>> {
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
    npm::files(&lock.npm[index], lock, |lib| {
        Ok(archives[&lib.spec].as_slice())
    })
    .unwrap()
    .into_iter()
    .collect()
}

#[test]
fn nested_versions_and_shared_packages_keep_distinct_peer_contexts() {
    let mut registry = LocalRegistry(BTreeMap::new());
    for (name, dependencies) in [
        ("left", serde_json::json!({"plugin":"1.0.0","host":"1.0.0"})),
        (
            "right",
            serde_json::json!({"plugin":"1.0.0","host":"2.0.0"}),
        ),
    ] {
        release(
            &mut registry,
            name,
            "1.0.0",
            serde_json::json!({"dependencies":dependencies}),
            &[],
        );
    }
    release(
        &mut registry,
        "plugin",
        "1.0.0",
        serde_json::json!({"peerDependencies":{"host":">=1.0.0 <3.0.0"}}),
        &[],
    );
    for version in ["1.0.0", "2.0.0"] {
        release(&mut registry, "host", version, serde_json::json!({}), &[]);
    }
    let (lock, store) = resolved(&registry, &["npm:left@1.0.0", "npm:right@1.0.0"]);
    assert_eq!(lock.libs.len(), 5);
    let files = files(&lock, &store, 0);
    for (side, version) in [("left", "1.0.0"), ("right", "2.0.0")] {
        let node = lock.npm[0]
            .instances
            .iter()
            .find(|node| node.path == format!("node_modules/{side}/node_modules/plugin"))
            .unwrap();
        assert_eq!(
            node.edges[0].target.as_deref(),
            Some(format!("node_modules/{side}/node_modules/host").as_str())
        );
        assert!(
            String::from_utf8(
                files[&format!("node_modules/{side}/node_modules/host/index.js")].clone()
            )
            .unwrap()
            .contains(version)
        );
    }
}

#[test]
fn optional_dependencies_override_regular_edges_and_record_target_or_absence() {
    let mut registry = LocalRegistry(BTreeMap::new());
    release(
        &mut registry,
        "root",
        "1.0.0",
        serde_json::json!({"dependencies":{"child":"1.0.0"},"optionalDependencies":{"child":"2.0.0","missing":"*","darwin":"*","musl":"*"}}),
        &[],
    );
    release(&mut registry, "child", "2.0.0", serde_json::json!({}), &[]);
    release(
        &mut registry,
        "darwin",
        "1.0.0",
        serde_json::json!({"os":["darwin"]}),
        &[],
    );
    release(
        &mut registry,
        "musl",
        "1.0.0",
        serde_json::json!({"os":["linux"],"libc":["musl"]}),
        &[],
    );
    let (lock, store) = resolved(&registry, &["npm:root@1.0.0"]);
    assert_eq!(lock.libs.len(), 2);
    let root = lock.npm[0]
        .instances
        .iter()
        .find(|node| node.spec.name == "root")
        .unwrap();
    assert_eq!(
        root.edges
            .iter()
            .find(|edge| edge.name == "child")
            .unwrap()
            .kind,
        npm::Kind::Optional
    );
    for name in ["darwin", "musl"] {
        assert_eq!(
            root.edges
                .iter()
                .find(|edge| edge.name == name)
                .unwrap()
                .omission,
            Some(npm::Omission::Target)
        );
    }
    assert_eq!(
        root.edges
            .iter()
            .find(|edge| edge.name == "missing")
            .unwrap()
            .omission,
        Some(npm::Omission::Unavailable)
    );
    assert!(
        !files(&lock, &store, 0)
            .keys()
            .any(|path| path.contains("darwin") || path.contains("musl"))
    );
}

#[test]
fn optional_integrity_and_protocol_failures_are_never_omissions() {
    let mut registry = LocalRegistry(BTreeMap::new());
    release(
        &mut registry,
        "root",
        "1.0.0",
        serde_json::json!({"optionalDependencies":{"child":"*"}}),
        &[],
    );
    release(&mut registry, "child", "1.0.0", serde_json::json!({}), &[]);
    registry.0.insert(
        (Ecosystem::Npm, "https://mock/child/1.0.0.tgz".into()),
        b"tampered".to_vec(),
    );
    let store = FixtureArtifactStore::default();
    assert!(
        local_resolver(&registry, &store)
            .resolve(&["npm:root@1.0.0".parse().unwrap()])
            .unwrap_err()
            .contains("integrity mismatch")
    );
    release(
        &mut registry,
        "child",
        "1.0.0",
        serde_json::json!({"dependencies":{"other":"not-a-semver"}}),
        &[],
    );
    assert!(
        local_resolver(&registry, &store)
            .resolve(&["npm:root@1.0.0".parse().unwrap()])
            .unwrap_err()
            .contains("invalid npm range")
    );
}

#[test]
fn required_peers_auto_install_and_backtrack_but_optional_peers_do_not() {
    let mut registry = LocalRegistry(BTreeMap::new());
    release(
        &mut registry,
        "root",
        "1.0.0",
        serde_json::json!({"dependencies":{"plugin":"*","host":"1.0.0"}}),
        &[],
    );
    release(
        &mut registry,
        "plugin",
        "2.0.0",
        serde_json::json!({"peerDependencies":{"host":"2.0.0"}}),
        &[],
    );
    release(
        &mut registry,
        "plugin",
        "1.0.0",
        serde_json::json!({"peerDependencies":{"host":"1.0.0","absent":"*"},"peerDependenciesMeta":{"absent":{"optional":true}}}),
        &[],
    );
    for version in ["1.0.0", "2.0.0"] {
        release(&mut registry, "host", version, serde_json::json!({}), &[]);
    }
    let (lock, _) = resolved(&registry, &["npm:root@1.0.0"]);
    let plugin = lock.npm[0]
        .instances
        .iter()
        .find(|node| node.spec.name == "plugin")
        .unwrap();
    assert_eq!(plugin.spec.version, "1.0.0");
    assert_eq!(
        plugin
            .edges
            .iter()
            .find(|edge| edge.name == "absent")
            .unwrap()
            .omission,
        Some(npm::Omission::OptionalPeerAbsent)
    );
    let (lock, _) = resolved(&registry, &["npm:plugin@1.0.0"]);
    assert!(
        lock.npm[0]
            .instances
            .iter()
            .any(|node| node.path == "node_modules/host")
    );
    let store = FixtureArtifactStore::default();
    assert!(
        local_resolver(&registry, &store)
            .resolve(&[
                "npm:plugin@2.0.0".parse().unwrap(),
                "npm:host@1.0.0".parse().unwrap()
            ])
            .is_err()
    );
}

#[test]
fn bundled_transitive_packages_are_bound_to_parent_and_never_downloaded() {
    let mut registry = LocalRegistry(BTreeMap::new());
    let extra = vec![
        (
            "package/node_modules/b/package.json",
            br#"{"name":"b","version":"1.0.0","dependencies":{"c":"2.0.0"}}"#.to_vec(),
        ),
        (
            "package/node_modules/b/index.js",
            b"module.exports = require('c');".to_vec(),
        ),
        (
            "package/node_modules/c/package.json",
            br#"{"name":"c","version":"2.0.0"}"#.to_vec(),
        ),
        (
            "package/node_modules/c/index.js",
            b"module.exports = 42;".to_vec(),
        ),
    ];
    release(
        &mut registry,
        "root",
        "1.0.0",
        serde_json::json!({"dependencies":{"b":"1.0.0"},"bundleDependencies":["b"]}),
        &extra,
    );
    let (lock, store) = resolved(&registry, &["npm:root@1.0.0"]);
    assert_eq!(lock.libs.len(), 1);
    assert_eq!(lock.npm[0].instances.len(), 3);
    assert_eq!(
        files(&lock, &store, 0)["node_modules/root/node_modules/c/index.js"],
        b"module.exports = 42;"
    );
    release(
        &mut registry,
        "root",
        "1.0.0",
        serde_json::json!({}),
        &extra,
    );
    assert!(
        local_resolver(&registry, &store)
            .resolve(&["npm:root@1.0.0".parse().unwrap()])
            .unwrap_err()
            .contains("undeclared")
    );
}

#[test]
fn worker_root_sets_keep_separate_npm_environments_and_detect_lock_tampering() {
    let mut registry = LocalRegistry(BTreeMap::new());
    release(
        &mut registry,
        "plugin",
        "1.0.0",
        serde_json::json!({"peerDependencies":{"host":"*"}}),
        &[],
    );
    for version in ["1.0.0", "2.0.0"] {
        release(&mut registry, "host", version, serde_json::json!({}), &[]);
    }
    let store = FixtureArtifactStore::default();
    let worker = |adapter: &str, version: &str| LockedWorker {
        port: "sample.worker".into(),
        adapter: adapter.into(),
        ecosystem: "npm".into(),
        runtime: Some(registry_runtime(Ecosystem::Npm).pack),
        entry: format!("worker/{adapter}.mjs"),
        schema: "checked-port".into(),
        capabilities: vec![],
        operations: vec!["read".into()],
        libs: vec![
            "npm:plugin@1.0.0".parse().unwrap(),
            format!("npm:host@{version}").parse().unwrap(),
        ],
    };
    let lock = resolve_workers(
        &local_resolver(&registry, &store),
        &[],
        vec![worker("first", "1.0.0"), worker("second", "2.0.0")],
    )
    .unwrap();
    assert_eq!(lock.npm.len(), 2);
    for index in 0..2 {
        assert!(files(&lock, &store, index).contains_key("node_modules/plugin/index.js"));
    }
    let mut changed = lock.clone();
    changed.npm[0].instances[0].path = "../escape".into();
    assert!(doctor(&changed).is_err());
    let mut changed = lock.clone();
    changed.npm[0]
        .instances
        .iter_mut()
        .find(|node| node.spec.name == "plugin")
        .unwrap()
        .edges[0]
        .target = Some("node_modules/missing".into());
    assert!(doctor(&changed).is_err());
    let mut changed = lock;
    changed.npm[0]
        .instances
        .iter_mut()
        .find(|node| node.spec.name == "plugin")
        .unwrap()
        .edges
        .clear();
    let archives = changed
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
    assert!(
        npm::files(&changed.npm[0], &changed, |lib| Ok(
            archives[&lib.spec].as_slice()
        ))
        .unwrap_err()
        .contains("differs from package.json")
    );
}

#[test]
fn bundled_instances_cannot_switch_to_another_parents_same_version_package() {
    let mut registry = LocalRegistry(BTreeMap::new());
    for name in ["a", "c"] {
        release(
            &mut registry,
            name,
            "1.0.0",
            serde_json::json!({"dependencies":{"b":"1.0.0"},"bundleDependencies":["b"]}),
            &[
                (
                    "package/node_modules/b/package.json",
                    br#"{"name":"b","version":"1.0.0"}"#.to_vec(),
                ),
                (
                    "package/node_modules/b/index.js",
                    format!("module.exports = '{name}';").into_bytes(),
                ),
            ],
        );
    }
    release(&mut registry, "b", "1.0.0", serde_json::json!({}), &[]);
    let (original, store) = resolved(&registry, &["npm:a@1.0.0", "npm:c@1.0.0", "npm:b@1.0.0"]);
    let archives = original
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
    for (archive, prefix) in [
        ("npm:c@1.0.0", "package/node_modules/b/"),
        ("npm:b@1.0.0", "package/"),
    ] {
        let mut changed = original.clone();
        changed.npm[0]
            .instances
            .iter_mut()
            .find(|node| node.path == "node_modules/a/node_modules/b")
            .unwrap()
            .source = npm::Source {
            archive: archive.parse().unwrap(),
            prefix: prefix.into(),
        };
        assert!(
            doctor(&changed)
                .unwrap_err()
                .contains("parent archive installation")
        );
        assert!(
            npm::files(&changed.npm[0], &changed, |lib| Ok(
                archives[&lib.spec].as_slice()
            ))
            .unwrap_err()
            .contains("parent archive installation")
        );
        // Removing the locked layout cannot hide the original archive binding.
        for node in &mut changed.npm[0].instances {
            node.bundled.clear();
        }
        assert!(
            npm::files(&changed.npm[0], &changed, |lib| Ok(
                archives[&lib.spec].as_slice()
            ))
            .is_err()
        );
    }
}

#[test]
fn production_and_optional_dependencies_override_same_named_peers() {
    let mut registry = LocalRegistry(BTreeMap::new());
    release(&mut registry, "host", "2.0.0", serde_json::json!({}), &[]);
    release(
        &mut registry,
        "plugin",
        "1.0.0",
        serde_json::json!({"dependencies":{"host":"2.0.0"},"peerDependencies":{"host":"1.0.0"}}),
        &[],
    );
    let (lock, _) = resolved(&registry, &["npm:plugin@1.0.0", "npm:host@2.0.0"]);
    let plugin = lock.npm[0]
        .instances
        .iter()
        .find(|node| node.spec.name == "plugin")
        .unwrap();
    assert_eq!(plugin.edges.len(), 1);
    assert_eq!(plugin.edges[0].kind, npm::Kind::Dependency);
    assert_eq!(
        plugin.edges[0].target.as_deref(),
        Some("node_modules/plugin/node_modules/host")
    );
    release(
        &mut registry,
        "plugin",
        "1.0.0",
        serde_json::json!({"optionalDependencies":{"absent":"*"},"peerDependencies":{"absent":"1.0.0"}}),
        &[],
    );
    let (lock, _) = resolved(&registry, &["npm:plugin@1.0.0"]);
    assert_eq!(lock.npm[0].instances[0].edges.len(), 1);
    assert_eq!(lock.npm[0].instances[0].edges[0].kind, npm::Kind::Optional);
    assert_eq!(
        lock.npm[0].instances[0].edges[0].omission,
        Some(npm::Omission::Unavailable)
    );
}

#[test]
fn removing_a_root_prunes_its_entire_dependency_cycle() {
    for bundled in [false, true] {
        let mut registry = LocalRegistry(BTreeMap::new());
        release(&mut registry, "z", "1.0.0", serde_json::json!({}), &[]);
        let b = serde_json::json!({"name":"b","version":"1.0.0","dependencies":{"a":"1.0.0"}});
        let mut a = serde_json::json!({"dependencies":{"b":"1.0.0"}});
        let extra = if bundled {
            a["bundleDependencies"] = serde_json::json!(["b"]);
            vec![(
                "package/node_modules/b/package.json",
                serde_json::to_vec(&b).unwrap(),
            )]
        } else {
            release(&mut registry, "b", "1.0.0", b, &[]);
            vec![]
        };
        release(&mut registry, "a", "1.0.0", a, &extra);
        let (lock, _) = resolved(&registry, &["npm:a@1.0.0", "npm:z@1.0.0"]);
        let root = TemporaryDirectory::new();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/setting.json"),
            br#"{"lib":["npm:a@1.0.0","npm:z@1.0.0"]}"#,
        )
        .unwrap();
        lock.write_atomic(root.path()).unwrap();
        dever_cli::libs::execute("remove", root.path(), &["npm:a@1.0.0".into()]).unwrap();
        let retained =
            LockFile::decode(&fs::read(root.path().join("dever.lock")).unwrap()).unwrap();
        assert_eq!(retained.libs.len(), 1);
        assert_eq!(retained.libs[0].spec.name, "z");
        assert_eq!(retained.npm[0].instances.len(), 1);
        doctor(&retained).unwrap();
    }
}

#[test]
fn removing_a_dependency_still_required_by_a_remaining_root_is_rejected() {
    let mut registry = LocalRegistry(BTreeMap::new());
    release(
        &mut registry,
        "a",
        "1.0.0",
        serde_json::json!({"dependencies":{"b":"1.0.0"}}),
        &[],
    );
    release(&mut registry, "b", "1.0.0", serde_json::json!({}), &[]);
    let (lock, _) = resolved(&registry, &["npm:a@1.0.0", "npm:b@1.0.0"]);
    let root = TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    let setting = br#"{"lib":["npm:a@1.0.0","npm:b@1.0.0"]}"#;
    fs::write(root.path().join("config/setting.json"), setting).unwrap();
    lock.write_atomic(root.path()).unwrap();
    let before = fs::read(root.path().join("dever.lock")).unwrap();
    assert!(
        dever_cli::libs::execute("remove", root.path(), &["npm:b@1.0.0".into()])
            .unwrap_err()
            .contains("required by")
    );
    assert_eq!(fs::read(root.path().join("dever.lock")).unwrap(), before);
    assert_eq!(
        fs::read(root.path().join("config/setting.json")).unwrap(),
        setting
    );
}

#[test]
fn repeated_archive_instances_are_bounded_before_materialization_clones() {
    let mut registry = LocalRegistry(BTreeMap::new());
    release(
        &mut registry,
        "leaf",
        "1.0.0",
        serde_json::json!({}),
        &[("package/payload.js", vec![b'x'; 2 * 1024 * 1024])],
    );
    let mut roots = Vec::new();
    for number in 0..129 {
        let name = format!("root-{number}");
        release(
            &mut registry,
            &name,
            "1.0.0",
            serde_json::json!({"dependencies":{"leaf":"1.0.0"}}),
            &[],
        );
        roots.push(format!("npm:{name}@1.0.0"));
    }
    let (lock, store) = resolved(
        &registry,
        &roots.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_eq!(lock.npm[0].instances.len(), 258);
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
    assert!(
        npm::files(
            &lock.npm[0],
            &lock,
            |lib| Ok(archives[&lib.spec].as_slice())
        )
        .unwrap_err()
        .contains("tree file/byte budget")
    );
}
