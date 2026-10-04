//! Real checked-source, LLVM-object and runtime-archive result acceptance.

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

    const SINGLE: &str = r#"type Outcome {
  Done(value: Text)
  error Failed(message: Text)
}
step(value: 0) (answer: Text) { fail(Outcome.Failed("specific failure")) }
step(value: 1) (answer: Text) { fail(Outcome.Failed("second failure")) }
step(value: other) (answer: Text) { answer = "successful value" }
inspect(value: Outcome.Done(text), expected: Text, wanted_success: Bool) (answer: Bool) recover("test handles failures") { answer = wanted_success and text == expected }
inspect(value: Outcome.Failed(message), expected: Text, wanted_success: Bool) (answer: Bool) recover("test handles failures") { answer = not wanted_success and message == expected }
public main() (answer: Bool) {
  first = inspect(result(step(0)), "specific failure", false)
  second = inspect(result(step(7)), "successful value", true)
  third = inspect(result(step(1)), "second failure", false)
  answer = first and second and third
}
"#;

    const ARITY: &str = r#"type EmptyResult {
  Done
  error Failed(message: Text)
}
type PairResult {
  Many(label: Text, names: List<Text>)
  error Failed(message: Text)
}
empty(bad: true) () { fail(EmptyResult.Failed("empty failed")) }
empty(bad: false) () {}
pair(bad: true) (label: Text, names: List<Text>) { fail(PairResult.Failed("pair failed")) }
pair(bad: false) (label: Text, names: List<Text>) { label = "pair label"
names = ["one", "two"] }
read_empty(value: EmptyResult.Done, wanted_success: Bool) (answer: Bool) recover("test handles failures") { answer = wanted_success }
read_empty(value: EmptyResult.Failed(message), wanted_success: Bool) (answer: Bool) recover("test handles failures") { answer = not wanted_success and message == "empty failed" }
read_pair(value: PairResult.Many(label, names), wanted_success: Bool) (answer: Bool) recover("test handles failures") { answer = wanted_success and label == "pair label" and names == ["one", "two"] }
read_pair(value: PairResult.Failed(message), wanted_success: Bool) (answer: Bool) recover("test handles failures") { answer = not wanted_success and message == "pair failed" }
public main() (answer: Bool) {
  answer = read_empty(result(empty(false)), true) and read_empty(result(empty(true)), false) and read_pair(result(pair(false)), true) and read_pair(result(pair(true)), false)
}
"#;

    const WRAPPED: &str = r#"type Failure {
  error Missing(message: Text)
  error Conflict(message: Text)
}
type Outcome {
  Done(value: Int)
  error Failed(error: Failure)
}
load(value: 0) (answer: Int) { fail(Failure.Missing("missing")) }
load(value: 1) (answer: Int) { fail(Failure.Conflict("conflict")) }
load(value: other) (answer: Int) { answer = value }
rethrow(value: Outcome.Done(number)) (answer: Int) { answer = number }
rethrow(value: Outcome.Failed(error)) (answer: Int) { fail(Outcome.Failed(error)) }
inspect(value: Outcome.Done(number), wanted: Text) (answer: Bool) recover("test handles failures") { answer = wanted == "done" and number == 7 }
inspect(value: Outcome.Failed(error), wanted: Text) (answer: Bool) recover("test handles failures") { answer = inspect_error(error, wanted) }
inspect_error(value: Failure.Missing(message), wanted: Text) (answer: Bool) recover("test handles failures") { answer = wanted == "missing" and message == "missing" }
inspect_error(value: Failure.Conflict(message), wanted: Text) (answer: Bool) recover("test handles failures") { answer = wanted == "conflict" and message == "conflict" }
public main() (answer: Bool) {
  answer = inspect(result(load(0)), "missing") and inspect(result(load(1)), "conflict") and inspect(result(rethrow(result(load(0)))), "missing") and inspect(result(load(7)), "done")
}
"#;

    const HANDLERS: &str = r#"type Detail {
  label: Text
  history: List<Text>
}
type Outcome {
  Done(value: List<Text>)
  error Failed(detail: Detail)
}
type State {
  prefix: Text
  names: List<Text>
}
map_name(value: Text, prefix: Text) (answer: Text) {
  label = prefix + value
  fail_if(value, label)
  answer = label
}
fail_if(value: "stop", label: Text) () {
  fail(Outcome.Failed(Detail { label = label
history = ["before", label] }))
}
fail_if(value: other, label: Text) () {}
collect(value: Text, state: State) (next: State) {
  label = state.prefix + value
  fail_if(value, label)
  next = State { prefix = state.prefix
names = append(state.names, label) }
}
mapped(bad: true) (value: List<Text>) { value = each(map_name, ["first", "stop", "unvisited"], "ctx=") }
mapped(bad: false) (value: List<Text>) { value = each(map_name, ["first", "last"], "ctx=") }
reduced(bad: true) (value: List<Text>) { value = reduce(collect, ["first", "stop", "unvisited"], State { prefix = "ctx="
names = ["seed"] }).names }
reduced(bad: false) (value: List<Text>) { value = reduce(collect, ["first", "last"], State { prefix = "ctx="
names = ["seed"] }).names }
inspect(value: Outcome.Done(items), expected: List<Text>) (answer: Bool) recover("test handles failures") { answer = items == expected }
inspect(value: Outcome.Failed(detail), expected: List<Text>) (answer: Bool) recover("test handles failures") { answer = detail.label == "ctx=stop" and detail.history == ["before", "ctx=stop"] and length(expected) == 0 }
public main() (answer: Bool) {
  first = inspect(result(mapped(true)), [])
  second = inspect(result(mapped(false)), ["ctx=first", "ctx=last"])
  third = inspect(result(reduced(true)), [])
  fourth = inspect(result(reduced(false)), ["seed", "ctx=first", "ctx=last"])
  fifth = inspect(result(mapped(true)), [])
  answer = first and second and third and fourth and fifth
}
"#;

    const ROOT_FAILURE: &str = r#"type Detail {
  names: List<Text>
  lookup: Map<Text, List<Text>>
}
type Outcome {
  error Other(message: Text)
  error Failed(detail: Detail)
}
origin() () {
  names = ["owned", "payload"]
  detail = Detail { names = names
lookup = { "key" = names } }
  fail(Outcome.Failed(detail))
}
middle() () { origin() }
public main() () { middle() }
"#;

    const BYTES_OK: &str = r#"decoded(value: dever.bytes.DecodeResult.Decoded(text)) (answer: Text) recover("test handles decode failures") { answer = text }
decoded(value: dever.bytes.DecodeResult.Failed(message)) (answer: Text) recover("test handles decode failures") { answer = message }
ready(value: dever.bytes.BufferResult.Ready(bytes)) (answer: Bytes) recover("test handles byte failures") { answer = bytes }
ready(value: dever.bytes.BufferResult.Failed(message)) (answer: Bytes) recover("test handles byte failures") { answer = dever.bytes.from_text(message) }
public main() (answer: Bool) {
  original = dever.bytes.from_text("中🙂")
  alias = original
  numeric = ready(result(dever.bytes.from_ints([228, 184, 173, 240, 159, 153, 130])))
  trimmed = ready(result(dever.bytes.slice(numeric, 3, 7)))
  empty = ready(result(dever.bytes.from_ints([])))
  answer = decoded(result(dever.bytes.to_text(numeric))) == "中🙂" and decoded(result(dever.bytes.to_text(trimmed))) == "🙂" and decoded(result(dever.bytes.to_text(empty))) == "" and dever.bytes.length(empty) == 0 and original == alias and original == numeric and dever.bytes.length(trimmed) == 4 and dever.bytes.to_text(original) == "中🙂"
}
"#;

    const BYTES_BAD: &str = r#"buffer_error(value: dever.bytes.BufferResult.Ready(bytes)) (answer: Text) recover("test handles byte failures") { answer = "unexpected success" }
buffer_error(value: dever.bytes.BufferResult.Failed(message)) (answer: Text) recover("test handles byte failures") { answer = message }
decode_error(value: dever.bytes.DecodeResult.Decoded(text)) (answer: Text) recover("test handles decode failures") { answer = "unexpected success" }
decode_error(value: dever.bytes.DecodeResult.Failed(message)) (answer: Text) recover("test handles decode failures") { answer = message }
public main() (answer: Bool) {
  bytes = dever.bytes.from_text("abc")
  invalid = dever.bytes.from_text("x")
  answer = buffer_error(result(dever.bytes.from_ints([-1]))) == "byte value must be between 0 and 255" and buffer_error(result(dever.bytes.from_ints([256]))) == "byte value must be between 0 and 255" and buffer_error(result(dever.bytes.slice(bytes, -1, 2))) == "byte range is out of bounds" and buffer_error(result(dever.bytes.slice(bytes, 0, 4))) == "byte range is out of bounds" and buffer_error(result(dever.bytes.slice(bytes, 2, 1))) == "byte range is out of bounds" and decode_error(result(dever.bytes.to_text(dever.bytes.from_ints([255])))) == "invalid utf-8 sequence of 1 bytes from index 0" and invalid == dever.bytes.from_text("x")
}
"#;

    const NESTED_OWNED: &str = r#"type Detail {
  names: List<Text>
  lookup: Map<Text, List<Text>>
}
type Outcome {
  Done(value: Text)
  error Failed(detail: Detail)
}
fail_owned() (answer: Text) {
  names = ["first", "second"]
  fail(Outcome.Failed(Detail { names = names
lookup = { "key" = names } }))
}
inspect(value: Outcome.Done(text)) (answer: Bool) recover("test handles failures") { answer = text == "none" }
inspect(value: Outcome.Failed(detail)) (answer: Bool) recover("test handles failures") {
  alias = detail
  alias.names = append(alias.names, "third")
  alias.lookup = put(alias.lookup, "key", alias.names)
  answer = detail.names == ["first", "second"] and get(detail.lookup, "key") == optional_names(["first", "second"]) and alias.names == ["first", "second", "third"] and get(alias.lookup, "key") == optional_names(["first", "second", "third"])
}
optional_names(value: List<Text>) (answer: List<Text>?) { answer = value }
public main() (answer: Bool) { answer = inspect(result(fail_owned())) }
"#;

    const ROOT_SUCCESS: &str = r#"type Outcome {
  Done(value: Text)
  error Failed(message: Text)
}
step(bad: true) (value: Text) { fail(Outcome.Failed("unexpected")) }
step(bad: false) (value: Text) { value = "root-owned" }
unwrap(value: Outcome.Done(text)) (answer: Text) recover("test handles failures") { answer = text }
unwrap(value: Outcome.Failed(message)) (answer: Text) recover("test handles failures") { answer = message }
public main() (answer: Text) { answer = unwrap(result(step(false))) }
"#;

    #[test]
    fn result_fixtures_pass_the_real_checker() {
        for source in [
            SINGLE,
            ARITY,
            WRAPPED,
            HANDLERS,
            ROOT_FAILURE,
            BYTES_OK,
            BYTES_BAD,
            NESTED_OWNED,
            ROOT_SUCCESS,
        ] {
            llvm::lower(source);
        }
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn direct_failure_and_single_success_capture() {
        llvm_managed::boolean(SINGLE);
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn zero_and_multiple_outputs_capture_in_order() {
        llvm_managed::boolean(ARITY);
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn wrapped_and_explicit_outer_failures_keep_nominal_identity() {
        llvm_managed::boolean(WRAPPED);
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn static_handler_failures_release_cursors_context_and_state() {
        llvm_managed::boolean(HANDLERS);
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn root_failure_retains_business_code_owned_payload_and_call_chain() {
        let origin = ROOT_FAILURE.find("fail(Outcome.Failed(detail))").unwrap();
        let middle_call = ROOT_FAILURE.rfind("origin()").unwrap();
        let root_call = ROOT_FAILURE.rfind("middle()").unwrap();
        llvm_managed::execute(
            ROOT_FAILURE,
            "{}",
            &format!(
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
  %variant_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 6
  %variant = load i32, ptr %variant_ptr
  %correct_variant = icmp eq i32 %variant, 1
  %origin_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 1
  %origin_id = load i32, ptr %origin_ptr
  %origin_span = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %origin_id
  %origin_bounds = load {{ i64, i64, i64 }}, ptr %origin_span
  %origin_start = extractvalue {{ i64, i64, i64 }} %origin_bounds, 1
  %origin_end = extractvalue {{ i64, i64, i64 }} %origin_bounds, 2
  %origin_start_ok = icmp eq i64 %origin_start, {origin}
  %origin_end_ok = icmp eq i64 %origin_end, {}
  %origin_ok = and i1 %origin_start_ok, %origin_end_ok
  %frame0_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 3, i32 0
  %frame0_id = load i32, ptr %frame0_ptr
  %frame0_span = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %frame0_id
  %frame0_bounds = load {{ i64, i64, i64 }}, ptr %frame0_span
  %frame0_start = extractvalue {{ i64, i64, i64 }} %frame0_bounds, 1
  %frame0_end = extractvalue {{ i64, i64, i64 }} %frame0_bounds, 2
  %frame0_start_ok = icmp eq i64 %frame0_start, {middle_call}
  %frame0_end_ok = icmp eq i64 %frame0_end, {}
  %frame0_ok = and i1 %frame0_start_ok, %frame0_end_ok
  %frame1_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 3, i32 1
  %frame1_id = load i32, ptr %frame1_ptr
  %frame1_span = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %frame1_id
  %frame1_bounds = load {{ i64, i64, i64 }}, ptr %frame1_span
  %frame1_start = extractvalue {{ i64, i64, i64 }} %frame1_bounds, 1
  %frame1_end = extractvalue {{ i64, i64, i64 }} %frame1_bounds, 2
  %frame1_start_ok = icmp eq i64 %frame1_start, {root_call}
  %frame1_end_ok = icmp eq i64 %frame1_end, {}
  %frame1_ok = and i1 %frame1_start_ok, %frame1_end_ok
  %payload = getelementptr %dever.fault, ptr %fault, i32 0, i32 7
  %names_ptr = getelementptr i64, ptr %payload, i32 1
  %names = load ptr, ptr %names_ptr
  %lookup_ptr = getelementptr i64, ptr %payload, i32 2
  %lookup = load ptr, ptr %lookup_ptr
  %names_ok = icmp ne ptr %names, null
  %lookup_ok = icmp ne ptr %lookup, null
  %owners_ok = and i1 %names_ok, %lookup_ok
  %first = and i1 %failed, %business
  %second = and i1 %frames, %typed
  %third = and i1 %first, %second
  %fourth = and i1 %third, %correct_variant
  %fifth = and i1 %origin_ok, %frame0_ok
  %sixth = and i1 %fifth, %frame1_ok
  %seventh = and i1 %fourth, %sixth
  %passed = and i1 %seventh, %owners_ok",
                origin + "fail(Outcome.Failed(detail))".len(),
                middle_call + "origin()".len(),
                root_call + "middle()".len()
            ),
        );
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn nested_owned_failure_payload_keeps_cow_aliases_independent() {
        llvm_managed::boolean(NESTED_OWNED);
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn captured_success_transfers_owned_root_output() {
        llvm_managed::execute(
            ROOT_SUCCESS,
            "{ ptr }",
            "  %okay = icmp eq i32 %status, 0
  %answer = load ptr, ptr %out
  %owned = icmp ne ptr %answer, null
  %passed = and i1 %okay, %owned",
        );
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn capture_does_not_consume_dynamic_numeric_faults() {
        llvm_managed::execute(
            r#"type Outcome { Done(value: Int)
error Failed(message: Text) }
divide(value: -1) (answer: Int) { fail(Outcome.Failed("business")) }
divide(value: other) (answer: Int) { answer = 9 // value }
inspect(value: Outcome.Done(number)) (answer: Bool) recover("test handles failures") { answer = number == 0 }
inspect(value: Outcome.Failed(message)) (answer: Bool) recover("test handles failures") { answer = message == "none" }
public main() (answer: Bool) { denominator = length(["one"]) - 1
answer = inspect(result(divide(denominator))) }"#,
            "{ i1 }",
            "  %failed = icmp eq i32 %status, 1
  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0
  %code = load i32, ptr %code_ptr
  %division = icmp eq i32 %code, 2
  %passed = and i1 %failed, %division",
        );
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn capture_does_not_consume_original_runtime_abi_faults() {
        llvm_managed::runtime_fault(
            r#"type Outcome { Done(value: Map<Text, List<Text>>)
error Failed(message: Text) }
identity(value: Text) (answer: Text) { answer = value }
build(bad: true) (answer: Map<Text, List<Text>>) { fail(Outcome.Failed("business")) }
build(bad: false) (answer: Map<Text, List<Text>>) { key = identity("same")
answer = { key = ["first"]
identity(key) = ["second"] } }
inspect(value: Outcome.Done(entries)) (answer: Bool) recover("test handles failures") { answer = length(entries) == 0 }
inspect(value: Outcome.Failed(message)) (answer: Bool) recover("test handles failures") { answer = message == "none" }
public main() (answer: Bool) { answer = inspect(result(build(false))) }"#,
            "duplicate Map key",
        );
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn bytes_results_decode_unicode_and_keep_aliases() {
        llvm_managed::boolean(BYTES_OK);
    }

    #[test]
    #[ignore = "requires explicit target/native-runtime-abi staticlib"]
    fn bytes_failures_keep_original_messages() {
        llvm_managed::boolean(BYTES_BAD);
    }
}
