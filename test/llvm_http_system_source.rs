//! Checked-source protocol/system acceptance through the actual LLVM runtime archive.

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
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    const COMMON: &str = r#"some_h2(value: dever.http.Http2Limits) (answer: dever.http.Http2Limits?) { answer = value }
some_text(value: Text) (answer: Text?) { answer = value }
some_int(value: Int) (answer: Int?) { answer = value }
status(valid: true) (code: Int) { code = 207 }
status(valid: false) (code: Int) { code = 500 }
named(header: dever.http.Header, name: Text) (matches: Bool) { matches = header.name == name }
header_bytes(header: dever.http.Header, previous: List<Bytes>) (values: List<Bytes>) { values = append(previous, header.value) }
collect(event: dever.io.ReadEvent.Chunk(bytes), previous: Bytes) (next: Bytes) { next = dever.bytes.concat(previous, bytes) }
collect(event: dever.io.ReadEvent.Failed(message), previous: Bytes) (next: Bytes) { fail(dever.io.ReadEvent.Failed(message)) }
body(values: AsyncStream<dever.io.ReadEvent>) (bytes: Bytes) { bytes = reduce(collect, values, dever.bytes.from_text("")) }
request() (value: dever.http.Request) {
  value = dever.http.Request { method = "POST"
    target = "/echo?raw=1"
    headers = [dever.http.header("Host", "fixture.test"), dever.http.header("X-Check", "one"), dever.http.header("X-Check", "two")]
    body = dever.bytes.from_text("payload") }
}
route(request: dever.http.Request) (response: dever.http.Response) {
  values = reduce(header_bytes, filter(named, request.headers, "x-check"), [])
  valid = request.method == "POST" and request.target == "/echo?raw=1" and values == [dever.bytes.from_text("one"), dever.bytes.from_text("two")]
  response = dever.http.Response { status = status(valid)
    headers = [dever.http.header("X-Echo", "first"), dever.http.header("X-Echo", "second")]
    body = request.body }
}
good(response: dever.http.Response) (valid: Bool) {
  values = reduce(header_bytes, filter(named, response.headers, "x-echo"), [])
  valid = response.status == 207 and response.body == dever.bytes.from_text("payload") and values == [dever.bytes.from_text("first"), dever.bytes.from_text("second")]
}
"#;

    const HTTP_SEND: &str = r#"public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  port = dever.net.port(listener)
  server = run(dever.http.serve(route, listener, dever.http.default_limits()))
  response = dever.http.send("127.0.0.1", port, request(), dever.http.default_limits())
  answer = good(response)
  stop(server)
  dever.net.close_listener(listener)
}
"#;

    const HTTP_POOL: &str = r#"public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  port = dever.net.port(listener)
  limits = dever.http.default_limits()
  limits.http2 = HTTP2
  server = run(SERVE)
  client = dever.http.client(ORIGIN + int.to_text(port), TRUST, limits, dever.http.default_pool_limits())
  first = dever.http.request(client, request())
  first_good = good(first)
  second = dever.http.open(client, request())
  second_status = second.status
  second_body = body(second.body)
  input = channel(dever.io.ReadEvent, 2)
  send(input, dever.io.ReadEvent.Chunk(dever.bytes.from_text("pay")))
  send(input, dever.io.ReadEvent.Chunk(dever.bytes.from_text("load")))
  close(input)
  third_request = dever.http.StreamRequest { method = "POST"
    target = "/echo?raw=1"
    headers = request().headers
    body = stream(input) }
  third = dever.http.open_stream(client, third_request)
  third_status = third.status
  third_body = body(third.body)
  dever.http.close_client(client)
  stop(server)
  dever.net.close_listener(listener)
  answer = first_good and second_status == 207 and second_body == dever.bytes.from_text("payload") and third_status == 207 and third_body == dever.bytes.from_text("payload")
}
"#;

    const LIVE: &str = r#"live(request: dever.http.Request, reply: dever.system.HttpReply) () {
  LIVE_BODY
}
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  port = dever.net.port(listener)
  server = run(dever.http.serve_live(live, listener, dever.http.default_limits(), dever.http.default_live_limits()))
  response = dever.http.send("127.0.0.1", port, request(), dever.http.default_limits())
  answer = LIVE_CHECK
  stop(server)
  dever.net.close_listener(listener)
}
"#;

    const HANDLER_FAILURE: &str = r#"type Failure { error Broken(message: Text, history: List<Text>) }
fault_route(request: dever.http.Request) (response: dever.http.Response) {
  response = pick_route(request.target, request)
}
pick_route(target: "/fail", request: dever.http.Request) (response: dever.http.Response) {
  dever.task.sleep(1)
  fail(Failure.Broken("owned handler fault", ["original", "payload"]))
}
pick_route(target: other, request: dever.http.Request) (response: dever.http.Response) { response = route(request) }
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  port = dever.net.port(listener)
  server = run(dever.http.serve(fault_route, listener, dever.http.default_limits()))
  failed_request = request()
  failed_request.target = "/fail"
  failed = dever.http.send("127.0.0.1", port, failed_request, dever.http.default_limits())
  failed_status = failed.status
  response = dever.http.send("127.0.0.1", port, request(), dever.http.default_limits())
  answer = failed_status == 500 and good(response)
  stop(server)
  dever.net.close_listener(listener)
}
"#;

    const CONTEXT: &str = r#"type Context { label: Text
  tags: List<Text> }
first_text(value: Text) (text: Text) { text = value }
first_text(value: null) (text: Text) { text = "missing" }
context_route(request: dever.http.Request, context: Context) (response: dever.http.Response) {
  dever.task.sleep(1)
  response = dever.http.Response { status = 200
    headers = []
    body = dever.bytes.from_text(context.label + first_text(first(context.tags))) }
}
serve_context(listener: dever.system.Listener, context: Context) () {
  dever.system.http_serve(context_route, listener, dever.http.default_limits(), context)
}
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  port = dever.net.port(listener)
  context = Context { label = "initial"
    tags = ["-owned"] }
  server = run(serve_context(listener, context))
  context.label = "changed"
  response = dever.http.send("127.0.0.1", port, request(), dever.http.default_limits())
  answer = response.status == 200 and response.body == dever.bytes.from_text("initial-owned") and context.label == "changed"
  stop(server)
  dever.net.close_listener(listener)
}
"#;

    const ERRORS: &str = r#"bad_date(value: dever.time.DateResult.Read(date)) (valid: Bool) recover("fixture expects invalid calendar") { valid = false }
bad_date(value: dever.time.DateResult.Failed(message)) (valid: Bool) recover("fixture expects invalid calendar") { valid = message == "invalid calendar Date" }
bad_time(value: dever.time.TimeResult.Read(time)) (valid: Bool) recover("fixture expects invalid clock") { valid = false }
bad_time(value: dever.time.TimeResult.Failed(message)) (valid: Bool) recover("fixture expects invalid clock") { valid = message == "invalid clock Time" }
bad_tls(value: dever.tls.ClientResult.Ready(config)) (valid: Bool) recover("fixture expects invalid CA") { valid = false }
bad_tls(value: dever.tls.ClientResult.Failed(message)) (valid: Bool) recover("fixture expects invalid CA") { valid = message == "TLS PEM contains no certificates" }
bad_token(value: dever.crypto.TokenResult.Ready(secret)) (valid: Bool) recover("fixture expects invalid token length") { valid = false }
bad_token(value: dever.crypto.TokenResult.Failed(message)) (valid: Bool) recover("fixture expects invalid token length") { valid = message == "token length must be between 16 and 1024 bytes" }
public main() (answer: Bool) {
  answer = bad_date(result(dever.time.parse_date("2024-02-30"))) and bad_time(result(dever.time.parse_time("25:00:00"))) and bad_tls(result(dever.tls.client(dever.bytes.from_text("")))) and bad_token(result(dever.crypto.token(1)))
}
"#;

    const WEBSOCKET: &str = r#"message(state: dever.websocket.ReceiveState.Read(value)) (message: dever.websocket.Message) { message = value }
message(state: dever.websocket.ReceiveState.End) (message: dever.websocket.Message) { message = dever.websocket.Message.Text("unexpected end") }
echo(request: dever.http.Request, reply: dever.system.HttpReply) () {
  socket = dever.websocket.accept(reply, dever.websocket.default_limits())
  first = message(dever.websocket.receive(socket))
  dever.websocket.send(socket, first)
  second = message(dever.websocket.receive(socket))
  dever.websocket.send(socket, second)
  dever.websocket.close(socket, 1000, "done")
}
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  port = dever.net.port(listener)
  server = run(WS_SERVE)
  socket = WS_CONNECT
  dever.websocket.send(socket, dever.websocket.Message.Text("你好"))
  first = message(dever.websocket.receive(socket))
  first_ok = first == dever.websocket.Message.Text("你好")
  dever.websocket.send(socket, dever.websocket.Message.Binary(dever.bytes.from_ints([0, 255, 1])))
  second = message(dever.websocket.receive(socket))
  second_ok = second == dever.websocket.Message.Binary(dever.bytes.from_ints([0, 255, 1]))
  close_state = dever.websocket.receive(socket)
  ended = is_end(close_state)
  stop(server)
  dever.net.close_listener(listener)
  answer = first_ok and second_ok and ended
}
is_end(state: dever.websocket.ReceiveState.End) (yes: Bool) { yes = true }
is_end(state: dever.websocket.ReceiveState.Read(value)) (yes: Bool) { yes = false }
"#;

    const WEBSOCKET_STREAM: &str = r#"push_messages(request: dever.http.Request, reply: dever.system.HttpReply) () {
  socket = dever.websocket.accept(reply, dever.websocket.default_limits())
  dever.websocket.send(socket, dever.websocket.Message.Text("stream"))
  dever.websocket.send(socket, dever.websocket.Message.Binary(dever.bytes.from_ints([0, 255])))
  dever.websocket.send(socket, dever.websocket.Message.Ping(dever.bytes.from_ints([4])))
  dever.websocket.send(socket, dever.websocket.Message.Pong(dever.bytes.from_ints([5])))
  dever.websocket.close(socket, 1000, "done")
}
collect_message(event: dever.websocket.MessageEvent.Read(message), previous: List<dever.websocket.Message>) (next: List<dever.websocket.Message>) { next = append(previous, message) }
collect_message(event: dever.websocket.MessageEvent.Failed(message), previous: List<dever.websocket.Message>) (next: List<dever.websocket.Message>) { fail(dever.websocket.MessageEvent.Failed(message)) }
public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  port = dever.net.port(listener)
  server = run(dever.http.serve_live(push_messages, listener, dever.http.default_limits(), dever.http.default_live_limits()))
  socket = dever.websocket.connect("127.0.0.1", port, "/events", dever.websocket.default_limits())
  values = dever.websocket.messages(socket)
  alias = values
  messages = reduce(collect_message, values, [])
  remaining = reduce(collect_message, alias, [])
  stop(server)
  dever.net.close_listener(listener)
  answer = messages == [dever.websocket.Message.Text("stream"), dever.websocket.Message.Binary(dever.bytes.from_ints([0, 255])), dever.websocket.Message.Ping(dever.bytes.from_ints([4])), dever.websocket.Message.Pong(dever.bytes.from_ints([5]))] and length(remaining) == 0
}
"#;

    const TIME: &str = r#"public main() (answer: Bool) {
  value = dever.time.parse_datetime("2024-02-29T01:02:03.004+01:00")
  date = dever.time.parse_date("2024-02-29")
  time = dever.time.parse_time("01:02:03.004")
  delta = dever.time.duration(1000)
  later = dever.time.add(value, delta)
  previous = dever.time.subtract(later, delta)
  difference = dever.time.difference(later, value)
  now = dever.time.now()
  clock = dever.time.unix_millis()
  first = dever.time.monotonic_nanos()
  second = dever.time.monotonic_nanos()
  answer = dever.time.format_datetime(value) == "2024-02-29T00:02:03.004Z" and dever.time.format_date(date) == "2024-02-29" and dever.time.format_time(time) == "01:02:03.004" and previous == value and difference == delta and text.ends_with(dever.time.format_datetime(now), "Z") and clock > 0 and second >= first
}
"#;

    const UUID: &str = r#"type Identifier { value: Uuid }
to_text(value: null) (text: Text) { text = "none" }
to_text(value: Uuid) (text: Text) { text = dever.system.uuid_to_text(value) }
public main() (answer: Bool) {
  value = dever.system.uuid_parse("550E8400-E29B-41D4-A716-446655440000")
  missing = dever.system.uuid_parse("invalid")
  identifier = Identifier { value = "550e8400-e29b-41d4-a716-446655440000" }
  expected = Identifier { value = "550E8400-E29B-41D4-A716-446655440000" }
  answer = to_text(value) == "550e8400-e29b-41d4-a716-446655440000" and to_text(missing) == "none" and identifier.value == expected.value
}
"#;

    const CRYPTO: &str = r#"public main() (answer: Bool) {
  key = dever.crypto.token(16)
  alias = key
  bytes = dever.bytes.from_text("abc")
  digest = dever.crypto.sha256(bytes)
  first = dever.crypto.hmac_sha256(key, bytes)
  second = dever.crypto.hmac_sha256(alias, bytes)
  expected = dever.bytes.from_ints([186,120,22,191,143,1,207,234,65,65,64,222,93,174,34,35,176,3,97,163,150,23,122,156,180,16,255,97,242,0,21,173])
  answer = digest == expected and dever.bytes.length(first) == 32 and dever.crypto.constant_time_eq(first, second) and not dever.crypto.constant_time_eq(first, dever.bytes.from_text("different"))
}
"#;

    const PASSWORD: &str = r#"public main() (answer: Bool) {
  key = dever.crypto.token(16)
  wrong = dever.crypto.token(16)
  encoded = blocking(dever.crypto.password_hash(key))
  verified = blocking(dever.crypto.password_verify(key, encoded))
  rejected = blocking(dever.crypto.password_verify(wrong, encoded))
  answer = verified and not rejected and text.starts_with(encoded, "$argon2id$v=19$")
}
"#;

    const OUTPUT: &str = r#"public main() (answer: Bool) {
  dever.io.println("原生输出")
  fields = {"component" = "fixture"
    "quoted" = "line\n\"value\""}
  dever.log.debug("ready", fields)
  dever.log.info("ready", fields)
  dever.log.warn("ready", fields)
  dever.log.error("ready", fields)
  answer = true
}
"#;

    fn source(body: &str) -> String {
        format!("{COMMON}\n{body}")
    }

    fn pool(http2: bool, tls: bool) -> String {
        let serve = if tls {
            "dever.http.serve_tls(route, listener, limits, dever.tls.server(CERT, KEY))"
        } else {
            "dever.http.serve(route, listener, limits)"
        };
        certificates(
            &HTTP_POOL
                .replace(
                    "HTTP2",
                    if http2 {
                        "some_h2(dever.http.default_http2_limits())"
                    } else {
                        "null"
                    },
                )
                .replace("SERVE", serve)
                .replace(
                    "ORIGIN",
                    if tls {
                        "\"https://localhost:\""
                    } else {
                        "\"http://127.0.0.1:\""
                    },
                )
                .replace(
                    "TRUST",
                    if tls {
                        "dever.tls.client(ROOT)"
                    } else {
                        "dever.tls.system()"
                    },
                ),
        )
    }

    fn certificates(body: &str) -> String {
        body.replace(
            "CERT",
            &format!(
                "dever.bytes.from_text({:?})",
                include_str!("dever-tests/fixtures/tls/server.pem")
            ),
        )
        .replace(
            "KEY",
            &format!(
                "dever.bytes.from_text({:?})",
                include_str!("dever-tests/fixtures/tls/server-key.pem")
            ),
        )
        .replace(
            "ROOT",
            &format!(
                "dever.bytes.from_text({:?})",
                include_str!("dever-tests/fixtures/tls/root.pem")
            ),
        )
    }

    fn live(body: &str, check: &str) -> String {
        LIVE.replace("LIVE_BODY", body).replace("LIVE_CHECK", check)
    }

    fn websocket(tls: bool, url: bool) -> String {
        let serve = if tls {
            "dever.http.serve_live_tls(echo, listener, dever.http.default_limits(), dever.http.default_live_limits(), dever.tls.server(CERT, KEY))"
        } else {
            "dever.http.serve_live(echo, listener, dever.http.default_limits(), dever.http.default_live_limits())"
        };
        let connect = if tls {
            "dever.websocket.open(\"wss://localhost:\" + int.to_text(port) + \"/echo\", dever.tls.client(ROOT), dever.websocket.default_limits())"
        } else if url {
            "dever.websocket.open(\"ws://127.0.0.1:\" + int.to_text(port) + \"/echo\", dever.tls.system(), dever.websocket.default_limits())"
        } else {
            "dever.websocket.connect(\"127.0.0.1\", port, \"/echo\", dever.websocket.default_limits())"
        };
        certificates(
            &WEBSOCKET
                .replace("WS_SERVE", serve)
                .replace("WS_CONNECT", connect),
        )
    }

    fn cases() -> Vec<(&'static str, String)> {
        vec![
            ("http_send", HTTP_SEND.into()),
            ("http_pool", pool(false, false)),
            ("http2_pool", pool(true, false)),
            ("tls_pool", pool(false, true)),
            ("tls_http2_pool", pool(true, true)),
            (
                "live_respond",
                live(
                    "dever.http.respond(reply, route(request))",
                    "good(response)",
                ),
            ),
            (
                "live_chunks",
                live(
                    "dever.http.start(reply, 207, [])\n  dever.http.write(reply, dever.bytes.from_text(\"pay\"))\n  dever.task.sleep(1)\n  dever.http.write(reply, dever.bytes.from_text(\"load\"))\n  dever.http.finish(reply)",
                    "response.status == 207 and response.body == dever.bytes.from_text(\"payload\")",
                ),
            ),
            (
                "sse",
                live(
                    "dever.sse.start(reply, [])\n  dever.sse.send(reply, dever.sse.Event { event = \"update\"\n    data = \"first\\nsecond\\n\"\n    id = some_text(\"42\")\n    retry_ms = some_int(123) })\n  dever.http.finish(reply)",
                    "response.status == 200 and response.body == dever.bytes.from_text(\"event: update\\nid: 42\\nretry: 123\\ndata: first\\ndata: second\\ndata: \\n\\n\")",
                ),
            ),
            ("handler_failure", HANDLER_FAILURE.into()),
            ("context", CONTEXT.into()),
            ("ws_connect", websocket(false, false)),
            ("ws_open", websocket(false, true)),
            ("wss_open", websocket(true, true)),
            ("ws_messages", WEBSOCKET_STREAM.into()),
            ("time", TIME.into()),
            ("uuid", UUID.into()),
            ("crypto", CRYPTO.into()),
            ("password", PASSWORD.into()),
            ("output", OUTPUT.into()),
            ("errors", ERRORS.into()),
            (
                "arguments",
                "public main() (answer: Bool) { answer = length(dever.process.arguments()) == 0 }"
                    .into(),
            ),
        ]
    }

    #[test]
    fn protocol_system_sources_pass_the_existing_checker() {
        for (name, body) in cases() {
            let mut sources = SourceMap::default();
            sources.add("main.dever", source(&body));
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
    fn protocol_system_sources_emit_objects_for_six_targets() {
        for (name, body) in cases() {
            let ir = llvm::lower(&source(&body));
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

    fn run_case(name: &str) {
        let body = cases()
            .into_iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap()
            .1;
        llvm_managed::boolean(&source(&body));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn single_http_request_preserves_headers_body_and_status() {
        run_case("http_send");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn pooled_http_requests_and_stream_upload_download() {
        run_case("http_pool");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn pooled_http2_requests_and_stream_upload_download() {
        run_case("http2_pool");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn tls_verifies_local_certificate_and_streams_http() {
        run_case("tls_pool");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn tls_negotiates_http2_and_streams_upload_download() {
        run_case("tls_http2_pool");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn live_reply_respond_preserves_response() {
        run_case("live_respond");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn live_reply_start_write_finish_preserves_chunks() {
        run_case("live_chunks");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn sse_preserves_nullable_metadata_and_multiline_data() {
        run_case("sse");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn owned_handler_failure_is_a_500_then_service_continues() {
        run_case("handler_failure");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn static_suspending_handler_clones_owned_context_before_initial_suspend() {
        run_case("context");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn websocket_connect_echoes_text_binary_and_natural_end() {
        run_case("ws_connect");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn websocket_url_open_echoes_text_binary_and_natural_end() {
        run_case("ws_open");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn secure_websocket_uses_local_tls_trust() {
        run_case("wss_open");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback"]
    fn websocket_message_stream_keeps_all_typed_variants_and_closes_aliases() {
        run_case("ws_messages");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn calendar_duration_and_clocks_keep_runtime_semantics() {
        run_case("time");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn uuid_parse_null_and_canonical_text() {
        run_case("uuid");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn crypto_secret_alias_sha256_hmac_and_comparison() {
        run_case("crypto");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn recoverable_system_errors_keep_the_original_messages() {
        run_case("errors");
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and bounded crypto workers"]
    fn password_hash_verify_uses_the_existing_blocking_boundary() {
        llvm_managed::boolean_bounded(&source(PASSWORD), 2, Duration::from_secs(45), &[]);
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn process_arguments_exclude_executable_and_keep_unicode() {
        llvm_managed::boolean_bounded(
            "public main() (answer: Bool) { answer = dever.process.arguments() == [\"first\", \"中文\"] }",
            64,
            Duration::from_secs(5),
            &["first", "中文"],
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn stdout_and_structured_logs_are_flushed_before_native_return() {
        let output =
            llvm_managed::boolean_bounded(&source(OUTPUT), 64, Duration::from_secs(5), &[]);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "原生输出\n".repeat(65)
        );
        let record = |level: &str| {
            format!(
                "{{\"level\":\"{level}\",\"message\":\"ready\",\"fields\":{{\"component\":\"fixture\",\"quoted\":\"line\\n\\\"value\\\"\"}}}}\n"
            )
        };
        let expected = ["debug", "info", "warn", "error"]
            .map(record)
            .concat()
            .repeat(65);
        assert_eq!(String::from_utf8(output.stderr).unwrap(), expected);
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and independent owned loopback peer"]
    fn http_client_matches_independent_chunked_wire_and_repeated_headers() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            for _ in 0..65 {
                let mut socket = accept_before(&listener, deadline);
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let expected =
                    b"POST /echo HTTP/1.1\r\nhost: example.test\r\ncontent-length: 1\r\n\r\nx";
                let mut request = vec![0; expected.len()];
                socket.read_exact(&mut request).unwrap();
                assert_eq!(request, expected);
                for chunk in [
                    b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\nSet-Cookie: a=1\r\n"
                        .as_slice(),
                    b"Set-Cookie: b=2\r\n\r\n1\r\na\r\n",
                    b"2\r\nbc\r\n0\r\n\r\n",
                ] {
                    socket.write_all(chunk).unwrap();
                }
            }
        });
        let source = source(&format!(
            r#"public main() (answer: Bool) {{
  request = dever.http.Request {{ method = "POST"
    target = "/echo"
    headers = [dever.http.header("Host", "example.test")]
    body = dever.bytes.from_text("x") }}
  response = dever.http.send("127.0.0.1", {port}, request, dever.http.default_limits())
  values = reduce(header_bytes, filter(named, response.headers, "set-cookie"), [])
  answer = response.status == 201 and response.body == dever.bytes.from_text("abc") and values == [dever.bytes.from_text("a=1"), dever.bytes.from_text("b=2")]
}}
"#
        ));
        // Always join our peer, including when source lowering or execution fails.
        let execution = std::panic::catch_unwind(|| llvm_managed::boolean(&source));
        let peer_result = peer.join();
        if let Err(failure) = execution {
            std::panic::resume_unwind(failure);
        }
        peer_result.unwrap();
    }

    fn accept_before(listener: &TcpListener, deadline: std::time::Instant) -> TcpStream {
        loop {
            match listener.accept() {
                Ok((socket, _)) => return socket,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(error) => panic!("owned HTTP peer accept failed: {error}"),
            }
        }
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive and owned loopback listener"]
    fn invalid_server_limits_are_an_uncaptured_original_runtime_fault() {
        llvm_managed::runtime_fault(
            &source(
                r#"public main() (answer: Bool) {
  listener = dever.net.listen("127.0.0.1", 0)
  limits = dever.http.default_limits()
  limits.header_bytes = 1
  dever.http.serve(route, listener, limits)
  answer = false
}
"#,
            ),
            "HTTP header_bytes must be at least 8192",
        );
    }
}
