mod support;

use dever_runtime::concurrent;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[test]
#[ignore = "explicit bounded local scheduling benchmark"]
fn parallel_task_granularity_measurement() {
    use std::time::Instant;
    let values: Vec<usize> = (0..32_768).collect();
    for rounds in [0, 512] {
        for sample in 0..6 {
            let total = AtomicUsize::new(0);
            let apply = |value: usize| {
                let mut result = value;
                for _ in 0..rounds {
                    result =
                        std::hint::black_box(result.wrapping_mul(1664525).wrapping_add(1013904223));
                }
                total.fetch_add(result, Ordering::Relaxed);
                Ok(())
            };
            let start = Instant::now();
            concurrent::each(values.iter().copied(), 4, apply).unwrap();
            let parallel = start.elapsed();
            let expected = total.swap(0, Ordering::Relaxed);
            let start = Instant::now();
            concurrent::each_slice(&values, 4, |value| apply(*value)).unwrap();
            let batched = start.elapsed();
            assert_eq!(total.swap(0, Ordering::Relaxed), expected);
            let start = Instant::now();
            for value in &values {
                apply(*value).unwrap();
            }
            let serial = start.elapsed();
            assert_eq!(total.load(Ordering::Relaxed), expected);
            println!(
                "rounds={rounds} sample={sample} queue_ns={} batch_ns={} serial_ns={}",
                parallel.as_nanos(),
                batched.as_nanos(),
                serial.as_nanos()
            );
        }
    }
}

#[test]
fn batched_memory_actions_visit_once_and_stop_within_a_batch_on_failure() {
    let values: Vec<_> = (0..4096).collect();
    let seen: Vec<_> = values.iter().map(|_| AtomicUsize::new(0)).collect();
    concurrent::each_slice(&values, 4, |value| {
        seen[*value].fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    assert!(seen.iter().all(|count| count.load(Ordering::SeqCst) == 1));
    let calls = AtomicUsize::new(0);
    let failed = concurrent::each_slice(&values, 1, |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Err("expected".into())
    });
    assert_eq!(failed, Err("expected".into()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        concurrent::each_slice(&values, 0, |_| panic!("invalid workers invoked action")).is_err()
    );

    let calls = AtomicUsize::new(0);
    concurrent::each_slice(&[1], 256, |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn bounded_workers_overlap_and_all_finish_before_returning() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let releases = Mutex::new(releases);
    let finished = Arc::new(AtomicUsize::new(0));
    let completed = Arc::clone(&finished);
    let job = std::thread::spawn(move || {
        concurrent::each(0..2, 2, |_| {
            started.send(()).unwrap();
            releases
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .map_err(|error| error.to_string())?;
            completed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    });
    let first = starts.recv_timeout(Duration::from_secs(2));
    let second = starts.recv_timeout(Duration::from_secs(2));
    let _ = release.send(());
    let _ = release.send(());
    let result = job.join().unwrap();
    assert!(
        first.is_ok() && second.is_ok(),
        "both workers must start before either is released"
    );
    result.unwrap();
    assert_eq!(finished.load(Ordering::SeqCst), 2);
}

#[test]
fn invalid_limits_do_not_pull_and_failures_stop_dispatch_without_deadlock() {
    for limit in [-1, 0, 257, i64::MAX] {
        let pulled = AtomicUsize::new(0);
        let values = std::iter::from_fn(|| {
            pulled.fetch_add(1, Ordering::SeqCst);
            Some(1)
        });
        assert!(concurrent::each(values, limit, |_| Ok(())).is_err());
        assert_eq!(pulled.load(Ordering::SeqCst), 0);
    }
    let called = AtomicUsize::new(0);
    let result = concurrent::each(0..100, 1, |_| {
        called.fetch_add(1, Ordering::SeqCst);
        Err("expected failure".into())
    });
    assert_eq!(result, Err("expected failure".into()));
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[test]
fn native_parallel_handlers_are_static_and_join_before_following_statements() {
    let output = support::stdout(
        r#"show(value: Int, prefix: Text) () { dever.io.println(prefix + int.to_text(value)) }
invoke(route: handler(value: Int, prefix: Text) (), values: List<Int>) () {
  parallel_each(route, values, 2, "value=")
}
public main() () {
  invoke(show, [1, 2, 3])
  parallel_each(show, dever.bytes.from_text("ab"), 2, "byte=")
  dever.io.println("joined")
}
"#,
    );
    let mut lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.pop(), Some("joined"));
    lines.sort_unstable();
    assert_eq!(
        lines,
        ["byte=97", "byte=98", "value=1", "value=2", "value=3"]
    );
}

#[test]
fn parallel_checker_rejects_outputs_and_shared_stream_inputs() {
    support::rejected(
        "f(value: Int) (out: Int) { out = value }\npublic main() () { parallel_each(f, [1], 2) }",
        "zero-output handler",
    );
    support::rejected(
        "public type Box { stream: Stream<Int> }\nf(value: Box) () {}\npublic run(values: List<Box>) () { parallel_each(f, values, 2) }",
        "cannot contain Stream",
    );
    let failed = support::run("f(value: Int) () {}\npublic main() () { parallel_each(f, [1], 0) }");
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr)
            .contains("parallel worker count must be between 1 and 256")
    );
}
