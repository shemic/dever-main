use super::*;
use dever_cli::libs::build::{Inputs, pack};
use dever_cli::libs::restore::restore_artifacts;

pub(super) struct RuntimeOnly;

pub(super) fn archives_only(lock: &LockFile, registry: &LocalRegistry) -> LocalRegistry {
    fn collect(
        lock: &LockFile,
        registry: &LocalRegistry,
        files: &mut BTreeMap<(Ecosystem, String), Vec<u8>>,
    ) {
        for artifact in lock
            .libs
            .iter()
            .flat_map(|lib| &lib.artifacts)
            .chain(lock.builds.iter().flat_map(|receipt| &receipt.sources))
        {
            if let Some(source) = &artifact.source {
                let key = (source.ecosystem.clone(), source.locator.clone());
                files.insert(key.clone(), registry.0[&key].clone());
            }
        }
        for receipt in &lock.builds {
            collect(&receipt.inputs, registry, files);
        }
    }
    let mut files = BTreeMap::new();
    collect(lock, registry, &mut files);
    LocalRegistry(files)
}
impl Inputs for RuntimeOnly {
    fn runtime(&self, ecosystem: Ecosystem) -> Result<(RegistryRuntime, Vec<u8>), String> {
        Ok((registry_runtime(ecosystem), vec![]))
    }
    fn tools(&self, _: Ecosystem) -> Result<(pack::Descriptor, Vec<u8>), String> {
        panic!("archive restoration must not load build extensions")
    }
    fn assets(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        panic!("archive restoration must not execute a sandbox")
    }
}

#[test]
fn cold_cache_restore_uses_only_locked_sources_and_never_registry_metadata() {
    let mut registry = local_registry();
    let original = FixtureArtifactStore::default();
    let roots = ["pip:a@1.0.0", "npm:root@1.0.0", "go:example.com/a@1.0.0"]
        .map(|spec| spec.parse().unwrap());
    let lock = dever_cli::libs::LibResolver::resolve(&local_resolver(&registry, &original), &roots)
        .unwrap();
    let before = lock.encode().unwrap();
    let locators = lock
        .libs
        .iter()
        .flat_map(|lib| &lib.artifacts)
        .map(|artifact| {
            let source = artifact.source.as_ref().unwrap();
            assert!(!source.filename.is_empty());
            (source.ecosystem.clone(), source.locator.clone())
        })
        .collect::<std::collections::BTreeSet<_>>();
    registry.0.retain(|key, _| locators.contains(key));
    let cold = FixtureArtifactStore::default();
    restore_artifacts(
        &lock,
        &registry,
        &cold,
        &RuntimeOnly,
        "linux-x86_64",
        &sumdb_tests::verifier(),
    )
    .unwrap();
    assert_eq!(lock.encode().unwrap(), before);
    assert_eq!(
        verify_locked_artifacts(&lock, &cold, "linux-x86_64").unwrap(),
        verify_locked_artifacts(&lock, &original, "linux-x86_64").unwrap()
    );
    // A warm restore must also succeed with no available registry at all.
    restore_artifacts(
        &lock,
        &LocalRegistry(BTreeMap::new()),
        &cold,
        &RuntimeOnly,
        "linux-x86_64",
        &sumdb_tests::verifier(),
    )
    .unwrap();
}

#[test]
fn restored_archive_mismatch_fails_before_cache_publication() {
    let mut registry = local_registry();
    let lock = dever_cli::libs::LibResolver::resolve(
        &local_resolver(&registry, &FixtureArtifactStore::default()),
        &["pip:a@1.0.0".parse().unwrap()],
    )
    .unwrap();
    let source = lock.libs[0].artifacts[0].source.as_ref().unwrap();
    registry.0.insert(
        (source.ecosystem.clone(), source.locator.clone()),
        b"different bytes".to_vec(),
    );
    let cold = FixtureArtifactStore::default();
    assert!(
        restore_artifacts(
            &lock,
            &registry,
            &cold,
            &RuntimeOnly,
            "linux-x86_64",
            &sumdb_tests::verifier()
        )
        .unwrap_err()
        .contains("byte/hash")
    );
    let artifact = &lock.libs[0].artifacts[0];
    assert!(!cold.has(&artifact.sha256, &artifact.target).unwrap());
}

#[test]
fn corrupt_cache_is_an_error_and_never_falls_back_to_download() {
    struct CorruptCache;
    impl ArtifactStore for CorruptCache {
        fn has(&self, _: &str, _: &str) -> Result<bool, String> {
            unreachable!()
        }
        fn publish(&self, _: &[u8], _: &str) -> Result<String, String> {
            panic!("corrupt cache cannot be replaced")
        }
        fn get(&self, _: &str, _: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(Some(b"corrupt".to_vec()))
        }
    }
    let registry = local_registry();
    let lock = dever_cli::libs::LibResolver::resolve(
        &local_resolver(&registry, &FixtureArtifactStore::default()),
        &["pip:a@1.0.0".parse().unwrap()],
    )
    .unwrap();
    // No registry entries exist: a fallback would produce a distinct missing-path error.
    assert!(
        restore_artifacts(
            &lock,
            &LocalRegistry(BTreeMap::new()),
            &CorruptCache,
            &RuntimeOnly,
            "linux-x86_64",
            &sumdb_tests::verifier()
        )
        .unwrap_err()
        .contains("byte/hash")
    );
}

#[test]
fn lib_install_preserves_setting_and_lock_bytes_and_rejects_declaration_drift() {
    let root = TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    let setting = b"{\n  \"lib\": [\"pip:fixture@1.0.0\"]\n}\n";
    fs::write(root.path().join("config/setting.json"), setting).unwrap();
    assert!(
        dever_cli::libs::execute("install", root.path(), &[])
            .unwrap_err()
            .contains("existing dever.lock")
    );
    let lock = FixtureRegistry::builtin()
        .resolve(&["pip:fixture@1.0.0".parse().unwrap()])
        .unwrap();
    lock.write_atomic(root.path()).unwrap();
    let before = fs::read(root.path().join("dever.lock")).unwrap();
    dever_cli::libs::execute("install", root.path(), &[]).unwrap();
    assert_eq!(
        fs::read(root.path().join("config/setting.json")).unwrap(),
        setting
    );
    assert_eq!(fs::read(root.path().join("dever.lock")).unwrap(), before);
    assert!(
        dever_cli::libs::execute("install", root.path(), &["pip:fixture@1.0.0".into()])
            .unwrap_err()
            .contains("no Lib specs")
    );
    fs::write(root.path().join("config/setting.json"), b"{\"lib\":[]}").unwrap();
    assert!(
        dever_cli::libs::execute("install", root.path(), &[])
            .unwrap_err()
            .contains("declared closure")
    );
    assert_eq!(fs::read(root.path().join("dever.lock")).unwrap(), before);
}
