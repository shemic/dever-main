use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use dever_runtime::{
    async_stream::AsyncStream, bytes::Bytes, channel::Channel, net, resource::Pull, task,
};
use futures_util::stream;

fn run_test(test: impl Future<Output = Result<(), String>>) {
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 1,
            max_blocking_threads: 2,
            task_capacity: 32,
        },
        async {
            tokio::time::timeout(Duration::from_secs(5), test)
                .await
                .map_err(|_| "async network test exceeded its deadline".to_owned())?
        },
    )
    .unwrap();
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn closing_an_idle_stream_wakes_pull_and_drops_the_producer() {
    run_test(async {
        let entered = Channel::new(1)?;
        let ready = entered.clone();
        let dropped = Arc::new(AtomicBool::new(false));
        let marker = Dropped(dropped.clone());
        let stream = AsyncStream::<i64>::new(stream::unfold(marker, move |marker| {
            let ready = ready.clone();
            async move {
                ready.send(()).await.unwrap();
                std::future::pending::<()>().await;
                Some((Pull::Item(1), marker))
            }
        }));
        let alias = stream.clone();
        let reader = task::run(async move { Ok(alias.pull().await) }).await?;
        entered.receive().await?;
        stream.close();
        stream.close();
        assert_eq!(task::wait(reader).await?, None);
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(stream.pull().await, None);
        Ok(())
    });
}

#[test]
fn mapped_stream_shares_cursor_and_emits_terminal_failure_once() {
    run_test(async {
        let stream = AsyncStream::new(stream::iter([
            Pull::Item(Ok(7)),
            Pull::Last(Err("failed")),
            Pull::Item(Ok(99)),
        ]))
        .map(|value| value.map(|value| value * 2));
        let alias = stream.clone();
        assert_eq!(stream.pull().await, Some(Ok(14)));
        assert_eq!(alias.pull().await, Some(Err("failed")));
        assert_eq!(stream.pull().await, None);
        Ok(())
    });
}

#[test]
fn stream_parallelism_applies_backpressure_before_pull_and_stop_drains_children() {
    run_test(async {
        let pulls = Arc::new(AtomicUsize::new(0));
        let observed = pulls.clone();
        let source_dropped = Arc::new(AtomicBool::new(false));
        let marker = Dropped(source_dropped.clone());
        let stream = AsyncStream::new(stream::unfold(marker, move |marker| {
            observed.fetch_add(1, Ordering::SeqCst);
            async move { Some((Pull::Item(1_i64), marker)) }
        }));
        let alias = stream.clone();
        let started = Channel::new(1)?;
        let notify = started.clone();
        let child_dropped = Arc::new(AtomicBool::new(false));
        let observed_child = child_dropped.clone();
        let consumer = task::run(async move {
            task::parallel_each_stream(stream, 1, move |_| {
                let notify = notify.clone();
                let marker = Dropped(observed_child.clone());
                async move {
                    let _marker = marker;
                    notify.send(()).await?;
                    std::future::pending::<()>().await;
                    Ok(())
                }
            })
            .await
        })
        .await?;
        started.receive().await?;
        assert_eq!(pulls.load(Ordering::SeqCst), 1);
        task::stop(consumer).await?;
        assert!(source_dropped.load(Ordering::SeqCst));
        assert!(child_dropped.load(Ordering::SeqCst));
        assert_eq!(alias.pull().await, None);
        Ok(())
    });
}

#[test]
fn handler_failure_interrupts_an_idle_producer() {
    run_test(async {
        let idle = Channel::new(1)?;
        let notify_idle = idle.clone();
        let dropped = Arc::new(AtomicBool::new(false));
        let stream = AsyncStream::new(stream::unfold(
            (false, Dropped(dropped.clone())),
            move |(sent, marker)| {
                let notify_idle = notify_idle.clone();
                async move {
                    if sent {
                        notify_idle.send(()).await.unwrap();
                        std::future::pending::<()>().await;
                    }
                    Some((Pull::Item(1_i64), (true, marker)))
                }
            },
        ));
        let result = task::parallel_each_stream(stream, 2, move |_| {
            let idle = idle.clone();
            async move {
                idle.receive().await?;
                Err("expected handler failure".into())
            }
        })
        .await;
        assert_eq!(result.unwrap_err(), "expected handler failure");
        assert!(dropped.load(Ordering::SeqCst));
        Ok(())
    });
}

async fn pair() -> Result<(net::Socket, net::Socket), String> {
    let listener = net::listen("127.0.0.1", 0).await?;
    let client = net::connect_timeout("127.0.0.1", net::port(&listener)?, 1000).await?;
    let server = net::accept(&listener).await?;
    net::timeout(&client, 1000)?;
    net::timeout(&server, 1000)?;
    Ok((client, server))
}

#[test]
fn tcp_duplex_progresses_with_one_worker_and_pending_read() {
    run_test(async {
        let (client, server) = pair().await?;
        let reading = client.clone();
        let reader = task::run(async move { net::read(&reading, 1).await }).await?;
        // A read may be pending on the same Socket while its write completes.
        net::write(&client, &Bytes::from_text("x")).await?;
        assert_eq!(net::read(&server, 1).await?.unwrap().to_text()?, "x");
        net::write(&server, &Bytes::from_text("y")).await?;
        assert_eq!(task::wait(reader).await?.unwrap().to_text()?, "y");
        let chunks = net::chunks(client.clone(), 1)?;
        chunks.close();
        // Closing a stream releases its alias; the explicitly held socket stays usable.
        net::write(&client, &Bytes::from_text("z")).await?;
        assert_eq!(net::read(&server, 1).await?.unwrap().to_text()?, "z");
        client.close()?;
        assert!(net::write(&client, &Bytes::from_text("x")).await.is_err());
        assert_eq!(net::read(&server, 1).await?, None);
        Ok(())
    });
}

#[test]
fn closing_resources_wakes_pending_accepts_and_reads() {
    run_test(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let accepting = listener.clone();
        let accept = task::run(async move { net::accept(&accepting).await }).await?;
        let (client, server) = pair().await?;
        let reading = server.clone();
        let read = task::run(async move { net::read(&reading, 1).await }).await?;
        tokio::task::yield_now().await;
        listener.close()?;
        server.close()?;
        assert_eq!(task::wait(accept).await.unwrap_err(), "resource is closed");
        assert_eq!(task::wait(read).await.unwrap_err(), "resource is closed");
        assert_eq!(net::read(&client, 1).await?, None);
        Ok(())
    });
}

#[test]
fn read_timeout_does_not_close_or_consume_the_next_message() {
    run_test(async {
        let (client, server) = pair().await?;
        net::timeout(&server, 10)?;
        assert_eq!(
            net::read(&server, 1).await.unwrap_err(),
            "network operation timed out"
        );
        net::timeout(&server, 1000)?;
        net::write(&client, &Bytes::from_text("x")).await?;
        assert_eq!(net::read(&server, 1).await?.unwrap().to_text()?, "x");
        assert!(net::read(&server, 0).await.is_err());
        assert!(net::connect_timeout("localhost", 0, 1).await.is_err());
        assert!(net::connect("127.0.0.1", -1).await.is_err());
        Ok(())
    });
}

#[test]
fn cancelling_a_partial_write_closes_socket_before_another_write() {
    run_test(async {
        let (client, server) = pair().await?;
        // More than a loopback socket's send buffer, with no draining reader.
        let sending = client.clone();
        let writer =
            task::run(
                async move { net::write(&sending, &Bytes::new(vec![7; 16 * 1024 * 1024])).await },
            )
            .await?;
        assert_eq!(net::read(&server, 1).await?.unwrap().values(), &[7]);
        task::stop(writer).await?;
        assert_eq!(
            net::write(&client, &Bytes::from_text("next"))
                .await
                .unwrap_err(),
            "resource is closed"
        );
        Ok(())
    });
}

#[test]
fn stopped_connection_consumer_releases_its_listener() {
    run_test(async {
        let listener = net::listen("127.0.0.1", 0).await?;
        let port = net::port(&listener)?;
        let stream = net::connections(listener);
        let consumer =
            task::run(
                async move { task::parallel_each_stream(stream, 2, |_| async { Ok(()) }).await },
            )
            .await?;
        tokio::task::yield_now().await;
        task::stop(consumer).await?;
        let rebound = net::listen("127.0.0.1", port).await?;
        assert_eq!(net::port(&rebound)?, port);
        let stream = net::connections(rebound.clone());
        rebound.close()?;
        assert!(stream.pull().await.unwrap().is_err());
        assert!(stream.pull().await.is_none());
        Ok(())
    });
}
