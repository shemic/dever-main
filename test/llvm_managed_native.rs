//! Bounded native acceptance for the typed, managed-value LLVM kernel subset.
//! These checks use an explicit test archive, never a host-language fallback.

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
    use dever_backend_bridge::{Target, emit_object};
    use llvm_managed::{boolean, execute, runtime_fault};

    #[test]
    fn managed_nested_values_emit_real_objects_on_all_six_targets() {
        let ir = llvm::lower(
            r#"type Bundle {
  ready: Bool
  label: Text
  bytes: Bytes
  costs: List<Decimal>
  lookup: Map<Text, List<Decimal>>
}
type Envelope { Ready(value: Bundle)
Missing }
optional(value: Bundle) (answer: Envelope?) { answer = Envelope.Ready(value) }
public main() (answer: Bool) {
  costs = [1.25, 2.5]
  bundle = Bundle { ready = true
label = "typed"
bytes = dever.bytes.from_text("bytes")
costs = costs
lookup = { "costs" = costs } }
  answer = optional(bundle) == optional(bundle)
}
"#,
        );
        for target in Target::ALL {
            assert!(emit_object(&ir, target).unwrap().len() > 128);
        }
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn unicode_text_nullable_results_and_aliases_execute_and_release() {
        boolean(
            r#"identity(value: Text) (answer: Text) { answer = value }
public main() (answer: Bool) {
  original = " A中🙂 "
  alias = identity(original)
  normalized = text.lower(text.trim(alias))
  replaced = text.replace(normalized, "a", "b")
  parts = text.split("中🙂", "")
  answer = original == " A中🙂 " and alias == original and normalized == "a中🙂" and replaced == "b中🙂" and text.upper(normalized) == "A中🙂" and text.at(normalized, 1) == maybe_text("中") and text.slice(normalized, 1, 3) == maybe_text("中🙂") and text.slice(normalized, 3, 1) == null and text.index_of(normalized, "🙂") == maybe_int(2) and text.index_of(normalized, "z") == null and text.contains(normalized, "中") and text.starts_with(normalized, "a") and text.ends_with(normalized, "🙂") and text.codepoint("🙂") == maybe_int(128578) and text.codepoint("ab") == null and text.from_codepoint(128578) == maybe_text("🙂") and text.from_codepoint(-1) == null and parts == ["中", "🙂"] and length(normalized) == 3 and "a" + "中" == "a中"
}
"#,
        );
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn decimal_numbers_parsing_rendering_and_ieee_intrinsics_execute() {
        boolean(
            r#"public main() (answer: Bool) {
  ratio = 7.0 / decimal.from_int(2)
  rounded = decimal.round(2.345, 2)
  nan = 0.0e0 / 0.0e0
  infinity = 1.0e0 / 0.0e0
  answer = ratio == 3.5 and ratio > 3.0 and ratio <= 3.5 and -ratio == -3.5 and 1.5 + 2.0 == 3.5 and 5.5 - 2.0 == 3.5 and 1.75 * 2.0 == 3.5 and rounded == 2.34 and decimal.parse("3.5") == maybe_decimal(ratio) and decimal.parse("invalid") == null and decimal.to_text(ratio) == "3.5" and int.parse("42") == maybe_int(42) and int.parse("invalid") == null and int.to_text(-42) == "-42" and float.parse("3.5") == maybe_float(3.5e0) and float.parse("invalid") == null and float.to_text(3.5e0) == "3.5" and float.from_int(2) == 2.0e0 and math.sqrt(4.0e0) == 2.0e0 and math.sin(0.0e0) == 0.0e0 and math.cos(0.0e0) == 1.0e0 and math.log(1.0e0) == 0.0e0 and math.pow(2.0e0, 3.0e0) == 8.0e0 and math.is_nan(nan) and math.is_infinite(infinity) and math.is_finite(3.5e0) and not (nan == nan)
}
"#,
        );
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn nested_records_choices_nullable_and_field_overwrites_keep_value_semantics() {
        boolean(
            r#"type Profile {
  name: Text
  aliases: List<Text>
}
type Selection {
  Found(value: Profile)
  Missing
}
read(value: Selection.Found(profile)) (answer: Profile) { answer = profile }
read(value: Selection.Missing) (answer: Profile) { answer = Profile { name = "missing"
aliases = [] } }
read(value: null) (answer: Profile) { answer = Profile { name = "absent"
aliases = [] } }
optional(value: Selection) (answer: Selection?) { answer = value }
public main() (answer: Bool) {
  original = Profile { name = "first"
aliases = ["a", "b"] }
  changed = original
  changed.name = "second"
  changed.aliases = append(changed.aliases, changed.name)
  wrapped = optional(Selection.Found(changed))
  selected = read(wrapped)
  answer = original.name == "first" and original.aliases == ["a", "b"] and changed.name == "second" and selected.aliases == ["a", "b", "second"] and read(null).name == "absent" and wrapped != optional(Selection.Missing)
}
"#,
        );
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn list_map_cow_order_and_supported_map_keys_execute() {
        boolean(
            r#"type Category { First
Second }
key(value: MapEntry<Text, List<Text>>) (answer: Text) { answer = value.key }
first_name(values: List<Text>) (answer: Text?) { answer = first(values) }
public main() (answer: Bool) {
  names = ["a", "b"]
  extended = append(names, "c")
  original = { "first" = names
"second" = extended }
  changed = put(original, "first", ["new"])
  removed = remove(changed, "second")
  reordered = { "second" = extended
"first" = names }
  flags = { true = "yes"
false = "no" }
  counts = { 7 = "seven"
9 = "nine" }
  categories = { Category.First = names
Category.Second = extended }
  identifier = dever.id.from_text("stable")
  identities = { identifier = names }
  nan = 0.0e0 / 0.0e0
  unordered = [nan]
  unordered_map = { "value" = nan }
  answer = names == ["a", "b"] and extended == ["a", "b", "c"] and get(original, "first") == maybe_list(names) and get(changed, "first") == maybe_list(["new"]) and get(removed, "second") == null and length(removed) == 1 and original != reordered and each(key, entries(original)) == ["first", "second"] and get(flags, false) == maybe_text("no") and get(counts, 7) == maybe_text("seven") and get(categories, Category.Second) == maybe_list(extended) and first(extended) == maybe_text("a") and first_name([]) == null and get(identities, dever.id.from_text("stable")) == maybe_list(names) and dever.id.to_text(identifier) == "stable" and unordered != unordered and unordered_map != unordered_map
}
"#,
        );
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn static_handler_traversals_and_bytes_execute_without_interpreting_values() {
        boolean(
            r#"add(value: Int, total: Int) (next: Int) { next = total + value }
until(value: Int, total: Int) (next: Int, stop: Bool) { next = total + value
stop = value == 2 }
increment(value: Int) (answer: Int) { answer = value + 1 }
even(value: Int) (answer: Bool) { answer = value % 2 == 0 }
label(value: Text, prefix: Text) (answer: Text) { answer = prefix + value }
forward(route: handler(value: Int) (answer: Int), values: List<Int>) (answer: List<Int>) { answer = each(route, values) }
total(values: List<Int>) (answer: Int) { answer = sum(values) }
public main() (answer: Bool) {
  bytes = dever.bytes.from_text("ABC")
  joined = dever.bytes.concat(bytes, dever.bytes.from_text("D"))
  labels = each(label, ["a", "b"], "prefix=")
  answer = reduce(add, [1, 2, 3], 0) == 6 and reduce(add, [], 7) == 7 and reduce_until(until, [1, 2, 100], 0) == 3 and sum([1, 2, 3]) == 6 and sum(increment, [1, 2, 3]) == 9 and total([]) == 0 and filter(even, [1, 2, 3, 4]) == [2, 4] and find(even, [1, 2, 3]) == maybe_int(2) and find(even, [1, 3]) == null and labels == ["prefix=a", "prefix=b"] and forward(increment, [1, 2]) == [2, 3] and reduce(add, bytes, 0) == 198 and each(increment, bytes) == [66, 67, 68] and dever.bytes.length(bytes) == 3 and dever.bytes.length(joined) == 4 and dever.bytes.at(joined, 3) == maybe_int(68) and dever.bytes.at(joined, -1) == null and bytes == dever.bytes.from_text("ABC")
}
"#,
        );
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn nullable_elements_flatten_and_unit_handlers_and_owned_reduce_state_release() {
        boolean(
            r#"present(value: Int) (answer: Bool) { answer = true }
present(value: null) (answer: Bool) { answer = false }
nonempty(value: Text) (answer: Bool) { answer = true }
nonempty(value: null) (answer: Bool) { answer = false }
first_optional(values: List<Text?>) (answer: Text?) { answer = first(values) }
ignore(value: Text) () {}
collect(value: Text, state: List<Text>) (next: List<Text>) { next = append(state, value) }
until_name(value: Text, state: List<Text>) (next: List<Text>, stop: Bool) {
  next = append(state, value)
  stop = value == "b"
}
public main() (answer: Bool) {
  each(ignore, ["discarded", "also-discarded"])
  collected = reduce(collect, ["a", "b"], [])
  stopped = reduce_until(until_name, ["a", "b", "unvisited"], [])
  labels = { "present" = maybe_text("value")
"absent" = null }
  answer = first_optional([null, "later"]) == null and first_optional(["first", null]) == maybe_text("first") and find(present, [null, 1, 2]) == maybe_int(1) and find(nonempty, [null, "found"]) == maybe_text("found") and get(labels, "present") == maybe_text("value") and get(labels, "absent") == null and get(labels, "missing") == null and collected == ["a", "b"] and stopped == ["a", "b"] and sum([1.25, 2.5]) == 3.75 and sum([1.0e16, 1.0e0, -1.0e16]) == 0.0e0
}
"#,
        );
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn traversal_failure_cleans_managed_arguments_results_and_error_paths() {
        execute(
            r#"fault(value: Text) (answer: Text) {
  prepared = [value, value + "alias"]
  zero = length(prepared) - 2
  answer = int.to_text(1 // zero)
}
public main() (answer: Bool) {
  mapped = each(fault, ["a", "b"])
  answer = length(mapped) == 2
}
"#,
            "{ i1 }",
            "  %failed = icmp eq i32 %status, 1
  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0
  %code = load i32, ptr %code_ptr
  %division = icmp eq i32 %code, 2
  %depth_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 2
  %depth = load i32, ptr %depth_ptr
  %called = icmp uge i32 %depth, 1
  %correct = and i1 %failed, %division
  %passed = and i1 %correct, %called",
        );
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn dynamic_map_duplicate_key_preserves_runtime_diagnostic_and_releases_it() {
        runtime_fault(
            r#"identity(value: Text) (answer: Text) { answer = value }
public main() (answer: Bool) {
  key = identity("same")
  values = { key = ["first"]
identity(key) = ["second"] }
  answer = length(values) == 1
}
"#,
            "duplicate Map key",
        );
    }

    #[test]
    #[ignore = "build runtime-abi-only staticlib in target/native-runtime-abi before running"]
    fn root_owned_outputs_are_transferred_and_released_by_the_caller() {
        execute(
            r#"public main() (label: Text, bytes: Bytes, names: List<Text>) {
  label = "root-owned"
  bytes = dever.bytes.from_text(label)
  names = [label, label]
}
"#,
            "{ ptr, ptr, ptr }",
            "  %okay = icmp eq i32 %status, 0
  %values = load { ptr, ptr, ptr }, ptr %out
  %label = extractvalue { ptr, ptr, ptr } %values, 0
  %bytes = extractvalue { ptr, ptr, ptr } %values, 1
  %names = extractvalue { ptr, ptr, ptr } %values, 2
  %label_ok = icmp ne ptr %label, null
  %bytes_ok = icmp ne ptr %bytes, null
  %names_ok = icmp ne ptr %names, null
  %first_ok = and i1 %label_ok, %bytes_ok
  %second_ok = and i1 %first_ok, %names_ok
  %passed = and i1 %okay, %second_ok",
        );
    }
}
