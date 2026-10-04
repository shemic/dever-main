use crate::{process, temp};
use dever_backend_bridge::{BinaryResource, Target, emit_object_with_resources};
use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// Kernel and application probes share the real archive and atomic ledger.
/// Deployment settings belong only to this owned test directory.
pub fn execute(
    ir: &str,
    fixture: &str,
    iterations: usize,
    timeout: Duration,
    arguments: &[&str],
    setting: Option<&str>,
) -> Output {
    let directory = temp::TemporaryDirectory::new();
    if let Some(setting) = setting {
        fs::create_dir(directory.path().join("config")).unwrap();
        fs::write(directory.path().join("config/setting.json"), setting).unwrap();
    }
    execute_in(&directory, ir, fixture, iterations, timeout, arguments)
}

/// Successive schema revisions share only this owned executable/data directory.
pub fn execute_in(
    directory: &temp::TemporaryDirectory,
    ir: &str,
    fixture: &str,
    iterations: usize,
    timeout: Duration,
    arguments: &[&str],
) -> Output {
    let program = link_in(directory, ir, fixture, iterations);
    execute_program(&program, fixture, timeout, arguments)
}

/// Relocated executables use the same empty environment and bounded process owner.
pub fn execute_program(
    program: &Path,
    fixture: &str,
    timeout: Duration,
    arguments: &[&str],
) -> Output {
    let directory = program.parent().expect("fixture executable directory");
    let stdout = directory.join("stdout");
    let stderr = directory.join("stderr");
    let status = process::status(
        Command::new(program)
            .env_clear()
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap()),
        timeout,
    )
    .unwrap_or_else(|error| {
        let completed = fs::read_to_string(&stdout)
            .unwrap_or_default()
            .lines()
            .count();
        panic!("{fixture}: {error}; completed output lines: {completed}");
    });
    let stdout = fs::read(stdout).unwrap();
    let stderr = fs::read(stderr).unwrap();
    assert!(
        status.success(),
        "LLVM fixture failed: {status}\n{}\n{fixture}",
        String::from_utf8_lossy(&stderr)
    );
    Output {
        status,
        stdout,
        stderr,
    }
}

/// HTTP probes control their own child after linking the same ledger driver.
pub fn link_in(
    directory: &temp::TemporaryDirectory,
    ir: &str,
    fixture: &str,
    iterations: usize,
) -> std::path::PathBuf {
    link_with_driver(directory, ir, fixture, iterations, "managed-driver.c")
}

/// Suite dispatch needs a case-index driver; object/archive/ledger linkage stays shared.
pub fn link_with_driver(
    directory: &temp::TemporaryDirectory,
    ir: &str,
    fixture: &str,
    iterations: usize,
    driver: &str,
) -> std::path::PathBuf {
    link_with_resources(directory, ir, fixture, iterations, driver, &[])
}

pub fn link_with_resources(
    directory: &temp::TemporaryDirectory,
    ir: &str,
    fixture: &str,
    iterations: usize,
    driver: &str,
    resources: &[BinaryResource<'_>],
) -> std::path::PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let archive = root.join("target/native-runtime-abi/debug/libdever_backend_bridge.a");
    assert!(
        archive.is_file(),
        "missing explicit runtime ABI fixture archive"
    );
    let object = directory.path().join("entry.o");
    fs::write(directory.path().join("entry.ll"), ir).unwrap();
    fs::write(
        &object,
        emit_object_with_resources(ir, Target::LinuxX86_64, resources).unwrap(),
    )
    .unwrap();
    let program = directory.path().join("managed-kernel");
    let mut compiler = Command::new("/usr/bin/cc");
    compiler
        .env_clear()
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-O2"])
        .arg(format!("-DDEVER_TEST_ITERATIONS={iterations}"))
        .arg(root.join("test/native-runtime-abi").join(driver))
        .arg(object)
        .arg(archive)
        .args([
            "-Wl,--wrap=malloc",
            "-Wl,--wrap=calloc",
            "-Wl,--wrap=realloc",
            "-Wl,--wrap=free",
            "-Wl,--wrap=posix_memalign",
            "-Wl,--wrap=realpath",
            "-ldl",
            "-lpthread",
            "-lm",
            "-o",
        ])
        .arg(&program)
        // Author tools are explicit, not a product PATH fallback.
        .arg("-B/usr/bin/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    assert!(
        process::status(&mut compiler, Duration::from_secs(30))
            .unwrap()
            .success(),
        "could not link LLVM fixture\n{fixture}"
    );
    program
}

/// Exact status/kind/message oracle for the common generated fault layout.
pub fn fault_check(code: u32, message: Option<&str>) -> String {
    let mut checks = format!(
        "  %failed = icmp eq i32 %status, 1\n  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0\n  %code = load i32, ptr %code_ptr\n  %kind = icmp eq i32 %code, {code}\n  %status_ok = and i1 %failed, %kind"
    );
    let Some(message) = message else {
        checks.push_str("\n  %passed = and i1 %status_ok, true");
        return checks;
    };
    write!(checks, "\n  %message_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 4, i32 0\n  %message = load ptr, ptr %message_ptr\n  %length_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 4, i32 1\n  %length = load i64, ptr %length_ptr\n  %length_ok = icmp eq i64 %length, {}\n  %has_message = icmp ne ptr %message, null\n  %buffer_ok = and i1 %length_ok, %has_message\n  %header_ok = and i1 %status_ok, %buffer_ok\n  br i1 %header_ok, label %content, label %wrong\ncontent:\n", message.len()).unwrap();
    let mut previous = "true".to_owned();
    for (index, byte) in message.bytes().enumerate() {
        writeln!(checks, "  %byte_ptr{index} = getelementptr i8, ptr %message, i64 {index}\n  %byte{index} = load i8, ptr %byte_ptr{index}\n  %byte_ok{index} = icmp eq i8 %byte{index}, {byte}\n  %content_ok{index} = and i1 {previous}, %byte_ok{index}").unwrap();
        previous = format!("%content_ok{index}");
    }
    write!(checks, "  br label %done\nwrong:\n  br label %done\ndone:\n  %passed = phi i1 [ {previous}, %content ], [ false, %wrong ]").unwrap();
    checks
}
