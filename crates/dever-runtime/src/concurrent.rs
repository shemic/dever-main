use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};

pub const MAX_WORKERS: usize = 256;
const MAX_BATCH_ITEMS: usize = 256;
const BATCHES_PER_WORKER: usize = 8;

pub fn worker_count(workers: i64) -> Result<usize, &'static str> {
    usize::try_from(workers)
        .ok()
        .filter(|count| (1..=MAX_WORKERS).contains(count))
        .ok_or("parallel worker count must be between 1 and 256")
}

/// 单一生产者按需取值，有界队列防止 Stream 被提前收集；返回前回收全部工作线程。
pub fn each<T: Send>(
    values: impl IntoIterator<Item = T>,
    workers: i64,
    apply: impl Fn(T) -> Result<(), String> + Sync,
) -> Result<(), String> {
    each_typed(values, workers, apply)
}

pub fn each_typed<T: Send, E: From<String> + Send>(
    values: impl IntoIterator<Item = T>,
    workers: i64,
    apply: impl Fn(T) -> Result<(), E> + Sync,
) -> Result<(), E> {
    let workers = worker_count(workers).map_err(|error| E::from(error.to_owned()))?;
    run_typed(values, workers, |value, _| apply(value))
}

/// 内存序列没有拉取副作用；限制每批元素数，同时给工作线程保留多批调度机会。
pub fn each_slice<T: Sync>(
    values: &[T],
    workers: i64,
    apply: impl Fn(&T) -> Result<(), String> + Sync,
) -> Result<(), String> {
    each_slice_typed(values, workers, apply)
}

pub fn each_slice_typed<T: Sync, E: From<String> + Send>(
    values: &[T],
    workers: i64,
    apply: impl Fn(&T) -> Result<(), E> + Sync,
) -> Result<(), E> {
    let workers = worker_count(workers).map_err(|error| E::from(error.to_owned()))?;
    let batch = batch_size(values.len(), workers);
    run_typed(values.chunks(batch), workers, |chunk, cancelled| {
        for value in chunk {
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            apply(value)?;
        }
        Ok(())
    })
}

pub(crate) fn batch_size(items: usize, workers: usize) -> usize {
    (items / (workers * BATCHES_PER_WORKER)).clamp(1, MAX_BATCH_ITEMS)
}

fn run_typed<T: Send, E: From<String> + Send>(
    values: impl IntoIterator<Item = T>,
    workers: usize,
    apply: impl Fn(T, &AtomicBool) -> Result<(), E> + Sync,
) -> Result<(), E> {
    let mut values = values.into_iter();
    let (minimum_items, maximum_items) = values.size_hint();
    let workers = match maximum_items {
        Some(maximum_items) if maximum_items == minimum_items => workers.min(maximum_items),
        _ => workers,
    };
    if workers == 0 {
        return Ok(());
    }
    let (sender, receiver) = mpsc::sync_channel(workers);
    let receiver = Mutex::new(receiver);
    let cancelled = AtomicBool::new(false);
    let failure = Mutex::new(None);
    let record_failure = |error: E| {
        cancelled.store(true, Ordering::Release);
        let mut failure = failure.lock().expect("worker failure lock");
        if failure.is_none() {
            *failure = Some(error);
        }
    };
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let spawn = std::thread::Builder::new().spawn_scoped(scope, || {
                loop {
                    let next = receiver.lock().expect("worker receiver lock").recv();
                    let Ok(value) = next else { break };
                    // 故障后继续清空队列直到发送端关闭，避免生产者卡在有界 send。
                    if !cancelled.load(Ordering::Acquire) {
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            apply(value, &cancelled)
                        })) {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => record_failure(error),
                            Err(_) => record_failure(E::from("parallel worker panicked".into())),
                        }
                    }
                }
            });
            match spawn {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    record_failure(E::from(error.to_string()));
                    break;
                }
            }
        }
        while !cancelled.load(Ordering::Acquire) {
            let Some(value) = values.next() else { break };
            if sender.send(value).is_err() {
                break;
            }
        }
        drop(sender);
        for handle in handles {
            if handle.join().is_err() {
                record_failure(E::from("parallel worker panicked".into()));
            }
        }
    });
    match failure.into_inner().expect("worker failure lock") {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
