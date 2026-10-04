use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::{AbortHandle, Abortable};
use futures_util::{StreamExt, stream::FuturesUnordered};
use hyper_util::rt::TokioIo;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::time::Instant;

use super::client_body::RequestBody;
use super::executor::Http2Executor;
use super::protocol::Http2Limits;
use crate::transport::Transport;

pub(super) type Driver = Pin<Box<dyn Future<Output = ()> + Send>>;

pub(super) struct Entry {
    pub sender: hyper::client::conn::http1::SendRequest<RequestBody>,
    pub abort: AbortHandle,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

pub(super) struct Idle {
    entries: Mutex<Vec<(Instant, Entry)>>,
    changed: Notify,
    timeout: Duration,
}

impl Idle {
    pub fn new(timeout: Duration) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(Vec::new()),
            changed: Notify::new(),
            timeout,
        })
    }

    pub fn take(&self) -> Option<Entry> {
        let mut entries = self.entries.lock().expect("pool state lock");
        entries.retain(|(deadline, entry)| *deadline > Instant::now() && !entry.sender.is_closed());
        entries.pop().map(|(_, entry)| entry)
    }

    pub fn put(&self, entry: Entry) {
        self.entries
            .lock()
            .expect("pool state lock")
            .push((Instant::now() + self.timeout, entry));
        self.changed.notify_one();
    }

    fn deadline(&self) -> Option<Instant> {
        let mut entries = self.entries.lock().expect("pool state lock");
        entries.retain(|(deadline, entry)| *deadline > Instant::now() && !entry.sender.is_closed());
        entries.iter().map(|(deadline, _)| *deadline).min()
    }
}

pub(super) struct Control {
    pub closed: Semaphore,
    pub requests: Arc<Semaphore>,
    pub connections: Arc<Semaphore>,
}

impl Control {
    pub fn new(requests: usize, connections: usize) -> Arc<Self> {
        Arc::new(Self {
            closed: Semaphore::new(0),
            requests: Arc::new(Semaphore::new(requests)),
            connections: Arc::new(Semaphore::new(connections)),
        })
    }

    pub fn close(&self) {
        self.closed.close();
        self.requests.close();
        self.connections.close();
    }

    pub async fn while_open<T>(
        &self,
        future: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        tokio::select! {
            biased;
            _ = self.closed.acquire() => Err("HTTP client is closed".into()),
            result = future => result,
        }
    }
}

pub(super) struct Http2Entry {
    pub sender: hyper::client::conn::http2::SendRequest<RequestBody>,
    streams: Arc<Semaphore>,
    accepting: AtomicBool,
    active: AtomicUsize,
    idle_deadline: Mutex<Option<Instant>>,
    abort: AbortHandle,
    changed: Arc<Notify>,
    idle_timeout: Duration,
}

impl Http2Entry {
    fn new(
        sender: hyper::client::conn::http2::SendRequest<RequestBody>,
        streams: usize,
        abort: AbortHandle,
        changed: Arc<Notify>,
        idle_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            sender,
            streams: Arc::new(Semaphore::new(streams)),
            accepting: AtomicBool::new(true),
            active: AtomicUsize::new(0),
            idle_deadline: Mutex::new(Some(Instant::now() + idle_timeout)),
            abort,
            changed,
            idle_timeout,
        })
    }

    pub fn acquire(self: &Arc<Self>) -> Option<OwnedSemaphorePermit> {
        if !self.accepting.load(Ordering::Acquire) || self.sender.is_closed() {
            self.drain();
            return None;
        }
        let permit = self.streams.clone().try_acquire_owned().ok()?;
        if !self.accepting.load(Ordering::Acquire) || self.sender.is_closed() {
            self.drain();
            return None;
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        *self.idle_deadline.lock().expect("HTTP/2 pool state lock") = None;
        Some(permit)
    }

    fn release(&self) {
        let previous = self.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "active HTTP/2 stream count");
        if previous == 1 && self.accepting.load(Ordering::Acquire) {
            *self.idle_deadline.lock().expect("HTTP/2 pool state lock") =
                Some(Instant::now() + self.idle_timeout);
        }
        self.changed.notify_waiters();
    }

    pub fn drain(&self) {
        if self.accepting.swap(false, Ordering::AcqRel) {
            self.streams.close();
            *self.idle_deadline.lock().expect("HTTP/2 pool state lock") = None;
            self.changed.notify_waiters();
        }
    }

    fn retire(&self) {
        self.drain();
        self.abort.abort();
    }

    fn deadline(&self) -> Option<Instant> {
        *self.idle_deadline.lock().expect("HTTP/2 pool state lock")
    }
}

pub(super) struct Http2Pool {
    entries: Mutex<Vec<Arc<Http2Entry>>>,
    changed: Arc<Notify>,
    streams: usize,
    idle_timeout: Duration,
}

impl Http2Pool {
    pub fn new(streams: usize, idle_timeout: Duration) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(Vec::new()),
            changed: Arc::new(Notify::new()),
            streams,
            idle_timeout,
        })
    }

    pub fn take(&self) -> Option<(Arc<Http2Entry>, OwnedSemaphorePermit)> {
        let mut entries = self.entries.lock().expect("HTTP/2 pool state lock");
        entries.retain(|entry| entry.accepting.load(Ordering::Acquire));
        entries
            .iter()
            .find_map(|entry| entry.acquire().map(|permit| (Arc::clone(entry), permit)))
    }

    pub fn add(
        &self,
        sender: hyper::client::conn::http2::SendRequest<RequestBody>,
        abort: AbortHandle,
    ) -> Arc<Http2Entry> {
        let entry = Http2Entry::new(
            sender,
            self.streams,
            abort,
            Arc::clone(&self.changed),
            self.idle_timeout,
        );
        self.entries
            .lock()
            .expect("HTTP/2 pool state lock")
            .push(Arc::clone(&entry));
        self.changed.notify_waiters();
        entry
    }

    pub fn changed(&self) -> tokio::sync::futures::OwnedNotified {
        self.changed.clone().notified_owned()
    }

    fn expire_and_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("HTTP/2 pool state lock");
        entries.retain(|entry| {
            if !entry.accepting.load(Ordering::Acquire) || entry.sender.is_closed() {
                entry.drain();
                return false;
            }
            if entry.active.load(Ordering::Acquire) == 0
                && entry.deadline().is_some_and(|deadline| deadline <= now)
            {
                entry.retire();
                return false;
            }
            true
        });
        entries.iter().filter_map(|entry| entry.deadline()).min()
    }

    fn close(&self) {
        let entries = std::mem::take(&mut *self.entries.lock().expect("HTTP/2 pool state lock"));
        for entry in entries {
            entry.retire();
        }
        self.changed.notify_waiters();
    }
}

#[derive(Clone)]
pub(super) struct Http2Lease {
    _owner: Arc<Http2LeaseOwner>,
}

struct Http2LeaseOwner {
    entry: Arc<Http2Entry>,
    stream: Option<OwnedSemaphorePermit>,
    request: Option<OwnedSemaphorePermit>,
}

impl Http2Lease {
    pub fn new(
        entry: Arc<Http2Entry>,
        stream: OwnedSemaphorePermit,
        request: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            _owner: Arc::new(Http2LeaseOwner {
                entry,
                stream: Some(stream),
                request: Some(request),
            }),
        }
    }
}

impl Drop for Http2LeaseOwner {
    fn drop(&mut self) {
        self.stream.take();
        self.request.take();
        self.entry.release();
    }
}

struct DriverOwner {
    control: Arc<Control>,
    idle: Arc<Idle>,
    http2: Option<Arc<Http2Pool>>,
}

impl Drop for DriverOwner {
    fn drop(&mut self) {
        self.control.close();
        self.idle.entries.lock().expect("pool state lock").clear();
        if let Some(http2) = &self.http2 {
            http2.close();
        }
    }
}

pub(super) async fn start(
    control: Arc<Control>,
    idle: Arc<Idle>,
    http2: Option<Arc<Http2Pool>>,
    capacity: usize,
) -> Result<(mpsc::Sender<Driver>, crate::task::Task<()>), String> {
    let (queue, mut receiver) = mpsc::channel::<Driver>(capacity);
    // The parent may finish before this future is first polled.
    let owner = DriverOwner {
        control: control.clone(),
        idle: idle.clone(),
        http2: http2.clone(),
    };
    let task = crate::task::run(async move {
        let _owner = owner;
        let mut drivers = FuturesUnordered::<Driver>::new();
        loop {
            let deadline = [
                idle.deadline(),
                http2.as_ref().and_then(|pool| pool.expire_and_deadline()),
            ]
            .into_iter()
            .flatten()
            .min();
            let expire = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };
            let http2_changed = async {
                match &http2 {
                    Some(pool) => pool.changed().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                _ = control.closed.acquire() => return Ok(()),
                Some(()) = drivers.next(), if !drivers.is_empty() => {},
                _ = idle.changed.notified() => {},
                _ = http2_changed => {},
                _ = expire => {},
                next = receiver.recv() => match next {
                    Some(driver) => drivers.push(driver),
                    None => return Ok(()),
                },
            }
        }
    })
    .await?;
    Ok((queue, task))
}

pub(super) fn driver(
    connection: impl Future<Output = Result<(), hyper::Error>> + Send + 'static,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> (AbortHandle, Driver) {
    let (abort, registration) = AbortHandle::new_pair();
    let driver = Box::pin(async move {
        let _permit = permit;
        // Protocol/I/O failures are observed through SendRequest/Incoming.
        let _completion = Abortable::new(connection, registration).await;
    });
    (abort, driver)
}

async fn drive_http2<C>(
    connection: C,
    mut owner: crate::task::ScopedOwner,
    fault: crate::task::ScopeFault,
    entry: Arc<Http2Entry>,
    registration: futures_util::future::AbortRegistration,
) where
    C: Future<Output = Result<(), hyper::Error>> + Send + 'static,
{
    let completion = {
        let connection = Abortable::new(owner.enter(connection), registration);
        tokio::pin!(connection);
        tokio::select! {
            result = &mut connection => Some(result),
            _ = fault.wait() => None,
        }
    };
    entry.drain();
    match completion {
        Some(Ok(Ok(()))) => {
            let _ = owner.finish().await;
        }
        Some(Ok(Err(_))) | Some(Err(_)) | None => {
            let _ = owner.stop().await;
        }
    }
}

pub(super) fn http2_handshake(
    transport: Transport,
    permit: OwnedSemaphorePermit,
    limits: Http2Limits,
    headers: usize,
    timeout: Duration,
    pool: Arc<Http2Pool>,
) -> (Driver, oneshot::Receiver<Result<Arc<Http2Entry>, String>>) {
    let (result, receiver) = oneshot::channel();
    let driver = Box::pin(async move {
        let (mut owner, spawner, fault) = match crate::task::scoped_owner(permit) {
            Ok(owner) => owner,
            Err(error) => {
                let _ = result.send(Err(error));
                return;
            }
        };
        let handshake = {
            let builder = limits.client_builder(Http2Executor(spawner), headers);
            let handshake = tokio::time::timeout(
                timeout,
                owner.enter(builder.handshake(TokioIo::new(transport))),
            );
            tokio::pin!(handshake);
            tokio::select! {
                result = &mut handshake => match result {
                    Ok(result) => result.map_err(|error| error.to_string()),
                    Err(_) => Err("HTTP/2 handshake timed out".to_owned()),
                },
                error = fault.wait() => Err(error),
            }
        };
        let (sender, connection) = match handshake {
            Ok(connection) => connection,
            Err(error) => {
                let _ = result.send(Err(error));
                let _ = owner.stop().await;
                return;
            }
        };
        let (abort, registration) = AbortHandle::new_pair();
        let entry = pool.add(sender, abort);
        let _ = result.send(Ok(Arc::clone(&entry)));
        drive_http2(connection, owner, fault, entry, registration).await;
    });
    (driver, receiver)
}
