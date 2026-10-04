//! Checked-source async acceptance against the real LLVM object and runtime archive.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm.rs"]
mod llvm;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm_managed.rs"]
mod llvm_managed;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::{llvm, llvm_managed};
    use dever_core::{check, source::SourceMap};
    use std::fmt::Write;

    const TIMER: &str = r#"slept(value: dever.time.SleepResult.Done) (okay: Bool) recover("test checks timer result") { okay = true }
slept(value: dever.time.SleepResult.Failed(message)) (okay: Bool) recover("test checks timer result") { okay = false }
"#;

    const NAMED_VALUES: &str = r#"type Bundle {
  label: Text
  names: List<Text>
  lookup: Map<Text, List<Text>>
  sequence: List<Int>
  first: Int
  second: Int
  third: Int
  fourth: Int
  fifth: Int
  sixth: Int
}
delayed(label: Text, names: List<Text>, lookup: Map<Text, List<Text>>) (bundle: Bundle, ready: Bool) {
  ready = slept(result(dever.task.sleep(1)))
  bundle = Bundle { label = label
    names = names
    lookup = lookup
    sequence = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    first = 11
    second = 12
    third = 13
    fourth = 14
    fifth = 15
    sixth = 16 }
}
public main() (answer: Bool) {
  value = delayed("owned", ["first", "second"], { "key" = ["inside"] })
  answer = value.ready and value.bundle.label == "owned" and value.bundle.names == ["first", "second"] and get(value.bundle.lookup, "key") == optional_names(["inside"]) and value.bundle.sequence == [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] and value.bundle.first == 11 and value.bundle.second == 12 and value.bundle.third == 13 and value.bundle.fourth == 14 and value.bundle.fifth == 15 and value.bundle.sixth == 16
}
optional_names(value: List<Text>) (answer: List<Text>?) { answer = value }
"#;

    const TASKS_AND_GROUPS: &str = r#"some_text(value: Text) (answer: Text?) { answer = value }
delayed(value: Text, names: List<Text>) (answer: Text) {
  ready = slept(result(dever.task.sleep(1)))
  answer = value + first_name(names) + suffix(ready)
}
first_name(names: List<Text>) (answer: Text) { answer = read_name(first(names)) }
read_name(value: Text) (answer: Text) { answer = value }
read_name(value: null) (answer: Text) { answer = "missing" }
suffix(ready: true) (answer: Text) { answer = "!" }
suffix(ready: false) (answer: Text) { answer = "bad" }
send_label(channel: Channel<Text>, label: Text) () { send(channel, label) }
public main() (answer: Bool) {
  task = run(delayed("A", ["B"]))
  result = wait(task)
  channel = channel(Text, 2)
  workers = group(1)
  run(workers, send_label(channel, "first"))
  run(workers, send_label(channel, "second"))
  wait(workers)
  first_value = receive(channel)
  second_value = receive(channel)
  close(channel)
  answer = result == "AB!" and first_value == some_text("first") and second_value == some_text("second")
}
"#;

    const NULLABLE_CHANNEL: &str = r#"some_int(value: Int) (answer: Int?) { answer = value }
send_later(channel: Channel<Int?>) () { send(channel, some_int(7)) }
public main() (answer: Bool) {
  channel = channel(Int?, 1)
  send(channel, null)
  sender = run(send_later(channel))
  ready = slept(result(dever.task.sleep(1)))
  first_value = receive(channel)
  wait(sender)
  close(channel)
  second_value = receive(channel)
  end = receive(channel)
  answer = ready and first_value == null and second_value == some_int(7) and end == null
}
"#;

    const TASK_BUSINESS_FAILURE: &str = r#"type Detail {
  message: Text
  history: List<Text>
}
type Outcome {
  Done(value: Text)
  error Failed(detail: Detail)
}
fail_job() (answer: Text) { fail(Outcome.Failed(Detail { message = "task failed"
  history = ["before", "task"] })) }
wrapper() (answer: Text) {
  task = run(fail_job())
  answer = wait(task)
}
inspect(value: Outcome.Done(text)) (answer: Bool) recover("test expects task failure") { answer = false }
inspect(value: Outcome.Failed(detail)) (answer: Bool) recover("test expects task failure") { answer = detail.message == "task failed" and detail.history == ["before", "task"] }
public main() (answer: Bool) { answer = inspect(result(wrapper())) }
"#;

    const CANCEL_AND_NESTED: &str = r#"type Outcome {
  Done
  error Failed(message: Text)
}
wait_long(value: Text) () { ready = slept(result(dever.task.sleep(1000))) }
parent() () {
  child = run(wait_long("descendant"))
  fail(Outcome.Failed("parent exit"))
}
inspect(value: Outcome.Done) (answer: Bool) recover("test expects parent failure") { answer = false }
inspect(value: Outcome.Failed(message)) (answer: Bool) recover("test expects parent failure") { answer = message == "parent exit" }
public main() (answer: Bool) {
  task = run(wait_long("task"))
  ready = slept(result(dever.task.sleep(1)))
  stop(task)
  workers = group(1)
  run(workers, wait_long("group"))
  stop(workers)
  answer = ready and inspect(result(parent()))
}
"#;

    const BLOCKING_AND_PARALLEL: &str = r#"double(value: Int) (answer: Int) pure { answer = value * 2 }
pause() (ready: Bool) { ready = slept(result(dever.time.sleep(1))) }
public main() (answer: Bool) {
  number = parallel(double(21))
  ready = blocking(pause())
  answer = number == 42 and ready
}
"#;

    const CAPTURE_THEN_REFAIL: &str = r#"type Outcome {
  Done(value: Text)
  error Failed(message: Text, history: List<Text>)
}
origin() (answer: Text) {
  ready = slept(result(dever.task.sleep(1)))
  fail(Outcome.Failed("origin", ["owned", "before"]))
}
rethrow(value: Outcome.Done(text)) (answer: Text) { answer = text }
rethrow(value: Outcome.Failed(message, history)) (answer: Text) {
  fail(Outcome.Failed(message + " again", append(history, "after")))
}
wrapper() (answer: Text) { answer = rethrow(result(origin())) }
inspect(value: Outcome.Done(text)) (answer: Bool) recover("test expects repeated failure") { answer = false }
inspect(value: Outcome.Failed(message, history)) (answer: Bool) recover("test expects repeated failure") { answer = message == "origin again" and history == ["owned", "before", "after"] }
public main() (answer: Bool) { answer = inspect(result(wrapper())) }
"#;

    const UNCAPTURED_RUNTIME_FAULT: &str = r#"divide(value: Int, denominator: Int) (answer: Int) pure { answer = value // denominator }
worker() (answer: Int) {
  ready = slept(result(dever.task.sleep(1)))
  answer = guard(parallel(divide(42, 0)))
}
type Outcome {
  Done(value: Int)
  error Failed(message: Text)
}
guard(value: 0) (answer: Int) { fail(Outcome.Failed("zero")) }
guard(value: other) (answer: Int) { answer = value }
inspect(value: Outcome.Done(number)) (answer: Bool) recover("test checks capture") { answer = number == 42 }
inspect(value: Outcome.Failed(message)) (answer: Bool) recover("test checks capture") { answer = false }
public main() (answer: Bool) { answer = inspect(result(worker())) }
"#;

    const BLOCKING_RUNTIME_FAULT: &str = r#"pause_divide(denominator: Int) (answer: Int) {
  ready = slept(result(dever.time.sleep(1)))
  answer = 42 // denominator
}
public main() (answer: Int) { answer = blocking(pause_divide(0)) }
"#;

    const DEFAULT_FAILURE_TRACE: &str = r#"type Outcome {
  Done
  error Failed(message: Text, history: List<Text>)
}
origin() () {
  ready = slept(result(dever.task.sleep(1)))
  fail(Outcome.Failed("origin", ["owned", "history"]))
}
middle() () { origin() }
public main() () { middle() }
"#;

    const ASYNC_STATIC_TRAVERSAL: &str = r#"label(value: Text, prefix: Text) (answer: Text) { answer = prefix + value }
collect(value: Text, state: List<Text>) (next: List<Text>) { next = append(state, value) }
add(value: Int, state: Int) (next: Int) { next = state + value }
public main() (answer: Bool) {
  ready = slept(result(dever.task.sleep(1)))
  labels = each(label, ["one", "two"], "item=")
  collected = reduce(collect, labels, ["seed"])
  bytes = dever.bytes.from_text("AB")
  total = reduce(add, bytes, 0)
  answer = ready and labels == ["item=one", "item=two"] and collected == ["seed", "item=one", "item=two"] and total == 131
}
"#;

    const SYNC_TASK_AND_GROUP: &str = r#"type Bundle {
  label: Text
  names: List<Text>
  lookup: Map<Text, List<Text>>
}
make(label: Text, names: List<Text>, lookup: Map<Text, List<Text>>) (text: Text, items: List<Text>, bundle: Bundle) {
  text = label + "!"
  items = append(names, "last")
  bundle = Bundle { label = label
    names = names
    lookup = lookup }
}
send_label(channel: Channel<Text>, label: Text) () { send(channel, label) }
some_text(value: Text) (answer: Text?) { answer = value }
public main() (answer: Bool) {
  channel = channel(Text, 1)
  workers = group(1)
  run(workers, send_label(channel, "group"))
  wait(workers)
  received = receive(channel)
  close(channel)
  task = run(make("sync", ["first"], { "key" = ["inside"] }))
  value = wait(task)
  answer = value.text == "sync!" and value.items == ["first", "last"] and value.bundle.label == "sync" and value.bundle.names == ["first"] and get(value.bundle.lookup, "key") == optional_names(["inside"]) and received == some_text("group")
}
optional_names(value: List<Text>) (answer: List<Text>?) { answer = value }
"#;

    const OWNED_BLOCKING_AND_PARALLEL: &str = r#"type Bundle {
  label: Text
  names: List<Text>
  lookup: Map<Text, List<Text>>
}
transform(label: Text, names: List<Text>, lookup: Map<Text, List<Text>>) (bundle: Bundle, names_again: List<Text>) pure {
  bundle = Bundle { label = label + "!"
    names = append(names, "tail")
    lookup = lookup }
  names_again = names
}
blocking_transform(label: Text, names: List<Text>, lookup: Map<Text, List<Text>>) (bundle: Bundle, names_again: List<Text>) {
  ready = slept(result(dever.time.sleep(1)))
  value = transform(label, names, lookup)
  bundle = value.bundle
  names_again = value.names_again
}
optional_names(value: List<Text>) (answer: List<Text>?) { answer = value }
check_blocking() (answer: Bool) {
  value = blocking(blocking_transform("block", ["a"], { "key" = ["b"] }))
  answer = value.bundle.label == "block!" and value.bundle.names == ["a", "tail"] and value.names_again == ["a"] and get(value.bundle.lookup, "key") == optional_names(["b"])
}
check_parallel() (answer: Bool) {
  value = parallel(transform("parallel", ["x"], { "key" = ["y"] }))
  answer = value.bundle.label == "parallel!" and value.bundle.names == ["x", "tail"] and value.names_again == ["x"] and get(value.bundle.lookup, "key") == optional_names(["y"])
}
public main() (answer: Bool) {
  ready = slept(result(dever.task.sleep(1)))
  background = check_blocking()
  compute = check_parallel()
  answer = ready and background and compute
}
"#;

    const BLOCKING_AND_PARALLEL_BUSINESS_FAILURE: &str = r#"type Detail {
  message: Text
  history: List<Text>
}
type Outcome {
  Done(value: Text)
  error Failed(detail: Detail)
}
fail_owned(label: Text, history: List<Text>) (answer: Text) pure {
  fail(Outcome.Failed(Detail { message = label
    history = append(history, "failed") }))
}
fail_owned_blocking(label: Text, history: List<Text>) (answer: Text) {
  ready = slept(result(dever.time.sleep(1)))
  answer = fail_owned(label, history)
}
blocking_job() (answer: Text) { answer = blocking(fail_owned_blocking("blocking", ["before"])) }
parallel_job() (answer: Text) { answer = parallel(fail_owned("parallel", ["before"])) }
inspect(value: Outcome.Done(text), expected: Text) (answer: Bool) recover("test expects business failure") { answer = false }
inspect(value: Outcome.Failed(detail), expected: Text) (answer: Bool) recover("test expects business failure") { answer = detail.message == expected and detail.history == ["before", "failed"] }
public main() (answer: Bool) {
  ready = slept(result(dever.task.sleep(1)))
  answer = ready and inspect(result(blocking_job()), "blocking") and inspect(result(parallel_job()), "parallel")
}
"#;

    const NEGATIVE_TASK_SLEEP: &str = r#"inspect(value: dever.time.SleepResult.Done) (answer: Bool) recover("test checks negative timer") { answer = false }
inspect(value: dever.time.SleepResult.Failed(message)) (answer: Bool) recover("test checks negative timer") { answer = message == "sleep duration must not be negative" }
public main() (answer: Bool) { answer = inspect(result(dever.task.sleep(-1))) }
"#;

    const SYNC_CHANNEL_ONLY: &str = r#"public main() (answer: Bool) {
  channel = channel(Int, 1)
  alias = channel
  answer = true
}
"#;

    fn source(body: &str) -> String {
        format!("{TIMER}\n{body}")
    }

    fn checked(source: &str) {
        let mut sources = SourceMap::default();
        sources.add("main.dever", source);
        check(&sources).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&sources))
                    .collect::<String>()
            )
        });
    }

    #[test]
    fn async_sources_pass_the_existing_checker() {
        for body in [
            NAMED_VALUES,
            TASKS_AND_GROUPS,
            NULLABLE_CHANNEL,
            TASK_BUSINESS_FAILURE,
            CANCEL_AND_NESTED,
            BLOCKING_AND_PARALLEL,
            CAPTURE_THEN_REFAIL,
            UNCAPTURED_RUNTIME_FAULT,
            BLOCKING_RUNTIME_FAULT,
            DEFAULT_FAILURE_TRACE,
            ASYNC_STATIC_TRAVERSAL,
            SYNC_TASK_AND_GROUP,
            OWNED_BLOCKING_AND_PARALLEL,
            BLOCKING_AND_PARALLEL_BUSINESS_FAILURE,
            NEGATIVE_TASK_SLEEP,
            SYNC_CHANNEL_ONLY,
        ] {
            checked(&source(body));
        }
    }

    #[test]
    fn task_spawn_rejects_nontransferable_sync_stream_at_source() {
        let mut sources = SourceMap::default();
        sources.add(
            "main.dever",
            "consume(stream: Stream<dever.io.ReadEvent>) () { close(stream) }\npublic main() () { file = dever.io.open(\"owned-test-file\")\nstream = dever.io.chunks(file, 2)\ntask = run(consume(stream))\nstop(task) }",
        );
        let errors = check(&sources).expect_err("Task must reject a synchronous Stream argument");
        assert!(errors.iter().any(|error| {
            let rendered = error.render(&sources);
            rendered.contains("main.dever:4:")
                && rendered.contains("task inputs and outputs must be transferable between threads")
        }));
    }

    #[test]
    fn suspending_source_rejects_inline_file_io_at_source() {
        let mut sources = SourceMap::default();
        sources.add(
            "main.dever",
            "slept(value: dever.time.SleepResult.Done) () recover(\"test timer\") {}\nslept(value: dever.time.SleepResult.Failed(message)) () recover(\"test timer\") {}\npublic main() (file: dever.system.File) {\nslept(result(dever.task.sleep(1)))\nfile = dever.io.open(\"owned-test-file\")\n}",
        );
        let errors =
            check(&sources).expect_err("inline file I/O must fail in a suspending function");
        assert!(errors.iter().any(|error| {
            let rendered = error.render(&sources);
            rendered.contains("main.dever:5:")
                && rendered.contains("a blocking or parallel operation cannot run inline inside a suspending function")
        }));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn named_async_call_preserves_nested_managed_output() {
        llvm_managed::boolean(&source(NAMED_VALUES));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn cloned_channel_fields_survive_their_source_scope() {
        llvm_managed::boolean(
            "type Pair { first: Channel<Int>\n  second: Channel<Int> }\n\
             shared() (pair: Pair) { pipe = channel(Int, 1)\n\
               pair = Pair { first = pipe\n  second = pipe } }\n\
             some_int(value: Int) (answer: Int?) { answer = value }\n\
             public main() (okay: Bool) { task = run(shared())\n  pair = wait(task)\n\
               send(pair.first, 7)\n  received = receive(pair.second)\n\
               close(pair.first)\n  close(pair.second)\n  okay = received == some_int(7) }",
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn task_wait_and_capacity_one_group_preserve_order() {
        llvm_managed::boolean(&source(TASKS_AND_GROUPS));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn nullable_channel_backpressure_close_and_drain() {
        llvm_managed::boolean(&source(NULLABLE_CHANNEL));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn task_business_failure_preserves_owned_payload() {
        llvm_managed::boolean(&source(TASK_BUSINESS_FAILURE));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn stop_task_group_and_nested_descendants_release_frames() {
        llvm_managed::boolean(&source(CANCEL_AND_NESTED));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn blocking_and_parallel_return_checked_values() {
        llvm_managed::boolean(&source(BLOCKING_AND_PARALLEL));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn capture_then_refail_preserves_owned_history() {
        llvm_managed::boolean(&source(CAPTURE_THEN_REFAIL));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn runtime_fault_is_not_captured_as_business_failure() {
        numeric_division_fault(
            &source(UNCAPTURED_RUNTIME_FAULT),
            "value // denominator",
            "{ i1 }",
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn blocking_sync_call_propagates_typed_runtime_fault() {
        numeric_division_fault(
            &source(BLOCKING_RUNTIME_FAULT),
            "42 // denominator",
            "{ i64 }",
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn blocking_abi_fault_keeps_original_buffer_and_is_not_captured() {
        llvm_managed::runtime_fault(
            &source(
                r#"identity(value: Text) (answer: Text) pure { answer = value }
build() (answer: Map<Text, List<Text>>) {
  ready = slept(result(dever.time.sleep(1)))
  key = identity("same")
  answer = { key = ["first"]
    identity(key) = ["second"] }
}
type Outcome {
  Done(value: Map<Text, List<Text>>)
  error Failed(message: Text)
}
require_rows(count: 0) () { fail(Outcome.Failed("empty")) }
require_rows(count: other) () {}
worker() (answer: Map<Text, List<Text>>) {
  ready = slept(result(dever.task.sleep(1)))
  answer = blocking(build())
  require_rows(length(answer))
}
inspect(value: Outcome.Done(entries)) (answer: Bool) recover("test expects an ABI fault") { answer = false }
inspect(value: Outcome.Failed(message)) (answer: Bool) recover("test expects an ABI fault") { answer = false }
public main() (answer: Bool) { answer = inspect(result(worker())) }
"#,
            ),
            "duplicate Map key",
        );
    }

    fn numeric_division_fault(source: &str, expression: &str, output_type: &str) {
        let origin = source.find(expression).unwrap();
        llvm_managed::execute(
            source,
            output_type,
            &format!(
                "  %failed = icmp eq i32 %status, 1
  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0
  %code = load i32, ptr %code_ptr
  %division = icmp eq i32 %code, 2
  %type_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 5
  %type = load i32, ptr %type_ptr
  %not_business = icmp eq i32 %type, 0
  %origin_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 1
  %origin_id = load i32, ptr %origin_ptr
  %origin_span = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %origin_id, i32 1
  %origin_start = load i64, ptr %origin_span
  %at_expression = icmp eq i64 %origin_start, {origin}
  %depth_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 2
  %depth = load i32, ptr %depth_ptr
  %called = icmp uge i32 %depth, 1
  %fault_ok = and i1 %failed, %division
  %kind_ok = and i1 %not_business, %at_expression
  %trace_ok = and i1 %kind_ok, %called
  %passed = and i1 %fault_ok, %trace_ok"
            ),
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn async_static_handlers_preserve_traversal_state() {
        llvm_managed::boolean(&source(ASYNC_STATIC_TRAVERSAL));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn synchronous_task_multi_output_and_group_unit() {
        llvm_managed::boolean(&source(SYNC_TASK_AND_GROUP));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn blocking_and_parallel_preserve_owned_inputs_and_outputs() {
        llvm_managed::boolean(&source(OWNED_BLOCKING_AND_PARALLEL));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn blocking_and_parallel_preserve_typed_business_failure() {
        llvm_managed::boolean(&source(BLOCKING_AND_PARALLEL_BUSINESS_FAILURE));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn negative_task_sleep_keeps_runtime_message() {
        llvm_managed::boolean(&source(NEGATIVE_TASK_SLEEP));
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn synchronous_channel_only_root_releases_aliases() {
        llvm_managed::boolean(SYNC_CHANNEL_ONLY);
    }

    #[test]
    #[ignore = "requires explicit runtime-abi archive"]
    fn default_failure_retains_origin_callers_and_owned_payload() {
        let source = source(DEFAULT_FAILURE_TRACE);
        let origin = source.find("fail(Outcome.Failed(").unwrap();
        let middle = source.rfind("origin()").unwrap();
        let root = source.rfind("middle()").unwrap();
        let mut checks = String::from(
            "  %failed = icmp eq i32 %status, 1
  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0
  %code = load i32, ptr %code_ptr
  %business = icmp eq i32 %code, 4
  %depth_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 2
  %depth = load i32, ptr %depth_ptr
  %frames = icmp eq i32 %depth, 2
  %type_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 5
  %type = load i32, ptr %type_ptr
  %typed = icmp ne i32 %type, 0
  %payload = getelementptr %dever.fault, ptr %fault, i32 0, i32 7
  %message_ptr = getelementptr i64, ptr %payload, i32 1
  %message = load ptr, ptr %message_ptr
  %history_ptr = getelementptr i64, ptr %payload, i32 2
  %history = load ptr, ptr %history_ptr
  %message_owned = icmp ne ptr %message, null
  %history_owned = icmp ne ptr %history, null
  %owned = and i1 %message_owned, %history_owned
  %base = and i1 %failed, %business
  %base2 = and i1 %frames, %typed
  %base3 = and i1 %base, %base2
  %base4 = and i1 %base3, %owned
",
        );
        for (index, (field, expected)) in [(1, origin), (3, middle), (3, root)]
            .into_iter()
            .enumerate()
        {
            let pointer = if index == 0 {
                format!(
                    "  %id_ptr{index} = getelementptr %dever.fault, ptr %fault, i32 0, i32 {field}\n"
                )
            } else {
                format!(
                    "  %id_ptr{index} = getelementptr %dever.fault, ptr %fault, i32 0, i32 {field}, i32 {}\n",
                    index - 1
                )
            };
            checks.push_str(&pointer);
            write!(
                checks,
                "  %id{index} = load i32, ptr %id_ptr{index}
  %span_ptr{index} = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %id{index}
  %span{index} = load {{ i64, i64, i64 }}, ptr %span_ptr{index}
  %start{index} = extractvalue {{ i64, i64, i64 }} %span{index}, 1
  %at{index} = icmp eq i64 %start{index}, {expected}
"
            )
            .unwrap();
        }
        checks.push_str(
            "  %trace01 = and i1 %at0, %at1
  %trace = and i1 %trace01, %at2
  %passed = and i1 %base4, %trace",
        );
        llvm_managed::execute(&source, "{}", &checks);
    }

    #[test]
    fn async_source_emits_objects_for_all_targets() {
        let ir = llvm::lower(&source(NAMED_VALUES));
        for target in dever_backend_bridge::Target::ALL {
            assert!(
                !dever_backend_bridge::emit_object(&ir, target)
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
