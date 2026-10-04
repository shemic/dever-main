mod support;

use support::{checked, sources, stdout};

#[test]
fn async_program_compiles_and_runs_every_execution_class() {
    let output = stdout(
        r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("test handles timer failures") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("test handles timer failures") {}
pause() (result: dever.time.SleepResult) { result = result(dever.time.sleep(1)) }
double(value: Int) (answer: Int) pure { answer = value * 2 }
delayed(value: Int, millis: Int) (answer: Int) {
  sleep_ok(result(dever.task.sleep(millis)))
  answer = value
}
send_value(channel: Channel<Int>, value: Int) () {
  send(channel, value)
}
wait_until_stopped() () {
  sleep_ok(result(dever.task.sleep(1000)))
}
public main() (direct: Int, first: Int, second: Int, received: Int?, grouped: Int?, computed: Int) {
  direct = delayed(1, 1)
  first_task = run(delayed(2, 10))
  second_task = run(delayed(3, 1))
  second = wait(second_task)
  first = wait(first_task)

  channel = channel(Int, 2)
  sender = run(send_value(channel, 4))
  received = receive(channel)
  wait(sender)

  workers = group(2)
  run(workers, send_value(channel, 5))
  wait(workers)
  grouped = receive(channel)
  close(channel)

  cancelled = run(wait_until_stopped())
  stop(cancelled)
  computed = parallel(double(21))
  sleep_ok(blocking(pause()))
}"#,
    );

    assert_eq!(
        output,
        "direct = 1\nfirst = 2\nsecond = 3\nreceived = 4\ngrouped = 5\ncomputed = 42\n"
    );
}

#[test]
fn only_inferred_suspending_entries_start_the_shared_runtime() {
    let sync_sources = sources("public main() (answer: Int) { answer = 1 }");
    let sync =
        dever_core::native::emit(&checked(&sync_sources), &sync_sources, "main.main").unwrap();
    assert!(sync.contains("match entry()"), "{sync}");
    assert!(!sync.contains("task::run_entry_typed(entry())"), "{sync}");

    let async_sources = sources(
        r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {}
public main() () { sleep_ok(result(dever.task.sleep(0))) }"#,
    );
    let asynchronous =
        dever_core::native::emit(&checked(&async_sources), &async_sources, "main.main").unwrap();
    assert!(
        asynchronous.contains("match task::run_entry_typed(entry())"),
        "{asynchronous}"
    );
}

#[test]
fn timer_suspension_is_inferred_without_a_source_modifier() {
    let program_sources = sources(
        "public main() (result: dever.time.SleepResult) { result = dever.system.task_sleep(1) }",
    );
    let generated =
        dever_core::native::emit(&checked(&program_sources), &program_sources, "main.main")
            .unwrap();
    assert!(generated.contains("async fn"), "{generated}");
    assert!(
        generated.contains("task::run_entry_typed(entry())"),
        "{generated}"
    );
}

#[test]
fn async_parallel_each_runs_outside_the_runtime_worker() {
    let source = r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {}
visit(value: Int) () {}
public main() (answer: Int) {
  sleep_ok(result(dever.task.sleep(0)))
  parallel_each(visit, [1, 2, 3], 2)
  parallel_each(visit, dever.bytes.from_text("ab"), 2)
  answer = 7
}"#;
    let program_sources = sources(source);
    let generated =
        dever_core::native::emit(&checked(&program_sources), &program_sources, "main.main")
            .unwrap();
    assert!(
        generated.contains("task::parallel_each_typed(a0.into_values()"),
        "{generated}"
    );
    assert_eq!(stdout(source), "answer = 7\n");
}

#[test]
fn nullable_channel_items_are_flattened_at_the_language_boundary() {
    assert_eq!(
        stdout(
            "public main() (received: Int?) { channel = channel(Int?, 1)\nsend(channel, null)\nreceived = receive(channel)\nclose(channel) }",
        ),
        "received = null\n"
    );
}

#[test]
fn inferred_suspension_requires_explicit_transitive_blocking_boundaries() {
    support::rejected(
        r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {}
pause() (result: dever.time.SleepResult) { result = result(dever.time.sleep(1)) }
public main() (result: dever.time.SleepResult) {
  sleep_ok(result(dever.task.sleep(0)))
  result = pause()
}"#,
        "cannot run inline inside a suspending function",
    );
    support::rejected(
        r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {}
visit(value: Int) () {}
visit_all(values: List<Int>) () { parallel_each(visit, values, 2) }
public main() () {
  sleep_ok(result(dever.task.sleep(0)))
  visit_all([1, 2, 3])
}"#,
        "cannot run inline inside a suspending function",
    );
    checked(&sources(
        r#"visit(value: Int) () {}
visit_all(values: Stream<Int>) () { parallel_each(visit, values, 2) }"#,
    ));
}
