//! Developer-only C ABI acceptance; normal execution never invokes these tools.
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

// Requires an explicitly produced runtime-abi-only archive. It is kept separate
// from the default tests because Cargo must not recursively build from a test.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
fn canonical_c_header_links_and_executes_the_real_runtime_archive() {
    run_driver("driver.c", None, false);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn run_driver(source: &str, llvm_object: Option<&Path>, allocation_ledger: bool) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let archive = root.join("target/native-runtime-abi/debug/libdever_backend_bridge.a");
    assert!(
        archive.is_file(),
        "missing explicit runtime ABI fixture archive"
    );
    let directory = temp::TemporaryDirectory::new();
    let program = directory.path().join("abi-acceptance");
    let mut command = Command::new("/usr/bin/cc");
    command
        .env_clear()
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-O2", "-I"])
        .arg(root.join("crates/dever-backend-bridge/include"))
        .arg(root.join("test/native-runtime-abi").join(source));
    if let Some(object) = llvm_object {
        command.arg("-DDEVER_LLVM_ABI").arg(object);
    }
    if allocation_ledger {
        command.arg(root.join("test/native-runtime-abi/managed-driver.c"));
        command.args([
            "-Wl,--wrap=malloc",
            "-Wl,--wrap=calloc",
            "-Wl,--wrap=realloc",
            "-Wl,--wrap=free",
            "-Wl,--wrap=posix_memalign",
            "-Wl,--wrap=realpath",
        ]);
    }
    let compile = command
        .arg(archive)
        .args(["-ldl", "-lpthread", "-lm", "-o"])
        .arg(&program)
        // GCC's internal assembler/linker are fixture authoring tools, not a
        // runtime PATH fallback. Their search directories are explicit.
        .arg("-B/usr/bin/")
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let status = process::status(
        Command::new(program)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit()),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(status.success(), "C ABI driver failed: {status}");
}

#[cfg(all(feature = "embedded", target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
fn llvm_object_calls_the_versioned_runtime_abi_without_rust_layouts() {
    use dever_backend_bridge::{Target, emit_object};
    let directory = temp::TemporaryDirectory::new();
    let object = directory.path().join("probe.o");
    std::fs::write(
        &object,
        emit_object(ABI_PROBE, Target::LinuxX86_64).unwrap(),
    )
    .unwrap();
    run_driver("driver.c", Some(&object), false);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
fn protocol_poll_rejects_invalid_output_before_advancing_the_operation() {
    run_driver("protocol-driver.c", None, false);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
fn logger_flush_is_a_stable_native_allocation_barrier() {
    run_driver("log-driver.c", None, true);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
fn wire_codecs_validate_slots_before_callback_and_release_partial_values() {
    run_driver("wire-driver.c", None, true);
}

#[cfg(all(
    any(feature = "runtime-sqlite", feature = "runtime-postgres"),
    target_os = "linux",
    target_arch = "x86_64"
))]
#[test]
#[ignore = "build a runtime-sqlite or runtime-postgres staticlib in target/native-runtime-abi before running"]
fn database_abi_preserves_typed_owners_and_rejects_invalid_slots_without_io() {
    run_driver("database-driver.c", None, true);
}

#[cfg(all(feature = "embedded", target_os = "linux", target_arch = "x86_64"))]
const ABI_PROBE: &str = r#"
%abi.buffer = type { ptr, i64 }
%abi.decimal = type { i64, i64 }
declare i32 @dever_rt_v1_version()
declare i32 @dever_rt_v1_int_add(i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_from_int(i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_div(i64, i64, i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_to_text(i64, i64, ptr)
declare void @dever_rt_v1_buffer_free(ptr, i64)
define i32 @dever_abi_probe() {
entry:
  %buffer = alloca %abi.buffer
  %integer = alloca i64
  %seven = alloca %abi.decimal
  %two = alloca %abi.decimal
  %decimal = alloca %abi.decimal
  %version = call i32 @dever_rt_v1_version()
  %version_ok = icmp eq i32 %version, 1
  %add = call i32 @dever_rt_v1_int_add(i64 40, i64 2, ptr %integer, ptr %buffer)
  %add_ok = icmp eq i32 %add, 0
  %first_ok = and i1 %version_ok, %add_ok
  br i1 %first_ok, label %added, label %failed
added:
  %value = load i64, ptr %integer
  %value_ok = icmp eq i64 %value, 42
  %overflow = call i32 @dever_rt_v1_int_add(i64 9223372036854775807, i64 1, ptr %integer, ptr %buffer)
  %overflow_ok = icmp eq i32 %overflow, 1
  %error = load %abi.buffer, ptr %buffer
  %error_ptr = extractvalue %abi.buffer %error, 0
  %error_len = extractvalue %abi.buffer %error, 1
  %message_ok = icmp eq i64 %error_len, 12
  call void @dever_rt_v1_buffer_free(ptr %error_ptr, i64 %error_len)
  %error_ok = and i1 %overflow_ok, %message_ok
  %numeric_ok = and i1 %value_ok, %error_ok
  br i1 %numeric_ok, label %decimal_input, label %failed
decimal_input:
  %from_seven = call i32 @dever_rt_v1_decimal_from_int(i64 7, ptr %seven, ptr %buffer)
  %from_two = call i32 @dever_rt_v1_decimal_from_int(i64 2, ptr %two, ptr %buffer)
  %seven_ok = icmp eq i32 %from_seven, 0
  %two_ok = icmp eq i32 %from_two, 0
  %inputs_ok = and i1 %seven_ok, %two_ok
  br i1 %inputs_ok, label %divide, label %failed
divide:
  %seven_value = load %abi.decimal, ptr %seven
  %two_value = load %abi.decimal, ptr %two
  %seven_low = extractvalue %abi.decimal %seven_value, 0
  %seven_high = extractvalue %abi.decimal %seven_value, 1
  %two_low = extractvalue %abi.decimal %two_value, 0
  %two_high = extractvalue %abi.decimal %two_value, 1
  %divided = call i32 @dever_rt_v1_decimal_div(i64 %seven_low, i64 %seven_high, i64 %two_low, i64 %two_high, ptr %decimal, ptr %buffer)
  %divide_ok = icmp eq i32 %divided, 0
  br i1 %divide_ok, label %render, label %failed
render:
  %result = load %abi.decimal, ptr %decimal
  %low = extractvalue %abi.decimal %result, 0
  %high = extractvalue %abi.decimal %result, 1
  %rendered = call i32 @dever_rt_v1_decimal_to_text(i64 %low, i64 %high, ptr %buffer)
  %render_ok = icmp eq i32 %rendered, 0
  %text = load %abi.buffer, ptr %buffer
  %text_ptr = extractvalue %abi.buffer %text, 0
  %text_len = extractvalue %abi.buffer %text, 1
  %len_ok = icmp eq i64 %text_len, 3
  %text_ok = and i1 %render_ok, %len_ok
  br i1 %text_ok, label %read_text, label %release_failed
read_text:
  %byte0 = load i8, ptr %text_ptr
  %ptr1 = getelementptr i8, ptr %text_ptr, i64 1
  %ptr2 = getelementptr i8, ptr %text_ptr, i64 2
  %byte1 = load i8, ptr %ptr1
  %byte2 = load i8, ptr %ptr2
  %byte0_ok = icmp eq i8 %byte0, 51
  %byte1_ok = icmp eq i8 %byte1, 46
  %byte2_ok = icmp eq i8 %byte2, 53
  %prefix_ok = and i1 %byte0_ok, %byte1_ok
  %passed = and i1 %prefix_ok, %byte2_ok
  call void @dever_rt_v1_buffer_free(ptr %text_ptr, i64 %text_len)
  %status = select i1 %passed, i32 0, i32 1
  ret i32 %status
release_failed:
  call void @dever_rt_v1_buffer_free(ptr %text_ptr, i64 %text_len)
  br label %failed
failed:
  ret i32 1
}
"#;
