mod client;
mod client_body;
mod client_driver;
mod executor;
mod live;
mod message;
mod protocol;
pub(crate) mod reply;
mod server_h2;
mod write_timeout;

pub use protocol::Http2Limits;

pub use client::{
    HttpClient, PoolLimits, StreamRequest, StreamResponse, client, close_client, open, open_stream,
    request,
};
pub use live::{serve_live, serve_live_tls};
pub use reply::{HttpReply, LiveLimits, finish, respond, start, write};

use std::convert::Infallible;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};

use crate::bytes::Bytes;
use crate::{net, task};
use message::{collect_body, decode_headers, encode_request, encode_response, header_size};
use write_timeout::WriteTimeout;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
tokio::task_local! { static CURRENT_REQUEST: RequestContext; }

#[cfg(feature = "wire")]
pub(crate) fn request_id() -> u64 {
    CURRENT_REQUEST
        .try_with(|request| request.id)
        .unwrap_or_else(|_| NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed))
}

#[cfg(feature = "wire")]
pub(crate) fn client_address() -> Option<String> {
    CURRENT_REQUEST
        .try_with(|request| request.client_address.clone())
        .ok()
        .flatten()
}

#[derive(Clone)]
struct RequestContext {
    id: u64,
    protocol: &'static str,
    target: String,
    client_address: Option<String>,
}

impl RequestContext {
    fn new(protocol: &'static str, target: String, client_address: Option<String>) -> Self {
        Self {
            id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
            protocol,
            target,
            client_address,
        }
    }

    fn report(&self, error: impl Into<String>) {
        crate::log::error(
            error,
            vec![
                crate::log::Field {
                    name: "boundary".into(),
                    value: "http".into(),
                },
                crate::log::Field {
                    name: "request_id".into(),
                    value: self.id.to_string(),
                },
                crate::log::Field {
                    name: "protocol".into(),
                    value: self.protocol.into(),
                },
                crate::log::Field {
                    name: "target".into(),
                    value: self.target.clone(),
                },
            ],
        );
    }
}

#[derive(Clone, Debug)]
pub struct Header {
    pub name: String,
    pub value: Bytes,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<Header>,
    pub body: Bytes,
}

/// Server-only body ownership. Source-level HTTP values remain bounded Request values.
pub struct IncomingRequest {
    pub head: Request,
    pub body: Incoming,
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: i64,
    pub headers: Vec<Header>,
    pub body: Bytes,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub header_bytes: i64,
    pub body_bytes: i64,
    pub timeout_ms: i64,
    pub connections: i64,
    pub http2: Option<Http2Limits>,
}

#[derive(Clone, Copy)]
struct Budget {
    headers: usize,
    body: usize,
    timeout: Duration,
    http2: Option<Http2Limits>,
}

impl Limits {
    fn check(self) -> Result<Budget, String> {
        let headers = usize::try_from(self.header_bytes)
            .ok()
            .filter(|value| *value >= 8192)
            .ok_or("HTTP header_bytes must be at least 8192")?;
        let body =
            usize::try_from(self.body_bytes).map_err(|_| "HTTP body_bytes must be nonnegative")?;
        let millis = u64::try_from(self.timeout_ms)
            .ok()
            .filter(|value| *value > 0)
            .ok_or("HTTP timeout_ms must be positive")?;
        if !(1..=65536).contains(&self.connections) {
            return Err("HTTP connections must be between 1 and 65536".into());
        }
        if let Some(http2) = self.http2 {
            http2.check(self.connections, headers)?;
        }
        Ok(Budget {
            headers,
            body,
            timeout: Duration::from_millis(millis),
            http2: self.http2,
        })
    }
}

/// 连接由现有任务组持有，取消 serve 会沿 Scope 排空连接和 handler 子任务。
pub async fn serve<F, Fut>(route: F, listener: net::Listener, limits: Limits) -> Result<(), String>
where
    F: Fn(Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response, String>> + Send + 'static,
{
    serve_buffered(route, listener, limits, None).await
}

pub async fn serve_tls<F, Fut>(
    route: F,
    listener: net::Listener,
    limits: Limits,
    tls: crate::tls::ServerTls,
) -> Result<(), String>
where
    F: Fn(Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response, String>> + Send + 'static,
{
    serve_buffered(route, listener, limits, Some(tls)).await
}

async fn serve_buffered<F, Fut>(
    route: F,
    listener: net::Listener,
    limits: Limits,
    tls: Option<crate::tls::ServerTls>,
) -> Result<(), String>
where
    F: Fn(Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response, String>> + Send + 'static,
{
    let budget = limits.check()?;
    serve_transport(
        move |incoming: IncomingRequest| {
            let route = route.clone();
            async move {
                let mut request = incoming.head;
                request.body = match tokio::time::timeout(
                    budget.timeout,
                    collect_body(incoming.body, budget.body),
                )
                .await
                {
                    Ok(Ok(body)) => body,
                    Ok(Err(status)) => {
                        return Ok(Response {
                            status: status.as_u16().into(),
                            headers: Vec::new(),
                            body: Bytes::new(Vec::new()),
                        });
                    }
                    Err(_) => {
                        return Ok(Response {
                            status: 408,
                            headers: Vec::new(),
                            body: Bytes::new(Vec::new()),
                        });
                    }
                };
                route(request).await
            }
        },
        listener,
        limits,
        tls,
    )
    .await
}

pub async fn serve_incoming<F, Fut>(
    route: F,
    listener: net::Listener,
    limits: Limits,
) -> Result<(), String>
where
    F: Fn(IncomingRequest) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response, String>> + Send + 'static,
{
    serve_transport(route, listener, limits, None).await
}

async fn serve_transport<F, Fut>(
    route: F,
    listener: net::Listener,
    limits: Limits,
    tls: Option<crate::tls::ServerTls>,
) -> Result<(), String>
where
    F: Fn(IncomingRequest) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response, String>> + Send + 'static,
{
    let budget = limits.check()?;
    let tls = tls.map(|tls| protocol::server_tls(tls, budget.http2));
    task::parallel_each_stream(
        net::incoming(listener.clone(), true),
        limits.connections,
        move |connection| {
            let route = route.clone();
            let tls = tls.clone();
            let listener = listener.clone();
            async move {
                let stream = connection?;
                let client_address = stream
                    .peer_addr()
                    .ok()
                    .map(|address| address.ip().to_string());
                let stream = match crate::tls::accept(stream, tls.as_ref(), limits.timeout_ms).await
                {
                    Ok(stream) => stream,
                    Err(_) => return Ok(()), // A peer's TLS failure does not terminate the listener.
                };
                if protocol::check_alpn(&stream, budget.http2).is_err() {
                    return Ok(());
                }
                if let Some(http2) = budget.http2 {
                    return serve_http2(route, listener, stream, budget, http2, client_address)
                        .await;
                }
                let service = service_fn(move |request| {
                    dispatch(request, route.clone(), budget, client_address.clone())
                });
                let connection = server_builder(budget).serve_connection(
                    TokioIo::new(WriteTimeout::new(stream, budget.timeout)),
                    service,
                );
                tokio::pin!(connection);
                let result = tokio::select! {
                    result = &mut connection => result,
                    _ = listener.closed() => {
                        connection.as_mut().graceful_shutdown();
                        connection.await
                    }
                };
                connection_result(result)
            }
        },
    )
    .await
}

async fn serve_http2<F, Fut>(
    route: F,
    listener: net::Listener,
    stream: crate::transport::Transport,
    budget: Budget,
    http2: Http2Limits,
    client_address: Option<String>,
) -> Result<(), String>
where
    F: Fn(IncomingRequest) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response, String>> + Send + 'static,
{
    let (mut owner, spawner, fault) = task::scoped_owner(())?;
    let first_request = server_h2::FirstRequest::new();
    let service = {
        let first_request = first_request.clone();
        service_fn(move |request| {
            first_request.mark();
            let route = route.clone();
            dispatch(request, route, budget, client_address.clone())
        })
    };
    let mut builder = http2.server_builder(executor::Http2Executor(spawner), budget.headers);
    server_h2::configure_liveness(&mut builder, budget.timeout);
    let connection = builder.serve_connection(
        TokioIo::new(WriteTimeout::new(stream, budget.timeout)),
        service,
    );
    let result = owner
        .enter(server_h2::drive(
            connection,
            &listener,
            &first_request,
            budget.timeout,
            &fault,
            |connection| connection.graceful_shutdown(),
        ))
        .await;
    server_h2::complete(owner, result).await
}

fn server_builder(budget: Budget) -> hyper::server::conn::http1::Builder {
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(budget.timeout)
        .max_buf_size(budget.headers);
    builder
}

fn connection_result(result: Result<(), hyper::Error>) -> Result<(), String> {
    // 对端断开、畸形报文和 I/O 超时只结束该连接；请求故障已在 dispatch 边界处理。
    let _ = result;
    Ok(())
}

async fn dispatch<F, Fut>(
    request: hyper::Request<Incoming>,
    route: F,
    budget: Budget,
    client_address: Option<String>,
) -> Result<hyper::Response<Full<bytes::Bytes>>, Infallible>
where
    F: Fn(IncomingRequest) -> Fut,
    Fut: Future<Output = Result<Response, String>>,
{
    if request.method() == Method::CONNECT || request.headers().contains_key(hyper::header::UPGRADE)
    {
        return Ok(reject(StatusCode::NOT_IMPLEMENTED, budget));
    }
    let request = match read_incoming(request, budget) {
        Ok(request) => request,
        Err(status) => return Ok(reject(status, budget)),
    };
    let context = RequestContext::new(
        if budget.http2.is_some() {
            "http/2"
        } else {
            "http/1.1"
        },
        request.head.target.clone(),
        client_address,
    );
    let response = match tokio::time::timeout(
        budget.timeout,
        CURRENT_REQUEST.scope(context.clone(), route(request)),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            context.report(error);
            return Ok(reject(StatusCode::INTERNAL_SERVER_ERROR, budget));
        }
        Err(_) => return Ok(reject(StatusCode::GATEWAY_TIMEOUT, budget)),
    };
    match encode_response(response, budget) {
        Ok(response) => Ok(response),
        Err(error) => {
            context.report(error);
            Ok(reject(StatusCode::INTERNAL_SERVER_ERROR, budget))
        }
    }
}

async fn read_request(
    request: hyper::Request<Incoming>,
    budget: Budget,
) -> Result<Request, StatusCode> {
    let incoming = read_incoming(request, budget)?;
    let mut request = incoming.head;
    request.body = tokio::time::timeout(budget.timeout, collect_body(incoming.body, budget.body))
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)??;
    Ok(request)
}

fn read_incoming(
    mut request: hyper::Request<Incoming>,
    budget: Budget,
) -> Result<IncomingRequest, StatusCode> {
    if request.method() == Method::CONNECT {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let host_count = request
        .headers()
        .get_all(hyper::header::HOST)
        .iter()
        .count();
    let target = if request.version() == hyper::Version::HTTP_2 {
        let authority = request
            .uri()
            .authority()
            .ok_or(StatusCode::BAD_REQUEST)?
            .as_str()
            .to_owned();
        if host_count > 1 {
            return Err(StatusCode::BAD_REQUEST);
        }
        if let Some(host) = request.headers().get(hyper::header::HOST) {
            if !host.as_bytes().eq_ignore_ascii_case(authority.as_bytes()) {
                return Err(StatusCode::BAD_REQUEST);
            }
        } else {
            let host = hyper::header::HeaderValue::from_str(&authority)
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            request.headers_mut().insert(hyper::header::HOST, host);
        }
        request
            .uri()
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/")
            .to_owned()
    } else {
        if request.version() == hyper::Version::HTTP_11 && host_count != 1 {
            return Err(StatusCode::BAD_REQUEST);
        }
        request.uri().to_string()
    };
    let start_bytes = request.method().as_str().len() + target.len() + 14;
    if header_size(request.headers()) + start_bytes > budget.headers {
        return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
    }
    let (head, body) = request.into_parts();
    Ok(IncomingRequest {
        head: Request {
            method: head.method.to_string(),
            target,
            headers: decode_headers(head.headers),
            body: Bytes::new(Vec::new()),
        },
        body,
    })
}

fn reject(status: StatusCode, budget: Budget) -> hyper::Response<Full<bytes::Bytes>> {
    let mut response = hyper::Response::new(Full::new(bytes::Bytes::new()));
    *response.status_mut() = status;
    if budget.http2.is_none() {
        response.headers_mut().insert(
            hyper::header::CONNECTION,
            hyper::header::HeaderValue::from_static("close"),
        );
    }
    response
}

/// 单次请求独占驱动 future；完成、错误和取消都直接释放连接，不产生后台任务。
pub async fn send(
    address: &str,
    port: i64,
    request: Request,
    limits: Limits,
) -> Result<Response, String> {
    let budget = limits.check()?;
    let request = encode_request(request, budget)?;
    if budget.http2.is_some() {
        return client::send_http2(address, port, request, budget).await;
    }
    tokio::time::timeout(budget.timeout, async {
        let stream = net::connect_stream_timeout(address, port, limits.timeout_ms).await?;
        let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
            .max_buf_size(budget.headers)
            .handshake(TokioIo::new(stream))
            .await
            .map_err(|error| error.to_string())?;
        let exchange = async {
            let response = sender
                .send_request(request)
                .await
                .map_err(|error| error.to_string())?;
            if response.status() == StatusCode::SWITCHING_PROTOCOLS {
                return Err("HTTP upgrades are not supported by send".into());
            }
            if header_size(response.headers()) + 32 > budget.headers {
                return Err("HTTP response headers exceed header_bytes".into());
            }
            let (head, body) = response.into_parts();
            let body = collect_body(body, budget.body)
                .await
                .map_err(|status| format!("HTTP response body rejected: {status}"))?;
            Ok(Response {
                status: i64::from(head.status.as_u16()),
                headers: decode_headers(head.headers),
                body,
            })
        };
        tokio::pin!(exchange, connection);
        tokio::select! {
            result = &mut exchange => result,
            result = &mut connection => {
                result.map_err(|error| error.to_string())?;
                // Connection: close 的最后一帧可能已入队，继续取完响应。
                exchange.await
            }
        }
    })
    .await
    .map_err(|_| "HTTP request timed out".to_owned())?
}
