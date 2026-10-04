mod support;

use dever_core::{native, reference, source::SourceMap};
use std::ffi::OsString;
use std::process::Command;
use support::{checked, sources};

fn compare_sources(sources: &SourceMap, expected: &str) {
    let program = checked(sources);
    let evaluated = reference::evaluate(&program, "main.main").unwrap();
    assert!(evaluated.fault.is_none(), "{:?}", evaluated.fault);
    assert_eq!(evaluated.stdout, expected);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let executable = native::compile(&program, sources, "main.main", &rustc).unwrap();
    let output = Command::new(executable.executable()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), evaluated.stdout);
}

fn compare(source: &str, expected: &str) {
    compare_sources(&sources(source), expected);
}

#[test]
fn uuid_parsing_and_canonical_text_agree() {
    compare(
        r#"display(value: Uuid) (text: Text) { text = uuid.to_text(value) }
display(value: null) (text: Text) { text = "invalid" }
public main() (parsed: Text, invalid: Text, literal: Text) {
  parsed = display(uuid.parse("018F08D8-59B0-7CC2-98C4-DC0C0C07398F"))
  invalid = display(uuid.parse("not-a-uuid"))
  literal = uuid.to_text("12345678-1234-5678-9012-123456789abc")
}"#,
        "parsed = 018f08d8-59b0-7cc2-98c4-dc0c0c07398f\ninvalid = invalid\nliteral = 12345678-1234-5678-9012-123456789abc\n",
    );
}

#[test]
fn complete_core_example_has_independent_expected_results() {
    compare(
        include_str!("../../../examples/old/dever/core/module/main.dever"),
        concat!(
            "original = 1\nchanged = 99\nchoice = 7\ncategory = large\nexact = true\n",
            "quotient = 2.5\nthird = 0.3333333333333333333333333333333333\nrounded = 2.34\n",
            "integer_division = -2\nremainder = -1\nselected = [3, 6]\nfound = 3\ntotal = 18\n",
            "empty_total = 0\nnames = ABCabc\nordered_keys = [second, first]\nmaps_equal = false\n",
            "nan_equal = false\nfinite = true\noptionals = [null, 1, null]\nkey_value = 2\nidentity = true\n",
        ),
    );
}

#[test]
fn expression_order_nested_aliases_and_named_output_projections_agree() {
    compare(
        r#"public type Pair { left: Int
right: Int }
public type Box { pair: Pair
values: List<Int> }
mark(label: Text, value: Int) (answer: Int) { dever.io.println(label)
answer = value }
key(label: Text) (answer: Text) { dever.io.println(label)
answer = label }
parts() (left: Int, right: Int) { left = 3
right = 4 }
never() (answer: Bool) { dever.io.println("unexpected")
answer = true }
public main() (saved: Box, changed: Box, pair: Pair, ordered: Map<Text, Int>, old: List<Int>, short: Bool) {
  pair = Pair { right = mark("right", 2)
left = mark("left", 1) }
  saved = Box { pair = pair
values = [1] }
  changed = saved
  changed.pair.left = 9
  changed.values = append(changed.values, 2)
  old = saved.values
  outputs = parts()
  copy = outputs
  pair.right = copy.left
  ordered = { key("a") = mark("av", 1)
key("b") = mark("bv", 2) }
  short = false and never() or true or never()
}
"#,
        "right\nleft\na\nav\nb\nbv\nsaved = main.Box { pair = main.Pair { left = 1, right = 2 }, values = [1] }\nchanged = main.Box { pair = main.Pair { left = 9, right = 2 }, values = [1, 2] }\npair = main.Pair { left = 1, right = 3 }\nordered = {a = 1, b = 2}\nold = [1]\nshort = true\n",
    );
}

#[test]
fn nullable_choice_payloads_domains_and_signed_zero_agree() {
    compare(
        r#"type Result { Found(value: Int)
Missing }
unwrap(value: Result.Found(payload)) (answer: Int) { answer = payload }
unwrap(value: Result.Missing) (answer: Int) { answer = -1 }
unwrap(value: null) (answer: Int) { answer = -2 }
decimal_kind(value: Decimal < 0) (answer: Int) { answer = -1 }
decimal_kind(value: Decimal >= 0 and <= 1) (answer: Int) { answer = 0 }
decimal_kind(value: Decimal > 1) (answer: Int) { answer = 1 }
label(value: "x") (answer: Int) { answer = 1 }
label(value: other) (answer: Int) { answer = 2 }
sign(value: 0.0e0) (answer: Float) { answer = 1.0e0 / value }
sign(value: other) (answer: Float) { answer = 0.0e0 }
present(value: Int) (answer: Bool) { answer = true }
present(value: null) (answer: Bool) { answer = false }
optional(value: Int) (answer: Int?) { answer = value }
public main() (values: List<Int>, first_value: Int?, found: Int?, missing: Int?, negative: Bool, kind: Int, text_kind: Int, ordered: Float) {
 values = [unwrap(Result.Found(7)), unwrap(Result.Missing), unwrap(null)]
 first_value = first([null, 1])
 found = find(present, [null, 2])
 mapping = { "x" = optional(1) }
 missing = get(mapping, "none")
 negative = sign(-0.0e0) < 0.0
 kind = decimal_kind(-0.1) + decimal_kind(0.5) + decimal_kind(1.1)
 text_kind = label("x") + label("y")
 ordered = sum([1.0e16, 1.0e0, -1.0e16])
}
"#,
        "values = [7, -1, -2]\nfirst_value = null\nfound = 2\nmissing = null\nnegative = true\nkind = 0\ntext_kind = 3\nordered = 0\n",
    );
}

#[test]
fn static_handlers_cross_packages_and_reduce_all_finite_sequences() {
    let mut sources = sources(
        r#"add(value: Int, state: Int) (next: Int) { next = state + value }
until(value: Int, state: Int) (next: Int, stop: Bool) { next = state + value
stop = value == 2 }
increment(value: Int) (answer: Int) { answer = value + 1 }
show(value: Int) () { dever.io.println(int.to_text(value)) }
public main() (list: Int, empty: Int, stopped: Int, bytes: Int, mapped: List<Int>, forwarded: Int) {
 list = reduce(add, [1, 2, 3], 0)
 empty = reduce(add, [], 9)
 stopped = reduce_until(until, [1, 2, 100], 0)
 raw = dever.bytes.from_text("AB")
 bytes = reduce(add, raw, 0)
 mapped = each(increment, raw)
 each(show, [1, 2])
 each(show, dever.bytes.from_text("A"))
 forwarded = helper.forward(increment, 4)
}
"#,
    );
    sources.add("helper.dever", r#"public forward(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = apply(value, route) }
apply(value: Int, route: handler(value: Int) (answer: Int)) (answer: Int) { answer = route(value) }
"#);
    compare_sources(
        &sources,
        "1\n2\n65\nlist = 6\nempty = 9\nstopped = 3\nbytes = 131\nmapped = [66, 67]\nforwarded = 5\n",
    );
}

#[test]
fn runtime_faults_match_exact_locations_call_traces_and_prior_stdout() {
    for (function, arguments, expected) in [
        (
            "compute_failure(a: Int, b: Int) (answer: Int) { answer = a // b }",
            "1, 0",
            "Int division by zero",
        ),
        (
            "compute_failure(a: Int, b: Int) (answer: Int) { answer = a + b }",
            "9223372036854775807, 1",
            "Int overflow",
        ),
        (
            "compute_failure(a: Decimal, b: Decimal) (answer: Decimal) { answer = a / b }",
            "1.0, 0.0",
            "Decimal division by zero",
        ),
        (
            "compute_failure(a: Int, b: Int) (answer: Map<Int, Int>) { answer = { a = 1\nb = 2 } }",
            "1, 1",
            "duplicate Map key",
        ),
    ] {
        let sources = sources(&format!(
            "{function}\npublic main() () {{ dever.io.println(\"before\")\nanswer = compute_failure({arguments}) }}"
        ));
        let program = checked(&sources);
        let evaluated = reference::evaluate(&program, "main.main").unwrap();
        assert_eq!(evaluated.stdout, "before\n");
        let fault = evaluated.fault.unwrap();
        assert_eq!(fault.message, expected);
        assert!(fault.spans.len() >= 2);
        assert_eq!(
            sources
                .get(fault.spans[0].source)
                .position(fault.spans[0].start)
                .0,
            1
        );
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
        let executable = native::compile(&program, &sources, "main.main", &rustc).unwrap();
        let output = Command::new(executable.executable()).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), evaluated.stdout);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        let record: serde_json::Value = serde_json::from_str(&stderr).unwrap();
        assert_eq!(
            record,
            serde_json::json!({"level": "error", "message": fault.render(&sources), "fields": {}})
        );
    }
}

#[test]
fn standard_text_numeric_and_bytes_results_agree() {
    compare(
        r#"decode(value: dever.bytes.DecodeResult.Decoded(text)) (answer: Text) recover("test observes decode failures in its assertions") { answer = text }
decode(value: other) (answer: Text) recover("test observes decode failures in its assertions") { answer = "invalid" }
buffer(value: dever.bytes.BufferResult.Ready(bytes)) (answer: Bytes) recover("test observes buffer failures in its assertions") { answer = bytes }
buffer(value: other) (answer: Bytes) recover("test observes buffer failures in its assertions") { answer = dever.bytes.from_text("") }
public main() (text_value: Text, unicode: Text?, slice: Text?, index: Int?, predicates: Bool, integer: Int?, decimal_value: Decimal?, float_value: Float?, invalid: Int?, numbers: Text, bytes: Text, outside: Int?, bad: Text) {
 text_value = text.join(text.split(text.upper("a,b"), ","), "-")
 unicode = text.at("中文", 1)
 slice = text.slice("中文a", 1, 3)
 index = text.index_of("中文a", "a")
 predicates = text.contains("abc", "b") and text.starts_with("abc", "a") and text.ends_with("abc", "c") and text.replace("abc", "b", "x") == "axc"
 integer = int.parse("-12")
 decimal_value = decimal.parse("1.25")
 float_value = float.parse("1.5")
 invalid = int.parse("no")
 numbers = int.to_text(4) + decimal.to_text(decimal.from_int(2)) + float.to_text(1.5e0)
 combined = dever.bytes.concat(dever.bytes.from_text("ab"), dever.bytes.from_text("cd"))
 bytes = decode(result(dever.bytes.to_text(buffer(result(dever.bytes.slice(combined, 1, 3))))))
 outside = dever.bytes.at(combined, -1)
 bad = decode(result(dever.bytes.to_text(buffer(result(dever.bytes.from_ints([255]))))))
}
"#,
        "text_value = A-B\nunicode = 文\nslice = 文a\nindex = 2\npredicates = true\ninteger = -12\ndecimal_value = 1.25\nfloat_value = 1.5\ninvalid = null\nnumbers = 421.5\nbytes = bc\noutside = null\nbad = invalid\n",
    );
}

#[test]
fn file_stream_aliases_early_stop_resume_and_terminal_close_agree() {
    let directory = support::temp::TemporaryDirectory::new();
    let path = directory.path().join("stream.txt");
    std::fs::write(&path, "ABC").unwrap();
    let source = r#"add(byte: Int, state: Int) (next: Int) { next = state + byte }
collect(event: dever.io.ReadEvent.Chunk(bytes), state: Int) (next: Int) recover("test observes collect failures in its assertions") { next = reduce(add, bytes, state) }
collect(event: dever.io.ReadEvent.Failed(message), state: Int) (next: Int) recover("test observes collect failures in its assertions") { next = state + 1000 }
first(event: dever.io.ReadEvent, state: Int) (next: Int, stop: Bool) { next = collect(event, state)
stop = true }
show(event: dever.io.ReadEvent) () recover("test observes show failures in its assertions") { dever.io.println("unexpected") }
streaming(result: dever.io.ReadStreamResult.Streaming(stream), file: dever.system.File) (answer: List<Int>) recover("test observes streaming failures in its assertions") {
 alias = stream
 prefix = reduce_until(first, stream, 0)
 suffix = reduce(collect, alias, 0)
 exhausted = reduce(collect, stream, 7)
 close(stream)
 close(alias)
 each(show, alias)
 answer = [prefix, suffix, exhausted]
}
streaming(result: other, file: dever.system.File) (answer: List<Int>) recover("test observes streaming failures in its assertions") { answer = [] }
terminal(result: dever.io.ReadStreamResult.Streaming(stream), file: dever.system.File) (answer: List<Int>) recover("test observes terminal failures in its assertions") {
 dever.io.close(file)
 once = reduce(collect, stream, 0)
 again = reduce(collect, stream, 0)
 answer = [once, again]
}
terminal(result: other, file: dever.system.File) (answer: List<Int>) recover("test observes terminal failures in its assertions") { answer = [] }
opened(result: dever.io.OpenResult.Opened(file)) (answer: List<Int>) recover("test observes opened failures in its assertions") { answer = streaming(result(dever.io.chunks(file, 1)), file) }
opened(result: other) (answer: List<Int>) recover("test observes opened failures in its assertions") { answer = [] }
closed(result: dever.io.OpenResult.Opened(file)) (answer: List<Int>) recover("test observes closed failures in its assertions") { answer = terminal(result(dever.io.chunks(file, 1)), file) }
closed(result: other) (answer: List<Int>) recover("test observes closed failures in its assertions") { answer = [] }
public main() (values: List<Int>, failure: List<Int>) {
 values = opened(result(dever.io.open(PATH)))
 failure = closed(result(dever.io.open(PATH)))
}
"#.replace("PATH", &format!("{:?}", path.to_str().unwrap()));
    compare(&source, "values = [65, 133, 7]\nfailure = [1000, 0]\n");
}

#[test]
fn repeated_arguments_live_fields_and_growing_reduction_state_keep_value_semantics() {
    compare(
        r#"public type Pair { left: List<Int>
right: List<Int> }
combine(left: List<Int>, right: List<Int>) (answer: List<Int>) {
 left = append(left, 8)
 answer = append(right, length(left))
}
grow(value: Int, state: List<Int>) (next: List<Int>) { next = append(state, value) }
public main() (original: List<Int>, combined: List<Int>, old: Pair, changed: Pair, reduced: List<Int>, shared: List<Int>) {
 original = [1]
 combined = combine(original, original)
 changed = Pair { left = original
right = original }
 old = changed
 changed.left = append(changed.right, length(changed.left))
 changed.right = changed.left
 shared = [9]
 reduced = reduce(grow, [1, 2, 3], shared)
}
"#,
        "original = [1]\ncombined = [1, 2]\nold = main.Pair { left = [1], right = [1] }\nchanged = main.Pair { left = [1, 1], right = [1, 1] }\nreduced = [9, 1, 2, 3]\nshared = [9]\n",
    );
}

#[test]
fn numeric_operators_math_nullable_collections_and_key_types_agree() {
    compare(
        r#"arithmetic(a: Int, b: Int) (answer: Int) { answer = -(a - b) * b + a % b }
decimal_math(a: Decimal, b: Decimal) (answer: Decimal) { answer = -(a - b) * b / 2 }
floating(a: Float, b: Float) (answer: Float) { answer = -(a - b) * b + a / b }
entry_key(entry: MapEntry<Bool, Text>) (answer: Bool) { answer = entry.key }
absent() (answer: Int?) { answer = null }
public main() (integer: Int, fixed: Decimal, float_value: Float, math_ok: Bool, no_nan_order: Bool, first_value: Int?, found: Int?, identity: Text, keys: List<Bool>, entry: MapEntry<Bool, Text>?) {
 integer = arithmetic(7, 3)
 fixed = decimal_math(1.5, 0.5)
 float_value = floating(3.0e0, 2.0e0)
 math_ok = math.sqrt(4.0e0) == 2.0e0 and math.sin(0.0e0) == 0.0e0 and math.cos(0.0e0) == 1.0e0 and math.log(1.0e0) == 0.0e0 and math.pow(2.0e0, 3.0e0) == 8.0e0 and math.is_infinite(1.0e0 / 0.0e0)
 nan = 0.0e0 / 0.0e0
 no_nan_order = not (nan < 1.0e0 or nan <= 1.0e0 or nan > 1.0e0 or nan >= 1.0e0) and math.is_nan(nan)
 first_value = first([])
 optional_map = { "x" = absent() }
 found = get(optional_map, "x")
 id = dever.id.from_text("stable")
 ids = { id = 7 }
 identity = dever.id.to_text(id)
 values = { true = "a"
false = "b" }
 keys = each(entry_key, entries(values))
 entry = first(entries(values))
}
"#,
        "integer = -11\nfixed = -0.25\nfloat_value = -0.5\nmath_ok = true\nno_nan_order = true\nfirst_value = null\nfound = null\nidentity = stable\nkeys = [true, false]\nentry = {key = true, value = a}\n",
    );
}

#[test]
fn time_failures_are_source_defined_results() {
    compare(
        r#"sleep_failed(result: dever.time.SleepResult.Failed(message)) (answer: Bool) recover("test observes sleep_failed failures in its assertions") { answer = true }
sleep_failed(result: other) (answer: Bool) recover("test observes sleep_failed failures in its assertions") { answer = false }
clock_ok(result: dever.time.ClockResult.Read(value)) (answer: Bool) recover("test observes clock_ok failures in its assertions") { answer = value >= 0 }
clock_ok(result: other) (answer: Bool) recover("test observes clock_ok failures in its assertions") { answer = false }
public main() (sleep: Bool, wall: Bool, monotonic: Bool) {
 sleep = sleep_failed(result(dever.time.sleep(-1)))
 wall = clock_ok(result(dever.time.unix_millis()))
 monotonic = clock_ok(result(dever.time.monotonic_nanos()))
}
"#,
        "sleep = true\nwall = true\nmonotonic = true\n",
    );
}
