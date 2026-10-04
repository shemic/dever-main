use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::client::conn::{http1, http2};
use hyper::header::{CONTENT_TYPE, HOST, HeaderValue};
use hyper::{Request, StatusCode, Uri, Version};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::runtime::Builder;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::config::{ConnectionShape, HEADER_BYTES, HTTP2_FRAME_BYTES, Http2Config, HttpVersion};
use crate::metrics::Histogram;
use crate::{response, tls};

const MAX_RATE: u64 = 1_000_000;
const MAX_SECONDS: f64 = 3_600.0;

#[derive(Clone, Debug)]
pub struct Arguments {
    http_version: HttpVersion,
    target: Target,
    path: String,
    expected: response::ExpectedResponse,
    rate: u64,
    seconds: Duration,
    shape: ConnectionShape,
    timeout: Duration,
    reuse: bool,
    ca_path: String,
}

impl Arguments {
    pub fn parse(values: Vec<String>) -> Result<Self, String> {
        let [
            http_version,
            url,
            path,
            rate,
            seconds,
            physical_connections,
            streams_per_connection,
            timeout_ms,
            reuse,
            ca,
        ] = values.as_slice()
        else {
            return Err(crate::usage());
        };
        let http_version = HttpVersion::parse(http_version)?;
        let target = Target::parse(url)?;
        let orm_count = if path == response::ORM_COUNT_PATH {
            let settings = crate::settings::Settings::load()?;
            if settings.value("orm_count").is_none() {
                return Err("benchmark.orm_count is required for ORM count requests".into());
            }
            Some(settings.integer("orm_count", 0, 1, usize::MAX)?)
        } else {
            None
        };
        let expected = response::ExpectedResponse::for_path(path, orm_count)?;
        let rate = number(rate, "RATE", 1, MAX_RATE)?;
        let seconds = decimal_duration(seconds, "SECONDS", MAX_SECONDS)?;
        let physical_connections =
            number(physical_connections, "PHYSICAL_CONNECTIONS", 1, 65_536)? as usize;
        let streams_per_connection =
            number(streams_per_connection, "STREAMS_PER_CONNECTION", 1, 65_536)? as usize;
        let shape =
            ConnectionShape::new(http_version, physical_connections, streams_per_connection)?;
        let timeout_ms = number(timeout_ms, "TIMEOUT_MS", 1, 300_000)?;
        let reuse = match reuse.as_str() {
            "true" => true,
            "false" => false,
            _ => return Err("REUSE must be 'true' or 'false'".into()),
        };
        if http_version == HttpVersion::H2 && !reuse {
            return Err("HTTP/2 requires REUSE to be 'true'".into());
        }
        if target.tls && ca == "-" {
            return Err("CA must name a root PEM file for HTTPS".into());
        }
        Ok(Self {
            http_version,
            target,
            path: path.clone(),
            expected,
            rate,
            seconds,
            shape,
            timeout: Duration::from_millis(timeout_ms),
            reuse,
            ca_path: ca.clone(),
        })
    }
}

#[derive(Clone, Debug)]
struct Target {
    tls: bool,
    port: u16,
    authority: String,
}

impl Target {
    fn parse(value: &str) -> Result<Self, String> {
        let uri = value
            .parse::<Uri>()
            .map_err(|error| format!("URL is invalid: {error}"))?;
        let scheme = uri
            .scheme_str()
            .ok_or("URL must include http:// or https://")?;
        let host = uri.host().ok_or("URL must include a host")?;
        let tls = match scheme {
            "http" => false,
            "https" => true,
            _ => return Err("URL scheme must be http or https".into()),
        };
        if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
            return Err("benchmark URL must use a loopback host".into());
        }
        if tls && host != "localhost" {
            return Err(
                "HTTPS benchmark URL must use localhost for certificate verification".into(),
            );
        }
        if uri.path() != "/" || uri.query().is_some() {
            return Err(
                "URL must contain only the scheme, loopback host and port; pass PATH separately"
                    .into(),
            );
        }
        let port = uri.port_u16().ok_or("URL must include an explicit port")?;
        let authority = uri
            .authority()
            .ok_or("URL must include an authority")?
            .to_string();
        Ok(Self {
            tls,
            port,
            authority,
        })
    }
}

pub fn run(arguments: Arguments) -> Result<(), String> {
    let settings = crate::settings::Settings::load()?;
    let workers = settings.workers()?;
    let runtime = Builder::new_multi_thread()
        .worker_threads(workers)
        .max_blocking_threads(1)
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| format!("cannot start load runtime: {error}"))?;
    runtime.block_on(execute(arguments, &settings))
}

async fn execute(arguments: Arguments, settings: &crate::settings::Settings) -> Result<(), String> {
    let tls = arguments
        .target
        .tls
        .then(|| tls::client(&arguments.ca_path, arguments.http_version))
        .transpose()?;
    let http2 = (arguments.http_version == HttpVersion::H2)
        .then(|| Http2Config::from_settings(settings, arguments.shape.streams_per_connection))
        .transpose()?;
    let client = Arc::new(ClientConfig {
        target: arguments.target.clone(),
        path: arguments.path.clone(),
        expected: arguments.expected,
        timeout: arguments.timeout,
        reuse: arguments.reuse,
        tls,
        http2,
    });
    let mut http2_connections = if arguments.http_version == HttpVersion::H2 {
        Http2Connections::connect(&client, arguments.shape).await?
    } else {
        Http2Connections::default()
    };
    let event_capacity = arguments.shape.request_slots.saturating_mul(2).max(1);
    let (events, mut event_receiver) = mpsc::channel(event_capacity);
    let mut commands = Vec::with_capacity(arguments.shape.request_slots);
    let mut workers = Vec::with_capacity(arguments.shape.request_slots);
    for id in 0..arguments.shape.request_slots {
        let (sender, receiver) = mpsc::channel(1);
        commands.push(sender);
        let connection = match arguments.http_version {
            HttpVersion::H1 => WorkerConnection::Http1(None),
            HttpVersion::H2 => {
                let connection = id / arguments.shape.streams_per_connection;
                WorkerConnection::Http2(http2_connections.sender(connection))
            }
        };
        workers.push(tokio::spawn(worker(
            id,
            receiver,
            events.clone(),
            client.clone(),
            connection,
        )));
    }
    drop(events);

    let mut ready = VecDeque::with_capacity(arguments.shape.request_slots);
    while ready.len() < arguments.shape.request_slots {
        let event = tokio::time::timeout(arguments.timeout, event_receiver.recv())
            .await
            .map_err(|_| "load workers did not become ready before TIMEOUT_MS".to_owned())?
            .ok_or("load workers stopped before becoming ready")?;
        match event {
            Event::Ready(id) => ready.push_back(id),
            Event::Complete(_) => {
                return Err("load worker completed before receiving a request".into());
            }
        }
    }

    let scheduled_float = arguments.rate as f64 * arguments.seconds.as_secs_f64();
    if !scheduled_float.is_finite() || scheduled_float > u64::MAX as f64 {
        return Err("RATE * SECONDS is too large".into());
    }
    let scheduled = scheduled_float.ceil() as u64;
    let start = Instant::now();
    let end = start + arguments.seconds;
    let mut result = Results::new(
        scheduled,
        arguments.http_version,
        arguments.shape,
        http2_connections.successful_handshakes(),
    );

    for sequence in 0..scheduled {
        let planned = start + Duration::from_secs_f64(sequence as f64 / arguments.rate as f64);
        loop {
            tokio::select! {
                event = event_receiver.recv() => {
                    let event = event.ok_or("all load workers stopped unexpectedly")?;
                    result.observe(event, &mut ready);
                }
                _ = tokio::time::sleep_until(planned) => break,
            }
        }
        while let Ok(event) = event_receiver.try_recv() {
            result.observe(event, &mut ready);
        }
        if Instant::now() >= end {
            result.dropped += 1;
            result.dispatch_expired += 1;
            continue;
        }
        let Some(worker) = ready.pop_front() else {
            result.dropped += 1;
            result.dropped_capacity += 1;
            continue;
        };
        let job = Job { planned };
        if commands[worker].try_send(job).is_ok() {
            result.sent += 1;
        } else {
            result.dropped += 1;
            result.dropped_capacity += 1;
        }
    }

    while Instant::now() < end {
        tokio::select! {
            event = event_receiver.recv() => {
                let event = event.ok_or("all load workers stopped unexpectedly")?;
                result.observe(event, &mut ready);
            }
            _ = tokio::time::sleep_until(end) => break,
        }
    }

    let drain_deadline = end + arguments.timeout;
    while result.completed < result.sent {
        match tokio::time::timeout_at(drain_deadline, event_receiver.recv()).await {
            Ok(Some(event)) => result.observe(event, &mut ready),
            _ => break,
        }
    }
    drop(commands);
    let pending = result.sent - result.completed;
    result.completed += pending;
    result.errors += pending;
    for worker in &workers {
        if !worker.is_finished() {
            worker.abort();
        }
    }
    for worker in workers {
        let _ = worker.await;
    }
    http2_connections.close().await;
    result.elapsed = start.elapsed();
    println!("{}", result.json());
    Ok(())
}

struct Job {
    planned: Instant,
}

struct Completion {
    succeeded: bool,
    successful_handshakes: u64,
    latency_us: u64,
    dispatch_lag_us: u64,
    request_latency_us: u64,
}

enum Event {
    Ready(usize),
    Complete(Completion),
}

async fn worker(
    id: usize,
    mut jobs: mpsc::Receiver<Job>,
    events: mpsc::Sender<Event>,
    config: Arc<ClientConfig>,
    mut connection: WorkerConnection,
) {
    if events.send(Event::Ready(id)).await.is_err() {
        return;
    }
    while let Some(job) = jobs.recv().await {
        let dispatched = Instant::now();
        let outcome = request(&config, &mut connection).await;
        let completed = Instant::now();
        let completion = Completion {
            succeeded: outcome.result.is_ok(),
            successful_handshakes: outcome.successful_handshakes,
            latency_us: micros(completed.saturating_duration_since(job.planned)),
            dispatch_lag_us: micros(dispatched.saturating_duration_since(job.planned)),
            request_latency_us: micros(completed.saturating_duration_since(dispatched)),
        };
        if events.send(Event::Complete(completion)).await.is_err()
            || events.send(Event::Ready(id)).await.is_err()
        {
            break;
        }
    }
    if let WorkerConnection::Http1(Some(connection)) = connection {
        connection.close().await;
    }
}

struct RequestOutcome {
    result: Result<(), String>,
    successful_handshakes: u64,
}

async fn request(config: &ClientConfig, connection: &mut WorkerConnection) -> RequestOutcome {
    let mut successful_handshakes = 0;
    let result = match tokio::time::timeout(config.timeout, async {
        match connection {
            WorkerConnection::Http1(connection) => {
                if connection.is_none() {
                    *connection = Some(Http1Connection::connect(config).await?);
                    successful_handshakes = 1;
                }
                connection
                    .as_mut()
                    .expect("connection established")
                    .request(config)
                    .await
            }
            WorkerConnection::Http2(sender) => http2_request(sender, config).await,
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err("request timed out".to_owned()),
    };
    if let WorkerConnection::Http1(connection) = connection
        && (result.is_err() || !config.reuse)
        && let Some(connection) = connection.take()
    {
        connection.close().await;
    }
    RequestOutcome {
        result,
        successful_handshakes,
    }
}

struct ClientConfig {
    target: Target,
    path: String,
    expected: response::ExpectedResponse,
    timeout: Duration,
    reuse: bool,
    tls: Option<Arc<rustls::ClientConfig>>,
    http2: Option<Http2Config>,
}

enum WorkerConnection {
    Http1(Option<Http1Connection>),
    Http2(http2::SendRequest<Empty<Bytes>>),
}

struct Http1Connection {
    sender: http1::SendRequest<Empty<Bytes>>,
    driver: Option<JoinHandle<Result<(), hyper::Error>>>,
}

impl Http1Connection {
    async fn connect(config: &ClientConfig) -> Result<Self, String> {
        let stream = tls::connect(
            config.target.port,
            config.timeout,
            config.tls.as_ref(),
            HttpVersion::H1,
        )
        .await?;
        let (sender, connection) =
            tokio::time::timeout(config.timeout, http1::handshake(TokioIo::new(stream)))
                .await
                .map_err(|_| "HTTP handshake timed out".to_owned())?
                .map_err(|error| error.to_string())?;
        Ok(Self {
            sender,
            driver: Some(tokio::spawn(connection)),
        })
    }

    async fn request(&mut self, config: &ClientConfig) -> Result<(), String> {
        self.sender
            .ready()
            .await
            .map_err(|error| error.to_string())?;
        let request = Request::builder()
            .method("GET")
            .uri(config.path.as_str())
            .header(
                HOST,
                HeaderValue::from_str(&config.target.authority)
                    .map_err(|error| error.to_string())?,
            )
            .body(Empty::new())
            .map_err(|error| error.to_string())?;
        let response = self
            .sender
            .send_request(request)
            .await
            .map_err(|error| error.to_string())?;
        validate_response(&config.expected, response).await
    }

    async fn close(mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
            let _ = driver.await;
        }
    }
}

impl Drop for Http1Connection {
    fn drop(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
        }
    }
}

#[derive(Default)]
struct Http2Connections {
    connections: Vec<Http2Connection>,
}

impl Http2Connections {
    async fn connect(config: &ClientConfig, shape: ConnectionShape) -> Result<Self, String> {
        let mut connections = Vec::with_capacity(shape.physical_connections);
        for _ in 0..shape.physical_connections {
            match Http2Connection::connect(config).await {
                Ok(connection) => connections.push(connection),
                Err(error) => {
                    close_http2_connections(connections).await;
                    return Err(error);
                }
            }
        }
        Ok(Self { connections })
    }

    fn sender(&self, connection: usize) -> http2::SendRequest<Empty<Bytes>> {
        self.connections[connection].sender.clone()
    }

    fn successful_handshakes(&self) -> u64 {
        self.connections.len() as u64
    }

    async fn close(&mut self) {
        close_http2_connections(std::mem::take(&mut self.connections)).await;
    }
}

struct Http2Connection {
    sender: http2::SendRequest<Empty<Bytes>>,
    driver: Option<JoinHandle<Result<(), hyper::Error>>>,
}

impl Http2Connection {
    async fn connect(config: &ClientConfig) -> Result<Self, String> {
        let limits = config.http2.ok_or("HTTP/2 client has no HTTP/2 limits")?;
        let stream = tls::connect(
            config.target.port,
            config.timeout,
            config.tls.as_ref(),
            HttpVersion::H2,
        )
        .await?;
        let mut builder = http2::Builder::new(TokioExecutor::new());
        builder
            .timer(TokioTimer::new())
            .adaptive_window(false)
            .initial_stream_window_size(limits.stream_window_bytes)
            .initial_connection_window_size(limits.connection_window_bytes)
            .initial_max_send_streams(limits.streams)
            .max_concurrent_streams(0)
            .max_header_list_size(HEADER_BYTES as u32)
            .max_frame_size(HTTP2_FRAME_BYTES)
            .max_send_buf_size(HTTP2_FRAME_BYTES as usize);
        let (sender, connection) =
            tokio::time::timeout(config.timeout, builder.handshake(TokioIo::new(stream)))
                .await
                .map_err(|_| "HTTP/2 handshake timed out".to_owned())?
                .map_err(|error| error.to_string())?;
        Ok(Self {
            sender,
            driver: Some(tokio::spawn(connection)),
        })
    }

    async fn close(mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
            let _ = driver.await;
        }
    }
}

impl Drop for Http2Connection {
    fn drop(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
        }
    }
}

async fn close_http2_connections(connections: Vec<Http2Connection>) {
    for connection in connections {
        connection.close().await;
    }
}

async fn http2_request(
    sender: &mut http2::SendRequest<Empty<Bytes>>,
    config: &ClientConfig,
) -> Result<(), String> {
    sender.ready().await.map_err(|error| error.to_string())?;
    let scheme = if config.target.tls { "https" } else { "http" };
    let uri = format!("{scheme}://{}{}", config.target.authority, config.path);
    let request = Request::builder()
        .method("GET")
        .version(Version::HTTP_2)
        .uri(uri)
        .body(Empty::new())
        .map_err(|error| error.to_string())?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| error.to_string())?;
    validate_response(&config.expected, response).await
}

async fn validate_response(
    expected: &response::ExpectedResponse,
    response: hyper::Response<Incoming>,
) -> Result<(), String> {
    if response.status() != StatusCode::OK {
        return Err(format!("unexpected response status {}", response.status()));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .ok_or("response has no Content-Type")?;
    if content_type.as_bytes() != expected.content_type.as_bytes() {
        return Err("response Content-Type does not match the benchmark contract".into());
    }
    let mut body = response.into_body();
    let mut offset: usize = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| error.to_string())?;
        let data = frame
            .into_data()
            .map_err(|_| "response contains unexpected trailers")?;
        let end = offset.saturating_add(data.len());
        if end > expected.body.len() || data.as_ref() != &expected.body[offset..end] {
            return Err("response body does not match the benchmark contract".into());
        }
        offset = end;
    }
    if offset != expected.body.len() {
        return Err("response body is shorter than the benchmark contract".into());
    }
    Ok(())
}

struct Results {
    http_version: HttpVersion,
    shape: ConnectionShape,
    successful_handshakes: u64,
    scheduled: u64,
    sent: u64,
    completed: u64,
    succeeded: u64,
    errors: u64,
    dropped: u64,
    dropped_capacity: u64,
    dispatch_expired: u64,
    elapsed: Duration,
    latency: Histogram,
    dispatch_lag: Histogram,
    request_latency: Histogram,
}

impl Results {
    fn new(
        scheduled: u64,
        http_version: HttpVersion,
        shape: ConnectionShape,
        successful_handshakes: u64,
    ) -> Self {
        Self {
            http_version,
            shape,
            successful_handshakes,
            scheduled,
            sent: 0,
            completed: 0,
            succeeded: 0,
            errors: 0,
            dropped: 0,
            dropped_capacity: 0,
            dispatch_expired: 0,
            elapsed: Duration::ZERO,
            latency: Histogram::new(),
            dispatch_lag: Histogram::new(),
            request_latency: Histogram::new(),
        }
    }

    fn observe(&mut self, event: Event, ready: &mut VecDeque<usize>) {
        match event {
            Event::Ready(id) => ready.push_back(id),
            Event::Complete(completion) => {
                self.completed += 1;
                self.successful_handshakes += completion.successful_handshakes;
                self.dispatch_lag.record(completion.dispatch_lag_us);
                if completion.succeeded {
                    self.succeeded += 1;
                    self.latency.record(completion.latency_us);
                    self.request_latency.record(completion.request_latency_us);
                } else {
                    self.errors += 1;
                }
            }
        }
    }

    fn json(&self) -> String {
        let elapsed = self.elapsed.as_secs_f64();
        let qps = if elapsed == 0.0 {
            0.0
        } else {
            self.succeeded as f64 / elapsed
        };
        let latency = percentiles_json(&self.latency);
        let dispatch_lag = percentiles_json(&self.dispatch_lag);
        let request_latency = percentiles_json(&self.request_latency);
        format!(
            concat!(
                "{{\"http_version\":\"{}\",\"physical_connections\":{},",
                "\"streams_per_connection\":{},\"request_slots\":{},",
                "\"successful_handshakes\":{},",
                "\"scheduled\":{},\"sent\":{},\"completed\":{},",
                "\"succeeded\":{},\"errors\":{},\"dropped\":{},",
                "\"dropped_capacity\":{},\"dispatch_expired\":{},",
                "\"latency_samples\":{},\"dispatch_lag_samples\":{},",
                "\"request_latency_samples\":{},",
                "\"elapsed_seconds\":{:.6},\"actual_success_qps\":{:.3},",
                "\"latency_ms\":{},\"dispatch_lag_ms\":{},",
                "\"request_latency_ms\":{}}}"
            ),
            self.http_version.as_str(),
            self.shape.physical_connections,
            self.shape.streams_per_connection,
            self.shape.request_slots,
            self.successful_handshakes,
            self.scheduled,
            self.sent,
            self.completed,
            self.succeeded,
            self.errors,
            self.dropped,
            self.dropped_capacity,
            self.dispatch_expired,
            self.latency.count(),
            self.dispatch_lag.count(),
            self.request_latency.count(),
            elapsed,
            qps,
            latency,
            dispatch_lag,
            request_latency,
        )
    }
}

fn percentiles_json(histogram: &Histogram) -> String {
    if histogram.count() == 0 {
        return "null".into();
    }
    format!(
        "{{\"sample_count\":{},\"p50\":{:.3},\"p95\":{:.3},\"p99\":{:.3}}}",
        histogram.count(),
        histogram.percentile_ms(50),
        histogram.percentile_ms(95),
        histogram.percentile_ms(99),
    )
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn number(value: &str, name: &str, minimum: u64, maximum: u64) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{name} must be an integer"))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(format!("{name} must be between {minimum} and {maximum}"));
    }
    Ok(parsed)
}

fn decimal_duration(value: &str, name: &str, maximum: f64) -> Result<Duration, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| format!("{name} must be a number"))?;
    if !seconds.is_finite() || seconds <= 0.0 || seconds > maximum {
        return Err(format!(
            "{name} must be greater than 0 and at most {maximum}"
        ));
    }
    Ok(Duration::from_secs_f64(seconds))
}
