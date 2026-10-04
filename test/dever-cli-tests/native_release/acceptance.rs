//! Explicit Linux acceptance using four separately built optimized archives.
use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use dever_cli::toolchain::{Layout, MachineManager, Version, cache_status, packaging, sha256_file};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::Deserialize;
use serde_json::{Value, json};

#[path = "../support/daemon.rs"]
mod daemon;
#[path = "../support/llvm_inputs.rs"]
mod inputs;
#[path = "../../dever-tests/tests/support/process.rs"]
mod process;
#[path = "../../dever-tests/tests/support/sandbox.rs"]
mod sandbox;
use super::temp::TemporaryDirectory;

#[path = "ecosystem_acceptance.rs"]
mod ecosystems;

#[path = "bootstrap_acceptance.rs"]
mod bootstrap;

#[path = "cross_acceptance.rs"]
mod cross;

#[path = "command_acceptance.rs"]
mod command_acceptance;

const PROFILES: [&str; 4] = ["base", "sqlite", "postgres", "both"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceSettings {
    machine_temporary_root: Option<PathBuf>,
}

fn machine_temporary_root(workspace: &Path) -> Result<Option<PathBuf>, String> {
    let path = workspace.join("target/native-release-inputs/config/setting.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read acceptance settings: {error}")),
    };
    let settings: AcceptanceSettings = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid acceptance settings: {error}"))?;
    if let Some(root) = &settings.machine_temporary_root
        && (!root.is_absolute()
            || root
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)))
    {
        return Err("machine_temporary_root must be an absolute path without '..'".into());
    }
    Ok(settings.machine_temporary_root)
}

fn machine_temporary_directory(workspace: &Path) -> TemporaryDirectory {
    match machine_temporary_root(workspace).expect("valid native release acceptance settings") {
        Some(root) => TemporaryDirectory::new_in(&root),
        None => TemporaryDirectory::new(),
    }
}

#[test]
fn acceptance_machine_directory_uses_only_explicit_author_settings() {
    let workspace = TemporaryDirectory::new();
    assert_eq!(machine_temporary_root(workspace.path()).unwrap(), None);
    let config = "target/native-release-inputs/config/setting.json";
    write(
        workspace.path(),
        config,
        br#"{"machine_temporary_root":"relative"}"#,
    );
    assert!(
        machine_temporary_root(workspace.path())
            .unwrap_err()
            .contains("absolute path")
    );
    write(
        workspace.path(),
        config,
        br#"{"machine_temporary_root":"/tmp","unknown":true}"#,
    );
    assert!(
        machine_temporary_root(workspace.path())
            .unwrap_err()
            .contains("unknown field")
    );
    let selected = TemporaryDirectory::new();
    write(
        workspace.path(),
        config,
        serde_json::to_vec(&json!({"machine_temporary_root":selected.path()})).unwrap(),
    );
    let directory = machine_temporary_directory(workspace.path());
    assert_eq!(directory.path().parent(), Some(selected.path()));
    assert!(directory.path().is_dir());
    let path = directory.path().to_owned();
    drop(directory);
    assert!(!path.exists());
    assert!(selected.path().is_dir());
}

fn write(root: &Path, relative: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn input(author: &Path, source: &Path, name: &str) -> Value {
    let relative = format!("inputs/{name}");
    let path = author.join(&relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // This is immutable test input staging, never a published release artifact.
    fs::hard_link(source, &path).unwrap();
    json!({"source":relative,"sha256":sha256_file(&path).unwrap()})
}

fn author_inputs(author: &Path, workspace: &Path) -> Ed25519KeyPair {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let key = Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap();
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(author.join("signing.pk8"))
        .unwrap()
        .write_all(document.as_ref())
        .unwrap();
    let mut profiles = serde_json::Map::new();
    for profile in PROFILES {
        let name = format!("{profile}.a");
        profiles.insert(
            profile.into(),
            input(
                author,
                &workspace.join("target/native-release-inputs").join(&name),
                &name,
            ),
        );
    }
    let compiler = input(author, Path::new(env!("CARGO_BIN_EXE_dever")), "dever");
    let core_libraries = inputs::core_libraries()
        .into_iter()
        .map(|(name, source)| {
            let mut value = input(author, &source, name);
            value["path"] = json!(name);
            value
        })
        .collect::<Vec<_>>();
    let mut links = serde_json::Map::new();
    for (name, source) in inputs::native_inputs()
        .into_iter()
        .filter(|(name, _)| *name != "runtime.a")
    {
        let mut entry = input(author, &source, name);
        entry["path"] = json!(name);
        links.insert(name.into(), entry);
    }
    let ordered = |names: &[&str]| {
        names
            .iter()
            .map(|name| links[*name].clone())
            .collect::<Vec<_>>()
    };
    let sandbox = sandbox::assets(workspace).into_iter().map(|asset| {
        let relative = format!("inputs/{}", asset.path);
        write(author, &relative, &asset.bytes);
        fs::set_permissions(author.join(&relative), fs::Permissions::from_mode(if asset.executable { 0o755 } else { 0o644 })).unwrap();
        json!({"source":relative, "path":asset.path.strip_prefix("sandbox/").unwrap(), "sha256":sha256_file(&author.join(&relative)).unwrap()})
    }).collect::<Vec<_>>();
    write(
        author,
        "inputs/SKILL.md",
        b"---\nname: dever-language\ndescription: Dever acceptance skill.\n---\n",
    );
    let skill = json!({"source":"inputs/SKILL.md", "path":"SKILL.md", "sha256":sha256_file(&author.join("inputs/SKILL.md")).unwrap()});
    write(
        author,
        "config/setting.json",
        serde_json::to_vec(&json!({
            "format":"dever-native-release-input-v1", "version":env!("CARGO_PKG_VERSION"),
            "target":"x86_64-unknown-linux-gnu", "signing_key":"signing.pk8",
            "core":compiler, "core_libraries":core_libraries, "profiles":profiles,
            "skill":[skill],
            "start":ordered(&["crt1.o","crti.o","crtbeginT.o"]),
            "libraries":ordered(&["libc.a","libm.a","libmvec.a","libgcc.a","libgcc_eh.a"]),
            "end":ordered(&["crtend.o","crtn.o"]), "sandbox":sandbox
        }))
        .unwrap(),
    );
    key
}

fn command(command: &mut Command) -> Output {
    command_with_timeout(command, Duration::from_secs(190))
}

fn isolated_os_command(root: &Path) -> Command {
    let mut command = isolated_os_namespace(root);
    command.args(["--", "/fixture-proc-init"]);
    command
}

fn isolated_os_namespace(root: &Path) -> Command {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/sandbox-inputs/assets")
        .canonicalize()
        .expect("prepare the explicit Linux isolation assets");
    let init = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/examples/native-acceptance-init")
        .canonicalize()
        .expect("build the explicit static native-acceptance-init example");
    let bytes = fs::read(&init).unwrap();
    let executable = goblin::elf::Elf::parse(&bytes).unwrap();
    assert!(executable.interpreter.is_none() && executable.libraries.is_empty());
    // A plain chroot cannot create a nested user namespace. Use a real mount
    // namespace root so the program can retain its normal Worker sandbox.
    dever_sandbox::validate_bwrap(&fs::read(assets.join("bin/bwrap")).unwrap()).unwrap();
    let mut command = Command::new(assets.join("bin/bwrap"));
    command
        .env_clear()
        .args([
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--bind",
        ])
        .arg(root)
        .args(["/", "--proc", "/proc", "--dev", "/dev", "--chdir", "/"])
        .arg("--ro-bind")
        .arg(init)
        .arg("/fixture-proc-init");
    command
}

fn command_with_timeout(command: &mut Command, timeout: Duration) -> Output {
    let capture = TemporaryDirectory::new();
    let stdout = capture.path().join("stdout");
    let stderr = capture.path().join("stderr");
    let status = process::status(
        command
            .env_clear()
            .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
            .stderr(Stdio::from(fs::File::create(&stderr).unwrap())),
        timeout,
    )
    .unwrap();
    let output = Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    };
    assert!(
        output.status.success(),
        "{:?}: {}\n{}",
        command,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn application(root: &Path, profile: &str) {
    let sqlite = json!({"type":"sqlite","path":"data/owned.sqlite","max_connections":1});
    let postgres =
        json!({"type":"postgres","url":"postgres://fixture@127.0.0.1:1/unused", "tls":"disabled"});
    let settings = match profile {
        "base" => json!({}),
        "sqlite" => json!({"database":{"default":sqlite}}),
        "postgres" => json!({"database":{"default":postgres}}),
        "both" => json!({"database":{"default":sqlite,"unused":postgres}}),
        _ => unreachable!(),
    };
    write(
        root,
        "config/setting.json",
        serde_json::to_vec(&settings).unwrap(),
    );
    write(
        root,
        "module/sample/value/api.dever",
        "cmd exercise = app.exercise\n",
    );
    let app = if matches!(profile, "sqlite" | "both") {
        write(
            root,
            "module/sample/value/model.dever",
            "type Value { name: Text(1, 32) }\n",
        );
        "exercise() (okay: Bool) { saved = model.create({ name = \"pack\" })\n  count = model.count()\n  removed = model.delete(saved.id)\n  okay = count == 1 and removed == 1 }\n"
    } else {
        "exercise() (okay: Bool) { dever.task.sleep(1)\n  okay = true }\n"
    };
    write(root, "module/sample/value/app.dever", app);
}

fn assert_response(output: Output) {
    assert_response_data(output, json!(true));
}

fn assert_response_data(output: Output, expected: Value) {
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["code"], 0);
    assert_eq!(value["data"], expected);
}

fn exec_application(root: &Path, workspace: &Path) {
    for (name, source) in [
        ("app", "read() (value: Int) { value = port.read() }"),
        ("port", "read() (value: Int) fails dever.time.SleepResult"),
        (
            "adapter",
            "setting { mode: Text }\nexternal exec \"worker\" {}",
        ),
        ("api", "cmd read = app.read"),
    ] {
        write(
            root,
            &format!("module/notification/delivery/{name}.dever"),
            source,
        );
    }
    write(
        root,
        "config/setting.json",
        br#"{"adapter":{"notification.delivery":{"setting":{"mode":"embedded"}}}}"#,
    );
    let fixture = workspace.join("target/native-release-inputs/component-fixture");
    assert!(
        fixture.is_file(),
        "prepare the immutable component-fixture in target/native-release-inputs"
    );
    fs::copy(fixture, root.join("module/notification/delivery/worker")).unwrap();
}

fn install_release(layout: &Layout, version: &Version, key: &Ed25519KeyPair) {
    let public_key = key
        .public_key()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    fs::write(layout.state().join("trusted-release-key"), public_key).unwrap();
    fs::set_permissions(
        layout.state().join("trusted-release-key"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    MachineManager::new(layout.clone())
        .install_requested(version.as_str())
        .unwrap();
}

fn archive_report(workspace: &Path) -> Value {
    let mut profiles = serde_json::Map::new();
    for profile in PROFILES {
        let path = workspace
            .join("target/native-release-inputs")
            .join(format!("{profile}.a"));
        let output = command(
            Command::new("/usr/bin/nm")
                .args(["--defined-only", "--demangle"])
                .arg(&path),
        );
        let symbols = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            symbols.contains(" sqlite3_open"),
            matches!(profile, "sqlite" | "both"),
            "{profile} SQLite driver"
        );
        assert_eq!(
            symbols.contains("tokio_postgres"),
            matches!(profile, "postgres" | "both"),
            "{profile} PostgreSQL driver"
        );
        assert!(!symbols.contains("LLVMContextCreate"));
        profiles.insert(profile.into(), json!({"bytes":fs::metadata(&path).unwrap().len(),"sha256":sha256_file(&path).unwrap()}));
    }
    json!(profiles)
}

#[test]
#[ignore = "requires prepared optimized archives and root for an owned GNU ABI root"]
fn signed_core_builds_without_host_compiler_libraries_or_tools() {
    assert_eq!(
        rustix::process::geteuid().as_raw(),
        0,
        "explicit owned namespace acceptance requires root"
    );
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let author = TemporaryDirectory::new();
    author_inputs(author.path(), &workspace);
    let release = machine_temporary_directory(&workspace);
    let payload = release.path().join("release");
    let manifest = packaging::create(author.path(), &payload).unwrap();
    let isolated = machine_temporary_directory(&workspace);
    ecosystems::minimal_os(isolated.path());
    let compiler_root = isolated.path().join("compiler");
    for artifact in &manifest.artifacts {
        let relative = if artifact.path == "dever-core" {
            "dever"
        } else {
            &artifact.path
        };
        let destination = compiler_root.join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        // Immutable signed inputs; no host directory is mounted into the root.
        fs::hard_link(payload.join(&artifact.path), destination).unwrap();
    }
    application(&isolated.path().join("project"), "sqlite");
    let invoke = |arguments: &[&str]| command(isolated_os_command(isolated.path()).args(arguments));
    invoke(&["/compiler/dever", "check", "/project"]);
    invoke(&[
        "/compiler/dever",
        "build",
        "/project",
        "--output",
        "/project/program",
    ]);
    fs::remove_dir_all(isolated.path().join("project/module")).unwrap();
    assert_response(invoke(&["/project/program", "sample.value.exercise", "{}"]));
    write(&workspace, "target/closure-core-loader-acceptance.json", serde_json::to_vec_pretty(&json!({
        "platform":"linux-x86_64", "os_baseline":"GNU libc 2.39 and its named companion libraries",
        "core_libraries":manifest.artifacts.iter().filter(|artifact| artifact.path.starts_with("lib/")).collect::<Vec<_>>(),
        "checks":{"signed_payload":true,"check_in_minimal_root":true,"sqlite_build_in_minimal_root":true,"source_removed_execution":true},
        "host_compiler_libraries":false,"host_language_tools":false
    })).unwrap());
}

#[test]
#[ignore = "requires four optimized archives and component-fixture; starts only an owned daemon, SQLite and exec Worker fixtures"]
fn optimized_profiles_make_reproducible_signed_release_and_run_through_daemon() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let profiles = archive_report(&workspace);
    let author = TemporaryDirectory::new();
    let key = author_inputs(author.path(), &workspace);
    let machine = machine_temporary_directory(&workspace);
    let layout = Layout::new(machine.path());
    layout.initialize().unwrap();
    let first = machine.path().join("first");
    let expected = packaging::create(author.path(), &first).unwrap();
    let manifest = fs::read(first.join("manifest.json")).unwrap();
    let signature = fs::read(first.join("manifest.sig")).unwrap();
    // Remove only this test's first completed copy before making the second.
    fs::remove_dir_all(&first).unwrap();
    let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let release = layout.downloads().join(version.as_str());
    command(
        Command::new(workspace.join("target/debug/examples/native-release"))
            .arg(author.path())
            .arg("--output")
            .arg(&release),
    );
    assert_eq!(
        serde_json::from_slice::<dever_cli::toolchain::ReleaseManifest>(
            &fs::read(release.join("manifest.json")).unwrap()
        )
        .unwrap(),
        expected
    );
    assert_eq!(fs::read(release.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(release.join("manifest.sig")).unwrap(), signature);
    install_release(&layout, &version, &key);
    fs::remove_dir_all(&release).unwrap();
    drop(author);
    let compiler = layout.versions().join(version.as_str()).join("dever-core");
    let mut daemon = daemon::TestDaemon::start(&layout);
    let projects = TemporaryDirectory::new();
    let mut binaries = serde_json::Map::new();
    let mut programs: Vec<(PathBuf, PathBuf)> = Vec::new();
    for profile in PROFILES {
        let project = projects.path().join(profile);
        application(&project, profile);
        command(Command::new(&compiler).arg("check").arg(&project));
        assert_response(command(
            Command::new(&compiler).arg("run").arg(&project).args([
                "--",
                "sample.value.exercise",
                "{}",
            ]),
        ));
        let executable = project.join("program");
        command(
            Command::new(&compiler)
                .arg("build")
                .arg(&project)
                .arg("--output")
                .arg(&executable),
        );
        binaries.insert(profile.into(), json!({"bytes":fs::metadata(&executable).unwrap().len(),"sha256":sha256_file(&executable).unwrap()}));
        programs.push((project, executable));
    }
    assert_eq!(cache_status(&layout).unwrap().entries, 4);
    let exec_project = TemporaryDirectory::new();
    let exec_source = exec_project.path().to_owned();
    exec_application(&exec_source, &workspace);
    assert_response_data(
        command(Command::new(&compiler).arg("run").arg(&exec_source).args([
            "--",
            "notification.delivery.read",
            "{}",
        ])),
        json!(7),
    );
    let standalone = TemporaryDirectory::new();
    let exec_program = standalone.path().join("application");
    fs::create_dir(standalone.path().join("config")).unwrap();
    fs::copy(
        exec_source.join("config/setting.json"),
        standalone.path().join("config/setting.json"),
    )
    .unwrap();
    command(
        Command::new(&compiler)
            .arg("build")
            .arg(&exec_source)
            .arg("--output")
            .arg(&exec_program),
    );
    assert_eq!(cache_status(&layout).unwrap().entries, 5);
    drop(exec_project);
    assert!(!exec_source.exists());
    daemon.stop();
    assert_response_data(
        command(
            Command::new(&exec_program)
                .current_dir(standalone.path())
                .args(["notification.delivery.read", "{}"]),
        ),
        json!(7),
    );
    assert!(!standalone.path().join("module").exists());
    let exec = json!({
        "bytes": fs::metadata(&exec_program).unwrap().len(),
        "sha256": sha256_file(&exec_program).unwrap(),
        "managed_run": true, "standalone_run": true,
        "source_project_removed": true, "response": 7
    });
    for (project, executable) in programs {
        fs::remove_dir_all(project.join("module")).unwrap();
        assert_response(command(
            Command::new(executable).args(["sample.value.exercise", "{}"]),
        ));
    }
    let report = json!({
        "format":"dever-native-release-acceptance-v1", "platform":"linux-x86_64",
        "version":version.as_str(), "profiles":profiles,"programs":binaries,"exec":exec,
        "manifest_sha256":format!("{:x}", sha2::Sha256::digest(&manifest)),
        "checks":{"pack_determinism":true,"signed_install":true,"managed_profiles":4,"standalone_profiles":4,"sqlite_crud":true,"signed_exec_worker":true},
        "postgres_server":"not_run", "cross_host_source_reproducibility":"not_run"
    });
    use sha2::Digest;
    write(
        &workspace,
        "target/closure-native-release-acceptance.json",
        serde_json::to_vec_pretty(&report).unwrap(),
    );
}
