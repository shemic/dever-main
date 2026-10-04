use std::future::Future;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::stream;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as AsyncMutex;

use crate::async_stream::AsyncStream;
use crate::bytes::Bytes;
use crate::endpoint::Endpoint;
use crate::resource::{Pull, read_limit};

#[derive(Clone, Debug)]
pub struct Listener(Arc<Endpoint<TcpListener>>);

impl Listener {
    pub fn close(&self) -> Result<(), String> {
        self.0.close()
    }
}

#[derive(Debug)]
struct SocketState {
    endpoint: Endpoint<TcpStream>,
    reader: AsyncMutex<()>,
    writer: AsyncMutex<()>,
    timeout_millis: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct Socket(Arc<SocketState>);

impl Socket {
    fn new(stream: TcpStream) -> Self {
        Self(Arc::new(SocketState {
            endpoint: Endpoint::new(stream),
            reader: AsyncMutex::new(()),
            writer: AsyncMutex::new(()),
            timeout_millis: AtomicU64::new(0),
        }))
    }

    pub fn close(&self) -> Result<(), String> {
        self.0.endpoint.close()
    }

    async fn operation<R>(
        &self,
        operation: impl Future<Output = Result<R, String>>,
    ) -> Result<R, String> {
        let operation = self.0.endpoint.while_open(operation);
        match self.0.timeout_millis.load(Ordering::Relaxed) {
            0 => operation.await,
            millis => timed(Duration::from_millis(millis), operation).await,
        }
    }
}

pub async fn connect(host: &str, port: i64) -> Result<Socket, String> {
    connect_stream(host, port).await.map(Socket::new)
}

pub(crate) async fn connect_stream(host: &str, port: i64) -> Result<TcpStream, String> {
    let addresses = addresses(host, port).await?;
    let stream = TcpStream::connect(addresses.as_slice())
        .await
        .map_err(|error| error.to_string())?;
    prepare_stream(stream)
}

pub async fn connect_timeout(address: &str, port: i64, millis: i64) -> Result<Socket, String> {
    connect_stream_timeout(address, port, millis)
        .await
        .map(Socket::new)
}

pub(crate) async fn connect_stream_timeout(
    address: &str,
    port: i64,
    millis: i64,
) -> Result<TcpStream, String> {
    let address = address
        .parse::<IpAddr>()
        .map_err(|_| "timed connect requires a numeric IP address")?;
    let address = SocketAddr::new(address, port_number(port)?);
    timed(timeout_duration(millis)?, async {
        let stream = TcpStream::connect(address)
            .await
            .map_err(|error| error.to_string())?;
        prepare_stream(stream)
    })
    .await
}

pub async fn listen(host: &str, port: i64) -> Result<Listener, String> {
    let addresses = addresses(host, port).await?;
    let listener = TcpListener::bind(addresses.as_slice())
        .await
        .map_err(|error| error.to_string())?;
    Ok(Listener(Arc::new(Endpoint::new(listener))))
}

pub async fn accept(listener: &Listener) -> Result<Socket, String> {
    accept_stream(listener).await.map(Socket::new)
}

async fn accept_stream(listener: &Listener) -> Result<TcpStream, String> {
    let socket = listener.0.get()?;
    listener
        .0
        .while_open(async {
            let (stream, _) = socket.accept().await.map_err(|error| error.to_string())?;
            prepare_stream(stream)
        })
        .await
}

pub fn port(listener: &Listener) -> Result<i64, String> {
    listener
        .0
        .get()?
        .local_addr()
        .map(|address| i64::from(address.port()))
        .map_err(|error| error.to_string())
}

fn prepare_stream(stream: TcpStream) -> Result<TcpStream, String> {
    stream
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;
    Ok(stream)
}

pub fn timeout(socket: &Socket, millis: i64) -> Result<(), String> {
    let duration = timeout_duration(millis)?;
    socket.0.endpoint.get()?;
    socket
        .0
        .timeout_millis
        .store(duration.as_millis() as u64, Ordering::Relaxed);
    Ok(())
}

pub async fn read(socket: &Socket, limit: i64) -> Result<Option<Bytes>, String> {
    let limit = read_limit(limit)?;
    let stream = socket.0.endpoint.get()?;
    socket
        .operation(async {
            let _reader = socket.0.reader.lock().await;
            // 等待读锁和可读事件后再分配缓冲区，避免等待者各占一份内存。
            stream.readable().await.map_err(|error| error.to_string())?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(limit)
                .map_err(|error| error.to_string())?;
            bytes.resize(limit, 0);
            loop {
                match stream.try_read(&mut bytes) {
                    Ok(length) => {
                        bytes.truncate(length);
                        return Ok((length != 0).then(|| Bytes::new(bytes)));
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        stream.readable().await.map_err(|error| error.to_string())?;
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
        })
        .await
}

// 部分写入中断时先关闭连接，再释放写锁，阻止下一次写入接在残缺内容后。
struct WriteProgress<'a> {
    socket: &'a Socket,
    partial: bool,
}

impl Drop for WriteProgress<'_> {
    fn drop(&mut self) {
        if self.partial {
            let _ = self.socket.close();
        }
    }
}

pub async fn write(socket: &Socket, bytes: &Bytes) -> Result<(), String> {
    let stream = socket.0.endpoint.get()?;
    socket
        .operation(async {
            let _writer = socket.0.writer.lock().await;
            let mut progress = WriteProgress {
                socket,
                partial: false,
            };
            let mut offset = 0;
            while offset < bytes.values().len() {
                stream.writable().await.map_err(|error| error.to_string())?;
                match stream.try_write(&bytes.values()[offset..]) {
                    Ok(0) => return Err("socket write returned zero bytes".into()),
                    Ok(length) => {
                        offset += length;
                        progress.partial = offset < bytes.values().len();
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(())
        })
        .await
}

pub fn connections(listener: Listener) -> AsyncStream<Result<Socket, String>> {
    incoming(listener, false).map(|connection| connection.map(Socket::new))
}

pub(crate) fn incoming(
    listener: Listener,
    end_on_close: bool,
) -> AsyncStream<Result<TcpStream, String>> {
    AsyncStream::new(stream::unfold(listener, move |listener| async move {
        let result = tokio::select! {
            biased;
            _ = listener.closed(), if end_on_close => return None,
            result = accept_stream(&listener) => result,
        };
        let event = match result {
            Ok(socket) => Pull::Item(Ok(socket)),
            Err(error) => Pull::Last(Err(error)),
        };
        Some((event, listener))
    }))
}

impl Listener {
    pub(crate) async fn closed(&self) {
        self.0.closed().await;
    }
}

pub fn chunks(socket: Socket, limit: i64) -> Result<AsyncStream<Result<Bytes, String>>, String> {
    read_limit(limit)?;
    socket.0.endpoint.get()?;
    Ok(AsyncStream::new(stream::unfold(
        socket,
        move |socket| async move {
            let event = match read(&socket, limit).await {
                Ok(Some(bytes)) => Pull::Item(Ok(bytes)),
                Ok(None) => return None,
                Err(error) => Pull::Last(Err(error)),
            };
            Some((event, socket))
        },
    )))
}

async fn addresses(host: &str, port: i64) -> Result<Vec<SocketAddr>, String> {
    let port = port_number(port)?;
    if let Ok(address) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(address, port)]);
    }
    let host = host.to_owned();
    // DNS 复用有界 blocking 通道及其结构化清理。
    crate::task::blocking(move || {
        (host.as_str(), port)
            .to_socket_addrs()
            .map(Iterator::collect)
            .map_err(|error| error.to_string())
    })
    .await
}

fn port_number(port: i64) -> Result<u16, String> {
    u16::try_from(port).map_err(|_| "port must be between 0 and 65535".into())
}

fn timeout_duration(millis: i64) -> Result<Duration, String> {
    let millis = u64::try_from(millis)
        .ok()
        .filter(|millis| *millis > 0)
        .ok_or("timeout must be a positive Int")?;
    Ok(Duration::from_millis(millis))
}

async fn timed<T>(
    duration: Duration,
    operation: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(duration, operation)
        .await
        .map_err(|_| "network operation timed out".to_owned())?
}
