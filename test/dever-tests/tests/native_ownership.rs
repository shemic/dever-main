mod support;

#[test]
fn empty_byte_concatenation_reuses_storage_without_changing_value_aliases() {
    use dever_runtime::bytes::Bytes;
    let original = Bytes::from_text("abc");
    let empty = Bytes::new(Vec::new());
    let prefixed = empty.clone().concat(&original);
    let suffixed = original.clone().concat(&empty);
    assert_eq!(prefixed.values().as_ptr(), original.values().as_ptr());
    assert_eq!(suffixed.values().as_ptr(), original.values().as_ptr());
    let changed = prefixed.concat(&Bytes::from_text("d"));
    assert_eq!(changed.values(), b"abcd");
    assert_eq!(original.values(), b"abc");
    assert_eq!(suffixed.values(), b"abc");
}

#[test]
fn owned_bytes_transfer_text_storage_in_both_directions() {
    use dever_runtime::bytes::Bytes;

    let text = String::from("transfer");
    let allocation = text.as_ptr();
    let bytes = Bytes::from_string(text);
    assert_eq!(bytes.values().as_ptr(), allocation);
    let text = bytes.into_text().unwrap();
    assert_eq!(text.as_ptr(), allocation);
    assert_eq!(text, "transfer");

    let shared = Bytes::from_text("shared");
    assert_eq!(shared.clone().into_text().unwrap(), "shared");
    assert_eq!(shared.values(), b"shared");
}

#[test]
fn byte_slices_share_storage_and_consuming_iteration_preserves_aliases() {
    use dever_runtime::bytes::Bytes;
    let original = Bytes::from_text("abcdef");
    let slice = original.slice(2, 5).unwrap();
    assert_eq!(slice.values().as_ptr(), original.values()[2..].as_ptr());
    assert_eq!(slice.clone().into_values().collect::<Vec<_>>(), b"cde");
    assert_eq!(slice.concat(&Bytes::from_text("x")).values(), b"cdex");
    assert_eq!(original.values(), b"abcdef");
    assert_eq!(
        Bytes::from_text("xyz").into_values().collect::<Vec<_>>(),
        b"xyz"
    );
}

#[test]
fn scalar_reads_after_record_transfer_preserve_old_values_and_reassignments() {
    let source = r#"type State { text: Text
count: Int }
append_text(state: State) (next: State) { next = state
next.text = state.text + "x"
next.count = 99 }
step(value: Int, state: State) (next: State) {
  next = append_text(state)
  next.count = state.count + 1
}
public main() (text: Text, count: Int, changed: Int) {
  state = State { text = "a"
count = 0 }
  final = reduce(step, [1, 2, 3], state)
  text = final.text
  count = final.count
  copied = append_text(state)
  state.count = 7
  changed = state.count
}
"#;
    assert_eq!(
        support::stdout(source),
        "text = axxx\ncount = 3\nchanged = 7\n"
    );
}

#[test]
fn total_collection_stages_fuse_but_faulting_stages_keep_eager_order() {
    let source = r#"positive(value: Float) (result: Bool) { result = value > 0e0 }
twice(value: Float) (result: Float) { result = value * 2e0 }
public main() (result: Float) {
  mapped = each(twice, [1e0, -2e0, 3e0])
  selected = filter(positive, mapped)
  result = sum(selected)
}
"#;
    let sources = support::sources(source);
    let generated =
        dever_core::native::emit(&support::checked(&sources), &sources, "main.main").unwrap();
    assert!(generated.contains("checked total handler"));
    assert_eq!(support::stdout(source), "result = 8\n");

    let faulting = r#"increment(value: Int) (result: Int) pure { result = value + 1 }
observe(value: Int) () { dever.io.println(int.to_text(value)) }
public main() () { each(observe, each(increment, [1, 9223372036854775807])) }
"#;
    let output = support::run(faulting);
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "mapping must finish before any observation"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("overflow"));
}

#[test]
fn record_updates_preserve_old_fields_aliases_and_expression_order() {
    let source = r#"type Pair { first: Text
second: Text }
grow(value: Int, state: Pair) (next: Pair) {
  next = state
  next.first = state.first + "x"
  next.second = state.second + "y"
}

old_field(state: Pair) (next: Pair) {
  next = state
  next.first = "changed"
  next.second = state.first
}
echo(value: Text) (result: Text) { dever.io.println(value)
result = value }
public main() (grown: Text, original: Text, old: Text, repeated: Text, siblings: Text) {
  pair = Pair { first = "a"
second = "b" }
  alias = pair
  updated = reduce(grow, [1, 2, 3], pair)
  grown = updated.first + updated.second
  original = alias.first + alias.second
  changed = old_field(alias)
  old = changed.first + changed.second
  pair.first = echo(pair.first) + echo(pair.first)
  repeated = pair.first
  pair.first = pair.second + pair.first
  siblings = pair.first + pair.second
}
"#;
    assert_eq!(
        support::stdout(source),
        "a\na\ngrown = axxxbyyy\noriginal = ab\nold = changeda\nrepeated = aa\nsiblings = baab\n"
    );
}

#[test]
fn consumed_lists_move_unique_elements_and_clone_shared_elements_only_on_pull() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Counted(Arc<AtomicUsize>);
    impl Clone for Counted {
        fn clone(&self) -> Self {
            self.0.fetch_add(1, Ordering::SeqCst);
            Self(self.0.clone())
        }
    }
    let copies = Arc::new(AtomicUsize::new(0));
    let make =
        || dever_runtime::collections::List::new((0..8).map(|_| Counted(copies.clone())).collect());
    let owned = make();
    assert_eq!(owned.into_values().count(), 8);
    assert_eq!(copies.load(Ordering::SeqCst), 0);
    let shared = make();
    let mut consumed = shared.clone().into_values();
    assert!(consumed.next().is_some());
    drop(consumed);
    assert_eq!(copies.load(Ordering::SeqCst), 1);
    assert_eq!(shared.values().len(), 8);
}

#[test]
fn consuming_collection_lookups_move_unique_values_and_preserve_aliases() {
    use dever_runtime::collections::{List, Map};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Counted(Arc<AtomicUsize>);
    impl Clone for Counted {
        fn clone(&self) -> Self {
            self.0.fetch_add(1, Ordering::SeqCst);
            Self(Arc::clone(&self.0))
        }
    }

    let copies = Arc::new(AtomicUsize::new(0));
    let counted = || Counted(Arc::clone(&copies));

    assert!(List::new(vec![counted()]).into_first().is_some());
    assert_eq!(copies.load(Ordering::SeqCst), 0);
    let shared_list = List::new(vec![counted()]);
    assert!(shared_list.clone().into_first().is_some());
    assert_eq!(copies.load(Ordering::SeqCst), 1);
    assert_eq!(shared_list.values().len(), 1);

    let key = "key".to_owned();
    let owned_map = Map::new(vec![(key.clone(), counted())]).unwrap();
    assert!(owned_map.into_get(&key).is_some());
    assert_eq!(copies.load(Ordering::SeqCst), 1);
    let shared_map = Map::new(vec![(key.clone(), counted())]).unwrap();
    assert!(shared_map.clone().into_get(&key).is_some());
    assert_eq!(copies.load(Ordering::SeqCst), 2);
    assert!(shared_map.get(&key).is_some());
    assert_eq!(copies.load(Ordering::SeqCst), 3);

    let entries = Map::new(vec![(key, counted())]).unwrap().into_entries();
    assert_eq!(entries.values().len(), 1);
    assert_eq!(copies.load(Ordering::SeqCst), 3);
}

#[test]
fn direct_split_consumption_preserves_unicode_empty_pieces_and_early_stop() {
    let source = r#"join(value: Text, state: Text) (next: Text) { next = state + "[" + value + "]" }
stop(value: Text, state: Text) (next: Text, stop: Bool) {
  dever.io.println(value)
  next = state + value
  stop = value == "好"
}
public main() (unicode: Text, empty: Text, separated: Text, stopped: Text, alias: List<Text>) {
  unicode = reduce(join, text.split("你好🙂", ""), "")
  empty = reduce(join, text.split("", ""), "seed")
  separated = reduce(join, text.split(",a,,", ","), "")
  stopped = reduce_until(stop, text.split("你好🙂", ""), "")
  parts = text.split("ab", "")
  consumed = reduce(join, parts, "")
  alias = parts
}
"#;
    let sources = support::sources(source);
    let generated =
        dever_core::native::emit(&support::checked(&sources), &sources, "main.main").unwrap();
    assert!(generated.contains("dever_runtime::text::pieces("));
    assert_eq!(
        support::stdout(source),
        "你\n好\nunicode = [你][好][🙂]\nempty = seed\nseparated = [][a][][]\nstopped = 你好\nalias = [a, b]\n"
    );
}

#[test]
fn complete_decimal_domain_needs_no_runtime_range_comparison() {
    let sources = support::sources(
        "add(value: Int, total: Decimal) (next: Decimal) { next = total + value }\npublic main() (result: Decimal) { result = reduce(add, dever.bytes.from_text(\"abc\"), 0.0) }",
    );
    let generated =
        dever_core::native::emit(&support::checked(&sources), &sources, "main.main").unwrap();
    assert!(
        !generated.contains(">= DecimalValue") && !generated.contains("<= DecimalValue"),
        "{generated}"
    );
    assert_eq!(support::stdout(sources.files()[0].text()), "result = 294\n");
}

#[test]
fn moved_inputs_keep_live_aliases_and_partial_assignments_independent() {
    assert_eq!(
        support::stdout(
            r#"type Pair {
  first: Text
  second: Text
}
copy(value: Text) (answer: Text) { answer = value }
grow(value: Int, state: List<Int>) (next: List<Int>) { next = append(state, value) }
join(left: Text, right: Text) (answer: Text) { answer = left + right }
public main() (original: Text, changed: Text, repeated: Text, before: List<Int>, after: List<Int>, preserved: Text) {
  pair = Pair {
    first = copy("a")
    second = copy("b")
  }
  alias = pair
  pair.first = pair.second + pair.first
  original = alias.first + alias.second
  changed = pair.first + pair.second
  value = copy("x")
  repeated = join(value, value)
  initial = [0]
  after = reduce(grow, [1, 2, 3], initial)
  before = initial
  record = Pair {
    first = copy("f")
    second = copy("s")
  }
  field = record.first
  preserved = record.second + field
}
"#
        ),
        "original = ab\nchanged = bab\nrepeated = xx\nbefore = [0]\nafter = [0, 1, 2, 3]\npreserved = sf\n"
    );
}

#[test]
fn reduction_state_is_transferred_and_unique_values_are_not_cloned() {
    let sources = support::sources(
        r#"grow(value: Int, state: List<Int>) (next: List<Int>) { next = append(state, value) }
public main() (answer: List<Int>) { answer = reduce(grow, [1, 2, 3], []) }
"#,
    );
    let generated =
        dever_core::native::emit(&support::checked(&sources), &sources, "main.main").unwrap();
    assert!(!generated.contains("state.clone()"), "{generated}");
    assert!(!generated.contains("_p1.clone()"), "{generated}");
    assert!(!generated.contains("_v1.clone()"), "{generated}");
    assert_eq!(
        support::stdout(sources.files()[0].text()),
        "answer = [1, 2, 3]\n"
    );
}
