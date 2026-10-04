#[path = "support/wire_peer.rs"]
mod wire_peer;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use dever_runtime::{
    async_stream::AsyncStream, bytes::Bytes, channel::Channel, http, net, task, tls, websocket,
};
use wire_peer::Peer;

const ROOT: &[u8] = include_bytes!("../fixtures/tls/root.pem");
const CERT: &[u8] = include_bytes!("../fixtures/tls/server.pem");
const KEY: &[u8] = include_bytes!("../fixtures/tls/server-key.pem");

fn bytes(value: &[u8]) -> Bytes {
    Bytes::new(value.to_vec())
}
fn limits() -> http::Limits {
    http::Limits {
        header_bytes: 8192,
        body_bytes: 65536,
        timeout_ms: 1000,
        connections: 2,
        http2: None,
    }
}
fn pool_limits() -> http::PoolLimits {
    http::PoolLimits {
        idle_ms: 1000,
        chunk_bytes: 4,
        read_ms: 500,
    }
}
fn request(target: &str) -> http::Request {
    http::Request {
        method: "GET".into(),
        target: target.into(),
        headers: vec![],
        body: bytes(b""),
    }
}
fn trust() -> tls::ClientTls {
    tls::client(&bytes(ROOT)).unwrap()
}
fn identity() -> tls::ServerTls {
    tls::server(&bytes(CERT), &bytes(KEY)).unwrap()
}
fn run(future: impl std::future::Future<Output = Result<(), String>>) {
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 1,
            max_blocking_threads: 2,
            task_capacity: 64,
        },
        async {
            tokio::time::timeout(Duration::from_secs(8), future)
                .await
                .expect("bounded owned peers")
        },
    )
    .unwrap();
}
async fn client(
    port: i64,
    limits: http::Limits,
    pool: http::PoolLimits,
) -> Result<http::HttpClient, String> {
    http::client(
        &format!("http://localhost:{port}"),
        tls::system(),
        limits,
        pool,
    )
    .await
}
async fn accept(listener: &net::Listener) -> Result<Peer, String> {
    Peer::new(net::accept(listener).await?)
}

#[test]
fn pooled_requests_reuse_one_transport_and_preserve_wire_contracts() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let peer = task::run(async move {
            let mut peer = accept(&listener).await?;
            for index in 0..20 {
                let head = peer.head().await?;
                assert!(head.starts_with(&format!("GET /{index} HTTP/1.1\r\n")), "{head}");
                assert!(head.to_ascii_lowercase().contains(&format!("host: localhost:{port}\r\n")));
                peer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n\r\nabcdef").await?;
            }
            assert!(net::read(&peer.socket, 1).await?.is_none());
            Ok(())
        }).await?;
        let client = client(port, limits(), pool_limits()).await?;
        for index in 0..20 {
            let response = http::request(&client, request(&format!("/{index}"))).await?;
            assert_eq!(response.body.values(), b"abcdef");
            assert_eq!(
                response
                    .headers
                    .iter()
                    .filter(|header| header.name == "set-cookie")
                    .count(),
                2
            );
        }
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn streamed_response_delivers_early_chunks_past_the_header_deadline() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let gate = Channel::new(1).unwrap();
        let signal = gate.clone();
        let peer = task::run(async move {
            let mut peer = accept(&listener).await?;
            peer.head().await?;
            peer.write(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/event-stream\r\n\r\n6\r\nhello!\r\n").await?;
            signal.receive().await?;
            task::sleep(80).await?;
            peer.write(b"6\r\nworld!\r\n0\r\n\r\n").await?;
            assert!(net::read(&peer.socket, 1).await?.is_none());
            Ok(())
        }).await?;
        let mut bounded = limits();
        bounded.timeout_ms = 40;
        let client = client(port, bounded, pool_limits()).await?;
        let response = http::open(&client, request("/events")).await?;
        assert_eq!(response.body.pull().await.unwrap()?.values(), b"hell");
        assert_eq!(response.body.pull().await.unwrap()?.values(), b"o!");
        gate.send(()).await?;
        let mut tail = Vec::new();
        while let Some(chunk) = response.body.pull().await {
            let chunk = chunk?;
            assert!(chunk.values().len() <= 4);
            tail.extend_from_slice(chunk.values());
        }
        assert_eq!(tail, b"world!");
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn abandoned_response_releases_capacity_and_closes_incomplete_connection() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let peer = task::run(async move {
            let mut first = accept(&listener).await?;
            first.head().await?;
            first
                .write(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\na")
                .await?;
            assert!(net::read(&first.socket, 1).await?.is_none());
            let mut second = accept(&listener).await?;
            second.head().await?;
            second
                .write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await?;
            assert!(net::read(&second.socket, 1).await?.is_none());
            Ok(())
        })
        .await?;
        let mut bounded = limits();
        bounded.connections = 1;
        let client = client(port, bounded, pool_limits()).await?;
        let first = http::open(&client, request("/first")).await?;
        let pending_client = client.clone();
        let pending =
            task::run(async move { http::request(&pending_client, request("/second")).await })
                .await?;
        task::sleep(20).await?;
        first.body.close();
        assert_eq!(task::wait(pending).await?.body.values(), b"ok");
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn closing_pool_wakes_head_waiters_queue_waiters_and_body_readers() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let accepted = Channel::new(1).unwrap();
        let signal = accepted.clone();
        let peer = task::run(async move {
            let mut peer = accept(&listener).await?;
            peer.head().await?;
            signal.send(()).await?;
            assert!(net::read(&peer.socket, 1).await?.is_none());
            Ok(())
        })
        .await?;
        let mut bounded = limits();
        bounded.connections = 1;
        let client = client(port, bounded, pool_limits()).await?;
        let a = client.clone();
        let first =
            task::run(async move { Ok(http::open(&a, request("/first")).await.is_err()) }).await?;
        accepted.receive().await?;
        let b = client.clone();
        let queued =
            task::run(async move { Ok(http::open(&b, request("/queued")).await.is_err()) }).await?;
        http::close_client(&client).await?;
        http::close_client(&client).await?;
        assert!(task::wait(first).await? && task::wait(queued).await?);
        task::wait(peer).await?;

        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let peer = task::run(async move {
            let mut peer = accept(&listener).await?;
            peer.head().await?;
            peer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n")
                .await?;
            assert!(net::read(&peer.socket, 1).await?.is_none());
            Ok(())
        })
        .await?;
        let client = self::client(port, limits(), pool_limits()).await?;
        let body = http::open(&client, request("/body")).await?.body;
        let reader = body.clone();
        let pending = task::run(async move { Ok(reader.pull().await.unwrap().is_err()) }).await?;
        http::close_client(&client).await?;
        assert!(task::wait(pending).await?);
        assert!(body.pull().await.is_none());
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn idle_expiry_closes_transport_without_another_request() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let peer = task::run(async move {
            let mut peer = accept(&listener).await?;
            peer.head().await?;
            peer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await?;
            assert!(net::read(&peer.socket, 1).await?.is_none());
            Ok(())
        })
        .await?;
        let mut pool = pool_limits();
        pool.idle_ms = 30;
        let client = client(port, limits(), pool).await?;
        http::request(&client, request("/")).await?;
        tokio::time::timeout(Duration::from_millis(300), task::wait(peer))
            .await
            .unwrap()?;
        http::close_client(&client).await?;
        Ok(())
    });
}

#[test]
fn upload_is_chunked_and_pull_driven() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let accepted = Channel::new(1).unwrap();
        let signal = accepted.clone();
        let peer = task::run(async move {
            let mut peer = accept(&listener).await?;
            let head = peer.head().await?.to_ascii_lowercase();
            assert!(head.starts_with("post /upload http/1.1\r\n"));
            assert!(head.contains("transfer-encoding: chunked\r\n"));
            assert_eq!(peer.chunk().await?, b"abc");
            signal.send(()).await?;
            assert_eq!(peer.chunk().await?, b"def");
            assert!(peer.chunk().await?.is_empty());
            peer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await?;
            assert!(net::read(&peer.socket, 1).await?.is_none());
            Ok(())
        })
        .await?;
        let client = client(port, limits(), pool_limits()).await?;
        let input = Channel::new(1).unwrap();
        input.send(Ok(bytes(b"abc"))).await?;
        let source = input.clone().stream();
        let sender_client = client.clone();
        let upload = task::run(async move {
            http::open_stream(
                &sender_client,
                http::StreamRequest {
                    method: "POST".into(),
                    target: "/upload".into(),
                    headers: vec![],
                    body: source,
                },
            )
            .await
        })
        .await?;
        accepted.receive().await?;
        input.send(Ok(bytes(b"def"))).await?;
        input.close().await?;
        let response = task::wait(upload).await?;
        assert_eq!(response.body.pull().await.unwrap()?.values(), b"ok");
        assert!(response.body.pull().await.is_none());
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn truncated_and_oversized_bodies_are_errors_and_stream_fault_is_terminal() {
    run(async {
        for wire in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\na".as_slice(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\n0\r\nX-Trailer: bad\r\n\r\n".as_slice(),
        ] {
            let listener = net::listen("127.0.0.1", 0).await?;
            let port = net::port(&listener)?;
            let peer = task::run(async move {
                let mut peer = accept(&listener).await?;
                peer.head().await?;
                peer.write(wire).await?;
                peer.socket.close()?;
                Ok(())
            }).await?;
            let client = client(port, limits(), pool_limits()).await?;
            let body = http::open(&client, request("/")).await?.body;
            let mut failures = 0;
            while let Some(chunk) = body.pull().await { if chunk.is_err() { failures += 1; } }
            assert_eq!(failures, 1);
            assert!(body.pull().await.is_none());
            http::close_client(&client).await?;
            task::wait(peer).await?;
        }
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let peer = task::run(async move {
            let mut peer = accept(&listener).await?;
            peer.head().await?;
            peer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n1234")
                .await?;
            assert!(net::read(&peer.socket, 1).await?.is_none());
            Ok(())
        })
        .await?;
        let mut bounded = limits();
        bounded.body_bytes = 3;
        let client = client(port, bounded, pool_limits()).await?;
        assert!(
            http::request(&client, request("/"))
                .await
                .unwrap_err()
                .contains("body_bytes")
        );
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn upload_rejects_oversized_chunks_and_source_failure() {
    run(async {
        for chunk in [Ok(bytes(b"too large")), Err("source failed".to_owned())] {
            let listener = net::listen("127.0.0.1", 0).await?;
            let port = net::port(&listener)?;
            let peer = task::run(async move {
                let peer = accept(&listener).await?;
                while net::read(&peer.socket, 4096).await?.is_some() {}
                Ok(())
            })
            .await?;
            let client = client(port, limits(), pool_limits()).await?;
            let response = http::open_stream(
                &client,
                http::StreamRequest {
                    method: "POST".into(),
                    target: "/".into(),
                    headers: vec![],
                    body: AsyncStream::from_values([chunk]),
                },
            )
            .await;
            assert!(response.is_err());
            http::close_client(&client).await?;
            task::wait(peer).await?;
        }
        Ok(())
    });
}

#[test]
fn https_verifies_ca_and_hostname_and_keeps_server_alive_after_rejection() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let server = task::run(http::serve_tls(
            |request| async move {
                Ok(http::Response {
                    status: 200,
                    headers: vec![],
                    body: Bytes::from_string(request.target),
                })
            },
            listener,
            limits(),
            identity(),
        ))
        .await?;
        let untrusted = http::client(
            &format!("https://localhost:{port}"),
            tls::system(),
            limits(),
            pool_limits(),
        )
        .await?;
        assert!(
            http::request(&untrusted, request("/untrusted"))
                .await
                .is_err()
        );
        http::close_client(&untrusted).await?;
        let wrong_name = http::client(
            &format!("https://127.0.0.1:{port}"),
            trust(),
            limits(),
            pool_limits(),
        )
        .await?;
        assert!(
            http::request(&wrong_name, request("/wrong-name"))
                .await
                .is_err()
        );
        http::close_client(&wrong_name).await?;
        let trusted = http::client(
            &format!("https://localhost:{port}"),
            trust(),
            limits(),
            pool_limits(),
        )
        .await?;
        for path in ["/one", "/two"] {
            assert_eq!(
                http::request(&trusted, request(path)).await?.body.values(),
                path.as_bytes()
            );
        }
        http::close_client(&trusted).await?;
        task::stop(server).await?;
        assert!(tls::client(&bytes(b"not a certificate")).is_err());
        assert!(tls::server(&bytes(CERT), &bytes(ROOT)).is_err());
        Ok(())
    });
}

#[test]
fn wss_upgrade_duplex_and_close_share_the_tls_transport() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let ws_limits = websocket::Limits {
            message_bytes: 1024,
            idle_ms: 1000,
            write_ms: 1000,
        };
        let live = http::LiveLimits {
            chunk_bytes: 1024,
            idle_ms: 1000,
            heartbeat_ms: 100,
        };
        let server = task::run(http::serve_live_tls(
            move |_, reply| async move {
                let socket = websocket::accept(&reply, ws_limits).await?;
                if let Some(message) = websocket::receive(&socket).await? {
                    websocket::send(&socket, message).await?;
                }
                assert!(websocket::receive(&socket).await?.is_none());
                Ok(())
            },
            listener,
            limits(),
            live,
            identity(),
        ))
        .await?;
        let socket =
            websocket::open(&format!("wss://localhost:{port}/ws"), trust(), ws_limits).await?;
        websocket::send(&socket, websocket::Message::Text("中文 TLS".into())).await?;
        assert!(
            matches!(websocket::receive(&socket).await?, Some(websocket::Message::Text(value)) if value == "中文 TLS")
        );
        websocket::close(&socket, 1000, "done").await?;
        task::stop(server).await?;
        Ok(())
    });
}

#[test]
fn graceful_shutdown_stops_accepting_and_finishes_active_response() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let gate = Channel::new(1).unwrap();
        let release = gate.clone();
        let returned = Arc::new(AtomicBool::new(false));
        let completed = returned.clone();
        let live = http::LiveLimits {
            chunk_bytes: 1024,
            idle_ms: 1000,
            heartbeat_ms: 100,
        };
        let server = task::run(http::serve_live(
            move |_, reply| {
                let gate = gate.clone();
                let returned = returned.clone();
                async move {
                    http::start(&reply, 200, vec![]).await?;
                    http::write(&reply, bytes(b"first")).await?;
                    gate.receive().await?;
                    http::write(&reply, bytes(b"last")).await?;
                    returned.store(true, Ordering::Release);
                    Ok(())
                }
            },
            listener.clone(),
            limits(),
            live,
        ))
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await?;
        peer.head().await?;
        assert_eq!(peer.chunk().await?, b"first");
        listener.close()?;
        assert!(!completed.load(Ordering::Acquire));
        release.send(()).await?;
        assert_eq!(peer.chunk().await?, b"last");
        assert!(peer.chunk().await?.is_empty());
        task::wait(server).await?;
        assert!(completed.load(Ordering::Acquire));
        assert!(net::connect_timeout("127.0.0.1", port, 100).await.is_err());
        Ok(())
    });
}

#[test]
fn task_scope_closes_an_escaped_pool_instead_of_detaching_drivers() {
    run(async {
        let child = task::run(async {
            http::client("http://localhost:1", tls::system(), limits(), pool_limits()).await
        })
        .await?;
        let pool = task::wait(child).await?;
        assert_eq!(
            http::open(&pool, request("/")).await.unwrap_err(),
            "HTTP client is closed"
        );
        http::close_client(&pool).await?;
        Ok(())
    });
}

#[test]
fn stopping_an_upload_closes_partial_request_and_pool_remains_usable() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let accepted = Channel::new(1).unwrap();
        let signal = accepted.clone();
        let peer = task::run(async move {
            let mut first = accept(&listener).await?;
            first.head().await?;
            assert_eq!(first.chunk().await?, b"abc");
            signal.send(()).await?;
            assert!(net::read(&first.socket, 1).await?.is_none());
            let mut next = accept(&listener).await?;
            assert!(next.head().await?.starts_with("GET /next HTTP/1.1"));
            next.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await?;
            assert!(net::read(&next.socket, 1).await?.is_none());
            Ok(())
        })
        .await?;
        let mut bounded = limits();
        bounded.connections = 1;
        let client = client(port, bounded, pool_limits()).await?;
        let chunks = Channel::new(1).unwrap();
        chunks.send(Ok(bytes(b"abc"))).await?;
        let stream = chunks.clone().stream();
        let source = stream.clone();
        let sender_client = client.clone();
        let sender = task::run(async move {
            let _response = http::open_stream(
                &sender_client,
                http::StreamRequest {
                    method: "POST".into(),
                    target: "/upload".into(),
                    headers: vec![],
                    body: source,
                },
            )
            .await?;
            Ok(())
        })
        .await?;
        accepted.receive().await?;
        task::stop(sender).await?;
        assert!(stream.pull().await.is_none());
        assert_eq!(
            http::request(&client, request("/next"))
                .await?
                .body
                .values(),
            b"ok"
        );
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn pool_queue_and_body_deadlines_release_only_their_own_operations() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let peer = task::run(async move {
            let mut first = accept(&listener).await?;
            first.head().await?;
            first
                .write(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\na")
                .await?;
            assert!(net::read(&first.socket, 1).await?.is_none());
            let mut next = accept(&listener).await?;
            next.head().await?;
            next.write(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await?;
            assert!(net::read(&next.socket, 1).await?.is_none());
            Ok(())
        })
        .await?;
        let mut bounded = limits();
        bounded.connections = 1;
        bounded.timeout_ms = 30;
        let mut pool = pool_limits();
        pool.read_ms = 30;
        let client = client(port, bounded, pool).await?;
        let body = http::open(&client, request("/first")).await?.body;
        assert!(
            http::request(&client, request("/queued"))
                .await
                .unwrap_err()
                .contains("timed out")
        );
        assert_eq!(body.pull().await.unwrap()?.values(), b"a");
        assert!(
            body.pull()
                .await
                .unwrap()
                .unwrap_err()
                .contains("read timed out")
        );
        assert!(body.pull().await.is_none());
        assert_eq!(
            http::request(&client, request("/next"))
                .await?
                .body
                .values(),
            b"ok"
        );
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}
