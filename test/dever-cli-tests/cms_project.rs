use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[path = "../dever-tests/tests/support/process.rs"]
pub mod process;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "support/llvm_pack.rs"]
pub mod llvm_pack;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TemporaryProject(PathBuf);

impl TemporaryProject {
    fn new() -> Self {
        let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("dever-cms-project-{}-{suffix}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn copy_from(source: &Path) -> Self {
        let project = Self::new();
        copy_directory(&source.join("config"), &project.path().join("config"));
        copy_directory(&source.join("module"), &project.path().join("module"));
        let tests = source.join("test");
        if tests.exists() {
            copy_directory(&tests, &project.path().join("test"));
        }
        fs::create_dir(project.path().join("data")).unwrap();
        project
    }

    fn package(source: &Path, executable: &Path) -> Self {
        let project = Self::new();
        copy_directory(&source.join("config"), &project.path().join("config"));
        fs::copy(executable, project.path().join("cms-app")).unwrap();
        project
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn block_deployment_database(&self) {
        // Package metadata needs valid JSON. A directory as the configured
        // database proves tests use their own SQLite configuration instead.
        fs::create_dir(self.path().join("data/deployment.db")).unwrap();
        fs::write(
            self.path().join("config/setting.json"),
            r#"{"database":{"default":{"type":"sqlite","path":"data/deployment.db"}}}"#,
        )
        .unwrap();
    }
}

impl Drop for TemporaryProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_directory(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source = entry.path();
        let target = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&source, &target);
        } else {
            fs::copy(source, target).unwrap();
        }
    }
}

fn dever(project: &Path, action: &str, arguments: &[&str]) -> Output {
    dever_with_pid(project, action, arguments).1
}

fn dever_with_pid(project: &Path, action: &str, arguments: &[&str]) -> (u32, Output) {
    output_with_pid(
        Command::new(env!("CARGO_BIN_EXE_dever"))
            .arg(action)
            .arg(project)
            .args(arguments),
    )
}

fn output_with_pid(command: &mut Command) -> (u32, Output) {
    let capture = TemporaryProject::new();
    let stdout = capture.path().join("stdout");
    let stderr = capture.path().join("stderr");
    let mut child = command
        .env_clear()
        .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
        .stderr(Stdio::from(fs::File::create(&stderr).unwrap()))
        .spawn()
        .unwrap();
    let pid = child.id();
    let status = process::wait(&mut child, Duration::from_secs(180)).unwrap();
    (
        pid,
        Output {
            status,
            stdout: fs::read(stdout).unwrap(),
            stderr: fs::read(stderr).unwrap(),
        },
    )
}

fn assert_test_artifacts_removed(pid: u32) {
    let test_prefix = format!("dever-application-test-{pid}-");
    let build_prefix = format!("dever-build-{pid}-");
    let leftovers = fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&test_prefix) || name.starts_with(&build_prefix))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "temporary test artifacts remain: {leftovers:?}"
    );
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn clean_removes_only_stale_owned_dever_artifacts() {
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::time::{SystemTime, UNIX_EPOCH};

    let project = TemporaryProject::new();
    let pid = u64::from(std::process::id()) + 1_000_000;
    let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let stale = std::env::temp_dir().join(format!("dever-build-{pid}-{suffix}"));
    fs::create_dir(&stale).unwrap();
    let owner = fs::metadata(&stale).unwrap().uid();
    fs::write(
        stale.join(".dever-owner"),
        format!(
            "dever-build-owner-v1\ncreated=1\npid={pid}\nowner={owner}\ncompiler={}\n",
            env!("CARGO_PKG_VERSION"),
        ),
    )
    .unwrap();
    fs::write(stale.join("program"), b"stale").unwrap();

    let fresh_pid = pid + 1;
    let fresh = std::env::temp_dir().join(format!("dever-build-{fresh_pid}-{suffix}"));
    fs::create_dir(&fresh).unwrap();
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    fs::write(
        fresh.join(".dever-owner"),
        format!(
            "dever-build-owner-v1\ncreated={created}\npid={fresh_pid}\nowner={owner}\ncompiler={}\n",
            env!("CARGO_PKG_VERSION"),
        ),
    ).unwrap();
    let unknown = std::env::temp_dir().join(format!("dever-build-{}-{suffix}", pid + 2));
    fs::create_dir(&unknown).unwrap();
    let link = std::env::temp_dir().join(format!("dever-build-{}-{suffix}", pid + 3));
    symlink(&unknown, &link).unwrap();

    let mut active_process = Command::new("/bin/sleep").arg("60").spawn().unwrap();
    let active_pid = u64::from(active_process.id());
    let active = std::env::temp_dir().join(format!("dever-build-{active_pid}-{suffix}"));
    fs::create_dir(&active).unwrap();
    fs::write(
        active.join(".dever-owner"),
        format!(
            "dever-build-owner-v1\ncreated=1\npid={active_pid}\nowner={owner}\ncompiler={}\n",
            env!("CARGO_PKG_VERSION"),
        ),
    )
    .unwrap();
    fs::write(active.join("program"), b"active").unwrap();

    let legacy = project.path().join(format!(".dever-run-{pid}"));
    fs::write(&legacy, b"stale legacy").unwrap();
    let legacy_file = fs::OpenOptions::new().write(true).open(&legacy).unwrap();
    legacy_file
        .set_times(
            fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1)),
        )
        .unwrap();
    let fresh_legacy = project.path().join(format!(".dever-run-{}", pid + 1));
    fs::write(&fresh_legacy, b"fresh legacy").unwrap();
    let active_legacy = project.path().join(format!(".dever-run-{active_pid}"));
    fs::write(&active_legacy, b"active legacy").unwrap();
    let active_legacy_file = fs::OpenOptions::new()
        .write(true)
        .open(&active_legacy)
        .unwrap();
    active_legacy_file
        .set_times(
            fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1)),
        )
        .unwrap();
    let explicit = project.path().join("application");
    fs::write(&explicit, b"explicit output").unwrap();

    let output = dever(project.path(), "clean", &[]);
    active_process.kill().unwrap();
    active_process.wait().unwrap();
    assert_success(&output);
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("cleaned 1 directories, 3 files, ")
    );
    assert!(!stale.exists());
    assert!(fresh.exists());
    assert!(unknown.exists());
    assert!(link.symlink_metadata().is_ok());
    assert!(active.exists());
    assert!(!legacy.exists());
    assert!(fresh_legacy.exists());
    assert!(active_legacy.exists());
    assert!(explicit.exists());

    fs::remove_dir_all(fresh).unwrap();
    fs::remove_file(link).unwrap();
    fs::remove_dir_all(unknown).unwrap();
    fs::remove_dir_all(active).unwrap();
}

#[test]
fn cms_project_checks_runs_and_builds_from_an_isolated_copy() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for format in ["dever", "md"] {
        let project = TemporaryProject::copy_from(&workspace.join("examples/cms").join(format));
        let settings_path = project.path().join("config/setting.json");
        let production_settings = fs::read(&settings_path).unwrap();
        project.block_deployment_database();
        for _ in 0..2 {
            let (pid, output) = dever_with_pid(project.path(), "test", &[]);
            assert_cms_tests(&output);
            assert_test_artifacts_removed(pid);
        }
        fs::write(&settings_path, production_settings).unwrap();
        fs::write(project.path().join("test/ignored.dever"), "not valid Dever").unwrap();

        assert_success(&dever(project.path(), "check", &[]));
        for _ in 0..2 {
            assert_health_stdout(&dever(
                project.path(),
                "run",
                &["--", "system.health.ping", "{}"],
            ));
        }

        let executable = build(project.path(), "cms-app");
        let package = TemporaryProject::package(project.path(), &executable);
        assert!(!package.path().join("module").exists());
        for _ in 0..2 {
            let output = output_with_pid(
                Command::new(package.path().join("cms-app"))
                    .args(["system.health.ping", "{}"])
                    .current_dir(package.path()),
            )
            .1;
            assert_health_stdout(&output);
        }
        assert!(fs::read_dir(project.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".dever-run-")
        }));
    }
}

#[test]
fn project_without_tests_reports_zero_and_succeeds() {
    let project = TemporaryProject::new();
    fs::create_dir(project.path().join("module")).unwrap();
    fs::write(project.path().join("module/main.dever"), "main() () {}").unwrap();

    let (pid, output) = dever_with_pid(project.path(), "test", &[]);
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 not run\n"
    );
    assert_test_artifacts_removed(pid);
}

#[test]
fn mixed_suite_initializes_database_only_for_database_cases() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = TemporaryProject::copy_from(&workspace.join("examples/cms/dever"));
    project.block_deployment_database();
    let health = project.path().join("test/system/health");
    fs::create_dir_all(&health).unwrap();
    fs::write(
        health.join("ping.dever"),
        "ping() () { assert_eq(\"ready\", \"ready\") }\n",
    )
    .unwrap();

    let (pid, output) = dever_with_pid(project.path(), "test", &[]);
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "running 6 tests\ntest content/category/lifecycle ... ok\ntest news/article/publish ... ok\ntest system/health/ping ... ok\ntest user/account/bootstrap ... ok\ntest user/account/validation ... ok\ntest user/session/lifecycle ... ok\n\ntest result: ok. 6 passed; 0 failed; 0 not run\n"
    );
    assert_test_artifacts_removed(pid);
}

#[test]
fn failed_test_reports_captured_output_and_does_not_stop_later_tests() {
    let project = TemporaryProject::new();
    fs::create_dir(project.path().join("module")).unwrap();
    fs::create_dir_all(project.path().join("test/sample/case")).unwrap();
    fs::write(project.path().join("module/main.dever"), "main() () {}").unwrap();
    fs::write(
        project.path().join("test/sample/case/fails.dever"),
        r#"fails() () {
  blocking(dever.io.println("failed output"))
  assert_eq(1, 2)
}
"#,
    )
    .unwrap();
    fs::write(
        project.path().join("test/sample/case/passes.dever"),
        r#"passes() () {
  blocking(dever.io.println("hidden successful output"))
  assert(true)
}
"#,
    )
    .unwrap();

    let (pid, output) = dever_with_pid(project.path(), "test", &[]);
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let failed = stdout.find("test sample/case/fails ... FAILED").unwrap();
    let passed = stdout.find("test sample/case/passes ... ok").unwrap();
    assert!(failed < passed, "{stdout}");
    assert!(
        stdout.contains("---- stdout ----\nfailed output\n"),
        "{stdout}"
    );
    assert!(stdout.contains("---- stderr ----"), "{stdout}");
    assert!(
        stdout.contains("assert_eq failed: actual = 1, expected = 2"),
        "{stdout}"
    );
    assert!(!stdout.contains("hidden successful output"), "{stdout}");
    assert!(
        stdout.ends_with("test result: FAILED. 1 passed; 1 failed; 0 not run\n"),
        "{stdout}"
    );
    assert_test_artifacts_removed(pid);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn suite_compilation_failure_marks_every_discovered_case_failed() {
    let project = TemporaryProject::new();
    fs::create_dir(project.path().join("module")).unwrap();
    fs::create_dir_all(project.path().join("test/sample/case")).unwrap();
    fs::write(project.path().join("module/main.dever"), "main() () {}").unwrap();
    fs::write(
        project.path().join("test/sample/case/first.dever"),
        "first() () { assert(true) }\n",
    )
    .unwrap();
    fs::write(
        project.path().join("test/sample/case/second.dever"),
        "second() () { assert(true) }\n",
    )
    .unwrap();

    let toolchain = TemporaryProject::new();
    let compiler = llvm_pack::compiler(
        Path::new(env!("CARGO_BIN_EXE_dever")),
        toolchain.path(),
        "dever",
    );
    let (pid, output) = output_with_pid(Command::new(compiler).arg("test").arg(project.path()));
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("test sample/case/first ... FAILED"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test sample/case/second ... FAILED"),
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("cannot inspect runtime pack").count(),
        2,
        "{stdout}"
    );
    assert!(
        stdout.ends_with("test result: FAILED. 0 passed; 2 failed; 0 not run\n"),
        "{stdout}"
    );
    assert_test_artifacts_removed(pid);
    assert!(!toolchain.path().join("cache").exists());
}

fn build(project: &Path, name: &str) -> PathBuf {
    let executable = project.join(name);
    let build = dever(
        project,
        "build",
        &["--output", executable.to_str().unwrap()],
    );
    assert_success(&build);
    assert!(executable.is_file());
    executable
}

fn assert_health_stdout(output: &Output) {
    assert_success(output);
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["code"], 0);
    assert_eq!(body["message"], "ok");
    assert_eq!(body["data"]["status"], "ready");
}

fn assert_cms_tests(output: &Output) {
    assert_success(output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "running 5 tests\ntest content/category/lifecycle ... ok\ntest news/article/publish ... ok\ntest user/account/bootstrap ... ok\ntest user/account/validation ... ok\ntest user/session/lifecycle ... ok\n\ntest result: ok. 5 passed; 0 failed; 0 not run\n"
    );
}
