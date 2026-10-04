mod support;
use support::{checked, rejected, sources, stdout};

const ROUTE: &str = r#"
route(request: dever.http.Request, reply: dever.system.HttpReply) () recover("connection policy") {
  dever.sse.start(reply, [])
  dever.sse.send(reply, dever.sse.Event { event = "tick"
    data = "hello"
    id = "1"
    retry_ms = 1000 })
}
public start(listener: dever.system.Listener) () {
  dever.http.serve_live(route, listener, dever.http.default_limits(), dever.http.default_live_limits())
}
"#;

#[test]
fn live_handler_contract_and_transitive_effects_are_checked() {
    let source = ROUTE;
    let map = sources(source);
    let program = checked(&map);
    let api = program.api_snapshot();
    let start = api
        .lines()
        .find(|line| line.starts_with("function main.start"))
        .unwrap();
    assert!(
        start.contains("network")
            && start.contains("concurrency")
            && start.contains("connection policy"),
        "{start}"
    );
    rejected(
        &source.replace("dever.system.HttpReply) ()", "dever.system.Socket) ()"),
        "expected",
    );
    rejected(
        &source.replace(
            "  dever.sse.start",
            "  dever.io.println(\"blocked\")\n  dever.sse.start",
        ),
        "cannot run inline",
    );
}

#[test]
fn live_example_checks_and_formats_idempotently() {
    let source = include_str!("../../../examples/old/dever/live_http/module/main.dever");
    let map = sources(source);
    checked(&map);
    let first = dever_core::format::format(&map.files()[0]).unwrap();
    let second = sources(&first);
    assert_eq!(
        dever_core::format::format(&second.files()[0]).unwrap(),
        first
    );
}

#[test]
fn native_live_server_and_client_cover_stream_sse_and_websocket_resources() {
    let example = include_str!("../../../examples/old/dever/live_http/module/main.dever");
    let routes = example.split("\nmain() () {").next().unwrap();
    let program = format!(
        r#"{routes}
checked_response(result: dever.http.ResponseResult.Read(response), expected: Bytes) (ok: Bool) recover("test reports response errors") {{ ok = response.status == 200 and response.body == expected }}
checked_response(result: dever.http.ResponseResult.Failed(message), expected: Bytes) (ok: Bool) recover("test reports response errors") {{ ok = false }}
fetch(port: Int, target: Text, expected: Bytes) (ok: Bool) {{
  request = dever.http.Request {{ method = "GET"
    target = target
    headers = [dever.http.header("Host", "test")]
    body = dever.bytes.from_text("") }}
  ok = checked_response(result(dever.http.send("127.0.0.1", port, request, dever.http.default_limits())), expected)
}}
received(result: dever.websocket.ReceiveResult.Done(state)) (ok: Bool) recover("test reports socket errors") {{ ok = received_state(state) }}
received(result: other) (ok: Bool) recover("test reports socket errors") {{ ok = false }}
received_state(state: dever.websocket.ReceiveState.Read(message)) (ok: Bool) {{ ok = text_message(message) }}
received_state(state: other) (ok: Bool) {{ ok = false }}
text_message(message: dever.websocket.Message.Text(value)) (ok: Bool) {{ ok = value == "native" }}
text_message(message: other) (ok: Bool) {{ ok = false }}
connected(result: dever.websocket.ConnectResult.Connected(socket)) (ok: Bool) recover("test reports socket errors") {{
  dever.websocket.send(socket, dever.websocket.Message.Text("native"))
  ok = received(result(dever.websocket.receive(socket)))
  dever.websocket.close(socket, 1000, "done")
}}
connected(result: dever.websocket.ConnectResult.Failed(message)) (ok: Bool) recover("test reports socket errors") {{ ok = false }}
bound(result: dever.net.PortResult.Bound(port), listener: dever.system.Listener) (ok: Bool) recover("test reports startup errors") {{
  server = run(dever.http.serve_live(route, listener, dever.http.default_limits(), dever.http.default_live_limits()))
  normal = fetch(port, "/", dever.bytes.from_text("Dever: /stream, /events, /ws"))
  stream = fetch(port, "/stream", dever.bytes.from_text("first\nsecond\n"))
  events = fetch(port, "/events", dever.bytes.from_text("event: tick\nid: 1\nretry: 1000\ndata: 1\n\nevent: tick\nid: 2\nretry: 1000\ndata: 2\n\nevent: tick\nid: 3\nretry: 1000\ndata: 3\n\n"))
  websocket = connected(result(dever.websocket.connect("127.0.0.1", port, "/ws", dever.websocket.default_limits())))
  ok = normal and stream and events and websocket
  stop(server)
  dever.net.close_listener(listener)
}}
bound(result: dever.net.PortResult.Failed(message), listener: dever.system.Listener) (ok: Bool) recover("test reports startup errors") {{ ok = false }}
listening(result: dever.net.ListenResult.Listening(listener)) (ok: Bool) recover("test reports startup errors") {{ ok = bound(result(dever.net.port(listener)), listener) }}
listening(result: dever.net.ListenResult.Failed(message)) (ok: Bool) recover("test reports startup errors") {{ ok = false }}
main() (ok: Bool) {{ ok = listening(result(dever.net.listen("127.0.0.1", 0))) }}
"#
    );
    assert_eq!(stdout(&program), "ok = true\n");
}
