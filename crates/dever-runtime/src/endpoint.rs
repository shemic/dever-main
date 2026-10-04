use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

#[derive(Debug)]
pub(crate) struct Endpoint<T> {
    socket: Mutex<Option<Arc<T>>>,
    closed: Semaphore,
}

impl<T> Endpoint<T> {
    pub(crate) fn new(socket: T) -> Self {
        Self {
            socket: Mutex::new(Some(Arc::new(socket))),
            closed: Semaphore::new(0),
        }
    }

    pub(crate) fn get(&self) -> Result<Arc<T>, String> {
        self.socket
            .lock()
            .expect("endpoint lock poisoned")
            .clone()
            .ok_or_else(|| "resource is closed".into())
    }

    pub(crate) fn close(&self) -> Result<(), String> {
        self.take().map(|_| ())
    }

    pub(crate) fn take(&self) -> Result<Arc<T>, String> {
        let socket = self.socket.lock().expect("endpoint lock poisoned").take();
        self.closed.close();
        socket.ok_or_else(|| "resource is closed".into())
    }

    pub(crate) async fn while_open<R>(
        &self,
        operation: impl Future<Output = Result<R, String>>,
    ) -> Result<R, String> {
        tokio::select! {
            biased;
            _ = self.closed.acquire() => Err("resource is closed".into()),
            result = operation => result,
        }
    }

    pub(crate) async fn closed(&self) {
        let _closed = self.closed.acquire().await;
    }
}
