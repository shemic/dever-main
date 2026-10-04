use std::future::{Future, poll_fn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use dever_runtime::{bytes::Bytes, http, net, sse, task, tls};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use tokio::io::AsyncReadExt;

const ROOT: &[u8] = include_bytes!("../fixtures/tls/root.pem");
const CERT: &[u8] = include_bytes!("../fixtures/tls/server.pem");
const KEY: &[u8] = include_bytes!("../fixtures/tls/server-key.pem");

fn bytes(value: &[u8]) -> Bytes {
    Bytes::new(value.to_vec())
}

fn limits(timeout_ms: i64) -> http::Limits {
    http::Limits {
        header_bytes: 8192,
        body_bytes: 65536,
        timeout_ms,
        connections: 2,
        http2: Some(http::Http2Limits {
            streams: 4,
            stream_window_bytes: 65535,
            connection_window_bytes: 65535,
        }),
    }
}

fn live() -> http::LiveLimits {
    http::LiveLimits {
        chunk_bytes: 1024,
        idle_ms: 1000,
        heartbeat_ms: 30,
    }
}

fn run(future: impl Future<Output = Result<(), String>>) {
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 1,
            max_blocking_threads: 2,
            task_capacity: 64,
        },
        async {
            tokio::time::timeout(Duration::from_secs(8), future)
                .await
                .map_err(|_| "HTTP/2 server test deadline exceeded".to_owned())?
        },
    )
    .unwrap();
}

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn request(port: i64, target: &str) -> hyper::Request<()> {
    hyper::Request::builder()
        .version(hyper::Version::HTTP_2)
        .uri(format!("http://localhost:{port}{target}"))
        .body(())
        .unwrap()
}

async fn connect(
    port: i64,
) -> Result<(h2::client::SendRequest<bytes::Bytes>, task::Task<()>), String> {
    let socket = tokio::net::TcpStream::connect(("127.0.0.1", port as u16))
        .await
        .map_err(error)?;
    let (sender, connection) = h2::client::handshake(socket).await.map_err(error)?;
    let driver = task::run(async move { connection.await.map_err(error) }).await?;
    Ok((sender, driver))
}

async fn connect_with_window(
    port: i64,
    stream_window: u32,
) -> Result<(h2::client::SendRequest<bytes::Bytes>, task::Task<()>), String> {
    let socket = tokio::net::TcpStream::connect(("127.0.0.1", port as u16))
        .await
        .map_err(error)?;
    let mut builder = h2::client::Builder::new();
    builder.initial_window_size(stream_window);
    let (sender, connection) = builder
        .handshake::<_, bytes::Bytes>(socket)
        .await
        .map_err(error)?;
    let driver = task::run(async move { connection.await.map_err(error) }).await?;
    Ok((sender, driver))
}

async fn start_request(
    sender: h2::client::SendRequest<bytes::Bytes>,
    request: hyper::Request<()>,
) -> Result<
    (
        h2::client::SendRequest<bytes::Bytes>,
        h2::client::ResponseFuture,
    ),
    String,
> {
    let mut sender = sender.ready().await.map_err(error)?;
    let (response, _) = sender.send_request(request, true).map_err(error)?;
    Ok((sender, response))
}

async fn response_body(response: hyper::Response<h2::RecvStream>) -> Result<Vec<u8>, String> {
    let mut body = response.into_body();
    let mut result = Vec::new();
    while let Some(chunk) = body.data().await {
        result.extend_from_slice(&chunk.map_err(error)?);
    }
    Ok(result)
}

async fn wait_until(mut condition: impl FnMut() -> bool) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "condition was not reached".to_owned())
}

#[test]
fn buffered_server_multiplexes_and_preserves_http2_request_semantics() {
    run(async {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let invoked = Arc::new(AtomicUsize::new(0));
        let route_gate = gate.clone();
        let route_invoked = invoked.clone();
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let server = task::run(http::serve(
            move |request| {
                let gate = route_gate.clone();
                let invoked = route_invoked.clone();
                dever_runtime::api::scoped_request(request, move |request| async move {
                    invoked.fetch_add(1, Ordering::SeqCst);
                    let path = request.target.split('?').next().unwrap().to_owned();
                    let request_id = dever_runtime::api::request_id()?;
                    assert_eq!(
                        dever_runtime::api::client_address()?,
                        Some("127.0.0.1".into())
                    );
                    assert!(request.headers.iter().any(|header| {
                        header.name == "host"
                            && header.value.values() == format!("localhost:{port}").as_bytes()
                    }));
                    if request.target.starts_with("/slow") {
                        let _ = gate.acquire().await.map_err(error)?;
                    }
                    assert_eq!(dever_runtime::api::path()?, path);
                    assert_eq!(dever_runtime::api::request_id()?, request_id);
                    dever_runtime::api::response_header("x-api-path", &path)?;
                    let response = http::Response {
                        status: 200,
                        headers: vec![],
                        body: Bytes::from_string(request.target),
                    };
                    dever_runtime::api::commit_response_metadata()?;
                    Ok(response)
                })
            },
            owner,
            http::Limits {
                body_bytes: 16,
                ..limits(500)
            },
        ))
        .await?;
        let (sender, driver) = connect(port).await?;
        let (sender, slow) = start_request(sender, request(port, "/slow?value=1")).await?;
        let (sender, fast) = start_request(sender, request(port, "/fast")).await?;
        let fast = tokio::time::timeout(Duration::from_millis(200), fast)
            .await
            .map_err(|_| "HTTP/2 streams were serialized".to_owned())?
            .map_err(error)?;
        assert_eq!(fast.status(), hyper::StatusCode::OK);
        assert_eq!(fast.headers().get("x-api-path").unwrap(), "/fast");
        assert_eq!(response_body(fast).await?, b"/fast");
        gate.add_permits(1);
        let slow = slow.await.map_err(error)?;
        assert_eq!(slow.status(), hyper::StatusCode::OK);
        assert_eq!(slow.headers().get("x-api-path").unwrap(), "/slow");
        assert_eq!(response_body(slow).await?, b"/slow?value=1");

        let mismatched = hyper::Request::builder()
            .version(hyper::Version::HTTP_2)
            .uri(format!("http://localhost:{port}/mismatch"))
            .header(hyper::header::HOST, "other.example")
            .body(())
            .unwrap();
        let (sender, rejected) = start_request(sender, mismatched).await?;
        let rejected = rejected.await.map_err(error)?;
        assert_eq!(rejected.status(), hyper::StatusCode::BAD_REQUEST);
        assert!(!rejected.headers().contains_key(hyper::header::CONNECTION));
        assert_eq!(response_body(rejected).await?, b"");
        assert_eq!(invoked.load(Ordering::SeqCst), 2);

        let mut sender = sender.ready().await.map_err(error)?;
        let oversized = hyper::Request::builder()
            .method(hyper::Method::POST)
            .version(hyper::Version::HTTP_2)
            .uri(format!("http://localhost:{port}/oversized"))
            .body(())
            .unwrap();
        let (oversized, mut upload) = sender.send_request(oversized, false).map_err(error)?;
        upload
            .send_data(bytes::Bytes::from_static(b"12345678901234567"), true)
            .map_err(error)?;
        drop(upload);
        let oversized = oversized.await.map_err(error)?;
        assert_eq!(oversized.status(), hyper::StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response_body(oversized).await?, b"");
        assert_eq!(invoked.load(Ordering::SeqCst), 2);

        let (sender, neighbor) = start_request(sender, request(port, "/neighbor")).await?;
        assert_eq!(
            response_body(neighbor.await.map_err(error)?).await?,
            b"/neighbor"
        );
        assert_eq!(invoked.load(Ordering::SeqCst), 3);
        drop(sender);
        task::wait(driver).await?;
        listener.close()?;
        task::wait(server).await
    });
}

struct Finished(Arc<AtomicUsize>);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Marked(Arc<AtomicBool>);

impl Drop for Marked {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn resetting_one_live_stream_keeps_sse_neighbor_and_drains_blocking_work() {
    run(async {
        let finished = Arc::new(AtomicUsize::new(0));
        let blocking_started = Arc::new(AtomicBool::new(false));
        let blocking_done = Arc::new(AtomicBool::new(false));
        let route_finished = finished.clone();
        let route_blocking_started = blocking_started.clone();
        let route_blocking_done = blocking_done.clone();
        let stop_gate = Arc::new(tokio::sync::Semaphore::new(0));
        let stop_started = Arc::new(AtomicBool::new(false));
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let owner_stop_gate = stop_gate.clone();
        let owner_stop_started = stop_started.clone();
        let server_owner = task::run(async move {
            let server = task::run(http::serve_live(
                move |request, reply| {
                    let finished = route_finished.clone();
                    let blocking_started = route_blocking_started.clone();
                    let blocking_done = route_blocking_done.clone();
                    async move {
                        let _finished = Finished(finished.clone());
                        match request.target.as_str() {
                            "/pending" => {
                                task::parallel(move || {
                                    blocking_started.store(true, Ordering::SeqCst);
                                    std::thread::sleep(Duration::from_millis(500));
                                    blocking_done.store(true, Ordering::SeqCst);
                                    Ok(())
                                })
                                .await?;
                                std::future::pending().await
                            }
                            "/events" => {
                                sse::start(&reply, vec![]).await?;
                                sse::send(
                                    &reply,
                                    sse::Event {
                                        event: "update".into(),
                                        data: "neighbor".into(),
                                        id: None,
                                        retry_ms: None,
                                    },
                                )
                                .await?;
                                std::future::pending().await
                            }
                            _ => {
                                http::respond(
                                    &reply,
                                    http::Response {
                                        status: 200,
                                        headers: vec![],
                                        body: bytes(b"still-open"),
                                    },
                                )
                                .await
                            }
                        }
                    }
                },
                owner,
                limits(1000),
                live(),
            ))
            .await?;
            let _ = owner_stop_gate.acquire().await.map_err(error)?;
            owner_stop_started.store(true, Ordering::SeqCst);
            task::stop(server).await
        })
        .await?;
        let (sender, driver) = connect(port).await?;
        let (sender, pending) = start_request(sender, request(port, "/pending")).await?;
        wait_until(|| blocking_started.load(Ordering::SeqCst)).await?;
        drop(pending);

        let (sender, events) = start_request(sender, request(port, "/events")).await?;
        let mut events = events.await.map_err(error)?.into_body();
        assert_eq!(
            events.data().await.unwrap().map_err(error)?,
            b"event: update\ndata: neighbor\n\n"[..]
        );

        let (sender, final_response) = start_request(sender, request(port, "/after-reset")).await?;
        assert_eq!(
            response_body(final_response.await.map_err(error)?).await?,
            b"still-open"
        );
        drop(events);
        stop_gate.add_permits(1);
        wait_until(|| stop_started.load(Ordering::SeqCst)).await?;
        task::stop(server_owner).await?;
        assert!(blocking_done.load(Ordering::SeqCst));
        drop(sender);
        let _ = task::wait(driver).await;
        listener.close()?;
        assert!(finished.load(Ordering::SeqCst) >= 3);
        Ok(())
    });
}

#[test]
fn request_faults_return_500_without_stopping_the_http2_connection() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let server = task::run(http::serve(
            |request| async move {
                if request.target == "/fault" {
                    return Err("buffered handler fault".to_owned());
                }
                Ok(http::Response {
                    status: 200,
                    headers: vec![],
                    body: bytes(b"still-open"),
                })
            },
            owner,
            limits(500),
        ))
        .await?;
        let (sender, driver) = connect(port).await?;
        let (sender, response) = start_request(sender, request(port, "/fault")).await?;
        let response = response.await.map_err(error)?;
        assert_eq!(response.status(), hyper::StatusCode::INTERNAL_SERVER_ERROR);
        let (sender, response) = start_request(sender, request(port, "/after-fault")).await?;
        assert_eq!(
            response_body(response.await.map_err(error)?).await?,
            b"still-open"
        );
        drop(sender);
        task::stop(server).await?;
        let _ = task::wait(driver).await;
        listener.close()?;

        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let server = task::run(http::serve_live(
            |request, reply| async move {
                if request.target == "/fault" {
                    return Err("live handler fault".to_owned());
                }
                http::respond(
                    &reply,
                    http::Response {
                        status: 200,
                        headers: vec![],
                        body: bytes(b"still-open"),
                    },
                )
                .await
            },
            owner,
            limits(500),
            live(),
        ))
        .await?;
        let (sender, driver) = connect(port).await?;
        let (sender, response) = start_request(sender, request(port, "/fault")).await?;
        let response = response.await.map_err(error)?;
        assert_eq!(response.status(), hyper::StatusCode::INTERNAL_SERVER_ERROR);
        let (sender, response) = start_request(sender, request(port, "/after-fault")).await?;
        assert_eq!(
            response_body(response.await.map_err(error)?).await?,
            b"still-open"
        );
        drop(sender);
        task::stop(server).await?;
        let _ = task::wait(driver).await;
        listener.close()?;

        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let server = task::run(http::serve(
            |request| async move {
                if request.target != "/invalid-response" {
                    return Ok(http::Response {
                        status: 200,
                        headers: vec![],
                        body: bytes(b"still-open"),
                    });
                }
                Ok(http::Response {
                    status: 200,
                    headers: vec![http::Header {
                        name: "te".into(),
                        value: bytes(b"trailers"),
                    }],
                    body: bytes(b""),
                })
            },
            owner,
            limits(500),
        ))
        .await?;
        let (sender, driver) = connect(port).await?;
        let (sender, response) = start_request(sender, request(port, "/invalid-response")).await?;
        let response = response.await.map_err(error)?;
        assert_eq!(response.status(), hyper::StatusCode::INTERNAL_SERVER_ERROR);
        let (sender, response) = start_request(sender, request(port, "/after-invalid")).await?;
        assert_eq!(
            response_body(response.await.map_err(error)?).await?,
            b"still-open"
        );
        drop(sender);
        task::stop(server).await?;
        let _ = task::wait(driver).await;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn flow_controlled_stream_does_not_block_its_neighbor_and_resumes_on_window_update() {
    run(async {
        let body_finished = Arc::new(AtomicBool::new(false));
        let reset_dropped = Arc::new(AtomicBool::new(false));
        let reset_writes = Arc::new(AtomicUsize::new(0));
        let route_finished = body_finished.clone();
        let route_reset_dropped = reset_dropped.clone();
        let route_reset_writes = reset_writes.clone();
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let server = task::run(http::serve_live(
            move |request, reply| {
                let body_finished = route_finished.clone();
                let reset_dropped = route_reset_dropped.clone();
                let reset_writes = route_reset_writes.clone();
                async move {
                    match request.target.as_str() {
                        "/large" => {
                            http::start(&reply, 200, vec![]).await?;
                            for _ in 0..4 {
                                http::write(&reply, Bytes::new(vec![7; 16384])).await?;
                            }
                            http::finish(&reply).await?;
                            body_finished.store(true, Ordering::SeqCst);
                            Ok(())
                        }
                        "/reset-write" => {
                            let _dropped = Marked(reset_dropped);
                            http::start(&reply, 200, vec![]).await?;
                            for _ in 0..16 {
                                reset_writes.fetch_add(1, Ordering::SeqCst);
                                http::write(&reply, Bytes::new(vec![8; 16384])).await?;
                            }
                            http::finish(&reply).await
                        }
                        _ => {
                            http::respond(
                                &reply,
                                http::Response {
                                    status: 200,
                                    headers: vec![],
                                    body: bytes(b"neighbor"),
                                },
                            )
                            .await
                        }
                    }
                }
            },
            owner,
            limits(500),
            http::LiveLimits {
                chunk_bytes: 16384,
                ..live()
            },
        ))
        .await?;
        let (sender, driver) = connect_with_window(port, 1024).await?;
        let (sender, large) = start_request(sender, request(port, "/large")).await?;
        let mut large = large.await.map_err(error)?.into_body();
        let (sender, neighbor) = start_request(sender, request(port, "/neighbor")).await?;
        let neighbor = tokio::time::timeout(Duration::from_millis(200), neighbor)
            .await
            .map_err(|_| "flow-controlled stream blocked its neighbor".to_owned())?
            .map_err(error)?;
        assert_eq!(response_body(neighbor).await?, b"neighbor");
        assert!(!body_finished.load(Ordering::SeqCst));

        let mut received = 0;
        let mut flow = large.flow_control().clone();
        while let Some(chunk) = large.data().await {
            let chunk = chunk.map_err(error)?;
            received += chunk.len();
            flow.release_capacity(chunk.len()).map_err(error)?;
        }
        assert_eq!(received, 4 * 16384);
        wait_until(|| body_finished.load(Ordering::SeqCst)).await?;
        drop(flow);
        drop(large);

        let (sender, reset) = start_request(sender, request(port, "/reset-write")).await?;
        let reset = reset.await.map_err(error)?.into_body();
        wait_until(|| reset_writes.load(Ordering::SeqCst) >= 3).await?;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(reset_writes.load(Ordering::SeqCst) < 16);
        drop(reset);
        wait_until(|| reset_dropped.load(Ordering::SeqCst)).await?;
        let (sender, neighbor) = start_request(sender, request(port, "/after-reset-write")).await?;
        assert_eq!(
            response_body(neighbor.await.map_err(error)?).await?,
            b"neighbor"
        );

        drop(sender);
        task::wait(driver).await?;
        listener.close()?;
        task::wait(server).await
    });
}

#[test]
fn listener_close_sends_goaway_and_drains_an_active_stream() {
    run(async {
        let entered = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let route_entered = entered.clone();
        let route_gate = gate.clone();
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let server = task::run(http::serve(
            move |_| {
                let entered = route_entered.clone();
                let gate = route_gate.clone();
                async move {
                    entered.store(true, Ordering::SeqCst);
                    let _ = gate.acquire().await.map_err(error)?;
                    Ok(http::Response {
                        status: 200,
                        headers: vec![],
                        body: bytes(b"drained"),
                    })
                }
            },
            owner,
            limits(500),
        ))
        .await?;
        let (sender, driver) = connect(port).await?;
        let (mut sender, response) = start_request(sender, request(port, "/active")).await?;
        wait_until(|| entered.load(Ordering::SeqCst)).await?;
        listener.close()?;
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if poll_fn(|context| sender.poll_ready(context)).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "client did not observe server GOAWAY".to_owned())?;
        gate.add_permits(1);
        assert_eq!(
            response_body(response.await.map_err(error)?).await?,
            b"drained"
        );
        drop(sender);
        task::wait(driver).await?;
        task::wait(server).await
    });
}

#[test]
fn silent_peer_times_out_and_tls_negotiates_h2() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let server = task::run(http::serve(
            |_| async {
                Ok(http::Response {
                    status: 200,
                    headers: vec![],
                    body: bytes(b"ok"),
                })
            },
            owner,
            limits(80),
        ))
        .await?;
        let mut silent = tokio::net::TcpStream::connect(("127.0.0.1", port as u16))
            .await
            .map_err(error)?;
        tokio::time::sleep(Duration::from_millis(160)).await;
        let mut received = Vec::new();
        tokio::time::timeout(
            Duration::from_millis(500),
            silent.read_to_end(&mut received),
        )
        .await
        .map_err(|_| "silent HTTP/2 peer was not closed after its deadline".to_owned())?
        .map_err(error)?;
        let (sender, driver) = connect(port).await?;
        let (sender, response) = start_request(sender, request(port, "/after-timeout")).await?;
        assert_eq!(response_body(response.await.map_err(error)?).await?, b"ok");
        drop(sender);
        task::wait(driver).await?;
        listener.close()?;
        task::wait(server).await?;

        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let owner = listener.clone();
        let server = task::run(http::serve_tls(
            |_| async {
                Ok(http::Response {
                    status: 200,
                    headers: vec![],
                    body: bytes(b"tls"),
                })
            },
            owner,
            limits(500),
            tls::server(&bytes(CERT), &bytes(KEY))?,
        ))
        .await?;
        let socket = tokio::net::TcpStream::connect(("127.0.0.1", port as u16))
            .await
            .map_err(error)?;
        let mut roots = RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(ROOT) {
            roots.add(certificate.map_err(error)?).map_err(error)?;
        }
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let name = ServerName::try_from("localhost".to_owned()).map_err(error)?;
        let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(name, socket)
            .await
            .map_err(error)?;
        assert_eq!(stream.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
        let (sender, connection) = h2::client::handshake(stream).await.map_err(error)?;
        let driver = task::run(async move { connection.await.map_err(error) }).await?;
        let (sender, response) = start_request(sender, request(port, "/tls")).await?;
        assert_eq!(response_body(response.await.map_err(error)?).await?, b"tls");
        drop(sender);
        task::wait(driver).await?;
        listener.close()?;
        task::wait(server).await
    });
}
