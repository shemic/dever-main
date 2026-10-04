mod support;

use support::{checked, run, sources, stdout};

#[test]
fn typed_core_compiles_to_native_values_with_named_outputs() {
    let output = stdout(include_str!(
        "../../../examples/old/dever/core/module/main.dever"
    ));
    assert_eq!(
        output,
        concat!(
            "original = 1\nchanged = 99\nchoice = 7\ncategory = large\nexact = true\n",
            "quotient = 2.5\nthird = 0.3333333333333333333333333333333333\nrounded = 2.34\n",
            "integer_division = -2\nremainder = -1\nselected = [3, 6]\nfound = 3\ntotal = 18\n",
            "empty_total = 0\nnames = ABCabc\nordered_keys = [second, first]\nmaps_equal = false\n",
            "nan_equal = false\nfinite = true\noptionals = [null, 1, null]\nkey_value = 2\nidentity = true\n",
        )
    );
}

#[test]
fn source_field_order_actions_and_short_circuiting_are_preserved() {
    let output = stdout(
        r#"type Pair {
  first: Int
  second: Int
}
mark(label: Text, value: Int) (answer: Int) {
  dever.io.println(label)
  answer = value
}
probe(value: Int) (answer: Bool) {
  dever.io.println("probe")
  answer = value == 2
}
show(value: Int) () {
  dever.io.println("each")
}
public main() (answer: Int?, first_value: Int?, short: Bool, pair_value: Int) {
  pair = Pair {
    second = mark("second", 2)
    first = mark("first", 1)
  }
  pair_value = pair.first + pair.second
  each(show, [1, 2])
  answer = find(probe, [1, 2, 3])
  short = false and probe(10) or true
  first_value = first([])
}
"#,
    );
    assert_eq!(
        output,
        "second\nfirst\neach\neach\nprobe\nprobe\nanswer = 2\nfirst_value = null\nshort = true\npair_value = 3\n"
    );
}

#[test]
fn nullable_variants_bind_payloads_and_optional_results_flatten() {
    let output = stdout(
        r#"type Result {
  Found(value: Int)
  Missing
}
choose(value: Result.Found(payload)) (answer: Int) { answer = payload }
choose(value: Result.Missing) (answer: Int) { answer = -1 }
choose(value: null) (answer: Int) { answer = -2 }
present(value: Int) (answer: Bool) { answer = true }
present(value: null) (answer: Bool) { answer = false }
select(values: List<Int?>) (answer: Int?) { answer = first(values) }
public main() (payload: Int, missing: Int, absent: Int, first_value: Int?, found: Int?, label: Int?) {
  payload = choose(Result.Found(7))
  missing = choose(Result.Missing)
  absent = choose(null)
  first_value = select([null, 1])
  found = find(present, [null, 1, 2])
  labels = { "first" = optional(4) }
  label = get(labels, "first")
}
optional(value: Int) (answer: Int?) { answer = value }
"#,
    );
    assert_eq!(
        output,
        "payload = 7\nmissing = -1\nabsent = -2\nfirst_value = null\nfound = 1\nlabel = 4\n"
    );
}

#[test]
fn dynamic_numeric_faults_retain_the_original_source_location() {
    for (function, arguments, message) in [
        (
            "divide(left: Int, right: Int) (answer: Int) { answer = left // right }",
            "1, 0",
            "Int division by zero",
        ),
        (
            "divide(left: Decimal, right: Decimal) (answer: Decimal) { answer = left / right }",
            "1.0, 0.0",
            "Decimal division by zero",
        ),
        (
            "divide(left: Int, right: Int) (answer: Int) { answer = left + right }",
            "9223372036854775807, 1",
            "Int overflow",
        ),
    ] {
        let output = run(&format!(
            "{function}\npublic main() () {{ answer = divide({arguments}) }}"
        ));
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("main.dever:2:"), "{error}");
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn standard_bytes_are_normal_choice_results_and_not_dynamic_values() {
    let output = stdout(
        r#"decode(result: dever.bytes.DecodeResult.Decoded(text)) (answer: Text) recover("test observes decode failures in its assertions") { answer = text }
decode(result: dever.bytes.DecodeResult.Failed(message)) (answer: Text) recover("test observes decode failures in its assertions") { answer = "invalid" }
buffer(result: dever.bytes.BufferResult.Ready(bytes)) (answer: Bytes) recover("test observes buffer failures in its assertions") { answer = bytes }
buffer(result: dever.bytes.BufferResult.Failed(message)) (answer: Bytes) recover("test observes buffer failures in its assertions") { answer = dever.bytes.from_text(message) }
public main() (decoded: Text, invalid: Text, count: Int, byte: Int?, outside: Int?) {
  original = dever.bytes.from_text("abc")
  combined = dever.bytes.concat(original, dever.bytes.from_text("def"))
  sliced = buffer(result(dever.bytes.slice(combined, 1, 4)))
  decoded = decode(result(dever.bytes.to_text(sliced)))
  invalid = decode(result(dever.bytes.to_text(buffer(result(dever.bytes.from_ints([255]))))))
  count = dever.bytes.length(original)
  byte = dever.bytes.at(original, 1)
  outside = dever.bytes.at(original, -1)
}
"#,
    );
    assert_eq!(
        output,
        "decoded = bcd\ninvalid = invalid\ncount = 3\nbyte = 98\noutside = null\n"
    );
}

#[test]
fn official_file_source_compiles_with_explicit_resource_results() {
    let directory = support::temp::TemporaryDirectory::new();
    let path = directory.path().join("native.txt");
    let functions = r#"
created(result: dever.io.OpenResult.Opened(file), path: Text) (answer: Bool) recover("test observes created failures in its assertions") {
  alias = file
  written = write_ok(result(dever.io.write(file, dever.bytes.from_text("hello"))))
  closed = close_ok(result(dever.io.close(file)))
  rejected = write_failed(result(dever.io.write(alias, dever.bytes.from_text("bad"))))
  content = opened(result(dever.io.open(path)))
  answer = written and closed and rejected and content
}
created(result: dever.io.OpenResult.Failed(message), path: Text) (answer: Bool) recover("test observes created failures in its assertions") { answer = false }
opened(result: dever.io.OpenResult.Opened(file)) (answer: Bool) recover("test observes opened failures in its assertions") {
  read = read_ok(result(dever.io.read(file, 64)))
  closed = close_ok(result(dever.io.close(file)))
  answer = read and closed
}
opened(result: dever.io.OpenResult.Failed(message)) (answer: Bool) recover("test observes opened failures in its assertions") { answer = false }
read_ok(result: dever.io.ReadResult.Done(state)) (answer: Bool) recover("test observes read_ok failures in its assertions") { answer = read_state_ok(state) }
read_ok(result: other) (answer: Bool) recover("test observes read_ok failures in its assertions") { answer = false }
read_state_ok(state: dever.io.ReadState.Read(bytes)) (answer: Bool) { answer = decode_ok(result(dever.bytes.to_text(bytes))) }
read_state_ok(state: other) (answer: Bool) { answer = false }
decode_ok(result: dever.bytes.DecodeResult.Decoded(text)) (answer: Bool) recover("test observes decode_ok failures in its assertions") { answer = text == "hello" }
decode_ok(result: other) (answer: Bool) recover("test observes decode_ok failures in its assertions") { answer = false }
write_ok(result: dever.io.WriteResult.Done) (answer: Bool) recover("test observes write_ok failures in its assertions") { answer = true }
write_ok(result: other) (answer: Bool) recover("test observes write_ok failures in its assertions") { answer = false }
write_failed(result: dever.io.WriteResult.Failed(message)) (answer: Bool) recover("test observes write_failed failures in its assertions") { answer = true }
write_failed(result: other) (answer: Bool) recover("test observes write_failed failures in its assertions") { answer = false }
close_ok(result: dever.io.CloseResult.Closed) (answer: Bool) recover("test observes close_ok failures in its assertions") { answer = true }
close_ok(result: other) (answer: Bool) recover("test observes close_ok failures in its assertions") { answer = false }
"#;
    let source = format!(
        "public main() (result: Bool) {{ result = created(result(dever.io.create({path:?})), {path:?}) }}\n{functions}",
        path = path.to_str().unwrap()
    );
    assert_eq!(stdout(&source), "result = true\n");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "hello");
}

#[test]
fn official_tcp_source_runs_a_single_local_connection_and_closes_it() {
    let output = stdout(
        r#"public main() (result: Bool) { result = listening(result(dever.net.listen("127.0.0.1", 0))) }
listening(result: dever.net.ListenResult.Listening(listener)) (answer: Bool) recover("test observes listening failures in its assertions") {
  answer = bound(result(dever.net.port(listener)), listener)
}
listening(result: other) (answer: Bool) recover("test observes listening failures in its assertions") { answer = false }
bound(result: dever.net.PortResult.Bound(port), listener: dever.system.Listener) (answer: Bool) recover("test observes bound failures in its assertions") {
  answer = connected(result(dever.net.connect("127.0.0.1", port)), listener)
}
bound(result: other, listener: dever.system.Listener) (answer: Bool) recover("test observes bound failures in its assertions") { answer = false }
connected(result: dever.net.ConnectResult.Connected(client), listener: dever.system.Listener) (answer: Bool) recover("test observes connected failures in its assertions") {
  answer = accepted(result(dever.net.accept(listener)), client, listener)
}
connected(result: other, listener: dever.system.Listener) (answer: Bool) recover("test observes connected failures in its assertions") { answer = false }
accepted(result: dever.net.ConnectResult.Connected(server), client: dever.system.Socket, listener: dever.system.Listener) (answer: Bool) recover("test observes accepted failures in its assertions") {
  client_timeout = write_ok(result(dever.net.timeout(client, 1000)))
  server_timeout = write_ok(result(dever.net.timeout(server, 1000)))
  written = write_ok(result(dever.net.write(client, dever.bytes.from_text("x"))))
  received = read_ok(result(dever.net.read(server, 1)))
  client_closed = close_ok(result(dever.net.close(client)))
  server_closed = close_ok(result(dever.net.close(server)))
  listener_closed = close_ok(result(dever.net.close_listener(listener)))
  answer = client_timeout and server_timeout and written and received and client_closed and server_closed and listener_closed
}
accepted(result: other, client: dever.system.Socket, listener: dever.system.Listener) (answer: Bool) recover("test observes accepted failures in its assertions") { answer = false }
write_ok(result: dever.io.WriteResult.Done) (answer: Bool) recover("test observes write_ok failures in its assertions") { answer = true }
write_ok(result: other) (answer: Bool) recover("test observes write_ok failures in its assertions") { answer = false }
close_ok(result: dever.io.CloseResult.Closed) (answer: Bool) recover("test observes close_ok failures in its assertions") { answer = true }
close_ok(result: other) (answer: Bool) recover("test observes close_ok failures in its assertions") { answer = false }
read_ok(result: dever.io.ReadResult.Done(state)) (answer: Bool) recover("test observes read_ok failures in its assertions") { answer = read_state_ok(state) }
read_ok(result: other) (answer: Bool) recover("test observes read_ok failures in its assertions") { answer = false }
read_state_ok(state: dever.io.ReadState.Read(bytes)) (answer: Bool) { answer = bytes == dever.bytes.from_text("x") }
read_state_ok(state: other) (answer: Bool) { answer = false }
"#,
    );
    assert_eq!(output, "result = true\n");
}

#[test]
fn grouping_and_null_only_collections_do_not_change_semantics() {
    let output = stdout(
        r#"same(value: Int?) (answer: Bool) { answer = (null) == value and value == ((null)) }
public main() (size: Int, equal: Bool, first_value: Int?, mapped: List<Text>, appended: List<Int>, found: Int?) {
  values = [(null), 1]
  size = length(values)
  equal = same(null)
  first_value = first([null])
  mapped = each((text.trim), [" a ", " b "])
  appended = append(([]), 1)
  labels = put(({}), "a", 2)
  found = get(labels, "a")
}
"#,
    );
    assert_eq!(
        output,
        "size = 2\nequal = true\nfirst_value = null\nmapped = [a, b]\nappended = [1]\nfound = 2\n"
    );
}

#[test]
fn float_patterns_preserve_signed_zero_and_literal_context_is_symmetric() {
    let output = stdout(
        r#"sign(value: 0.0e0) (answer: Float) { answer = 1.0e0 / value }
sign(value: other) (answer: Float) { answer = 0.0e0 }
compare(value: Float) (answer: Bool) { answer = value < 0.0 and 0.0 > value and value == (-1.0) }
public main() (negative: Bool, comparison: Bool, ordered_sum: Float) {
  negative = sign(-0.0e0) < 0.0
  comparison = compare(-1.0e0)
  ordered_sum = sum([1.0e16, 1.0e0, -1.0e16])
}
"#,
    );
    assert_eq!(
        output,
        "negative = true\ncomparison = true\nordered_sum = 0\n"
    );
}

#[test]
fn reduce_bytes_and_forwarded_handlers_are_direct_native_operations() {
    let output = stdout(
        r#"add(value: Int, total: Int) (next: Int) { next = total + value }
until(value: Int, total: Int) (next: Int, stop: Bool) { next = total + value
stop = value == 2 }
increment(value: Int) (answer: Int) { answer = value + 1 }
apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }
forward(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = apply(route, value) }
public main() (list_total: Int, empty_total: Int, stopped: Int, bytes_total: Int, mapped: List<Int>, forwarded: Int) {
  list_total = reduce(add, [1, 2, 3], 0)
  empty_total = reduce(add, [], 7)
  stopped = reduce_until(until, [1, 2, 100], 0)
  bytes = dever.bytes.from_text("ABC")
  bytes_total = reduce(add, bytes, 0)
  mapped = each(increment, bytes)
  forwarded = forward(increment, 4)
}
"#,
    );
    assert_eq!(
        output,
        "list_total = 6\nempty_total = 7\nstopped = 3\nbytes_total = 198\nmapped = [66, 67, 68]\nforwarded = 5\n"
    );
}

#[test]
fn handler_codegen_emits_concrete_specializations_without_function_values() {
    let sources = sources(
        r#"increment(value: Int) (answer: Int) { answer = value + 1 }
decrement(value: Int) (answer: Int) { answer = value - 1 }
apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }
public main() (up: Int, down: Int) { up = apply(increment, 1)
down = apply(decrement, 1) }
"#,
    );
    let generated = dever_core::native::emit(&checked(&sources), &sources, "main.main").unwrap();
    assert_eq!(
        generated
            .lines()
            .filter(|line| line.starts_with("fn f") && line.contains('h'))
            .count(),
        2
    );
    assert!(!generated.contains("dyn Fn"));
    assert!(!generated.contains("fn("));
}

#[test]
fn bytes_reduce_codegen_iterates_bytes_without_materializing_a_list() {
    let sources = sources(
        r#"add(value: Int, total: Int) (next: Int) { next = total + value }
public main() (answer: Int) {
  bytes = dever.bytes.from_text("ABC")
  answer = reduce(add, bytes, 0)
}
"#,
    );
    let generated = dever_core::native::emit(&checked(&sources), &sources, "main.main").unwrap();
    assert!(generated.contains("for element in a0.values()"));
    assert!(generated.contains("i64::from(*element)"));
    assert!(!generated.contains("List::<i64>::new"));
}

#[test]
fn official_file_chunks_stop_and_resume_the_same_stream_alias() {
    let directory = support::temp::TemporaryDirectory::new();
    let path = directory.path().join("chunks.txt");
    std::fs::write(&path, "abcdef").unwrap();
    let source = format!(
        r#"decode(result: dever.bytes.DecodeResult.Decoded(text), state: Text) (next: Text) recover("test observes decode failures in its assertions") {{ next = state + text }}
decode(result: dever.bytes.DecodeResult.Failed(message), state: Text) (next: Text) recover("test observes decode failures in its assertions") {{ next = state }}
collect(event: dever.io.ReadEvent.Chunk(bytes), state: Text) (next: Text) recover("test observes collect failures in its assertions") {{ next = decode(result(dever.bytes.to_text(bytes)), state) }}
collect(event: dever.io.ReadEvent.Failed(message), state: Text) (next: Text) recover("test observes collect failures in its assertions") {{ next = state }}
first(event: dever.io.ReadEvent.Chunk(bytes), state: Text) (next: Text, stop: Bool) recover("test observes first failures in its assertions") {{ next = decode(result(dever.bytes.to_text(bytes)), state)
stop = true }}
first(event: dever.io.ReadEvent.Failed(message), state: Text) (next: Text, stop: Bool) recover("test observes first failures in its assertions") {{ next = state
stop = true }}
streaming(result: dever.io.ReadStreamResult.Streaming(stream)) (answer: Text) recover("test observes streaming failures in its assertions") {{ alias = stream
prefix = reduce_until(first, stream, "")
answer = reduce(collect, alias, prefix)
close(stream) }}
streaming(result: dever.io.ReadStreamResult.Failed(message)) (answer: Text) recover("test observes streaming failures in its assertions") {{ answer = message }}
opened(result: dever.io.OpenResult.Opened(file)) (answer: Text) recover("test observes opened failures in its assertions") {{ answer = streaming(result(dever.io.chunks(file, 2))) }}
opened(result: dever.io.OpenResult.Failed(message)) (answer: Text) recover("test observes opened failures in its assertions") {{ answer = message }}
public main() (answer: Text) {{ answer = opened(result(dever.io.open({path:?}))) }}
"#,
        path = path.to_str().unwrap()
    );
    assert_eq!(stdout(&source), "answer = abcdef\n");
}

#[test]
fn official_file_chunks_report_creation_failures_without_a_stream() {
    let directory = support::temp::TemporaryDirectory::new();
    let path = directory.path().join("creation-failure.txt");
    std::fs::write(&path, "x").unwrap();
    let source = format!(
        r#"failed(result: dever.io.ReadStreamResult.Streaming(stream)) (answer: Bool) recover("test observes failed failures in its assertions") {{ close(stream)
answer = false }}
failed(result: dever.io.ReadStreamResult.Failed(message)) (answer: Bool) recover("test observes failed failures in its assertions") {{ answer = true }}
opened(result: dever.io.OpenResult.Opened(file)) (invalid: Bool, closed: Bool) recover("test observes opened failures in its assertions") {{
  invalid = failed(result(dever.io.chunks(file, 0)))
  dever.io.close(file)
  closed = failed(result(dever.io.chunks(file, 1)))
}}
opened(result: dever.io.OpenResult.Failed(message)) (invalid: Bool, closed: Bool) recover("test observes opened failures in its assertions") {{ invalid = false
closed = false }}
public main() (invalid: Bool, closed: Bool) {{ result = opened(result(dever.io.open({path:?})))
invalid = result.invalid
closed = result.closed }}
"#,
        path = path.to_str().unwrap()
    );
    assert_eq!(stdout(&source), "invalid = true\nclosed = true\n");
}

#[test]
fn official_loopback_connections_and_socket_chunks_are_streamed() {
    let output = stdout(
        r#"type State {
  client: dever.system.Socket
  text: Text
}
decode(result: dever.bytes.DecodeResult.Decoded(text), state: State) (next: State) recover("test observes decode failures in its assertions") { next = State { client = state.client
text = state.text + text } }
decode(result: dever.bytes.DecodeResult.Failed(message), state: State) (next: State) recover("test observes decode failures in its assertions") { next = state }
collect(event: dever.io.ReadEvent.Chunk(bytes), state: State) (next: State) recover("test observes collect failures in its assertions") { next = decode(result(dever.bytes.to_text(bytes)), state) }
collect(event: dever.io.ReadEvent.Failed(message), state: State) (next: State) recover("test observes collect failures in its assertions") { next = state }
reading(result: dever.net.ReadStreamResult.Streaming(stream), state: State) (next: State) recover("test observes reading failures in its assertions") { next = reduce(collect, stream, state) }
reading(result: dever.net.ReadStreamResult.Failed(message), state: State) (next: State) recover("test observes reading failures in its assertions") { next = state }
accepted(result: dever.net.ConnectResult.Connected(server), state: State) (next: State, stop: Bool) recover("test observes accepted failures in its assertions") {
  dever.net.timeout(state.client, 1000)
  dever.net.timeout(server, 1000)
  dever.net.write(state.client, dever.bytes.from_text("xyz"))
  dever.net.close(state.client)
  next = reading(result(dever.net.chunks(server, 2)), state)
  stop = true
}
accepted(result: dever.net.ConnectResult.Failed(message), state: State) (next: State, stop: Bool) recover("test observes accepted failures in its assertions") { next = state
stop = true }
connected(result: dever.net.ConnectResult.Connected(client), listener: dever.system.Listener) (answer: Text) recover("test observes connected failures in its assertions") {
  stream = dever.net.connections(listener)
  state = State { client = client
text = "" }
  finished = reduce_until(accepted, stream, state)
  close(stream)
  answer = finished.text
}
connected(result: dever.net.ConnectResult.Failed(message), listener: dever.system.Listener) (answer: Text) recover("test observes connected failures in its assertions") { answer = message }
bound(result: dever.net.PortResult.Bound(port), listener: dever.system.Listener) (answer: Text) recover("test observes bound failures in its assertions") { answer = connected(result(dever.net.connect("127.0.0.1", port)), listener) }
bound(result: dever.net.PortResult.Failed(message), listener: dever.system.Listener) (answer: Text) recover("test observes bound failures in its assertions") { answer = message }
listening(result: dever.net.ListenResult.Listening(listener)) (answer: Text) recover("test observes listening failures in its assertions") { answer = bound(result(dever.net.port(listener)), listener) }
listening(result: dever.net.ListenResult.Failed(message)) (answer: Text) recover("test observes listening failures in its assertions") { answer = message }
public main() (answer: Text) { answer = listening(result(dever.net.listen("127.0.0.1", 0))) }
"#,
    );
    assert_eq!(output, "answer = xyz\n");
}

#[test]
fn each_consumes_a_stream_only_for_zero_output_actions() {
    let directory = support::temp::TemporaryDirectory::new();
    let path = directory.path().join("each.txt");
    std::fs::write(&path, "abcd").unwrap();
    let source = format!(
        r#"print(result: dever.bytes.DecodeResult.Decoded(text)) () recover("test observes print failures in its assertions") {{ dever.io.println(text) }}
print(result: dever.bytes.DecodeResult.Failed(message)) () recover("test observes print failures in its assertions") {{ dever.io.println("failed") }}
show(event: dever.io.ReadEvent.Chunk(bytes)) () recover("test observes show failures in its assertions") {{ print(result(dever.bytes.to_text(bytes))) }}
show(event: dever.io.ReadEvent.Failed(message)) () recover("test observes show failures in its assertions") {{ dever.io.println("failed") }}
streaming(result: dever.io.ReadStreamResult.Streaming(stream)) (answer: Bool) recover("test observes streaming failures in its assertions") {{ each(show, stream)
answer = true }}
streaming(result: dever.io.ReadStreamResult.Failed(message)) (answer: Bool) recover("test observes streaming failures in its assertions") {{ answer = false }}
opened(result: dever.io.OpenResult.Opened(file)) (answer: Bool) recover("test observes opened failures in its assertions") {{ answer = streaming(result(dever.io.chunks(file, 2))) }}
opened(result: dever.io.OpenResult.Failed(message)) (answer: Bool) recover("test observes opened failures in its assertions") {{ answer = false }}
public main() (answer: Bool) {{ answer = opened(result(dever.io.open({path:?}))) }}
"#,
        path = path.to_str().unwrap()
    );
    assert_eq!(stdout(&source), "ab\ncd\nanswer = true\n");
}
