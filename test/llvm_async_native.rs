//! Actual LLVM coroutine object linked with the runtime-only archive.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
fn switched_resume_coroutine_suspends_resumes_and_destroys() {
    use dever_backend_bridge::{Target, emit_object};
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let archive = root.join("target/native-runtime-abi/debug/libdever_backend_bridge.a");
    assert!(
        archive.is_file(),
        "missing explicit runtime ABI fixture archive"
    );
    let directory = temp::TemporaryDirectory::new();
    let object = directory.path().join("coroutine.o");
    std::fs::write(
        &object,
        emit_object(COROUTINE, Target::LinuxX86_64).unwrap(),
    )
    .unwrap();
    let executable = directory.path().join("coroutine");
    let compiler = Command::new("/usr/bin/cc")
        .env_clear()
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-O2"])
        .arg(root.join("test/native-runtime-abi/async-driver.c"))
        .arg(object)
        .arg(archive)
        .args(["-ldl", "-lpthread", "-lm", "-o"])
        .arg(&executable)
        .arg("-B/usr/bin/")
        .output()
        .unwrap();
    assert!(
        compiler.status.success(),
        "{}",
        String::from_utf8_lossy(&compiler.stderr)
    );
    let status = process::status(
        Command::new(executable)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(status.success(), "coroutine driver failed: {status}");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
fn async_operations_preserve_scope_and_backpressure() {
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let archive = root.join("target/native-runtime-abi/debug/libdever_backend_bridge.a");
    assert!(
        archive.is_file(),
        "missing explicit runtime ABI fixture archive"
    );
    let directory = temp::TemporaryDirectory::new();
    let executable = directory.path().join("async-operations");
    let compiler = Command::new("/usr/bin/cc")
        .env_clear()
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-O2"])
        .arg("-I")
        .arg(root.join("crates/dever-backend-bridge/include"))
        .arg(root.join("test/native-runtime-abi/async-operations-driver.c"))
        .arg(archive)
        .args(["-ldl", "-lpthread", "-lm", "-o"])
        .arg(&executable)
        .arg("-B/usr/bin/")
        .output()
        .unwrap();
    assert!(
        compiler.status.success(),
        "{}",
        String::from_utf8_lossy(&compiler.stderr)
    );
    let status = process::status(
        Command::new(executable)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(status.success(), "async operation driver failed: {status}");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const COROUTINE: &str = r#"
@progress = global i32 0
declare ptr @malloc(i64)
declare void @free(ptr)
declare token @llvm.coro.id(i32, ptr, ptr, ptr)
declare i64 @llvm.coro.size.i64()
declare ptr @llvm.coro.begin(token, ptr)
declare i8 @llvm.coro.suspend(token, i1)
declare ptr @llvm.coro.free(token, ptr)
declare i1 @llvm.coro.end(ptr, i1, token)
declare void @llvm.coro.resume(ptr)
declare void @llvm.coro.destroy(ptr)
declare i1 @llvm.coro.done(ptr)
declare i64 @llvm.coro.align.i64()
declare ptr @dever_rt_v1_async_frame_alloc(i64, i64)
declare void @dever_rt_v1_async_frame_free(ptr, i64, i64)
declare void @dever_rt_v1_async_complete(ptr, i32)
declare i32 @dever_rt_v1_async_root(ptr, ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_sleep(i64)
declare i32 @dever_rt_v1_async_op_poll(ptr, ptr, ptr, ptr, ptr, ptr)
declare void @dever_rt_v1_async_op_release(ptr)
@root_polls = global i32 0

@probe_type = private constant { i64, i64, ptr, ptr, i8 } { i64 8, i64 8, ptr @probe_move, ptr @probe_drop, i8 1 }
@probe_function = private constant { ptr, ptr, ptr, ptr, ptr, ptr, i8 } {
  ptr @probe_type, ptr @probe_type, ptr @dever_coro_root_create,
  ptr @dever_coro_root_resume, ptr @dever_coro_root_destroy, ptr @dever_coro_root_done, i8 1
}

define void @probe_move(ptr %source, ptr %destination) {
entry:
  %value = load i64, ptr %source
  store i64 %value, ptr %destination
  store i64 0, ptr %source
  ret void
}

define void @probe_drop(ptr %value) {
entry:
  ret void
}

define ptr @dever_coro_probe_create() presplitcoroutine {
entry:
  %id = call token @llvm.coro.id(i32 0, ptr null, ptr null, ptr null)
  %size = call i64 @llvm.coro.size.i64()
  %memory = call ptr @malloc(i64 %size)
  %frame = call ptr @llvm.coro.begin(token %id, ptr %memory)
  %initial = call i8 @llvm.coro.suspend(token none, i1 false)
  switch i8 %initial, label %suspend [i8 0, label %body i8 1, label %cleanup]
body:
  store i32 42, ptr @progress
  %final = call i8 @llvm.coro.suspend(token none, i1 true)
  switch i8 %final, label %suspend [i8 1, label %cleanup]
cleanup:
  %allocation = call ptr @llvm.coro.free(token %id, ptr %frame)
  call void @free(ptr %allocation)
  br label %suspend
suspend:
  call i1 @llvm.coro.end(ptr %frame, i1 false, token none)
  ret ptr %frame
}

define i32 @dever_coro_probe_progress() {
entry:
  %value = load i32, ptr @progress
  ret i32 %value
}

define void @dever_coro_probe_resume(ptr %frame) {
entry:
  call void @llvm.coro.resume(ptr %frame)
  ret void
}

define i1 @dever_coro_probe_done(ptr %frame) {
entry:
  %done = call i1 @llvm.coro.done(ptr %frame)
  ret i1 %done
}

define void @dever_coro_probe_destroy(ptr %frame) {
entry:
  call void @llvm.coro.destroy(ptr %frame)
  ret void
}

define ptr @dever_coro_root_create(ptr %state, ptr %input, ptr %output, ptr %fault) presplitcoroutine {
entry:
  %id = call token @llvm.coro.id(i32 0, ptr null, ptr null, ptr null)
  %size = call i64 @llvm.coro.size.i64()
  %align = call i64 @llvm.coro.align.i64()
  %memory = call ptr @dever_rt_v1_async_frame_alloc(i64 %size, i64 %align)
  %frame = call ptr @llvm.coro.begin(token %id, ptr %memory)
  %operation = alloca ptr
  store ptr null, ptr %operation
  %error = alloca { ptr, i64 }
  %initial = call i8 @llvm.coro.suspend(token none, i1 false)
  switch i8 %initial, label %suspend [i8 0, label %body i8 1, label %cleanup]
body:
  %sleep = call ptr @dever_rt_v1_async_sleep(i64 1)
  store ptr %sleep, ptr %operation
  br label %poll_sleep
poll_sleep:
  %polls = load i32, ptr @root_polls
  %next_polls = add i32 %polls, 1
  store i32 %next_polls, ptr @root_polls
  %active = load ptr, ptr %operation
  %sleep_status = call i32 @dever_rt_v1_async_op_poll(ptr %state, ptr %active, ptr null, ptr null, ptr null, ptr %error)
  %pending = icmp eq i32 %sleep_status, 0
  br i1 %pending, label %wait_sleep, label %finished_sleep
wait_sleep:
  %waiting = call i8 @llvm.coro.suspend(token none, i1 false)
  switch i8 %waiting, label %suspend [i8 0, label %poll_sleep i8 1, label %cleanup]
finished_sleep:
  %completed_op = load ptr, ptr %operation
  call void @dever_rt_v1_async_op_release(ptr %completed_op)
  store ptr null, ptr %operation
  store i64 42, ptr %output
  call void @dever_rt_v1_async_complete(ptr %state, i32 1)
  %final = call i8 @llvm.coro.suspend(token none, i1 true)
  switch i8 %final, label %suspend [i8 1, label %cleanup]
cleanup:
  %remaining = load ptr, ptr %operation
  %has_op = icmp ne ptr %remaining, null
  br i1 %has_op, label %release_op, label %free_frame
release_op:
  call void @dever_rt_v1_async_op_release(ptr %remaining)
  br label %free_frame
free_frame:
  %allocation = call ptr @llvm.coro.free(token %id, ptr %frame)
  call void @dever_rt_v1_async_frame_free(ptr %allocation, i64 %size, i64 %align)
  br label %suspend
suspend:
  call i1 @llvm.coro.end(ptr %frame, i1 false, token none)
  ret ptr %frame
}

define void @dever_coro_root_resume(ptr %frame) {
entry:
  call void @llvm.coro.resume(ptr %frame)
  ret void
}

define i8 @dever_coro_root_done(ptr %frame) {
entry:
  %done = call i1 @llvm.coro.done(ptr %frame)
  %result = zext i1 %done to i8
  ret i8 %result
}

define void @dever_coro_root_destroy(ptr %frame) {
entry:
  call void @llvm.coro.destroy(ptr %frame)
  ret void
}

define i32 @dever_coro_root_run() {
entry:
  %output = alloca i64
  %fault = alloca i64
  %error = alloca { ptr, i64 }
  %status = call i32 @dever_rt_v1_async_root(ptr @probe_function, ptr null, ptr %output, ptr %fault, ptr %error)
  %value = load i64, ptr %output
  %value_ok = icmp eq i64 %value, 42
  %status_ok = icmp eq i32 %status, 1
  %polls = load i32, ptr @root_polls
  %suspended = icmp uge i32 %polls, 2
  %result_ok = and i1 %value_ok, %status_ok
  %passed = and i1 %result_ok, %suspended
  %code = select i1 %passed, i32 0, i32 1
  ret i32 %code
}
"#;
