//! Checked-source resource acceptance using owned temporary files only.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm.rs"]
mod llvm;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm_managed.rs"]
mod llvm_managed;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::{llvm, llvm_managed, process, temp};
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    const HELPERS: &str = r#"read_text(value: dever.io.ReadState.Read(bytes)) (answer: Text) { answer = dever.bytes.to_text(bytes) }
read_text(value: dever.io.ReadState.End) (answer: Text) { answer = "EOF" }
closed(value: dever.io.CloseResult.Closed, wanted: Bool) (answer: Bool) recover("test explicitly checks close results") { answer = wanted }
closed(value: dever.io.CloseResult.Failed(message), wanted: Bool) (answer: Bool) recover("test explicitly checks close results") { answer = not wanted and message == "resource is closed" }
read_failed(value: dever.io.ReadResult.Done(state), expected: Text) (answer: Bool) recover("test checks failure identity and message") { answer = false }
read_failed(value: dever.io.ReadResult.Failed(message), expected: Text) (answer: Bool) recover("test checks failure identity and message") { answer = message == expected }
take_one(value: dever.io.ReadEvent.Chunk(bytes), previous: Text) (answer: Text, stop: Bool) recover("test records exactly one stream event") { answer = dever.bytes.to_text(bytes)
stop = true }
take_one(value: dever.io.ReadEvent.Failed(message), previous: Text) (answer: Text, stop: Bool) recover("test records exactly one stream event") { answer = message
stop = true }
next(stream: Stream<dever.io.ReadEvent>) (answer: Text) recover("test records exactly one stream event") { answer = reduce_until(take_one, stream, "EOF") }
count(value: dever.io.ReadEvent.Chunk(bytes), total: Int) (answer: Int) recover("test observes stream terminal errors") { answer = total + dever.bytes.length(bytes) }
count(value: dever.io.ReadEvent.Failed(message), total: Int) (answer: Int) recover("test observes stream terminal errors") { answer = -1 }
finish(value: dever.io.ReadEvent.Chunk(bytes), total: Int) (answer: Int, stop: Bool) recover("test observes stream terminal errors") { answer = total + dever.bytes.length(bytes)
stop = true }
finish(value: dever.io.ReadEvent.Failed(message), total: Int) (answer: Int, stop: Bool) recover("test observes stream terminal errors") { answer = -1
stop = true }
"#;

    fn source(body: &str, path: &Path) -> String {
        format!(
            "{HELPERS}\n{}",
            body.replace("FIXTURE_PATH", &format!("{:?}", path.to_str().unwrap()))
        )
    }

    fn fixture() -> (temp::TemporaryDirectory, std::path::PathBuf) {
        let directory = temp::TemporaryDirectory::new();
        let path = directory.path().join("owned.txt");
        fs::write(&path, "abcdef").unwrap();
        (directory, path)
    }

    #[test]
    fn file_and_stream_lowering_uses_runtime_owners_and_original_choices() {
        let ir = llvm::lower(&source(
            r#"public main() (answer: Bool) {
file = dever.io.open(FIXTURE_PATH)
stream = dever.io.chunks(file, 2)
first_chunk = next(stream)
remaining = reduce(count, stream, 0)
close(stream)
answer = first_chunk == "ab" and remaining == 4
}"#,
            Path::new("owned-test-file"),
        ));
        assert!(ir.contains("@dever_rt_v1_file_open"));
        assert!(ir.contains("@dever_rt_v1_file_chunks"));
        assert!(ir.contains("@dever_rt_v1_stream_pull"));
        assert!(ir.contains("@dever_rt_v1_stream_release"));
        assert!(ir.contains("@dever_read_Chunk_"));
        assert!(ir.contains("@dever_read_Failed_"));
        assert!(!ir.contains("dever_runtime"));
        for target in dever_backend_bridge::Target::ALL {
            assert!(
                !dever_backend_bridge::emit_object(&ir, target)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn read_eof_and_alias_close_preserve_business_error_identity() {
        let (_directory, path) = fixture();
        llvm_managed::boolean(&source(
            r#"public main() (answer: Bool) {
file = dever.io.open(FIXTURE_PATH)
alias = file
first_read = read_text(dever.io.read(file, 2))
remaining = read_text(dever.io.read(alias, 8))
eof = read_text(dever.io.read(file, 2))
bad_limit = read_failed(result(dever.io.read(file, 0)), "read size must be a positive Int")
first_close = closed(result(dever.io.close(file)), true)
second_close = closed(result(dever.io.close(alias)), false)
closed_read = read_failed(result(dever.io.read(alias, 1)), "resource is closed")
answer = first_read == "ab" and remaining == "cdef" and eof == "EOF" and bad_limit and first_close and second_close and closed_read
}"#,
            &path,
        ));
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn stream_aliases_share_progress_and_early_close() {
        let (_directory, path) = fixture();
        llvm_managed::boolean(&source(
            r#"public main() (answer: Bool) {
file = dever.io.open(FIXTURE_PATH)
stream = dever.io.chunks(file, 2)
alias = stream
first_chunk = next(stream)
second_chunk = next(alias)
close(stream)
close(alias)
ended = next(alias)
file_still_open = read_text(dever.io.read(file, 2))
answer = first_chunk == "ab" and second_chunk == "cd" and ended == "EOF" and file_still_open == "ef"
}"#,
            &path,
        ));
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn terminal_read_failure_is_one_event_then_end() {
        let (_directory, path) = fixture();
        llvm_managed::boolean(&source(
            r#"public main() (answer: Bool) {
file = dever.io.open(FIXTURE_PATH)
stream = dever.io.chunks(file, 2)
dever.io.close(file)
failed = next(stream)
ended = next(stream)
answer = failed == "resource is closed" and ended == "EOF"
}"#,
            &path,
        ));
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn reduce_until_retains_the_shared_unvisited_cursor() {
        let (_directory, path) = fixture();
        llvm_managed::boolean(&source(
            r#"public main() (answer: Bool) {
file = dever.io.open(FIXTURE_PATH)
stream = dever.io.chunks(file, 2)
alias = stream
total = reduce_until(finish, stream, 0)
next_chunk = next(alias)
remaining = reduce(count, alias, 0)
answer = total == 2 and next_chunk == "cd" and remaining == 2
}"#,
            &path,
        ));
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn owned_file_output_and_stream_release_do_not_leak() {
        let (_directory, path) = fixture();
        llvm_managed::execute(
            &source(
                r#"public main() (file: dever.system.File, answer: Bool) {
file = dever.io.open(FIXTURE_PATH)
stream = dever.io.chunks(file, 2)
answer = next(stream) == "ab"
}"#,
                &path,
            ),
            "{ ptr, i1 }",
            "  %okay = icmp eq i32 %status, 0\n  %row = load { ptr, i1 }, ptr %out\n  %file = extractvalue { ptr, i1 } %row, 0\n  %answer = extractvalue { ptr, i1 } %row, 1\n  %present = icmp ne ptr %file, null\n  %valid = and i1 %okay, %present\n  %passed = and i1 %valid, %answer",
        );
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn create_new_and_write_never_overwrite_existing_content() {
        let directory = temp::TemporaryDirectory::new();
        let path = directory.path().join("exclusive.txt");
        assert!(!path.exists());
        llvm_managed::boolean(&source(
            r#"created(value: dever.io.OpenResult.Opened(file)) (answer: Bool) recover("test checks create_new on repeated calls") {
dever.io.write(file, dever.bytes.from_text("written"))
dever.io.close(file)
answer = not_created(result(dever.io.create(FIXTURE_PATH)))
}
created(value: dever.io.OpenResult.Failed(message)) (answer: Bool) recover("test checks create_new on repeated calls") { answer = dever.system.text_contains(message, "File exists") }
not_created(value: dever.io.OpenResult.Opened(file)) (answer: Bool) recover("test rejects a second successful creation") {
dever.io.write(file, dever.bytes.from_text("unexpected overwrite"))
dever.io.close(file)
answer = false
}
not_created(value: dever.io.OpenResult.Failed(message)) (answer: Bool) recover("test rejects a second successful creation") { answer = dever.system.text_contains(message, "File exists") }
public main() (answer: Bool) {
created_once = created(result(dever.io.create(FIXTURE_PATH)))
file = dever.io.open(FIXTURE_PATH)
answer = created_once and read_text(dever.io.read(file, 64)) == "written"
}"#,
            &path,
        ));
        assert_eq!(fs::read(&path).unwrap(), b"written");
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn runtime_fault_unwinds_owned_file_and_partly_read_stream() {
        let (_directory, path) = fixture();
        llvm_managed::runtime_fault(
            &source(
                r#"identity(value: Text) (answer: Text) { answer = value }
duplicate(file: dever.system.File) (answer: Int) {
stream = dever.io.chunks(file, 2)
observed = next(stream)
key = identity("same")
keys = { key = 1
identity(key) = 2 }
answer = length(keys) + length(observed)
}
public main() (answer: Int) {
file = dever.io.open(FIXTURE_PATH)
answer = duplicate(file)
}"#,
                &path,
            ),
            "duplicate Map key",
        );
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn each_propagates_owned_business_failure_and_releases_its_cursor() {
        let (_directory, path) = fixture();
        llvm_managed::boolean(&source(
            r#"type TraversalResult {
Done
error Failed(message: Text, history: List<Text>)
}
stream_value(value: dever.io.ReadStreamResult.Streaming(stream)) (answer: Stream<dever.io.ReadEvent>) { answer = stream }
stream_value(value: dever.io.ReadStreamResult.Failed(message)) (answer: Stream<dever.io.ReadEvent>) { fail(TraversalResult.Failed(message, ["terminal error"])) }
close_value(value: dever.io.CloseResult.Closed) () {}
close_value(value: dever.io.CloseResult.Failed(message)) () { fail(TraversalResult.Failed(message, ["terminal error"])) }
consume(value: dever.io.ReadEvent.Chunk(bytes), file: dever.system.File) () { close_value(result(dever.io.close(file))) }
consume(value: dever.io.ReadEvent.Failed(message), file: dever.system.File) () { fail(TraversalResult.Failed(message, ["terminal error"])) }
drain(file: dever.system.File) () {
stream = stream_value(result(dever.io.chunks(file, 2)))
each(consume, stream, file)
}
observed(value: TraversalResult.Done) (answer: Bool) recover("test requires the owned terminal failure") { answer = false }
observed(value: TraversalResult.Failed(message, history)) (answer: Bool) recover("test requires the owned terminal failure") { answer = message == "resource is closed" and history == ["terminal error"] }
public main() (answer: Bool) { answer = observed(result(drain(dever.io.open(FIXTURE_PATH)))) }
"#,
            &path,
        ));
    }

    #[test]
    #[ignore = "requires the explicitly prepared runtime-abi archive"]
    fn resource_c_abi_rejects_invalid_outputs_before_io_or_cursor_progress() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let archive = root.join("target/native-runtime-abi/debug/libdever_backend_bridge.a");
        assert!(
            archive.is_file(),
            "missing explicit runtime ABI fixture archive"
        );
        let directory = temp::TemporaryDirectory::new();
        let program = directory.path().join("resource-abi");
        let mut compiler = Command::new("/usr/bin/cc");
        compiler
            .env_clear()
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-O2", "-I"])
            .arg(root.join("crates/dever-backend-bridge/include"))
            .arg(root.join("test/native-runtime-abi/resource-driver.c"))
            .arg(archive)
            .args(["-ldl", "-lpthread", "-lm", "-o"])
            .arg(&program)
            .arg("-B/usr/bin/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        assert!(
            process::status(&mut compiler, Duration::from_secs(30))
                .unwrap()
                .success()
        );
        assert!(
            process::status(
                Command::new(program)
                    .arg(directory.path().join("exclusive.txt"))
                    .env_clear()
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit()),
                Duration::from_secs(5)
            )
            .unwrap()
            .success()
        );
    }
}
