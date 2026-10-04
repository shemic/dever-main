use std::pin::Pin;
use std::sync::Arc;

use futures_util::{Stream, StreamExt, stream};
use tokio::sync::{Mutex, Semaphore};

use crate::resource::Pull;

type Producer<T> = Pin<Box<dyn Stream<Item = Pull<T>> + Send>>;

struct State<T> {
    producer: Mutex<Option<Producer<T>>>,
    closed: Semaphore,
}

pub struct AsyncStream<T>(Arc<State<T>>);

impl<T> Clone for AsyncStream<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> std::fmt::Debug for AsyncStream<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AsyncStream(<resource>)")
    }
}

impl<T> AsyncStream<T> {
    pub fn new(producer: impl Stream<Item = Pull<T>> + Send + 'static) -> Self {
        Self(Arc::new(State {
            producer: Mutex::new(Some(Box::pin(producer))),
            closed: Semaphore::new(0),
        }))
    }

    async fn pull_next(&self) -> Pull<T> {
        let mut producer = self.0.producer.lock().await;
        let result = match producer.as_mut() {
            Some(producer) => tokio::select! {
                biased;
                _ = self.0.closed.acquire() => Pull::End,
                value = producer.next() => value.unwrap_or(Pull::End),
            },
            None => Pull::End,
        };
        if matches!(&result, Pull::Last(_) | Pull::End) {
            producer.take();
        }
        result
    }

    pub async fn pull(&self) -> Option<T> {
        match self.pull_next().await {
            Pull::Item(value) | Pull::Last(value) => Some(value),
            Pull::End => None,
        }
    }

    pub fn close(&self) {
        self.0.closed.close();
        // 挂起的 pull 收到关闭信号后，会在其持有的锁内释放生产者。
        if let Ok(mut producer) = self.0.producer.try_lock() {
            producer.take();
        }
    }

    pub fn close_on_drop(&self) -> CloseOnDrop<T> {
        CloseOnDrop(self.clone())
    }
}

impl<T: Send + 'static> AsyncStream<T> {
    pub fn from_values(values: impl IntoIterator<Item = T, IntoIter: Send + 'static>) -> Self {
        Self::new(stream::iter(values.into_iter().map(Pull::Item)))
    }

    pub fn map<U: Send + 'static>(
        self,
        transform: impl FnMut(T) -> U + Send + 'static,
    ) -> AsyncStream<U> {
        let owner = self.close_on_drop();
        AsyncStream::new(stream::unfold(
            (owner, transform),
            |(owner, mut transform)| async move {
                let result = match owner.0.pull_next().await {
                    Pull::Item(value) => Pull::Item(transform(value)),
                    Pull::Last(value) => Pull::Last(transform(value)),
                    Pull::End => return None,
                };
                Some((result, (owner, transform)))
            },
        ))
    }
}

pub struct CloseOnDrop<T>(AsyncStream<T>);

impl<T> Drop for CloseOnDrop<T> {
    fn drop(&mut self) {
        self.0.close();
    }
}
