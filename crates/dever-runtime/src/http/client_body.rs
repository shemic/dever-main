use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::stream;
use http_body_util::{BodyExt, StreamBody, combinators::UnsyncBoxBody};
use hyper::body::{Body, Frame, Incoming};

use super::client::Lease;
use super::client_driver::Http2Lease;
use crate::{async_stream::AsyncStream, bytes::Bytes, resource::Pull};

pub(super) type RequestBody = UnsyncBoxBody<bytes::Bytes, io::Error>;

struct LeasedRequestBody {
    body: RequestBody,
    _lease: UploadLease,
}

#[derive(Clone)]
pub(super) struct UploadLease(Arc<Mutex<Option<Http2Lease>>>);

impl UploadLease {
    pub(super) fn replace(&self, lease: Http2Lease) {
        *self.0.lock().expect("HTTP/2 upload lease lock") = Some(lease);
    }

    pub(super) fn clear(&self) {
        self.0.lock().expect("HTTP/2 upload lease lock").take();
    }
}

impl Body for LeasedRequestBody {
    type Data = bytes::Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.body).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}

pub(super) fn hold_upload(body: RequestBody) -> (RequestBody, UploadLease) {
    let lease = UploadLease(Arc::new(Mutex::new(None)));
    let body = LeasedRequestBody {
        body,
        _lease: lease.clone(),
    }
    .boxed_unsync();
    (body, lease)
}

pub(super) fn upload(
    source: AsyncStream<Result<Bytes, String>>,
    maximum: usize,
    timeout: Duration,
) -> RequestBody {
    let owner = source.close_on_drop();
    StreamBody::new(stream::unfold(
        (source, owner),
        move |(source, owner)| async move {
            let chunk = tokio::time::timeout(timeout, source.pull()).await;
            let result = match chunk {
                Ok(Some(Ok(bytes))) if bytes.values().len() <= maximum => {
                    Ok(Frame::data(bytes.into_http()))
                }
                Ok(Some(Ok(_))) => Err(io::Error::other("HTTP upload chunk exceeds chunk_bytes")),
                Ok(Some(Err(error))) => Err(io::Error::other(error)),
                Ok(None) => return None,
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "HTTP upload timed out",
                )),
            };
            if result.is_err() {
                source.close();
            }
            Some((result, (source, owner)))
        },
    ))
    .boxed_unsync()
}

struct Download {
    body: Incoming,
    pending: bytes::Bytes,
    lease: Lease,
}

pub(super) fn download(body: Incoming, lease: Lease) -> AsyncStream<Result<Bytes, String>> {
    AsyncStream::new(stream::unfold(
        Download {
            body,
            pending: bytes::Bytes::new(),
            lease,
        },
        |mut state| async move {
            let client = state.lease.client().clone();
            let next = client
                .control()
                .while_open(async {
                    tokio::time::timeout(client.read_timeout(), state.next())
                        .await
                        .map_err(|_| "HTTP response read timed out".to_owned())?
                })
                .await;
            match next {
                Ok(Some((chunk, last))) => {
                    let item = if last {
                        Pull::Last(Ok(chunk))
                    } else {
                        Pull::Item(Ok(chunk))
                    };
                    Some((item, state))
                }
                Ok(None) => None,
                Err(error) => Some((Pull::Last(Err(error)), state)),
            }
        },
    ))
}

impl Download {
    async fn next(&mut self) -> Result<Option<(Bytes, bool)>, String> {
        while self.pending.is_empty() {
            if self.body.is_end_stream() {
                self.lease.complete().await;
                return Ok(None);
            }
            let Some(frame) = self.body.frame().await else {
                self.lease.complete().await;
                return Ok(None);
            };
            let frame = frame.map_err(|error| error.to_string())?;
            match frame.into_data() {
                Ok(bytes) => self.pending = bytes,
                Err(frame)
                    if frame
                        .trailers_ref()
                        .is_some_and(|trailers| !trailers.is_empty()) =>
                {
                    return Err("HTTP response trailers are not supported".into());
                }
                Err(_) => {}
            }
        }
        let length = self.pending.len().min(self.lease.client().chunk_limit());
        let chunk = Bytes::from_http(self.pending.split_to(length));
        let last = self.pending.is_empty() && self.body.is_end_stream();
        if last {
            self.lease.complete().await;
        }
        Ok(Some((chunk, last)))
    }
}
