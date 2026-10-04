use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{
    SinkExt, StreamExt,
    stream::{self, SplitSink, SplitStream},
};
use tokio::sync::Mutex;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        self,
        protocol::{CloseFrame, Role, WebSocketConfig, frame::coding::CloseCode},
    },
};

use crate::transport::Transport;
use crate::{
    async_stream::AsyncStream, bytes::Bytes, endpoint::Endpoint, http::HttpReply, net,
    resource::Pull,
};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub message_bytes: i64,
    pub idle_ms: i64,
    pub write_ms: i64,
}

impl Limits {
    pub(crate) fn check(self) -> Result<WebSocketConfig, String> {
        let maximum = usize::try_from(self.message_bytes)
            .ok()
            .filter(|value| *value > 0 && value.checked_add(14).is_some())
            .ok_or("WebSocket message_bytes must be positive and leave space for a frame header")?;
        if self.idle_ms <= 0 || self.write_ms <= 0 {
            return Err("WebSocket idle_ms and write_ms must be positive".into());
        }
        Ok(WebSocketConfig::default()
            .read_buffer_size(4096)
            .write_buffer_size(0)
            .max_write_buffer_size(maximum.max(125) + 14)
            .max_message_size(Some(maximum))
            .max_frame_size(Some(maximum.max(125))))
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Text(String),
    Binary(Bytes),
    Ping(Bytes),
    Pong(Bytes),
}

type Wire = WebSocketStream<Transport>;
struct Parts {
    reader: Mutex<SplitStream<Wire>>,
    writer: Mutex<SplitSink<Wire, tungstenite::Message>>,
}

#[derive(Clone)]
pub struct WebSocket {
    endpoint: Arc<Endpoint<Parts>>,
    limits: Limits,
}

impl std::fmt::Debug for WebSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebSocket(<resource>)")
    }
}

impl WebSocket {
    fn new(stream: Wire, limits: Limits) -> Self {
        let (writer, reader) = stream.split();
        Self {
            endpoint: Arc::new(Endpoint::new(Parts {
                reader: Mutex::new(reader),
                writer: Mutex::new(writer),
            })),
            limits,
        }
    }

    pub(crate) fn abort(&self) {
        let _closed = self.endpoint.close();
    }

    async fn operation<T>(
        &self,
        millis: i64,
        operation: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        tokio::time::timeout(
            Duration::from_millis(millis as u64),
            self.endpoint.while_open(operation),
        )
        .await
        .map_err(|_| "WebSocket operation timed out".to_owned())?
    }
}

// 中途取消或失败的帧写入不能重试；关闭整个传输，避免留下半帧。
struct AbortOnDrop<'a>(Option<&'a WebSocket>);
impl Drop for AbortOnDrop<'_> {
    fn drop(&mut self) {
        if let Some(socket) = self.0 {
            socket.abort();
        }
    }
}

pub async fn accept(reply: &HttpReply, limits: Limits) -> Result<WebSocket, String> {
    reply.upgrade(limits).await
}

pub(crate) async fn from_server(
    stream: Transport,
    buffered: Vec<u8>,
    limits: Limits,
) -> Result<WebSocket, String> {
    let stream =
        WebSocketStream::from_partially_read(stream, buffered, Role::Server, Some(limits.check()?))
            .await;
    Ok(WebSocket::new(stream, limits))
}

/// address 是数字 IP；target 是 / 开头的路径，握手、掩码与响应校验交给 tungstenite。
pub async fn connect(
    address: &str,
    port: i64,
    target: &str,
    limits: Limits,
) -> Result<WebSocket, String> {
    let config = limits.check()?;
    if !target.starts_with('/') || target.contains('#') {
        return Err("WebSocket target must be an origin-form path without a fragment".into());
    }
    tokio::time::timeout(Duration::from_millis(limits.write_ms as u64), async {
        let socket = net::connect_stream_timeout(address, port, limits.write_ms).await?;
        let host = if address.contains(':') {
            format!("[{address}]")
        } else {
            address.to_owned()
        };
        let uri = format!("ws://{host}:{port}{target}");
        let (stream, _) =
            tokio_tungstenite::client_async_with_config(uri, Transport::Tcp(socket), Some(config))
                .await
                .map_err(|error| error.to_string())?;
        Ok(WebSocket::new(stream, limits))
    })
    .await
    .map_err(|_| "WebSocket handshake timed out".to_owned())?
}

pub async fn open(
    url: &str,
    tls: crate::tls::ClientTls,
    limits: Limits,
) -> Result<WebSocket, String> {
    let config = limits.check()?;
    let uri: hyper::Uri = url
        .parse()
        .map_err(|error| format!("invalid WebSocket URL: {error}"))?;
    if url.contains('#')
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
    {
        return Err("WebSocket URL cannot contain credentials or a fragment".into());
    }
    let secure = match uri.scheme_str() {
        Some("ws") => false,
        Some("wss") => true,
        _ => return Err("WebSocket URL must use ws or wss".into()),
    };
    let host = uri
        .host()
        .ok_or("WebSocket URL requires a host")?
        .trim_matches(['[', ']']);
    let port = i64::from(uri.port_u16().unwrap_or(if secure { 443 } else { 80 }));
    tokio::time::timeout(crate::task::positive_duration(limits.write_ms)?, async {
        let transport = crate::transport::connect(host, port, secure.then_some(&tls)).await?;
        let (stream, _) = tokio_tungstenite::client_async_with_config(url, transport, Some(config))
            .await
            .map_err(|error| error.to_string())?;
        Ok(WebSocket::new(stream, limits))
    })
    .await
    .map_err(|_| "WebSocket handshake timed out".to_owned())?
}

pub async fn send(socket: &WebSocket, message: Message) -> Result<(), String> {
    let message = match message {
        Message::Text(text) => tungstenite::Message::Text(text.into()),
        Message::Binary(bytes) => tungstenite::Message::Binary(bytes.into_http()),
        Message::Ping(bytes) => tungstenite::Message::Ping(bytes.into_http()),
        Message::Pong(bytes) => tungstenite::Message::Pong(bytes.into_http()),
    };
    let limit = if message.is_ping() || message.is_pong() {
        125
    } else {
        socket.limits.message_bytes as usize
    };
    if message.len() > limit {
        return Err("WebSocket message exceeds its size limit".into());
    }
    let parts = socket.endpoint.get()?;
    write_wire(socket, &parts, Some(message)).await
}

async fn write_wire(
    socket: &WebSocket,
    parts: &Parts,
    message: Option<tungstenite::Message>,
) -> Result<(), String> {
    socket
        .operation(socket.limits.write_ms, async {
            let mut writer = parts.writer.lock().await;
            // 等待方向锁时尚未写入；此时取消不应中断另一个任务正在发送的帧。
            let mut guard = AbortOnDrop(Some(socket));
            match message {
                Some(message) => writer.send(message).await,
                None => writer.flush().await,
            }
            .map_err(|error| error.to_string())?;
            guard.0 = None;
            Ok(())
        })
        .await
}

pub async fn receive(socket: &WebSocket) -> Result<Option<Message>, String> {
    let parts = socket.endpoint.get()?;
    let incoming = socket
        .operation(socket.limits.idle_ms, async {
            parts
                .reader
                .lock()
                .await
                .next()
                .await
                .transpose()
                .map_err(|error| error.to_string())
        })
        .await;
    let incoming = match incoming {
        Ok(incoming) => incoming,
        Err(error) => {
            socket.abort();
            return Err(error);
        }
    };
    use tungstenite::Message as WireMessage;
    let message = match incoming {
        Some(WireMessage::Text(text)) => Message::Text(text.to_string()),
        Some(WireMessage::Binary(bytes)) => Message::Binary(Bytes::from_http(bytes)),
        Some(WireMessage::Pong(bytes)) => Message::Pong(Bytes::from_http(bytes)),
        Some(WireMessage::Ping(bytes)) => {
            write_wire(socket, &parts, None).await?;
            Message::Ping(Bytes::from_http(bytes))
        }
        Some(WireMessage::Close(_)) => {
            let _guard = AbortOnDrop(Some(socket));
            write_wire(socket, &parts, None).await?;
            return Ok(None);
        }
        None => {
            socket.abort();
            return Ok(None);
        }
        Some(WireMessage::Frame(_)) => unreachable!("tungstenite never returns raw frames"),
    };
    Ok(Some(message))
}

pub fn messages(socket: WebSocket) -> AsyncStream<Result<Message, String>> {
    AsyncStream::new(stream::unfold(socket, |socket| async move {
        let value = match receive(&socket).await {
            Ok(Some(message)) => Pull::Item(Ok(message)),
            Ok(None) => return None,
            Err(error) => Pull::Last(Err(error)),
        };
        Some((value, socket))
    }))
}

pub async fn close(socket: &WebSocket, code: i64, reason: &str) -> Result<(), String> {
    let code = u16::try_from(code)
        .map(CloseCode::from)
        .ok()
        .filter(|code| code.is_allowed())
        .ok_or("invalid WebSocket close code")?;
    if reason.len() > 123 {
        return Err("WebSocket close reason exceeds 123 UTF-8 bytes".into());
    }
    // 先唤醒并禁止普通读写，再由此操作独占最后的传输所有权，完成关闭握手。
    let parts = socket.endpoint.take()?;
    tokio::time::timeout(
        Duration::from_millis(socket.limits.write_ms as u64),
        async {
            parts
                .writer
                .lock()
                .await
                .send(tungstenite::Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.to_owned().into(),
                })))
                .await
                .map_err(|error| error.to_string())?;
            while let Some(message) = parts.reader.lock().await.next().await {
                if matches!(
                    message.map_err(|error| error.to_string())?,
                    tungstenite::Message::Close(_)
                ) {
                    return Ok(());
                }
            }
            Ok(())
        },
    )
    .await
    .map_err(|_| "WebSocket close handshake timed out".to_owned())?
}
