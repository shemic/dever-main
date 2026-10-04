use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

use crate::{
    net,
    task::{ScopeFault, ScopedOwner},
};

pub(super) fn configure_liveness<E>(
    builder: &mut hyper::server::conn::http2::Builder<E>,
    timeout: Duration,
) {
    builder
        .keep_alive_interval(Some(timeout))
        .keep_alive_timeout(timeout);
}

#[derive(Clone)]
pub(super) struct FirstRequest(Arc<FirstRequestState>);

struct FirstRequestState {
    seen: AtomicBool,
    changed: Notify,
}

impl FirstRequest {
    pub(super) fn new() -> Self {
        Self(Arc::new(FirstRequestState {
            seen: AtomicBool::new(false),
            changed: Notify::new(),
        }))
    }

    pub(super) fn mark(&self) {
        if !self.0.seen.swap(true, Ordering::AcqRel) {
            self.0.changed.notify_one();
        }
    }

    async fn wait(&self) {
        loop {
            let changed = self.0.changed.notified();
            if self.0.seen.load(Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }
}

pub(super) async fn drive<C, Shutdown>(
    connection: C,
    listener: &net::Listener,
    first_request: &FirstRequest,
    timeout: Duration,
    fault: &ScopeFault,
    mut shutdown: Shutdown,
) -> Result<(), String>
where
    C: Future<Output = Result<(), hyper::Error>>,
    Shutdown: for<'a> FnMut(Pin<&'a mut C>),
{
    tokio::pin!(connection);
    tokio::select! {
        biased;
        error = fault.wait() => return Err(error),
        result = &mut connection => return super::connection_result(result),
        _ = listener.closed() => {
            shutdown(connection.as_mut());
            return drain(connection.as_mut(), fault).await;
        }
        _ = first_request.wait() => {}
        _ = tokio::time::sleep(timeout) => return Ok(()),
    }
    tokio::select! {
        biased;
        error = fault.wait() => Err(error),
        result = &mut connection => super::connection_result(result),
        _ = listener.closed() => {
            shutdown(connection.as_mut());
            drain(connection.as_mut(), fault).await
        }
    }
}

async fn drain<C>(mut connection: Pin<&mut C>, fault: &ScopeFault) -> Result<(), String>
where
    C: Future<Output = Result<(), hyper::Error>>,
{
    tokio::select! {
        biased;
        error = fault.wait() => Err(error),
        result = connection.as_mut() => super::connection_result(result),
    }
}

pub(super) async fn complete(owner: ScopedOwner, result: Result<(), String>) -> Result<(), String> {
    match result {
        Ok(()) => owner.finish().await,
        Err(error) => {
            let _ = owner.stop().await;
            Err(error)
        }
    }
}
