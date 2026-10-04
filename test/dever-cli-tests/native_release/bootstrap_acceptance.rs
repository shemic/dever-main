//! Real bootstrap installation and execution; no global service is changed.
use super::*;
use std::os::unix::fs::{MetadataExt, chown};
use std::os::unix::process::CommandExt;
use std::process::Child;

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_socket(daemon: &mut Daemon, socket: &Path) {
    for _ in 0..200 {
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "installed daemon exited before becoming ready"
        );
        if socket.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("installed daemon did not create its socket");
}

/// Clients share the daemon's PID namespace so SO_PEERCRED can identify both
/// peers. Separate bwrap invocations would hide the daemon PID from clients.
struct IsolatedDaemon {
    process: Daemon,
    namespace_pid: u32,
    namespace: std::os::fd::OwnedFd,
    root: PathBuf,
    stopped: bool,
}

impl IsolatedDaemon {
    fn start(root: &Path) -> Self {
        let information = TemporaryDirectory::new();
        let path = information.path().join("namespace.json");
        let output = fs::File::create(&path).unwrap();
        let mut process = Daemon(
            isolated_os_namespace(root)
                .args([
                    "--info-fd",
                    "1",
                    "--",
                    "/fixture-proc-init",
                    "/opt/dever/bin/deverd",
                    "--root",
                    "/opt/dever",
                ])
                .stdout(Stdio::from(output))
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        wait_socket(&mut process, &root.join("run/dever/deverd.sock"));
        assert!(fs::metadata(&path).unwrap().len() <= 4096);
        let info: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let namespace_pid = u32::try_from(info["child-pid"].as_u64().unwrap()).unwrap();
        assert_ne!(namespace_pid, 0);
        let namespace = rustix::process::pidfd_open(
            rustix::process::Pid::from_raw(i32::try_from(namespace_pid).unwrap()).unwrap(),
            rustix::process::PidfdFlags::empty(),
        )
        .unwrap();
        Self {
            process,
            namespace_pid,
            namespace,
            root: root.to_owned(),
            stopped: false,
        }
    }

    fn client(&mut self) -> Command {
        assert!(self.process.0.try_wait().unwrap().is_none());
        let target = fs::metadata(format!("/proc/{}/root", self.namespace_pid)).unwrap();
        let expected = fs::metadata(&self.root).unwrap();
        assert_eq!(
            (target.dev(), target.ino()),
            (expected.dev(), expected.ino())
        );
        let mut command = Command::new("/usr/bin/nsenter");
        command.env_clear().args([
            "--target",
            &self.namespace_pid.to_string(),
            "--all",
            "--root",
            "--wdns=/",
            "--",
        ]);
        command
    }

    fn stop(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        // Target the pinned namespace init, not a reusable numeric PID. Its
        // termination also kills its namespace descendants; waiting only for
        // the outer bwrap process does not establish daemon lock release.
        match rustix::process::pidfd_send_signal(&self.namespace, rustix::process::Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(error) => return Err(format!("cannot stop owned namespace: {error}")),
        }
        let expected = fs::metadata(&self.root).map_err(|error| error.to_string())?;
        let lock = fs::File::open(self.root.join("opt/dever/state/deverd.lock"))
            .map_err(|error| error.to_string())?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let wrapper_exited = self
                .process
                .0
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some();
            let namespace_exited = match fs::metadata(format!("/proc/{}/root", self.namespace_pid))
            {
                Ok(current) => (current.dev(), current.ino()) != (expected.dev(), expected.ino()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => return Err(format!("cannot inspect owned namespace exit: {error}")),
            };
            if wrapper_exited && namespace_exited {
                match fs2::FileExt::try_lock_exclusive(&lock) {
                    Ok(()) => {
                        self.stopped = true;
                        return Ok(());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        return Err(format!("cannot check owned daemon lock release: {error}"));
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err("owned namespace or daemon lock remained live after 10 seconds".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for IsolatedDaemon {
    fn drop(&mut self) {
        // Cleanup must not cause a second panic while unwinding a failed
        // assertion. Normal stage transitions explicitly check stop().
        if let Err(error) = self.stop() {
            eprintln!("bootstrap fixture cleanup failed: {error}");
        }
    }
}

fn add_inputs(author: &Path) {
    let launcher = input(
        author,
        Path::new(env!("CARGO_BIN_EXE_dever-launcher")),
        "bootstrap/dever",
    );
    let daemon = input(
        author,
        Path::new(env!("CARGO_BIN_EXE_deverd")),
        "bootstrap/deverd",
    );
    let mut gcc = input(
        author,
        Path::new("/lib/x86_64-linux-gnu/libgcc_s.so.1"),
        "bootstrap/libgcc_s.so.1",
    );
    gcc["path"] = json!("libgcc_s.so.1");
    let path = author.join("config/setting.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    settings["bootstrap"] = json!({"launcher":launcher,"daemon":daemon,"libraries":[gcc]});
    fs::write(path, serde_json::to_vec(&settings).unwrap()).unwrap();
}

#[test]
#[ignore = "requires optimized author archives and Linux root; installs only into an owned image root"]
fn signed_bootstrap_installs_and_serves_two_projects_without_host_libraries() {
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let author = TemporaryDirectory::new();
    let key = author_inputs(author.path(), &workspace);
    add_inputs(author.path());
    let downloaded = machine_temporary_directory(&workspace);
    let release = downloaded.path().join("release");
    let manifest = packaging::create(author.path(), &release).unwrap();
    let image = machine_temporary_directory(&workspace);
    ecosystems::minimal_os(image.path());
    let installer = TemporaryDirectory::new();
    let public = installer.path().join("release.pub");
    fs::write(
        &public,
        key.public_key()
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    write(installer.path(), "config/setting.json", serde_json::to_vec(&json!({
        "release":release,"system_root":image.path(),"trusted_key":public,"activate_service":false
    })).unwrap());
    // This author-built installer is the independently trusted first program;
    // no executable from an unchecked downloaded directory starts as root.
    command(
        Command::new(env!("CARGO_BIN_EXE_dever-launcher"))
            .arg("--dever-install")
            .arg(installer.path()),
    );
    let layout = Layout::new(image.path().join("opt/dever"));
    let sandbox_artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.path == "sandbox/linux-x86_64/bin/bwrap")
        .unwrap();
    let sandbox_store = layout.root().join("sandbox");
    let sandbox_helper = sandbox_store.join(&sandbox_artifact.sha256).join("bwrap");
    assert_eq!(
        sha256_file(&sandbox_helper).unwrap(),
        sandbox_artifact.sha256
    );
    assert_eq!(
        fs::metadata(&sandbox_helper).unwrap().mode() & 0o7777,
        0o755
    );
    assert_eq!(
        fs::read_to_string(image.path().join("etc/apparmor.d/dever-sandbox")).unwrap(),
        dever_sandbox::APPARMOR_PROFILE
    );
    let launcher = layout.bin().join("dever");
    let executable = layout.bin().join("deverd");
    assert_eq!(
        fs::read_link(image.path().join("usr/local/bin/dever")).unwrap(),
        Path::new("/opt/dever/bin/dever")
    );
    assert_eq!(
        fs::read_link(
            image
                .path()
                .join("etc/systemd/system/multi-user.target.wants/deverd.service")
        )
        .unwrap(),
        Path::new("../deverd.service")
    );
    let unit = fs::read_to_string(image.path().join("etc/systemd/system/deverd.service")).unwrap();
    assert!(unit.contains("ExecStart=/opt/dever/bin/deverd --root /opt/dever"));
    assert!(unit.contains("User=root"));
    for file in [
        &launcher,
        &executable,
        &layout.bin().join("lib/libgcc_s.so.1"),
        &layout.state().join("trusted-release-key"),
        &image.path().join("etc/systemd/system/deverd.service"),
    ] {
        let metadata = fs::metadata(file).unwrap();
        assert_eq!(metadata.uid(), 0);
        assert_eq!(metadata.mode() & 0o022, 0);
    }
    let active = fs::read(layout.state().join("active-version")).unwrap();
    let launcher_hash = sha256_file(&launcher).unwrap();
    // Model an older, correctly signed bootstrap service template. Its exact
    // signed bytes establish ownership; only the incoming payload must match
    // the current installer's supported service template.
    let unit_path = image.path().join("etc/systemd/system/deverd.service");
    let current_unit = fs::read_to_string(&unit_path).unwrap();
    let old_unit = current_unit.replace("RestartSec=1", "RestartSec=2");
    assert_ne!(old_unit, current_unit);
    fs::write(layout.bin().join("deverd.service"), &old_unit).unwrap();
    let mut old_manifest: Value =
        serde_json::from_slice(&fs::read(layout.bin().join("manifest.json")).unwrap()).unwrap();
    let service_artifact = old_manifest["artifacts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|artifact| artifact["path"] == "bootstrap/deverd.service")
        .unwrap();
    service_artifact["bytes"] = json!(old_unit.len());
    service_artifact["sha256"] = json!(sha256_file(&layout.bin().join("deverd.service")).unwrap());
    let encoded = serde_json::to_vec(&old_manifest).unwrap();
    fs::write(
        layout.bin().join("manifest.sig"),
        key.sign(&encoded)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    fs::write(layout.bin().join("manifest.json"), encoded).unwrap();
    fs::write(&unit_path, b"unrelated system unit").unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_dever-launcher"))
        .env_clear()
        .arg("--dever-install")
        .arg(installer.path())
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("unrelated system service"));
    assert_eq!(
        fs::read(layout.state().join("active-version")).unwrap(),
        active
    );
    fs::write(&unit_path, old_unit).unwrap();
    // Retain a different, correctly signed static helper identity through an
    // upgrade. ELF trailing padding keeps this executable's actual behavior.
    let mut previous_helper = fs::read(&sandbox_helper).unwrap();
    previous_helper.push(0);
    let previous_hash = dever_sandbox::validate_bwrap(&previous_helper).unwrap();
    let previous_root = sandbox_store.join(&previous_hash);
    fs::create_dir(&previous_root).unwrap();
    fs::write(previous_root.join("bwrap"), &previous_helper).unwrap();
    fs::set_permissions(
        previous_root.join("bwrap"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let mut previous_manifest = manifest.clone();
    let previous_artifact = previous_manifest
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.path == sandbox_artifact.path)
        .unwrap();
    previous_artifact.sha256 = previous_hash.clone();
    previous_artifact.bytes = previous_helper.len() as u64;
    let previous_encoded = serde_json::to_vec(&previous_manifest).unwrap();
    fs::write(
        previous_root.join("manifest.sig"),
        key.sign(&previous_encoded)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    fs::write(previous_root.join("manifest.json"), previous_encoded).unwrap();
    // Retry is supported, but a later damaged package cannot damage the
    // published launcher or compiler selection.
    command(
        Command::new(env!("CARGO_BIN_EXE_dever-launcher"))
            .arg("--dever-install")
            .arg(installer.path()),
    );
    assert_eq!(fs::read_to_string(&unit_path).unwrap(), current_unit);
    assert_eq!(
        sha256_file(&sandbox_helper).unwrap(),
        sandbox_artifact.sha256
    );
    assert_eq!(
        sha256_file(&previous_root.join("bwrap")).unwrap(),
        previous_hash
    );
    assert_eq!(fs::read_dir(&sandbox_store).unwrap().count(), 2);
    let before_bad_install = fs::read(layout.state().join("active-version")).unwrap();
    fs::write(
        release.join("bootstrap/lib/libgcc_s.so.1"),
        b"changed after signing",
    )
    .unwrap();
    let failed = Command::new(env!("CARGO_BIN_EXE_dever-launcher"))
        .env_clear()
        .arg("--dever-install")
        .arg(installer.path())
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(
        fs::read(layout.state().join("active-version")).unwrap(),
        before_bad_install
    );
    assert_eq!(sha256_file(&launcher).unwrap(), launcher_hash);
    assert!(!active.is_empty());
    drop(downloaded);
    drop(author);
    drop(installer);
    for name in ["first", "second"] {
        application(&image.path().join(name), "base");
    }
    let mut daemon = IsolatedDaemon::start(image.path());
    for name in ["first", "second"] {
        command(
            daemon
                .client()
                .args(["/usr/local/bin/dever", "check", &format!("/{name}")]),
        );
        assert_response(command(daemon.client().args([
            "/usr/local/bin/dever",
            "run",
            &format!("/{name}"),
            "--",
            "sample.value.exercise",
            "{}",
        ])));
        command(daemon.client().args([
            "/usr/local/bin/dever",
            "build",
            &format!("/{name}"),
            "--output",
            &format!("/{name}/program"),
        ]));
    }
    let status = command(
        daemon
            .client()
            .args(["/usr/local/bin/dever", "cache", "status"]),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap()["entries"],
        1
    );
    daemon
        .stop()
        .expect("isolated daemon and namespace fully stopped");
    drop(daemon);

    // Namespace-root execution above proves the complete loader closure. These
    // clients retain real distinct kernel UIDs, without a single-UID userns
    // translating file ownership to overflow IDs.
    fs::set_permissions(image.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let mut daemon = Daemon(
        Command::new(&executable)
            .env_clear()
            .args(["--root", layout.root().to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    wait_socket(&mut daemon, &layout.service_socket());
    for (name, uid) in [("first", 65533), ("second", 65534)] {
        let project = image.path().join(name);
        chown(&project, Some(uid), Some(uid)).unwrap();
        assert_response(command(
            Command::new(&launcher)
                .uid(uid)
                .gid(uid)
                .arg("run")
                .arg(&project)
                .args(["--", "sample.value.exercise", "{}"]),
        ));
        let denied = Command::new(&launcher)
            .uid(uid)
            .gid(uid)
            .env_clear()
            .args(["use", manifest.version.as_str()])
            .output()
            .unwrap();
        assert!(!denied.status.success());
        assert_eq!(
            fs::read(layout.state().join("active-version")).unwrap(),
            before_bad_install
        );
    }
    assert_eq!(cache_status(&layout).unwrap().entries, 1);
    drop(daemon);
    for name in ["first", "second"] {
        fs::remove_dir_all(image.path().join(name).join("module")).unwrap();
    }
    fs::remove_dir_all(layout.root()).unwrap();
    for name in ["first", "second"] {
        assert_response(command(isolated_os_command(image.path()).args([
            &format!("/{name}/program"),
            "sample.value.exercise",
            "{}",
        ])));
    }
    write(&workspace, "target/closure-linux-bootstrap-acceptance.json", serde_json::to_vec_pretty(&json!({
        "format":"dever-linux-bootstrap-acceptance-v1","platform":"linux-x86_64",
        "bootstrap":manifest.artifacts.iter().filter(|artifact| artifact.path.starts_with("bootstrap/")).collect::<Vec<_>>(),
        "checks":{"signed_first_install":true,"stable_public_entry":true,"root_owned_systemd_assets":true,"two_projects_in_minimal_os_root":true,"real_distinct_uids":true,"shared_compile_entries":1,"failed_install_preserves_old_entry":true,"source_and_machine_removed_execution":true},
        "global_service_actions":"not_run","public_release":"not_published"
    })).unwrap());
}
