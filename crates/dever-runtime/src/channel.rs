use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;

pub const MAX_CHANNEL_CAPACITY: usize = 65_536;

struct State<T> {
    queue: Mutex<Queue<T>>,
    available_items: Arc<Semaphore>,
    available_slots: Arc<Semaphore>,
}

struct Queue<T> {
    values: VecDeque<T>,
    closed: bool,
}

pub struct Channel<T>(Arc<State<T>>);

impl<T> std::fmt::Debug for Channel<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Channel(<resource>)")
    }
}

impl<T> Clone for Channel<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Send + 'static> Channel<T> {
    pub fn stream(self) -> crate::async_stream::AsyncStream<T> {
        crate::async_stream::AsyncStream::new(futures_util::stream::unfold(
            self,
            |channel| async move {
                let value = channel
                    .receive()
                    .await
                    .expect("channel state lock poisoned")?;
                Some((crate::resource::Pull::Item(value), channel))
            },
        ))
    }
}

impl<T> Channel<T> {
    pub fn new(capacity: i64) -> Result<Self, &'static str> {
        let capacity = usize::try_from(capacity)
            .ok()
            .filter(|capacity| (1..=MAX_CHANNEL_CAPACITY).contains(capacity))
            .ok_or("channel capacity must be between 1 and 65536")?;
        Ok(Self(Arc::new(State {
            queue: Mutex::new(Queue {
                values: VecDeque::new(),
                closed: false,
            }),
            available_items: Arc::new(Semaphore::new(0)),
            available_slots: Arc::new(Semaphore::new(capacity)),
        })))
    }

    pub async fn send(&self, value: T) -> Result<(), String> {
        let slot = Arc::clone(&self.0.available_slots)
            .acquire_owned()
            .await
            .map_err(|_| "channel is closed".to_owned())?;
        let mut queue = self
            .0
            .queue
            .lock()
            .map_err(|_| "channel state lock failed".to_owned())?;
        if queue.closed {
            return Err("channel is closed".to_owned());
        }
        queue.values.push_back(value);
        slot.forget();
        drop(queue);
        self.0.available_items.add_permits(1);
        Ok(())
    }

    pub async fn receive(&self) -> Result<Option<T>, String> {
        let item = Arc::clone(&self.0.available_items).acquire_owned().await;
        let value = self
            .0
            .queue
            .lock()
            .map_err(|_| "channel state lock failed".to_owned())?
            .values
            .pop_front();
        if let Ok(item) = item {
            item.forget();
            if value.is_some() {
                self.0.available_slots.add_permits(1);
            }
        }
        Ok(value)
    }

    pub async fn close(&self) -> Result<(), String> {
        let mut queue = self
            .0
            .queue
            .lock()
            .map_err(|_| "channel state lock failed".to_owned())?;
        if queue.closed {
            return Ok(());
        }
        queue.closed = true;
        drop(queue);
        self.0.available_slots.close();
        self.0.available_items.close();
        Ok(())
    }
}
