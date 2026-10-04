use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll};
use std::time::Duration;

use http_body_util::Full;
use hyper::body::{Body, Frame, SizeHint};
use tokio::sync::{Mutex as AsyncMutex, Semaphore, mpsc, oneshot};
use tokio::time::Sleep;

use super::{Budget, Header, Response, message::encode_response};
use crate::{bytes::Bytes, task, websocket};

#[derive(Clone, Copy, Debug)]
pub struct LiveLimits {
    pub chunk_bytes: i64,
    pub idle_ms: i64,
    pub heartbeat_ms: i64,
}

impl LiveLimits {
    pub(super) fn check(self) -> Result<(), String> {
        if self.chunk_bytes <= 0 || usize::try_from(self.chunk_bytes).is_err() {
            return Err("HTTP chunk_bytes must be positive".into());
        }
        if self.idle_ms <= 0 || self.heartbeat_ms <= 0 || self.heartbeat_ms >= self.idle_ms {
            return Err("HTTP live limits require 0 < heartbeat_ms < idle_ms".into());
        }
        Ok(())
    }
}

type Head = hyper::Response<Outgoing>;
enum Mode {
    Pending,
    Streaming {
        sender: mpsc::Sender<bytes::Bytes>,
        sse: bool,
    },
    Finished,
    Upgraded(Option<websocket::WebSocket>),
}

struct State {
    mode: Mode,
    head: Option<oneshot::Sender<Head>>,
    upgrade: Option<(hyper::Request<()>, hyper::upgrade::OnUpgrade)>,
    fault: Option<String>,
    body_cancelled: bool,
}

struct Shared {
    state: Mutex<State>,
    writer: AsyncMutex<()>,
    closed: Semaphore,
    budget: Budget,
    live: LiveLimits,
    upgraded: Arc<AtomicBool>,
}

impl Shared {
    fn close_body(&self, cancelled: bool) {
        if cancelled {
            self.state
                .lock()
                .expect("reply lock poisoned")
                .body_cancelled = true;
        }
        self.closed.close();
    }
}

#[derive(Clone)]
pub struct HttpReply(Arc<Shared>);

impl std::fmt::Debug for HttpReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpReply(<resource>)")
    }
}

impl HttpReply {
    pub(crate) fn chunk_limit(&self) -> usize {
        self.0.live.chunk_bytes as usize
    }

    pub(crate) fn fault(&self) -> Option<String> {
        self.0
            .state
            .lock()
            .expect("reply lock poisoned")
            .fault
            .clone()
    }

    fn publish(&self, prepare: impl FnOnce() -> (Head, Mode)) -> Result<(), String> {
        let mut state = self.0.state.lock().expect("reply lock poisoned");
        if !matches!(state.mode, Mode::Pending) {
            return Err("HTTP response has already started".into());
        }
        let head = state.head.take().ok_or("HTTP response is closed")?;
        // 只有真正提交的正文才能拥有取消信号；拒绝重复提交时不能关闭已有响应。
        let (response, mode) = prepare();
        state.upgrade.take();
        state.mode = mode;
        // SendError 会析构响应正文；先释放状态锁，正文 Drop 才能登记传输取消。
        drop(state);
        if head.send(response).is_err() {
            // 空正文从创建起就是 end-stream，不能只依赖 BufferedBody::drop 判断取消。
            self.0.close_body(true);
            return Err("HTTP peer is closed".into());
        }
        Ok(())
    }

    pub(crate) async fn upgrade(
        &self,
        limits: websocket::Limits,
    ) -> Result<websocket::WebSocket, String> {
        limits.check()?;
        let (head, response, upgrade) = {
            let mut state = self.0.state.lock().expect("reply lock poisoned");
            if !matches!(state.mode, Mode::Pending) {
                return Err("HTTP response has already started".into());
            }
            let (request, upgrade) = state.upgrade.take().ok_or("HTTP upgrade is unavailable")?;
            let response =
                tokio_tungstenite::tungstenite::handshake::server::create_response(&request)
                    .map_err(|error| error.to_string())?;
            let response = response.map(|()| Outgoing::Full(Full::new(bytes::Bytes::new())));
            state.mode = Mode::Upgraded(None);
            self.0.upgraded.store(true, Ordering::Release);
            let head = state.head.take().ok_or("HTTP response is closed")?;
            (head, response, upgrade)
        };
        if head.send(response).is_err() {
            self.0.close_body(true);
            return Err("HTTP peer is closed".into());
        }
        let stream = tokio::time::timeout(self.0.budget.timeout, upgrade)
            .await
            .map_err(|_| "WebSocket upgrade timed out")?
            .map_err(|error| error.to_string())?;
        // 取回已知的底层 I/O 和预读帧；升级后不再继承 HTTP 写入期限。
        let parts = stream
            .downcast::<hyper_util::rt::TokioIo<super::write_timeout::WriteTimeout>>()
            .map_err(|_| "WebSocket upgrade transport does not match the HTTP server")?;
        let socket = websocket::from_server(
            parts.io.into_inner().into_inner(),
            parts.read_buf.to_vec(),
            limits,
        )
        .await?;
        let mut state = self.0.state.lock().expect("reply lock poisoned");
        state.mode = Mode::Upgraded(Some(socket.clone()));
        Ok(socket)
    }
}

pub(super) struct Session(HttpReply);

impl Session {
    pub(super) fn new(
        budget: Budget,
        live: LiveLimits,
        upgrade: Option<(hyper::Request<()>, hyper::upgrade::OnUpgrade)>,
        upgraded: Arc<AtomicBool>,
    ) -> (Self, oneshot::Receiver<Head>) {
        let (sender, receiver) = oneshot::channel();
        (
            Self(HttpReply(Arc::new(Shared {
                state: Mutex::new(State {
                    mode: Mode::Pending,
                    head: Some(sender),
                    upgrade,
                    fault: None,
                    body_cancelled: false,
                }),
                writer: AsyncMutex::new(()),
                closed: Semaphore::new(0),
                budget,
                live,
                upgraded,
            }))),
            receiver,
        )
    }

    pub(super) fn reply(&self) -> HttpReply {
        self.0.clone()
    }

    pub(super) async fn run(
        self,
        route: impl Future<Output = Result<(), String>>,
    ) -> Result<(), String> {
        let result = tokio::select! {
            biased;
            result = route => result,
            _ = self.0.0.closed.acquire() => Ok(()),
        };
        if let Err(error) = &result {
            let mut state = self.0.0.state.lock().expect("reply lock poisoned");
            // 传输侧先取消正文时，随后被唤醒的 write 失败属于该流，不是程序故障。
            if state.body_cancelled {
                return Ok(());
            }
            state.fault = Some(error.clone());
        }
        result
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut state = self.0.0.state.lock().expect("reply lock poisoned");
        // 函数返回或取消都释放正文发送端/升级连接；外部别名不能延长会话。
        if let Mode::Upgraded(Some(socket)) = &state.mode {
            socket.abort();
        }
        state.mode = Mode::Finished;
        state.upgrade.take();
        state.head.take();
    }
}

pub async fn respond(reply: &HttpReply, response: Response) -> Result<(), String> {
    let _writer = reply.0.writer.lock().await;
    let response = encode_response(response, reply.0.budget)?;
    reply.publish(|| {
        (
            response.map(|body| {
                Outgoing::Buffered(BufferedBody {
                    body,
                    reply: reply.clone(),
                })
            }),
            Mode::Finished,
        )
    })
}

pub async fn start(reply: &HttpReply, status: i64, headers: Vec<Header>) -> Result<(), String> {
    start_stream(reply, status, headers, false).await
}

pub(crate) async fn start_stream(
    reply: &HttpReply,
    status: i64,
    headers: Vec<Header>,
    sse: bool,
) -> Result<(), String> {
    if matches!(status, 204 | 205 | 304) {
        return Err("bodyless status requires http.respond".into());
    }
    let _writer = reply.0.writer.lock().await;
    let response = encode_response(
        Response {
            status,
            headers,
            body: Bytes::new(Vec::new()),
        },
        reply.0.budget,
    )?;
    reply.publish(|| {
        let (sender, receiver) = mpsc::channel(1);
        let millis = if sse {
            reply.0.live.heartbeat_ms
        } else {
            reply.0.live.idle_ms
        };
        let interval = Duration::from_millis(millis as u64);
        let body = StreamBody {
            receiver,
            reply: reply.clone(),
            interval,
            deadline: Box::pin(tokio::time::sleep(interval)),
            sse,
            ended: false,
        };
        (
            response.map(|_| Outgoing::Stream(body)),
            Mode::Streaming { sender, sse },
        )
    })
}

pub async fn write(reply: &HttpReply, bytes: Bytes) -> Result<(), String> {
    write_chunk(reply, bytes, false).await
}

pub(crate) async fn write_chunk(reply: &HttpReply, bytes: Bytes, sse: bool) -> Result<(), String> {
    if bytes.values().len() > reply.chunk_limit() {
        return Err("HTTP chunk exceeds chunk_bytes".into());
    }
    tokio::time::timeout(reply.0.budget.timeout, async {
        let _writer = reply.0.writer.lock().await;
        let sender = {
            let state = reply.0.state.lock().expect("reply lock poisoned");
            match &state.mode {
                Mode::Streaming { sender, sse: mode } if *mode == sse => sender.clone(),
                _ => return Err("HTTP reply is not an open stream of this kind".into()),
            }
        };
        sender
            .send(bytes.into_http())
            .await
            .map_err(|_| "HTTP peer is closed".into())
    })
    .await
    .map_err(|_| "HTTP stream write timed out".to_owned())?
}

pub async fn finish(reply: &HttpReply) -> Result<(), String> {
    let _writer = reply.0.writer.lock().await;
    let mut state = reply.0.state.lock().expect("reply lock poisoned");
    if !matches!(state.mode, Mode::Streaming { .. }) {
        return Err("HTTP reply is not an open stream".into());
    }
    state.mode = Mode::Finished;
    Ok(())
}

pub(super) enum Outgoing {
    Full(Full<bytes::Bytes>),
    Buffered(BufferedBody),
    Stream(StreamBody),
    Handler(HandlerBody),
}

impl Outgoing {
    pub(super) fn with_handler(self, handler: task::Group) -> Self {
        Self::Handler(HandlerBody {
            body: Box::new(self),
            _handler: handler,
        })
    }
}

pub(super) struct HandlerBody {
    body: Box<Outgoing>,
    _handler: task::Group,
}

pub(super) struct BufferedBody {
    body: Full<bytes::Bytes>,
    reply: HttpReply,
}

impl Drop for BufferedBody {
    fn drop(&mut self) {
        self.reply.0.close_body(!self.body.is_end_stream());
    }
}

pub(super) struct StreamBody {
    receiver: mpsc::Receiver<bytes::Bytes>,
    reply: HttpReply,
    interval: Duration,
    deadline: Pin<Box<Sleep>>,
    sse: bool,
    ended: bool,
}

impl Drop for StreamBody {
    fn drop(&mut self) {
        self.reply.0.close_body(!self.ended);
    }
}

impl Body for Outgoing {
    type Data = bytes::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let body = match self.get_mut() {
            Self::Stream(body) => body,
            Self::Full(body) => {
                return Pin::new(body)
                    .poll_frame(cx)
                    .map(|frame| frame.map(|frame| frame.map_err(|never| match never {})));
            }
            Self::Buffered(body) => {
                return Pin::new(&mut body.body)
                    .poll_frame(cx)
                    .map(|frame| frame.map(|frame| frame.map_err(|never| match never {})));
            }
            Self::Handler(body) => return Pin::new(body.body.as_mut()).poll_frame(cx),
        };
        if body.ended {
            return Poll::Ready(None);
        }
        match body.receiver.poll_recv(cx) {
            Poll::Ready(Some(bytes)) => {
                body.deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + body.interval);
                return Poll::Ready(Some(Ok(Frame::data(bytes))));
            }
            Poll::Ready(None) => {
                body.ended = true;
                let fault = body
                    .reply
                    .0
                    .state
                    .lock()
                    .expect("reply lock poisoned")
                    .fault
                    .clone();
                return Poll::Ready(fault.map(|fault| Err(std::io::Error::other(fault))));
            }
            Poll::Pending => {}
        }
        if body.deadline.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        body.deadline
            .as_mut()
            .reset(tokio::time::Instant::now() + body.interval);
        if body.sse {
            Poll::Ready(Some(Ok(Frame::data(bytes::Bytes::from_static(b":\n\n")))))
        } else {
            body.ended = true;
            body.reply.0.close_body(true);
            Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "HTTP stream idle timeout",
            ))))
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Full(body) => body.is_end_stream(),
            Self::Buffered(body) => body.body.is_end_stream(),
            Self::Stream(body) => body.ended,
            Self::Handler(body) => body.body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Full(body) => body.size_hint(),
            Self::Buffered(body) => body.body.size_hint(),
            Self::Stream(_) => SizeHint::default(),
            Self::Handler(body) => body.body.size_hint(),
        }
    }
}
