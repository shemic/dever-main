use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{CONNECTION, HOST, UPGRADE};
use hyper::server::conn::{http1, http2};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Version};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout, timeout_at};

use crate::config::{
    BODY_BYTES, HEADER_BYTES, HTTP2_FRAME_BYTES, Http2Config, HttpVersion, ServiceConfig,
    ServiceKind,
};
use crate::{response, tls};

pub fn run(kind: ServiceKind) -> Result<(), String> {
    let settings = crate::settings::Settings::load()?;
    let config = ServiceConfig::from_settings(&settings, kind)?;
    if kind.uses_runtime() {
        run_runtime(config, kind)
    } else {
        run_hyper(config, kind)
    }
}

fn run_runtime(config: ServiceConfig, kind: ServiceKind) -> Result<(), String> {
    let server_tls = config
        .cert_path
        .as_deref()
        .zip(config.key_path.as_deref())
        .map(|(cert, key)| tls::runtime_server(cert, key))
        .transpose()?;
    let runtime = dever_runtime::task::RuntimeConfig {
        worker_threads: config.workers,
        max_blocking_threads: 2,
        task_capacity: config.task_capacity,
    };
    dever_runtime::task::run_entry_with(runtime, async move {
        let responses = response::RuntimeResponses::new();
        let listener = dever_runtime::net::listen("127.0.0.1", 0).await?;
        let port = dever_runtime::net::port(&listener)?;
        ready(u16::try_from(port).map_err(|_| "listener returned an invalid port")?)?;
        let limits = dever_runtime::http::Limits {
            header_bytes: HEADER_BYTES as i64,
            body_bytes: BODY_BYTES as i64,
            timeout_ms: config.timeout.as_millis() as i64,
            connections: config.connections as i64,
            http2: config.http2.map(|limits| dever_runtime::http::Http2Limits {
                streams: limits.streams as i64,
                stream_window_bytes: i64::from(limits.stream_window_bytes),
                connection_window_bytes: i64::from(limits.connection_window_bytes),
            }),
        };
        let owned = listener.clone();
        let route = move |request: dever_runtime::http::Request| {
            let response = responses.response(&request.target);
            async move { Ok(response) }
        };
        let server: Pin<Box<dyn Future<Output = Result<(), String>> + Send>> = match server_tls {
            Some(tls) if kind.uses_tls() => {
                Box::pin(dever_runtime::http::serve_tls(route, owned, limits, tls))
            }
            _ => Box::pin(dever_runtime::http::serve(route, owned, limits)),
        };
        tokio::pin!(server);
        tokio::select! {
            result = server.as_mut() => result,
            _ = tokio::time::sleep(config.lifetime) => {
                listener.close()?;
                timeout(config.timeout, server.as_mut())
                    .await
                    .map_err(|_| "runtime server shutdown timed out".to_owned())?
            }
        }
    })
}

fn run_hyper(config: ServiceConfig, kind: ServiceKind) -> Result<(), String> {
    let server_tls = config
        .cert_path
        .as_deref()
        .zip(config.key_path.as_deref())
        .map(|(cert, key)| tls::server(cert, key, kind.http_version()))
        .transpose()?;
    let runtime = Builder::new_multi_thread()
        .worker_threads(config.workers)
        .max_blocking_threads(1)
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| format!("cannot start benchmark runtime: {error}"))?;
    runtime.block_on(hyper_server(
        config,
        kind.http_version(),
        kind.uses_tls().then_some(server_tls).flatten(),
    ))
}

async fn hyper_server(
    config: ServiceConfig,
    http_version: HttpVersion,
    server_tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<(), String> {
    let responses = Arc::new(response::HyperResponses::new());
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| error.to_string())?;
    ready(
        listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port(),
    )?;

    let connections = Arc::new(Semaphore::new(config.connections));
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let mut tasks = JoinSet::new();
    let deadline = Instant::now() + config.lifetime;

    loop {
        while let Some(result) = tasks.try_join_next() {
            result.map_err(|error| format!("connection task failed: {error}"))??;
        }
        let connections = connections.clone();
        let accepted = timeout_at(deadline, async {
            let permit = connections
                .acquire_owned()
                .await
                .map_err(|_| "connection limit closed".to_owned())?;
            let (stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
            stream
                .set_nodelay(true)
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((permit, stream))
        })
        .await;
        let Ok(accepted) = accepted else {
            break;
        };
        let (permit, stream) = accepted?;
        let connection_config = config.clone();
        let connection_tls = server_tls.clone();
        let connection_shutdown = shutdown_receiver.clone();
        let connection_responses = responses.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let stream = match tls::accept(
                stream,
                connection_config.timeout,
                connection_tls.as_ref(),
                http_version,
            )
            .await
            {
                Ok(stream) => stream,
                Err(_) => return Ok(()),
            };
            serve_connection(
                stream,
                connection_config.timeout,
                connection_shutdown,
                connection_responses,
                http_version,
                connection_config.http2,
            )
            .await
        });
    }

    let _ = shutdown.send(true);
    if timeout(config.timeout, drain_connections(&mut tasks))
        .await
        .is_err()
    {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        return Err("hyper server shutdown timed out".into());
    }
    Ok(())
}

async fn serve_connection(
    stream: tls::BoxedStream,
    operation_timeout: std::time::Duration,
    shutdown: watch::Receiver<bool>,
    responses: Arc<response::HyperResponses>,
    http_version: HttpVersion,
    http2_config: Option<Http2Config>,
) -> Result<(), String> {
    match http_version {
        HttpVersion::H1 => serve_http1(stream, operation_timeout, shutdown, responses).await,
        HttpVersion::H2 => {
            let limits = http2_config.ok_or("HTTP/2 service has no HTTP/2 limits")?;
            serve_http2(stream, operation_timeout, shutdown, responses, limits).await
        }
    }
}

async fn serve_http1(
    stream: tls::BoxedStream,
    operation_timeout: std::time::Duration,
    mut shutdown: watch::Receiver<bool>,
    responses: Arc<response::HyperResponses>,
) -> Result<(), String> {
    let service = service_fn(move |request| {
        dispatch(
            request,
            operation_timeout,
            responses.clone(),
            HttpVersion::H1,
        )
    });
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(operation_timeout)
        .max_buf_size(HEADER_BYTES);
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);
    let result = tokio::select! {
        result = connection.as_mut() => result,
        _ = shutdown.changed() => {
            connection.as_mut().graceful_shutdown();
            timeout(operation_timeout, connection.as_mut())
                .await
                .map_err(|_| "connection shutdown timed out".to_owned())?
        }
    };
    // Peer disconnects and malformed requests end only their own connection.
    let _ = result;
    Ok(())
}

async fn serve_http2(
    stream: tls::BoxedStream,
    operation_timeout: std::time::Duration,
    mut shutdown: watch::Receiver<bool>,
    responses: Arc<response::HyperResponses>,
    limits: Http2Config,
) -> Result<(), String> {
    let service = service_fn(move |request| {
        dispatch(
            request,
            operation_timeout,
            responses.clone(),
            HttpVersion::H2,
        )
    });
    let mut builder = http2::Builder::new(TokioExecutor::new());
    builder
        .timer(TokioTimer::new())
        .adaptive_window(false)
        .initial_stream_window_size(limits.stream_window_bytes)
        .initial_connection_window_size(limits.connection_window_bytes)
        .max_concurrent_streams(limits.streams as u32)
        .max_header_list_size(HEADER_BYTES as u32)
        .max_frame_size(HTTP2_FRAME_BYTES)
        .max_send_buf_size(HTTP2_FRAME_BYTES as usize);
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);
    let result = tokio::select! {
        result = connection.as_mut() => result,
        _ = shutdown.changed() => {
            connection.as_mut().graceful_shutdown();
            timeout(operation_timeout, connection.as_mut())
                .await
                .map_err(|_| "connection shutdown timed out".to_owned())?
        }
    };
    // Peer disconnects and malformed requests end only their own connection.
    let _ = result;
    Ok(())
}

async fn drain_connections(tasks: &mut JoinSet<Result<(), String>>) -> Result<(), String> {
    while let Some(result) = tasks.join_next().await {
        result.map_err(|error| format!("connection task failed: {error}"))??;
    }
    Ok(())
}

async fn dispatch(
    request: Request<Incoming>,
    operation_timeout: std::time::Duration,
    responses: Arc<response::HyperResponses>,
    http_version: HttpVersion,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let response = timeout(
        operation_timeout,
        read_and_route(request, responses, http_version),
    )
    .await
    .unwrap_or_else(|_| reject(StatusCode::REQUEST_TIMEOUT, http_version));
    Ok(response)
}

async fn read_and_route(
    request: Request<Incoming>,
    responses: Arc<response::HyperResponses>,
    http_version: HttpVersion,
) -> Response<Full<Bytes>> {
    if request.method() == Method::CONNECT || request.headers().contains_key(UPGRADE) {
        return reject(StatusCode::NOT_IMPLEMENTED, http_version);
    }
    if request.version() == Version::HTTP_11 && request.headers().get_all(HOST).iter().count() != 1
    {
        return reject(StatusCode::BAD_REQUEST, http_version);
    }
    if header_size(&request) > HEADER_BYTES {
        return reject(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE, http_version);
    }
    let (head, mut body) = request.into_parts();
    let mut received = BytesMut::new();
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return reject(StatusCode::BAD_REQUEST, http_version);
        };
        if frame.is_trailers() {
            return reject(StatusCode::NOT_IMPLEMENTED, http_version);
        }
        if let Ok(data) = frame.into_data() {
            if received.len().saturating_add(data.len()) > BODY_BYTES {
                return reject(StatusCode::PAYLOAD_TOO_LARGE, http_version);
            }
            received.extend_from_slice(&data);
        }
    }
    responses.response(head.uri.path())
}

fn header_size(request: &Request<Incoming>) -> usize {
    let start = request.method().as_str().len() + request.uri().to_string().len() + 14;
    request
        .headers()
        .iter()
        .fold(start, |total, (name, value)| {
            total
                .saturating_add(name.as_str().len())
                .saturating_add(value.as_bytes().len())
                .saturating_add(4)
        })
}

fn reject(status: StatusCode, http_version: HttpVersion) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    if http_version == HttpVersion::H1 {
        response
            .headers_mut()
            .insert(CONNECTION, hyper::header::HeaderValue::from_static("close"));
    }
    response
}

fn ready(port: u16) -> Result<(), String> {
    let mut output = io::stdout().lock();
    writeln!(output, "READY|{port}").map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())
}
