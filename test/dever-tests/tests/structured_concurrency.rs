mod support;

use dever_core::source::SourceMap;
use dever_core::syntax::{Declaration, ExpressionKind, InputKind};
use support::{checked, rejected, sources};

fn formatted(source: &str) -> String {
    let sources = sources(source);
    dever_core::format::format(&sources.files()[0]).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    })
}

#[test]
fn calls_and_handlers_need_no_async_syntax() {
    let source =
        "public main(route: handler(value: Int) (answer: Int)) (answer: Int) { answer = route(1) }";
    let package = dever_core::parse(&sources(source).files()[0]).unwrap();
    let Declaration::Function(function) = &package.declarations[0] else {
        panic!("expected function");
    };
    assert!(matches!(function.inputs[0].kind, InputKind::Handler(_)));
    let dever_core::syntax::StatementKind::Assign { value, .. } = &function.body[0].kind else {
        panic!("expected assignment");
    };
    assert!(matches!(value.kind, ExpressionKind::Call { .. }));

    let once = formatted(source);
    assert!(!once.contains("async "));
    assert!(!once.contains("await("));
    assert_eq!(formatted(&once), once);
}

#[test]
fn run_wait_groups_parallel_blocking_and_channels_check_together() {
    let source = r#"pure_double(value: Int) (answer: Int) pure { answer = value * 2 }
pause() (result: dever.time.SleepResult) { result = result(dever.time.sleep(1)) }
value(value: Int) (answer: Int) { answer = value }
work() () {}
public main() (answer: Int, received: Int?, slept: dever.time.SleepResult) {
  direct = value(20)
  task = run(value(21))
  answer = wait(task) + parallel(pure_double(direct))
  tasks = group(2)
  run(tasks, work())
  wait(tasks)
  channel = channel(Int, 2)
  send(channel, answer)
  received = receive(channel)
  close(channel)
  slept = blocking(pause())
}"#;
    let snapshot = checked(&sources(source)).api_snapshot();
    assert!(snapshot.contains("function main.main"));
    assert!(!snapshot.contains("async="));
    assert!(
        snapshot.contains("effects=[concurrency, time]"),
        "{snapshot}"
    );
}

#[test]
fn suspension_is_inferred_through_direct_calls() {
    let source = r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {}
work() () { sleep_ok(result(dever.task.sleep(0))) }
public main() () { work() }"#;
    let snapshot = checked(&sources(source)).api_snapshot();
    assert!(
        snapshot.contains("effects=[concurrency, time]"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("async="));

    rejected("async main() () {}", "'(' after a function name");
    rejected(
        "work() () {}\nmain() () { await(work()) }",
        "unknown function 'await'",
    );
}

#[test]
fn handler_suspension_reaches_the_call_graph() {
    let source = r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {}
route(value: Int) (answer: Int) {
  sleep_ok(result(dever.task.sleep(0)))
  answer = value
}
apply(route: handler(value: Int) (answer: Int)) (answer: Int) { answer = route(1) }
public main() (answer: Int) { answer = apply(route) }"#;
    let snapshot = checked(&sources(source)).api_snapshot();
    assert!(
        snapshot.contains("effects=[concurrency, time]"),
        "{snapshot}"
    );

    rejected(
        "route(value: Text) (answer: Int) { answer = 1 }\napply(route: handler(value: Int) (answer: Int)) (answer: Int) { answer = route(1) }\nmain() (answer: Int) { answer = apply(route) }",
        "handler signature does not match",
    );
    rejected("cycle() () { cycle() }", "recursive function");
}

#[test]
fn tasks_and_groups_are_affine() {
    checked(&sources(
        "work() () {}\npublic main() () { task = run(work())\ntasks = group(1)\nwork()\nstop(task)\nstop(tasks) }",
    ));
    for (body, message) in [
        ("task = run(work())", "must be consumed"),
        (
            "task = run(work())\nwait(task)\nwait(task)",
            "already been consumed",
        ),
        (
            "task = run(work())\ntask = run(work())\nstop(task)",
            "consumed before it can be overwritten",
        ),
        ("task = run(work())\nvalues = [task]", "is affine"),
        (
            "tasks = [run(work())]\nvalue = first(tasks)\nstop(value)",
            "cannot be stored in another value",
        ),
    ] {
        rejected(&format!("work() () {{}}\nmain() () {{ {body} }}"), message);
    }
    rejected(
        "value() (answer: Int) { answer = 1 }\nmain() () { task = run(value())\nstop(task) }",
        "only a zero-output Task",
    );
    rejected(
        "value() (answer: Int) { answer = 1 }\nmain() () { tasks = group(1)\nrun(tasks, value())\nwait(tasks) }",
        "only zero-output calls",
    );
}

#[test]
fn non_transferable_locals_cannot_cross_suspension_points() {
    for (suspension, expression) in [
        ("work()", "work()"),
        (
            "parallel_each(visit, [1], 1)",
            "parallel_each(visit, [1], 1)",
        ),
        (
            "sleep_ok(result(dever.task.sleep(0)))",
            "result(dever.task.sleep(0))",
        ),
    ] {
        let source = format!(
            r#"observe(value: Stream<Int>) () {{}}
visit(value: Int) () {{}}
sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {{}}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {{}}
work() () {{ sleep_ok(result(dever.task.sleep(0))) }}
main(stream: Stream<Int>) () {{
  {suspension}
  observe(stream)
}}"#
        );
        let source_map = sources(&source);
        let errors = dever_core::check(&source_map).expect_err("non-transferable live stream");
        let start = source.rfind(expression).unwrap();
        assert!(
            errors.iter().any(|error| {
                error.message == "Stream<Int> local cannot remain live across a suspension point"
                    && error.primary.start == start
                    && error.primary.end == start + expression.len()
            }),
            "suspension {suspension}: {errors:?}"
        );
    }
    checked(&sources(
        r#"type Outcome { Done
error Failed }
fail_work() () { fail(Outcome.Failed) }
observe(value: Stream<Int>) () {}
main(stream: Stream<Int>) (outcome: Outcome) {
  outcome = result(fail_work())
  observe(stream)
}"#,
    ));
}

#[test]
fn channel_types_capacity_and_operations_are_checked() {
    checked(&sources(
        "public main() (received: Int?) { channel = channel(Int, 1)\nsend(channel, 7)\nreceived = receive(channel)\nclose(channel)\nclose(channel) }",
    ));
    checked(&sources(
        "main(channel: Channel<Int>) () { close(channel) }",
    ));
    for (source, message) in [
        (
            "main() () { channel = channel(Stream<Int>, 1)\nclose(channel) }",
            "item type must be transferable",
        ),
        (
            "main() () { channel = channel(Int, 0)\nclose(channel) }",
            "channel capacity must be between",
        ),
        (
            "main() () { channel = channel(Int, 65537)\nclose(channel) }",
            "channel capacity must be between",
        ),
        (
            "main() () { channel = channel(Int, 1)\nsend(channel, \"wrong\")\nclose(channel) }",
            "expected Int",
        ),
    ] {
        rejected(source, message);
    }
}

#[test]
fn group_limits_parallel_purity_and_blocking_effects_are_checked() {
    for limit in ["0", "65537"] {
        rejected(
            &format!("main() () {{ tasks = group({limit})\nwait(tasks) }}"),
            "group limit must be between",
        );
    }
    rejected(
        "noisy() (answer: Int) { dever.io.println(\"x\")\nanswer = 1 }\nmain() (answer: Int) { answer = parallel(noisy()) }",
        "pure function call",
    );
    rejected(
        r#"visit(value: Int) () {}
pause_all(values: List<Int>) (result: dever.time.SleepResult) {
  parallel_each(visit, values, 2)
  result = result(dever.time.sleep(0))
}
main() (result: dever.time.SleepResult) { result = blocking(pause_all([1])) }"#,
        "blocking cannot wrap a call that uses parallel_each",
    );
    rejected(
        "plain() (answer: Int) pure { answer = 1 }\nmain() (answer: Int) { answer = blocking(plain()) }",
        "transitive blocking system effect",
    );
}

#[test]
fn collection_handlers_preserve_suspension_and_worker_boundaries() {
    let source = r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {}
visit(value: Int) () { sleep_ok(result(dever.task.sleep(value))) }
public main() () { each(visit, [1]) }"#;
    let snapshot = checked(&sources(source)).api_snapshot();
    assert!(
        snapshot.contains("effects=[concurrency, time]"),
        "{snapshot}"
    );

    checked(&sources("apply(route: handler() ()) () { route() }"));
    checked(&sources(
        "apply(route: handler(value: Int) ()) () { each(route, [1]) }",
    ));
    checked(&sources(
        "main() () { dever.io.println(\"blocking output\") }",
    ));
    for blocking_call in [
        "dever.io.println(\"blocking output\")",
        "sleep_ok(result(dever.time.sleep(0)))",
    ] {
        let source = format!(
            r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {{}}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {{}}
main() () {{
  sleep_ok(result(dever.task.sleep(0)))
  {blocking_call}
}}"#
        );
        rejected(&source, "cannot run inline inside a suspending function");
    }
    checked(&sources(
        r#"sleep_ok(result: dever.time.SleepResult.Done) () recover("timer failure handled") {}
sleep_ok(result: dever.time.SleepResult.Failed(message)) () recover("timer failure handled") {}
main() () {
  sleep_ok(result(dever.task.sleep(0)))
  blocking(dever.io.println("bounded output"))
}"#,
    ));
}

#[test]
fn task_and_channel_boundaries_preserve_failure_obligations() {
    let declarations = "type Result { Ready\nerror Failed }\nfail_work() () { fail(Result.Failed) }\nproduce_failure() (result: Result) { result = result(fail_work()) }\n";
    rejected(
        &format!(
            "{declarations}main() () {{ task = run(produce_failure())\nresult = wait(task) }}"
        ),
        "failure is not handled",
    );
    checked(&sources(&format!(
        "{declarations}main() (result: Result) {{ task = run(produce_failure())\nresult = wait(task) }}"
    )));
    rejected(
        &format!(
            "{declarations}main() () {{ channel = channel(Result, 1)\nsend(channel, produce_failure())\nreceived = receive(channel)\nclose(channel) }}"
        ),
        "failure is not handled",
    );
}

#[test]
fn api_and_markdown_contracts_hide_internal_suspension() {
    let plain = checked(&sources(
        "public apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }",
    ));
    let snapshot = plain.api_snapshot();
    assert!(!snapshot.contains("async="), "{snapshot}");
    assert!(!snapshot.contains("async handler("), "{snapshot}");

    let document = r#"# Application package

Documents a package.

- 包：`main`
- 公开类型：无
- 公开方法：
  - `main`
- 使用：
  - `main.main()`

## Main

Documents the entry.

- 函数：`main`
- 输入：无
- 输出：无

```dever
public main() () {}
```
"#;
    let mut markdown = SourceMap::default();
    markdown.add("main.dever.md", document);
    let program = checked(&markdown);
    assert!(!program.api_snapshot().contains("async="));
    let formatted = dever_core::format::format(&markdown.files()[0]).unwrap();
    assert!(formatted.contains("main() () {}"));
    assert!(!formatted.contains("async main"));
}
