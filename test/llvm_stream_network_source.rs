//! Checked-source stream/network/task composition, not hand-written ABI approximations.

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
    use super::{llvm, llvm_managed};
    use dever_backend_bridge::{Target, emit_object};
    use dever_core::{check, source::SourceMap};

    const STREAMS: &str = r#"collect(value: Text?, previous: List<Text?>) (next: List<Text?>) { next = append(previous, value) }
add(value: Int, previous: Int) (next: Int) { next = previous + value }
one(value: Text, previous: List<Text>) (next: List<Text>, stop: Bool) { next = append(previous, value)
  stop = true }
collect_text(value: Text, previous: List<Text>) (next: List<Text>) { next = append(previous, value) }
public main() (answer: Bool) {
  nullable = stream([some_text("first"), null, some_text("last")])
  collected = reduce(collect, nullable, [])
  bytes = stream(dever.bytes.from_text("AB"))
  total = reduce(add, bytes, 0)
  values = stream(["one", "two"])
  alias = values
  first = reduce_until(one, values, [])
  remaining = reduce(collect_text, alias, [])
  close(alias)
  close(values)
  answer = collected == [some_text("first"), null, some_text("last")] and total == 131 and first == ["one"] and length(remaining) == 0
}
"#;

    const CHANNEL_STREAM: &str = r#"send_values(channel: Channel<Text?>) () {
  send(channel, null)
  dever.task.sleep(1)
  send(channel, some_text("pending"))
  close(channel)
}
collect(value: Text?, previous: List<Text?>) (next: List<Text?>) {
  dever.task.sleep(1)
  next = append(previous, value)
}
public main() (answer: Bool) {
  channel = channel(Text?, 1)
  sender = run(send_values(channel))
  values = stream(channel)
  alias = values
  collected = reduce(collect, values, [])
  wait(sender)
  remaining = reduce(collect, alias, [])
  answer = collected == [null, some_text("pending")] and length(remaining) == 0
}
"#;

    const TICKS: &str = r#"tick(value: Int, count: Int) (next: Int, stop: Bool) { next = count + 1
  stop = next == 2 }
add(value: Int, previous: Int) (next: Int) { next = previous + value }
public main() (answer: Bool) {
  values = dever.task.ticks(1)
  alias = values
  count = reduce_until(tick, values, 0)
  rest = reduce(add, alias, 0)
  close(values)
  answer = count == 2 and rest == 0
}
"#;

    const PARALLEL_STREAM: &str = r#"collect(value: Int, output: Channel<Int>) () { dever.task.sleep(1)
  send(output, value) }
add(value: Int, previous: Int) (next: Int) { next = previous + value }
public main() (answer: Bool) {
  output = channel(Int, 8)
  parallel_each(collect, stream([1, 2, 3]), 2, output)
  parallel_each(collect, dever.bytes.from_text("AB"), 2, output)
  parallel_each(collect, [4, 5], 2, output)
  close(output)
  total = reduce(add, stream(output), 0)
  answer = total == 146
}
"#;

    const PARALLEL_FAILURE: &str = r#"type Outcome { Done
  error Failed(message: Text, history: List<Text>) }
visit(value: 2) () { fail(Outcome.Failed("parallel original", ["owned", "worker"])) }
visit(value: other) () { ready = pause(1) }
apply() () { parallel_each(visit, stream([1, 2, 3]), 2) }
inspect(value: Outcome.Done) (answer: Bool) recover("test requires parallel failure") { answer = false }
inspect(value: Outcome.Failed(message, history)) (answer: Bool) recover("test requires parallel failure") { answer = message == "parallel original" and history == ["owned", "worker"] }
public main() (answer: Bool) { answer = inspect(result(apply())) }
"#;

    const IDLE_PARALLEL_FAILURE: &str = r#"type Outcome { Done
  error Failed(message: Text, history: List<Text>) }
visit(value: Int) () { fail(Outcome.Failed("idle producer", ["owned", "worker"])) }
apply(values: AsyncStream<Int>) () { parallel_each(visit, values, 2) }
add(value: Int, previous: Int) (next: Int) { next = previous + value }
inspect(value: Outcome.Done) (answer: Bool) recover("test requires failure without another producer item") { answer = false }
inspect(value: Outcome.Failed(message, history)) (answer: Bool) recover("test requires failure without another producer item") { answer = message == "idle producer" and history == ["owned", "worker"] }
public main() (answer: Bool) {
  input = channel(Int, 1)
  send(input, 2)
  values = stream(input)
  alias = values
  failed = inspect(result(apply(values)))
  remaining = reduce(add, alias, 0)
  send(input, 3)
  independent = receive(input)
  close(input)
  answer = failed and remaining == 0 and independent == some_int(3)
}
"#;

    const SYNC_PARALLEL: &str = r#"type Outcome { Done
  error Failed(message: Text, history: List<Text>) }
visit(value: 0) () { fail(Outcome.Failed("sync original", ["owned", "worker"])) }
visit(value: other) () {}
good() () { parallel_each(visit, [1, 2, 3], 2)
  parallel_each(visit, dever.bytes.from_text("AB"), 2) }
bad() () { parallel_each(visit, [1, 0, 3], 2) }
inspect(value: Outcome.Done, expected: Bool) (answer: Bool) recover("test checks the expected branch") { answer = expected }
inspect(value: Outcome.Failed(message, history), expected: Bool) (answer: Bool) recover("test checks the expected branch") { answer = not expected and message == "sync original" and history == ["owned", "worker"] }
public main() (answer: Bool) { answer = inspect(result(good()), true) and inspect(result(bad()), false) }
"#;

    const ASYNC_SYNC_PARALLEL: &str = r#"type Outcome { Done
  error Failed(message: Text, history: List<Text>) }
visit(value: 0) () { fail(Outcome.Failed("sync original", ["owned", "worker"])) }
visit(value: other) () {}
good() () { ready = pause(1)
  parallel_each(visit, [1, 2, 3], 2)
  parallel_each(visit, dever.bytes.from_text("AB"), 2)
  parallel_each(visit, stream([1, 2]), 2) }
bad() () { ready = pause(1)
  parallel_each(visit, stream([1, 0, 3]), 2) }
inspect(value: Outcome.Done, expected: Bool) (answer: Bool) recover("test checks the expected branch") { answer = expected }
inspect(value: Outcome.Failed(message, history), expected: Bool) (answer: Bool) recover("test checks the expected branch") { answer = not expected and message == "sync original" and history == ["owned", "worker"] }
public main() (answer: Bool) { answer = inspect(result(good()), true) and inspect(result(bad()), false) }
"#;

    const SYNC_FILE_PARALLEL: &str = r#"type Outcome { Done
  error Failed(message: Text, history: List<Text>) }
visit(value: dever.io.ReadEvent.Chunk(bytes)) () { fail(Outcome.Failed("stream original", ["owned", "worker"])) }
visit(value: dever.io.ReadEvent.Failed(message)) () { fail(Outcome.Failed(message, ["wrong", "read failure"])) }
apply(values: Stream<dever.io.ReadEvent>) () { parallel_each(visit, values, 2) }
inspect(value: Outcome.Done) (answer: Bool) recover("test requires a worker fault") { answer = false }
inspect(value: Outcome.Failed(message, history)) (answer: Bool) recover("test requires a worker fault") { answer = message == "stream original" and history == ["owned", "worker"] }
public main() (answer: Bool) {
  file = dever.io.open(FIXTURE_PATH)
  values = dever.io.chunks(file, 1)
  answer = inspect(result(apply(values)))
  close(values)
  dever.io.close(file)
}
"#;

    const UNIT_COMPOSITION: &str = r#"finish() () { ready = pause(1) }
pending() () { ready = pause(1000) }
fallback() () { ready = pause(1) }
public main() (answer: Bool) {
  slow = run(pending())
  timeout(slow, 1, fallback)
  quick = run(finish())
  timeout(quick, 500, fallback)
  first = run(pending())
  second = run(finish())
  race(first, second)
  answer = true
}
"#;

    const TIMEOUT_VALUES: &str = r#"type Bundle { label: Text
  names: List<Text>
  lookup: Map<Text, List<Text>> }
pair(millis: Int) (bundle: Bundle, names: List<Text>) {
  dever.task.sleep(millis)
  bundle = Bundle { label = "completed"
    names = ["inside"]
    lookup = { "key" = ["value"] } }
  names = ["result"]
}
fallback() (bundle: Bundle, names: List<Text>) {
  dever.task.sleep(1)
  bundle = Bundle { label = "fallback"
    names = ["fallback"]
    lookup = { "key" = ["timeout"] } }
  names = ["fallback result"]
}
nullable() (value: Text?) { value = null }
wrong_nullable() (value: Text?) { value = "wrong fallback" }
public main() (answer: Bool) {
  slow = run(pair(1000))
  timed = timeout(slow, 1, fallback)
  timed_ok = timed.bundle.label == "fallback" and timed.bundle.names == ["fallback"] and get(timed.bundle.lookup, "key") == some_names(["timeout"]) and timed.names == ["fallback result"]
  quick = run(pair(1))
  ready = timeout(quick, 500, fallback)
  ready_ok = ready.bundle.label == "completed" and ready.names == ["result"]
  optional = run(nullable())
  nothing = timeout(optional, 500, wrong_nullable)
  answer = timed_ok and ready_ok and nothing == null
}
some_names(value: List<Text>) (answer: List<Text>?) { answer = value }
"#;

    const RACE_VALUES: &str = r#"pair(label: Text, millis: Int) (text: Text, history: List<Text>) {
  dever.task.sleep(millis)
  text = label
  history = [label, "owned"]
}
public main() (answer: Bool) {
  slow = run(pair("slow", 1000))
  fast = run(pair("fast", 1))
  third = run(pair("other", 1000))
  winner = race(slow, fast, third)
  answer = winner.text == "fast" and winner.history == ["fast", "owned"]
}
"#;

    const COMPOSITION_FAILURE: &str = r#"type Outcome { Done(value: Text)
  error Failed(message: Text, history: List<Text>) }
broken() (value: Text) { ready = pause(1)
  fail(Outcome.Failed("original", ["owned", "failure"])) }
slow() (value: Text) { ready = pause(1000)
  value = "wrong success" }
fallback() (value: Text) { value = "wrong fallback" }
timed() (value: Text) { job = run(broken())
  value = timeout(job, 500, fallback) }
compete() (value: Text) { failing = run(broken())
  pending = run(slow())
  value = race(failing, pending) }
inspect(value: Outcome.Done(text)) (answer: Bool) recover("test requires the original failure") { answer = false }
inspect(value: Outcome.Failed(message, history)) (answer: Bool) recover("test requires the original failure") { answer = message == "original" and history == ["owned", "failure"] }
public main() (answer: Bool) { answer = inspect(result(timed())) and inspect(result(compete())) }
"#;

    const TCP_ROUNDTRIP: &str = r#"collect_body(listener: dever.system.Listener) (body: Bytes) {
  socket = dever.net.accept(listener)
  dever.net.timeout(socket, 500)
  values = dever.net.chunks(socket, 2)
  body = reduce(collect, values, dever.bytes.from_text(""))
  dever.net.close(socket)
}
collect(value: dever.io.ReadEvent.Chunk(bytes), previous: Bytes) (next: Bytes) { next = dever.bytes.concat(previous, bytes) }
collect(value: dever.io.ReadEvent.Failed(message), previous: Bytes) (next: Bytes) { fail(dever.io.ReadResult.Failed(message)) }
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  worker = run(collect_body(listener))
  socket = dever.net.connect("127.0.0.1", dever.net.port(listener))
  dever.net.timeout(socket, 500)
  dever.net.write(socket, dever.bytes.from_text("ping"))
  dever.net.close(socket)
  body = wait(worker)
  dever.net.close_listener(listener)
  answer = body == dever.bytes.from_text("ping")
}
"#;

    // Exactly one byte is written and one byte read; this does not assume TCP chunk boundaries.
    const TCP_DUPLEX: &str = r#"bytes(value: dever.io.ReadState.Read(bytes)) (answer: Bytes) { answer = bytes }
bytes(value: dever.io.ReadState.End) (answer: Bytes) { answer = dever.bytes.from_text("wrong EOF") }
ended(value: dever.io.ReadState.Read(bytes)) (answer: Bool) { answer = false }
ended(value: dever.io.ReadState.End) (answer: Bool) { answer = true }
closed(value: dever.io.CloseResult.Closed) (answer: Bool) recover("test requires shared close failure") { answer = false }
closed(value: dever.io.CloseResult.Failed(message)) (answer: Bool) recover("test requires shared close failure") { answer = message == "resource is closed" }
echo(listener: dever.system.Listener) (answer: Bool) {
  socket = dever.net.accept(listener)
  dever.net.timeout(socket, 500)
  request = bytes(dever.net.read(socket, 1))
  dever.net.write(socket, request)
  dever.net.close(socket)
  answer = request == dever.bytes.from_text("Q")
}
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  worker = run(echo(listener))
  socket = dever.net.connect_timeout("127.0.0.1", dever.net.port(listener), 500)
  dever.net.timeout(socket, 500)
  dever.net.write(socket, dever.bytes.from_text("Q"))
  reply = bytes(dever.net.read(socket, 1))
  end = ended(dever.net.read(socket, 1))
  okay = wait(worker)
  alias = socket
  dever.net.close(alias)
  shared_close = closed(result(dever.net.close(socket)))
  dever.net.close_listener(listener)
  answer = okay and reply == dever.bytes.from_text("Q") and end and shared_close
}
"#;

    const NETWORK_ERRORS: &str = r#"bad_connect(value: dever.net.ConnectResult.Connected(socket)) (answer: Bool) recover("test rejects nonnumeric timed connect") { answer = false }
bad_connect(value: dever.net.ConnectResult.Failed(message)) (answer: Bool) recover("test rejects nonnumeric timed connect") { answer = message == "timed connect requires a numeric IP address" }
bad_listen(value: dever.net.ListenResult.Listening(listener)) (answer: Bool) recover("test rejects invalid listen port") { answer = false }
bad_listen(value: dever.net.ListenResult.Failed(message)) (answer: Bool) recover("test rejects invalid listen port") { answer = message == "port must be between 0 and 65535" }
closed_port(value: dever.net.PortResult.Bound(port)) (answer: Bool) recover("test checks shared listener closure") { answer = false }
closed_port(value: dever.net.PortResult.Failed(message)) (answer: Bool) recover("test checks shared listener closure") { answer = message == "resource is closed" }
public main() (answer: Bool) {
  connect = bad_connect(result(dever.net.connect_timeout("localhost", 0, 1)))
  listen = bad_listen(result(dever.net.listen("127.0.0.1", -1)))
  listener = dever.net.listen("127.0.0.1", 0)
  alias = listener
  dever.net.close_listener(alias)
  port = closed_port(result(dever.net.port(listener)))
  answer = connect and listen and port
}
"#;

    const TERMINAL_STREAM: &str = r#"count_read(value: dever.io.ReadEvent.Chunk(bytes), previous: Int) (next: Int) recover("test requires one terminal error") { next = -100 }
count_read(value: dever.io.ReadEvent.Failed(message), previous: Int) (next: Int) recover("test requires one terminal error") { next = previous + matching_closed(message) }
matching_closed(message: "resource is closed") (answer: Int) { answer = 1 }
matching_closed(message: other) (answer: Int) { answer = -100 }
count_connection(value: dever.net.ConnectResult.Connected(socket), previous: Int) (next: Int) recover("test requires one terminal error") { next = -100 }
count_connection(value: dever.net.ConnectResult.Failed(message), previous: Int) (next: Int) recover("test requires one terminal error") { next = previous + matching_closed(message) }
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  connecting = run(dever.net.connect_timeout("127.0.0.1", dever.net.port(listener), 500))
  socket = dever.net.accept(listener)
  peer = wait(connecting)
  reads = dever.net.chunks(socket, 1)
  read_alias = reads
  dever.net.close(socket)
  failed_reads = reduce(count_read, reads, 0)
  end_reads = reduce(count_read, read_alias, 0)
  incoming = dever.net.connections(listener)
  incoming_alias = incoming
  dever.net.close_listener(listener)
  failed_accept = reduce(count_connection, incoming, 0)
  end_accept = reduce(count_connection, incoming_alias, 0)
  dever.net.close(peer)
  answer = failed_reads == 1 and end_reads == 0 and failed_accept == 1 and end_accept == 0
}
"#;

    const TIMEOUT_DRAINS_SOCKET: &str = r#"ended(value: dever.io.ReadState.Read(bytes)) (answer: Bool) { answer = false }
ended(value: dever.io.ReadState.End) (answer: Bool) { answer = true }
pending(socket: dever.system.Socket) (answer: Bool) { answer = ended(dever.net.read(socket, 1)) }
parent(listener: dever.system.Listener, ready: Channel<Int>) (answer: Bool) {
  socket = dever.net.accept(listener)
  child = run(pending(socket))
  send(ready, 1)
  answer = wait(child)
}
fallback() (answer: Bool) { answer = true }
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  ready = channel(Int, 1)
  worker = run(parent(listener, ready))
  peer = dever.net.connect_timeout("127.0.0.1", dever.net.port(listener), 500)
  dever.net.timeout(peer, 500)
  started = receive(ready)
  timed = timeout(worker, 1, fallback)
  eof = ended(dever.net.read(peer, 1))
  close(ready)
  dever.net.close(peer)
  dever.net.close_listener(listener)
  answer = started == some_int(1) and timed and eof
}
"#;

    fn source(body: &str) -> String {
        let body = body.replace("FIXTURE_PATH", "\"owned-fixture-file\"");
        format!(
            "some_text(value: Text) (answer: Text?) {{ answer = value }}\nsome_int(value: Int) (answer: Int?) {{ answer = value }}\ntimer_status(value: dever.time.SleepResult.Done) (okay: Bool) recover(\"fixture checks timer\") {{ okay = true }}\ntimer_status(value: dever.time.SleepResult.Failed(message)) (okay: Bool) recover(\"fixture checks timer\") {{ okay = false }}\npause(millis: Int) (okay: Bool) {{ okay = timer_status(result(dever.task.sleep(millis))) }}\n{body}"
        )
    }

    const CASES: &[(&str, &str)] = &[
        ("streams", STREAMS),
        ("channel_stream", CHANNEL_STREAM),
        ("ticks", TICKS),
        ("parallel_stream", PARALLEL_STREAM),
        ("parallel_failure", PARALLEL_FAILURE),
        ("idle_parallel_failure", IDLE_PARALLEL_FAILURE),
        ("sync_parallel", SYNC_PARALLEL),
        ("async_sync_parallel", ASYNC_SYNC_PARALLEL),
        ("sync_file_parallel", SYNC_FILE_PARALLEL),
        ("unit_composition", UNIT_COMPOSITION),
        ("timeout_values", TIMEOUT_VALUES),
        ("race_values", RACE_VALUES),
        ("composition_failure", COMPOSITION_FAILURE),
        ("tcp_roundtrip", TCP_ROUNDTRIP),
        ("tcp_duplex", TCP_DUPLEX),
        ("network_errors", NETWORK_ERRORS),
        ("terminal_stream", TERMINAL_STREAM),
        ("timeout_drains_socket", TIMEOUT_DRAINS_SOCKET),
    ];

    #[test]
    fn stream_network_sources_pass_the_existing_checker() {
        for (name, body) in CASES {
            let mut sources = SourceMap::default();
            sources.add("main.dever", source(body));
            check(&sources).unwrap_or_else(|errors| {
                panic!(
                    "{name}: {}",
                    errors
                        .iter()
                        .map(|error| error.render(&sources))
                        .collect::<String>()
                )
            });
        }
    }

    #[test]
    fn stream_network_sources_emit_objects_for_six_targets() {
        for (name, body) in CASES {
            let ir = llvm::lower(&source(body));
            for target in [
                Target::LinuxX86_64,
                Target::LinuxAarch64,
                Target::MacosX86_64,
                Target::MacosAarch64,
                Target::WindowsX86_64,
                Target::WindowsAarch64,
            ] {
                let object = emit_object(&ir, target)
                    .unwrap_or_else(|error| panic!("{name}/{target:?}: {error}"));
                assert!(!object.is_empty(), "{name}/{target:?}");
            }
        }
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn nullable_memory_streams_early_stop_and_alias_close() {
        llvm_managed::boolean(&source(STREAMS));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn channel_stream_backpressure_with_suspending_handler() {
        llvm_managed::boolean(&source(CHANNEL_STREAM));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn ticks_suspend_then_early_stop_closes_aliases() {
        llvm_managed::boolean(&source(TICKS));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn parallel_stream_and_memory_workers_preserve_all_values() {
        llvm_managed::boolean(&source(PARALLEL_STREAM));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn parallel_stream_preserves_typed_owned_worker_failure() {
        llvm_managed::boolean(&source(PARALLEL_FAILURE));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn parallel_failure_interrupts_idle_producer_and_preserves_channel_alias() {
        llvm_managed::boolean(&source(IDLE_PARALLEL_FAILURE));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn synchronous_parallel_memory_dispatch_preserves_failure_and_ownership() {
        llvm_managed::boolean(&source(SYNC_PARALLEL));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn async_parallel_dispatch_supports_non_suspending_handlers() {
        llvm_managed::boolean(&source(ASYNC_SYNC_PARALLEL));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned temporary file"]
    fn synchronous_file_stream_parallel_failure_releases_workers_and_rows() {
        let directory = super::temp::TemporaryDirectory::new();
        let path = directory.path().join("parallel-source");
        std::fs::write(&path, b"abc").unwrap();
        llvm_managed::boolean(&source(
            &SYNC_FILE_PARALLEL.replace("FIXTURE_PATH", &format!("{:?}", path.to_str().unwrap())),
        ));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn timeout_preserves_owned_multi_output_and_some_null() {
        llvm_managed::boolean(&source(TIMEOUT_VALUES));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn race_preserves_owned_multi_output_and_drains_losers() {
        llvm_managed::boolean(&source(RACE_VALUES));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn zero_output_timeout_and_race_preserve_presence_and_cleanup() {
        llvm_managed::boolean(&source(UNIT_COMPOSITION));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn timeout_and_race_propagate_business_failure_without_fallback() {
        llvm_managed::boolean(&source(COMPOSITION_FAILURE));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback sockets"]
    fn tcp_read_write_eof_and_shared_resource_close() {
        llvm_managed::boolean(&source(TCP_DUPLEX));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback sockets"]
    fn tcp_chunks_collects_exact_bytes_across_short_reads_and_eof() {
        llvm_managed::boolean(&source(TCP_ROUNDTRIP));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback sockets"]
    fn invalid_network_inputs_preserve_original_failure_messages() {
        llvm_managed::boolean(&source(NETWORK_ERRORS));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback sockets"]
    fn chunks_and_connections_emit_one_terminal_failure_before_end() {
        llvm_managed::boolean(&source(TERMINAL_STREAM));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback sockets"]
    fn timeout_drains_pending_descendant_socket_before_fallback_returns() {
        llvm_managed::boolean(&source(TIMEOUT_DRAINS_SOCKET));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn zero_ticks_is_an_original_uncaptured_runtime_fault() {
        llvm_managed::runtime_fault(
            "public main() (values: AsyncStream<Int>) { values = dever.task.ticks(0) }",
            "duration must be positive",
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn zero_task_timeout_is_an_original_uncaptured_runtime_fault() {
        llvm_managed::runtime_fault(
            "pending() (answer: Bool) { dever.task.sleep(1000)\nanswer = false }\nfallback() (answer: Bool) { answer = false }\npublic main() (answer: Bool) { job = run(pending())\nanswer = timeout(job, 0, fallback) }",
            "duration must be positive",
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn timeout_and_race_keep_numeric_fault_kind_and_origin() {
        for body in [
            "job = run(broken(0))\nanswer = timeout(job, 500, fallback)",
            "job = run(broken(0))\nremaining = run(pending())\nanswer = race(job, remaining)",
        ] {
            let source = format!(
                "broken(denominator: Int) (answer: Int) {{ dever.task.sleep(1)\nanswer = 42 // denominator }}\npending() (answer: Int) {{ dever.task.sleep(1000)\nanswer = 0 }}\nfallback() (answer: Int) {{ answer = 9 }}\npublic main() (answer: Int) {{ {body} }}"
            );
            let origin = source.find("42 // denominator").unwrap();
            llvm_managed::execute(
                &source,
                "{ i64 }",
                &format!(
                    "  %failed = icmp eq i32 %status, 1
  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0
  %code = load i32, ptr %code_ptr
  %numeric = icmp eq i32 %code, 2
  %origin_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 1
  %origin_id = load i32, ptr %origin_ptr
  %span = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %origin_id, i32 1
  %start = load i64, ptr %span
  %original = icmp eq i64 %start, {origin}
  %depth_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 2
  %depth = load i32, ptr %depth_ptr
  %called = icmp uge i32 %depth, 1
  %kind_ok = and i1 %failed, %numeric
  %source_ok = and i1 %original, %called
  %passed = and i1 %kind_ok, %source_ok"
                ),
            );
        }
    }
}
