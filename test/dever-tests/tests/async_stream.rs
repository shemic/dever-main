mod support;

use support::{checked, rejected, sources, stdout};

#[test]
fn tcp_server_example_checks_without_starting_a_service() {
    checked(&sources(include_str!(
        "../../../examples/old/dever/async_tcp/module/main.dever"
    )));
}

#[test]
fn async_stream_handlers_and_transfer_rules_are_checked() {
    let source = r#"visit(value: Int) () {}
public consume(route: handler(value: Int) (), stream: AsyncStream<Int>) () {
  each(route, stream)
  parallel_each(route, stream, 2)
  close(stream)
}"#;
    let input = sources(source);
    checked(&input);
    let formatted = dever_core::format::format(&input.files()[0]).unwrap();
    let roundtrip = sources(&formatted);
    checked(&roundtrip);
    assert_eq!(
        dever_core::format::format(&roundtrip.files()[0]).unwrap(),
        formatted
    );
    checked(&sources(
        "visit(value: Int) () {}\nconsume(stream: AsyncStream<Int>) () { each(visit, stream) }",
    ));
    checked(&sources(
        "visit(value: Int) () {}\nconsume(stream: AsyncStream<Int>) () { parallel_each(visit, stream, 2) }",
    ));
    rejected(
        "visit(value: Int) () { dever.io.println(int.to_text(value)) }\nconsume(stream: AsyncStream<Int>) () { each(visit, stream) }",
        "cannot run inline inside a suspending function",
    );
    rejected(
        "consume(stream: AsyncStream<Stream<Int>>) () { close(stream) }",
        "must be transferable between threads",
    );
    checked(&sources(
        "main() (result: dever.net.ListenResult) { result = result(dever.net.listen(\"127.0.0.1\", 0)) }",
    ));
}

#[test]
fn native_async_handler_forwarding_uses_bounded_stream_tasks() {
    let source = r#"handle(event: dever.net.ConnectResult.Connected(socket), stream: AsyncStream<dever.net.ConnectResult>) () recover("test observes network failures through the client read") {
  close(stream)
  dever.net.write(socket, dever.bytes.from_text("x"))
  dever.net.close(socket)
}
handle(event: dever.net.ConnectResult.Failed(message), stream: AsyncStream<dever.net.ConnectResult>) () recover("test observes network failures through the client read") { close(stream) }
dispatch(route: handler(event: dever.net.ConnectResult, stream: AsyncStream<dever.net.ConnectResult>) (), stream: AsyncStream<dever.net.ConnectResult>) () {
  parallel_each(route, stream, 2, stream)
}
received(result: dever.io.ReadResult.Done(state)) (answer: Bool) recover("test asserts the received result") { answer = received_state(state) }
received(result: other) (answer: Bool) recover("test asserts the received result") { answer = false }
received_state(state: dever.io.ReadState.Read(bytes)) (answer: Bool) { answer = bytes == dever.bytes.from_text("x") }
received_state(state: other) (answer: Bool) { answer = false }
connected(result: dever.net.ConnectResult.Connected(client), listener: dever.system.Listener) (answer: Bool) recover("test asserts the connection result") {
  dever.net.timeout(client, 1000)
  stream = dever.net.connections(listener)
  dispatch(handle, stream)
  answer = received(result(dever.net.read(client, 1)))
  dever.net.close(client)
  dever.net.close_listener(listener)
}
connected(result: other, listener: dever.system.Listener) (answer: Bool) recover("test asserts the connection result") { answer = false }
bound(result: dever.net.PortResult.Bound(port), listener: dever.system.Listener) (answer: Bool) recover("test asserts the bound result") { answer = connected(result(dever.net.connect("127.0.0.1", port)), listener) }
bound(result: other, listener: dever.system.Listener) (answer: Bool) recover("test asserts the bound result") { answer = false }
listening(result: dever.net.ListenResult.Listening(listener)) (answer: Bool) recover("test asserts the listen result") { answer = bound(result(dever.net.port(listener)), listener) }
listening(result: other) (answer: Bool) recover("test asserts the listen result") { answer = false }
public main() (answer: Bool) { answer = listening(result(dever.net.listen("127.0.0.1", 0))) }
"#;
    let input = sources(source);
    let generated = dever_core::native::emit(&checked(&input), &input, "main.main").unwrap();
    assert!(generated.contains("task::parallel_each_stream_typed("));
    assert_eq!(stdout(source), "answer = true\n");
}

#[test]
fn closed_listener_stream_yields_one_failure() {
    assert_eq!(
        stdout(
            r#"count(event: dever.net.ConnectResult.Failed(message), state: Int) (next: Int) recover("test observes count failures in its assertions") { next = state + 1 }
count(event: dever.net.ConnectResult.Connected(socket), state: Int) (next: Int) recover("test observes count failures in its assertions") { next = state + 100 }
listening(result: dever.net.ListenResult.Listening(listener)) (answer: Int) recover("test observes listening failures in its assertions") {
 stream = dever.net.connections(listener)
 dever.net.close_listener(listener)
 answer = reduce(count, stream, 0) + reduce(count, stream, 0)
}
listening(result: other) (answer: Int) recover("test observes listening failures in its assertions") { answer = -1 }
public main() (answer: Int) { answer = listening(result(dever.net.listen("127.0.0.1", 0))) }
"#
        ),
        "answer = 1\n",
    );
}

#[test]
fn native_socket_stream_each_executes_async_actions() {
    assert_eq!(
        stdout(
            r#"show(event: dever.io.ReadEvent.Chunk(bytes)) () recover("test observes show failures in its assertions") { blocking(dever.io.println(int.to_text(dever.bytes.length(bytes)))) }
show(event: dever.io.ReadEvent.Failed(message)) () recover("test observes show failures in its assertions") { blocking(dever.io.println("failure")) }
consume(result: dever.net.ReadStreamResult.Streaming(stream)) () recover("test observes consume failures in its assertions") { each(show, stream) }
consume(result: dever.net.ReadStreamResult.Failed(message)) () recover("test observes consume failures in its assertions") { blocking(dever.io.println("failure")) }
accepted(result: dever.net.ConnectResult.Connected(server), client: dever.system.Socket, listener: dever.system.Listener) (answer: Bool) recover("test observes accepted failures in its assertions") {
 dever.net.timeout(server, 1000)
 dever.net.timeout(client, 1000)
 dever.net.write(client, dever.bytes.from_text("x"))
 dever.net.close(client)
 consume(result(dever.net.chunks(server, 1)))
 dever.net.close(server)
 dever.net.close_listener(listener)
 answer = true
}
accepted(result: other, client: dever.system.Socket, listener: dever.system.Listener) (answer: Bool) recover("test observes accepted failures in its assertions") { answer = false }
connected(result: dever.net.ConnectResult.Connected(client), listener: dever.system.Listener) (answer: Bool) recover("test observes connected failures in its assertions") { answer = accepted(result(dever.net.accept(listener)), client, listener) }
connected(result: other, listener: dever.system.Listener) (answer: Bool) recover("test observes connected failures in its assertions") { answer = false }
bound(result: dever.net.PortResult.Bound(port), listener: dever.system.Listener) (answer: Bool) recover("test observes bound failures in its assertions") { answer = connected(result(dever.net.connect("127.0.0.1", port)), listener) }
bound(result: other, listener: dever.system.Listener) (answer: Bool) recover("test observes bound failures in its assertions") { answer = false }
listening(result: dever.net.ListenResult.Listening(listener)) (answer: Bool) recover("test observes listening failures in its assertions") { answer = bound(result(dever.net.port(listener)), listener) }
listening(result: other) (answer: Bool) recover("test observes listening failures in its assertions") { answer = false }
public main() (answer: Bool) { answer = listening(result(dever.net.listen("127.0.0.1", 0))) }
"#
        ),
        "1\nanswer = true\n",
    );
}

#[test]
fn last_use_optimization_preserves_socket_alias_lifetime_until_scope_exit() {
    assert_eq!(
        stdout(
            r#"type Peer { socket: dever.system.Socket }
send(peer: Peer) () recover("test observes socket lifetime through the peer read") { dever.net.write(peer.socket, dever.bytes.from_text("x")) }
timeout(result: dever.io.ReadResult.Failed(message)) (answer: Bool) recover("test observes timeout failures in its assertions") { answer = true }
timeout(result: other) (answer: Bool) recover("test observes timeout failures in its assertions") { answer = false }
accepted(result: dever.net.ConnectResult.Connected(server), client: dever.system.Socket, listener: dever.system.Listener) (answer: Bool) recover("test observes accepted failures in its assertions") {
 dever.net.timeout(server, 20)
 peer = Peer { socket = client }
 send(peer)
 first = dever.net.read(server, 1)
 answer = timeout(result(dever.net.read(server, 1)))
 dever.net.close(server)
 dever.net.close_listener(listener)
}
accepted(result: other, client: dever.system.Socket, listener: dever.system.Listener) (answer: Bool) recover("test observes accepted failures in its assertions") { answer = false }
connected(result: dever.net.ConnectResult.Connected(client), listener: dever.system.Listener) (answer: Bool) recover("test observes connected failures in its assertions") { answer = accepted(result(dever.net.accept(listener)), client, listener) }
connected(result: other, listener: dever.system.Listener) (answer: Bool) recover("test observes connected failures in its assertions") { answer = false }
bound(result: dever.net.PortResult.Bound(port), listener: dever.system.Listener) (answer: Bool) recover("test observes bound failures in its assertions") { answer = connected(result(dever.net.connect("127.0.0.1", port)), listener) }
bound(result: other, listener: dever.system.Listener) (answer: Bool) recover("test observes bound failures in its assertions") { answer = false }
listening(result: dever.net.ListenResult.Listening(listener)) (answer: Bool) recover("test observes listening failures in its assertions") { answer = bound(result(dever.net.port(listener)), listener) }
listening(result: other) (answer: Bool) recover("test observes listening failures in its assertions") { answer = false }
public main() (answer: Bool) { answer = listening(result(dever.net.listen("127.0.0.1", 0))) }
"#
        ),
        "answer = true\n",
    );
}
