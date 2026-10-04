use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, client_async};

use crate::config::{Protocol, Target};

const INTERACTION_INTERVAL: Duration = Duration::from_millis(100);
const MAX_SSE_BUFFER: usize = 8_192;
const SSE_EVENT: &[u8] = b"event: live\nid: 1\ndata: ready\n\n";
const SSE_HEARTBEAT: &[u8] = b":\n\n";

pub struct Session {
    connection: Connection,
    cycle: usize,
    index: usize,
    timeout: Duration,
    messages: u64,
}

enum Connection {
    Tcp(TcpStream),
    WebSocket(Box<WebSocketStream<TcpStream>>),
    Sse(SseConnection),
}

struct SseConnection {
    body: Incoming,
    driver: ConnectionDriver,
    pending: Vec<u8>,
}

struct ConnectionDriver(Option<JoinHandle<Result<(), hyper::Error>>>);

impl ConnectionDriver {
    fn new(driver: JoinHandle<Result<(), hyper::Error>>) -> Self {
        Self(Some(driver))
    }

    async fn close(&mut self) {
        if let Some(driver) = self.0.take() {
            driver.abort();
            let _ = driver.await;
        }
    }
}

impl Drop for ConnectionDriver {
    fn drop(&mut self) {
        if let Some(driver) = &self.0 {
            driver.abort();
        }
    }
}

impl Session {
    pub async fn connect(
        protocol: Protocol,
        target: &Target,
        cycle: usize,
        index: usize,
        operation_timeout: Duration,
    ) -> Result<Self, String> {
        let stream = timeout(
            operation_timeout,
            TcpStream::connect(("127.0.0.1", target.port)),
        )
        .await
        .map_err(|_| "connection timed out".to_owned())?
        .map_err(|error| error.to_string())?;
        stream
            .set_nodelay(true)
            .map_err(|error| error.to_string())?;
        let connection = match protocol {
            Protocol::Tcp => Connection::Tcp(stream),
            Protocol::WebSocket => {
                let (socket, response) =
                    timeout(operation_timeout, client_async(target.url.as_str(), stream))
                        .await
                        .map_err(|_| "WebSocket handshake timed out".to_owned())?
                        .map_err(|error| error.to_string())?;
                if response.status() != StatusCode::SWITCHING_PROTOCOLS {
                    return Err(format!(
                        "WebSocket handshake returned {}",
                        response.status()
                    ));
                }
                Connection::WebSocket(Box::new(socket))
            }
            Protocol::Sse => {
                Connection::Sse(SseConnection::open(stream, target, operation_timeout).await?)
            }
        };
        let mut session = Self {
            connection,
            cycle,
            index,
            timeout: operation_timeout,
            messages: 0,
        };
        session.exchange(0).await?;
        Ok(session)
    }

    pub async fn hold(&mut self, duration: Duration) -> Result<(), String> {
        let deadline = Instant::now() + duration;
        let mut sequence = 1_u64;
        while Instant::now() < deadline {
            if let Connection::Sse(connection) = &mut self.connection {
                let heartbeats = connection.read_heartbeats(deadline).await?;
                if heartbeats == 0 {
                    return Err("SSE hold ended without a heartbeat".into());
                }
                self.messages += heartbeats;
                break;
            }
            self.exchange(sequence).await?;
            sequence += 1;
            let next = (Instant::now() + INTERACTION_INTERVAL).min(deadline);
            tokio::time::sleep_until(next).await;
        }
        Ok(())
    }

    pub async fn close(mut self) -> Result<u64, String> {
        match &mut self.connection {
            Connection::Tcp(_) => {}
            Connection::WebSocket(socket) => timeout(self.timeout, socket.as_mut().close(None))
                .await
                .map_err(|_| "WebSocket close timed out".to_owned())?
                .map_err(|error| error.to_string())?,
            Connection::Sse(connection) => connection.close().await,
        }
        Ok(self.messages)
    }

    async fn exchange(&mut self, sequence: u64) -> Result<(), String> {
        match &mut self.connection {
            Connection::Tcp(stream) => {
                let payload = payload(self.cycle, self.index, sequence);
                timeout(self.timeout, write_all(stream, &payload))
                    .await
                    .map_err(|_| "TCP write timed out".to_owned())??;
                let mut received = vec![0; payload.len()];
                timeout(self.timeout, read_exact(stream, &mut received))
                    .await
                    .map_err(|_| "TCP read timed out".to_owned())??;
                if received != payload {
                    return Err("TCP echo body mismatch".into());
                }
            }
            Connection::WebSocket(socket) => {
                let payload = String::from_utf8(payload(self.cycle, self.index, sequence))
                    .expect("benchmark payload is ASCII");
                timeout(
                    self.timeout,
                    socket.send(Message::Text(payload.clone().into())),
                )
                .await
                .map_err(|_| "WebSocket write timed out".to_owned())?
                .map_err(|error| error.to_string())?;
                let message = timeout(self.timeout, socket.next())
                    .await
                    .map_err(|_| "WebSocket read timed out".to_owned())?
                    .ok_or("WebSocket ended before echo")?
                    .map_err(|error| error.to_string())?;
                if !matches!(message, Message::Text(value) if value.as_str() == payload) {
                    return Err("WebSocket echo body mismatch".into());
                }
            }
            Connection::Sse(connection) => connection.read_event(self.timeout).await?,
        }
        self.messages += 1;
        Ok(())
    }
}

impl SseConnection {
    async fn open(
        stream: TcpStream,
        target: &Target,
        operation_timeout: Duration,
    ) -> Result<Self, String> {
        let (mut sender, connection) =
            timeout(operation_timeout, http1::handshake(TokioIo::new(stream)))
                .await
                .map_err(|_| "SSE HTTP handshake timed out".to_owned())?
                .map_err(|error| error.to_string())?;
        let driver = ConnectionDriver::new(tokio::spawn(connection));
        let request = Request::builder()
            .method("GET")
            .uri(&target.path)
            .header(HOST, format!("127.0.0.1:{}", target.port))
            .body(Empty::<Bytes>::new())
            .map_err(|error| error.to_string())?;
        let response = timeout(operation_timeout, sender.send_request(request))
            .await
            .map_err(|_| "SSE request timed out".to_owned())?
            .map_err(|error| error.to_string())?;
        if response.status() != StatusCode::OK {
            return Err(format!("SSE request returned {}", response.status()));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if content_type.split(';').next().map(str::trim) != Some("text/event-stream") {
            return Err("SSE response has the wrong Content-Type".into());
        }
        Ok(Self {
            body: response.into_body(),
            driver,
            pending: Vec::new(),
        })
    }

    async fn read_event(&mut self, operation_timeout: Duration) -> Result<(), String> {
        while self.pending.len() < SSE_EVENT.len() {
            let frame = timeout(operation_timeout, self.body.frame())
                .await
                .map_err(|_| "SSE event timed out".to_owned())?
                .ok_or("SSE body ended before the initial event")?
                .map_err(|error| error.to_string())?;
            if let Ok(bytes) = frame.into_data() {
                self.extend(bytes.as_ref())?;
            }
            if self.pending.len() < SSE_EVENT.len() && !SSE_EVENT.starts_with(&self.pending) {
                return Err("SSE initial event body mismatch".into());
            }
        }
        if !self.pending.starts_with(SSE_EVENT) {
            return Err("SSE initial event body mismatch".into());
        }
        self.pending.drain(..SSE_EVENT.len());
        Ok(())
    }

    async fn read_heartbeats(&mut self, deadline: Instant) -> Result<u64, String> {
        let mut messages = consume_heartbeats(&mut self.pending)?;
        loop {
            let frame = match timeout_at(deadline, self.body.frame()).await {
                Err(_) => break,
                Ok(None) => return Err("SSE body ended during hold".into()),
                Ok(Some(Err(error))) => return Err(error.to_string()),
                Ok(Some(Ok(frame))) => frame,
            };
            if let Ok(bytes) = frame.into_data() {
                self.extend(bytes.as_ref())?;
                messages += consume_heartbeats(&mut self.pending)?;
            }
        }
        Ok(messages)
    }

    fn extend(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.pending.len().saturating_add(bytes.len()) > MAX_SSE_BUFFER {
            return Err("SSE validation buffer exceeded its limit".into());
        }
        self.pending.extend_from_slice(bytes);
        Ok(())
    }

    async fn close(&mut self) {
        self.driver.close().await;
    }
}

fn consume_heartbeats(buffer: &mut Vec<u8>) -> Result<u64, String> {
    let mut messages = 0;
    while buffer.len() >= SSE_HEARTBEAT.len() {
        if !buffer.starts_with(SSE_HEARTBEAT) {
            return Err("SSE hold data was not a heartbeat".into());
        }
        buffer.drain(..SSE_HEARTBEAT.len());
        messages += 1;
    }
    if !SSE_HEARTBEAT.starts_with(buffer) {
        return Err("SSE heartbeat body mismatch".into());
    }
    Ok(messages)
}

fn payload(cycle: usize, index: usize, sequence: u64) -> Vec<u8> {
    format!("live:{cycle}:{index}:{sequence}\n").into_bytes()
}

async fn write_all(stream: &TcpStream, bytes: &[u8]) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        stream.writable().await.map_err(|error| error.to_string())?;
        match stream.try_write(&bytes[offset..]) {
            Ok(0) => return Err("TCP write returned zero bytes".into()),
            Ok(length) => offset += length,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

async fn read_exact(stream: &TcpStream, bytes: &mut [u8]) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        stream.readable().await.map_err(|error| error.to_string())?;
        match stream.try_read(&mut bytes[offset..]) {
            Ok(0) => return Err("TCP connection ended before the echo completed".into()),
            Ok(length) => offset += length,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}
