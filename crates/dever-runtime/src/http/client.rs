use std::sync::Arc;
use std::time::Duration;

use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::sync::{Mutex, OwnedSemaphorePermit, mpsc};
use tokio::time::Instant;

use super::client_body::{RequestBody, download, hold_upload, upload};
use super::client_driver::{self, Control, Driver, Entry, Http2Entry, Http2Lease, Http2Pool, Idle};
use super::{Budget, Header, Limits, Request, Response, message};
use crate::{async_stream::AsyncStream, bytes::Bytes, task, tls::ClientTls};

#[derive(Clone, Copy, Debug)]
pub struct PoolLimits {
    pub idle_ms: i64,
    pub chunk_bytes: i64,
    pub read_ms: i64,
}

#[derive(Clone, Debug)]
pub struct StreamRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<Header>,
    pub body: AsyncStream<Result<Bytes, String>>,
}

#[derive(Clone, Debug)]
pub struct StreamResponse {
    pub status: i64,
    pub headers: Vec<Header>,
    pub body: AsyncStream<Result<Bytes, String>>,
}

struct State {
    authority: String,
    scheme: &'static str,
    host: String,
    port: i64,
    tls: Option<ClientTls>,
    budget: Budget,
    chunks: usize,
    read_timeout: Duration,
    control: Arc<Control>,
    idle: Arc<Idle>,
    http2: Option<Arc<Http2Pool>>,
    queue: mpsc::Sender<Driver>,
    driver: Mutex<Option<task::Task<()>>>,
}

impl Drop for State {
    fn drop(&mut self) {
        self.control.close();
    }
}

#[derive(Clone)]
pub struct HttpClient(Arc<State>);

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpClient(<resource>)")
    }
}

pub async fn client(
    origin: &str,
    tls: ClientTls,
    limits: Limits,
    pool: PoolLimits,
) -> Result<HttpClient, String> {
    let budget = limits.check()?;
    let idle_timeout = task::positive_duration(pool.idle_ms)?;
    let read_timeout = task::positive_duration(pool.read_ms)?;
    let chunks = usize::try_from(pool.chunk_bytes)
        .ok()
        .filter(|size| *size > 0)
        .ok_or("HTTP chunk_bytes must be positive")?;
    let uri: hyper::Uri = origin
        .parse()
        .map_err(|error| format!("invalid HTTP origin: {error}"))?;
    let scheme = match uri.scheme_str() {
        Some("http") => "http",
        Some("https") => "https",
        _ => return Err("HTTP origin must use http or https".into()),
    };
    let secure = scheme == "https";
    let authority = uri
        .authority()
        .ok_or("HTTP origin requires a host")?
        .as_str()
        .to_owned();
    if authority.contains('@')
        || origin.contains('#')
        || uri.query().is_some()
        || !matches!(uri.path(), "" | "/")
    {
        return Err(
            "HTTP client origin cannot contain credentials, path, query or fragment".into(),
        );
    }
    let host = uri
        .host()
        .ok_or("HTTP origin requires a host")?
        .trim_matches(['[', ']'])
        .to_owned();
    let port = i64::from(uri.port_u16().unwrap_or(if secure { 443 } else { 80 }));
    let connections = limits.connections as usize;
    let request_capacity = budget
        .http2
        .map(|http2| connections * http2.streams as usize)
        .unwrap_or(connections);
    let control = Control::new(request_capacity, connections);
    let idle = Idle::new(idle_timeout);
    let http2 = budget
        .http2
        .map(|limits| Http2Pool::new(limits.streams as usize, idle_timeout));
    let (queue, driver) =
        client_driver::start(control.clone(), idle.clone(), http2.clone(), connections).await?;
    Ok(HttpClient(Arc::new(State {
        authority,
        scheme,
        host,
        port,
        tls: secure.then(|| super::protocol::client_tls(tls, budget.http2)),
        budget,
        chunks,
        read_timeout,
        control,
        idle,
        http2,
        queue,
        driver: Mutex::new(Some(driver)),
    })))
}

impl HttpClient {
    pub(super) fn control(&self) -> &Arc<Control> {
        &self.0.control
    }

    pub(super) fn chunk_limit(&self) -> usize {
        self.0.chunks
    }

    pub(super) fn read_timeout(&self) -> Duration {
        self.0.read_timeout
    }

    fn encode(
        &self,
        mut request: Request,
    ) -> Result<hyper::Request<http_body_util::Full<bytes::Bytes>>, String> {
        if !request
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("host"))
        {
            request.headers.push(Header {
                name: "host".into(),
                value: Bytes::from_string(self.0.authority.clone()),
            });
        }
        let mut request = message::encode_request(request, self.0.budget)?;
        if self.0.http2.is_some() {
            message::http2_request(&mut request, self.0.scheme)?;
        }
        Ok(request)
    }

    async fn http1_connection(&self) -> Result<Entry, String> {
        while let Some(mut entry) = self.0.idle.take() {
            if entry.sender.ready().await.is_ok() {
                return Ok(entry);
            }
            // Discard a closed idle transport before sending any request bytes.
        }
        let permit = self
            .0
            .control
            .connections
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "HTTP client is closed".to_owned())?;
        let transport =
            crate::transport::connect(&self.0.host, self.0.port, self.0.tls.as_ref()).await?;
        super::protocol::check_alpn(&transport, self.0.budget.http2)?;
        let (sender, connection) = hyper::client::conn::http1::Builder::new()
            .max_buf_size(self.0.budget.headers)
            .handshake(TokioIo::new(transport))
            .await
            .map_err(|error| error.to_string())?;
        let (abort, driver) = client_driver::driver(connection, permit);
        // The connection permit bounds queued plus running drivers together.
        self.0
            .queue
            .try_send(driver)
            .map_err(|_| "HTTP client driver is closed".to_owned())?;
        Ok(Entry { sender, abort })
    }

    async fn new_http2_connection(
        &self,
        permit: OwnedSemaphorePermit,
    ) -> Result<Arc<Http2Entry>, String> {
        let limits = self.0.budget.http2.expect("HTTP/2 client limits");
        let pool = self.0.http2.as_ref().expect("HTTP/2 client pool");
        let transport =
            crate::transport::connect(&self.0.host, self.0.port, self.0.tls.as_ref()).await?;
        super::protocol::check_alpn(&transport, self.0.budget.http2)?;
        let (driver, handshake) = client_driver::http2_handshake(
            transport,
            permit,
            limits,
            self.0.budget.headers,
            self.0.budget.timeout,
            Arc::clone(pool),
        );
        self.0
            .queue
            .try_send(driver)
            .map_err(|_| "HTTP client driver is closed".to_owned())?;
        handshake
            .await
            .map_err(|_| "HTTP client driver closed during HTTP/2 handshake".to_owned())?
    }

    async fn http2_connection(&self) -> Result<(Arc<Http2Entry>, OwnedSemaphorePermit), String> {
        let pool = self.0.http2.as_ref().expect("HTTP/2 client pool");
        loop {
            let changed = pool.changed();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(connection) = pool.take() {
                return Ok(connection);
            }
            let connection = self.0.control.connections.clone().acquire_owned();
            tokio::pin!(connection);
            tokio::select! {
                biased;
                _ = changed => continue,
                permit = &mut connection => {
                    let permit = permit.map_err(|_| "HTTP client is closed".to_owned())?;
                    let entry = self.new_http2_connection(permit).await?;
                    if let Some(stream) = entry.acquire() {
                        return Ok((entry, stream));
                    }
                }
            }
        }
    }

    async fn exchange(
        &self,
        request: hyper::Request<RequestBody>,
    ) -> Result<StreamResponse, String> {
        if self.0.http2.is_some() {
            self.exchange_http2(request).await
        } else {
            self.exchange_http1(request).await
        }
    }

    async fn exchange_http1(
        &self,
        request: hyper::Request<RequestBody>,
    ) -> Result<StreamResponse, String> {
        self.0
            .control
            .while_open(async {
                tokio::time::timeout(self.0.budget.timeout, async {
                    let permit = self
                        .0
                        .control
                        .requests
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| "HTTP client is closed".to_owned())?;
                    let mut entry = self.http1_connection().await?;
                    let response = entry
                        .sender
                        .send_request(request)
                        .await
                        .map_err(|error| error.to_string())?;
                    let lease = Lease::Http1 {
                        entry: Some(entry),
                        client: self.clone(),
                        _permit: permit,
                    };
                    self.response(response, lease)
                })
                .await
                .map_err(|_| "HTTP request timed out".to_owned())?
            })
            .await
    }

    async fn exchange_http2(
        &self,
        request: hyper::Request<RequestBody>,
    ) -> Result<StreamResponse, String> {
        self.0
            .control
            .while_open(async {
                tokio::time::timeout(self.0.budget.timeout, async {
                    let (head, body) = request.into_parts();
                    let (body, upload_lease) = hold_upload(body);
                    let mut pending = Some(hyper::Request::from_parts(head, body));
                    loop {
                        let request_permit = self
                            .0
                            .control
                            .requests
                            .clone()
                            .acquire_owned()
                            .await
                            .map_err(|_| "HTTP client is closed".to_owned())?;
                        let (entry, stream_permit) = self.http2_connection().await?;
                        let stream = Http2Lease::new(entry.clone(), stream_permit, request_permit);
                        let mut sender = entry.sender.clone();
                        if sender.ready().await.is_err() {
                            entry.drain();
                            continue;
                        }
                        upload_lease.replace(stream.clone());
                        let request = pending.take().expect("pending HTTP/2 request");
                        let response = match sender.try_send_request(request).await {
                            Ok(response) => response,
                            Err(mut error) => {
                                if sender.is_closed() {
                                    entry.drain();
                                }
                                if let Some(request) = error.take_message() {
                                    upload_lease.clear();
                                    drop(stream);
                                    pending = Some(request);
                                    continue;
                                }
                                return Err(error.into_error().to_string());
                            }
                        };
                        let lease = Lease::Http2 {
                            client: self.clone(),
                            _stream: stream,
                        };
                        return self.response(response, lease);
                    }
                })
                .await
                .map_err(|_| "HTTP request timed out".to_owned())?
            })
            .await
    }

    fn response(
        &self,
        response: hyper::Response<Incoming>,
        lease: Lease,
    ) -> Result<StreamResponse, String> {
        if response.status() == hyper::StatusCode::SWITCHING_PROTOCOLS {
            return Err("HTTP client upgrades require the WebSocket API".into());
        }
        if message::header_size(response.headers()) + 32 > self.0.budget.headers {
            return Err("HTTP response headers exceed header_bytes".into());
        }
        let (head, body) = response.into_parts();
        Ok(StreamResponse {
            status: i64::from(head.status.as_u16()),
            headers: message::decode_headers(head.headers),
            body: download(body, lease),
        })
    }
}

pub(super) enum Lease {
    Http1 {
        entry: Option<Entry>,
        client: HttpClient,
        _permit: OwnedSemaphorePermit,
    },
    Http2 {
        client: HttpClient,
        _stream: Http2Lease,
    },
}

impl Lease {
    pub fn client(&self) -> &HttpClient {
        match self {
            Self::Http1 { client, .. } | Self::Http2 { client, .. } => client,
        }
    }

    pub async fn complete(&mut self) {
        let Self::Http1 { entry, client, .. } = self else {
            return;
        };
        let mut entry = entry.take().expect("active HTTP lease");
        if !client.control().closed.is_closed() && entry.sender.ready().await.is_ok() {
            client.0.idle.put(entry);
        }
    }
}

pub(super) async fn send_http2(
    address: &str,
    port: i64,
    mut request: hyper::Request<http_body_util::Full<bytes::Bytes>>,
    budget: Budget,
) -> Result<Response, String> {
    use http_body_util::BodyExt;

    message::http2_request(&mut request, "http")?;
    let deadline = Instant::now() + budget.timeout;
    let timeout_ms = i64::try_from(budget.timeout.as_millis())
        .map_err(|_| "HTTP timeout is too large".to_owned())?;
    let transport = tokio::time::timeout_at(
        deadline,
        crate::net::connect_stream_timeout(address, port, timeout_ms),
    )
    .await
    .map_err(|_| "HTTP request timed out".to_owned())??;
    let (mut owner, spawner, fault) = task::scoped_owner(())?;
    let limits = budget.http2.expect("HTTP/2 send limits");
    let builder = limits.client_builder(super::executor::Http2Executor(spawner), budget.headers);
    let handshake = {
        let handshake = tokio::time::timeout_at(
            deadline,
            owner.enter(builder.handshake(TokioIo::new(transport))),
        );
        tokio::pin!(handshake);
        tokio::select! {
            result = &mut handshake => match result {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(_) => Err("HTTP request timed out".to_owned()),
            },
            error = fault.wait() => Err(error),
        }
    };
    let (mut sender, connection) = match handshake {
        Ok(connection) => connection,
        Err(error) => {
            let _ = owner.stop().await;
            return Err(error);
        }
    };
    let request = request.map(|body| body.map_err(|never| match never {}).boxed_unsync());
    let exchange = async {
        let response = sender
            .send_request(request)
            .await
            .map_err(|error| error.to_string())?;
        if response.status() == hyper::StatusCode::SWITCHING_PROTOCOLS {
            return Err("HTTP upgrades are not supported by send".into());
        }
        if message::header_size(response.headers()) + 32 > budget.headers {
            return Err("HTTP response headers exceed header_bytes".into());
        }
        let (head, body) = response.into_parts();
        let body = message::collect_body(body, budget.body)
            .await
            .map_err(|status| format!("HTTP response body rejected: {status}"))?;
        Ok(Response {
            status: i64::from(head.status.as_u16()),
            headers: message::decode_headers(head.headers),
            body,
        })
    };
    let result = {
        let connection = owner.enter(connection);
        tokio::pin!(connection, exchange);
        match tokio::time::timeout_at(deadline, async {
            tokio::select! {
                result = &mut exchange => result,
                result = &mut connection => {
                    result.map_err(|error| error.to_string())?;
                    exchange.await
                }
                error = fault.wait() => Err(error),
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err("HTTP request timed out".to_owned()),
        }
    };
    let cleanup = owner.stop().await;
    match result {
        Ok(response) => {
            cleanup?;
            Ok(response)
        }
        Err(error) => Err(error),
    }
}

pub async fn open(client: &HttpClient, request: Request) -> Result<StreamResponse, String> {
    use http_body_util::BodyExt;
    let request = client
        .encode(request)?
        .map(|body| body.map_err(|never| match never {}).boxed_unsync());
    client.exchange(request).await
}

pub async fn open_stream(
    client: &HttpClient,
    request: StreamRequest,
) -> Result<StreamResponse, String> {
    let head = client.encode(Request {
        method: request.method,
        target: request.target,
        headers: request.headers,
        body: Bytes::new(Vec::new()),
    })?;
    let request = head.map(|_| upload(request.body, client.chunk_limit(), client.read_timeout()));
    client.exchange(request).await
}

pub async fn request(client: &HttpClient, request: Request) -> Result<Response, String> {
    tokio::time::timeout(client.0.budget.timeout, async {
        let response = open(client, request).await?;
        let _owner = response.body.close_on_drop();
        let mut body = Bytes::new(Vec::new());
        while let Some(chunk) = response.body.pull().await {
            let chunk = chunk?;
            if body.values().len() + chunk.values().len() > client.0.budget.body {
                return Err("HTTP response body exceeds body_bytes".into());
            }
            body = body.concat(&chunk);
        }
        Ok(Response {
            status: response.status,
            headers: response.headers,
            body,
        })
    })
    .await
    .map_err(|_| "HTTP request timed out".to_owned())?
}

pub async fn close_client(client: &HttpClient) -> Result<(), String> {
    client.0.control.close();
    task::stop_resource(&mut *client.0.driver.lock().await).await
}
