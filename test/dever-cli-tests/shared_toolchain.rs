use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};

use dever_cli::toolchain::{
    Artifact, BuildIdentity, CacheStore, CommandResult, Layout, MachineManager, PlatformContract,
    ReleaseManifest, Version, artifact_get, artifact_put, execute_in, platform_contract,
};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use sha2::{Digest, Sha256};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
#[path = "shared_toolchain_extensions.rs"]
mod extensions;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "dever-toolchain-test-{}-{}",
                std::process::id(),
                NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
struct TestDaemon(Child);

#[cfg(unix)]
impl TestDaemon {
    fn start(layout: &Layout) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_deverd"))
            .env_clear()
            .args(["--root", layout.root().to_str().unwrap()])
            .spawn()
            .unwrap();
        let mut daemon = Self(child);
        for _ in 0..100 {
            if dever_cli::toolchain::cache_status(layout).is_ok() {
                return daemon;
            }
            assert!(
                daemon.0.try_wait().unwrap().is_none(),
                "deverd exited before becoming ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("deverd did not publish its endpoint");
    }

    fn stop(&mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

#[cfg(unix)]
impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
fn raw_request(layout: &Layout, token: &str, operation: &str) -> serde_json::Value {
    raw_operation(layout, token, serde_json::Value::String(operation.into()))
}

#[cfg(unix)]
fn raw_operation(layout: &Layout, token: &str, operation: serde_json::Value) -> serde_json::Value {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(layout.service_socket()).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let payload = serde_json::to_vec(&serde_json::json!({
        "token": token,
        "operation": operation,
        "installed": [],
    }))
    .unwrap();
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&payload).unwrap();
    read_response(&mut stream)
}

#[cfg(unix)]
#[test]
fn compilation_control_rejects_client_ir_paths_and_oversized_bodies() {
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let _daemon = TestDaemon::start(&layout);
    let token = fs::read_to_string(layout.service_token()).unwrap();
    for extra in ["ir", "output", "cache_key", "compiler"] {
        let mut operation = serde_json::json!({"compile":{"bytes":1}});
        operation["compile"][extra] = serde_json::json!("caller-controlled");
        let response = raw_operation(&layout, token.trim(), operation);
        assert_eq!(response["ok"], false);
        assert!(
            response["error"]
                .as_str()
                .unwrap()
                .contains("unknown field"),
            "{response}"
        );
    }
    for bytes in [0, 384 * 1024 * 1024 + 1, u64::MAX] {
        let response = raw_operation(
            &layout,
            token.trim(),
            serde_json::json!({"compile":{"bytes":bytes}}),
        );
        assert_eq!(response["ok"], false);
        assert!(
            response["error"]
                .as_str()
                .unwrap()
                .contains("transfer limit")
        );
    }
    assert_eq!(
        dever_cli::toolchain::cache_status(&layout).unwrap().entries,
        0
    );
}

#[cfg(unix)]
fn read_response(stream: &mut std::os::unix::net::UnixStream) -> serde_json::Value {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).unwrap();
    let mut payload = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut payload).unwrap();
    serde_json::from_slice(&payload).unwrap()
}

#[test]
fn runtime_accepts_only_an_exact_project_toolchain_version() {
    let temporary = TemporaryDirectory::new();
    let config = temporary.path().join("config");
    fs::create_dir(&config).unwrap();
    fs::write(
        config.join("setting.json"),
        r#"{"dever":{"version":"1.2.3"}}"#,
    )
    .unwrap();
    let settings = dever_runtime::config::Settings::load_project(temporary.path()).unwrap();
    assert_eq!(settings.dever_version(), Some("1.2.3"));

    for invalid in [
        r#"{"dever":{"version":"latest"}}"#,
        r#"{"dever":{"version":"01.2.3"}}"#,
        r#"{"dever":{"version":"1.2"}}"#,
        r#"{"dever":{"version":"1.2.3","channel":"stable"}}"#,
        r#"{"dever":{"version":"1.2.3","version":"2.0.0"}}"#,
    ] {
        fs::write(config.join("setting.json"), invalid).unwrap();
        assert!(
            dever_runtime::config::Settings::load_project(temporary.path()).is_err(),
            "accepted {invalid}"
        );
    }
    assert!(Version::parse("1.10.0").unwrap() > Version::parse("1.2.0").unwrap());
}

#[test]
fn all_supported_platforms_have_one_machine_entry_and_service_contract() {
    for (os, expected) in [
        ("linux", ("/opt/dever", "/usr/local/bin/dever")),
        (
            "macos",
            ("/Library/Application Support/Dever", "/usr/local/bin/dever"),
        ),
        (
            "windows",
            (
                r"C:\Program Files\Dever",
                r"C:\Program Files\Dever\bin\dever.exe",
            ),
        ),
    ] {
        let PlatformContract {
            machine_root,
            public_entry,
            service_definition,
            ipc_endpoint,
        } = platform_contract(os).expect("supported platform contract");
        assert_eq!((machine_root, public_entry), expected);
        assert!(!service_definition.is_empty());
        assert!(!ipc_endpoint.is_empty());
    }
    assert!(platform_contract("freebsd").is_none());
}

#[cfg(unix)]
#[test]
fn signed_versions_install_select_dispatch_update_and_rollback_atomically() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let key = signing_key();
    fs::write(
        layout.state().join("trusted-release-key"),
        format!("{}\n", hex(key.public_key().as_ref())),
    )
    .unwrap();
    let dispatch_log = temporary.path().join("dispatch.log");
    for (version, healthy, valid_signature) in [
        ("1.0.0", true, true),
        ("2.0.0", true, true),
        ("3.0.0", false, true),
        ("4.0.0", true, false),
    ] {
        write_release(
            &layout,
            &key,
            version,
            &dispatch_log,
            healthy,
            valid_signature,
        );
    }
    fs::write(layout.downloads().join("latest"), "2.0.0\n").unwrap();
    let manager = MachineManager::new(layout.clone());

    fs::write(
        layout.state().join("active-version"),
        "dever-active-version-v1\n\npartial",
    )
    .unwrap();
    assert_eq!(manager.active_version().unwrap(), None);
    assert_eq!(
        manager.install_requested("1.0.0").unwrap().as_str(),
        "1.0.0"
    );
    assert_eq!(
        fs::metadata(layout.versions().join("1.0.0/dever-core"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(layout.versions().join("1.0.0/runtime.pack"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(manager.active_version().unwrap().unwrap().as_str(), "1.0.0");
    fs::OpenOptions::new()
        .append(true)
        .open(layout.state().join("active-version"))
        .unwrap()
        .write_all(b"\npartial-record")
        .unwrap();
    assert_eq!(manager.active_version().unwrap().unwrap().as_str(), "1.0.0");
    manager.install_requested("2.0.0").unwrap();
    assert_eq!(manager.active_version().unwrap().unwrap().as_str(), "1.0.0");

    let skill_path = |layout: &Layout| {
        let CommandResult::Message(path) =
            execute_in(layout, &["skill".into(), "path".into()]).unwrap()
        else {
            panic!("skill path must return the active signed directory")
        };
        PathBuf::from(path)
    };
    assert_eq!(
        fs::read_to_string(skill_path(&layout).join("SKILL.md")).unwrap(),
        "skill 1.0.0\n"
    );
    let skill_entry = temporary.path().join("dever-language");
    execute_in(
        &layout,
        &[
            "skill".into(),
            "install".into(),
            skill_entry.as_os_str().into(),
        ],
    )
    .unwrap();
    let loader = fs::read(skill_entry.join("SKILL.md")).unwrap();
    assert!(String::from_utf8_lossy(&loader).contains("skill path"));
    assert!(
        execute_in(
            &layout,
            &[
                "skill".into(),
                "install".into(),
                skill_entry.as_os_str().into()
            ]
        )
        .is_err()
    );
    assert_eq!(fs::read(skill_entry.join("SKILL.md")).unwrap(), loader);

    let new_project = temporary.path().join("not-created-yet");
    let CommandResult::Exited(code) = execute_in(
        &layout,
        &[
            "new".into(),
            new_project.as_os_str().into(),
            "--markdown".into(),
        ],
    )
    .unwrap() else {
        panic!("new must dispatch without project settings")
    };
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(&dispatch_log).unwrap(),
        format!("new\n{}\n--markdown\n", new_project.display())
    );

    let project = temporary.path().join("project");
    fs::create_dir_all(project.join("config")).unwrap();
    fs::write(
        project.join("config/setting.json"),
        r#"{"dever":{"version":"2.0.0"}}"#,
    )
    .unwrap();
    let arguments = ["check", project.to_str().unwrap()].map(OsString::from);
    let CommandResult::Message(pinned_skill) = execute_in(
        &layout,
        &["skill".into(), "path".into(), project.as_os_str().into()],
    )
    .unwrap() else {
        panic!("project skill must match its selected compiler")
    };
    assert_eq!(
        Path::new(&pinned_skill),
        layout.versions().join("2.0.0/skills/dever-language")
    );
    let CommandResult::Exited(code) = execute_in(&layout, &arguments).unwrap() else {
        panic!("project command must dispatch")
    };
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(&dispatch_log).unwrap(),
        format!("check\n{}\n", project.display())
    );

    let clean_arguments = ["clean", project.to_str().unwrap()].map(OsString::from);
    let CommandResult::Exited(code) = execute_in(&layout, &clean_arguments).unwrap() else {
        panic!("clean command must use the selected project core")
    };
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(&dispatch_log).unwrap(),
        format!("clean\n{}\n", project.display())
    );

    assert_eq!(manager.update().unwrap().as_str(), "2.0.0");
    assert_eq!(manager.active_version().unwrap().unwrap().as_str(), "2.0.0");
    assert_eq!(
        fs::read_to_string(skill_path(&layout).join("SKILL.md")).unwrap(),
        "skill 2.0.0\n"
    );
    assert_eq!(fs::read(skill_entry.join("SKILL.md")).unwrap(), loader);
    fs::write(layout.downloads().join("latest"), "1.0.0\n").unwrap();
    assert!(
        manager
            .update()
            .unwrap_err()
            .contains("refusing to downgrade")
    );
    assert_eq!(manager.active_version().unwrap().unwrap().as_str(), "2.0.0");
    fs::write(layout.downloads().join("latest"), "2.0.0\n").unwrap();
    assert!(
        manager
            .uninstall(&Version::parse("2.0.0").unwrap())
            .unwrap_err()
            .contains("active")
    );
    manager.activate(&Version::parse("1.0.0").unwrap()).unwrap();
    assert_eq!(
        fs::read_to_string(skill_path(&layout).join("SKILL.md")).unwrap(),
        "skill 1.0.0\n"
    );
    manager
        .uninstall(&Version::parse("2.0.0").unwrap())
        .unwrap();
    assert!(!layout.versions().join("2.0.0").exists());

    let before = manager.active_version().unwrap();
    assert!(
        manager
            .install_requested("3.0.0")
            .unwrap_err()
            .contains("health check")
    );
    assert_eq!(manager.active_version().unwrap(), before);
    assert!(!layout.versions().join("3.0.0").exists());
    assert!(
        manager
            .install_requested("4.0.0")
            .unwrap_err()
            .contains("signature")
    );
    assert_eq!(manager.active_version().unwrap(), before);
    write_release(&layout, &key, "5.0.0", &dispatch_log, true, true);
    fs::write(
        layout.downloads().join("5.0.0/runtime.pack"),
        "tampered runtime",
    )
    .unwrap();
    assert!(
        manager
            .install_requested("5.0.0")
            .unwrap_err()
            .contains("integrity")
    );
    assert_eq!(fs::read_dir(layout.staging()).unwrap().count(), 0);

    let install_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(layout.state().join("install.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&install_lock).unwrap();
    assert!(
        manager
            .install_requested("1.0.0")
            .unwrap_err()
            .contains("active")
    );
    fs2::FileExt::unlock(&install_lock).unwrap();
    drop(install_lock);

    fs::set_permissions(layout.state(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        project.join("config/setting.json"),
        r#"{"dever":{"version":"9.9.9"}}"#,
    )
    .unwrap();
    let error = match execute_in(&layout, &arguments) {
        Ok(_) => panic!("missing locked version must fail"),
        Err(error) => error,
    };
    assert!(error.contains("dever install 9.9.9"), "{error}");

    fs::set_permissions(
        layout.versions().join("1.0.0/dever-core"),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    let error = manager
        .resolve_core(&Version::parse("1.0.0").unwrap())
        .unwrap_err();
    assert!(
        error.contains("not writable by group or other users"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn installed_launcher_is_one_shared_entry_for_multiple_projects() {
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let key = signing_key();
    fs::write(
        layout.state().join("trusted-release-key"),
        format!("{}\n", hex(key.public_key().as_ref())),
    )
    .unwrap();
    let dispatch_log = temporary.path().join("dispatch.log");
    write_release(&layout, &key, "1.0.0", &dispatch_log, true, true);
    MachineManager::new(layout.clone())
        .install_requested("1.0.0")
        .unwrap();

    let launcher = layout.bin().join("dever");
    fs::copy(env!("CARGO_BIN_EXE_dever-launcher"), &launcher).unwrap();
    let version = Command::new(&launcher)
        .env_clear()
        .arg("version")
        .output()
        .unwrap();
    assert!(
        version.status.success(),
        "{}",
        String::from_utf8_lossy(&version.stderr)
    );
    assert_eq!(version.stdout, b"1.0.0\n");

    for name in ["first-project", "second-project"] {
        let project = temporary.path().join(name);
        fs::create_dir_all(project.join("config")).unwrap();
        fs::write(project.join("config/setting.json"), "{}\n").unwrap();
        for prefix in [vec!["check"], vec!["lib", "list"], vec!["package", "list"]] {
            let output = Command::new(&launcher)
                .env_clear()
                .env("DEVER_TEST_SENTINEL", "must-not-reach-version-core")
                .args(&prefix)
                .arg(&project)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                fs::read_to_string(&dispatch_log).unwrap(),
                format!("{}\n{}\n", prefix.join("\n"), project.display())
            );
        }
    }
    let project = temporary.path().join("exit-42");
    fs::create_dir_all(project.join("config")).unwrap();
    fs::write(project.join("config/setting.json"), "{}\n").unwrap();
    let status = Command::new(&launcher)
        .env_clear()
        .args(["check", project.to_str().unwrap()])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(42));
}

#[test]
fn shared_cache_uses_complete_crypto_identity_integrity_leases_and_safe_cleanup() {
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let version = Version::parse("1.0.0").unwrap();
    let program = temporary.path().join("program");
    fs::write(&program, b"verified executable bytes").unwrap();
    let build = identity(&version, b"generated one", "linux-x86_64");
    let service = CacheStore::new(&layout);

    let keys = std::thread::scope(|scope| {
        let first = scope.spawn(|| CacheStore::new(&layout).publish(&build, &program).unwrap());
        let second = scope.spawn(|| CacheStore::new(&layout).publish(&build, &program).unwrap());
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(keys[0], keys[1]);
    let restored = service.restore(&build).unwrap().expect("cache hit");
    assert_eq!(restored.as_bytes(), b"verified executable bytes");
    assert!(
        service
            .restore(&identity(&version, b"generated two", "linux-x86_64"))
            .unwrap()
            .is_none()
    );
    for target in ["linux-aarch64", "windows-x86_64"] {
        assert!(
            service
                .restore(&identity(&version, b"generated one", target))
                .unwrap()
                .is_none()
        );
    }

    let status = service.status().unwrap();
    assert_eq!((status.entries, status.corrupt_entries), (1, 0));
    assert_eq!(status.versions.get("1.0.0"), Some(&1));
    service.clean(&BTreeSet::new()).unwrap();
    assert_eq!(
        service.status().unwrap().entries,
        1,
        "leased entry was deleted"
    );
    drop(restored);
    fs::create_dir_all(layout.native_cache().join("operations")).unwrap();
    fs::write(
        layout.native_cache().join("operations/active"),
        "fixture operation",
    )
    .unwrap();
    assert!(
        service
            .clean(&BTreeSet::new())
            .unwrap_err()
            .contains("busy")
    );
    fs::remove_file(layout.native_cache().join("operations/active")).unwrap();
    service.clean(&BTreeSet::new()).unwrap();
    assert_eq!(service.status().unwrap().entries, 0);

    let key = service.publish(&build, &program).unwrap();
    fs::write(
        layout
            .native_cache()
            .join("entries")
            .join(&key)
            .join("program"),
        b"tampered",
    )
    .unwrap();
    let error = match service.restore(&build) {
        Ok(_) => panic!("modified cache entry must fail"),
        Err(error) => error,
    };
    assert!(error.contains("modified"), "{error}");
    assert_eq!(service.status().unwrap().corrupt_entries, 1);
    let installed = BTreeSet::from([version]);
    service.clean(&installed).unwrap();
    assert_eq!(service.status().unwrap().corrupt_entries, 0);
}

#[test]
fn launcher_does_not_bypass_an_unavailable_authenticated_cache_service() {
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    for arguments in [["cache", "status"], ["cache", "clean"]] {
        let arguments = arguments.map(OsString::from);
        let error = match execute_in(&layout, &arguments) {
            Ok(_) => panic!("cache command must not open the service store directly"),
            Err(error) => error,
        };
        assert!(error.contains("authenticated deverd"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn deverd_authenticates_cache_commands_over_local_ipc() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let _daemon = TestDaemon::start(&layout);
    let status = execute_in(
        &layout,
        &[OsString::from("cache"), OsString::from("status")],
    )
    .unwrap();
    let CommandResult::Message(status) = status else {
        panic!("cache status must return a message");
    };
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["entries"], 0);
    let version = Version::parse("1.0.0").unwrap();
    let artifact = temporary.path().join("program");
    fs::write(&artifact, b"authenticated cleanup fixture").unwrap();
    CacheStore::new(&layout)
        .publish(&identity(&version, b"auth", "linux-x86_64"), &artifact)
        .unwrap();
    let token = fs::read_to_string(layout.service_token()).unwrap();
    let padded = format!("{}{}", token.trim(), "\0".repeat(256));
    assert_eq!(raw_request(&layout, &padded, "clean")["ok"], false);
    assert_eq!(
        dever_cli::toolchain::cache_status(&layout).unwrap().entries,
        1
    );
    assert_eq!(raw_request(&layout, token.trim(), "status")["ok"], true);
    let moved = layout.state().join("deverd.token.saved");
    fs::rename(layout.service_token(), &moved).unwrap();
    assert_eq!(raw_request(&layout, "", "clean")["ok"], false);
    assert!(dever_cli::toolchain::cache_clean(&layout, &BTreeSet::new()).is_err());
    assert!(!layout.service_token().exists());
    fs::rename(moved, layout.service_token()).unwrap();
    fs::set_permissions(layout.service_token(), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(raw_request(&layout, token.trim(), "clean")["ok"], false);
    fs::set_permissions(layout.service_token(), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        dever_cli::toolchain::cache_status(&layout).unwrap().entries,
        1
    );
    fs::write(layout.service_token(), format!("{token}tampered")).unwrap();
    let error = execute_in(
        &layout,
        &[OsString::from("cache"), OsString::from("status")],
    )
    .unwrap_err();
    assert!(
        error.contains("authentication token") || error.contains("invalid"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn deverd_rejects_a_second_instance_and_recovers_its_socket_after_exit() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let mut daemon = TestDaemon::start(&layout);
    let socket_inode = fs::symlink_metadata(layout.service_socket()).unwrap().ino();
    let second = Command::new(env!("CARGO_BIN_EXE_deverd"))
        .args(["--root", layout.root().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"));
    assert_eq!(
        fs::symlink_metadata(layout.service_socket()).unwrap().ino(),
        socket_inode
    );
    assert!(dever_cli::toolchain::cache_status(&layout).is_ok());

    daemon.stop();
    assert!(layout.service_socket().exists());
    let lock = layout.state().join("deverd.lock");
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
    let insecure = Command::new(env!("CARGO_BIN_EXE_deverd"))
        .args(["--root", layout.root().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!insecure.status.success());
    assert!(String::from_utf8_lossy(&insecure.stderr).contains("deverd lock"));
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    let _recovered = TestDaemon::start(&layout);
    assert!(dever_cli::toolchain::cache_status(&layout).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn deverd_bounds_workers_when_clients_stall() {
    use std::os::unix::net::UnixStream;

    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let daemon = TestDaemon::start(&layout);
    let clients: Vec<_> = (0..48)
        .map(|_| {
            let mut stream = UnixStream::connect(layout.service_socket()).unwrap();
            let _ = stream.write_all(&[0]);
            stream
        })
        .collect();
    let status = fs::read_to_string(format!("/proc/{}/status", daemon.0.id())).unwrap();
    let threads: usize = status
        .lines()
        .find_map(|line| {
            line.strip_prefix("Threads:")
                .map(|value| value.trim().parse().unwrap())
        })
        .unwrap();
    assert!(
        threads <= 10,
        "deverd created {threads} threads for stalled clients"
    );
    drop(clients);
    let recovered = (0..100).any(|_| {
        if dever_cli::toolchain::cache_status(&layout).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        false
    });
    assert!(recovered);
}

#[cfg(unix)]
#[test]
fn deverd_keeps_serving_after_bad_and_slow_clients() {
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let _daemon = TestDaemon::start(&layout);

    let mut slow = UnixStream::connect(layout.service_socket()).unwrap();
    slow.write_all(&[0]).unwrap();
    let start = Instant::now();
    assert!(dever_cli::toolchain::cache_status(&layout).is_ok());
    assert!(start.elapsed() < Duration::from_secs(2));

    let mut oversized = UnixStream::connect(layout.service_socket()).unwrap();
    oversized
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    oversized.write_all(&u32::MAX.to_be_bytes()).unwrap();
    assert_eq!(read_response(&mut oversized)["ok"], false);

    let mut partial = UnixStream::connect(layout.service_socket()).unwrap();
    partial
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    partial.write_all(&100_u32.to_be_bytes()).unwrap();
    partial.shutdown(Shutdown::Write).unwrap();
    assert_eq!(read_response(&mut partial)["ok"], false);

    let mut disconnected = UnixStream::connect(layout.service_socket()).unwrap();
    disconnected.write_all(&0_u32.to_be_bytes()).unwrap();
    disconnected.shutdown(Shutdown::Both).unwrap();
    drop(slow);
    assert!(dever_cli::toolchain::cache_status(&layout).is_ok());
}

#[cfg(unix)]
#[test]
fn deverd_transfers_verified_artifacts_without_client_cache_paths() {
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let _daemon = TestDaemon::start(&layout);
    // Larger than a transport scratch buffer; the daemon streams to staging.
    let bytes = (0..262_147)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let receipt = artifact_put(&layout, &bytes).unwrap();
    assert_eq!(receipt.bytes, bytes.len() as u64);
    assert_eq!(
        artifact_get(&layout, &receipt.sha256, receipt.bytes).unwrap(),
        bytes
    );
    std::thread::scope(|scope| {
        for _ in 0..6 {
            scope.spawn(|| assert_eq!(artifact_put(&layout, &bytes).unwrap(), receipt));
        }
    });
    let status = dever_cli::toolchain::cache_status(&layout).unwrap();
    assert_eq!(status.artifact_entries, 1);
    assert_eq!(status.artifact_bytes, receipt.bytes);
    assert_eq!(status.staging_entries, 0);
    assert!(artifact_get(&layout, "../outside", 1).is_err());
    assert!(artifact_get(&layout, &receipt.sha256, receipt.bytes + 1).is_err());
    assert!(artifact_get(&layout, &receipt.sha256, 64 * 1024 * 1024 + 1).is_err());
    fs::write(
        layout
            .native_cache()
            .join("artifacts")
            .join(&receipt.sha256),
        b"tampered",
    )
    .unwrap();
    assert!(artifact_get(&layout, &receipt.sha256, receipt.bytes).is_err());
    assert_eq!(
        dever_cli::toolchain::cache_status(&layout)
            .unwrap()
            .corrupt_artifacts,
        1
    );
    dever_cli::toolchain::cache_clean(&layout, &BTreeSet::new()).unwrap();
    assert!(artifact_get(&layout, &receipt.sha256, receipt.bytes).is_err());
}

#[cfg(unix)]
#[test]
fn deverd_rejects_bad_and_incomplete_artifact_uploads_before_publication() {
    use std::os::unix::net::UnixStream;

    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let _daemon = TestDaemon::start(&layout);
    let token = fs::read_to_string(layout.service_token()).unwrap();
    for (bytes, digest, body, complete) in [
        (
            4_u64,
            hex(&Sha256::digest(b"good")),
            b"bad!".as_slice(),
            true,
        ),
        (4, hex(&Sha256::digest(b"good")), b"go".as_slice(), false),
    ] {
        let mut stream = UnixStream::connect(layout.service_socket()).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let request = serde_json::to_vec(&serde_json::json!({
            "token": token.trim(), "operation": {"artifact_put": {"sha256": digest, "bytes": bytes}}, "installed": []
        })).unwrap();
        stream
            .write_all(&(request.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&request).unwrap();
        assert_eq!(read_response(&mut stream)["ok"], true);
        stream.write_all(body).unwrap();
        if complete {
            assert_eq!(read_response(&mut stream)["ok"], false);
        }
        drop(stream);
        for attempt in 0..100 {
            let status = dever_cli::toolchain::cache_status(&layout).unwrap();
            if status.staging_entries == 0 {
                assert_eq!(status.artifact_entries, 0);
                break;
            }
            assert!(attempt < 99, "incomplete upload staging was not removed");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let receipt = artifact_put(&layout, b"good").unwrap();
    assert_eq!(
        artifact_get(&layout, &receipt.sha256, receipt.bytes).unwrap(),
        b"good"
    );
}

#[cfg(unix)]
#[test]
fn shared_artifact_quota_counts_bytes_and_entries_and_cleans_failed_staging() {
    for (entries, artifact_bytes) in [(32_u32, 64 * 1024 * 1024), (4096, 0)] {
        let temporary = TemporaryDirectory::new();
        let layout = Layout::new(temporary.path().join("machine"));
        layout.initialize().unwrap();
        let artifacts = layout.native_cache().join("artifacts");
        fs::create_dir(&artifacts).unwrap();
        // Sparse owned fixtures test byte and entry quotas without allocating 2 GiB.
        for index in 0_u32..entries {
            let file = fs::File::create(artifacts.join(hex(&Sha256::digest(index.to_le_bytes()))))
                .unwrap();
            file.set_len(artifact_bytes).unwrap();
        }
        let body = b"new verified artifact";
        let receipt = dever_cli::toolchain::ArtifactReceipt {
            sha256: hex(&Sha256::digest(body)),
            bytes: body.len() as u64,
        };
        let store = CacheStore::new(&layout);
        assert!(
            store
                .publish_artifact(&receipt, &mut body.as_slice())
                .unwrap_err()
                .contains("cache is full")
        );
        assert!(!artifacts.join(&receipt.sha256).exists());
        assert_eq!(
            fs::read_dir(layout.native_cache().join("staging"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(
            fs::read_dir(layout.native_cache().join("operations"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[cfg(unix)]
#[test]
fn shared_cache_summary_cannot_claim_payload_integrity() {
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let store = CacheStore::new(&layout);
    let bytes = b"good";
    let receipt = dever_cli::toolchain::ArtifactReceipt {
        sha256: hex(&Sha256::digest(bytes)),
        bytes: bytes.len() as u64,
    };
    store
        .publish_artifact(&receipt, &mut bytes.as_slice())
        .unwrap();
    fs::write(
        layout
            .native_cache()
            .join("artifacts")
            .join(&receipt.sha256),
        b"bad!",
    )
    .unwrap();
    let summary = store.summary().unwrap();
    assert!(!summary.verified);
    assert_eq!(summary.artifact_entries, 1);
    assert_eq!(summary.corrupt_artifacts, 0);
    let checked = store.status().unwrap();
    assert!(checked.verified);
    assert_eq!(checked.artifact_entries, 0);
    assert_eq!(checked.corrupt_artifacts, 1);
    assert!(store.artifact(&receipt).is_err());
}

#[test]
fn compiled_cache_has_independent_byte_entry_and_output_limits() {
    for (count, bytes) in [(32, 64 * 1024 * 1024), (4096, 1)] {
        let temporary = TemporaryDirectory::new();
        let layout = Layout::new(temporary.path().join("machine"));
        layout.initialize().unwrap();
        let version = Version::parse("0.1.0").unwrap();
        for index in 0..count {
            let key = format!("{index:064x}");
            let directory = layout.native_cache().join("entries").join(&key);
            fs::create_dir_all(&directory).unwrap();
            fs::File::create(directory.join("program"))
                .unwrap()
                .set_len(bytes)
                .unwrap();
            let entry = dever_cli::toolchain::CacheEntry {
                format: "dever-machine-cache-entry-v1".into(),
                key,
                version: version.clone(),
                target: "linux-x86_64".into(),
                bytes,
                sha256: "0".repeat(64),
            };
            fs::write(
                directory.join("entry.json"),
                serde_json::to_vec(&entry).unwrap(),
            )
            .unwrap();
        }
        let program = temporary.path().join("program");
        fs::write(&program, b"new compiled output").unwrap();
        let store = CacheStore::new(&layout);
        assert!(
            store
                .publish(&identity(&version, b"new", "linux-x86_64"), &program)
                .unwrap_err()
                .contains("quota exceeded")
        );
        fs::File::create(&program)
            .unwrap()
            .set_len(320 * 1024 * 1024 + 1)
            .unwrap();
        assert!(
            store
                .publish(&identity(&version, b"large", "linux-x86_64"), &program)
                .unwrap_err()
                .contains("320 MiB")
        );
        assert_eq!(
            fs::read_dir(layout.native_cache().join("staging"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(
            fs::read_dir(layout.native_cache().join("operations"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires root; drops only an owned skill client to an unprivileged uid"]
fn active_skill_is_readable_by_other_machine_users_without_install_lock_access() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let key = signing_key();
    fs::write(
        layout.state().join("trusted-release-key"),
        format!("{}\n", hex(key.public_key().as_ref())),
    )
    .unwrap();
    write_release(
        &layout,
        &key,
        "1.0.0",
        &temporary.path().join("dispatch"),
        true,
        true,
    );
    MachineManager::new(layout.clone())
        .install_requested("1.0.0")
        .unwrap();
    let launcher = layout.bin().join("dever");
    fs::copy(env!("CARGO_BIN_EXE_dever-launcher"), &launcher).unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(&launcher)
        .args(["skill", "path"])
        .uid(65534)
        .gid(65534)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        Path::new(path.trim()),
        layout.versions().join("1.0.0/skills/dever-language")
    );
    let read = Command::new("/bin/cat")
        .arg(Path::new(path.trim()).join("SKILL.md"))
        .uid(65534)
        .gid(65534)
        .env_clear()
        .output()
        .unwrap();
    assert!(read.status.success());
    assert_eq!(read.stdout, b"skill 1.0.0\n");
}

#[cfg(unix)]
#[test]
fn corrupt_skill_cannot_be_published_or_resolved() {
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let key = signing_key();
    fs::write(
        layout.state().join("trusted-release-key"),
        format!("{}\n", hex(key.public_key().as_ref())),
    )
    .unwrap();
    for version in ["1.0.0", "2.0.0"] {
        write_release(
            &layout,
            &key,
            version,
            &temporary.path().join("dispatch"),
            true,
            true,
        );
    }
    let manager = MachineManager::new(layout.clone());
    manager.install_requested("1.0.0").unwrap();
    fs::write(
        layout
            .downloads()
            .join("2.0.0/skills/dever-language/SKILL.md"),
        "tampered",
    )
    .unwrap();
    fs::write(layout.downloads().join("latest"), "2.0.0\n").unwrap();
    assert!(manager.update().unwrap_err().contains("integrity"));
    assert_eq!(manager.active_version().unwrap().unwrap().as_str(), "1.0.0");
    assert!(!layout.versions().join("2.0.0").exists());
    fs::write(
        layout
            .versions()
            .join("1.0.0/skills/dever-language/SKILL.md"),
        "tampered",
    )
    .unwrap();
    assert!(
        execute_in(&layout, &["skill".into(), "path".into()])
            .unwrap_err()
            .contains("integrity")
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires root; drops only owned test clients to an unprivileged uid"]
fn deverd_uses_kernel_credentials_for_other_machine_users() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    assert_eq!(
        rustix::process::geteuid().as_raw(),
        0,
        "this scoped acceptance requires root"
    );
    let temporary = TemporaryDirectory::new();
    let layout = Layout::new(temporary.path().join("machine"));
    layout.initialize().unwrap();
    let _daemon = TestDaemon::start(&layout);
    let launcher = layout.bin().join("dever");
    fs::copy(env!("CARGO_BIN_EXE_dever-launcher"), &launcher).unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
    let status = Command::new(&launcher)
        .args(["cache", "status"])
        .uid(65534)
        .gid(65534)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout).unwrap()["verified"],
        false
    );
    let clean = Command::new(&launcher)
        .args(["cache", "clean"])
        .uid(65534)
        .gid(65534)
        .env_clear()
        .output()
        .unwrap();
    assert!(!clean.status.success());
    assert!(String::from_utf8_lossy(&clean.stderr).contains("service owner"));
    let client = r#"
import hashlib, json, socket, struct, sys
endpoint, token_path = sys.argv[1:]
try:
    open(token_path, 'rb')
except PermissionError:
    pass
else:
    raise AssertionError('foreign user read the owner token')
def response(peer):
    def read(n):
        out = b''
        while len(out) < n:
            part = peer.recv(n - len(out))
            if not part: raise AssertionError('incomplete response')
            out += part
        return out
    length, = struct.unpack('>I', read(4))
    return json.loads(read(length))
held = []
try:
    for _ in range(2):
        peer = socket.socket(socket.AF_UNIX)
        peer.settimeout(2)
        peer.connect(endpoint)
        held.append(peer)
    with socket.socket(socket.AF_UNIX) as excess:
        excess.settimeout(2)
        excess.connect(endpoint)
        assert excess.recv(1) == b'', 'foreign UID exceeded its two-client admission'
    request = json.dumps(dict(operation='status')).encode()
    for peer in held:
        peer.sendall(struct.pack('>I', len(request)) + request)
        assert response(peer)['status']['verified'] is False
finally:
    for peer in held:
        peer.close()
body = b'shared across machine users'
identity = dict(sha256=hashlib.sha256(body).hexdigest(), bytes=len(body))
for operation in ('artifact_put', 'artifact_get'):
    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(5)
        peer.connect(endpoint)
        request = json.dumps(dict(operation={operation: identity})).encode()
        peer.sendall(struct.pack('>I', len(request)) + request)
        assert response(peer)['artifact'] == identity
        if operation == 'artifact_put':
            peer.sendall(body)
            assert response(peer)['artifact'] == identity
        else:
            read = b''
            while len(read) < len(body):
                part = peer.recv(len(body) - len(read))
                assert part
                read += part
            assert read == body
"#;
    let result = Command::new("/usr/bin/python3")
        .args(["-I", "-c", client])
        .arg(layout.service_socket())
        .arg(layout.service_token())
        .uid(65534)
        .gid(65534)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let status = dever_cli::toolchain::cache_status(&layout).unwrap();
    assert_eq!(status.artifact_entries, 1);
}

fn identity<'a>(version: &'a Version, generated: &'a [u8], target: &'a str) -> BuildIdentity<'a> {
    BuildIdentity {
        version,
        compiler: b"compiler identity",
        runtime: b"runtime identity",
        target,
        generated,
        options: b"release options",
        environment: b"backend environment",
    }
}

fn signing_key() -> Ed25519KeyPair {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap()
}

#[cfg(unix)]
fn write_release(
    layout: &Layout,
    key: &Ed25519KeyPair,
    version: &str,
    dispatch_log: &Path,
    healthy: bool,
    valid_signature: bool,
) {
    use std::os::unix::fs::PermissionsExt;

    let root = layout.downloads().join(version);
    fs::create_dir(&root).unwrap();
    let core = root.join("dever-core");
    let log = dispatch_log.display().to_string().replace('\'', "'\\''");
    fs::write(
        &core,
        format!(
            "#!/bin/sh\nif [ \"${{DEVER_TEST_SENTINEL+x}}\" = x ]; then exit 98; fi\nif [ \"$1\" = \"--dever-health\" ]; then exit {}; fi\nif [ \"${{2##*/}}\" = \"exit-42\" ]; then exit 42; fi\nprintf '%s\\n' \"$@\" > '{}'\n",
            if healthy { 0 } else { 9 },
            log,
        ),
    ).unwrap();
    fs::set_permissions(&core, fs::Permissions::from_mode(0o755)).unwrap();
    let runtime = root.join("runtime.pack");
    fs::write(&runtime, format!("runtime {version}")).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o666)).unwrap();
    fs::set_permissions(&core, fs::Permissions::from_mode(0o777)).unwrap();
    let skill = root.join("skills/dever-language/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, format!("skill {version}\n")).unwrap();
    let manifest = ReleaseManifest::new(
        Version::parse(version).unwrap(),
        vec![
            artifact(&core, "dever-core"),
            artifact(&runtime, "runtime.pack"),
            artifact(&skill, "skills/dever-language/SKILL.md"),
        ],
    );
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(root.join("manifest.json"), &bytes).unwrap();
    let mut signature = key.sign(&bytes).as_ref().to_vec();
    if !valid_signature {
        signature[0] ^= 1;
    }
    fs::write(root.join("manifest.sig"), format!("{}\n", hex(&signature))).unwrap();
}

fn artifact(path: &Path, relative: &str) -> Artifact {
    let bytes = fs::read(path).unwrap();
    Artifact {
        path: relative.into(),
        bytes: bytes.len() as u64,
        sha256: hex(&Sha256::digest(&bytes)),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}
