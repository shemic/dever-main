//! Bootstrap package validation and recovery use only owned fixture directories.
use super::*;
use dever_cli::toolchain::bootstrap;
use sha2::{Digest, Sha256};

#[test]
fn bootstrap_service_recovery_orders_actions_around_file_restoration() {
    use bootstrap::RecoveryAction::{Reload, Restart, Stop};
    for active in [false, true] {
        for loaded in [false, true] {
            assert_eq!(
                bootstrap::recovery_actions(false, active, loaded),
                (vec![], vec![])
            );
        }
    }
    assert_eq!(
        bootstrap::recovery_actions(true, false, false),
        (vec![], vec![Reload])
    );
    assert_eq!(
        bootstrap::recovery_actions(true, false, true),
        (vec![Stop], vec![Reload])
    );
    assert_eq!(
        bootstrap::recovery_actions(true, true, true),
        (vec![Stop], vec![Reload, Restart])
    );
    assert_eq!(
        bootstrap::recovery_actions(true, true, false),
        (vec![], vec![Reload, Restart])
    );
}

fn add_bootstrap(author: &mut Author) {
    let input = |name: &str, bytes: &[u8]| {
        let relative = format!("inputs/{name}");
        fs::write(author.root.join(&relative), bytes).unwrap();
        json!({"source":relative,"sha256":sha256_file(&author.root.join(&relative)).unwrap()})
    };
    let launcher = input(
        "dever",
        &native_elf::compiler(&["libgcc_s.so.1", "libc.so.6"]),
    );
    let daemon = input(
        "deverd",
        &native_elf::compiler(&["libgcc_s.so.1", "libc.so.6"]),
    );
    let mut gcc = input("gcc", &native_elf::library("libgcc_s.so.1", &["libc.so.6"]));
    gcc["path"] = json!("libgcc_s.so.1");
    author.settings["bootstrap"] = json!({"launcher":launcher,"daemon":daemon,"libraries":[gcc]});
}

#[test]
fn bootstrap_payload_has_a_separate_signed_complete_loader_closure() {
    let mut author = Author::new();
    add_bootstrap(&mut author);
    author.save();
    let release = author.output("complete");
    let manifest = packaging::create(&author.root, &release).unwrap();
    assert_signed(&author, &release);
    for path in [
        "bootstrap/dever",
        "bootstrap/deverd",
        "bootstrap/lib/libgcc_s.so.1",
        "bootstrap/deverd.service",
        "bootstrap/dever-sandbox",
    ] {
        let artifact = manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.path == path)
            .unwrap();
        assert_eq!(artifact.sha256, sha256_file(&release.join(path)).unwrap());
    }
    let unit = fs::read_to_string(release.join("bootstrap/deverd.service")).unwrap();
    assert!(unit.contains("ExecStart=/opt/dever/bin/deverd --root /opt/dever"));
    assert!(unit.contains("RuntimeDirectory=dever"));
    assert!(!unit.contains("Environment"));
    assert!(!unit.contains("PATH"));
    assert_eq!(
        fs::read_to_string(release.join("bootstrap/dever-sandbox")).unwrap(),
        dever_sandbox::APPARMOR_PROFILE
    );
}

#[test]
fn bootstrap_authoring_rejects_missing_libraries_wrong_sonames_and_search_paths() {
    let mut author = Author::new();
    add_bootstrap(&mut author);
    author.settings["bootstrap"]["libraries"] = json!([]);
    assert!(author.reject().contains("libgcc_s.so.1"));

    let mut author = Author::new();
    add_bootstrap(&mut author);
    fs::write(
        author.root.join("inputs/gcc"),
        native_elf::library("libwrong.so", &["libc.so.6"]),
    )
    .unwrap();
    author.settings["bootstrap"]["libraries"][0]["sha256"] =
        json!(sha256_file(&author.root.join("inputs/gcc")).unwrap());
    assert!(author.reject().contains("SONAME"));

    let mut author = Author::new();
    add_bootstrap(&mut author);
    fs::write(
        author.root.join("inputs/dever"),
        native_elf::elf(None, &["libgcc_s.so.1"], Some("/usr/lib")),
    )
    .unwrap();
    author.settings["bootstrap"]["launcher"]["sha256"] =
        json!(sha256_file(&author.root.join("inputs/dever")).unwrap());
    assert!(author.reject().contains("RPATH"));
}

#[test]
fn signed_installed_versions_keep_their_original_bootstrap_service_template() {
    use dever_cli::toolchain::{Layout, MachineManager, Version};
    let mut author = Author::new();
    add_bootstrap(&mut author);
    author.save();
    let machine = TemporaryDirectory::new();
    let layout = Layout::new(machine.path().join("machine"));
    layout.initialize().unwrap();
    let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let installed = layout.versions().join(version.as_str());
    let mut manifest = packaging::create(&author.root, &installed).unwrap();
    let service = installed.join("bootstrap/deverd.service");
    let current = fs::read_to_string(&service).unwrap();
    let old = current.replace("RestartSec=1", "RestartSec=2");
    assert_ne!(current, old);
    fs::write(&service, old).unwrap();
    let artifact = manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.path == "bootstrap/deverd.service")
        .unwrap();
    artifact.bytes = fs::metadata(&service).unwrap().len();
    artifact.sha256 = sha256_file(&service).unwrap();
    let encoded = serde_json::to_vec(&manifest).unwrap();
    let key =
        Ed25519KeyPair::from_pkcs8(&fs::read(author.root.join("signing.pk8")).unwrap()).unwrap();
    fs::write(installed.join("manifest.json"), &encoded).unwrap();
    fs::write(
        installed.join("manifest.sig"),
        key.sign(&encoded)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    fs::write(
        layout.state().join("trusted-release-key"),
        author
            .public_key
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    let manager = MachineManager::new(layout);
    assert_eq!(
        manager.resolve_core(&version).unwrap(),
        installed.join("dever-core")
    );
    fs::write(service, b"unsigned modification").unwrap();
    assert!(
        manager
            .resolve_core(&version)
            .unwrap_err()
            .contains("integrity verification")
    );
}

#[test]
#[ignore = "root-only installer trust rejection; never touches the actual machine installation"]
fn bootstrap_installer_rejects_untrusted_inputs_before_publication() {
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    assert!(
        bootstrap::install(Path::new("."))
            .unwrap_err()
            .contains("absolute paths")
    );
    assert!(
        bootstrap::recover(Path::new("."))
            .unwrap_err()
            .contains("absolute paths")
    );
    let mut author = Author::new();
    add_bootstrap(&mut author);
    author.save();
    let package = author.output("release");
    packaging::create(&author.root, &package).unwrap();
    let installer = TemporaryDirectory::new();
    let system = TemporaryDirectory::new();
    let key = installer.path().join("trusted.pub");
    fs::write(
        &key,
        author
            .public_key
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    fs::create_dir(installer.path().join("config")).unwrap();
    let save = |key: &Path, activate: bool| {
        fs::write(installer.path().join("config/setting.json"), serde_json::to_vec(&json!({"release":package,"system_root":system.path(),"trusted_key":key,"activate_service":activate})).unwrap()).unwrap()
    };
    save(&key, true);
    assert!(
        bootstrap::install(installer.path())
            .unwrap_err()
            .contains("actual system root")
    );
    let inside = package.join("trusted.pub");
    fs::copy(&key, &inside).unwrap();
    save(&inside, false);
    assert!(
        bootstrap::install(installer.path())
            .unwrap_err()
            .contains("independently")
    );
    save(&key, false);
    fs::set_permissions(&key, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        bootstrap::install(installer.path())
            .unwrap_err()
            .contains("root-owned")
    );
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    let signature = fs::read(package.join("manifest.sig")).unwrap();
    fs::write(package.join("manifest.sig"), "00".repeat(64)).unwrap();
    assert!(
        bootstrap::install(installer.path())
            .unwrap_err()
            .contains("signature verification")
    );
    fs::write(package.join("manifest.sig"), signature).unwrap();
    // The key is pinned before the intentionally non-executable synthetic core
    // fails its health check. Empty/partial temporary files model interrupted
    // writes without injecting product failpoints or touching real services.
    let state = system.path().join("opt/dever/state");
    let temporary_key = state.join(".trusted-release-key.next");
    let pinned_key = state.join("trusted-release-key");
    for partial in ["", "abc"] {
        fs::write(&temporary_key, partial).unwrap();
        let error = bootstrap::install(installer.path()).unwrap_err();
        assert!(!error.contains("temporary release key"), "{error}");
        assert_eq!(
            fs::read_to_string(&pinned_key).unwrap().trim(),
            fs::read_to_string(&key).unwrap()
        );
        assert!(!temporary_key.exists());
        fs::remove_file(&pinned_key).unwrap();
    }
    fs::write(&pinned_key, "11".repeat(32)).unwrap();
    assert!(
        bootstrap::install(installer.path())
            .unwrap_err()
            .contains("pinned release key")
    );
    assert_eq!(fs::read_to_string(&pinned_key).unwrap(), "11".repeat(32));
    fs::remove_file(&pinned_key).unwrap();
    fs::write(package.join("bootstrap/lib/libgcc_s.so.1"), b"tampered").unwrap();
    assert!(
        bootstrap::install(installer.path())
            .unwrap_err()
            .contains("integrity verification")
    );
    fs::remove_file(package.join("bootstrap/lib/libgcc_s.so.1")).unwrap();
    assert!(bootstrap::install(installer.path()).is_err());
    assert!(!system.path().join("usr/local/bin/dever").exists());
    assert!(
        !system
            .path()
            .join("opt/dever/state/active-version")
            .exists()
    );
}

// Independent construction of a one-node journal identity lets recovery tests
// simulate interruption at publication boundaries without product failpoints.
fn node_hash(kind: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(0u64.to_le_bytes());
    hash.update(
        if kind == "directory" {
            0o755u32
        } else {
            0o644u32
        }
        .to_le_bytes(),
    );
    hash.update(kind.as_bytes());
    if kind == "file" {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

#[test]
#[ignore = "root-only crash-recovery fixture; all journal targets are below an owned image root"]
fn bootstrap_recovery_is_idempotent_and_rejects_unrecorded_targets() {
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let system = TemporaryDirectory::new();
    let root = system.path();
    let layout = dever_cli::toolchain::Layout::new(root.join("opt/dever"));
    layout.initialize().unwrap();
    for directory in [
        "usr/local/bin",
        "etc/systemd/system/multi-user.target.wants",
        "etc/apparmor.d",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    let service = root.join("etc/systemd/system/deverd.service");
    fs::write(&service, b"new service").unwrap();
    let entry = root.join("usr/local/bin/dever");
    symlink("/opt/dever/bin/dever", &entry).unwrap();
    let enable_stage = root
        .join("etc/systemd/system/multi-user.target.wants/.dever-bootstrap-deverd.service.next");
    symlink("../deverd.service", &enable_stage).unwrap();
    let active_stage = layout.state().join(".dever-bootstrap-active-version.next");
    fs::write(&active_stage, b"new active").unwrap();
    let sentinel = root.join("etc/systemd/system/unrelated.service");
    fs::write(&sentinel, b"keep").unwrap();
    let link_hash =
        |target: &str| format!("{:x}", Sha256::digest(format!("link\0{target}").as_bytes()));
    let journal = json!({"format":"dever-bootstrap-transaction-v1","phase":"prepared","service_was_active":null,"service_start_requested":false,"changes":[
        {"target":"sandbox","before":null,"after":node_hash("directory",b"")},
        {"target":"apparmor","before":null,"after":node_hash("file",b"new profile")},
        {"target":"bin","before":node_hash("directory",b""),"after":node_hash("directory",b"")},
        {"target":"service","before":null,"after":node_hash("file",b"new service")},
        {"target":"entry","before":null,"after":link_hash("/opt/dever/bin/dever")},
        {"target":"enable","before":null,"after":link_hash("../deverd.service")},
        {"target":"active","before":null,"after":node_hash("file",b"new active")}
    ]});
    fs::create_dir(root.join("opt/dever/.dever-bootstrap-sandbox.next")).unwrap();
    fs::write(
        root.join("etc/apparmor.d/.dever-bootstrap-dever-sandbox.next"),
        b"new profile",
    )
    .unwrap();
    let path = layout.state().join("bootstrap-install.json");
    fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    let mut invalid = journal.clone();
    invalid["changes"][0]["target"] = json!("../../unrelated");
    fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(bootstrap::recover(root).is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    fs::write(&service, b"not this transaction").unwrap();
    assert!(
        bootstrap::recover(root)
            .unwrap_err()
            .contains("outside this transaction")
    );
    assert_eq!(fs::read(&service).unwrap(), b"not this transaction");
    fs::write(&service, b"new service").unwrap();
    bootstrap::recover(root).unwrap();
    bootstrap::recover(root).unwrap();
    assert!(!service.exists());
    assert!(fs::symlink_metadata(&entry).is_err());
    assert!(!active_stage.exists());
    assert!(fs::symlink_metadata(&enable_stage).is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    assert!(!path.exists());

    // A committed publication cleans only its old staging copies; it must not
    // undo the new active version, including after a failed cleanup retry.
    fs::write(&service, b"new service").unwrap();
    symlink("/opt/dever/bin/dever", &entry).unwrap();
    fs::create_dir(root.join("opt/dever/sandbox")).unwrap();
    fs::write(root.join("etc/apparmor.d/dever-sandbox"), b"new profile").unwrap();
    symlink(
        "../deverd.service",
        root.join("etc/systemd/system/multi-user.target.wants/deverd.service"),
    )
    .unwrap();
    fs::write(layout.state().join("active-version"), b"new active").unwrap();
    fs::write(&active_stage, b"old active").unwrap();
    let mut committed = journal;
    committed["phase"] = json!("committed");
    fs::write(&path, serde_json::to_vec(&committed).unwrap()).unwrap();
    fs::write(&service, b"unexpected modification").unwrap();
    assert!(
        bootstrap::recover(root)
            .unwrap_err()
            .contains("committed bootstrap destination changed")
    );
    assert!(path.exists());
    assert_eq!(
        fs::read(layout.state().join("active-version")).unwrap(),
        b"new active"
    );
    fs::write(&service, b"new service").unwrap();
    bootstrap::recover(root).unwrap();
    bootstrap::recover(root).unwrap();
    assert_eq!(
        fs::read(layout.state().join("active-version")).unwrap(),
        b"new active"
    );
    assert!(!active_stage.exists());
    assert!(!path.exists());
    assert_eq!(fs::read(sentinel).unwrap(), b"keep");
}
