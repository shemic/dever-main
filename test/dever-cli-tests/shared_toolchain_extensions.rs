use super::*;
use dever_cli::libs::Ecosystem;
use dever_cli::toolchain::{
    BuildTarget, Extension, ExtensionId, ExtensionKind, ExtensionSource, SignedResources,
    artifact_get_optional, prepare_extension_with,
};
use std::sync::atomic::AtomicUsize;

fn id(ecosystem: Ecosystem) -> ExtensionId {
    ExtensionId {
        kind: ExtensionKind::Runtime(ecosystem),
        target: BuildTarget::host().unwrap(),
    }
}

fn logical_path(id: &ExtensionId) -> String {
    let ExtensionKind::Runtime(ecosystem) = &id.kind else {
        panic!("runtime fixture")
    };
    format!(
        "runtime/{}/{}/runtime.pack",
        ecosystem.as_str(),
        id.target.platform()
    )
}

struct Source {
    requests: AtomicUsize,
    fail: bool,
}

impl Source {
    fn new(fail: bool) -> Self {
        Self {
            requests: AtomicUsize::new(0),
            fail,
        }
    }
}

impl ExtensionSource for Source {
    fn open(&self, version: &Version, extension: &ExtensionId) -> Result<Box<dyn Read>, String> {
        self.requests.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            return Err("fixture extension download failed".into());
        }
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_ustar();
        header.set_size(version.as_str().len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header,
            logical_path(extension),
            version.as_str().as_bytes(),
        )
        .unwrap();
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(&tar.into_inner().unwrap()).unwrap();
        Ok(Box::new(std::io::Cursor::new(encoder.finish().unwrap())))
    }
}

fn release(layout: &Layout, key: &Ed25519KeyPair, version: &str) {
    write_release(
        layout,
        key,
        version,
        &layout.root().join("dispatch"),
        true,
        true,
    );
    let directory = layout.downloads().join(version);
    let path = directory.join("manifest.json");
    let mut manifest: ReleaseManifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for ecosystem in [Ecosystem::Pip, Ecosystem::Npm] {
        let id = id(ecosystem);
        manifest.extensions.push(Extension {
            kind: id.kind.clone(),
            target: id.target,
            artifacts: vec![Artifact {
                path: logical_path(&id),
                bytes: version.len() as u64,
                sha256: hex(&Sha256::digest(version.as_bytes())),
            }],
        });
    }
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(path, &bytes).unwrap();
    fs::write(
        directory.join("manifest.sig"),
        hex(key.sign(&bytes).as_ref()),
    )
    .unwrap();
}

fn machine() -> (TemporaryDirectory, Layout, MachineManager, Ed25519KeyPair) {
    let root = TemporaryDirectory::new();
    let layout = Layout::new(root.path().join("machine"));
    layout.initialize().unwrap();
    let key = signing_key();
    fs::write(
        layout.state().join("trusted-release-key"),
        hex(key.public_key().as_ref()),
    )
    .unwrap();
    let manager = MachineManager::new(layout.clone());
    (root, layout, manager, key)
}

#[test]
fn signed_extension_preparation_is_shared_immutable_and_concurrent() {
    let (_root, layout, manager, key) = machine();
    release(&layout, &key, "1.0.0");
    let version = manager.install_requested("1.0.0").unwrap();
    let extension = id(Ecosystem::Pip);
    let logical = logical_path(&extension);
    let resources = SignedResources::load(&layout, &version).unwrap();
    assert!(
        resources
            .read(&logical)
            .unwrap_err()
            .contains("dever lib install")
    );
    let source = Source::new(false);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| prepare_extension_with(&layout, &version, &extension, &source).unwrap());
        }
    });
    assert_eq!(source.requests.load(Ordering::Relaxed), 1);
    assert_eq!(resources.read(&logical).unwrap(), b"1.0.0");
    let path = resources.path(&logical).unwrap();
    assert!(path.starts_with(layout.extensions()));
    assert!(!layout.versions().join("1.0.0").join(&logical).exists());
    fs::write(&path, b"other").unwrap();
    assert!(resources.read(&logical).unwrap_err().contains("integrity"));
    assert!(
        prepare_extension_with(&layout, &version, &extension, &source)
            .unwrap_err()
            .contains("integrity")
    );
    assert_eq!(source.requests.load(Ordering::Relaxed), 1);
}

#[test]
fn update_prepares_only_installed_extensions_and_preserves_active_on_failure() {
    let (_root, layout, manager, key) = machine();
    for version in ["1.0.0", "2.0.0"] {
        release(&layout, &key, version);
    }
    let old = manager.install_requested("1.0.0").unwrap();
    let extension = id(Ecosystem::Pip);
    prepare_extension_with(&layout, &old, &extension, &Source::new(false)).unwrap();
    fs::write(layout.downloads().join("latest"), "2.0.0").unwrap();
    let journal = fs::read(layout.state().join("active-version")).unwrap();
    assert!(
        manager
            .update_with(&Source::new(true))
            .unwrap_err()
            .contains("download failed")
    );
    assert_eq!(
        fs::read(layout.state().join("active-version")).unwrap(),
        journal
    );
    assert_eq!(
        SignedResources::load(&layout, &old)
            .unwrap()
            .read(&logical_path(&extension))
            .unwrap(),
        b"1.0.0"
    );
    let source = Source::new(false);
    let new = manager.update_with(&source).unwrap();
    assert_eq!(new.as_str(), "2.0.0");
    assert_eq!(source.requests.load(Ordering::Relaxed), 1);
    let resources = SignedResources::load(&layout, &new).unwrap();
    assert_eq!(resources.read(&logical_path(&extension)).unwrap(), b"2.0.0");
    assert!(
        resources
            .read(&logical_path(&id(Ecosystem::Npm)))
            .unwrap_err()
            .contains("not installed")
    );
    assert_eq!(manager.active_version().unwrap(), Some(new));
}

#[test]
fn optional_artifact_ipc_distinguishes_absence_corruption_and_service_failure() {
    let (_root, layout, _manager, _key) = machine();
    let mut daemon = TestDaemon::start(&layout);
    let digest = hex(&Sha256::digest(b"payload"));
    assert!(
        artifact_get_optional(&layout, &digest, 7)
            .unwrap()
            .is_none()
    );
    artifact_put(&layout, b"payload").unwrap();
    assert_eq!(
        artifact_get_optional(&layout, &digest, 7).unwrap().unwrap(),
        b"payload"
    );
    fs::write(
        layout.native_cache().join("artifacts").join(&digest),
        b"changed",
    )
    .unwrap();
    assert!(
        artifact_get_optional(&layout, &digest, 7)
            .unwrap_err()
            .contains("SHA-256")
    );
    daemon.stop();
    assert!(artifact_get_optional(&layout, &digest, 7).is_err());
}

#[test]
fn extension_ipc_rejects_caller_paths_and_invalid_typed_identity() {
    let (_root, layout, _manager, _key) = machine();
    let _daemon = TestDaemon::start(&layout);
    let token = fs::read_to_string(layout.service_token()).unwrap();
    for selection in [
        serde_json::json!({"version":"1.0.0","extension":{"kind":{"type":"runtime","ecosystem":"pip"},"target":"linux-x86_64"},"url":"http://localhost/"}),
        serde_json::json!({"version":"../../outside","extension":{"kind":{"type":"target"},"target":"linux-aarch64"}}),
        serde_json::json!({"version":"1.0.0","extension":{"kind":{"type":"shell"},"target":"linux-x86_64"}}),
    ] {
        let result = raw_operation(
            &layout,
            token.trim(),
            serde_json::json!({"ensure_extension":selection}),
        );
        assert_eq!(result["ok"], false, "{result}");
    }
}
