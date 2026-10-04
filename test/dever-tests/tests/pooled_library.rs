mod support;

use support::{checked, rejected, sources, stdout};

const CONTEXT: &str = r#"public type Context { name: Text
  client: dever.system.HttpClient
  events: Channel<Int> }
route(request: dever.http.Request, context: Context) (response: dever.http.Response) {
  send(context.events, 1)
  response = dever.http.Response { status = 200
    headers = []
    body = dever.bytes.from_text(context.name) }
}
public serve(listener: dever.system.Listener, context: Context) () {
  dever.system.http_serve(route, listener, dever.http.default_limits(), context)
}
"#;

#[test]
fn typed_shared_context_and_async_effects_are_checked_before_emission() {
    let map = sources(CONTEXT);
    let api = checked(&map).api_snapshot();
    let serve = api
        .lines()
        .find(|line| line.starts_with("function main.serve"))
        .unwrap();
    assert!(
        serve.contains("network") && serve.contains("concurrency"),
        "{serve}"
    );
    rejected(
        &CONTEXT.replace("context: Context) (response", "context: Text) (response"),
        "signature",
    );
    rejected(
        &CONTEXT.replace("client: dever.system.HttpClient", "client: Stream<Int>"),
        "transferable",
    );
    rejected(
        &CONTEXT.replace(
            "  send(context.events, 1)",
            "  dever.io.println(context.name)",
        ),
        "cannot run inline",
    );
}

#[test]
fn stream_construction_does_not_hide_unhandled_failures() {
    rejected(
        "type Result { Ok\nerror Failed(message: Text) }\nfail_work() () { fail(Result.Failed(\"bad\")) }\nwork() (value: Result) { value = result(fail_work()) }\nmain() () { values = stream([work()])\nclose(values) }",
        "failure",
    );
    rejected(
        "work() (value: dever.time.SleepResult) { value = result(dever.task.sleep(1)) }\nfallback() (value: dever.time.SleepResult) { value = dever.time.SleepResult.Done }\nmain() () { job = run(work())\nvalue = timeout(job, 10, fallback) }",
        "failure",
    );
}

fn native_source(http2: bool) -> String {
    let root = include_str!("../fixtures/tls/root.pem");
    let cert = include_str!("../fixtures/tls/server.pem");
    let key = include_str!("../fixtures/tls/server-key.pem");
    let program = r#"type Context { prefix: Text
  client: dever.system.HttpClient
  hits: Channel<Int> }

request(target: Text) (value: dever.http.Request) {
  value = dever.http.Request { method = "GET"
    target = target
    headers = []
    body = dever.bytes.from_text("") }
}
serve(listener: dever.system.Listener, tls: dever.system.ServerTls, context: Context) () {
  limits = http_limits()
  # HTTP/1 的两条池连接之外，还需要一个独立 WSS 连接名额。
  limits.connections = 3
  dever.system.http_serve_live_tls(route, listener, limits, dever.http.default_live_limits(), tls, context)
}
copy_body(body: Bytes) (result: Bytes) { result = body }
route(request: dever.http.Request, reply: dever.system.HttpReply, context: Context) () {
  child = run(copy_body(request.body))
  request.body = wait(child)
  send(context.hits, 1)
  path(request.target, request.body, reply, context)
}
path(target: "/proxy", body: Bytes, reply: dever.system.HttpReply, context: Context) () recover("test route reports failures") {
  proxy(result(dever.http.request(context.client, request("/value"))), reply)
}
path(target: "/events", body: Bytes, reply: dever.system.HttpReply, context: Context) () recover("test route reports failures") {
  dever.sse.start(reply, [])
  dever.sse.send(reply, dever.sse.Event { event = ""
    data = context.prefix
    id = null
    retry_ms = null })
}
path(target: "/upload", body: Bytes, reply: dever.system.HttpReply, context: Context) () recover("test route reports failures") {
  dever.http.respond(reply, dever.http.Response { status = 200
    headers = []
    body = body })
}
path(target: "/ws", body: Bytes, reply: dever.system.HttpReply, context: Context) () recover("test route reports failures") {
  upgraded(result(dever.websocket.accept(reply, dever.websocket.default_limits())))
}
path(target: other, body: Bytes, reply: dever.system.HttpReply, context: Context) () recover("test route reports failures") {
  dever.http.respond(reply, dever.http.Response { status = 200
    headers = []
    body = dever.bytes.from_text(context.prefix) })
}
proxy(result: dever.http.ResponseResult.Read(response), reply: dever.system.HttpReply) () recover("test proxy reports failure") {
  response.headers = []
  dever.http.respond(reply, response)
}
proxy(result: dever.http.ResponseResult.Failed(message), reply: dever.system.HttpReply) () recover("test proxy reports failure") {
  dever.http.respond(reply, dever.http.Response { status = 502
    headers = []
    body = dever.bytes.from_text(message) })
}
upgraded(result: dever.websocket.ConnectResult.Connected(socket)) () recover("test WebSocket policy") {
  each(echo, dever.websocket.messages(socket), socket)
}
upgraded(result: dever.websocket.ConnectResult.Failed(message)) () recover("test WebSocket policy") {}
echo(event: dever.websocket.MessageEvent.Read(message), socket: dever.system.WebSocket) () recover("test WebSocket policy") {
  dever.websocket.send(socket, message)
}
echo(event: dever.websocket.MessageEvent.Failed(message), socket: dever.system.WebSocket) () recover("test WebSocket policy") {}
received(result: dever.websocket.ReceiveResult.Done(state)) (ok: Bool) recover("test WebSocket receive") { ok = received_state(state) }
received(result: other) (ok: Bool) recover("test WebSocket receive") { ok = false }
received_state(state: dever.websocket.ReceiveState.Read(message)) (ok: Bool) { ok = message_ok(message) }
received_state(state: other) (ok: Bool) { ok = false }
message_ok(message: dever.websocket.Message.Text(text)) (ok: Bool) { ok = text == "native" }
message_ok(message: other) (ok: Bool) { ok = false }
connected(result: dever.websocket.ConnectResult.Connected(socket)) (ok: Bool) recover("test WebSocket client") {
  dever.websocket.send(socket, dever.websocket.Message.Text("native"))
  ok = received(result(dever.websocket.receive(socket)))
  dever.websocket.close(socket, 1000, "done")
}
connected(result: dever.websocket.ConnectResult.Failed(message)) (ok: Bool) recover("test WebSocket client") { ok = false }
buffered(result: dever.http.ResponseResult.Read(response), expected: Text) (ok: Bool) recover("test buffered client") {
  ok = response.status == 200 and response.body == dever.bytes.from_text(expected)
}
buffered(result: dever.http.ResponseResult.Failed(message), expected: Text) (ok: Bool) recover("test buffered client") { ok = false }
chunk(event: dever.io.ReadEvent.Chunk(bytes), state: Bytes) (result: Bytes) recover("test stream failure") { result = dever.bytes.concat(state, bytes) }
chunk(event: dever.io.ReadEvent.Failed(message), state: Bytes) (result: Bytes) recover("test stream failure") { result = dever.bytes.from_text("stream failed") }
streamed(result: dever.http.StreamResponseResult.Read(response), expected: Text) (ok: Bool) recover("test streamed client") {
  body = reduce(chunk, response.body, dever.bytes.from_text(""))
  ok = response.status == 200 and body == dever.bytes.from_text(expected)
}
streamed(result: dever.http.StreamResponseResult.Failed(message), expected: Text) (ok: Bool) recover("test streamed client") { ok = false }
add(value: Int, state: Int) (sum: Int) { sum = state + value }
stopped() () {}

ready(result: dever.http.ClientResult.Ready(client), listener: dever.system.Listener, identity: dever.system.ServerTls, trust: dever.system.ClientTls, port: Int) (ok: Bool) recover("test client lifecycle") {
  hits = channel(Int, 16)
  context = Context { prefix = "native"
    client = client
    hits = hits }
  server = run(serve(listener, identity, context))
  proxy = buffered(result(dever.http.request(client, request("/proxy"))), "native")
  direct = buffered(result(dever.http.request(client, request("/value"))), "native")
  upload = dever.http.StreamRequest { method = "POST"
    target = "/upload"
    headers = []
    body = stream([dever.io.ReadEvent.Chunk(dever.bytes.from_text("ab")), dever.io.ReadEvent.Chunk(dever.bytes.from_text("cd"))]) }
  uploaded = streamed(result(dever.http.open_stream(client, upload)), "abcd")
  events = streamed(result(dever.http.open(client, request("/events"))), "data: native\n\n")
  socket = connected(result(dever.websocket.open("wss://localhost:" + int.to_text(port) + "/ws", trust, dever.websocket.default_limits())))
  dever.net.close_listener(listener)
  timeout(server, 1000, stopped)
  dever.http.close_client(client)
  close(hits)
  count = reduce(add, stream(hits), 0)
  ok = proxy and direct and uploaded and events and socket and count == 6
}
ready(result: dever.http.ClientResult.Failed(message), listener: dever.system.Listener, identity: dever.system.ServerTls, trust: dever.system.ClientTls, port: Int) (ok: Bool) recover("test client lifecycle") { ok = false }
bound(result: dever.net.PortResult.Bound(port), listener: dever.system.Listener, identity: dever.system.ServerTls, trust: dever.system.ClientTls) (ok: Bool) recover("test startup") {
  limits = http_limits()
  pool = dever.http.default_pool_limits()
  pool.chunk_bytes = 2
  ok = ready(result(dever.http.client("https://localhost:" + int.to_text(port), trust, limits, pool)), listener, identity, trust, port)
}
bound(result: dever.net.PortResult.Failed(message), listener: dever.system.Listener, identity: dever.system.ServerTls, trust: dever.system.ClientTls) (ok: Bool) recover("test startup") { ok = false }
listening(result: dever.net.ListenResult.Listening(listener), identity: dever.system.ServerTls, trust: dever.system.ClientTls) (ok: Bool) recover("test startup") {
  ok = bound(result(dever.net.port(listener)), listener, identity, trust)
}
listening(result: dever.net.ListenResult.Failed(message), identity: dever.system.ServerTls, trust: dever.system.ClientTls) (ok: Bool) recover("test startup") { ok = false }
identity(result: dever.tls.ServerResult.Ready(config), trust: dever.system.ClientTls) (ok: Bool) recover("test TLS identity") {
  ok = listening(result(dever.net.listen("127.0.0.1", 0)), config, trust)
}
identity(result: dever.tls.ServerResult.Failed(message), trust: dever.system.ClientTls) (ok: Bool) recover("test TLS identity") { ok = false }
trusted(result: dever.tls.ClientResult.Ready(config)) (ok: Bool) recover("test TLS trust") {
  ok = identity(result(dever.tls.server(dever.bytes.from_text(CERT_PEM), dever.bytes.from_text(KEY_PEM))), config)
}
trusted(result: dever.tls.ClientResult.Failed(message)) (ok: Bool) recover("test TLS trust") { ok = false }
public main() (ok: Bool) { ok = trusted(result(dever.tls.client(dever.bytes.from_text(ROOT_PEM)))) }
"#;
    let mut program = program
        .replace("CERT_PEM", &format!("{cert:?}"))
        .replace("KEY_PEM", &format!("{key:?}"))
        .replace("ROOT_PEM", &format!("{root:?}"));
    let (connections, protocol) = if http2 {
        (1, "dever.http.default_http2_limits()")
    } else {
        (2, "null")
    };
    program.push_str(&format!(
        r#"
http_limits() (limits: dever.http.Limits) {{
  limits = dever.http.default_limits()
  limits.connections = {connections}
  limits.http2 = {protocol}
}}
"#
    ));
    if http2 {
        // 嵌套 /proxy -> /value 必须复用唯一连接；普通 WSS 另由 HTTP/1 变体覆盖。
        program = program.replace(
            "socket = connected(result(dever.websocket.open(\"wss://localhost:\" + int.to_text(port) + \"/ws\", trust, dever.websocket.default_limits())))",
            "socket = true",
        ).replace("count == 6", "count == 5");
    }
    program
}

#[test]
fn native_source_checks_and_formats_with_tls_resources_and_streams() {
    for http2 in [false, true] {
        let map = sources(&native_source(http2));
        checked(&map);
        let formatted = dever_core::format::format(&map.files()[0]).unwrap();
        let map = sources(&formatted);
        checked(&map);
        assert_eq!(
            dever_core::format::format(&map.files()[0]).unwrap(),
            formatted
        );
    }
}

#[test]
fn native_tls_pool_context_upload_sse_websocket_and_graceful_shutdown() {
    assert_eq!(stdout(&native_source(false)), "ok = true\n");
}

#[test]
fn native_http2_tls_multiplexed_proxy_upload_sse_and_child_tasks() {
    assert_eq!(stdout(&native_source(true)), "ok = true\n");
}

#[test]
fn backend_example_checks_and_formats() {
    let map = sources(include_str!(
        "../../../examples/old/dever/backend/module/main.dever"
    ));
    checked(&map);
    let first = dever_core::format::format(&map.files()[0]).unwrap();
    let map = sources(&first);
    checked(&map);
    assert_eq!(dever_core::format::format(&map.files()[0]).unwrap(), first);
}
