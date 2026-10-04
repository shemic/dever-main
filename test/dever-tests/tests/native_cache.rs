mod support;

#[path = "../../../crates/dever-core/src/native/build/cache.rs"]
#[allow(dead_code)]
mod cache;
#[path = "../../../crates/dever-cli/src/api.rs"]
mod cli_api;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use dever_core::native::{NativeProgram, compile_with_cache};
use support::temp::TemporaryDirectory;

fn rustc() -> OsString {
    std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into())
}

fn compile(source: &str, root: &Path, compiler: &OsStr) -> NativeProgram {
    let sources = support::sources(source);
    compile_with_cache(
        &support::checked(&sources),
        &sources,
        "main.main",
        compiler,
        root,
    )
    .unwrap()
}

const HELLO: &str = "main() () { dever.io.println(\"cache hello\") }\n";

#[test]
fn identified_artifacts_share_owned_cache_and_reject_modified_bytes() {
    use dever_core::native::compile_artifact;

    let directory = TemporaryDirectory::new();
    let root = directory.path().join("cache");
    let first = compile_artifact(&root, "llvm-source-and-pack-1", |_, output| {
        fs::write(output, b"complete artifact").map_err(|error| error.to_string())
    })
    .unwrap();
    assert!(!first.cache_hit());
    let second = compile_artifact(&root, "llvm-source-and-pack-1", |_, _| {
        panic!("an exact cache hit must not invoke the compiler")
    })
    .unwrap();
    assert!(second.cache_hit());
    assert_ne!(first.executable(), second.executable());
    let output = directory.path().join("published");
    second.save(&output).unwrap();
    assert_eq!(
        second.save(&output).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    let temporary = first.executable().parent().unwrap().to_path_buf();
    drop(first);
    assert!(!temporary.exists());
    assert_eq!(fs::read(&output).unwrap(), b"complete artifact");

    let stored = fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
    fs::write(stored.join("program"), b"tampered").unwrap();
    let result = compile_artifact(&root, "llvm-source-and-pack-1", |_, _| {
        panic!("corruption must fail before compilation")
    });
    assert!(result.err().unwrap().contains("incomplete or modified"));
    let changed = compile_artifact(&root, "llvm-source-and-pack-2", |_, output| {
        fs::write(output, b"changed input").map_err(|error| error.to_string())
    })
    .unwrap();
    assert!(!changed.cache_hit());
}

#[test]
fn identified_artifact_failure_releases_directory_without_publication() {
    use dever_core::native::compile_artifact;

    let directory = TemporaryDirectory::new();
    let root = directory.path().join("cache");
    let mut temporary = None;
    let result = compile_artifact(&root, "failed-link", |build, output| {
        temporary = Some(build.to_path_buf());
        fs::write(output, b"incomplete").unwrap();
        Err("link rejected".into())
    });
    assert_eq!(result.err().unwrap(), "link rejected");
    assert!(!temporary.unwrap().exists());
    assert!(!root.exists());
}

#[test]
fn workspace_profiles_keep_independent_tests_without_debug_or_incremental_artifacts() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cargo = fs::read_to_string(workspace.join("Cargo.toml")).unwrap();
    for profile in ["dev", "test"] {
        let section = cargo
            .split(&format!("[profile.{profile}]"))
            .nth(1)
            .unwrap_or_else(|| panic!("missing profile.{profile}"))
            .split("\n[")
            .next()
            .unwrap();
        assert!(
            section.contains("debug = 0"),
            "profile.{profile} enables debuginfo"
        );
        assert!(
            section.contains("incremental = false"),
            "profile.{profile} enables incremental"
        );
    }
    let release = cargo
        .split("[profile.release]")
        .nth(1)
        .unwrap()
        .split("\n[")
        .next()
        .unwrap();
    assert!(release.contains("codegen-units = 1"));
    assert!(release.contains("lto = \"fat\""));

    let tests = fs::read_to_string(workspace.join("test/dever-tests/Cargo.toml")).unwrap();
    for target in [
        "native_cache",
        "api_routes",
        "sqlite_orm",
        "postgres_orm",
        "external_component",
    ] {
        assert!(
            tests.contains(&format!("name = \"{target}\"")),
            "missing test target {target}"
        );
    }
    assert!(
        tests.matches("[[test]]").count() >= 60,
        "test targets were unexpectedly merged"
    );
}

#[test]
fn repeated_build_reuses_native_code_and_keeps_owned_output_semantics() {
    let directory = TemporaryDirectory::new();
    let root = directory.path().join("cache");
    let started = Instant::now();
    let first = compile(HELLO, &root, &rustc());
    let first_time = started.elapsed();
    assert!(!first.cache_hit());
    let started = Instant::now();
    let second = compile(HELLO, &root, &rustc());
    eprintln!(
        "native cache: cold_ms={}, hit_ms={}",
        first_time.as_millis(),
        started.elapsed().as_millis()
    );
    assert!(second.cache_hit());
    assert_ne!(first.executable(), second.executable());
    assert_eq!(
        Command::new(second.executable()).output().unwrap().stdout,
        b"cache hello\n"
    );
    let output = directory.path().join("saved");
    second.save(&output).unwrap();
    let original = fs::read(&output).unwrap();
    assert_eq!(
        second.save(&output).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&output).unwrap(), original);
    let temporary = second.executable().parent().unwrap().to_owned();
    drop(second);
    assert!(!temporary.exists());
    for entry in fs::read_dir(&root).unwrap() {
        let mut files: Vec<_> = fs::read_dir(entry.unwrap().path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        files.sort();
        assert_eq!(files, ["executable-fingerprint", "identity", "program"]);
    }
    let modified = HELLO.replace("cache hello", "changed source");
    let changed = compile(&modified, &root, &rustc());
    assert!(!changed.cache_hit());
    assert_eq!(
        Command::new(changed.executable()).output().unwrap().stdout,
        b"changed source\n"
    );
}

#[test]
fn modified_cached_executables_are_never_run() {
    let directory = TemporaryDirectory::new();
    let root = directory.path().join("cache");
    let native = compile(HELLO, &root, &rustc());
    let entry = fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
    fs::write(entry.join("program"), "incomplete executable").unwrap();
    let sources = support::sources(HELLO);
    let result = compile_with_cache(
        &support::checked(&sources),
        &sources,
        "main.main",
        &rustc(),
        &root,
    );
    let Err(error) = result else {
        panic!("modified executable must not be reused")
    };
    assert!(error.contains("incomplete or modified"), "{error}");
    assert_eq!(
        Command::new(native.executable()).output().unwrap().stdout,
        b"cache hello\n"
    );
}

#[test]
fn concurrent_publish_never_exposes_partial_artifacts() {
    let directory = TemporaryDirectory::new();
    let root = directory.path().join("cache");
    let natives = std::thread::scope(|scope| {
        let first = scope.spawn(|| compile(HELLO, &root, &rustc()));
        let second = scope.spawn(|| compile(HELLO, &root, &rustc()));
        [first.join().unwrap(), second.join().unwrap()]
    });
    for native in natives {
        assert_eq!(
            Command::new(native.executable()).output().unwrap().stdout,
            b"cache hello\n"
        );
    }
    assert!(compile(HELLO, &root, &rustc()).cache_hit());
    assert_eq!(fs::read_dir(root).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn changing_compiler_bytes_invalidates_even_when_version_is_unchanged() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = TemporaryDirectory::new();
    let root = directory.path().join("cache");
    let sysroot = Command::new(rustc())
        .args(["--print", "sysroot"])
        .output()
        .unwrap();
    assert!(sysroot.status.success());
    let bin = Path::new(std::str::from_utf8(&sysroot.stdout).unwrap().trim()).join("bin");
    let wrapper = directory.path().join("rustc");
    let quoted = bin
        .join("rustc")
        .display()
        .to_string()
        .replace('\'', "'\\''");
    let script = format!("#!/bin/sh\nexec '{quoted}' \"$@\"\n");
    fs::write(&wrapper, &script).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(bin.join("cargo"), directory.path().join("cargo")).unwrap();
    assert!(!compile(HELLO, &root, wrapper.as_os_str()).cache_hit());
    assert!(compile(HELLO, &root, wrapper.as_os_str()).cache_hit());
    fs::write(&wrapper, format!("{script}# changed wrapper\n")).unwrap();
    assert!(!compile(HELLO, &root, wrapper.as_os_str()).cache_hit());
}

#[test]
fn runtime_dependencies_toolchain_version_and_options_are_cache_inputs() {
    let directory = TemporaryDirectory::new();
    let runtime = directory.path().join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::create_dir(runtime.join("deps")).unwrap();
    fs::write(runtime.join("libdever_runtime.rlib"), "runtime version one").unwrap();
    fs::write(
        runtime.join("deps/libdependency.rlib"),
        "dependency version one",
    )
    .unwrap();
    let root = directory.path().join("cache");
    fs::create_dir(&root).unwrap();
    let program = directory.path().join("program");
    fs::write(&program, "test executable bytes").unwrap();
    let inputs = cache::RuntimeInputs::read(&runtime).unwrap();
    let memoized_inputs = cache::RuntimeInputs::read(&runtime).unwrap();
    let original = cache::Cache::new(
        &root,
        "generated",
        &rustc(),
        b"version one",
        &inputs,
        &["opt-level=3"],
    )
    .unwrap();
    let staging = directory.path().join("staging");
    fs::create_dir(&staging).unwrap();
    original.publish(&program, &staging).unwrap();
    assert!(
        original
            .restore(&directory.path().join("restored"))
            .unwrap()
    );
    let memoized = cache::Cache::new(
        &root,
        "generated",
        &rustc(),
        b"version one",
        &memoized_inputs,
        &["opt-level=3"],
    )
    .unwrap();
    assert!(
        memoized
            .restore(&directory.path().join("memoized"))
            .unwrap()
    );
    for (version, options) in [
        (b"version two".as_slice(), "opt-level=3"),
        (b"version one".as_slice(), "opt-level=2"),
    ] {
        let changed =
            cache::Cache::new(&root, "generated", &rustc(), version, &inputs, &[options]).unwrap();
        assert!(!changed.restore(&directory.path().join("absent")).unwrap());
    }
    for path in [
        runtime.join("libdever_runtime.rlib"),
        runtime.join("deps/libdependency.rlib"),
    ] {
        fs::write(path, "changed bytes").unwrap();
        let changed_inputs = cache::RuntimeInputs::read(&runtime).unwrap();
        let changed = cache::Cache::new(
            &root,
            "generated",
            &rustc(),
            b"version one",
            &changed_inputs,
            &["opt-level=3"],
        )
        .unwrap();
        assert!(!changed.restore(&directory.path().join("absent")).unwrap());
    }
    let snapshot = directory.path().join("snapshot");
    inputs.write(&snapshot).unwrap();
    assert_eq!(
        fs::read_to_string(snapshot.join("libdever_runtime.rlib")).unwrap(),
        "runtime version one"
    );
    assert_eq!(
        fs::read_dir(&runtime)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".dever-runtime-identity-")
            })
            .count(),
        1
    );
}

#[test]
fn runtime_input_views_are_exact_reused_and_concurrently_published() {
    let directory = TemporaryDirectory::new();
    let source = directory.path().join("cargo/release/deps");
    fs::create_dir_all(&source).unwrap();
    let runtime = directory.path().join("cargo/release/libdever_runtime.rlib");
    fs::write(&runtime, "runtime one").unwrap();
    let first_dependency = source.join("libfirst-1.rlib");
    let second_dependency = source.join("libsecond-2.rlib");
    fs::write(&first_dependency, "first").unwrap();
    fs::write(&second_dependency, "second").unwrap();
    let inputs = directory.path().join("inputs");

    let first =
        cache::RuntimeInputs::publish(&inputs, &runtime, std::slice::from_ref(&first_dependency))
            .unwrap();
    fs::remove_file(&runtime).unwrap();
    fs::write(&runtime, "runtime two").unwrap();
    let second = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            cache::RuntimeInputs::publish(
                &inputs,
                &runtime,
                std::slice::from_ref(&second_dependency),
            )
            .unwrap()
        });
        let right = scope.spawn(|| {
            cache::RuntimeInputs::publish(
                &inputs,
                &runtime,
                std::slice::from_ref(&second_dependency),
            )
            .unwrap()
        });
        [left.join().unwrap(), right.join().unwrap()]
    });
    assert_eq!(fs::read_dir(&inputs).unwrap().count(), 2);

    let first_output = directory.path().join("first-output");
    first.write(&first_output).unwrap();
    assert!(first_output.join("deps/libfirst-1.rlib").is_file());
    assert!(!first_output.join("deps/libsecond-2.rlib").exists());
    for (index, input) in second.into_iter().enumerate() {
        let output = directory.path().join(format!("second-output-{index}"));
        input.write(&output).unwrap();
        assert_eq!(
            fs::read_to_string(output.join("libdever_runtime.rlib")).unwrap(),
            "runtime two"
        );
        assert!(output.join("deps/libsecond-2.rlib").is_file());
        assert!(!output.join("deps/libfirst-1.rlib").exists());
    }
}

#[cfg(unix)]
#[test]
fn runtime_input_views_reject_unknown_files_and_symbolic_links() {
    use std::os::unix::fs::symlink;

    let directory = TemporaryDirectory::new();
    let runtime = directory.path().join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::create_dir(runtime.join("deps")).unwrap();
    fs::write(runtime.join("libdever_runtime.rlib"), "runtime").unwrap();
    fs::write(runtime.join("deps/libdependency.rlib"), "dependency").unwrap();

    fs::write(runtime.join("deps/unknown.txt"), "unknown").unwrap();
    assert!(cache::RuntimeInputs::read(&runtime).is_err());
    fs::remove_file(runtime.join("deps/unknown.txt")).unwrap();

    fs::remove_file(runtime.join("deps/libdependency.rlib")).unwrap();
    let outside = directory.path().join("outside.rlib");
    fs::write(&outside, "outside").unwrap();
    symlink(&outside, runtime.join("deps/libdependency.rlib")).unwrap();
    assert!(cache::RuntimeInputs::read(&runtime).is_err());
}

#[test]
fn checked_program_cache_preserves_source_identity_and_rechecks_api_baselines() {
    let original = support::sources(HELLO);
    let started = Instant::now();
    let program = support::checked(&original);
    let cold = started.elapsed();
    let started = Instant::now();
    let cached = support::checked(&original);
    eprintln!(
        "frontend cache: cold_us={}, hit_us={}",
        cold.as_micros(),
        started.elapsed().as_micros()
    );
    assert_eq!(program.api_snapshot(), cached.api_snapshot());
    let directory = TemporaryDirectory::new();
    let baseline = directory.path().join("dever.api");
    cli_api::write(&program, Some(&baseline)).unwrap();
    cli_api::check_baseline(&cached, directory.path()).unwrap();
    fs::write(&baseline, "changed baseline").unwrap();
    assert!(cli_api::check_baseline(&support::checked(&original), directory.path()).is_err());

    let mut wrong_path = dever_core::source::SourceMap::default();
    wrong_path.add("wrong.dever", HELLO);
    let wrong_program = support::checked(&wrong_path);
    assert!(dever_core::native::emit(&wrong_program, &wrong_path, "main.main").is_err());
    let changed = support::sources(&HELLO.replace("dever.io.println", "unknown"));
    let errors = dever_core::check(&changed).unwrap_err();
    let repeated_errors = dever_core::check(&changed).unwrap_err();
    assert_eq!(
        errors[0].render(&changed),
        repeated_errors[0].render(&changed)
    );
    assert!(errors.iter().any(|error| error.code == "C004"));

    let mut additional = dever_core::source::SourceMap::default();
    additional.add("main.dever", HELLO);
    additional.add("user/account/app.dever", "extra() () {}\n");
    fs::write(&baseline, program.api_snapshot()).unwrap();
    assert!(cli_api::check_baseline(&support::checked(&additional), directory.path()).is_err());
}

#[test]
#[ignore = "requires a freshly built local dever-cli binary"]
fn actual_cli_checks_baseline_on_check_run_and_cached_build() {
    let compiler = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("dever{}", std::env::consts::EXE_SUFFIX));
    assert!(
        compiler.is_file(),
        "build dever-cli before running this test"
    );
    let directory = TemporaryDirectory::new();
    let domain = directory.path().join("module/sample/cache");
    fs::create_dir_all(&domain).unwrap();
    fs::create_dir(directory.path().join("config")).unwrap();
    fs::write(
        domain.join("app.dever"),
        "hello() (message: Text) { message = \"cache hello\" }\n",
    )
    .unwrap();
    fs::write(domain.join("api.dever"), "cmd hello = app.hello\n").unwrap();
    fs::write(directory.path().join("config/setting.json"), "{}\n").unwrap();
    let baseline = directory.path().join("dever.api");
    let api = Command::new(&compiler)
        .arg("api")
        .arg(directory.path())
        .arg("--output")
        .arg(&baseline)
        .output()
        .unwrap();
    assert!(
        api.status.success(),
        "{}",
        String::from_utf8_lossy(&api.stderr)
    );
    let check = Command::new(&compiler)
        .arg("check")
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let run = Command::new(&compiler)
        .arg("run")
        .arg(directory.path())
        .args(["--", "sample.cache.hello", "{}"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let expected = serde_json::json!({"code": 0, "message": "ok", "data": "cache hello"});
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&run.stdout).unwrap(),
        expected
    );
    let saved = directory.path().join("saved");
    let build = Command::new(&compiler)
        .arg("build")
        .arg(directory.path())
        .arg("--output")
        .arg(&saved)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let standalone = Command::new(saved)
        .current_dir(directory.path())
        .args(["sample.cache.hello", "{}"])
        .output()
        .unwrap();
    assert!(
        standalone.status.success(),
        "{}",
        String::from_utf8_lossy(&standalone.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&standalone.stdout).unwrap(),
        expected
    );
    fs::write(baseline, "changed baseline").unwrap();
    let blocked_output = directory.path().join("blocked");
    for action in ["check", "run", "build"] {
        let mut command = Command::new(&compiler);
        command.arg(action).arg(directory.path());
        if action == "build" {
            command.arg("--output").arg(&blocked_output);
        } else if action == "run" {
            command.args(["--", "sample.cache.hello", "{}"]);
        }
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "{action} skipped changed baseline"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("API baseline"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
    assert!(!blocked_output.exists());
}
