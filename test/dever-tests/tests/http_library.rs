mod support;

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

use support::{checked, rejected, sources, stdout};

const ROUTE: &str = r#"
route(request: dever.http.Request) (response: dever.http.Response) recover("route policy") {
  response = dever.http.Response { status = 201
headers = [dever.http.header("Set-Cookie", "a=1"), dever.http.header("Set-Cookie", "b=2")]
body = request.body }
}
"#;

const LOCAL_SERVER: &str = r#"
exchange(result: dever.net.PortResult.Bound(port), listener: dever.system.Listener) (answer: Bool) recover("test reports network failures") {
  server = run(dever.http.serve(route, listener, dever.http.default_limits()))
  request = dever.http.Request { method = "POST"
target = "/echo"
headers = [dever.http.header("Host", "test")]
body = dever.bytes.from_text("hello") }
  answer = received(result(dever.http.send("127.0.0.1", port, request, dever.http.default_limits())))
  stop(server)
  dever.net.close_listener(listener)
}
exchange(result: dever.net.PortResult.Failed(message), listener: dever.system.Listener) (answer: Bool) recover("test reports network failures") { answer = false }
listening(result: dever.net.ListenResult.Listening(listener)) (answer: Bool) recover("test reports network failures") {
  answer = exchange(result(dever.net.port(listener)), listener)
}
listening(result: dever.net.ListenResult.Failed(message)) (answer: Bool) recover("test reports network failures") { answer = false }
received(result: dever.http.ResponseResult.Read(response)) (answer: Bool) recover("test reports network failures") {
  answer = response.status == 201 and response.body == dever.bytes.from_text("hello") and length(response.headers) >= 2 and cookie(dever.http.header_value(response.headers, "SET-COOKIE"))
}
received(result: dever.http.ResponseResult.Failed(message)) (answer: Bool) recover("test reports network failures") { answer = false }
cookie(value: null) (answer: Bool) { answer = false }
cookie(value: Bytes) (answer: Bool) { answer = value == dever.bytes.from_text("a=1") }
main() (answer: Bool) { answer = listening(result(dever.net.listen("127.0.0.1", 0))) }
"#;

fn local_source(http2: bool) -> String {
    let protocol = if http2 {
        "dever.http.default_http2_limits()"
    } else {
        "null"
    };
    let server = LOCAL_SERVER.replace("dever.http.default_limits()", "http_limits()");
    format!(
        r#"{ROUTE}
{server}
http_limits() (limits: dever.http.Limits) {{
  limits = dever.http.default_limits()
  limits.http2 = {protocol}
}}
"#
    )
}

#[test]
fn native_http_uses_static_handlers_and_runs_under_structured_tasks() {
    let source = local_source(false);
    let map = sources(&source);
    let program = checked(&map);
    let generated = dever_core::native::emit(&program, &map, "main.main").unwrap();
    assert!(generated.contains("dever_runtime::http::serve("));
    assert!(generated.contains("dever_runtime::http::send("));
    assert!(!generated.contains("Box<dyn Fn"));
    assert_eq!(stdout(&source), "answer = true\n");
}

#[test]
fn native_http2_buffered_server_and_single_use_client() {
    assert_eq!(stdout(&local_source(true)), "answer = true\n");
}

#[test]
fn handler_contracts_and_forwarded_effects_reach_the_http_boundary() {
    let shared = format!(
        r#"{ROUTE}
public start(listener: dever.system.Listener) () {{
  dever.http.serve(route, listener, dever.http.default_limits())
}}
"#
    );
    let program = checked(&sources(&shared));
    let api = program.api_snapshot();
    let start = api
        .lines()
        .find(|line| line.starts_with("function main.start"))
        .unwrap();
    assert!(
        start.contains("network")
            && start.contains("concurrency")
            && start.contains("route policy"),
        "{start}"
    );
    rejected(
        &shared
            .replace(
                "(response: dever.http.Response)",
                "(wrong: dever.http.Response)",
            )
            .replace("  response =", "  wrong ="),
        "handler signature",
    );
    rejected(
        &shared.replace(
            "response = dever.http.Response",
            "dever.io.println(\"blocking\")\nresponse = dever.http.Response",
        ),
        "cannot run inline",
    );
}

#[test]
fn intrinsic_handlers_participate_in_recursion_checks() {
    rejected(
        r#"public route(request: dever.http.Request) (response: dever.http.Response) {
  start(null)
  response = dever.http.Response { status = 200
headers = []
body = request.body }
}
start(unused: Text?) () {
  open(result(dever.net.listen("127.0.0.1", 0)))
}
open(result: dever.net.ListenResult.Listening(listener)) () recover("test observes failure") { dever.http.serve(route, listener, dever.http.default_limits()) }
open(result: dever.net.ListenResult.Failed(message)) () recover("test observes failure") {}
"#,
        "recursive",
    );
}

#[test]
fn header_helpers_preserve_duplicates_bytes_and_case_insensitive_lookup() {
    assert_eq!(
        stdout(
            r#"public main() (found: Bytes?, missing: Bytes?, count: Int) {
  headers = [dever.http.header("X-Key", "one"), dever.http.header("X-Key", "two")]
  found = dever.http.header_value(headers, "x-KEY")
  missing = dever.http.header_value(headers, "absent")
  count = length(headers)
}"#
        ),
        "found = [111, 110, 101]\nmissing = null\ncount = 2\n"
    );
}

fn compile(source: &str) -> dever_core::native::NativeProgram {
    let sources = sources(source);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    dever_core::native::compile(&checked(&sources), &sources, "main.main", &rustc).unwrap()
}

fn accept_before(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((socket, _)) => return socket,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => panic!("test peer accept failed: {error}"),
        }
    }
}

#[test]
fn native_client_matches_independent_wire_and_retains_repeated_response_headers() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let native = compile(&format!(
        r#"received(result: dever.http.ResponseResult.Read(response)) (answer: Bool) recover("test reports wire failure") {{
 answer = response.status == 201 and response.body == dever.bytes.from_text("abc") and length(response.headers) == 3
}}
received(result: dever.http.ResponseResult.Failed(message)) (answer: Bool) recover("test reports wire failure") {{ answer = false }}
public main() (answer: Bool) {{
 request = dever.http.Request {{ method = "POST"
target = "/echo"
headers = [dever.http.header("Host", "example.test")]
body = dever.bytes.from_text("x") }}
 answer = received(result(dever.http.send("127.0.0.1", {port}, request, dever.http.default_limits())))
}}
"#
    ));
    let peer = std::thread::spawn(move || {
        let mut socket = accept_before(&listener);
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let expected = b"POST /echo HTTP/1.1\r\nhost: example.test\r\ncontent-length: 1\r\n\r\nx";
        let mut wire = vec![0; expected.len()];
        socket.read_exact(&mut wire).unwrap();
        assert_eq!(wire, expected);
        for piece in [
            b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\nSet-Cookie: a=1\r\n".as_slice(),
            b"Set-Cookie: b=2\r\n\r\n1\r\na\r\n",
            b"2\r\nbc\r\n0\r\n\r\n",
        ] {
            socket.write_all(piece).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    let output = Command::new(native.executable()).output().unwrap();
    peer.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "answer = true\n");
}

#[test]
fn http_and_server_examples_pass_checking_and_format_roundtrips() {
    for (path, check_source) in [
        ("library/dever/http.dever", false),
        ("examples/old/dever/http/module/main.dever", true),
    ] {
        let disk_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path);
        let source = std::fs::read_to_string(disk_path).unwrap();
        let mut map = dever_core::source::SourceMap::default();
        let id = map.add(path, source);
        let first = dever_core::format::format(map.get(id)).unwrap();
        let mut formatted = dever_core::source::SourceMap::default();
        let id = formatted.add("main.dever", first.clone());
        assert_eq!(
            dever_core::format::format(formatted.get(id)).unwrap(),
            first
        );
        if check_source {
            checked(&formatted);
        }
    }
}
