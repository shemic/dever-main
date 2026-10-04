//! Actual default CLI boundary. Native cases consume an explicitly prepared
//! runtime archive; ordinary negative checks never start a compiler subprocess.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

#[path = "support/llvm_pack.rs"]
mod pack;
#[path = "../dever-tests/tests/support/process.rs"]
mod process;
#[path = "../dever-tests/tests/support/temp.rs"]
mod temp;
use temp::TemporaryDirectory;

const APP: &str = r#"type Outcome { error Denied(message: Text, labels: List<Text?>) }
echo(name: Text) (value: Text) { dever.task.sleep(1)
  value = name }
divide(divisor: Int) (value: Int) { value = 42 // divisor }
deny() (value: Text) { fail(Outcome.Denied("owned denial", ["nested label", null])) }
"#;

fn write(root: &Path, relative: &str, source: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

fn project(root: &Path) {
    write(root, "module/sample/echo/app.dever", APP);
    write(
        root,
        "module/sample/echo/api.dever",
        "cmd echo = app.echo\ncmd divide = app.divide\ncmd deny = app.deny",
    );
    write(root, "config/setting.json", "{}");
}

fn compiler(root: &Path, name: &str) -> PathBuf {
    pack::compiler(Path::new(env!("CARGO_BIN_EXE_dever")), root, name)
}

fn output(command: &mut Command) -> Output {
    let capture = TemporaryDirectory::new();
    let stdout = capture.path().join("stdout");
    let stderr = capture.path().join("stderr");
    command
        .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
        .stderr(Stdio::from(fs::File::create(&stderr).unwrap()));
    let status = process::status(command, Duration::from_secs(180)).unwrap();
    Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}

fn cli(compiler: &Path, project: &Path, action: &str, args: &[&str]) -> Output {
    output(
        Command::new(compiler)
            .env_clear()
            .arg(action)
            .arg(project)
            .args(args),
    )
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn failure(output: &Output, expected: &str) {
    assert!(!output.status.success(), "unexpected success");
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        message.contains(expected),
        "expected {expected:?}: {message}"
    );
}

#[test]
fn missing_pack_fails_without_host_tools_and_check_and_empty_test_need_no_pack() {
    let toolchain = TemporaryDirectory::new();
    let compiler = compiler(toolchain.path(), "dever");
    let help = output(Command::new(&compiler).env_clear().arg("--help"));
    success(&help);
    assert!(String::from_utf8_lossy(&help.stdout).contains("dever run <project-root>"));
    let project_dir = TemporaryDirectory::new();
    project(project_dir.path());
    success(&cli(&compiler, project_dir.path(), "check", &[]));
    let tests = cli(&compiler, project_dir.path(), "test", &[]);
    success(&tests);
    assert!(String::from_utf8_lossy(&tests.stdout).contains("0 passed; 0 failed"));
    failure(
        &cli(
            &compiler,
            project_dir.path(),
            "run",
            &["--", "sample.echo.echo", "{\"name\":\"Ada\"}"],
        ),
        "runtime pack",
    );
    assert!(!toolchain.path().join("cache").exists());
}

#[test]
fn cross_build_requires_the_selected_pack_even_when_host_pack_exists() {
    let root = TemporaryDirectory::new();
    let compiler = compiler(root.path(), "dever");
    pack::manifest(root.path(), false);
    let project_dir = TemporaryDirectory::new();
    project(project_dir.path());
    let executable = root.path().join("arm-application");
    failure(
        &cli(
            &compiler,
            project_dir.path(),
            "build",
            &[
                "--target",
                "linux-aarch64",
                "--output",
                executable.to_str().unwrap(),
            ],
        ),
        "runtime/linux-aarch64",
    );
    assert!(!executable.exists());
    assert!(!root.path().join("cache").exists());
}

#[test]
fn managed_cores_never_fall_back_to_private_packs_or_cache() {
    let root = TemporaryDirectory::new();
    let project_dir = TemporaryDirectory::new();
    project(project_dir.path());
    for (directory, diagnostic) in [
        (
            root.path().join("standalone"),
            "managed compiler must be installed",
        ),
        (
            root.path().join("versions/0.1.0"),
            "cannot inspect deverd state directory",
        ),
    ] {
        let compiler = compiler(&directory, "dever-core");
        failure(&cli(&compiler, project_dir.path(), "run", &[]), diagnostic);
        assert!(!directory.join("cache").exists());
    }
    let installed = root.path().join("versions/0.1.0/dever-core");
    let worker = output(
        Command::new(installed)
            .env_clear()
            .current_dir(project_dir.path())
            .arg("--dever-compile-worker"),
    );
    failure(&worker, "service-owned native cache staging directory");
    assert!(!project_dir.path().join("program").exists());
}

#[test]
fn pack_contract_rejects_drift_duplicate_paths_and_undeclared_link_inputs() {
    let root = TemporaryDirectory::new();
    let compiler = compiler(root.path(), "dever");
    let original = pack::manifest(root.path(), false);
    let project_dir = TemporaryDirectory::new();
    project(project_dir.path());
    let mutations: &[(&str, Value, &str)] = &[
        ("/format", json!("future"), "does not match"),
        ("/compiler_version", json!("99.0.0"), "does not match"),
        ("/abi", json!(99), "does not match"),
        (
            "/target",
            json!("aarch64-unknown-linux-gnu"),
            "does not match",
        ),
        ("/profiles/base", Value::Null, "missing the required 'base'"),
        (
            "/profiles/base/runtime",
            json!("missing.a"),
            "undeclared or duplicate",
        ),
        (
            "/profiles/base/libraries/0",
            json!("runtime.a"),
            "undeclared or duplicate",
        ),
        (
            "/files/0/path",
            json!("../runtime.a"),
            "normalized relative",
        ),
        (
            "/files/0/path",
            json!("nested//runtime.a"),
            "normalized relative",
        ),
        (
            "/files/0/path",
            json!("C:\\runtime.a"),
            "normalized relative",
        ),
        (
            "/files/1/path",
            json!("runtime.a"),
            "duplicate runtime file",
        ),
        ("/files/0/bytes", json!(9), "SHA-256 verification"),
        (
            "/files/0/sha256",
            json!("0".repeat(64)),
            "SHA-256 verification",
        ),
    ];
    for (pointer, value, expected) in mutations {
        let mut manifest = original.clone();
        *manifest.pointer_mut(pointer).unwrap() = value.clone();
        pack::write_manifest(root.path(), &manifest);
        failure(&cli(&compiler, project_dir.path(), "run", &[]), expected);
        assert!(!root.path().join("cache").exists());
    }
    let text = serde_json::to_string(&original).unwrap();
    fs::write(
        pack::pack_root(root.path()).join("manifest.json"),
        text.replacen("\"abi\":1", "\"abi\":1,\"abi\":1", 1),
    )
    .unwrap();
    failure(
        &cli(&compiler, project_dir.path(), "run", &[]),
        "duplicate field",
    );
}

#[test]
fn pack_rejects_symlinks_and_disguised_linker_scripts() {
    use std::os::unix::fs::symlink;
    let root = TemporaryDirectory::new();
    let compiler = compiler(root.path(), "dever");
    let mut manifest = pack::manifest(root.path(), false);
    let project_dir = TemporaryDirectory::new();
    project(project_dir.path());
    let runtime = pack::pack_root(root.path()).join("runtime.a");
    let outside = root.path().join("outside.a");
    fs::rename(&runtime, &outside).unwrap();
    symlink(&outside, &runtime).unwrap();
    failure(
        &cli(&compiler, project_dir.path(), "run", &[]),
        "must be a real file",
    );
    fs::remove_file(&runtime).unwrap();
    for content in [b"INPUT(/usr/lib/forbidden.a)".as_slice(), b"!<thin>\n"] {
        fs::write(&runtime, content).unwrap();
        manifest["files"][0]["bytes"] = json!(content.len());
        manifest["files"][0]["sha256"] = json!(pack::digest(&runtime));
        pack::write_manifest(root.path(), &manifest);
        failure(
            &cli(&compiler, project_dir.path(), "run", &[]),
            "linker scripts and thin archives are forbidden",
        );
    }
    assert!(!root.path().join("cache").exists());
}

fn cached_programs(root: &Path) -> Vec<PathBuf> {
    let mut programs = fs::read_dir(root.join("cache/native"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("program"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    programs.sort();
    programs
}

#[test]
#[ignore = "requires explicitly prepared full runtime archive and Linux CRT files; serial offline CLI execution"]
fn default_llvm_runs_builds_moves_and_reuses_cache_without_host_toolchain() {
    let root = TemporaryDirectory::new();
    let compiler = compiler(root.path(), "dever");
    pack::manifest(root.path(), true);
    let project_dir = TemporaryDirectory::new();
    project(project_dir.path());
    let args = ["--", "sample.echo.echo", "{\"name\":\"你好 LLVM\"}"];
    let marker = root.path().join("forbidden-tool-was-started");
    for name in ["rustc", "cargo", "cc"] {
        use std::os::unix::fs::PermissionsExt;
        let path = root.path().join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf forbidden > '{}'\nexit 99\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let run = output(
        Command::new(&compiler)
            .env_clear()
            .env("PATH", root.path())
            .env("RUSTC", root.path().join("rustc"))
            .env("CARGO", root.path().join("cargo"))
            .env("CC", root.path().join("cc"))
            .arg("run")
            .arg(project_dir.path())
            .args(args),
    );
    success(&run);
    assert!(
        !marker.exists(),
        "LLVM compilation must not start a host compiler"
    );
    let response: Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(response["data"], "你好 LLVM");
    let cached = cached_programs(root.path());
    assert_eq!(cached.len(), 1);
    let modified = fs::metadata(&cached[0]).unwrap().modified().unwrap();
    let binary = project_dir.path().join("standalone");
    success(&cli(
        &compiler,
        project_dir.path(),
        "build",
        &["--output", binary.to_str().unwrap()],
    ));
    assert_eq!(
        fs::metadata(&cached[0]).unwrap().modified().unwrap(),
        modified
    );
    let first_digest = pack::digest(&binary);
    failure(
        &cli(
            &compiler,
            project_dir.path(),
            "build",
            &["--output", binary.to_str().unwrap()],
        ),
        "cannot save executable",
    );
    assert_eq!(pack::digest(&binary), first_digest);
    let moved = TemporaryDirectory::new();
    let relocated = moved.path().join("application");
    fs::rename(&binary, &relocated).unwrap();
    write(moved.path(), "config/setting.json", "{}");
    success(&output(
        Command::new(&relocated).env_clear().args(&args[1..]),
    ));
    {
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        let stderr = moved.path().join("closed-stdout-error");
        let status = process::status(
            Command::new(&relocated)
                .env_clear()
                .args(&args[1..])
                .stdout(Stdio::from(OwnedFd::from(writer)))
                .stderr(Stdio::from(fs::File::create(&stderr).unwrap())),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(
            status.code(),
            Some(1),
            "closed stdout must return a checked failure, not SIGPIPE"
        );
        let error = fs::read_to_string(stderr).unwrap();
        assert!(
            error.contains("Broken pipe") && error.contains(".dever"),
            "{error}"
        );
    }
    let denied = cli(
        &compiler,
        project_dir.path(),
        "run",
        &["--", "sample.echo.deny", "{}"],
    );
    failure(&denied, "owned denial");
    assert!(String::from_utf8_lossy(&denied.stderr).contains("nested label"));
    assert!(String::from_utf8_lossy(&denied.stderr).contains("app.dever"));
    failure(
        &cli(
            &compiler,
            project_dir.path(),
            "run",
            &["--", "sample.echo.echo", "{}"],
        ),
        "name",
    );
    let failure_output = cli(
        &compiler,
        project_dir.path(),
        "run",
        &["--", "sample.echo.divide", "{\"divisor\":0}"],
    );
    failure(&failure_output, "division");
    assert!(String::from_utf8_lossy(&failure_output.stderr).contains("app.dever"));
    // Corrupt only the fixture-owned cache copy, never a hard-linked runtime.
    fs::write(&cached[0], b"corrupt cached executable").unwrap();
    failure(
        &cli(&compiler, project_dir.path(), "run", &args),
        "incomplete or modified",
    );
    assert_eq!(fs::read(&cached[0]).unwrap(), b"corrupt cached executable");
    write(
        project_dir.path(),
        "module/sample/echo/app.dever",
        &APP.replace("value = name", "value = name + \"!\""),
    );
    let changed = cli(&compiler, project_dir.path(), "run", &args);
    success(&changed);
    assert_eq!(
        serde_json::from_slice::<Value>(&changed.stdout).unwrap()["data"],
        "你好 LLVM!"
    );
    assert_eq!(cached_programs(root.path()).len(), 2);

    // Even a byte-level manifest change has its own exact pack identity.
    let manifest = pack::pack_root(root.path()).join("manifest.json");
    let mut bytes = fs::read(&manifest).unwrap();
    bytes.push(b'\n');
    fs::write(&manifest, bytes).unwrap();
    success(&cli(&compiler, project_dir.path(), "run", &args));
    assert_eq!(cached_programs(root.path()).len(), 3);

    let markdown = TemporaryDirectory::new();
    write(markdown.path(), "config/setting.json", "{}");
    write(
        markdown.path(),
        "module/system/health/app.dever.md",
        include_str!("../../examples/cms/md/module/system/health/app.dever.md"),
    );
    write(
        markdown.path(),
        "module/system/health/api.dever.md",
        include_str!("../../examples/cms/md/module/system/health/api.dever.md"),
    );
    let response = cli(
        &compiler,
        markdown.path(),
        "run",
        &["--", "system.health.ping", "{}"],
    );
    success(&response);
    assert_eq!(
        serde_json::from_slice::<Value>(&response.stdout).unwrap()["data"]["status"],
        "ready"
    );
    assert_eq!(cached_programs(root.path()).len(), 4);
    let object = pack::pack_root(root.path()).join("crt1.o");
    let mut corrupt = fs::read(&object).unwrap();
    corrupt[16] ^= 1;
    // Break this fixture's hard link before mutation; the host CRT is immutable.
    fs::remove_file(&object).unwrap();
    fs::write(&object, corrupt).unwrap();
    failure(
        &cli(&compiler, project_dir.path(), "run", &args),
        "SHA-256 verification",
    );
    assert_eq!(cached_programs(root.path()).len(), 4);
}

#[test]
#[ignore = "requires explicitly prepared runtime archive and component-fixture; no downloads or language SDK"]
fn unsigned_llvm_run_and_build_reject_exec_workers() {
    let root = TemporaryDirectory::new();
    let compiler = compiler(root.path(), "dever");
    pack::manifest(root.path(), true);
    let project_dir = TemporaryDirectory::new();
    let directory = project_dir.path();
    write(
        directory,
        "module/notification/delivery/app.dever",
        "read() (value: Int) { value = port.read() }",
    );
    write(
        directory,
        "module/notification/delivery/port.dever",
        "read() (value: Int) fails dever.time.SleepResult",
    );
    write(
        directory,
        "module/notification/delivery/adapter.dever",
        "setting { mode: Text }\nexternal exec \"worker\" {}",
    );
    write(
        directory,
        "module/notification/delivery/api.dever",
        "cmd read = app.read",
    );
    let settings = r#"{"adapter":{"notification.delivery":{"setting":{"mode":"embedded"}}}}"#;
    write(directory, "config/setting.json", settings);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/native-release-inputs/component-fixture");
    assert!(
        fixture.is_file(),
        "prepare the immutable component-fixture in target/native-release-inputs"
    );
    fs::copy(
        fixture,
        directory.join("module/notification/delivery/worker"),
    )
    .unwrap();
    let args = ["--", "notification.delivery.read", "{}"];
    let run = cli(&compiler, directory, "run", &args);
    let binary = directory.join("packaged");
    let build = cli(
        &compiler,
        directory,
        "build",
        &["--output", binary.to_str().unwrap()],
    );
    for rejected in [run, build] {
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        assert_eq!(
            String::from_utf8(rejected.stderr).unwrap().trim(),
            "real Lib providers require a signed managed Dever release"
        );
    }
    assert!(!binary.exists());
    assert!(!root.path().join("cache").exists());
}

#[test]
#[ignore = "requires explicitly prepared full runtime archive; isolated SQLite and case-local fake processes"]
fn llvm_test_preserves_case_isolation_fakes_and_failure_continuation() {
    let root = TemporaryDirectory::new();
    let compiler = compiler(root.path(), "dever");
    pack::manifest(root.path(), true);
    let project_dir = TemporaryDirectory::new();
    let directory = project_dir.path();
    write(
        directory,
        "module/sample/account/model.dever",
        "type Account { name: Text }",
    );
    write(
        directory,
        "module/sample/account/app.dever",
        "create(name: Text) (count: Int) { row = model.create({ name = name })\n  count = model.count() }",
    );
    write(
        directory,
        "module/notification/mail/app.dever",
        "read() (value: Int) { value = port.read() }",
    );
    write(
        directory,
        "module/notification/mail/port.dever",
        "read() (value: Int) fails dever.time.SleepResult",
    );
    write(
        directory,
        "module/notification/mail/adapter.dever",
        "external exec \"worker/must-not-start\" {}",
    );
    write(
        directory,
        "config/setting.json",
        r#"{"database":{"default":{"type":"sqlite","path":"deployment.db"}}}"#,
    );
    // The CLI reads project dependency metadata from valid JSON, but test
    // execution must use its generated database, never this unusable directory.
    fs::create_dir(directory.join("deployment.db")).unwrap();
    for name in ["first", "second"] {
        write(
            directory,
            &format!("test/sample/account/{name}.dever"),
            &format!("{name}() () {{ assert_eq(app.create(\"Ada\"), 1) }}"),
        );
    }
    write(
        directory,
        "test/notification/mail/blocking.dever",
        "blocking() () { dever.time.sleep(1)\n  assert_eq(app.read(), 11) }\nport.read() (value: Int) { value = 11 }",
    );
    write(
        directory,
        "test/notification/mail/suspending.dever",
        "suspending() () { assert_eq(app.read(), 23) }\nport.read() (value: Int) { dever.task.sleep(1)\n  value = 23 }",
    );
    write(
        directory,
        "test/sample/account/failure.dever",
        "failure() () { assert_eq(1, 2) }",
    );
    write(
        directory,
        "test/sample/account/business.dever",
        "type Outcome { error Denied(message: Text) }\nbusiness() () { fail(Outcome.Denied(\"test business denial\")) }",
    );
    let result = cli(&compiler, directory, "test", &[]);
    failure(&result, "4 passed; 2 failed; 0 not run");
    let report = String::from_utf8_lossy(&result.stdout);
    assert!(
        report.contains("test sample/account/second ... ok"),
        "{report}"
    );
    assert!(report.contains("failure.dever"), "{report}");
    assert!(
        report.contains("assert_eq failed: actual = 1, expected = 2"),
        "{report}"
    );
    assert!(report.contains("test business denial"), "{report}");
    assert!(!directory.join("data").exists());
    assert_eq!(cached_programs(root.path()).len(), 1);
    let suite = &cached_programs(root.path())[0];
    for args in [
        vec![],
        vec![""],
        vec!["-1"],
        vec!["+0"],
        vec![" 0"],
        vec!["0 "],
        vec!["18446744073709551616"],
        vec!["6"],
        vec!["0", "extra"],
    ] {
        let rejected = output(Command::new(suite).env_clear().args(args));
        failure(&rejected, "test program requires a valid case index");
        assert_eq!(
            rejected.stderr,
            b"test program requires a valid case index\n"
        );
    }
}
