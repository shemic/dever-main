use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::io::Read;
use std::io::{Cursor, Write};
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::Command;

use dever_cli::libs::{ArtifactStore, FixtureArtifactStore, LockFile};
#[cfg(unix)]
use dever_cli::libs::{LockedWorker, RuntimePack};
#[cfg(unix)]
use dever_cli::packages::LockedPackage;
use dever_cli::packages::{
    PackageTransport, execute_with, owned_files_with, owned_worker_bytes_with, owns_worker,
    sources_with,
};
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use dever_cli::toolchain::cache_status;
#[cfg(unix)]
use dever_cli::toolchain::{
    Artifact, Layout, MachineManager, ReleaseManifest, Version, artifact_get, artifact_put,
};
#[cfg(unix)]
use dever_cli::workers;
#[cfg(unix)]
use dever_core::native::EmbeddedResource;
#[cfg(unix)]
use ring::rand::SystemRandom;
#[cfg(unix)]
use ring::signature::{Ed25519KeyPair, KeyPair};
use sha2::{Digest, Sha256};

#[path = "../dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(unix)]
#[path = "support/daemon.rs"]
mod daemon;
#[cfg(unix)]
use daemon::TestDaemon;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "support/llvm_pack.rs"]
mod llvm_pack;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "packages/managed_compile.rs"]
mod managed_compile;

use temp::TemporaryDirectory;

struct Registry(BTreeMap<String, Vec<u8>>);

impl PackageTransport for Registry {
    fn get(&self, path: &str, limit: usize) -> Result<Vec<u8>, String> {
        let bytes = self
            .0
            .get(path)
            .ok_or_else(|| format!("missing registry path {path}"))?;
        if bytes.len() > limit {
            return Err("registry response exceeds limit".into());
        }
        Ok(bytes.clone())
    }
}

fn archive(
    name: &str,
    version: &str,
    dependencies: serde_json::Value,
    libs: &[&str],
    source: &str,
) -> Vec<u8> {
    archive_with_sources(name, version, dependencies, libs, source, &[])
}

fn archive_with_sources(
    name: &str,
    version: &str,
    dependencies: serde_json::Value,
    libs: &[&str],
    source: &str,
    additional_sources: &[(&str, &str)],
) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in [
        (
            "dever-package.json".to_owned(),
            serde_json::to_vec(&serde_json::json!({
                "format": "dever-package-v1", "name": name, "version": version,
                "dependencies": dependencies, "lib": libs,
            }))
            .unwrap(),
        ),
        (
            format!("module/{name}/value/app.dever"),
            source.as_bytes().to_vec(),
        ),
    ] {
        writer
            .start_file(path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    for (path, source) in additional_sources {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(source.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn registry(archives: &[(&str, &str, Vec<u8>)]) -> Registry {
    let mut paths = BTreeMap::new();
    let mut versions = BTreeMap::<String, Vec<serde_json::Value>>::new();
    for (name, version, bytes) in archives {
        versions
            .entry((*name).into())
            .or_default()
            .push(serde_json::json!({
                "version": version, "sha256": digest(bytes), "bytes": bytes.len(),
            }));
        paths.insert(format!("/v1/packages/{name}/{version}.zip"), bytes.clone());
    }
    for (name, versions) in versions {
        paths.insert(
            format!("/v1/packages/{name}/index.json"),
            serde_json::to_vec(&serde_json::json!({
                "format": "dever-package-index-v1", "name": name, "versions": versions,
            }))
            .unwrap(),
        );
    }
    Registry(paths)
}

fn project() -> TemporaryDirectory {
    let root = TemporaryDirectory::new();
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::create_dir_all(root.path().join("module/portal/value")).unwrap();
    fs::write(
        root.path().join("config/setting.json"),
        r#"{"package":{"registry":"https://packages.example.test","use":[]}}"#,
    )
    .unwrap();
    fs::write(
        root.path().join("module/portal/value/app.dever"),
        "value() (result: Text) { result = storefront.value.value() }\n",
    )
    .unwrap();
    fs::write(
        root.path().join("module/portal/value/api.dever"),
        "cmd show = app.value\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("test/portal/value")).unwrap();
    fs::write(
        root.path().join("test/portal/value/smoke.dever"),
        "smoke() () { assert_eq(portal.value.value(), \"two\") }\n",
    )
    .unwrap();
    root
}

#[test]
fn package_add_resolves_transitive_versions_and_feeds_the_common_lib_lock() {
    let root = project();
    let registry = registry(&[
        (
            "storefront",
            "1.0.0",
            archive(
                "storefront",
                "1.0.0",
                serde_json::json!({"catalog":"^1.0.0"}),
                &[],
                "value() (result: Text) { result = catalog.value.value() }\n",
            ),
        ),
        (
            "catalog",
            "1.0.0",
            archive(
                "catalog",
                "1.0.0",
                serde_json::json!({}),
                &["pip:fixture@1.0.0"],
                "value() (result: Text) { result = \"one\" }\n",
            ),
        ),
        (
            "catalog",
            "1.2.0",
            archive(
                "catalog",
                "1.2.0",
                serde_json::json!({}),
                &["pip:fixture@1.0.0"],
                "value() (result: Text) { result = \"two\" }\n",
            ),
        ),
        (
            "catalog",
            "2.0.0",
            archive(
                "catalog",
                "2.0.0",
                serde_json::json!({}),
                &[],
                "value() (result: Text) { result = \"wrong\" }\n",
            ),
        ),
    ]);
    let store = FixtureArtifactStore::default();
    execute_with(
        "add",
        root.path(),
        &["storefront@^1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap();
    let lock = LockFile::decode(&fs::read(root.path().join("dever.lock")).unwrap()).unwrap();
    assert_eq!(
        lock.packages
            .iter()
            .map(|package| (package.name.as_str(), package.version.as_str()))
            .collect::<Vec<_>>(),
        [("catalog", "1.2.0"), ("storefront", "1.0.0")]
    );
    assert_eq!(lock.libs.len(), 1);
    assert_eq!(lock.libs[0].spec.key(), "pip:fixture@1.0.0");
    assert!(owns_worker(root.path(), "worker/catalog/tool").unwrap());
    assert!(owns_worker(root.path(), "module/catalog/value/worker/main.py").unwrap());
    assert!(!owns_worker(root.path(), "module/portal/value/worker/main.py").unwrap());
    let files = owned_files_with(root.path(), &store).unwrap();
    assert!(String::from_utf8_lossy(&files["module/catalog/value/app.dever"]).contains("two"));
    assert!(execute_with("doctor", root.path(), &[], &registry, &store).is_ok());

    let lock_before = fs::read(root.path().join("dever.lock")).unwrap();
    let setting_before = fs::read(root.path().join("config/setting.json")).unwrap();
    let archives_only = Registry(
        registry
            .0
            .iter()
            .filter(|(path, _)| path.ends_with(".zip"))
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect(),
    );
    let restored = FixtureArtifactStore::default();
    dever_cli::packages::restore_locked_with(root.path(), &lock, &archives_only, &restored)
        .unwrap();
    assert_eq!(owned_files_with(root.path(), &restored).unwrap(), files);
    assert_eq!(
        fs::read(root.path().join("dever.lock")).unwrap(),
        lock_before
    );
    assert_eq!(
        fs::read(root.path().join("config/setting.json")).unwrap(),
        setting_before
    );

    let mut damaged = lock.clone();
    damaged.packages[0].manifest_sha256 = "0".repeat(64);
    damaged.write_atomic(root.path()).unwrap();
    assert!(
        owned_files_with(root.path(), &store)
            .unwrap_err()
            .contains("manifest changed")
    );
}

#[test]
fn package_resolution_rejects_conflicts_without_changing_project_configuration() {
    let root = project();
    let before = fs::read(root.path().join("config/setting.json")).unwrap();
    let registry = registry(&[
        (
            "storefront",
            "1.0.0",
            archive(
                "storefront",
                "1.0.0",
                serde_json::json!({"catalog":"^2.0.0"}),
                &[],
                "value() (result: Text) { result = catalog.value.value() }\n",
            ),
        ),
        (
            "catalog",
            "1.0.0",
            archive(
                "catalog",
                "1.0.0",
                serde_json::json!({}),
                &[],
                "value() (result: Text) { result = \"one\" }\n",
            ),
        ),
    ]);
    let store = FixtureArtifactStore::default();
    let error = execute_with(
        "add",
        root.path(),
        &["storefront@1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap_err();
    assert!(error.contains("no Package version satisfies"), "{error}");
    assert_eq!(
        fs::read(root.path().join("config/setting.json")).unwrap(),
        before
    );
    assert!(!root.path().join("dever.lock").exists());
}

#[test]
fn package_archive_rejects_paths_owned_by_another_component() {
    let root = project();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in [
        (
            "dever-package.json",
            r#"{"format":"dever-package-v1","name":"storefront","version":"1.0.0"}"#,
        ),
        (
            "module/portal/value/app.dever",
            "value() (result: Text) { result = \"bad\" }",
        ),
    ] {
        writer
            .start_file(path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes.as_bytes()).unwrap();
    }
    let registry = registry(&[("storefront", "1.0.0", writer.finish().unwrap().into_inner())]);
    let store = FixtureArtifactStore::default();
    let error = execute_with(
        "add",
        root.path(),
        &["storefront@1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap_err();
    assert!(error.contains("unowned path"), "{error}");
}

#[test]
fn package_rejects_local_component_even_when_file_paths_do_not_overlap() {
    let root = project();
    fs::create_dir_all(root.path().join("module/storefront/other")).unwrap();
    fs::write(
        root.path().join("module/storefront/other/app.dever"),
        "value() (result: Text) { result = \"local\" }\n",
    )
    .unwrap();
    let registry = registry(&[(
        "storefront",
        "1.0.0",
        archive(
            "storefront",
            "1.0.0",
            serde_json::json!({}),
            &[],
            "value() (result: Text) { result = \"package\" }\n",
        ),
    )]);
    let store = FixtureArtifactStore::default();
    let error = execute_with(
        "add",
        root.path(),
        &["storefront@1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap_err();
    assert!(error.contains("collides with a local component"), "{error}");
    assert!(!root.path().join("dever.lock").exists());
}

#[test]
fn package_only_project_loads_without_a_local_module_directory() {
    let root = TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/setting.json"),
        r#"{"package":{"registry":"https://packages.example.test","use":[]}}"#,
    )
    .unwrap();
    let registry = registry(&[(
        "catalog",
        "1.0.0",
        archive(
            "catalog",
            "1.0.0",
            serde_json::json!({}),
            &[],
            "value() (result: Text) { result = \"ready\" }\n",
        ),
    )]);
    let store = FixtureArtifactStore::default();
    execute_with(
        "add",
        root.path(),
        &["catalog@1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap();
    let packages = sources_with(root.path(), &store).unwrap();
    let sources = dever_core::source::SourceMap::load_with_packages(
        &root.path().join("module"),
        None,
        &packages,
    )
    .unwrap();
    assert_eq!(sources.files().len(), 1);
    dever_core::check(&sources).unwrap();
}

#[test]
fn locked_package_worker_never_falls_back_to_a_local_file() {
    let root = project();
    let registry = registry(&[(
        "storefront",
        "1.0.0",
        archive(
            "storefront",
            "1.0.0",
            serde_json::json!({}),
            &[],
            "value() (result: Text) { result = \"two\" }\n",
        ),
    )]);
    let store = FixtureArtifactStore::default();
    execute_with(
        "add",
        root.path(),
        &["storefront@1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap();
    let local = root.path().join("worker/storefront/tool");
    fs::create_dir_all(local.parent().unwrap()).unwrap();
    fs::write(&local, b"local fallback must never execute").unwrap();
    assert!(owns_worker(root.path(), "worker/storefront/tool").unwrap());
    assert!(owns_worker(root.path(), "module/storefront/value/worker/main.py").unwrap());
    assert!(!owns_worker(root.path(), "worker/portal/tool").unwrap());
    let error = owned_worker_bytes_with(root.path(), "worker/storefront/tool", &store).unwrap_err();
    assert!(error.contains("missing from its locked archive"), "{error}");
    assert_eq!(
        owned_worker_bytes_with(root.path(), "worker/portal/tool", &store).unwrap(),
        None
    );
}

#[cfg(unix)]
#[test]
fn managed_package_worker_requires_its_locked_entry_and_rejects_local_collision() {
    let root = project();
    let (layout, executable) = signed_core(root.path(), &std::env::current_exe().unwrap(), false);
    let mut daemon = TestDaemon::start(&layout);
    let package_archive = archive_with_sources(
        "storefront",
        "1.0.0",
        serde_json::json!({}),
        &[],
        "type SendResult { error Unavailable(message: Text) }\nvalue() (result: Text) { result = \"two\" }\nsend(value: Int) (receipt: Int) { receipt = port.send(value) }\n",
        &[
            (
                "module/storefront/value/port.dever",
                "send(value: Int) (receipt: Int) fails app.SendResult\n",
            ),
            (
                "module/storefront/value/adapter.dever",
                "external pip \"worker/main.py\" { }\n",
            ),
        ],
    );
    fs::write(
        root.path().join("config/setting.json"),
        r#"{"package":{"registry":"https://packages.example.test","use":["storefront@1.0.0"]}}"#,
    )
    .unwrap();
    let store = ServiceStore(layout.clone());
    let package_sha256 = store.publish(&package_archive, "package").unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(&package_archive)).unwrap();
    let mut manifest = Vec::new();
    zip.by_name("dever-package.json")
        .unwrap()
        .read_to_end(&mut manifest)
        .unwrap();
    let mut lock = LockFile::new(Vec::new()).unwrap();
    lock.packages.push(LockedPackage {
        name: "storefront".into(),
        version: "1.0.0".into(),
        sha256: package_sha256,
        bytes: package_archive.len() as u64,
        dependencies: Vec::new(),
        manifest_sha256: digest(&manifest),
    });
    lock.write_atomic(root.path()).unwrap();

    let package_sources = dever_cli::packages::sources_with(root.path(), &store).unwrap();
    let sources = dever_core::source::SourceMap::load_with_packages(
        &root.path().join("module"),
        None,
        &package_sources,
    )
    .unwrap();
    let program = dever_core::check(&sources).unwrap();
    let contract = program.external_worker_contracts().remove(0);
    assert_eq!(contract.entry, "module/storefront/value/worker/main.py");
    assert!(owns_worker(root.path(), &contract.entry).unwrap());

    let target = dever_cli::toolchain::platform_identity();
    let metadata = serde_json::to_vec(&serde_json::json!({
        "format": "dever-worker-runtime-v1", "ecosystem": "pip", "target": target,
        "executable": "bin/python3", "arguments": ["-I", "-S", "-B"],
        "python_wheel_tags": ["py3-none-any"],
        "python_extension_suffixes": [".cpython-312-x86_64-linux-gnu.so", ".abi3.so", ".so"],
        "python_markers": {
            "implementation_name":"cpython", "implementation_version":"3.12.0", "os_name":"posix",
            "platform_machine":"x86_64", "platform_python_implementation":"CPython", "platform_release":"",
            "platform_system":"Linux", "platform_version":"", "python_full_version":"3.12.0",
            "python_version":"3.12", "sys_platform":"linux"
        },
    }))
    .unwrap();
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
    let mut tar = tar::Builder::new(encoder);
    for (path, bytes) in [
        ("dever-runtime.json", metadata.as_slice()),
        ("bin/python3", b"fixture-interpreter".as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, path, bytes).unwrap();
    }
    let runtime_bytes = tar.into_inner().unwrap().finish().unwrap();
    let runtime_sha256 = digest(&runtime_bytes);
    let runtime = RuntimePack {
        name: "fixture-pip".into(),
        version: "1".into(),
        sha256: runtime_sha256.clone(),
    };
    lock.workers.push(LockedWorker {
        port: contract.port,
        adapter: contract.adapter,
        ecosystem: contract.ecosystem,
        runtime: Some(runtime),
        entry: contract.entry.clone(),
        schema: contract.schema,
        capabilities: contract.capabilities,
        operations: contract.operations,
        libs: Vec::new(),
    });
    lock.write_atomic(root.path()).unwrap();
    fs::write(root.path().join("test-runtime.pack"), runtime_bytes).unwrap();
    let output = Command::new(executable)
        .args([
            "--exact",
            "installed_managed_package_worker_guard",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "signed child test failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    daemon.stop();
}

#[cfg(unix)]
#[test]
#[ignore = "run only from the signed private fixture"]
fn installed_managed_package_worker_guard() {
    let root = std::env::current_dir().unwrap();
    let package_sources = dever_cli::packages::sources(&root).unwrap();
    let sources = dever_core::source::SourceMap::load_with_packages(
        &root.join("module"),
        None,
        &package_sources,
    )
    .unwrap();
    let program = dever_core::check(&sources).unwrap();
    let entry = &program.external_worker_contracts()[0].entry;
    assert_eq!(entry, "module/storefront/value/worker/main.py");
    assert!(owns_worker(&root, entry).unwrap());
    let runtime_bytes = fs::read(root.join("test-runtime.pack")).unwrap();
    let resources = [EmbeddedResource {
        path: format!(
            "lib/runtime/pip/{}/runtime.pack",
            dever_cli::toolchain::platform_identity()
        ),
        sha256: digest(&runtime_bytes),
        bytes: runtime_bytes,
        executable: false,
    }];
    let error = workers::prepare(&root, &program, &resources).unwrap_err();
    assert!(
        error.contains("locked Package Worker entry") && error.contains("missing"),
        "{error}"
    );

    let local = root.join(entry);
    fs::create_dir_all(local.parent().unwrap()).unwrap();
    fs::write(local, b"local fallback must never execute").unwrap();
    let error = workers::prepare(&root, &program, &resources).unwrap_err();
    assert!(error.contains("collides with a local component"), "{error}");
}

#[test]
fn declared_package_worker_requires_a_lock_before_local_fallback() {
    let root = project();
    fs::write(
        root.path().join("config/setting.json"),
        r#"{"package":{"registry":"https://packages.example.test","use":["storefront@1.0.0"]}}"#,
    )
    .unwrap();
    let error = owns_worker(root.path(), "worker/storefront/tool").unwrap_err();
    assert!(error.contains("dever.lock"), "{error}");
}

#[test]
fn removing_a_package_keeps_locked_versions_without_registry_requests() {
    let root = project();
    let registry = registry(&[
        (
            "storefront",
            "1.0.0",
            archive(
                "storefront",
                "1.0.0",
                serde_json::json!({"catalog":"^1.0.0"}),
                &[],
                "value() (result: Text) { result = catalog.value.value() }\n",
            ),
        ),
        (
            "catalog",
            "1.2.0",
            archive(
                "catalog",
                "1.2.0",
                serde_json::json!({}),
                &["pip:fixture@1.0.0"],
                "value() (result: Text) { result = \"two\" }\n",
            ),
        ),
    ]);
    let store = FixtureArtifactStore::default();
    execute_with(
        "add",
        root.path(),
        &["storefront@1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap();
    execute_with(
        "add",
        root.path(),
        &["catalog@^1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap();
    fs::write(
        root.path().join("module/portal/value/app.dever"),
        "value() (result: Text) { result = catalog.value.value() }\n",
    )
    .unwrap();
    let offline = Registry(BTreeMap::new());
    execute_with(
        "remove",
        root.path(),
        &["storefront".into()],
        &offline,
        &store,
    )
    .unwrap();
    let lock = LockFile::decode(&fs::read(root.path().join("dever.lock")).unwrap()).unwrap();
    assert_eq!(lock.packages.len(), 1);
    assert_eq!(
        (
            lock.packages[0].name.as_str(),
            lock.packages[0].version.as_str()
        ),
        ("catalog", "1.2.0")
    );
    assert_eq!(lock.libs[0].spec.key(), "pip:fixture@1.0.0");
    assert!(
        !String::from_utf8_lossy(&fs::read(root.path().join("config/setting.json")).unwrap())
            .contains("storefront@")
    );
    fs::write(
        root.path().join("module/portal/value/app.dever"),
        "value() (result: Text) { result = \"two\" }\n",
    )
    .unwrap();
    execute_with("remove", root.path(), &["catalog".into()], &offline, &store).unwrap();
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("config/setting.json")).unwrap())
            .unwrap();
    assert_eq!(
        settings["package"]["registry"],
        "https://packages.example.test"
    );
    assert_eq!(settings["package"]["use"], serde_json::json!([]));
    execute_with(
        "add",
        root.path(),
        &["storefront@1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap();
    assert_eq!(
        LockFile::decode(&fs::read(root.path().join("dever.lock")).unwrap())
            .unwrap()
            .packages
            .len(),
        2
    );
}

#[cfg(unix)]
#[test]
fn package_update_restores_original_setting_bytes_when_lock_write_fails() {
    use std::os::unix::fs::symlink;

    let root = project();
    let registry = registry(&[(
        "storefront",
        "1.0.0",
        archive(
            "storefront",
            "1.0.0",
            serde_json::json!({}),
            &[],
            "value() (result: Text) { result = \"two\" }\n",
        ),
    )]);
    let store = FixtureArtifactStore::default();
    execute_with(
        "add",
        root.path(),
        &["storefront@1.0.0".into()],
        &registry,
        &store,
    )
    .unwrap();
    let settings = fs::read(root.path().join("config/setting.json")).unwrap();
    let lock_path = root.path().join("dever.lock");
    let saved = root.path().join("saved.lock");
    fs::rename(&lock_path, &saved).unwrap();
    symlink(&saved, &lock_path).unwrap();
    let error = execute_with("update", root.path(), &[], &registry, &store).unwrap_err();
    assert!(error.contains("symbolic link"), "{error}");
    assert_eq!(
        fs::read(root.path().join("config/setting.json")).unwrap(),
        settings
    );
    assert_eq!(fs::read(&saved).unwrap(), fs::read(&lock_path).unwrap());
}

#[cfg(unix)]
struct ServiceStore(Layout);

#[cfg(unix)]
impl ArtifactStore for ServiceStore {
    fn has(&self, _: &str, _: &str) -> Result<bool, String> {
        Err("exact length required".into())
    }
    fn publish(&self, bytes: &[u8], _: &str) -> Result<String, String> {
        Ok(artifact_put(&self.0, bytes)?.sha256)
    }
    fn get(&self, _: &str, _: &str) -> Result<Option<Vec<u8>>, String> {
        Err("exact length required".into())
    }
    fn get_exact(&self, sha256: &str, bytes: u64, _: &str) -> Result<Option<Vec<u8>>, String> {
        artifact_get(&self.0, sha256, bytes).map(Some)
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn installed_dever(root: &Path) -> (Layout, std::path::PathBuf) {
    signed_core(root, Path::new(env!("CARGO_BIN_EXE_dever")), true)
}

#[cfg(unix)]
fn signed_core(
    root: &Path,
    source_executable: &Path,
    health_checked_install: bool,
) -> (Layout, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let layout = Layout::new(root.join("machine"));
    layout.initialize().unwrap();
    let key_bytes = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let key = Ed25519KeyPair::from_pkcs8(key_bytes.as_ref()).unwrap();
    fs::write(
        layout.state().join("trusted-release-key"),
        format!("{}\n", hex(key.public_key().as_ref())),
    )
    .unwrap();
    let release = if health_checked_install {
        layout.downloads().join("0.1.0")
    } else {
        layout.versions().join("0.1.0")
    };
    fs::create_dir(&release).unwrap();
    let core = release.join("dever-core");
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    if health_checked_install {
        llvm_pack::compiler(source_executable, &release, "dever-core");
    } else {
        fs::copy(source_executable, &core).unwrap();
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    fs::copy(source_executable, &core).unwrap();
    fs::set_permissions(&core, fs::Permissions::from_mode(0o755)).unwrap();
    let bytes = fs::read(&core).unwrap();
    let artifacts = vec![Artifact {
        path: "dever-core".into(),
        bytes: bytes.len() as u64,
        sha256: digest(&bytes),
    }];
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let artifacts = if health_checked_install {
        artifacts
            .into_iter()
            .chain(native_release_inputs(&release))
            .collect()
    } else {
        artifacts
    };
    let manifest = ReleaseManifest::new(Version::parse("0.1.0").unwrap(), artifacts);
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(release.join("manifest.json"), &manifest_bytes).unwrap();
    fs::write(
        release.join("manifest.sig"),
        format!("{}\n", hex(key.sign(&manifest_bytes).as_ref())),
    )
    .unwrap();
    let manager = MachineManager::new(layout.clone());
    if health_checked_install {
        manager.install_requested("0.1.0").unwrap();
    } else {
        manager
            .resolve_core(&Version::parse("0.1.0").unwrap())
            .unwrap();
    }
    (layout.clone(), layout.versions().join("0.1.0/dever-core"))
}

#[cfg(unix)]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Explicit author inputs included in an ephemeral signature; these are not
/// the four trimmed runtime profiles of a formal distribution.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn native_release_inputs(root: &Path) -> Vec<Artifact> {
    let document = llvm_pack::manifest(root, true);
    let prefix = format!("runtime/{}", dever_cli::toolchain::platform_identity());
    let paths = document["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| format!("{prefix}/{}", file["path"].as_str().unwrap()))
        .chain([format!("{prefix}/manifest.json")])
        .chain(
            fs::read_dir(root.join("lib"))
                .unwrap()
                .map(|entry| format!("lib/{}", entry.unwrap().file_name().to_str().unwrap())),
        );
    paths
        .map(|path| {
            let file = root.join(&path);
            Artifact {
                path,
                bytes: fs::metadata(&file).unwrap().len(),
                sha256: dever_cli::toolchain::sha256_file(&file).unwrap(),
            }
        })
        .collect()
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn assert_cli_success(executable: &Path, arguments: &[&str]) -> std::process::Output {
    let output = Command::new(executable)
        .env_clear()
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} exited {}\nstdout: {}\nstderr: {}",
        arguments.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "requires explicitly prepared native runtime archive and owned daemon"]
fn installed_package_passes_check_test_run_build_and_standalone_execution() {
    let root = project();
    let (layout, executable) = installed_dever(root.path());
    let mut daemon = TestDaemon::start(&layout);
    let registry = registry(&[
        (
            "storefront",
            "1.0.0",
            archive(
                "storefront",
                "1.0.0",
                serde_json::json!({"catalog":"1.2.0"}),
                &[],
                "value() (result: Text) { result = catalog.value.value() }\n",
            ),
        ),
        (
            "catalog",
            "1.2.0",
            archive(
                "catalog",
                "1.2.0",
                serde_json::json!({}),
                &[],
                "value() (result: Text) { result = \"two\" }\n",
            ),
        ),
    ]);
    execute_with(
        "add",
        root.path(),
        &["storefront@1.0.0".into()],
        &registry,
        &ServiceStore(layout.clone()),
    )
    .unwrap();
    let project = root.path().to_str().unwrap();
    assert_cli_success(&executable, &["check", project]);
    assert_cli_success(&executable, &["test", project]);
    let run = assert_cli_success(
        &executable,
        &["run", project, "--", "portal.value.show", "{}"],
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains("two"));
    let output = root.path().join("standalone");
    assert_cli_success(
        &executable,
        &["build", project, "--output", output.to_str().unwrap()],
    );
    daemon.stop();
    let standalone = assert_cli_success(&output, &["portal.value.show", "{}"]);
    assert!(String::from_utf8_lossy(&standalone.stdout).contains("two"));
}
