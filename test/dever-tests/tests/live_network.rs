mod support;
#[path = "support/wire_peer.rs"]
mod wire_peer;

use dever_runtime::{bytes::Bytes, http, net, sse, task, websocket as ws};
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use wire_peer::Peer;

fn limits() -> http::Limits {
    http::Limits {
        header_bytes: 8192,
        body_bytes: 1024,
        timeout_ms: 500,
        connections: 4,
        http2: None,
    }
}
fn live() -> http::LiveLimits {
    http::LiveLimits {
        chunk_bytes: 256,
        idle_ms: 1000,
        heartbeat_ms: 30,
    }
}
fn ws_limits() -> ws::Limits {
    ws::Limits {
        message_bytes: 1024,
        idle_ms: 1000,
        write_ms: 500,
    }
}
fn run(test: impl Future<Output = Result<(), String>>) {
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 1,
            max_blocking_threads: 2,
            task_capacity: 64,
        },
        async {
            tokio::time::timeout(Duration::from_secs(8), test)
                .await
                .map_err(|_| "live test deadline".to_owned())?
        },
    )
    .unwrap();
}
async fn server<F, Fut>(
    route: F,
    limits: http::Limits,
    live: http::LiveLimits,
) -> Result<(task::Task<()>, net::Listener, i64), String>
where
    F: Fn(http::Request, http::HttpReply) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    let listener = net::listen("127.0.0.1", 0).await?;
    let port = net::port(&listener)?;
    let owner = listener.clone();
    let server =
        task::run(async move { http::serve_live(route, owner, limits, live).await }).await?;
    Ok((server, listener, port))
}
struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct Finished(Arc<tokio::sync::Semaphore>);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.add_permits(1);
    }
}

#[test]
fn sse_wire_preserves_empty_ids_multiline_data_and_exact_size_limit() {
    let event = sse::Event {
        event: "change".into(),
        data: "一\r\n二\r\n\r".into(),
        id: Some("".into()),
        retry_ms: Some(0),
    };
    let expected = "event: change\nid: \nretry: 0\ndata: 一\ndata: 二\ndata: \ndata: \n\n";
    assert_eq!(
        sse::encode(event.clone(), expected.len()).unwrap().values(),
        expected.as_bytes()
    );
    assert!(sse::encode(event, expected.len() - 1).is_err());
    for event in [
        sse::Event {
            event: "bad\rname".into(),
            data: "".into(),
            id: None,
            retry_ms: None,
        },
        sse::Event {
            event: "".into(),
            data: "".into(),
            id: Some("bad\0id".into()),
            retry_ms: None,
        },
        sse::Event {
            event: "".into(),
            data: "".into(),
            id: None,
            retry_ms: Some(-1),
        },
    ] {
        assert!(sse::encode(event, 1024).is_err());
    }
    assert_eq!(
        sse::encode(
            sse::Event {
                event: "".into(),
                data: "".into(),
                id: None,
                retry_ms: None
            },
            8
        )
        .unwrap()
        .values(),
        b"data: \n\n"
    );
}

#[test]
fn streaming_first_chunk_arrives_before_handler_finishes() {
    run(async {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let handler_gate = gate.clone();
        let (server, listener, port) = server(
            move |_, reply| {
                let gate = handler_gate.clone();
                async move {
                    http::start(&reply, 200, vec![]).await?;
                    http::write(&reply, Bytes::from_text("first")).await?;
                    let _permit = gate.acquire().await.unwrap();
                    http::write(&reply, Bytes::from_text("second")).await?;
                    http::finish(&reply).await
                }
            },
            limits(),
            live(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await?;
        assert!(
            peer.head()
                .await?
                .to_ascii_lowercase()
                .contains("transfer-encoding: chunked")
        );
        assert_eq!(peer.chunk().await?, b"first");
        gate.add_permits(1);
        assert_eq!(peer.chunk().await?, b"second");
        assert!(peer.chunk().await?.is_empty());
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn sse_heartbeats_outlive_request_deadline_and_stop_drains_handler() {
    run(async {
        let dropped = Arc::new(AtomicUsize::new(0));
        let marker = dropped.clone();
        let (server, listener, port) = server(
            move |request, reply| {
                let marker = marker.clone();
                async move {
                    let _guard = Dropped(marker);
                    assert!(
                        request
                            .headers
                            .iter()
                            .any(|header| header.name == "last-event-id"
                                && header.value.values() == b"42")
                    );
                    sse::start(&reply, vec![]).await?;
                    sse::send(
                        &reply,
                        sse::Event {
                            event: "update".into(),
                            data: "hello".into(),
                            id: Some("43".into()),
                            retry_ms: None,
                        },
                    )
                    .await?;
                    std::future::pending().await
                }
            },
            http::Limits {
                timeout_ms: 80,
                ..limits()
            },
            live(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost: test\r\nLast-Event-ID: 42\r\n\r\n")
            .await?;
        assert!(peer.head().await?.contains("text/event-stream"));
        assert_eq!(
            peer.chunk().await?,
            b"event: update\nid: 43\ndata: hello\n\n"
        );
        for _ in 0..5 {
            assert_eq!(peer.chunk().await?, b":\n\n");
        }
        task::stop(server).await?;
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert_eq!(net::read(&peer.socket, 1).await?, None);
        listener.close()?;
        Ok(())
    });
}

#[test]
fn head_body_drop_releases_stream_and_buffered_handlers_before_next_request() {
    run(async {
        let dropped = Arc::new(AtomicUsize::new(0));
        let marker = dropped.clone();
        let (server, listener, port) = server(
            move |request, reply| {
                let marker = marker.clone();
                async move {
                    let _guard = Dropped(marker);
                    if request.target == "/stream" {
                        http::start(&reply, 200, vec![]).await?;
                    } else {
                        http::respond(
                            &reply,
                            http::Response {
                                status: 200,
                                headers: vec![],
                                body: Bytes::from_text("ok"),
                            },
                        )
                        .await?;
                    }
                    std::future::pending().await
                }
            },
            limits(),
            live(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"HEAD /stream HTTP/1.1\r\nHost: test\r\n\r\nHEAD /full HTTP/1.1\r\nHost: test\r\n\r\nGET /full HTTP/1.1\r\nHost: test\r\n\r\n").await?;
        for _ in 0..3 {
            assert!(peer.head().await?.starts_with("HTTP/1.1 200"));
        }
        assert_eq!(peer.take(2).await?, b"ok");
        task::stop(server).await?;
        assert_eq!(dropped.load(Ordering::SeqCst), 3);
        listener.close()?;
        Ok(())
    });
}

#[test]
fn handler_fault_after_headers_truncates_only_that_response() {
    run(async {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let handler_gate = gate.clone();
        let (server, listener, port) = server(
            move |_, reply| {
                let gate = handler_gate.clone();
                async move {
                    http::start(&reply, 200, vec![]).await?;
                    http::write(&reply, Bytes::from_text("partial")).await?;
                    let _permit = gate.acquire().await.unwrap();
                    Err("precise handler fault".into())
                }
            },
            limits(),
            live(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await?;
        peer.head().await?;
        assert_eq!(peer.chunk().await?, b"partial");
        gate.add_permits(1);
        assert!(peer.chunk().await.is_err());
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

async fn echo(_request: http::Request, reply: http::HttpReply) -> Result<(), String> {
    let socket = ws::accept(&reply, ws_limits()).await?;
    while let Some(message) = ws::receive(&socket).await? {
        if matches!(message, ws::Message::Text(_) | ws::Message::Binary(_)) {
            ws::send(&socket, message).await?;
        }
    }
    Ok(())
}

#[test]
fn slow_stream_peer_applies_backpressure_and_stop_releases_the_writer() {
    run(async {
        let produced = Arc::new(AtomicUsize::new(0));
        let counter = produced.clone();
        let finished = Arc::new(tokio::sync::Semaphore::new(0));
        let signal = finished.clone();
        let (server, listener, port) = server(
            move |_, reply| {
                let counter = counter.clone();
                let signal = signal.clone();
                async move {
                    let _finished = Finished(signal);
                    http::start(&reply, 200, vec![]).await?;
                    let chunk = Bytes::new(vec![1; 65536]);
                    for _ in 0..10000 {
                        if http::write(&reply, chunk.clone()).await.is_err() {
                            break;
                        }
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(())
                }
            },
            http::Limits {
                timeout_ms: 100,
                ..limits()
            },
            http::LiveLimits {
                chunk_bytes: 65536,
                ..live()
            },
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await?;
        peer.head().await?;
        // 客户端停止读取；写入只能占用有界通道与传输缓冲，不能把全部正文排队。
        let _permit = tokio::time::timeout(Duration::from_secs(2), finished.acquire())
            .await
            .unwrap()
            .unwrap();
        let count = produced.load(Ordering::SeqCst);
        assert!(
            count > 0 && count < 10000,
            "produced {count} chunks without a consuming peer"
        );
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn stream_idle_timeout_is_a_peer_failure_and_server_accepts_next_request() {
    run(async {
        let (server, listener, port) = server(
            |request, reply| async move {
                if request.target == "/idle" {
                    http::start(&reply, 200, vec![]).await?;
                    std::future::pending().await
                } else {
                    http::respond(
                        &reply,
                        http::Response {
                            status: 200,
                            headers: vec![],
                            body: Bytes::from_text("ok"),
                        },
                    )
                    .await
                }
            },
            limits(),
            http::LiveLimits {
                idle_ms: 60,
                heartbeat_ms: 20,
                ..live()
            },
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET /idle HTTP/1.1\r\nHost: test\r\n\r\n")
            .await?;
        peer.head().await?;
        assert!(peer.chunk().await.is_err());
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET /ok HTTP/1.1\r\nHost: test\r\n\r\n")
            .await?;
        assert!(peer.head().await?.starts_with("HTTP/1.1 200"));
        assert_eq!(peer.take(2).await?, b"ok");
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn websocket_rejects_invalid_mask_utf8_and_fragmented_message_size() {
    run(async {
        let failures = dever_runtime::channel::Channel::new(4)?;
        let sent = failures.clone();
        let (server, listener, port) = server(
            move |_, reply| {
                let failures = sent.clone();
                async move {
                    let socket = ws::accept(
                        &reply,
                        ws::Limits {
                            message_bytes: 4,
                            ..ws_limits()
                        },
                    )
                    .await?;
                    let error = ws::receive(&socket).await.unwrap_err();
                    failures.send(error).await
                }
            },
            limits(),
            live(),
        )
        .await?;
        for case in 0..4 {
            let mut peer = Peer::connect(port).await?;
            peer.upgrade().await?;
            match case {
                0 => peer.send_frame(0x81, false, b"bad").await?,
                1 => peer.send_frame(0x81, true, &[255]).await?,
                2 => peer.send_frame(0x82, true, b"large").await?,
                _ => {
                    peer.send_frame(0x01, true, b"abc").await?;
                    peer.send_frame(0x80, true, b"def").await?;
                }
            }
            let error = failures.receive().await?.unwrap();
            assert!(!error.is_empty());
            assert_eq!(net::read(&peer.socket, 1).await?, None);
        }
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn upgraded_sessions_hold_connection_capacity_and_parent_stop_closes_them() {
    run(async {
        let (server, listener, port) = server(
            echo,
            http::Limits {
                connections: 1,
                ..limits()
            },
            live(),
        )
        .await?;
        let first = ws::connect("127.0.0.1", port, "/ws", ws_limits()).await?;
        let second = ws::connect("127.0.0.1", port, "/ws", ws_limits());
        tokio::pin!(second);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut second)
                .await
                .is_err()
        );
        ws::close(&first, 1000, "next").await?;
        let second = second.await?;
        let stream = ws::messages(second.clone());
        ws::send(&second, ws::Message::Binary(Bytes::from_text("next"))).await?;
        assert!(
            matches!(stream.pull().await.unwrap()?, ws::Message::Binary(bytes) if bytes.values() == b"next")
        );
        task::stop(server).await?;
        assert!(stream.pull().await.unwrap().is_err());
        assert!(stream.pull().await.is_none());
        listener.close()?;
        Ok(())
    });
}

#[test]
fn websocket_client_rejects_wrong_handshake_accept() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let peer = task::run(async move {
            let mut peer = Peer::new(net::accept(&owner).await?)?;
            let head = peer.head().await?;
            assert!(head.starts_with("GET /chat HTTP/1.1"));
            assert!(head.to_ascii_lowercase().contains("sec-websocket-key:"));
            peer.write(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: wrong\r\n\r\n").await?;
            assert_eq!(net::read(&peer.socket, 1).await?, None);
            Ok(())
        }).await?;
        assert!(
            ws::connect("127.0.0.1", port, "/chat", ws_limits())
                .await
                .is_err()
        );
        task::wait(peer).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn client_frames_are_masked_and_server_upgrade_retains_preread_frame_bytes() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let peer = task::run(async move {
            let mut peer = Peer::new(net::accept(&owner).await?)?;
            let head = peer.head().await?;
            let key = head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("sec-websocket-key").then(|| value.trim())
            }).unwrap();
            // 只复用握手摘要；帧头、掩码及负载由独立观察器核验。
            let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
            peer.write(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await?;
            assert_eq!(peer.frame().await?, (0x81, true, b"masked".to_vec()));
            peer.send_frame(0x88, false, &[3, 232]).await?;
            assert_eq!(peer.frame().await?, (0x88, true, vec![3, 232]));
            Ok(())
        }).await?;
        let socket = ws::connect("127.0.0.1", port, "/", ws_limits()).await?;
        ws::send(&socket, ws::Message::Text("masked".into())).await?;
        assert!(ws::receive(&socket).await?.is_none());
        task::wait(peer).await?;
        listener.close()?;

        let (server, listener, port) = server(echo, limits(), live()).await?;
        let mut peer = Peer::connect(port).await?;
        let mut request = b"GET /ws HTTP/1.1\r\nHost: test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n".to_vec();
        request.extend_from_slice(&[0x81, 0x82, 0, 0, 0, 0, b'o', b'k']);
        peer.write(&request).await?;
        assert!(peer.head().await?.starts_with("HTTP/1.1 101"));
        assert_eq!(peer.frame().await?, (0x81, false, b"ok".to_vec()));
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn websocket_cancel_waiting_writer_keeps_active_frame_and_cancel_active_writer_closes() {
    run(async {
        let sockets = dever_runtime::channel::Channel::new(1)?;
        let sender = sockets.clone();
        let (server, listener, port) = server(
            move |_, reply| {
                let sender = sender.clone();
                async move {
                    let socket = ws::accept(
                        &reply,
                        ws::Limits {
                            message_bytes: 8 * 1024 * 1024,
                            write_ms: 3000,
                            ..ws_limits()
                        },
                    )
                    .await?;
                    sender.send(socket).await?;
                    std::future::pending().await
                }
            },
            limits(),
            live(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.upgrade().await?;
        let socket = sockets.receive().await?.unwrap();
        let first = ws::send(
            &socket,
            ws::Message::Binary(Bytes::new(vec![42; 8 * 1024 * 1024])),
        );
        tokio::pin!(first);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut first)
                .await
                .is_err()
        );
        let waiting = socket.clone();
        let waiting = task::run(async move {
            ws::send(&waiting, ws::Message::Text("must not appear".into())).await
        })
        .await?;
        tokio::task::yield_now().await;
        task::stop(waiting).await?;
        let reader = async { peer.frame().await };
        let (sent, frame) = tokio::join!(first, reader);
        sent?;
        let frame = frame?;
        assert_eq!(
            (frame.0, frame.1, frame.2.len()),
            (0x82, false, 8 * 1024 * 1024)
        );
        assert!(frame.2.iter().all(|byte| *byte == 42));
        let writing = socket.clone();
        let writing = task::run(async move {
            ws::send(
                &writing,
                ws::Message::Binary(Bytes::new(vec![17; 8 * 1024 * 1024])),
            )
            .await
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(30)).await;
        task::stop(writing).await?;
        assert!(
            ws::send(&socket, ws::Message::Text("closed".into()))
                .await
                .is_err()
        );
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn stopping_live_service_drains_started_blocking_descendants() {
    run(async {
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let signal = started.clone();
        let count = completed.clone();
        let (server, listener, port) = server(
            move |_, reply| {
                let signal = signal.clone();
                let count = count.clone();
                async move {
                    sse::start(&reply, vec![]).await?;
                    let child = task::run(async move {
                        task::blocking(move || {
                            signal.add_permits(1);
                            std::thread::sleep(Duration::from_millis(80));
                            count.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                        .await
                    })
                    .await?;
                    task::wait(child).await
                }
            },
            limits(),
            live(),
        )
        .await?;
        let peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await?;
        let _permit = started.acquire().await.unwrap();
        task::stop(server).await?;
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        listener.close()?;
        Ok(())
    });
}

#[test]
fn rejected_reply_operations_leave_the_committed_stream_open() {
    run(async {
        let (server, listener, port) = server(
            |_, reply| async move {
                assert!(
                    http::write(&reply, Bytes::from_text("too early"))
                        .await
                        .is_err()
                );
                assert!(http::start(&reply, 204, vec![]).await.is_err());
                http::start(&reply, 200, vec![]).await?;
                assert!(http::start(&reply, 201, vec![]).await.is_err());
                assert!(
                    http::respond(
                        &reply,
                        http::Response {
                            status: 200,
                            headers: vec![],
                            body: Bytes::from_text("duplicate")
                        }
                    )
                    .await
                    .is_err()
                );
                assert!(sse::start(&reply, vec![]).await.is_err());
                assert!(ws::accept(&reply, ws_limits()).await.is_err());
                assert!(http::write(&reply, Bytes::new(vec![0; 257])).await.is_err());
                tokio::task::yield_now().await;
                http::write(&reply, Bytes::from_text("still-open")).await?;
                http::finish(&reply).await
            },
            limits(),
            live(),
        )
        .await?;
        let mut peer = Peer::connect(port).await?;
        peer.write(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await?;
        assert!(peer.head().await?.starts_with("HTTP/1.1 200"));
        assert_eq!(peer.chunk().await?, b"still-open");
        assert!(peer.chunk().await?.is_empty());
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn websocket_independent_frames_cover_fragmentation_ping_binary_and_close() {
    run(async {
        let (server, listener, port) = server(echo, limits(), live()).await?;
        let mut peer = Peer::connect(port).await?;
        peer.upgrade().await?;
        peer.send_frame(0x01, true, &[0xe4]).await?;
        peer.send_frame(0x89, true, b"alive").await?;
        assert_eq!(peer.frame().await?, (0x8a, false, b"alive".to_vec()));
        peer.send_frame(0x80, true, &[0xbd, 0xa0]).await?;
        assert_eq!(peer.frame().await?, (0x81, false, "你".as_bytes().to_vec()));
        peer.send_frame(0x82, true, &[0, 255, 1]).await?;
        assert_eq!(peer.frame().await?, (0x82, false, vec![0, 255, 1]));
        peer.send_frame(0x88, true, &[3, 232]).await?;
        assert_eq!(peer.frame().await?, (0x88, false, vec![3, 232]));
        assert_eq!(net::read(&peer.socket, 1).await?, None);
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn websocket_client_is_full_duplex_and_close_wakes_pending_receiver() {
    run(async {
        let (server, listener, port) = server(echo, limits(), live()).await?;
        let socket = ws::connect("127.0.0.1", port, "/ws", ws_limits()).await?;
        let reader = socket.clone();
        let waiting = task::run(async move { ws::receive(&reader).await }).await?;
        ws::send(&socket, ws::Message::Text("duplex".into())).await?;
        assert!(
            matches!(task::wait(waiting).await?, Some(ws::Message::Text(text)) if text == "duplex")
        );
        let reader = socket.clone();
        let waiting = task::run(async move { Ok(ws::receive(&reader).await.is_err()) }).await?;
        ws::close(&socket, 1000, "done").await?;
        assert!(task::wait(waiting).await?);
        assert!(
            ws::send(&socket, ws::Message::Text("closed".into()))
                .await
                .is_err()
        );
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}
