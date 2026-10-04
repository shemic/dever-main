mod support;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use dever_runtime::{channel::Channel, task};

fn run(future: impl std::future::Future<Output = Result<(), String>>) {
    task::run_entry_with(
        task::RuntimeConfig {
            worker_threads: 1,
            max_blocking_threads: 2,
            task_capacity: 32,
        },
        future,
    )
    .unwrap();
}

#[test]
fn source_runtime_configuration_validates_limits_and_starts_once() {
    assert!(task::configured(0, 1, 8).is_err());
    assert!(task::configured(1, -1, 8).is_err());
    assert!(task::configured(1, 1, 65537).is_err());
    let config = task::configured(1, 2, 8).unwrap();
    task::start_entry(config, async { Ok(()) }).unwrap();
    assert!(
        task::start_entry(config, async { Ok(()) })
            .unwrap_err()
            .contains("only be started once")
    );
}

#[test]
fn timeout_drains_started_blocking_children_before_returning() {
    run(async {
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let signal = started.clone();
        let completed = finished.clone();
        let job = task::run(async move {
            task::blocking(move || {
                signal.add_permits(1);
                std::thread::sleep(Duration::from_millis(40));
                completed.store(true, Ordering::Release);
                Ok(17)
            })
            .await
        })
        .await?;
        started.acquire().await.unwrap().forget();
        assert_eq!(task::wait_timeout(job, 1).await?, None);
        assert!(finished.load(Ordering::Acquire));
        Ok(())
    });
}

#[test]
fn race_stops_losers_and_preserves_program_faults() {
    run(async {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let ended = Arc::new(AtomicBool::new(false));
        let owner = Dropped(ended.clone());
        let slow = task::run(async move {
            let _owner = owner;
            std::future::pending::<Result<i64, String>>().await
        })
        .await?;
        let fast = task::run(async { Ok(42) }).await?;
        assert_eq!(task::race(vec![slow, fast]).await?, 42);
        assert!(ended.load(Ordering::Acquire));
        let failed = task::run(async { Err::<i64, _>("original fault".into()) }).await?;
        let pending = task::run(std::future::pending::<Result<i64, String>>()).await?;
        assert_eq!(
            task::race(vec![failed, pending]).await.unwrap_err(),
            "original fault"
        );
        let failed = task::run(async { Err::<(), _>("timer fault".into()) }).await?;
        assert_eq!(
            task::wait_timeout(failed, 100).await.unwrap_err(),
            "timer fault"
        );
        Ok(())
    });
}

#[test]
fn channel_stream_drains_close_without_closing_independent_aliases() {
    run(async {
        let channel = Channel::new(2).unwrap();
        channel.send(Some(1)).await?;
        channel.send(None).await?;
        let stream = channel.clone().stream();
        assert_eq!(stream.pull().await, Some(Some(1)));
        stream.close();
        assert_eq!(channel.receive().await?, Some(None));
        channel.send(Some(2)).await?;
        channel.close().await?;
        let stream = channel.stream();
        assert_eq!(stream.pull().await, Some(Some(2)));
        assert_eq!(stream.pull().await, None);
        Ok(())
    });
}

#[test]
fn ticks_delay_first_value_skip_missed_periods_and_close_pending_pull() {
    run(async {
        let ticks = task::ticks(20)?;
        assert!(
            tokio::time::timeout(Duration::from_millis(2), ticks.pull())
                .await
                .is_err()
        );
        assert_eq!(ticks.pull().await, Some(1));
        tokio::time::sleep(Duration::from_millis(70)).await;
        assert_eq!(ticks.pull().await, Some(2));
        assert!(
            tokio::time::timeout(Duration::from_millis(2), ticks.pull())
                .await
                .is_err()
        );
        let waiter = ticks.clone();
        let pending = task::run(async move { Ok(waiter.pull().await) }).await?;
        ticks.close();
        assert_eq!(task::wait(pending).await?, None);
        assert!(task::ticks(0).is_err());
        Ok(())
    });
}

#[test]
fn static_composition_rejects_wrong_shapes_and_reused_tasks() {
    support::rejected(
        "work() (value: Int) { value = 1 }\nfallback() (value: Text) { value = \"wrong\" }\nmain() () { job = run(work())\nvalue = timeout(job, 1, fallback) }",
        "signature",
    );
    support::rejected(
        "work() (value: Int) { value = 1 }\nmain() () { job = run(work())\nvalue = race(job, job) }",
        "consumed",
    );
}

#[test]
fn native_composition_and_suspending_memory_handlers() {
    let source = r#"delayed(value: Int, millis: Int) (answer: Int) recover("test timer") {
  dever.task.sleep(millis)
  answer = value
}
fallback() (answer: Int) { answer = 9 }
collect(value: Int, output: Channel<Int>) () { send(output, value) }
add(value: Int, state: Int) (result: Int) { result = state + value }
identity(value: Int) (result: Int) { result = value }
pair(millis: Int) (number: Int, text: Text) recover("test timer") {
  dever.task.sleep(millis)
  number = 8
  text = "pair"
}
pair_timeout() (number: Int, text: Text) { number = 0
  text = "timeout" }
worker() () {
  slow = run(delayed(1, 1000))
  timed = timeout(slow, 1, fallback)
  first = run(delayed(2, 1000))
  second = run(delayed(3, 1))
  fastest = race(first, second)
  values = each(add, [1, 2], 10)
  total = reduce(add, [1, 2, 3], 0)
  mapped = each(identity, [7, 8])
  mapped_total = sum(mapped)
  pair_task = run(pair(1000))
  pair_value = timeout(pair_task, 1, pair_timeout)
  pair_text = pair_value.text
  channel = channel(Int, 4)
  each(collect, [4, 5], channel)
  parallel_each(collect, dever.bytes.from_text("ab"), 2, channel)
  close(channel)
  received = reduce(add, stream(channel), 0)
  timer = dever.task.ticks(10)
  close(timer)
  blocking(dever.io.println(int.to_text(timed)))
  blocking(dever.io.println(int.to_text(fastest)))
  blocking(dever.io.println(int.to_text(sum(values))))
  blocking(dever.io.println(int.to_text(total)))
  blocking(dever.io.println(int.to_text(received)))
  blocking(dever.io.println(int.to_text(mapped_total)))
  blocking(dever.io.println(pair_text))
}
public main() () {
  worker()
}"#;
    assert_eq!(support::stdout(source), "9\n3\n23\n6\n204\n15\ntimeout\n");
}
