use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use dever_runtime::{bytes::Bytes, channel::Channel, http, net, task};

fn limits() -> http::Limits {
    http::Limits {
        header_bytes: 8192,
        body_bytes: 1024,
        timeout_ms: 1000,
        connections: 4,
        http2: None,
    }
}

fn run_test(test: impl Future<Output = Result<(), String>>) {
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 1,
            max_blocking_threads: 2,
            task_capacity: 32,
        },
        async {
            tokio::time::timeout(Duration::from_secs(8), test)
                .await
                .map_err(|_| "HTTP test deadline exceeded".to_owned())?
        },
    )
    .unwrap();
}

fn header(name: &str, value: &str) -> http::Header {
    http::Header {
        name: name.into(),
        value: Bytes::from_text(value),
    }
}

fn request() -> http::Request {
    http::Request {
        method: "GET".into(),
        target: "/".into(),
        headers: vec![header("host", "test")],
        body: Bytes::new(vec![]),
    }
}

fn response(body: Bytes) -> http::Response {
    http::Response {
        status: 200,
        headers: vec![],
        body,
    }
}

async fn server<F, Fut>(
    route: F,
    limits: http::Limits,
) -> Result<(task::Task<()>, net::Listener, i64), String>
where
    F: Fn(http::Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<http::Response, String>> + Send + 'static,
{
    let listener = net::listen("127.0.0.1", 0).await?;
    let port = net::port(&listener)?;
    let owned = listener.clone();
    let task = task::run(async move { http::serve(route, owned, limits).await }).await?;
    Ok((task, listener, port))
}

// 独立、只读的测试线协议观察器：按 Content-Length 保留下一条响应，不调用 Hyper。
struct Peer {
    socket: net::Socket,
    buffered: Vec<u8>,
}

impl Peer {
    async fn connect(port: i64) -> Result<Self, String> {
        let socket = net::connect_timeout("127.0.0.1", port, 2000).await?;
        net::timeout(&socket, 2000)?;
        Ok(Self {
            socket,
            buffered: vec![],
        })
    }

    async fn write(&self, wire: &[u8]) -> Result<(), String> {
        net::write(&self.socket, &Bytes::new(wire.to_vec())).await
    }

    async fn read_more(&mut self) -> Result<(), String> {
        let bytes = net::read(&self.socket, 4096)
            .await?
            .ok_or("unexpected HTTP EOF")?;
        self.buffered.extend_from_slice(bytes.values());
        Ok(())
    }

    async fn response(&mut self, head_only: bool) -> Result<(String, Vec<u8>), String> {
        let end = loop {
            if let Some(end) = self
                .buffered
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
            {
                break end + 4;
            }
            self.read_more().await?;
        };
        let headers = String::from_utf8(self.buffered[..end].to_vec()).unwrap();
        let length = if head_only {
            0
        } else {
            headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0)
        };
        while self.buffered.len() < end + length {
            self.read_more().await?;
        }
        let body = self.buffered[end..end + length].to_vec();
        self.buffered.drain(..end + length);
        Ok((headers.to_ascii_lowercase(), body))
    }
}

#[test]
fn persistent_pipeline_preserves_chunked_binary_body_and_duplicate_headers() {
    run_test(async {
        let (server, listener, port) = server(
            |request| async move {
                if request.target == "/first" {
                    assert_eq!(request.body.values(), &[0, 255, b'x']);
                    let values: Vec<_> = request
                        .headers
                        .iter()
                        .filter(|header| header.name == "x-many")
                        .map(|header| header.value.values())
                        .collect();
                    assert_eq!(values, [b"one".as_slice(), &[0x80]]);
                }
                Ok(http::Response {
                    status: 200,
                    headers: vec![header("set-cookie", "a=1"), header("set-cookie", "b=2")],
                    body: request.body,
                })
            },
            limits(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"POST /first HTTP/1.1\r\nHost: test\r\nX-Many: one\r\nX-Many: \x80\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n\0\xff\r\n1\r\nx\r\n0\r\n\r\nPOST /next HTTP/1.1\r\nHost: test\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await?;
        let (headers, body) = peer.response(false).await?;
        assert!(headers.starts_with("http/1.1 200"));
        assert_eq!(headers.matches("set-cookie:").count(), 2);
        assert_eq!(body, [0, 255, b'x']);
        assert_eq!(peer.response(false).await?.1, b"ok");
        assert_eq!(net::read(&peer.socket, 1).await?, None);
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn head_and_no_content_responses_keep_next_message_aligned() {
    run_test(async {
        let (server, listener, port) = server(
            |request| async move {
                let mut result = response(Bytes::from_text("head representation"));
                if request.target == "/empty" {
                    result.status = 204;
                    result.body = Bytes::new(vec![]);
                }
                if request.target == "/reset" {
                    result.status = 205;
                    result.body = Bytes::new(vec![]);
                }
                if request.target == "/cached" {
                    result.status = 304;
                    result.body = Bytes::new(vec![]);
                }
                Ok(result)
            },
            limits(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        for (wire, status, head) in [
            (
                b"HEAD / HTTP/1.1\r\nHost: test\r\n\r\n".as_slice(),
                "200",
                true,
            ),
            (b"GET /empty HTTP/1.1\r\nHost: test\r\n\r\n", "204", false),
            (b"GET /reset HTTP/1.1\r\nHost: test\r\n\r\n", "205", false),
            (b"GET /cached HTTP/1.1\r\nHost: test\r\n\r\n", "304", false),
        ] {
            peer.write(wire).await?;
            let (headers, body) = peer.response(head).await?;
            assert!(
                headers.starts_with(&format!("http/1.1 {status}")),
                "{headers}"
            );
            assert!(body.is_empty());
        }
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn invalid_peers_and_limits_do_not_reach_the_handler_or_stop_the_server() {
    run_test(async {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let mut budget = limits();
        budget.body_bytes = 4;
        let (server, listener, port) = server(
            move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                async { Ok(response(Bytes::new(vec![]))) }
            },
            budget,
        )
        .await?;
        let oversized = format!(
            "GET / HTTP/1.1\r\nHost: test\r\nX-Large: {}\r\n\r\n",
            "a".repeat(9000)
        );
        for (wire, status) in [
            (b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nx".as_slice(), "400"),
            (b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n", "400"),
            (b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabcde\r\n0\r\n\r\n", "413"),
            (b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\n\r\n", "413"),
            (b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Trailer: yes\r\n\r\n", "501"),
            (b"CONNECT test:80 HTTP/1.1\r\nHost: test\r\n\r\n", "501"),
            (b"GET / HTTP/1.1\r\nHost: test\r\nConnection: upgrade\r\nUpgrade: websocket\r\n\r\n", "501"),
            (oversized.as_bytes(), "431"),
        ] {
            let mut peer = Peer::connect(port).await?;
            peer.write(wire).await?;
            let (headers, _) = peer.response(false).await?;
            assert!(headers.starts_with(&format!("http/1.1 {status}")), "{headers}");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            http::send("127.0.0.1", port, request(), limits())
                .await?
                .status,
            200
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn stop_closes_active_connections_and_drains_handler_descendants() {
    run_test(async {
        let entered = Channel::new(1)?;
        let notify = entered.clone();
        let dropped = Arc::new(AtomicBool::new(false));
        let observed = dropped.clone();
        let (server, listener, port) = server(
            move |_| {
                let notify = notify.clone();
                let marker = Dropped(observed.clone());
                async move {
                    let child = task::run(async move {
                        let _marker = marker;
                        notify.send(()).await?;
                        std::future::pending::<()>().await;
                        Ok(())
                    })
                    .await?;
                    task::wait(child).await?;
                    Ok(response(Bytes::new(vec![])))
                }
            },
            limits(),
        )
        .await?;
        let peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await?;
        entered.receive().await?;
        task::stop(server).await?;
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(net::read(&peer.socket, 1).await?, None);
        // serve 只释放自己的 listener 别名，调用者仍可决定资源生命周期。
        assert_eq!(net::port(&listener)?, port);
        listener.close()?;
        Ok(())
    });
}

#[test]
fn handler_fault_returns_500_and_keeps_the_accept_loop_running() {
    run_test(async {
        let (server, listener, port) = server(
            |request| async move {
                if request.target == "/fault" {
                    return Err("route fault".into());
                }
                Ok(response(Bytes::from_text("healthy")))
            },
            limits(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET /fault HTTP/1.1\r\nHost: test\r\n\r\n")
            .await?;
        let (headers, body) = peer.response(false).await?;
        assert!(headers.starts_with("http/1.1 500"), "{headers}");
        assert!(body.is_empty());
        let healthy = http::send("127.0.0.1", port, request(), limits()).await?;
        assert_eq!(healthy.status, 200);
        assert_eq!(healthy.body.values(), b"healthy");
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn connection_capacity_backpressures_before_dispatching_another_request() {
    run_test(async {
        let entered = Channel::new(2)?;
        let release = Channel::new(2)?;
        let notify = entered.clone();
        let wait = release.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let mut budget = limits();
        budget.connections = 1;
        let (server, listener, port) = server(
            move |_| {
                let notify = notify.clone();
                let wait = wait.clone();
                let observed = observed.clone();
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    notify.send(()).await?;
                    wait.receive().await?;
                    Ok(response(Bytes::new(vec![])))
                }
            },
            budget,
        )
        .await?;
        let mut first = Peer::connect(port).await?;
        first
            .write(b"GET / HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
            .await?;
        entered.receive().await?;
        let mut second = Peer::connect(port).await?;
        second
            .write(b"GET / HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
            .await?;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.send(()).await?;
        first.response(false).await?;
        entered.receive().await?;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        release.send(()).await?;
        second.response(false).await?;
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn header_body_and_handler_deadlines_release_the_connection() {
    run_test(async {
        let mut budget = limits();
        budget.timeout_ms = 100;
        let (server, listener, port) = server(
            |_| async {
                std::future::pending::<()>().await;
                Ok(response(Bytes::new(vec![])))
            },
            budget,
        )
        .await?;
        let peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost:").await?;
        // Hyper 对未完成的请求头超时直接关闭连接，没有可交给 handler 的请求。
        assert_eq!(net::read(&peer.socket, 1).await?, None);
        for (wire, expected) in [
            (
                b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 2\r\n\r\nx".as_slice(),
                "408",
            ),
            (b"GET / HTTP/1.1\r\nHost: test\r\n\r\n", "504"),
        ] {
            let mut peer = Peer::connect(port).await?;
            peer.write(wire).await?;
            let (headers, _) = peer.response(false).await?;
            assert!(
                headers.starts_with(&format!("http/1.1 {expected}")),
                "{headers}"
            );
            assert_eq!(net::read(&peer.socket, 1).await?, None);
        }
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn client_accepts_chunked_and_eof_bodies_but_rejects_truncation_and_oversize() {
    run_test(async {
        for (wire, expected) in [
            (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n\r\n2\r\nok\r\n0\r\n\r\n".as_slice(), true),
            (b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nok", true),
            (b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nok", false),
            (b"HTTP/1.1 200 OK\r\nContent-Length: 2048\r\n\r\n", false),
        ] {
            let listener = net::listen("127.0.0.1", 0).await?;
            let port = net::port(&listener)?;
            let peer = task::run(async move {
                let socket = net::accept(&listener).await?;
                net::timeout(&socket, 1000)?;
                let sent = net::read(&socket, 4096).await?.unwrap().to_text()?;
                assert!(sent.starts_with("GET / HTTP/1.1\r\n"));
                assert!(sent.contains("host: test\r\n"));
                for piece in wire.chunks(7) { net::write(&socket, &Bytes::new(piece.to_vec())).await?; tokio::task::yield_now().await; }
                Ok(())
            }).await?;
            let received = http::send("127.0.0.1", port, request(), limits()).await;
            if expected { assert_eq!(received?.body.values(), b"ok"); } else { assert!(received.is_err()); }
            task::wait(peer).await?;
        }
        Ok(())
    });
}

#[test]
fn invalid_outgoing_messages_and_configuration_fail_before_connecting() {
    run_test(async {
        for invalid in [
            header("x-test", "ok\r\ninjected: yes"),
            header("content-length", "0"),
            header("transfer-encoding", "chunked"),
        ] {
            let mut request = request();
            request.headers.push(invalid);
            let error = http::send("127.0.0.1", 0, request, limits())
                .await
                .unwrap_err();
            assert!(!error.contains("Connection refused"), "{error}");
        }
        let mut excessive = request();
        excessive.headers.extend(vec![header("x", ""); 32769]);
        assert_eq!(
            http::send("127.0.0.1", 0, excessive, limits())
                .await
                .unwrap_err(),
            "HTTP headers exceed header_bytes"
        );
        for budget in [
            http::Limits {
                header_bytes: 1,
                ..limits()
            },
            http::Limits {
                body_bytes: -1,
                ..limits()
            },
            http::Limits {
                timeout_ms: 0,
                ..limits()
            },
            http::Limits {
                connections: 0,
                ..limits()
            },
        ] {
            assert!(
                http::send("127.0.0.1", 0, request(), budget)
                    .await
                    .unwrap_err()
                    .starts_with("HTTP")
            );
        }
        Ok(())
    });
}

#[test]
fn invalid_handler_responses_return_500_and_keep_the_accept_loop_running() {
    run_test(async {
        for status in [99, 204, 205, 304, 600] {
            let (server, listener, port) = server(
                move |request| async move {
                    if request.target != "/invalid" {
                        return Ok(response(Bytes::from_text("healthy")));
                    }
                    Ok(http::Response {
                        status,
                        headers: vec![],
                        body: Bytes::from_text("invalid body"),
                    })
                },
                limits(),
            )
            .await?;
            let mut peer = Peer::connect(port).await?;
            peer.write(b"GET /invalid HTTP/1.1\r\nHost: test\r\n\r\n")
                .await?;
            let (headers, body) = peer.response(false).await?;
            assert_eq!(headers.split_whitespace().nth(1), Some("500"), "{headers}");
            assert!(headers.lines().any(|line| line == "connection: close"));
            assert!(body.is_empty());
            assert!(peer.buffered.is_empty());
            assert_eq!(net::read(&peer.socket, 1).await?, None);
            let healthy = http::send("127.0.0.1", port, request(), limits()).await?;
            assert_eq!(healthy.status, 200);
            assert_eq!(healthy.body.values(), b"healthy");
            listener.close()?;
            task::wait(server).await?;
        }
        Ok(())
    });
}

#[test]
fn client_timeout_and_stop_release_the_owned_connection_driver() {
    run_test(async {
        for stop in [false, true] {
            let listener = net::listen("127.0.0.1", 0).await?;
            let port = net::port(&listener)?;
            let received = Channel::new(1)?;
            let notify = received.clone();
            let peer = task::run(async move {
                let socket = net::accept(&listener).await?;
                net::timeout(&socket, 2000)?;
                assert!(net::read(&socket, 4096).await?.is_some());
                notify.send(()).await?;
                assert_eq!(net::read(&socket, 1).await?, None);
                Ok(())
            })
            .await?;
            let mut budget = limits();
            budget.timeout_ms = if stop { 1000 } else { 100 };
            let client = task::run(async move {
                let result = http::send("127.0.0.1", port, request(), budget).await;
                assert_eq!(result.unwrap_err(), "HTTP request timed out");
                Ok(())
            })
            .await?;
            received.receive().await?;
            if stop {
                task::stop(client).await?;
            } else {
                task::wait(client).await?;
            }
            task::wait(peer).await?;
        }
        Ok(())
    });
}

#[test]
fn a_stalled_writer_releases_its_connection_slot() {
    run_test(async {
        let mut budget = limits();
        budget.timeout_ms = 100;
        budget.connections = 1;
        budget.body_bytes = 16 * 1024 * 1024;
        let entered = Channel::new(1)?;
        let notify = entered.clone();
        let (server, listener, port) = server(
            move |request| {
                let notify = notify.clone();
                async move {
                    let body = if request.target == "/slow" {
                        notify.send(()).await?;
                        Bytes::new(vec![b'x'; 16 * 1024 * 1024])
                    } else {
                        Bytes::from_text("next")
                    };
                    Ok(response(body))
                }
            },
            budget,
        )
        .await?;
        let slow = Peer::connect(port).await?;
        slow.write(b"GET /slow HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
            .await?;
        entered.receive().await?;
        let mut next = Peer::connect(port).await?;
        next.write(b"GET /next HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
            .await?;
        assert_eq!(next.response(false).await?.1, b"next");
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}
