use dever_core::source::SourceMap;

#[path = "dever-tests/tests/support/llvm.rs"]
mod llvm;
use llvm::lower;

#[test]
fn typed_clauses_records_choices_and_handlers_lower_to_direct_functions() {
    let ir = lower(
        r#"type Pair { left: Int
right: Int }
type Selection { Found(value: Int)
Missing }
increment(value: Int) (answer: Int) { answer = value + 1 }
apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }
choose(value: Selection.Found(payload)) (answer: Int) { answer = payload }
choose(value: Selection.Missing) (answer: Int) { answer = -1 }
optional(value: Int) (answer: Int?) { answer = value }
public main() (sum: Int, selected: Int, maybe: Int?, ready: Bool) {
  original = Pair { left = 4
right = 5 }
  changed = original
  changed.left = apply(increment, original.left)
  sum = changed.left + original.left
  selected = choose(Selection.Found(changed.right))
  maybe = optional(7)
  ready = false and (increment(9223372036854775807) == 0) or true
}
"#,
    );
    assert!(ir.contains("define i32 @dever_entry(ptr %out, ptr %fault)"));
    assert!(ir.contains("store { i64, i64, { i1, i64 }, i1 } zeroinitializer, ptr %out"));
    assert!(ir.contains("%dever.fault = type { i32, i32, i32, ["));
    assert!(ir.contains("@dever_fault_capacity = constant i32"));
    assert!(ir.contains("@dever_source_spans = constant"));
    assert!(ir.contains("@llvm.sadd.with.overflow.i64"));
    assert!(ir.contains("phi i1"));
    assert!(ir.contains("getelementptr %dever.fault, ptr %fault, i32 0, i32 3"));
    assert!(ir.contains("%T"));
    assert!(!ir.contains("dever_runtime"));
}

#[test]
fn stdout_writes_lower_through_the_typed_system_abi() {
    let ir = lower("public main() () { dever.io.println(\"hello\") }\n");
    assert!(ir.contains("call i32 @dever_rt_v1_stdout_write("));
}

#[test]
fn unsupported_reachable_effect_reports_original_source_location() {
    let mut sources = SourceMap::default();
    sources.add(
        "user/account/app.dever",
        "identity() (value: Text) { value = dever.auth.id() }\n",
    );
    let program = dever_core::check(&sources).unwrap();
    let error =
        dever_core::llvm::emit_kernel(&program, &sources, "user.account.identity").unwrap_err();
    let (_, location) = error.split_once(".dever:").expect("Dever source path");
    let mut position = location.split(':');
    assert!(position.next().unwrap().parse::<usize>().is_ok(), "{error}");
    assert!(position.next().unwrap().parse::<usize>().is_ok(), "{error}");
    assert!(error.contains("does not support"), "{error}");
}

#[test]
fn dynamic_integer_faults_have_origin_and_caller_frame_storage() {
    let ir = lower(
        r#"divide(left: Int, right: Int) (answer: Int) { answer = left // right }
outer(value: Int) (answer: Int) { answer = divide(value, 0) }
public main() (answer: Int) { answer = outer(7) }
"#,
    );
    assert!(ir.contains("@dever_fault_capacity = constant i32 4"));
    assert!(ir.contains("store i32 2, ptr"));
    assert!(ir.matches("i32 0, i32 3, i32 %v").count() >= 2);
    assert!(ir.contains("@dever_source_span_count = constant i32 3"));
}

#[test]
fn temporary_nominal_values_are_emitted_even_without_nominal_local_slots() {
    let ir = lower(
        r#"type Pair { left: Int
right: Int }
public main() (equal: Bool) {
  equal = Pair { left = 1
right = 2 } == Pair { left = 1
right = 2 }
}
"#,
    );
    assert!(ir.contains("%T0 = type { i64, i64 }"));
    assert!(ir.contains("extractvalue %T0"));
}

#[test]
fn nullable_collection_rows_keep_their_full_storage_before_flattening() {
    let ir = lower(
        r#"present(value: Int?) (answer: Bool) { answer = value != null }
first_optional(values: List<Text?>) (answer: Text?) { answer = first(values) }
lookup(values: Map<Text, Text?>) (answer: Text?) { answer = get(values, "key") }
find_optional(values: List<Int?>) (answer: Int?) { answer = find(present, values) }
public main() (first_value: Text?, map_value: Text?, found: Int?) {
  first_value = first_optional([null, "next"])
  map_value = lookup({ "key" = null })
  found = find_optional([null, 1])
}
"#,
    );
    assert!(ir.contains("@dever_rt_v1_list_first_take"));
    assert!(ir.contains("@dever_rt_v1_map_get_take"));
    assert!(ir.contains("@dever_rt_v1_list_cursor_next"));
    assert!(ir.matches("store { i1, ptr } %v").count() >= 2);
    assert!(ir.contains("store { i1, i64 } %v"));
}

#[test]
fn business_failures_and_result_capture_keep_typed_payloads() {
    let ir = lower(
        r#"public type Failure { error Missing(message: Text)
error Conflict(message: Text) }
public type Outcome { Done(value: Int)
error Failed(error: Failure) }
public type Empty { Done
error Missing(message: Text) }
public type Pair { Done(left: Int, right: Int)
error Missing(message: Text) }
load(value: 0) (answer: Int) { fail(Failure.Missing("absent")) }
load(value: 1) (answer: Int) { fail(Failure.Conflict("duplicate")) }
load(value: other) (answer: Int) { answer = value }
stop() () { fail(Empty.Missing("stopped")) }
two(value: 0) (left: Int, right: Int) { fail(Pair.Missing("missing")) }
two(value: other) (left: Int, right: Int) { left = 1
right = 2 }
public main() (outcome: Outcome, empty: Empty, pair: Pair) {
  outcome = result(load(0))
  empty = result(stop())
  pair = result(two(1))
}
"#,
    );
    assert!(ir.contains("i32, i32, ["));
    assert!(ir.contains("@dever_fault_release"));
    assert!(ir.contains("drop_failure_"));
    assert!(ir.contains("capture_match"));
    assert!(ir.contains("capture_propagate"));
    assert!(ir.contains("store %dever.fault zeroinitializer, ptr %fault"));
}

#[test]
fn bytes_results_distinguish_business_errors_from_abi_faults() {
    let ir = lower(
        r#"public main() (decoded: dever.bytes.DecodeResult, sliced: dever.bytes.BufferResult, from_ints: dever.bytes.BufferResult) {
  bytes = dever.system.bytes_from_text("abc")
  decoded = dever.system.bytes_to_text(bytes)
  sliced = dever.system.bytes_slice(bytes, 0, 2)
  from_ints = dever.system.bytes_from_ints([65, 66])
}
"#,
    );
    assert!(ir.contains("@dever_rt_v1_bytes_to_text_handle"));
    assert!(ir.contains("@dever_rt_v1_bytes_slice"));
    assert!(ir.contains("@dever_rt_v1_bytes_from_ints"));
    assert!(ir.matches("icmp eq i32 %v").count() >= 6);
    assert!(ir.contains("@dever_rt_v1_buffer_free"));
}
