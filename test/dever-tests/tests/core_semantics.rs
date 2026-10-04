mod support;

use support::{checked, rejected, sources};

#[test]
fn full_language_contract_package_checks_with_the_official_sources() {
    let prd = include_str!("../../../.trellis/tasks/09-04-dever-language-mvp/prd.md");
    let source = prd
        .split_once("## Complete Package Example\n")
        .unwrap()
        .1
        .split_once("```dever\n")
        .unwrap()
        .1
        .split_once("\n```")
        .unwrap()
        .0;
    let mut sources = dever_core::source::SourceMap::default();
    sources.add("account/user.dever", source);
    checked(&sources);
    assert_eq!(sources.files().len(), 1);
}

#[test]
fn clause_domains_are_disjoint_exhaustive_and_order_independent() {
    for clauses in [
        "choose(value: true) (answer: Int) { answer = 1 }\nchoose(value: false) (answer: Int) { answer = 2 }",
        "choose(value: false) (answer: Int) { answer = 2 }\nchoose(value: true) (answer: Int) { answer = 1 }",
        "choose(value: Text) (answer: Int) { answer = 1 }\nchoose(value: null) (answer: Int) { answer = 0 }",
        "choose(value: Int < 0) (answer: Int) { answer = -1 }\nchoose(value: Int >= 0) (answer: Int) { answer = 1 }",
        "choose(value: Decimal < 0.0) (answer: Int) { answer = -1 }\nchoose(value: other) (answer: Int) { answer = 1 }",
        "choose(value: \"yes\") (answer: Int) { answer = 1 }\nchoose(value: other) (answer: Int) { answer = 0 }",
        "choose(value: 1.0e0) (answer: Int) { answer = 1 }\nchoose(value: other) (answer: Int) { answer = 0 }",
    ] {
        checked(&sources(clauses));
    }
    for (clauses, message) in [
        ("choose(value: true) () {}", "do not cover"),
        (
            "choose(value: Bool) () {}\nchoose(value: true) () {}",
            "overlap",
        ),
        (
            "choose(value: true) () {}\nchoose(value: false) () {}\nchoose(value: other) () {}",
            "unreachable",
        ),
        (
            "choose(value: Text?) () {}\nchoose(value: null) () {}",
            "overlap",
        ),
        (
            "choose(value: null) () {}\nchoose(value: other) () {}",
            "cannot infer",
        ),
        ("choose(value: Int > 0 and < 1) () {}", "unreachable"),
        (
            "choose(value: Int > 9223372036854775807) () {}",
            "unreachable",
        ),
        (
            "choose(value: Decimal > 1.0 and < 1.000000000000000000000000000000001) () {}",
            "unreachable",
        ),
        ("choose(value: Float > 0.0) () {}", "range bounds"),
        ("choose(value: Int > 0 and > 1) () {}", "one lower"),
        (
            "choose(left: true, right: Bool) () {}\nchoose(left: other, right: false) () {}",
            "every input combination",
        ),
        ("choose(left: Bool, right: other) () {}", "cannot infer"),
        ("choose() () {}\nchoose() () {}", "overlap"),
        (
            "choose(value: true) (answer: Int) { answer = 1 }\nchoose(value: false) (different: Int) { different = 2 }",
            "identical ordered output",
        ),
    ] {
        rejected(clauses, message);
    }
}

#[test]
fn four_quadrants_cover_a_multi_input_product() {
    checked(&sources(
        "public choose(a: true, b: true) () {}\npublic choose(a: true, b: other) () {}\npublic choose(a: other, b: true) () {}\npublic choose(a: other, b: other) () {}",
    ));
}

#[test]
fn construction_initialization_and_stable_types_are_checked() {
    let prefix =
        "type User { name: Text\nphone: Text? }\ntype Result { Found(user: User)\nMissing }\n";
    for (body, message) in [
        ("main() () { user = User { name = \"a\" } }", "every field"),
        (
            "main() () { user = User { name = \"a\"\nphone = null\nextra = 1 } }",
            "unknown constructor field",
        ),
        (
            "main() () { user = User { name = \"a\"\nname = \"b\"\nphone = null } }",
            "duplicate constructor",
        ),
        ("main() (user: User) { user.name = \"a\" }", "uninitialized"),
        ("main() (answer: Int) {}", "uninitialized"),
        ("main() () { value = null }", "nullable type context"),
        ("main() () { value = 1\nvalue = \"a\" }", "expected Int"),
        ("main() () { value = Result.Found() }", "payload arity"),
        ("main() () { value = Result.Missing(1) }", "payload arity"),
        ("main() () { values = [] }", "type context"),
        ("main() () { labels = {} }", "type context"),
        ("main() () { values = [1, true] }", "one stable type"),
        ("main() () { values = { 1.5 = 1 } }", "Map key"),
        (
            "main() () { values = { \"a\" = 1\n\"a\" = 2 } }",
            "duplicate Map",
        ),
        (
            "main() (a: Int, b: Int) { a = 1\nb = 2 }\ninvalid() () { values = [main()] }",
            "multi-output",
        ),
        (
            "main() (a: Int, b: Int) { a = 1\nb = 2 }\ninvalid() (same: Bool) { same = main() == main() }",
            "comparable",
        ),
    ] {
        rejected(&format!("{prefix}{body}"), message);
    }
}

#[test]
fn scalar_rules_and_static_faults_are_enforced() {
    for (expression, message) in [
        ("9223372036854775808", "64-bit"),
        ("-9223372036854775809", "64-bit"),
        ("0.12345678901234567890123456789012345", "34 digits"),
        ("9223372036854775807 + 1", "overflow"),
        ("-9223372036854775808 // -1", "overflow"),
        ("1 // 0", "division by zero"),
        ("1 / 0", "division by zero"),
        ("1 == 1.0", "does not promote"),
        ("1 + 1.0e0", "cannot implicitly mix"),
        ("1.0 // 2", "require Int"),
        ("1 and true", "expected Bool"),
    ] {
        rejected(&format!("main() () {{ value = {expression} }}"), message);
    }
    checked(&sources(
        "public main() (min: Int, exact: Bool, quotient: Decimal, float: Float) { min = -9223372036854775808\nexact = 0.1 + 0.2 == 0.3\nquotient = 5 / 2\nfloat = 1.5 + 2.5 }",
    ));
}

#[test]
fn graphs_cover_type_references_and_collection_handlers() {
    rejected("type Recursive { next: Recursive? }", "recursive type");
    rejected("type A { b: B }\ntype B { a: List<A> }", "recursive type");
    rejected(
        "repeat_value(value: Int) (answer: Int) { values = each(repeat_value, [value])\nanswer = sum(values) }",
        "recursive function",
    );
    rejected(
        "f(value: true) () {}\ng() () {}\nf(value: false) () {}",
        "contiguous",
    );
    let mut sources = dever_core::source::SourceMap::default();
    sources.add(
        "a.dever",
        "public type A { value: Int }\nstart() () { b.run() }\nfinish() () {}",
    );
    sources.add("b.dever", "public run() () {}\nunused(value: a.A) () {}");
    assert!(
        dever_core::check(&sources)
            .unwrap_err()
            .iter()
            .any(|error| error.message.contains("cyclic package"))
    );
}

#[test]
fn invalid_handlers_are_rejected_before_build() {
    for (declarations, call, message) in [
        (
            "handler(value: Int) (a: Int, b: Int) { a = value\nb = value }",
            "each(handler, [1])",
            "invalid outputs",
        ),
        (
            "handler(value: Int) (value: Int) { value = 1 }",
            "filter(handler, [1])",
            "duplicate local",
        ),
        (
            "handler(value: Int) (answer: Int) { answer = value }",
            "filter(handler, [1])",
            "invalid outputs",
        ),
        (
            "handler(value: Int) () {}",
            "sum(handler, [1])",
            "invalid outputs",
        ),
        (
            "handler(value: Int) () {}",
            "each(handler, [1], 2)",
            "unknown function",
        ),
    ] {
        rejected(
            &format!("{declarations}\nmain() () {{ result = {call} }}"),
            message,
        );
    }
}

#[test]
fn unused_writes_are_warnings_not_errors() {
    let sources = sources("public main() () { value = 1\nvalue = 2 }");
    let program = checked(&sources);
    assert_eq!(
        program
            .warnings()
            .iter()
            .filter(|warning| warning.code == "W001")
            .count(),
        2
    );
    assert!(
        program.warnings()[0]
            .render(&sources)
            .contains("warning[W001]")
    );
}

#[test]
fn known_scalar_and_choice_keys_are_rejected_when_duplicated() {
    for body in [
        "labels = { Key.First = 1\nKey.First = 2 }",
        "key = Key.First\nlabels = { key = 1\nKey.First = 2 }",
        "key = \"same\"\nlabels = { key = 1\n\"same\" = 2 }",
    ] {
        rejected(
            &format!("type Key {{ First\nSecond }}\nmain() () {{ {body} }}"),
            "duplicate Map literal key",
        );
    }
    rejected(
        "invalid(key: Id) () { labels = { key = 1\nkey = 2 } }",
        "duplicate Map literal key",
    );
}

#[test]
fn known_numeric_faults_include_local_constants_and_constant_divisors() {
    rejected(
        "invalid(value: Int) (answer: Int) { answer = value // 0 }",
        "division by zero",
    );
    rejected(
        "invalid(value: Decimal) (answer: Decimal) { zero = 0\nanswer = value / zero }",
        "division by zero",
    );
    rejected(
        "invalid() () { value = 9223372036854775807\nanswer = value + 1 }",
        "overflow",
    );
}

#[test]
fn literal_clause_inference_is_stable_under_permutation() {
    for clauses in [
        "choose(value: 1) () {}\nchoose(value: 2.0) () {}\nchoose(value: other) () {}",
        "choose(value: 2.0) () {}\nchoose(value: other) () {}\nchoose(value: 1) () {}",
    ] {
        checked(&sources(clauses));
    }
}

#[test]
fn names_follow_the_existing_language_conventions() {
    rejected(
        "package App exposes ()",
        "package/exposes declarations were removed",
    );
    for source in [
        "type user { name: Text }",
        "Main() () {}",
        "main() () { bad__name = 1 }",
    ] {
        rejected(source, "ASCII");
    }
}

#[test]
fn float_literal_context_never_retypes_existing_numeric_values() {
    checked(&sources(
        "compare(value: Float) (answer: Bool) { answer = value < 0.0 or 0.0 == value or (-0.0) >= value }",
    ));
    rejected(
        "compare(value: Float) (answer: Bool) { decimal = 0.0\nanswer = value == decimal }",
        "same comparable type",
    );
    rejected(
        "compare(value: Float) (answer: Bool) { answer = value == 0 }",
        "same comparable type",
    );
    rejected(
        "invalid() () { decimal = 1.0\nvalue = decimal + 1.0e0 }",
        "cannot implicitly mix",
    );
}

#[test]
fn reduce_bytes_stream_and_static_handler_contracts_are_checked() {
    checked(&sources(
        r#"add(value: Int, total: Int) (next: Int) { next = total + value }
until(value: Int, total: Int) (next: Int, stop: Bool) { next = total + value
stop = value == 2 }
show(value: Int) () {}
apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }
forward(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = apply(route, value) }
consume(stream: Stream<Int>) (total: Int) { each(show, stream)
total = 0 }
main() () { total = reduce(add, [1, 2], 0)
partial = reduce_until(until, dever.bytes.from_text("abc"), 0) }"#,
    ));

    for (source, message) in [
        (
            "map(value: Int) (answer: Int) { answer = value }\nbad(stream: Stream<Int>) () { values = each(map, stream) }",
            "each on a stream",
        ),
        (
            "bad(value: Int, state: Int) (next: Int, done: Bool) { next = state\ndone = true }\nmain() () { value = reduce_until(bad, [1], 0) }",
            "stop: Bool",
        ),
        (
            "apply(route: handler(value: Int) (answer: Int)) (answer: Int) { answer = route }",
            "not runtime values",
        ),
        (
            "text(value: Text) (answer: Text) { answer = value }\napply(route: handler(value: Int) (answer: Int)) (answer: Int) { answer = route(1) }\nmain() () { answer = apply(text) }",
            "signature does not match",
        ),
        (
            "wrong(value: Int) (result: Int) { result = value }\napply(route: handler(value: Int) (answer: Int)) (answer: Int) { answer = route(1) }\nmain() () { answer = apply(wrong) }",
            "signature does not match",
        ),
        (
            "apply(route: handler(value: Int) (answer: Int)) () { values = [route] }",
            "not runtime values",
        ),
        (
            "apply(route: handler(value: Int) (answer: Int)) (answer: Bool) { answer = route == route }",
            "not runtime values",
        ),
        (
            "loop(next: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = next(value) }\nagain(value: Int) (answer: Int) { answer = loop(again, value) }",
            "specialized handler",
        ),
    ] {
        rejected(source, message);
    }
}

#[test]
fn handler_functions_can_cross_packages_and_forward_statically() {
    let mut sources = dever_core::source::SourceMap::default();
    sources.add(
        "worker.dever",
        "public double(value: Int) (answer: Int) { answer = value * 2 }",
    );
    sources.add(
        "main.dever",
        "apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }\nforward(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = apply(route, value) }\npublic main() (answer: Int) { answer = forward(worker.double, 3) }",
    );
    checked(&sources);
}

#[test]
fn handler_signatures_and_streams_reuse_value_type_restrictions() {
    for source in [
        "apply(route: handler(value: Map<List<Int>, Int>) ()) () {}",
        "apply(route: handler() (value: Map<List<Int>, Int>)) () {}",
        "bad(values: Map<Stream<Int>, Int>) () {}",
    ] {
        rejected(source, "invalid Map key type");
    }
    rejected(
        "bad(stream: Stream) () {}",
        "incorrect number of type arguments",
    );
    rejected(
        "bad(stream: Stream<Int>) (same: Bool) { same = stream == stream }",
        "same comparable type",
    );
    rejected(
        "bad(values: List<Int>) () { close(values) }",
        "close requires a Stream",
    );
}
