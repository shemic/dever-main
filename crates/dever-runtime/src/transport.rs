use std::io::{self, IoSlice};
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

// TLS state is large; plain TCP keeps its direct, allocation-free representation.
pub(crate) enum Transport {
    Tcp(TcpStream),
    Tls(Box<tokio_rustls::TlsStream<TcpStream>>),
}

macro_rules! dispatch {
    ($value:expr, $method:ident $(, $argument:expr)*) => {
        match $value {
            Transport::Tcp(stream) => Pin::new(stream).$method($($argument),*),
            Transport::Tls(stream) => Pin::new(stream.as_mut()).$method($($argument),*),
        }
    };
}

impl AsyncRead for Transport {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        dispatch!(self.get_mut(), poll_read, context, buffer)
    }
}

impl AsyncWrite for Transport {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        dispatch!(self.get_mut(), poll_write, context, bytes)
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        dispatch!(self.get_mut(), poll_write_vectored, context, buffers)
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(stream) => stream.is_write_vectored(),
            Self::Tls(stream) => stream.is_write_vectored(),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        dispatch!(self.get_mut(), poll_flush, context)
    }
    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        dispatch!(self.get_mut(), poll_shutdown, context)
    }
}

pub(crate) async fn connect(
    host: &str,
    port: i64,
    tls: Option<&crate::tls::ClientTls>,
) -> Result<Transport, String> {
    let stream = crate::net::connect_stream(host, port).await?;
    match tls {
        Some(config) => crate::tls::connect(stream, host, config).await,
        None => Ok(Transport::Tcp(stream)),
    }
}
