//! Real process tests are supplied with explicit author-owned sandbox assets.

#[path = "../dever-tests/tests/support/process.rs"]
pub mod process;
#[path = "../dever-tests/tests/support/temp.rs"]
mod temp;

use dever_sandbox::{Capabilities, Grant, Launch};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn bootstrap_modes_include_the_loader_and_only_declared_executables() {
    assert!(dever_sandbox::asset_executable("bin/bwrap"));
    assert!(dever_sandbox::asset_executable("bin/guard"));
    assert!(dever_sandbox::asset_executable("lib/ld-linux-x86-64.so.2"));
    assert!(dever_sandbox::asset_executable("lib/ld-linux-aarch64.so.1"));
    assert!(!dever_sandbox::asset_executable("lib/libc.so.6"));
    assert!(!dever_sandbox::asset_executable(
        "other/ld-linux-x86-64.so.2"
    ));
    assert!(!dever_sandbox::asset_executable(
        "other/ld-linux-aarch64.so.1"
    ));
    assert!(!dever_sandbox::asset_executable("lib/ld-linux-armhf.so.3"));
    assert!(!dever_sandbox::asset_executable(
        "lib/ld-linux-aarch64.so.1.backup"
    ));
    assert!(!dever_sandbox::asset_executable("bin/unverified"));
}

#[test]
fn sandbox_paths_reject_relative_inputs_before_starting_a_process() {
    use std::path::Path;
    let launch = dever_sandbox::Launch {
        assets: Path::new("relative"),
        worker: Path::new("/missing"),
        executable: Path::new("/missing/probe"),
        arguments: &[],
        working_directory: Path::new("/missing"),
        capabilities: dever_sandbox::Capabilities::default(),
        grants: &[],
    };
    assert!(
        launch
            .command()
            .unwrap_err()
            .contains("absolute and normalized")
    );
}

#[cfg(target_os = "linux")]
struct Fixture {
    directory: temp::TemporaryDirectory,
    assets: PathBuf,
    worker: PathBuf,
}

#[cfg(target_os = "linux")]
struct SandboxProcess(std::process::Child);

#[cfg(target_os = "linux")]
impl Drop for SandboxProcess {
    fn drop(&mut self) {
        // A failing assertion must not leave the owned namespace running.
        let _ = process::terminate(&mut self.0);
    }
}

#[cfg(target_os = "linux")]
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = temp::TemporaryDirectory::new();
        let assets = directory.path().join("assets");
        let worker = directory.path().join("worker");
        let prepared =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sandbox-inputs/assets");
        fs::create_dir_all(assets.join("bin")).unwrap();
        fs::create_dir_all(assets.join("lib")).unwrap();
        fs::create_dir(&worker).unwrap();
        fs::copy(prepared.join("bin/bwrap"), assets.join("bin/bwrap"))
            .expect("prepare explicit target/sandbox-inputs first");
        fs::copy(
            env!("CARGO_BIN_EXE_dever-sandbox-guard"),
            assets.join("bin/guard"),
        )
        .unwrap();
        for entry in fs::read_dir(prepared.join("lib")).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            fs::copy(entry.path(), assets.join("lib").join(entry.file_name())).unwrap();
        }
        fs::copy(std::env::current_exe().unwrap(), worker.join("probe")).unwrap();
        // The unprivileged case can traverse only these explicitly owned trees.
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            directory,
            assets,
            worker,
        }
    }

    fn command(&self, probe: &str, capabilities: Capabilities, grants: &[Grant]) -> Command {
        Launch {
            assets: &self.assets,
            worker: &self.worker,
            executable: &self.worker.join("probe"),
            arguments: &[
                "--ignored".into(),
                "--exact".into(),
                probe.into(),
                "--nocapture".into(),
            ],
            working_directory: &self.worker,
            capabilities,
            grants,
        }
        .command()
        .unwrap()
    }

    fn assert_success(&self, command: &mut Command) {
        let stdout = self.directory.path().join("stdout");
        let stderr = self.directory.path().join("stderr");
        command
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap());
        let result = process::status(command, Duration::from_secs(10));
        assert!(
            result.as_ref().is_ok_and(|status| status.success()),
            "sandbox failed: {result:?}\nstdout: {}\nstderr: {}",
            fs::read_to_string(stdout).unwrap(),
            fs::read_to_string(stderr).unwrap()
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicitly prepared target/sandbox-inputs assets; owns only test processes"]
fn enforces_files_network_process_and_threads() {
    let fixture = Fixture::new();
    fixture.assert_success(&mut fixture.command("restricted_probe", Capabilities::default(), &[]));
    let output = fixture.directory.path().join("output");
    fs::create_dir(&output).unwrap();
    fixture.assert_success(&mut fixture.command(
        "granted_probe",
        Capabilities {
            network: true,
            process: true,
        },
        &[Grant {
            source: output.clone(),
            destination: "/data/output".into(),
            writable: true,
        }],
    ));
    assert_eq!(fs::read(output.join("result")).unwrap(), b"granted");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicitly prepared sandbox assets; mutates only owned fixture paths"]
fn pins_grants_against_directory_replacement() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let granted = fixture.directory.path().join("granted");
    let outside = fixture.directory.path().join("outside");
    fs::create_dir(&granted).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(granted.join("value"), "authorized").unwrap();
    fs::write(outside.join("value"), "outside").unwrap();
    let mut command = fixture.command(
        "pinned_grant_probe",
        Capabilities::default(),
        &[Grant {
            source: granted.clone(),
            destination: "/data/input".into(),
            writable: false,
        }],
    );
    fs::rename(&granted, fixture.directory.path().join("moved")).unwrap();
    symlink(&outside, &granted).unwrap();
    fixture.assert_success(&mut command);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "internal sandbox descriptor-pinning probe"]
fn pinned_grant_probe() {
    assert_eq!(
        fs::read_to_string("/data/input/value").unwrap(),
        "authorized"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicitly prepared sandbox assets"]
fn rejects_writable_aliases_of_executable_inputs() {
    let fixture = Fixture::new();
    for source in [
        &fixture.assets,
        &fixture.worker,
        &fixture.directory.path().to_owned(),
    ] {
        let error = Launch {
            assets: &fixture.assets,
            worker: &fixture.worker,
            executable: &fixture.worker.join("probe"),
            arguments: &[],
            working_directory: &fixture.worker,
            capabilities: Capabilities::default(),
            grants: &[Grant {
                source: source.to_owned(),
                destination: "/data/alias".into(),
                writable: true,
            }],
        }
        .command()
        .unwrap_err();
        assert!(error.contains("overlaps executable inputs"), "{error}");
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicitly prepared signed-release Python/Node inputs and sandbox assets"]
fn prepared_python_and_node_run_without_host_files_or_process_creation() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ecosystem-release-inputs/prepared")
        .canonicalize()
        .unwrap();
    let python = "import ctypes, hashlib, json, math, os, pathlib, sqlite3, ssl, sys, threading, zlib; assert all(not p or pathlib.Path(p).is_relative_to('/worker') for p in sys.path); assert not os.path.exists('/usr/bin/python3'); assert json.loads(zlib.decompress(zlib.compress(b'{\"value\":42}')))['value'] == 42; t = threading.Thread(target=lambda: None); t.start(); t.join(); print('python okay')";
    let javascript = "const assert = require('node:assert/strict'); const fs = require('node:fs'); const crypto = require('node:crypto'); const zlib = require('node:zlib'); assert.equal(fs.existsSync('/usr/bin/node'), false); assert.equal(zlib.gunzipSync(zlib.gzipSync('42')).toString(), '42'); assert.equal(crypto.createHash('sha256').update('probe').digest().length, 32); console.log('node okay');";
    for (ecosystem, executable, args) in [
        (
            "pip",
            "bin/python3.12",
            vec!["-I", "-S", "-B", "-c", python],
        ),
        ("npm", "bin/node", vec!["-e", javascript]),
    ] {
        let source = root.join(ecosystem);
        let worker = fixture.directory.path().join(ecosystem);
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            fs::create_dir_all(worker.join(&relative)).unwrap();
            for entry in fs::read_dir(source.join(&relative)).unwrap() {
                let entry = entry.unwrap();
                let path = relative.join(entry.file_name());
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    pending.push(path);
                } else {
                    assert!(kind.is_file(), "prepared inputs must not contain links");
                    if path == Path::new(executable) {
                        fs::copy(entry.path(), worker.join(&path)).unwrap();
                        fs::set_permissions(worker.join(path), fs::Permissions::from_mode(0o700))
                            .unwrap();
                    } else {
                        fs::hard_link(entry.path(), worker.join(path)).unwrap();
                    }
                }
            }
        }
        let arguments = args
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let mut command = Launch {
            assets: &fixture.assets,
            worker: &worker,
            executable: &worker.join(executable),
            arguments: &arguments,
            working_directory: &worker,
            capabilities: Capabilities::default(),
            grants: &[],
        }
        .command()
        .unwrap();
        fixture.assert_success(&mut command);
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicitly prepared assets and host permission for unprivileged user namespaces"]
fn enforces_unprivileged_namespace() {
    if rustix::process::geteuid().as_raw() != 0 {
        unprivileged_launch();
        return;
    }
    // Select the product helper after the real UID transition. Changing only
    // Command.uid after root constructed Launch tests a different call flow.
    let mut command = Command::new("/usr/bin/setpriv");
    command
        .env_clear()
        .args([
            "--reuid=65534",
            "--regid=65534",
            "--clear-groups",
            "--inh-caps=-all",
            "--ambient-caps=-all",
            "--",
        ])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "unprivileged_launcher_probe",
            "--nocapture",
        ])
        .stdin(Stdio::null());
    assert!(
        process::status(&mut command, Duration::from_secs(20))
            .unwrap()
            .success()
    );
}

#[cfg(target_os = "linux")]
fn unprivileged_launch() {
    assert_ne!(rustix::process::geteuid().as_raw(), 0);
    let fixture = Fixture::new();
    let mut command = fixture.command("restricted_probe", Capabilities::default(), &[]);
    fixture.assert_success(&mut command);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "internal non-root launcher probe, invoked by enforces_unprivileged_namespace"]
fn unprivileged_launcher_probe() {
    assert!(rustix::process::getgroups().unwrap().is_empty());
    unprivileged_launch();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "internal sandbox probe, invoked only inside an owned namespace"]
fn restricted_probe() {
    assert_eq!(std::env::current_dir().unwrap(), Path::new("/worker"));
    assert!(std::env::vars_os().next().is_none());
    assert!(fs::read("/etc/passwd").is_err());
    assert!(fs::read("/proc/1/root/etc/passwd").is_err());
    assert!(fs::write("/worker/forbidden", b"no").is_err());
    assert!(std::net::TcpListener::bind("127.0.0.1:0").is_err());
    assert!(std::os::unix::net::UnixListener::bind("/tmp/denied.sock").is_err());
    assert!(
        Command::new("/worker/probe")
            .arg("--list")
            .status()
            .is_err()
    );
    assert_eq!(std::thread::spawn(|| 42).join().unwrap(), 42);
    fs::write("/tmp/private", b"temporary").unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicitly prepared assets; kills only this test's namespace"]
fn terminates_detached_descendants_on_kill() {
    let fixture = Fixture::new();
    let output = fixture.directory.path().join("output");
    fs::create_dir(&output).unwrap();
    let mut command = fixture.command(
        "tree_probe",
        Capabilities {
            network: false,
            process: true,
        },
        &[Grant {
            source: output.clone(),
            destination: "/data/output".into(),
            writable: true,
        }],
    );
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = SandboxProcess(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !output.join("child-ready").is_file() {
        if std::time::Instant::now() >= deadline || child.0.try_wait().unwrap().is_some() {
            panic!("sandbox descendant did not start");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut descendants = Vec::new();
    let mut pending = vec![child.0.id()];
    while let Some(pid) = pending.pop() {
        let children =
            fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap_or_default();
        for child in children
            .split_whitespace()
            .map(|value| value.parse::<u32>().unwrap())
        {
            descendants.push(child);
            pending.push(child);
        }
    }
    assert!(
        descendants.len() >= 2,
        "the probe must own a real descendant tree"
    );
    process::terminate(&mut child.0).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let running = descendants
            .iter()
            .filter(|pid| {
                fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                    !stat.rsplit_once(") ").is_some_and(|(_, fields)| {
                        fields.starts_with("Z ") || fields.starts_with("X ")
                    })
                })
            })
            .count();
        if running == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{running} owned sandbox descendants survived termination"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "internal sandbox process-tree probe"]
fn tree_probe() {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new("/worker/probe")
        .args(["--ignored", "--exact", "descendant_probe"])
        .process_group(0)
        .spawn()
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "internal sandbox descendant probe"]
fn descendant_probe() {
    fs::write("/data/output/child-ready", b"ready").unwrap();
    std::thread::sleep(Duration::from_secs(30));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "internal sandbox probe, invoked only inside an owned namespace"]
fn granted_probe() {
    use std::net::ToSocketAddrs;
    assert!(
        ("localhost", 80)
            .to_socket_addrs()
            .unwrap()
            .next()
            .is_some()
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    assert_ne!(listener.local_addr().unwrap().port(), 0);
    assert!(std::os::unix::net::UnixListener::bind("/tmp/denied.sock").is_err());
    assert!(
        Command::new("/worker/probe")
            .arg("--list")
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    fs::write("/data/output/result", b"granted").unwrap();
    assert!(fs::read("/etc/passwd").is_err());
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicitly prepared target/sandbox-inputs assets"]
fn rejects_missing_dependency_before_host_loader_can_supply_it() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.assets.join("lib/libc.so.6")).unwrap();
    let error = Launch {
        assets: &fixture.assets,
        worker: &fixture.worker,
        executable: &fixture.worker.join("probe"),
        arguments: &[],
        working_directory: &fixture.worker,
        capabilities: Capabilities::default(),
        grants: &[],
    }
    .command()
    .unwrap_err();
    assert!(
        error.contains("sandbox dependency 'lib/libc.so.6' is missing"),
        "{error}"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicit assets and the prepared C syscall probe"]
fn kernel_rejects_namespace_ptrace_and_truncated_ioctl_requests() {
    let fixture = Fixture::new();
    let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/sandbox-inputs/syscalls");
    let executable = fixture.worker.join("syscalls");
    fs::copy(probe, &executable).expect("prepare the explicit C syscall probe first");
    for process in [false, true] {
        let arguments = if process {
            vec![]
        } else {
            vec!["no-process".into()]
        };
        let mut command = Launch {
            assets: &fixture.assets,
            worker: &fixture.worker,
            executable: &executable,
            arguments: &arguments,
            working_directory: &fixture.worker,
            capabilities: Capabilities {
                network: true,
                process,
            },
            grants: &[],
        }
        .command()
        .unwrap();
        fixture.assert_success(&mut command);
    }
}
