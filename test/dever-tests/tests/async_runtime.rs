use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use dever_runtime::{channel::Channel, task};

struct DropSignal(Arc<AtomicBool>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn tasks_return_values_and_stop_waits_for_cleanup() {
    task::run_entry(async {
        let task = task::run(async { Ok::<_, String>(42_i64) }).await?;
        assert_eq!(task::wait(task).await?, 42);

        let started = Channel::new(1)?;
        let child_started = started.clone();
        let dropped = Arc::new(AtomicBool::new(false));
        let child_dropped = Arc::clone(&dropped);
        let task = task::run(async move {
            let _drop_signal = DropSignal(child_dropped);
            child_started.send(()).await?;
            std::future::pending::<()>().await;
            Ok(())
        })
        .await?;
        assert_eq!(started.receive().await?, Some(()));
        task::stop(task).await?;
        assert!(dropped.load(Ordering::SeqCst));
        Ok(())
    })
    .unwrap();
}

#[test]
fn group_failure_cancels_and_drains_remaining_tasks() {
    task::run_entry(async {
        let started = Channel::new(1)?;
        let child_started = started.clone();
        let dropped = Arc::new(AtomicBool::new(false));
        let child_dropped = Arc::clone(&dropped);
        let mut group = task::Group::new(2)?;
        group
            .run(async move {
                let _drop_signal = DropSignal(child_dropped);
                child_started.send(()).await?;
                std::future::pending::<()>().await;
                Ok(())
            })
            .await?;
        assert_eq!(started.receive().await?, Some(()));
        group
            .run(async { Err::<(), _>("expected child failure".to_owned()) })
            .await?;

        assert_eq!(group.wait().await.unwrap_err(), "expected child failure");
        assert!(dropped.load(Ordering::SeqCst));
        Ok(())
    })
    .unwrap();
}

#[test]
fn group_capacity_applies_backpressure_before_starting_more_work() {
    task::run_entry(async {
        let release = Channel::new(1)?;
        let wait_for_release = release.clone();
        let released = Arc::new(AtomicBool::new(false));
        let release_recorded = Arc::clone(&released);
        let mut group = task::Group::new(1)?;
        group
            .run(async move {
                wait_for_release.receive().await?;
                Ok(())
            })
            .await?;

        let unblock = task::run(async move {
            task::sleep(1).await?;
            release_recorded.store(true, Ordering::SeqCst);
            release.send(()).await
        })
        .await?;
        group.run(async { Ok(()) }).await?;
        assert!(released.load(Ordering::SeqCst));
        group.wait().await?;
        task::wait(unblock).await?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn channels_are_bounded_and_close_after_buffered_values() {
    task::run_entry(async {
        let channel = Channel::new(1)?;
        channel.send(7_i64).await?;
        channel.close().await?;
        assert_eq!(channel.receive().await?, Some(7));
        assert_eq!(channel.receive().await?, None);
        assert!(channel.send(8).await.is_err());
        Ok(())
    })
    .unwrap();

    assert!(Channel::<i64>::new(0).is_err());
    assert!(Channel::<i64>::new(65_537).is_err());
}

#[test]
fn closing_a_channel_wakes_blocked_senders_and_receivers() {
    task::run_entry(async {
        let full = Channel::new(1)?;
        full.send(1_i64).await?;
        let blocked_sender = full.clone();
        let send = task::run(async move { blocked_sender.send(2).await }).await?;
        full.close().await?;
        assert_eq!(task::wait(send).await.unwrap_err(), "channel is closed");
        assert_eq!(full.receive().await?, Some(1));
        assert_eq!(full.receive().await?, None);

        let empty = Channel::<i64>::new(1)?;
        let blocked_receiver = empty.clone();
        let receive = task::run(async move { blocked_receiver.receive().await }).await?;
        empty.close().await?;
        assert_eq!(task::wait(receive).await?, None);
        Ok(())
    })
    .unwrap();
}

#[test]
fn parallel_and_blocking_use_the_bounded_blocking_pool() {
    task::run_entry(async {
        task::sleep(0).await?;
        assert_eq!(
            task::sleep(-1).await,
            Err("sleep duration must not be negative")
        );
        assert_eq!(task::parallel(|| Ok::<_, String>(6 * 7)).await?, 42);
        assert_eq!(task::blocking(|| Ok::<_, String>("done")).await?, "done");
        Ok(())
    })
    .unwrap();
}

#[test]
fn runtime_configuration_rejects_unbounded_thread_counts() {
    let result = task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: task::MAX_ASYNC_WORKERS + 1,
            max_blocking_threads: 1,
            task_capacity: 1,
        },
        async { Ok::<_, String>(()) },
    );
    assert!(result.unwrap_err().contains("async worker count"));
}

#[test]
fn root_and_parent_failures_wait_for_descendant_cleanup() {
    let root_dropped = Arc::new(AtomicBool::new(false));
    let root_child_dropped = Arc::clone(&root_dropped);
    let root_result = task::run_entry(async move {
        let started = Channel::new(1)?;
        let child_started = started.clone();
        let _child = task::run(async move {
            let _drop_signal = DropSignal(root_child_dropped);
            child_started.send(()).await?;
            std::future::pending::<()>().await;
            Ok(())
        })
        .await?;
        assert_eq!(started.receive().await?, Some(()));
        Err::<(), _>("root failure".to_owned())
    });
    assert_eq!(root_result.unwrap_err(), "root failure");
    assert!(root_dropped.load(Ordering::SeqCst));

    let descendant_dropped = Arc::new(AtomicBool::new(false));
    let child_dropped = Arc::clone(&descendant_dropped);
    task::run_entry(async move {
        let child_started = Channel::new(1)?;
        let wait_for_child = child_started.clone();
        let parent_ready = Channel::new(1)?;
        let notify_root = parent_ready.clone();
        let parent = task::run(async move {
            let _descendant = task::run(async move {
                let _drop_signal = DropSignal(child_dropped);
                child_started.send(()).await?;
                std::future::pending::<()>().await;
                Ok(())
            })
            .await?;
            assert_eq!(wait_for_child.receive().await?, Some(()));
            notify_root.send(()).await?;
            Err::<(), _>("parent failure".to_owned())
        })
        .await?;
        assert_eq!(parent_ready.receive().await?, Some(()));
        assert_eq!(task::wait(parent).await.unwrap_err(), "parent failure");
        assert!(descendant_dropped.load(Ordering::SeqCst));
        Ok(())
    })
    .unwrap();

    let group_dropped = Arc::new(AtomicBool::new(false));
    let group_child_dropped = Arc::clone(&group_dropped);
    let group_result = task::run_entry(async move {
        let started = Channel::new(1)?;
        let child_started = started.clone();
        let mut group = task::Group::new(1)?;
        group
            .run(async move {
                let _drop_signal = DropSignal(group_child_dropped);
                child_started.send(()).await?;
                std::future::pending::<()>().await;
                Ok(())
            })
            .await?;
        assert_eq!(started.receive().await?, Some(()));
        Err::<(), _>("group parent failure".to_owned())
    });
    assert_eq!(group_result.unwrap_err(), "group parent failure");
    assert!(group_dropped.load(Ordering::SeqCst));
}

#[test]
fn stopping_a_task_waits_for_started_blocking_work() {
    let finished = Arc::new(AtomicBool::new(false));
    let blocking_finished = Arc::clone(&finished);
    let (started, running) = mpsc::channel();
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 1,
            max_blocking_threads: 1,
            task_capacity: 4,
        },
        async move {
            let child = task::run(async move {
                task::blocking(move || {
                    started.send(()).unwrap();
                    std::thread::sleep(Duration::from_millis(20));
                    blocking_finished.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
            })
            .await?;
            running.recv_timeout(Duration::from_secs(1)).unwrap();
            task::stop(child).await?;
            assert!(finished.load(Ordering::SeqCst));
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn async_parallel_each_shares_the_configured_blocking_limit() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 2,
            max_blocking_threads: 2,
            task_capacity: 4,
        },
        async {
            let run_batch = |active: Arc<AtomicUsize>, peak: Arc<AtomicUsize>| async move {
                task::parallel_each(0..32, 256, move |_| {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(1));
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
            };
            let first = task::run(run_batch(Arc::clone(&active), Arc::clone(&peak))).await?;
            let second = task::run(run_batch(Arc::clone(&active), Arc::clone(&peak))).await?;
            task::wait(first).await?;
            task::wait(second).await?;
            assert_eq!(peak.load(Ordering::SeqCst), 2);
            Ok(())
        },
    )
    .unwrap();
}
