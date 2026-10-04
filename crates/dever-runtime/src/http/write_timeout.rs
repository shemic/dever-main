use std::future::Future;
use std::io::{self, IoSlice};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::transport::Transport;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

// Header/body 的期限由协议层持有；这里仅限制无法取得进展的写操作。
pub(super) struct WriteTimeout {
    stream: Transport,
    timeout: Duration,
    deadline: Option<Pin<Box<Sleep>>>,
}

impl WriteTimeout {
    pub(super) fn into_inner(self) -> Transport {
        self.stream
    }
    pub fn new(stream: Transport, timeout: Duration) -> Self {
        Self {
            stream,
            timeout,
            deadline: None,
        }
    }

    fn output<T>(
        &mut self,
        context: &mut Context<'_>,
        write: impl FnOnce(Pin<&mut Transport>, &mut Context<'_>) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match write(Pin::new(&mut self.stream), context) {
            Poll::Ready(result) => {
                self.deadline = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                let deadline = self
                    .deadline
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(self.timeout)));
                match deadline.as_mut().poll(context) {
                    Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "HTTP write timed out",
                    ))),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

impl AsyncRead for WriteTimeout {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(context, buffer)
    }
}

impl AsyncWrite for WriteTimeout {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.output(context, |stream, context| stream.poll_write(context, bytes))
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.output(context, |stream, context| {
            stream.poll_write_vectored(context, buffers)
        })
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.output(context, |stream, context| stream.poll_flush(context))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.output(context, |stream, context| stream.poll_shutdown(context))
    }
}
