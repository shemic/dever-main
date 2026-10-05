use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use dever_cli::libs::{
    ArtifactStore, Ecosystem, FixtureArtifactStore, FixtureRegistry, LibSpec, LockFile,
    RegistryResolver, RegistryRuntime, RegistryTransport, RuntimePack, SdkFixture, prepare,
    prepare_resources, provider_manifest,
};
use pep508_rs::MarkerEnvironment;
use sha2::{Digest, Sha256, Sha512};

#[path = "../dever-tests/tests/support/temp.rs"]
mod temp;

use temp::TemporaryDirectory;

#[path = "external_libs/build.rs"]
mod build_tests;
#[path = "support/native_elf.rs"]
#[allow(
    dead_code,
    reason = "This test crate uses only the shared static ELF fixture"
)]
mod native_elf;
#[path = "external_libs/npm_build.rs"]
mod npm_build_tests;
#[path = "external_libs/npm.rs"]
mod npm_tests;
#[path = "external_libs/restore.rs"]
mod restore_tests;
#[path = "external_libs/sumdb.rs"]
mod sumdb_tests;
#[path = "support/wheel.rs"]
mod wheel_fixture;
#[path = "external_libs/wheel.rs"]
mod wheel_tests;

fn publish_fixture_artifacts(output: &std::path::Path, lock: &LockFile, store: &dyn ArtifactStore) {
    let archives = output.join("artifacts");
    fs::create_dir_all(&archives).unwrap();
    for artifact in lock
        .libs
        .iter()
        .flat_map(|lib| &lib.artifacts)
        .chain(dever_cli::libs::build::npm_outputs(lock).map(|receipt| &receipt.output))
    {
        let bytes = store
            .verify_exact(&artifact.sha256, artifact.bytes, &artifact.target)
            .unwrap();
        let path = archives.join(&artifact.sha256);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => file.write_all(&bytes).unwrap(),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                assert_eq!(fs::read(&path).unwrap(), bytes, "fixture archive drift");
            }
            Err(error) => panic!("cannot publish fixture archive: {error}"),
        }
    }
    lock.write_atomic(output).unwrap();
    assert_eq!(
        LockFile::decode(&fs::read(output.join("dever.lock")).unwrap()).unwrap(),
        *lock
    );
}

fn doctor(lock: &LockFile) -> Result<(), String> {
    dever_cli::libs::doctor_with_go_verifier(lock, &sumdb_tests::verifier())
}

fn verify_locked_artifacts(
    lock: &LockFile,
    store: &dyn ArtifactStore,
    target: &str,
) -> Result<Vec<dever_cli::libs::VerifiedArtifact>, String> {
    dever_cli::libs::verify_locked_artifacts_with_go_verifier(
        lock,
        store,
        target,
        &sumdb_tests::verifier(),
    )
}

fn exec_program() -> dever_core::hir::Program {
    let mut sources = dever_core::source::SourceMap::default();
    sources.add(
        "notification/mail/port.dever",
        "read() (value: Int) fails app.ReadResult\n",
    );
    sources.add("notification/mail/app.dever", "type ReadResult { error Unavailable(message: Text) }\nread() (value: Int) { value = port.read() }\n");
    sources.add(
        "notification/mail/adapter/first.dever",
        "external exec \"first-worker\" {}",
    );
    sources.add(
        "notification/mail/adapter/second.dever",
        "external exec \"second-worker\" {}",
    );
    dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"))
}

#[test]
fn exec_preparation_embeds_every_candidate_with_its_current_bytes() {
    let root = TemporaryDirectory::new();
    let program = exec_program();
    let entries = program.external_worker_entries();
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        let path = root.path().join(entry);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = native_elf::static_executable();
        bytes.extend_from_slice(entry.as_bytes());
        fs::write(&path, bytes).unwrap();
    }
    assert!(
        prepare_resources(
            root.path(),
            &program,
            dever_cli::toolchain::BuildTarget::host().unwrap()
        )
        .unwrap_err()
        .contains("signed managed Dever release")
    );
    let prepare = || {
        let input = entries
            .iter()
            .map(|entry| {
                let bytes = fs::read(root.path().join(entry)).unwrap();
                dever_core::native::EmbeddedResource {
                    path: entry.clone(),
                    sha256: digest(&bytes),
                    bytes,
                    executable: true,
                }
            })
            .collect::<Vec<_>>();
        dever_cli::workers::prepare(root.path(), &program, &input).unwrap()
    };
    let resources = prepare();
    assert_eq!(resources.len(), entries.len() * 2);
    for entry in &entries {
        let manifest = resources
            .iter()
            .find(|resource| resource.path == format!("{entry}.dever-worker.json"))
            .unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&manifest.bytes).unwrap();
        let worker = resources
            .iter()
            .find(|resource| resource.path == manifest["executable"].as_str().unwrap())
            .unwrap();
        assert_eq!(worker.bytes, fs::read(root.path().join(entry)).unwrap());
        assert!(worker.executable);
    }
    let mut changed_binary = native_elf::static_executable();
    changed_binary.extend_from_slice(b"changed");
    fs::write(root.path().join(&entries[0]), &changed_binary).unwrap();
    let changed = prepare();
    assert!(
        changed
            .iter()
            .any(|resource| resource.bytes == changed_binary)
    );
    assert!(
        !resources
            .iter()
            .any(|resource| resource.bytes == changed_binary)
    );
    fs::remove_file(root.path().join(&entries[1])).unwrap();
    assert!(
        prepare_resources(
            root.path(),
            &program,
            dever_cli::toolchain::BuildTarget::host().unwrap()
        )
        .unwrap_err()
        .contains("cannot inspect")
    );
}

#[test]
fn exec_worker_preparation_rejects_host_elf_for_arm_target() {
    use dever_cli::toolchain::BuildTarget;
    let root = TemporaryDirectory::new();
    let program = exec_program();
    let mut arm = native_elf::static_executable();
    arm[18..20].copy_from_slice(&183u16.to_le_bytes());
    let mut resources = program
        .external_worker_entries()
        .into_iter()
        .map(|path| dever_core::native::EmbeddedResource {
            path,
            sha256: digest(&arm),
            bytes: arm.clone(),
            executable: true,
        })
        .collect::<Vec<_>>();
    let output = dever_cli::workers::prepare_for_target(
        root.path(),
        &program,
        &resources,
        BuildTarget::LinuxAarch64,
        &[],
    )
    .unwrap();
    assert_eq!(
        output.iter().filter(|file| file.executable).count(),
        resources.len()
    );
    resources[0].bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    resources[0].sha256 = digest(&resources[0].bytes);
    assert!(
        dever_cli::workers::prepare_for_target(
            root.path(),
            &program,
            &resources,
            BuildTarget::LinuxAarch64,
            &[]
        )
        .unwrap_err()
        .contains("differs from target linux-aarch64")
    );
}

#[cfg(unix)]
#[test]
fn exec_preparation_rejects_project_worker_symlinks() {
    let root = TemporaryDirectory::new();
    let outside = TemporaryDirectory::new();
    let program = exec_program();
    for entry in program.external_worker_entries() {
        let path = root.path().join(entry);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(outside.path().join("worker"), b"outside").unwrap();
        std::os::unix::fs::symlink(outside.path().join("worker"), path).unwrap();
    }
    assert!(
        prepare_resources(
            root.path(),
            &program,
            dever_cli::toolchain::BuildTarget::host().unwrap()
        )
        .unwrap_err()
        .contains("symbolic link")
    );
}

static NEXT: AtomicU64 = AtomicU64::new(0);

fn project() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "dever-lib-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}

struct LocalRegistry(BTreeMap<(Ecosystem, String), Vec<u8>>);

impl RegistryTransport for LocalRegistry {
    fn get_optional(
        &self,
        ecosystem: Ecosystem,
        path: &str,
        _limit: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.get(&(ecosystem, path.into())).cloned())
    }
    fn get(&self, ecosystem: Ecosystem, path: &str, _limit: usize) -> Result<Vec<u8>, String> {
        self.0
            .get(&(ecosystem, path.into()))
            .cloned()
            .ok_or_else(|| format!("local registry has no {path}"))
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn zip_file(path: &str) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(path, zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"package bytes").unwrap();
    writer.finish().unwrap().into_inner()
}

fn python_markers() -> MarkerEnvironment {
    wheel_fixture::markers()
}

fn pure_wheel(name: &str, version: &str, requires: &[&str], extras: &[&str]) -> Vec<u8> {
    wheel_fixture::pack(
        name,
        version,
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        &wheel_fixture::metadata(name, version, requires, extras),
        &[(&format!("{name}/__init__.py"), b"package bytes".to_vec())],
    )
}

fn registry_runtime(ecosystem: Ecosystem) -> RegistryRuntime {
    RegistryRuntime {
        pack: RuntimePack {
            name: ecosystem.as_str().into(),
            version: "3.12.0".into(),
            sha256: digest(b"signed runtime"),
        },
        target: "linux-x86_64".into(),
        python_markers: (ecosystem == Ecosystem::Pip).then(python_markers),
        python_wheel_tags: if ecosystem == Ecosystem::Pip {
            vec!["py3-none-any".into()]
        } else {
            Vec::new()
        },
        npm_libc: (ecosystem == Ecosystem::Npm).then(|| "glibc".into()),
    }
}

fn local_registry() -> LocalRegistry {
    let py_a = pure_wheel("a", "1.0.0", &["b >=1.0.0,<2.0.0"], &[]);
    let py_b = pure_wheel("b", "1.1.0", &[], &[]);
    let npm_a = npm_tests::archive(
        "root",
        "1.0.0",
        serde_json::json!({"dependencies":{"child":"^1.0.0"}}),
        &[],
    );
    let npm_b = npm_tests::archive("child", "1.2.0", serde_json::json!({}), &[]);
    let go_a = zip_file("example.com/a@v1.0.0/a.go");
    let go_b = zip_file("example.com/b@v1.2.0/b.go");
    let mut files = BTreeMap::new();
    for (name, version, bytes, requires) in [
        ("a", "1.0.0", &py_a, vec!["b >=1.0.0,<2.0.0"]),
        ("b", "1.1.0", &py_b, vec![]),
    ] {
        files.insert(
            (Ecosystem::Pip, format!("/pypi/{name}/json")),
            serde_json::to_vec(&serde_json::json!({"releases": {(version): []}})).unwrap(),
        );
        files.insert((Ecosystem::Pip, format!("/pypi/{name}/{version}/json")), serde_json::to_vec(&serde_json::json!({
            "info": {"requires_dist": requires},
            "urls": [{"packagetype": "bdist_wheel", "filename": format!("{name}-{version}-py3-none-any.whl"),
                "url": format!("https://mock/{name}.whl"), "digests": {"sha256": digest(bytes)}}]
        })).unwrap());
        files.insert(
            (Ecosystem::Pip, format!("https://mock/{name}.whl")),
            bytes.clone(),
        );
    }
    for (name, version, bytes, dependencies) in [
        (
            "root",
            "1.0.0",
            &npm_a,
            serde_json::json!({"child": "^1.0.0"}),
        ),
        ("child", "1.2.0", &npm_b, serde_json::json!({})),
    ] {
        files.insert(
            (Ecosystem::Npm, format!("/{name}")),
            serde_json::to_vec(&serde_json::json!({"versions": {(version): {}}})).unwrap(),
        );
        files.insert((Ecosystem::Npm, format!("/{name}/{version}")), serde_json::to_vec(&serde_json::json!({
            "dependencies": dependencies,
            "dist": {"tarball": format!("https://mock/{name}.tgz"),
                "integrity": format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes)))}
        })).unwrap());
        files.insert(
            (Ecosystem::Npm, format!("https://mock/{name}.tgz")),
            bytes.clone(),
        );
    }
    files.insert(
        (Ecosystem::Go, "/example.com/a/@v/v1.0.0.mod".into()),
        b"module example.com/a\nrequire example.com/b v1.2.0\n".to_vec(),
    );
    files.insert(
        (Ecosystem::Go, "/example.com/b/@v/v1.2.0.mod".into()),
        b"module example.com/b\n".to_vec(),
    );
    files.insert((Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip".into()), go_a);
    files.insert((Ecosystem::Go, "/example.com/b/@v/v1.2.0.zip".into()), go_b);
    let mut registry = LocalRegistry(files);
    sumdb_tests::seal(&mut registry);
    registry
}

fn local_resolver<'a>(
    registry: &'a LocalRegistry,
    cache: &'a FixtureArtifactStore,
) -> RegistryResolver<'a> {
    RegistryResolver {
        build: None,
        transport: registry,
        store: cache,
        runtimes: [Ecosystem::Pip, Ecosystem::Npm, Ecosystem::Go]
            .into_iter()
            .map(|ecosystem| (ecosystem.clone(), registry_runtime(ecosystem)))
            .collect(),
        go_sumdb: dever_cli::libs::sumdb::ChecksumDatabase::new(sumdb_tests::verifier(), None)
            .unwrap(),
    }
}

#[test]
fn real_registry_protocols_resolve_transitive_closure_and_cache_exact_archives() {
    let registry = local_registry();
    let cache = FixtureArtifactStore::default();
    let resolver = local_resolver(&registry, &cache);
    let roots = ["pip:a@1.0.0", "npm:root@1.0.0", "go:example.com/a@1.0.0"]
        .into_iter()
        .map(str::parse)
        .collect::<Result<Vec<LibSpec>, _>>()
        .unwrap();
    let lock = dever_cli::libs::LibResolver::resolve(&resolver, &roots).unwrap();
    assert_eq!(lock.libs.len(), 6);
    assert_eq!(
        dever_cli::libs::LibResolver::resolve(&local_resolver(&registry, &cache), &roots)
            .unwrap()
            .encode()
            .unwrap(),
        lock.encode().unwrap()
    );
    doctor(&lock).unwrap();
    assert_eq!(
        verify_locked_artifacts(&lock, &cache, "linux-x86_64")
            .unwrap()
            .len(),
        6
    );
    assert!(
        verify_locked_artifacts(&lock, &FixtureArtifactStore::default(), "linux-x86_64")
            .unwrap_err()
            .contains("missing")
    );
}

#[test]
fn cross_target_resolver_locks_all_three_ecosystems_to_arm_without_host_tools() {
    let registry = local_registry();
    let cache = FixtureArtifactStore::default();
    let mut resolver = local_resolver(&registry, &cache);
    for runtime in resolver.runtimes.values_mut() {
        runtime.target = "linux-aarch64".into();
        if let Some(markers) = &runtime.python_markers {
            let mut markers = serde_json::to_value(markers).unwrap();
            markers["platform_machine"] = serde_json::json!("aarch64");
            runtime.python_markers = Some(serde_json::from_value(markers).unwrap());
        }
    }
    let roots = ["pip:a@1.0.0", "npm:root@1.0.0", "go:example.com/a@1.0.0"]
        .into_iter()
        .map(str::parse)
        .collect::<Result<Vec<LibSpec>, _>>()
        .unwrap();
    let lock = dever_cli::libs::LibResolver::resolve(&resolver, &roots).unwrap();
    assert_eq!(lock.libs.len(), 6);
    assert!(
        lock.libs
            .iter()
            .flat_map(|lib| &lib.artifacts)
            .all(|artifact| artifact.target == "linux-aarch64")
    );
    assert_eq!(
        verify_locked_artifacts(&lock, &cache, "linux-aarch64")
            .unwrap()
            .len(),
        6
    );
    assert!(
        verify_locked_artifacts(&lock, &cache, "linux-x86_64")
            .unwrap_err()
            .contains("not linux-x86_64")
    );
}

#[test]
fn real_registry_rejects_conflicts_hash_drift_and_archive_escape() {
    let registry = local_registry();
    let cache = FixtureArtifactStore::default();
    let resolver = local_resolver(&registry, &cache);
    let conflicting = [
        "pip:a@1.0.0".parse().unwrap(),
        "pip:a@2.0.0".parse().unwrap(),
    ];
    assert!(
        dever_cli::libs::LibResolver::resolve(&resolver, &conflicting)
            .unwrap_err()
            .contains("no pip version satisfies")
    );

    let mut registry = local_registry();
    registry.0.insert(
        (Ecosystem::Pip, "https://mock/a.whl".into()),
        zip_file("changed.py"),
    );
    let resolver = local_resolver(&registry, &cache);
    assert!(
        dever_cli::libs::LibResolver::resolve(&resolver, &["pip:a@1.0.0".parse().unwrap()])
            .unwrap_err()
            .contains("SHA-256 mismatch")
    );

    let mut registry = local_registry();
    registry.0.insert(
        (Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip".into()),
        zip_file("../escape"),
    );
    let resolver = local_resolver(&registry, &cache);
    assert!(
        dever_cli::libs::LibResolver::resolve(
            &resolver,
            &["go:example.com/a@1.0.0".parse().unwrap()]
        )
        .unwrap_err()
        .contains("escapes its package")
    );
}

#[test]
fn python_names_and_unconstrained_requirements_follow_pep_rules() {
    let canonical: LibSpec = "pip:Requests_FOO@1.0.0".parse().unwrap();
    assert_eq!(canonical.key(), "pip:requests-foo@1.0.0");
    assert_eq!(canonical, "pip:requests.foo@1.0.0".parse().unwrap());

    let mut registry = local_registry();
    let key = (Ecosystem::Pip, "/pypi/a/1.0.0/json".into());
    let mut release: serde_json::Value =
        serde_json::from_slice(registry.0.get(&key).unwrap()).unwrap();
    release["info"]["requires_dist"] = serde_json::json!(["b"]);
    let archive = pure_wheel("a", "1.0.0", &["b"], &[]);
    release["urls"][0]["digests"]["sha256"] = serde_json::json!(digest(&archive));
    registry
        .0
        .insert((Ecosystem::Pip, "https://mock/a.whl".into()), archive);
    registry
        .0
        .insert(key.clone(), serde_json::to_vec(&release).unwrap());
    let cache = FixtureArtifactStore::default();
    let resolver = local_resolver(&registry, &cache);
    assert_eq!(
        dever_cli::libs::LibResolver::resolve(&resolver, &["pip:a@1.0.0".parse().unwrap()])
            .unwrap()
            .libs
            .len(),
        2
    );

    release["info"]["requires_dist"] = serde_json::json!({"invalid": true});
    registry
        .0
        .insert(key, serde_json::to_vec(&release).unwrap());
    let resolver = local_resolver(&registry, &cache);
    // Even malformed release-level upload metadata cannot replace the selected
    // wheel's validated METADATA.
    assert_eq!(
        dever_cli::libs::LibResolver::resolve(&resolver, &["pip:a@1.0.0".parse().unwrap()])
            .unwrap()
            .libs
            .len(),
        2
    );
}

#[test]
fn registry_transport_failure_never_selects_an_older_candidate() {
    let mut registry = local_registry();
    registry.0.insert(
        (Ecosystem::Pip, "/pypi/b/json".into()),
        serde_json::to_vec(&serde_json::json!({"releases": {"1.2.0": [], "1.1.0": []}})).unwrap(),
    );
    let cache = FixtureArtifactStore::default();
    let resolver = local_resolver(&registry, &cache);
    let error = dever_cli::libs::LibResolver::resolve(&resolver, &["pip:a@1.0.0".parse().unwrap()])
        .unwrap_err();
    assert!(error.contains("local registry has no /pypi/b/1.2.0/json"));
}

fn python_release(
    registry: &mut LocalRegistry,
    name: &str,
    version: &str,
    extras: &[&str],
    requires: &[&str],
) {
    let index = registry
        .0
        .entry((Ecosystem::Pip, format!("/pypi/{name}/json")))
        .or_insert_with(|| br#"{"releases":{}}"#.to_vec());
    let mut document: serde_json::Value = serde_json::from_slice(index).unwrap();
    document["releases"][version] = serde_json::json!([]);
    *index = serde_json::to_vec(&document).unwrap();
    let wheel = pure_wheel(name, version, requires, extras);
    let url = format!("https://mock/{name}-{version}.whl");
    registry.0.insert((Ecosystem::Pip, format!("/pypi/{name}/{version}/json")),
        serde_json::to_vec(&serde_json::json!({
            "info": {"provides_extra": extras, "requires_dist": requires},
            "urls": [{"packagetype": "bdist_wheel", "filename": format!("{name}-{version}-py3-none-any.whl"),
                "url": url, "digests": {"sha256": digest(&wheel)}}]
        })).unwrap());
    registry.0.insert((Ecosystem::Pip, url), wheel);
}

fn python_lock(registry: &LocalRegistry, roots: &[&str]) -> Result<LockFile, String> {
    let cache = FixtureArtifactStore::default();
    let resolver = local_resolver(registry, &cache);
    dever_cli::libs::LibResolver::resolve(
        &resolver,
        &roots
            .iter()
            .map(|root| root.parse().unwrap())
            .collect::<Vec<_>>(),
    )
}

fn dependency_keys(lock: &LockFile, key: &str) -> Vec<String> {
    lock.libs
        .iter()
        .find(|lib| lib.spec.key() == key)
        .unwrap()
        .dependencies
        .iter()
        .map(|dependency| dependency.spec.key())
        .collect()
}

#[test]
fn python_extras_are_canonical_exact_identities() {
    let spec: LibSpec = "pip:Demo_Pkg[Foo_Bar,z,foo.bar,Z]@1.0.0".parse().unwrap();
    assert_eq!(spec.key(), "pip:demo-pkg[foo-bar,z]@1.0.0");
    assert_eq!(spec.distribution_name(), "demo-pkg");
    assert_eq!(spec.key().parse::<LibSpec>().unwrap(), spec);
    for invalid in [
        "pip:a[]@1.0.0",
        "pip:a[x,]@1.0.0",
        "pip:a[,x]@1.0.0",
        "pip:a[x,,y]@1.0.0",
        "pip:[x]@1.0.0",
        "pip:@1.0.0",
        "pip:a[x@y]@1.0.0",
        "pip:a[x][y]@1.0.0",
        "pip:a[x@1.0.0",
        "npm:a[x]@1.0.0",
        "go:example.com/a[x]@1.0.0",
    ] {
        assert!(invalid.parse::<LibSpec>().is_err(), "accepted {invalid}");
    }
    let mut registry = LocalRegistry(BTreeMap::new());
    python_release(&mut registry, "a", "1.0.0", &["x", "y"], &[]);
    let lock = python_lock(&registry, &["pip:a[y,x]@1.0.0"]).unwrap();
    let encoded = lock.encode().unwrap();
    assert_eq!(LockFile::decode(&encoded).unwrap(), lock);
    let invalid = String::from_utf8(encoded)
        .unwrap()
        .replace("a[x,y]", "a[y,x]");
    assert!(
        LockFile::decode(invalid.as_bytes())
            .unwrap_err()
            .contains("not canonical")
    );
    python_release(&mut registry, "a", "1.0.0", &[""], &[]);
    assert!(
        python_lock(&registry, &["pip:a@1.0.0"])
            .unwrap_err()
            .contains("Provides-Extra")
    );
}

#[test]
fn python_extras_expand_transitively_after_selection_without_leaking_into_base() {
    let mut registry = LocalRegistry(BTreeMap::new());
    python_release(&mut registry, "root", "1.0.0", &[], &["a[x]", "z"]);
    python_release(
        &mut registry,
        "a",
        "1.0.0",
        &["x", "y"],
        &[
            "c; extra == 'x'",
            "d; extra == 'y'",
            "absent; extra == 'x' and sys_platform == 'win32'",
        ],
    );
    python_release(&mut registry, "z", "1.0.0", &[], &["a[y]"]);
    python_release(&mut registry, "c", "1.0.0", &[], &[]);
    python_release(&mut registry, "d", "1.0.0", &[], &[]);
    let lock = python_lock(&registry, &["pip:root@1.0.0", "pip:a@1.0.0"]).unwrap();
    doctor(&lock).unwrap();
    assert_eq!(dependency_keys(&lock, "pip:a@1.0.0"), Vec::<String>::new());
    assert_eq!(dependency_keys(&lock, "pip:a[x]@1.0.0"), ["pip:c@1.0.0"]);
    assert_eq!(dependency_keys(&lock, "pip:a[y]@1.0.0"), ["pip:d@1.0.0"]);
    assert_eq!(dependency_keys(&lock, "pip:z@1.0.0"), ["pip:a[y]@1.0.0"]);
    let variants = lock
        .libs
        .iter()
        .filter(|lib| lib.spec.distribution_name() == "a")
        .collect::<Vec<_>>();
    assert_eq!(variants.len(), 3);
    assert!(
        variants
            .iter()
            .all(|lib| lib.artifacts == variants[0].artifacts)
    );
    assert_eq!(
        python_lock(&registry, &["pip:a@1.0.0"]).unwrap().libs.len(),
        1
    );
}

#[test]
fn python_extra_conflicts_backtrack_without_retaining_failed_edges() {
    let mut registry = LocalRegistry(BTreeMap::new());
    python_release(
        &mut registry,
        "root",
        "1.0.0",
        &[],
        &["b >=1,<3", "c ==1.0.0", "z"],
    );
    python_release(
        &mut registry,
        "b",
        "2.0.0",
        &["x"],
        &["c >=2; extra == 'x'"],
    );
    python_release(&mut registry, "b", "1.0.0", &["x"], &["d; extra == 'x'"]);
    python_release(&mut registry, "c", "1.0.0", &[], &[]);
    python_release(&mut registry, "d", "1.0.0", &[], &[]);
    python_release(&mut registry, "z", "1.0.0", &[], &["b[x]"]);
    let lock = python_lock(&registry, &["pip:root@1.0.0"]).unwrap();
    assert_eq!(dependency_keys(&lock, "pip:b[x]@1.0.0"), ["pip:d@1.0.0"]);
    assert!(!lock.libs.iter().any(|lib| lib.spec.version == "2.0.0"));
    doctor(&lock).unwrap();
    python_release(&mut registry, "b", "2.0.0", &[], &[]);
    assert!(
        python_lock(&registry, &["pip:root@1.0.0"])
            .unwrap()
            .libs
            .iter()
            .any(|lib| lib.spec.key() == "pip:b[x]@1.0.0")
    );
    assert!(
        python_lock(&registry, &["pip:b[missing]@1.0.0"])
            .unwrap_err()
            .contains("does not provide requested extra")
    );
    assert!(
        python_lock(&registry, &["pip:b[x]@1.0.0", "pip:b@2.0.0"])
            .unwrap_err()
            .contains("no pip version satisfies")
    );
}

#[test]
fn python_self_extra_activation_is_not_a_distribution_cycle() {
    let mut registry = LocalRegistry(BTreeMap::new());
    python_release(
        &mut registry,
        "a",
        "1.0.0",
        &["x"],
        &["a[x]", "b; extra == 'x'"],
    );
    python_release(&mut registry, "b", "1.0.0", &[], &[]);
    let lock = python_lock(&registry, &["pip:a@1.0.0"]).unwrap();
    doctor(&lock).unwrap();
    assert_eq!(dependency_keys(&lock, "pip:a@1.0.0"), ["pip:a[x]@1.0.0"]);
    assert_eq!(dependency_keys(&lock, "pip:a[x]@1.0.0"), ["pip:b@1.0.0"]);
    python_release(
        &mut registry,
        "a",
        "1.0.0",
        &["x", "y"],
        &[
            "a[y]; extra == 'x'",
            "a[x]; extra == 'y'",
            "b; extra == 'y'",
        ],
    );
    let lock = python_lock(&registry, &["pip:a[x]@1.0.0"]).unwrap();
    doctor(&lock).unwrap();
    assert_eq!(lock.libs.len(), 3);
    python_release(&mut registry, "b", "1.0.0", &[], &["a[x]"]);
    assert!(
        doctor(&python_lock(&registry, &["pip:a[x]@1.0.0"]).unwrap())
            .unwrap_err()
            .contains("cyclic locked lib")
    );
}

#[test]
fn python_extra_workers_keep_exact_roots_and_reject_inconsistent_locks() {
    use dever_cli::libs::{LockedWorker, resolve_workers};
    let mut registry = LocalRegistry(BTreeMap::new());
    python_release(&mut registry, "a", "1.0.0", &["x"], &["b; extra == 'x'"]);
    python_release(&mut registry, "b", "1.0.0", &[], &[]);
    python_release(&mut registry, "b", "2.0.0", &[], &[]);
    let cache = FixtureArtifactStore::default();
    let resolver = local_resolver(&registry, &cache);
    let worker = |adapter: &str, roots: &[&str]| LockedWorker {
        port: "sample.worker".into(),
        adapter: adapter.into(),
        ecosystem: "pip".into(),
        runtime: Some(registry_runtime(Ecosystem::Pip).pack),
        entry: format!("worker/{adapter}.py"),
        schema: "checked-port".into(),
        capabilities: vec![],
        operations: vec!["read".into()],
        libs: roots.iter().map(|root| root.parse().unwrap()).collect(),
    };
    let lock = resolve_workers(
        &resolver,
        &[],
        vec![
            worker("plain", &["pip:a@1.0.0"]),
            worker("extra", &["pip:a[x]@1.0.0"]),
            worker("old", &["pip:b@1.0.0"]),
        ],
    )
    .unwrap();
    assert_eq!(dependency_keys(&lock, "pip:a@1.0.0"), Vec::<String>::new());
    assert_eq!(dependency_keys(&lock, "pip:a[x]@1.0.0"), ["pip:b@2.0.0"]);
    assert_eq!(
        lock.workers
            .iter()
            .find(|worker| worker.adapter == "extra")
            .unwrap()
            .libs[0]
            .key(),
        "pip:a[x]@1.0.0"
    );
    assert_eq!(LockFile::decode(&lock.encode().unwrap()).unwrap(), lock);
    let root = TemporaryDirectory::new();
    lock.write_atomic(root.path()).unwrap();
    let report = prepare(root.path(), &["pip:a[x]@1.0.0".into()])
        .unwrap()
        .unwrap();
    assert_eq!(report.libs, 4);
    assert_eq!(report.artifacts, 3);
    assert_eq!(
        report.artifact_bytes,
        (pure_wheel("a", "1.0.0", &["b; extra == 'x'"], &["x"]).len()
            + pure_wheel("b", "1.0.0", &[], &[]).len()
            + pure_wheel("b", "2.0.0", &[], &[]).len()) as u64
    );
    assert!(
        prepare(root.path(), &["pip:a[y]@1.0.0".into()])
            .unwrap_err()
            .contains("does not contain declared")
    );
    let mut resources = verify_locked_artifacts(&lock, &cache, "linux-x86_64")
        .unwrap()
        .into_iter()
        .map(|artifact| dever_core::native::EmbeddedResource {
            path: artifact.path,
            sha256: artifact.sha256,
            bytes: artifact.bytes,
            executable: false,
        })
        .collect::<Vec<_>>();
    dever_cli::libs::prune_expanded_resources(
        &lock,
        &lock.workers,
        &["pip:a@1.0.0".parse().unwrap()],
        &mut resources,
    )
    .unwrap();
    let archive = &lock
        .libs
        .iter()
        .find(|lib| lib.spec.name == "a")
        .unwrap()
        .artifacts[0]
        .path;
    assert!(resources.iter().any(|resource| &resource.path == archive));
    let mut conflict = lock.clone();
    conflict
        .workers
        .iter_mut()
        .find(|worker| worker.adapter == "plain")
        .unwrap()
        .libs
        .extend([
            "pip:a[x]@1.0.0".parse().unwrap(),
            "pip:b@1.0.0".parse().unwrap(),
        ]);
    assert!(
        doctor(&conflict)
            .unwrap_err()
            .contains("conflicting versions")
    );
    let mut drift = lock.clone();
    drift
        .libs
        .iter_mut()
        .find(|lib| lib.spec.name == "a[x]")
        .unwrap()
        .artifacts[0]
        .sha256 = "00".repeat(32);
    assert!(
        doctor(&drift)
            .unwrap_err()
            .contains("inconsistent distribution artifacts")
    );
    let mut missing = lock;
    missing.libs.retain(|lib| lib.spec.name != "a[x]");
    assert!(doctor(&missing).unwrap_err().contains("references missing"));
    // Folding all Workers' variants into base names would invent an a/b cycle.
    python_release(&mut registry, "a", "1.0.0", &["x"], &["b; extra == 'x'"]);
    python_release(&mut registry, "b", "2.0.0", &["y"], &["a; extra == 'y'"]);
    let resolver = local_resolver(&registry, &cache);
    let independent = resolve_workers(
        &resolver,
        &[],
        vec![
            worker("first", &["pip:a[x]@1.0.0"]),
            worker("second", &["pip:b[y]@2.0.0"]),
        ],
    )
    .unwrap();
    doctor(&independent).unwrap();
}

#[test]
fn go_proxy_rejects_module_identity_and_unavailable_managed_toolchain() {
    let cache = FixtureArtifactStore::default();
    let mut registry = local_registry();
    registry.0.insert(
        (Ecosystem::Go, "/example.com/b/@v/v1.2.0.mod".into()),
        b"module wrong.example/b\n".to_vec(),
    );
    sumdb_tests::seal(&mut registry);
    let resolver = local_resolver(&registry, &cache);
    assert!(
        dever_cli::libs::LibResolver::resolve(
            &resolver,
            &["go:example.com/a@1.0.0".parse().unwrap()]
        )
        .unwrap_err()
        .contains("declaration does not match")
    );

    let mut registry = local_registry();
    registry.0.insert(
        (Ecosystem::Go, "/example.com/a/@v/v1.0.0.mod".into()),
        b"module example.com/a\ntoolchain go99.0.0\n".to_vec(),
    );
    sumdb_tests::seal(&mut registry);
    let resolver = local_resolver(&registry, &cache);
    assert!(
        dever_cli::libs::LibResolver::resolve(
            &resolver,
            &["go:example.com/a@1.0.0".parse().unwrap()]
        )
        .unwrap_err()
        .contains("requires toolchain")
    );

    let mut registry = local_registry();
    registry.0.insert(
        (Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip".into()),
        zip_file("other.example/a@v1.0.0/a.go"),
    );
    let resolver = local_resolver(&registry, &cache);
    assert!(
        dever_cli::libs::LibResolver::resolve(
            &resolver,
            &["go:example.com/a@1.0.0".parse().unwrap()]
        )
        .unwrap_err()
        .contains("mismatched module root")
    );
}

#[test]
fn http_registry_uses_an_explicit_loopback_origin_without_host_proxy() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let received = stream.read(&mut request).unwrap();
        assert!(
            std::str::from_utf8(&request[..received])
                .unwrap()
                .starts_with("GET /pypi/a/json HTTP/1.1")
        );
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
    });
    let registry = dever_cli::libs::HttpRegistry::new(
        [(Ecosystem::Pip, origin.clone())],
        [(Ecosystem::Pip, vec![origin])],
    );
    assert_eq!(
        registry.get(Ecosystem::Pip, "/pypi/a/json", 16).unwrap(),
        b"{}"
    );
    assert!(
        registry
            .get(Ecosystem::Pip, "https://other.example/a.whl", 16)
            .unwrap_err()
            .contains("untrusted origin")
    );
    server.join().unwrap();
}

fn loopback_go_proxy(
    respond: impl Fn(&str, &str) -> (u16, Option<String>, Vec<u8>) + Send + 'static,
) -> (
    String,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<Vec<String>>,
) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (done, finished) = std::sync::mpsc::channel();
    let server_origin = origin.clone();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        while finished.try_recv().is_err() {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("loopback proxy accept failed: {error}"),
            };
            let mut request = [0u8; 2048];
            let count = stream.read(&mut request).unwrap();
            let line = std::str::from_utf8(&request[..count])
                .unwrap()
                .lines()
                .next()
                .unwrap();
            let path = line.split_whitespace().nth(1).unwrap().to_owned();
            requests.push(path.clone());
            let (status, location, body) = respond(&server_origin, &path);
            let location = location
                .map(|value| format!("Location: {value}\r\n"))
                .unwrap_or_default();
            let header = format!(
                "HTTP/1.1 {status} Test\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
        requests
    });
    (origin, done, server)
}

fn loopback_go_registry(origin: &str) -> dever_cli::libs::HttpRegistry {
    dever_cli::libs::HttpRegistry::new(
        [(Ecosystem::Go, origin.to_owned())],
        [(Ecosystem::Go, vec![origin.to_owned()])],
    )
    .with_go_archive_mirror(&format!("{origin}/proxy-golang-org-prod/"))
    .unwrap()
}

#[test]
fn go_archive_redirect_resolves_and_locks_the_mirror_bytes() {
    let archive = zip_file("example.com/a@v1.0.0/a.go");
    let mirror_bytes = archive.clone();
    let (origin, done, server) = loopback_go_proxy(move |origin, path| match path {
        "/example.com/a/@v/v1.0.0.mod" => (200, None, b"module example.com/a\n".to_vec()),
        "/example.com/a/@v/v1.0.0.zip" => (
            302,
            Some(format!(
                "{origin}/proxy-golang-org-prod/module.zip?X-Goog-Signature=fixture"
            )),
            Vec::new(),
        ),
        "/proxy-golang-org-prod/module.zip?X-Goog-Signature=fixture" => {
            (200, None, mirror_bytes.clone())
        }
        _ => panic!("unexpected Go proxy request {path}"),
    });
    let registry = loopback_go_registry(&origin);
    let mut checksums = LocalRegistry(BTreeMap::from([
        (
            (Ecosystem::Go, "/example.com/a/@v/v1.0.0.mod".into()),
            b"module example.com/a\n".to_vec(),
        ),
        (
            (Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip".into()),
            archive.clone(),
        ),
    ]));
    sumdb_tests::seal(&mut checksums);
    let transport = sumdb_tests::Transport {
        registry: &registry,
        checksums: &checksums,
    };
    let cache = FixtureArtifactStore::default();
    let resolver = RegistryResolver {
        build: None,
        transport: &transport,
        store: &cache,
        runtimes: [(Ecosystem::Go, registry_runtime(Ecosystem::Go))].into(),
        go_sumdb: dever_cli::libs::sumdb::ChecksumDatabase::new(sumdb_tests::verifier(), None)
            .unwrap(),
    };
    let lock = dever_cli::libs::LibResolver::resolve(
        &resolver,
        &["go:example.com/a@1.0.0".parse().unwrap()],
    )
    .unwrap();
    assert_eq!(lock.libs[0].artifacts[0].sha256, digest(&archive));
    assert_eq!(
        cache.verify(&digest(&archive), "linux-x86_64").unwrap(),
        archive
    );
    done.send(()).unwrap();
    assert_eq!(
        server.join().unwrap(),
        [
            "/example.com/a/@v/v1.0.0.mod",
            "/example.com/a/@v/v1.0.0.zip",
            "/proxy-golang-org-prod/module.zip?X-Goog-Signature=fixture"
        ]
    );

    let (origin, done, server) = loopback_go_proxy(|origin, path| match path {
        "/example.com/a/@v/v1.0.0.mod" => (200, None, b"module example.com/a\n".to_vec()),
        "/example.com/a/@v/v1.0.0.zip" => (
            302,
            Some(format!("{origin}/proxy-golang-org-prod/module.zip")),
            Vec::new(),
        ),
        "/proxy-golang-org-prod/module.zip" => (200, None, zip_file("wrong.example/a@v1.0.0/a.go")),
        _ => panic!("unexpected Go proxy request {path}"),
    });
    let registry = loopback_go_registry(&origin);
    let transport = sumdb_tests::Transport {
        registry: &registry,
        checksums: &checksums,
    };
    let resolver = RegistryResolver {
        build: None,
        transport: &transport,
        store: &cache,
        runtimes: [(Ecosystem::Go, registry_runtime(Ecosystem::Go))].into(),
        go_sumdb: dever_cli::libs::sumdb::ChecksumDatabase::new(sumdb_tests::verifier(), None)
            .unwrap(),
    };
    assert!(
        dever_cli::libs::LibResolver::resolve(
            &resolver,
            &["go:example.com/a@1.0.0".parse().unwrap()]
        )
        .unwrap_err()
        .contains("mismatched module root")
    );
    done.send(()).unwrap();
    assert_eq!(server.join().unwrap().len(), 3);
}

#[test]
fn go_archive_redirect_rejects_untrusted_locations_and_loops_before_request() {
    for location in [
        "https://evil.example/proxy-golang-org-prod/archive.zip",
        "/proxy-golang-org-prod/archive.zip",
        "http://127.0.0.1:1/proxy-golang-org-prod/archive.zip",
        "http://user@127.0.0.1/proxy-golang-org-prod/archive.zip",
        "http://127.0.0.1/proxy-golang-org-prod/archive.zip#fragment",
    ] {
        let location = location.to_owned();
        let (origin, done, server) =
            loopback_go_proxy(move |_, _| (302, Some(location.clone()), Vec::new()));
        let registry = loopback_go_registry(&origin);
        assert!(
            registry
                .get(Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip", 1024)
                .is_err()
        );
        done.send(()).unwrap();
        assert_eq!(server.join().unwrap().len(), 1);
    }

    let (origin, done, server) = loopback_go_proxy(|origin, _| {
        let host = origin.strip_prefix("http://").unwrap();
        (
            302,
            Some(format!("http://@{host}/proxy-golang-org-prod/archive.zip")),
            Vec::new(),
        )
    });
    let registry = loopback_go_registry(&origin);
    let error = registry
        .get(Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip", 1024)
        .unwrap_err();
    assert!(error.contains("trusted mirror bucket"), "{error}");
    done.send(()).unwrap();
    assert_eq!(server.join().unwrap().len(), 1);

    let (origin, done, server) = loopback_go_proxy(|_, _| (302, None, Vec::new()));
    let registry = loopback_go_registry(&origin);
    assert!(
        registry
            .get(Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip", 1024)
            .unwrap_err()
            .contains("lacks Location")
    );
    done.send(()).unwrap();
    assert_eq!(server.join().unwrap().len(), 1);

    let (origin, done, server) = loopback_go_proxy(|origin, _| {
        (
            302,
            Some(format!("{origin}/proxy-golang-org-prod/archive.zip")),
            Vec::new(),
        )
    });
    let registry = loopback_go_registry(&origin);
    assert!(
        registry
            .get(Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip", 1024)
            .unwrap_err()
            .contains("loop")
    );
    done.send(()).unwrap();
    assert_eq!(server.join().unwrap().len(), 2);

    let (origin, done, server) = loopback_go_proxy(|origin, path| {
        let target = match path {
            "/example.com/a/@v/v1.0.0.zip" => "first.zip",
            "/proxy-golang-org-prod/first.zip" => "second.zip",
            "/proxy-golang-org-prod/second.zip" => "third.zip",
            _ => panic!("unexpected redirect request {path}"),
        };
        (
            302,
            Some(format!("{origin}/proxy-golang-org-prod/{target}")),
            Vec::new(),
        )
    });
    let registry = loopback_go_registry(&origin);
    assert!(
        registry
            .get(Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip", 1024)
            .unwrap_err()
            .contains("redirect is not allowed")
    );
    done.send(()).unwrap();
    assert_eq!(server.join().unwrap().len(), 3);

    let (origin, done, server) = loopback_go_proxy(|origin, path| match path {
        "/example.com/a/@v/v1.0.0.zip" => (
            302,
            Some(format!(
                "{origin}/proxy-golang-org-prod/module.zip?X-Goog-Signature=private"
            )),
            Vec::new(),
        ),
        "/proxy-golang-org-prod/module.zip?X-Goog-Signature=private" => (503, None, Vec::new()),
        _ => panic!("unexpected Go proxy request {path}"),
    });
    let registry = loopback_go_registry(&origin);
    let error = registry
        .get(Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip", 1024)
        .unwrap_err();
    assert!(!error.contains("X-Goog-Signature"));
    assert!(!error.contains("private"));
    done.send(()).unwrap();
    assert_eq!(server.join().unwrap().len(), 2);
}

#[test]
fn only_go_zip_endpoints_can_follow_the_fixed_mirror() {
    for (ecosystem, path) in [
        (Ecosystem::Go, "/example.com/a/@v/v1.0.0.mod"),
        (Ecosystem::Pip, "/pypi/a/json"),
        (Ecosystem::Npm, "/a"),
    ] {
        let (origin, done, server) = loopback_go_proxy(|origin, _| {
            (
                302,
                Some(format!("{origin}/proxy-golang-org-prod/archive.zip")),
                Vec::new(),
            )
        });
        let registry = dever_cli::libs::HttpRegistry::new(
            [
                (Ecosystem::Go, origin.clone()),
                (Ecosystem::Pip, origin.clone()),
                (Ecosystem::Npm, origin.clone()),
            ],
            [
                (Ecosystem::Go, vec![origin.clone()]),
                (Ecosystem::Pip, vec![origin.clone()]),
                (Ecosystem::Npm, vec![origin.clone()]),
            ],
        )
        .with_go_archive_mirror(&format!("{origin}/proxy-golang-org-prod/"))
        .unwrap();
        assert!(
            registry
                .get(ecosystem, path, 1024)
                .unwrap_err()
                .contains("redirect is not allowed")
        );
        done.send(()).unwrap();
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[test]
fn fixture_lock_is_offline_deterministic_and_cache_is_content_addressed() {
    let first: LibSpec = "pip:fixture@1.0.0".parse().unwrap();
    let second: LibSpec = "npm:fixture@1.0.0".parse().unwrap();
    let requested = [first, second];
    let lock = FixtureRegistry::builtin().resolve(&requested).unwrap();
    let bytes = lock.encode().unwrap();
    assert_eq!(LockFile::decode(&bytes).unwrap().encode().unwrap(), bytes);
    doctor(&lock).unwrap();

    let cache = FixtureArtifactStore::default();
    let digest = cache.publish(b"sdk fixture bytes", "linux-x86_64").unwrap();
    assert!(cache.has(&digest, "linux-x86_64").unwrap());
    assert!(!cache.has(&digest, "windows-x86_64").unwrap());
    assert_eq!(
        cache.verify(&digest, "linux-x86_64").unwrap(),
        b"sdk fixture bytes"
    );
    assert!(cache.verify(&digest, "windows-x86_64").is_err());

    let sdk = SdkFixture {
        ecosystem: Ecosystem::Pip,
        schema: "fixture-schema-pip-v1".into(),
    };
    sdk.handshake("fixture-schema-pip-v1").unwrap();
    assert!(sdk.handshake("fixture-schema-pip-v2").is_err());

    let root = project();
    fs::write(root.join("dever.lock"), bytes).unwrap();
    let decoded = LockFile::decode(&fs::read(root.join("dever.lock")).unwrap()).unwrap();
    assert_eq!(decoded.libs.len(), 2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn scoped_npm_names_are_checked() {
    assert!("npm:@dever/sdk@1.0.0".parse::<LibSpec>().is_ok());
    assert!("npm:@dever bad/sdk@1.0.0".parse::<LibSpec>().is_err());
    assert!("npm:@dever/SDK@1.0.0".parse::<LibSpec>().is_ok());
    assert!("npm:@dever/..@1.0.0".parse::<LibSpec>().is_err());
    assert!("go:example.com/../module@1.0.0".parse::<LibSpec>().is_err());
}

#[test]
fn project_preparation_requires_a_canonical_lock_and_reports_sizes() {
    let root = project();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/setting.json"),
        r#"{"lib":["pip:fixture@1.0.0"]}"#,
    )
    .unwrap();
    assert!(prepare(&root, &[]).is_err());
    let lock = FixtureRegistry::builtin()
        .resolve(&["pip:fixture@1.0.0".parse().unwrap()])
        .unwrap();
    lock.write_atomic(&root).unwrap();
    let report = prepare(&root, &[]).unwrap().unwrap();
    assert_eq!(report.libs, 1);
    assert_eq!(report.artifacts, 1);
    assert!(report.artifact_bytes > 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn lib_commands_preserve_project_settings_in_one_private_file() {
    let root = TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    let path = root.path().join("config/setting.json");
    fs::write(
        &path,
        r#"{"log":{"level":"warn"},"http":{"host":"127.0.0.1","port":9080}}"#,
    )
    .unwrap();
    assert!(prepare(root.path(), &[]).unwrap().is_none());
    let requested = vec!["pip:fixture@1.0.0".to_owned()];
    dever_cli::libs::execute("add", root.path(), &requested).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(document["lib"], serde_json::json!(requested));
    assert_eq!(document["log"]["level"], "warn");
    assert_eq!(document["http"]["port"], 9080);
    assert!(!root.path().join("config/lib.json").exists());
    assert_eq!(
        dever_runtime::config::Settings::load_project(root.path())
            .unwrap()
            .external_libs(),
        requested
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    dever_cli::libs::execute("remove", root.path(), &requested).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(document.get("lib").is_none());
    assert_eq!(document["log"]["level"], "warn");
    assert!(prepare(root.path(), &[]).unwrap().is_none());
}

#[test]
fn lib_remove_prunes_the_existing_lock_without_resolving_again() {
    let root = TemporaryDirectory::new();
    let requested = [
        "pip:fixture@1.0.0".to_owned(),
        "npm:fixture@1.0.0".to_owned(),
    ];
    dever_cli::libs::execute("add", root.path(), &requested).unwrap();
    dever_cli::libs::execute("remove", root.path(), &requested[..1]).unwrap();
    let lock = LockFile::decode(&fs::read(root.path().join("dever.lock")).unwrap()).unwrap();
    assert_eq!(
        lock.libs
            .iter()
            .map(|lib| lib.spec.key())
            .collect::<Vec<_>>(),
        vec![requested[1].clone()]
    );
    assert_eq!(
        dever_cli::libs::execute("list", root.path(), &[]).unwrap(),
        requested[1]
    );
}

#[test]
fn unmanaged_cli_cannot_resolve_real_packages_or_change_settings() {
    let root = TemporaryDirectory::new();
    let requested = vec!["pip:requests@2.31.0".to_owned()];
    let error = dever_cli::libs::execute("add", root.path(), &requested).unwrap_err();
    assert!(error.contains("signed managed Dever release"));
    assert!(!root.path().join("dever.lock").exists());
    assert!(!root.path().join("config/setting.json").exists());
}

#[test]
fn malformed_or_stale_lib_configuration_is_never_ignored() {
    let root = TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    for invalid in [
        r#"{"lib":null}"#,
        r#"{"lib":[1]}"#,
        r#"{"lib":["pip:fixture@latest"]}"#,
        r#"{"lib":[],"lib":[]}"#,
    ] {
        fs::write(root.path().join("config/setting.json"), invalid).unwrap();
        assert!(prepare(root.path(), &[]).is_err(), "accepted {invalid}");
    }
    fs::write(root.path().join("config/setting.json"), "{}").unwrap();
    fs::write(root.path().join("config/lib.json"), "{}").unwrap();
    assert!(
        prepare(root.path(), &[])
            .unwrap_err()
            .contains("not supported")
    );
}

#[cfg(unix)]
#[test]
fn lib_commands_do_not_follow_configuration_symlinks() {
    let root = TemporaryDirectory::new();
    let outside = TemporaryDirectory::new();
    fs::write(outside.path().join("setting.json"), "{}").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("config")).unwrap();
    let error =
        dever_cli::libs::execute("add", root.path(), &["pip:fixture@1.0.0".into()]).unwrap_err();
    assert!(error.contains("symbolic link"));
    assert_eq!(
        fs::read_to_string(outside.path().join("setting.json")).unwrap(),
        "{}"
    );
    assert!(!root.path().join("dever.lock").exists());
}

#[test]
fn lock_binds_the_checked_worker_contract_and_rejects_schema_drift() {
    let root = TemporaryDirectory::new();
    let domain = root.path().join("module/notification/mail");
    fs::create_dir_all(&domain).unwrap();
    fs::write(
        domain.join("port.dever"),
        "read() (value: Int) fails app.ReadResult\n",
    )
    .unwrap();
    fs::write(domain.join("app.dever"), "type ReadResult { error Unavailable(message: Text) }\nread() (value: Int) { value = port.read() }\n").unwrap();
    let adapter =
        "setting { endpoint: Text }\nexternal exec \"worker\" { lib \"pip:fixture@1.0.0\" }\n";
    fs::write(domain.join("adapter.dever"), adapter).unwrap();
    fn checked(root: &std::path::Path) -> dever_core::hir::Program {
        let sources = dever_core::source::SourceMap::load(&root.join("module")).unwrap();
        dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"))
    }
    let program = checked(root.path());
    assert!(
        dever_cli::libs::prepare_program(
            root.path(),
            &program,
            dever_cli::toolchain::BuildTarget::host().unwrap()
        )
        .is_err()
    );
    dever_cli::libs::execute("add", root.path(), &[]).unwrap();
    let lock = LockFile::decode(&fs::read(root.path().join("dever.lock")).unwrap()).unwrap();
    assert_eq!(lock.workers.len(), 1);
    assert_eq!(
        lock.workers[0].schema,
        program.external_worker_contracts()[0].schema
    );
    assert_ne!(lock.workers[0].schema, lock.libs[0].schema);
    assert!(
        dever_cli::libs::prepare_program(
            root.path(),
            &program,
            dever_cli::toolchain::BuildTarget::host().unwrap()
        )
        .is_ok()
    );
    fs::write(
        domain.join("adapter.dever"),
        adapter.replace("endpoint: Text", "endpoint: Int"),
    )
    .unwrap();
    let changed = checked(root.path());
    assert!(
        dever_cli::libs::prepare_program(
            root.path(),
            &changed,
            dever_cli::toolchain::BuildTarget::host().unwrap()
        )
        .unwrap_err()
        .contains("binding changed")
    );
    assert!(dever_cli::libs::execute("doctor", root.path(), &[]).is_err());
    dever_cli::libs::execute("update", root.path(), &[]).unwrap();
    assert!(
        dever_cli::libs::prepare_program(
            root.path(),
            &changed,
            dever_cli::toolchain::BuildTarget::host().unwrap()
        )
        .is_ok()
    );
    assert!(
        dever_cli::libs::execute("remove", root.path(), &["pip:fixture@1.0.0".into()])
            .unwrap_err()
            .contains("declared by an external Adapter")
    );
}

#[test]
fn independent_worker_environments_can_lock_different_library_versions() {
    use dever_cli::libs::{
        FixtureArtifact, FixturePackage, LockedWorker, RuntimePack, resolve_workers,
    };
    let first: LibSpec = "pip:library@1.0.0".parse().unwrap();
    let second: LibSpec = "pip:library@2.0.0".parse().unwrap();
    let mut registry = FixtureRegistry::default();
    for spec in [&first, &second] {
        registry
            .register(FixturePackage {
                spec: spec.clone(),
                dependencies: Vec::new(),
                runtime: RuntimePack {
                    name: "fixture-python".into(),
                    version: spec.version.clone(),
                    sha256: "00".repeat(32),
                },
                artifacts: vec![FixtureArtifact {
                    target: "host".into(),
                    path: format!("{}/library", spec.version),
                    bytes: vec![1],
                }],
                schema: "package-metadata".into(),
            })
            .unwrap();
    }
    let worker = |adapter: &str, libs| LockedWorker {
        port: "notification.mail".into(),
        adapter: adapter.into(),
        ecosystem: "exec".into(),
        runtime: None,
        entry: format!("worker/{adapter}"),
        schema: "checked-port".into(),
        operations: vec!["read".into()],
        capabilities: Vec::new(),
        libs,
    };
    let lock = resolve_workers(
        &registry,
        &[],
        vec![
            worker("first", vec![first.clone()]),
            worker("second", vec![second.clone()]),
        ],
    )
    .unwrap();
    assert_eq!(lock.libs.len(), 2);
    assert_eq!(lock.workers.len(), 2);
    doctor(&lock).unwrap();
    let error = resolve_workers(
        &registry,
        &[],
        vec![worker("conflict", vec![first.clone(), second.clone()])],
    )
    .unwrap_err();
    assert!(error.contains("version conflict"));
    let mut invalid = lock;
    invalid.workers[0].libs = vec![first, second];
    assert!(
        doctor(&invalid)
            .unwrap_err()
            .contains("conflicting versions")
    );
}

#[test]
fn managed_worker_without_libs_still_locks_its_runtime() {
    use dever_cli::libs::LockedWorker;

    let mut lock = LockFile::new(Vec::new()).unwrap();
    lock.workers.push(LockedWorker {
        port: "notification.mail".into(),
        adapter: "python".into(),
        ecosystem: "pip".into(),
        runtime: Some(registry_runtime(Ecosystem::Pip).pack),
        entry: "worker/entry.py".into(),
        schema: "checked-port".into(),
        operations: vec!["read".into()],
        capabilities: Vec::new(),
        libs: Vec::new(),
    });

    let encoded = lock.encode().unwrap();
    let decoded = LockFile::decode(&encoded).unwrap();
    doctor(&decoded).unwrap();
    assert_eq!(decoded.workers[0].runtime, lock.workers[0].runtime);

    lock.workers[0].runtime = None;
    assert!(lock.encode().unwrap_err().contains("runtime pack identity"));
}

#[test]
fn expanded_worker_resources_keep_archives_needed_by_exec_or_declarations() {
    use dever_cli::libs::{
        LockedArtifact, LockedDependency, LockedLib, LockedWorker, prune_expanded_resources,
    };
    use dever_core::native::EmbeddedResource;

    let shared: LibSpec = "pip:shared@1.0.0".parse().unwrap();
    let dependency: LibSpec = "pip:dependency@1.0.0".parse().unwrap();
    let managed_only: LibSpec = "pip:managed-only@1.0.0".parse().unwrap();
    let unrelated: LibSpec = "npm:unrelated@1.0.0".parse().unwrap();
    let target = dever_cli::toolchain::platform_identity();
    let path =
        |spec: &LibSpec| format!("lib/{}/{}/archive.whl", spec.ecosystem.as_str(), spec.name);
    let locked = |spec: &LibSpec, dependencies: Vec<LibSpec>| LockedLib {
        build: None,
        spec: spec.clone(),
        dependencies: dependencies
            .into_iter()
            .map(|spec| LockedDependency { spec })
            .collect(),
        runtime: registry_runtime(spec.ecosystem.clone()).pack,
        artifacts: vec![LockedArtifact {
            source: None,
            target: target.clone(),
            path: path(spec),
            bytes: 1,
            sha256: digest(b"x"),
        }],
        schema: "checked".into(),
    };
    let mut lock = LockFile::new(vec![
        locked(&shared, vec![dependency.clone()]),
        locked(&dependency, vec![]),
        locked(&managed_only, vec![]),
        locked(&unrelated, vec![]),
    ])
    .unwrap();
    lock.npm
        .push(dever_cli::libs::npm::Environment::single(unrelated.clone()));
    let worker = |ecosystem: &str, adapter: &str, libs: Vec<LibSpec>| LockedWorker {
        port: "notification.mail".into(),
        adapter: adapter.into(),
        ecosystem: ecosystem.into(),
        runtime: (ecosystem != "exec").then(|| registry_runtime(ecosystem.parse().unwrap()).pack),
        entry: format!("worker/{adapter}"),
        schema: "checked".into(),
        operations: vec!["send".into()],
        capabilities: vec![],
        libs,
    };
    let managed = worker("pip", "managed", vec![shared.clone(), managed_only.clone()]);
    let exec = worker("exec", "exec", vec![shared.clone()]);
    let npm = worker("npm", "empty", vec![]);
    let runtime_path = |ecosystem: &str| format!("lib/runtime/{ecosystem}/{target}/runtime.pack");
    let paths = [
        path(&shared),
        path(&dependency),
        path(&managed_only),
        path(&unrelated),
        runtime_path("pip"),
        runtime_path("npm"),
        "other/resource".into(),
    ];
    let resources = || {
        paths
            .iter()
            .map(|path| EmbeddedResource {
                path: path.clone(),
                bytes: vec![1],
                sha256: digest(&[1]),
                executable: false,
            })
            .collect::<Vec<_>>()
    };

    let mut with_exec = resources();
    prune_expanded_resources(
        &lock,
        &[managed.clone(), exec, npm.clone()],
        &[],
        &mut with_exec,
    )
    .unwrap();
    let kept = with_exec
        .iter()
        .map(|resource| resource.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(kept.contains(path(&shared).as_str()));
    assert!(kept.contains(path(&dependency).as_str()));
    assert!(kept.contains(runtime_path("pip").as_str()));
    assert!(!kept.contains(path(&managed_only).as_str()));
    assert!(!kept.contains(runtime_path("npm").as_str()));
    assert!(kept.contains(path(&unrelated).as_str()));
    assert!(kept.contains("other/resource"));

    let mut managed_only_resources = resources();
    prune_expanded_resources(
        &lock,
        &[managed.clone(), npm.clone()],
        std::slice::from_ref(&unrelated),
        &mut managed_only_resources,
    )
    .unwrap();
    let kept = managed_only_resources
        .iter()
        .map(|resource| resource.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!kept.contains(path(&shared).as_str()));
    assert!(!kept.contains(path(&dependency).as_str()));
    assert!(!kept.contains(runtime_path("pip").as_str()));
    assert!(kept.contains(path(&unrelated).as_str()));

    let mut declared_resources = resources();
    prune_expanded_resources(
        &lock,
        &[managed, npm],
        std::slice::from_ref(&managed_only),
        &mut declared_resources,
    )
    .unwrap();
    let kept = declared_resources
        .iter()
        .map(|resource| resource.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(kept.contains(path(&managed_only).as_str()));
    assert!(kept.contains(runtime_path("pip").as_str()));
    assert!(!kept.contains(path(&shared).as_str()));
}

#[test]
fn embedding_rejects_extra_artifacts_and_drifted_provider_metadata() {
    let root = TemporaryDirectory::new();
    let original = FixtureRegistry::builtin()
        .resolve(&[
            "pip:fixture@1.0.0".parse().unwrap(),
            "npm:fixture@1.0.0".parse().unwrap(),
        ])
        .unwrap();
    original.write_atomic(root.path()).unwrap();
    assert_eq!(
        dever_cli::libs::embedded_resources(root.path())
            .unwrap()
            .len(),
        2
    );
    for field in ["runtime", "schema", "artifact", "dependency"] {
        let mut changed = original.clone();
        match field {
            "runtime" => changed.libs[0].runtime.sha256 = "11".repeat(32),
            "schema" => changed.libs[0].schema = "other-schema".into(),
            "artifact" => {
                let mut extra = changed.libs[0].artifacts[0].clone();
                extra.path = "extra/resource".into();
                changed.libs[0].artifacts.push(extra);
            }
            "dependency" => {
                let dependency = changed.libs[1].spec.clone();
                changed.libs[0]
                    .dependencies
                    .push(dever_cli::libs::LockedDependency { spec: dependency });
            }
            _ => unreachable!(),
        }
        changed.write_atomic(root.path()).unwrap();
        assert!(
            dever_cli::libs::embedded_resources(root.path())
                .unwrap_err()
                .contains("metadata"),
            "accepted {field} drift"
        );
    }
}

#[test]
fn provider_boundary_requires_a_signed_runtime_without_path_fallback() {
    let spec: LibSpec = "pip:requests@2.31.0".parse().unwrap();
    let error = provider_manifest(Ecosystem::Pip).blocked_message(&spec);
    assert!(error.contains("managed-cpython"));
    assert!(error.contains("requires a signed"));
    assert!(!error.contains("install the managed"));
    assert!(error.contains("never use PATH"));
    let registry = FixtureRegistry::builtin();
    assert!(registry.resolve(std::slice::from_ref(&spec)).is_err());
}

#[test]
fn locked_package_dependencies_and_artifacts_are_verified_before_worker_use() {
    let root = project();
    let root_spec: LibSpec = "pip:fixture@1.0.0".parse().unwrap();
    let lock = FixtureRegistry::builtin()
        .resolve(std::slice::from_ref(&root_spec))
        .unwrap();
    let cache = FixtureArtifactStore::default();
    let bytes = b"dever fixture pip:fixture@1.0.0\n";
    let digest = cache.publish(bytes, "host").unwrap();
    assert_eq!(digest, lock.libs[0].artifacts[0].sha256);
    let verified = verify_locked_artifacts(&lock, &cache, "linux-x86_64").unwrap();
    assert_eq!(verified.len(), 1);
    assert_eq!(verified[0].lib, root_spec);
    assert_eq!(verified[0].bytes, bytes);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn doctor_rejects_missing_and_cyclic_transitive_dependencies() {
    let mut registry = FixtureRegistry::default();
    let a: LibSpec = "pip:a@1.0.0".parse().unwrap();
    let b: LibSpec = "pip:b@1.0.0".parse().unwrap();
    let runtime = dever_cli::libs::RuntimePack {
        name: "fixture".into(),
        version: "1.0.0".into(),
        sha256: "00".repeat(32),
    };
    let artifact = || dever_cli::libs::FixtureArtifact {
        target: "host".into(),
        path: "worker.bin".into(),
        bytes: vec![1],
    };
    registry
        .register(dever_cli::libs::FixturePackage {
            spec: a.clone(),
            dependencies: vec![b.clone()],
            runtime: runtime.clone(),
            artifacts: vec![artifact()],
            schema: "a".into(),
        })
        .unwrap();
    registry
        .register(dever_cli::libs::FixturePackage {
            spec: b.clone(),
            dependencies: vec![a.clone()],
            runtime,
            artifacts: vec![artifact()],
            schema: "b".into(),
        })
        .unwrap();
    let lock = registry.resolve(&[a]).unwrap();
    assert!(
        doctor(&lock)
            .unwrap_err()
            .contains("cyclic locked lib dependency")
    );
}
