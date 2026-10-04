use std::future::{Future, poll_fn};
use std::time::Duration;

use dever_runtime::{
    async_stream::AsyncStream, bytes::Bytes, channel::Channel, http, net, task, tls,
};
use tokio::sync::{mpsc, oneshot};

const ROOT: &[u8] = include_bytes!("../fixtures/tls/root.pem");
const CERT: &[u8] = include_bytes!("../fixtures/tls/server.pem");
const KEY: &[u8] = include_bytes!("../fixtures/tls/server-key.pem");

fn bytes(value: &[u8]) -> Bytes {
    Bytes::new(value.to_vec())
}

fn limits(connections: i64, streams: i64) -> http::Limits {
    http::Limits {
        header_bytes: 8192,
        body_bytes: 65536,
        timeout_ms: 1000,
        connections,
        http2: Some(http::Http2Limits {
            streams,
            stream_window_bytes: 65535,
            connection_window_bytes: 65535,
        }),
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
                .map_err(|_| "HTTP/2 client test deadline exceeded".to_owned())?
        },
    )
    .unwrap();
}

fn h2_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

async fn next_request(
    requests: &mut mpsc::Receiver<(
        hyper::Request<h2::RecvStream>,
        h2::server::SendResponse<bytes::Bytes>,
    )>,
) -> Result<
    (
        hyper::Request<h2::RecvStream>,
        h2::server::SendResponse<bytes::Bytes>,
    ),
    String,
> {
    requests
        .recv()
        .await
        .ok_or("HTTP/2 connection closed before request".into())
}

fn response() -> hyper::Response<()> {
    hyper::Response::builder().status(200).body(()).unwrap()
}

async fn start_peer(
    socket: tokio::net::TcpStream,
) -> Result<
    (
        mpsc::Receiver<(
            hyper::Request<h2::RecvStream>,
            h2::server::SendResponse<bytes::Bytes>,
        )>,
        oneshot::Sender<oneshot::Sender<()>>,
        task::Task<()>,
    ),
    String,
> {
    let mut connection = h2::server::handshake(socket).await.map_err(h2_error)?;
    let (requests, receiver) = mpsc::channel(4);
    let (shutdown, mut shutdown_request) = oneshot::channel::<oneshot::Sender<()>>();
    let driver = task::run(async move {
        let mut graceful = false;
        loop {
            let request = if graceful {
                connection.accept().await
            } else {
                tokio::select! {
                    request = connection.accept() => request,
                    command = &mut shutdown_request => {
                        if let Ok(acknowledge) = command {
                            connection.graceful_shutdown();
                            graceful = true;
                            let _ = acknowledge.send(());
                            continue;
                        }
                        connection.accept().await
                    }
                }
            };
            let Some(request) = request else {
                return Ok(());
            };
            requests
                .send(request.map_err(h2_error)?)
                .await
                .map_err(|_| "HTTP/2 peer request receiver closed".to_owned())?;
        }
    })
    .await?;
    Ok((receiver, shutdown, driver))
}

#[test]
fn one_physical_connection_multiplexes_upload_and_streaming_download() {
    run(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(h2_error)?;
        let port = i64::from(listener.local_addr().map_err(h2_error)?.port());
        let peer = task::run(async move {
            let (socket, _) = listener.accept().await.map_err(h2_error)?;
            let (mut requests, _shutdown, driver) = start_peer(socket).await?;

            let (first, mut first_reply) = next_request(&mut requests).await?;
            assert_eq!(first.method(), hyper::Method::POST);
            assert_eq!(first.uri().scheme_str(), Some("http"));
            let authority = format!("localhost:{port}");
            assert_eq!(
                first.uri().authority().map(|value| value.as_str()),
                Some(authority.as_str())
            );
            assert_eq!(first.uri().path(), "/upload");
            let mut upload = first.into_body();
            let mut uploaded = Vec::new();
            while let Some(chunk) = upload.data().await {
                let chunk = chunk.map_err(h2_error)?;
                uploaded.extend_from_slice(&chunk);
            }
            assert_eq!(uploaded, b"abcdefgh");
            let mut first_body = first_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            first_body
                .send_data(bytes::Bytes::from_static(b"first"), false)
                .map_err(h2_error)?;

            let (second, mut second_reply) = next_request(&mut requests).await?;
            assert_eq!(second.uri().path(), "/second");
            let mut second_body = second_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            second_body
                .send_data(bytes::Bytes::from_static(b"second"), true)
                .map_err(h2_error)?;
            first_body
                .send_data(bytes::Bytes::from_static(b"last"), true)
                .map_err(h2_error)?;

            task::wait(driver).await?;
            Ok(())
        })
        .await?;

        let client = http::client(
            &format!("http://localhost:{port}"),
            tls::system(),
            limits(1, 2),
            pool_limits(),
        )
        .await?;
        let first = http::open_stream(
            &client,
            http::StreamRequest {
                method: "POST".into(),
                target: "/upload".into(),
                headers: vec![],
                body: AsyncStream::from_values([Ok(bytes(b"abcd")), Ok(bytes(b"efgh"))]),
            },
        )
        .await?;
        assert_eq!(first.body.pull().await.unwrap()?.values(), b"firs");
        assert_eq!(first.body.pull().await.unwrap()?.values(), b"t");
        assert_eq!(
            http::request(&client, request("/second"))
                .await?
                .body
                .values(),
            b"second"
        );
        let mut tail = Vec::new();
        while let Some(chunk) = first.body.pull().await {
            tail.extend_from_slice(chunk?.values());
        }
        assert_eq!(tail, b"last");
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn canceling_one_response_resets_only_its_stream() {
    run(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(h2_error)?;
        let port = i64::from(listener.local_addr().map_err(h2_error)?.port());
        let peer = task::run(async move {
            let (socket, _) = listener.accept().await.map_err(h2_error)?;
            let (mut requests, _shutdown, driver) = start_peer(socket).await?;
            let (_, mut first_reply) = next_request(&mut requests).await?;
            let mut first_body = first_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            first_body
                .send_data(bytes::Bytes::from_static(b"open"), false)
                .map_err(h2_error)?;

            let (_, mut neighbor_reply) = next_request(&mut requests).await?;
            let mut neighbor_body = neighbor_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            neighbor_body
                .send_data(bytes::Bytes::from_static(b"neighbor"), true)
                .map_err(h2_error)?;

            let reason = poll_fn(|context| first_body.poll_reset(context))
                .await
                .map_err(h2_error)?;
            assert_eq!(reason, h2::Reason::CANCEL);

            let (_, mut final_reply) = next_request(&mut requests).await?;
            let mut final_body = final_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            final_body
                .send_data(bytes::Bytes::from_static(b"still-open"), true)
                .map_err(h2_error)?;
            task::wait(driver).await?;
            Ok(())
        })
        .await?;

        let client = http::client(
            &format!("http://localhost:{port}"),
            tls::system(),
            limits(1, 2),
            pool_limits(),
        )
        .await?;
        let first = http::open(&client, request("/cancel")).await?;
        assert_eq!(first.body.pull().await.unwrap()?.values(), b"open");
        assert_eq!(
            http::request(&client, request("/neighbor"))
                .await?
                .body
                .values(),
            b"neighbor"
        );
        first.body.close();
        assert_eq!(
            http::request(&client, request("/after-reset"))
                .await?
                .body
                .values(),
            b"still-open"
        );
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn stream_limit_blocks_without_expiring_an_active_body_then_reaps_at_eof() {
    run(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(h2_error)?;
        let port = i64::from(listener.local_addr().map_err(h2_error)?.port());
        let finish_first = Channel::new(1)?;
        let finish = finish_first.clone();
        let first_connection_closed = Channel::new(1)?;
        let closed = first_connection_closed.clone();
        let peer = task::run(async move {
            let (first_socket, _) = listener.accept().await.map_err(h2_error)?;
            let (mut first_requests, _shutdown, first_driver) = start_peer(first_socket).await?;
            let (_, mut first_reply) = next_request(&mut first_requests).await?;
            let mut first_body = first_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            first_body
                .send_data(bytes::Bytes::from_static(b"open"), false)
                .map_err(h2_error)?;
            finish.receive().await?;
            assert!(matches!(
                first_requests.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
            first_body
                .send_data(bytes::Bytes::from_static(b"done"), true)
                .map_err(h2_error)?;
            task::wait(first_driver).await?;
            closed.send(()).await?;

            let (second_socket, _) = listener.accept().await.map_err(h2_error)?;
            let (mut second_requests, _shutdown, second_driver) = start_peer(second_socket).await?;
            let (_, mut second_reply) = next_request(&mut second_requests).await?;
            let mut second_body = second_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            second_body
                .send_data(bytes::Bytes::from_static(b"new"), true)
                .map_err(h2_error)?;
            task::wait(second_driver).await?;
            Ok(())
        })
        .await?;

        let mut budget = limits(1, 1);
        budget.timeout_ms = 60;
        let mut pool = pool_limits();
        pool.idle_ms = 30;
        let client = http::client(
            &format!("http://localhost:{port}"),
            tls::system(),
            budget,
            pool,
        )
        .await?;
        let first = http::open(&client, request("/active")).await?;
        assert_eq!(first.body.pull().await.unwrap()?.values(), b"open");
        task::sleep(80).await?;
        assert!(
            http::request(&client, request("/blocked"))
                .await
                .unwrap_err()
                .contains("timed out")
        );
        finish_first.send(()).await?;
        assert_eq!(first.body.pull().await.unwrap()?.values(), b"done");
        assert!(first.body.pull().await.is_none());
        first_connection_closed.receive().await?;
        assert_eq!(
            http::request(&client, request("/after-idle"))
                .await?
                .body
                .values(),
            b"new"
        );
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn goaway_replaces_the_connection_within_the_physical_budget() {
    run(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(h2_error)?;
        let port = i64::from(listener.local_addr().map_err(h2_error)?.port());
        let goaway = Channel::new(1)?;
        let goaway_sent = goaway.clone();
        let finish_old_stream = Channel::new(1)?;
        let finish = finish_old_stream.clone();
        let peer = task::run(async move {
            let (first_socket, _) = listener.accept().await.map_err(h2_error)?;
            let (mut first_requests, shutdown, first_driver) = start_peer(first_socket).await?;
            let (_, mut first_reply) = next_request(&mut first_requests).await?;
            let mut first_body = first_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            first_body
                .send_data(bytes::Bytes::from_static(b"open"), false)
                .map_err(h2_error)?;
            let (acknowledge, acknowledged) = oneshot::channel();
            shutdown
                .send(acknowledge)
                .map_err(|_| "HTTP/2 peer driver closed before GOAWAY".to_owned())?;
            acknowledged
                .await
                .map_err(|_| "HTTP/2 peer did not begin GOAWAY".to_owned())?;
            task::sleep(100).await?;
            goaway_sent.send(()).await?;
            finish.receive().await?;
            assert!(matches!(
                first_requests.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
            first_body
                .send_data(bytes::Bytes::from_static(b"done"), true)
                .map_err(h2_error)?;
            task::wait(first_driver).await?;

            let (second_socket, _) = listener.accept().await.map_err(h2_error)?;
            let (mut second_requests, _shutdown, second_driver) = start_peer(second_socket).await?;
            let (_, mut second_reply) = next_request(&mut second_requests).await?;
            let mut second_body = second_reply
                .send_response(response(), false)
                .map_err(h2_error)?;
            second_body
                .send_data(bytes::Bytes::from_static(b"second"), true)
                .map_err(h2_error)?;
            task::wait(second_driver).await?;
            Ok(())
        })
        .await?;

        let mut budget = limits(1, 2);
        budget.timeout_ms = 80;
        let client = http::client(
            &format!("http://localhost:{port}"),
            tls::system(),
            budget,
            pool_limits(),
        )
        .await?;
        let first = http::open(&client, request("/before-goaway")).await?;
        assert_eq!(first.body.pull().await.unwrap()?.values(), b"open");
        goaway.receive().await?;
        let blocked = http::request(&client, request("/must-wait"))
            .await
            .unwrap_err();
        assert!(blocked.contains("timed out"), "{blocked}");
        finish_old_stream.send(()).await?;
        assert_eq!(first.body.pull().await.unwrap()?.values(), b"done");
        assert!(first.body.pull().await.is_none());
        assert_eq!(
            http::request(&client, request("/after-goaway"))
                .await?
                .body
                .values(),
            b"second"
        );
        http::close_client(&client).await?;
        task::wait(peer).await?;
        Ok(())
    });
}

#[test]
fn https_requires_h2_alpn_without_protocol_fallback() {
    run(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let mut http1 = limits(1, 1);
        http1.http2 = None;
        let server_listener = listener.clone();
        let server = task::run(async move {
            http::serve_tls(
                |_| async {
                    Ok(http::Response {
                        status: 200,
                        headers: vec![],
                        body: bytes(b"unexpected"),
                    })
                },
                server_listener,
                http1,
                identity(),
            )
            .await
        })
        .await?;
        let client = http::client(
            &format!("https://localhost:{port}"),
            trust(),
            limits(1, 1),
            pool_limits(),
        )
        .await?;
        let error = http::request(&client, request("/alpn")).await.unwrap_err();
        assert!(error.contains("NoApplicationProtocol"), "{error}");
        http::close_client(&client).await?;
        task::stop(server).await?;
        listener.close()?;
        Ok(())
    });
}

#[test]
fn invalid_http2_budgets_and_outgoing_headers_fail_before_connecting() {
    run(async {
        let base = limits(1, 2);
        let mut cases = Vec::new();
        for streams in [0, 65537] {
            let mut budget = base;
            budget.http2.as_mut().unwrap().streams = streams;
            cases.push((budget, "connections * streams"));
        }
        let mut multiplied = base;
        multiplied.connections = 32769;
        cases.push((multiplied, "connections * streams"));
        for window in [0, i64::from(i32::MAX) + 1] {
            let mut budget = base;
            budget.http2.as_mut().unwrap().stream_window_bytes = window;
            cases.push((budget, "stream_window_bytes"));
        }
        for window in [65534, i64::from(i32::MAX) + 1] {
            let mut budget = base;
            budget.http2.as_mut().unwrap().connection_window_bytes = window;
            cases.push((budget, "connection_window_bytes"));
        }
        for (budget, expected) in cases {
            let failure = http::client("http://127.0.0.1:1", tls::system(), budget, pool_limits())
                .await
                .unwrap_err();
            assert!(failure.contains(expected), "{failure}");
        }

        let client = http::client("http://127.0.0.1:1", tls::system(), base, pool_limits()).await?;
        for (name, value) in [
            ("Connection", "close"),
            ("Keep-Alive", "timeout=5"),
            ("TE", "gzip"),
        ] {
            let mut outgoing = request("/");
            outgoing.headers.push(http::Header {
                name: name.into(),
                value: bytes(value.as_bytes()),
            });
            let failure = http::request(&client, outgoing).await.unwrap_err();
            assert!(
                failure.contains("HTTP/2 forbids connection-specific header"),
                "{failure}"
            );
        }
        http::close_client(&client).await?;
        Ok(())
    });
}
