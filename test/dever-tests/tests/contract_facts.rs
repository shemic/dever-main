use dever_core::diagnostic::Diagnostic;
use dever_core::source::SourceMap;

fn checked(declarations: &str) -> Result<dever_core::hir::Program, Vec<Diagnostic>> {
    let mut sources = SourceMap::default();
    sources.add("main.dever", declarations);
    dever_core::check(&sources)
}

fn accepts(declarations: &str) {
    checked(declarations).unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"));
}

fn rejects(declarations: &str) {
    let errors = checked(declarations).expect_err("constraint proof must fail");
    assert!(
        errors.iter().any(|error| error.code == "C011"),
        "{errors:#?}"
    );
}

#[test]
fn constrained_construction_requires_a_proof_even_in_unused_functions() {
    rejects(
        "type Port { value: Int >= 1 and <= 65535 }\nbad() (port: Port) { port = Port { value = 0 } }",
    );
    rejects(
        "type Port { value: Int >= 1 and <= 65535 }\nbad(value: Int) (port: Port) { port = Port { value = value } }",
    );
    accepts(
        "type Port { value: Int >= 1 and <= 65535 }\nmake() (port: Port) { port = Port { value = 8080 } }",
    );
}

#[test]
fn clause_domains_establish_safe_construction_and_arithmetic() {
    accepts(
        "type Port { value: Int >= 1 and <= 65535 }\nmake(value: Int >= 1 and <= 65534) (port: Port) { port = Port { value = value + 1 } }\nmake(value: other) (port: Port) { port = Port { value = 8080 } }",
    );
    rejects(
        "type Port { value: Int >= 1 and <= 65535 }\nmake(value: Int >= 1 and <= 65535) (port: Port) { port = Port { value = value + 1 } }\nmake(value: other) (port: Port) { port = Port { value = 8080 } }",
    );
}

#[test]
fn constrained_outputs_and_input_transfer_survive_function_boundaries() {
    accepts(
        "type Port { value: Int >= 1 and <= 65535 }\nidentity(value: Int) (result: Int) { result = value }\nnumber(port: Port) (value: Int >= 1 and <= 65535) { value = identity(port.value) }\nmake(port: Port) (result: Port) { result = Port { value = number(port) } }",
    );
    rejects("bad(value: Int) (result: Int >= 1) { result = value }");
    accepts("valid() (result: Int >= 1) { result = 1 }");
}

#[test]
fn choice_payload_constraints_apply_at_construction_and_binding() {
    rejects(
        "type Result { Ready(value: Int >= 1) }\nbad() (result: Result) { result = Result.Ready(0) }",
    );
    accepts(
        "type Result { Ready(value: Int >= 1) }\nread(result: Result.Ready(value)) (number: Int >= 1) { number = value }",
    );
}

#[test]
fn nested_mutation_rechecks_constraints_and_aliases_keep_independent_facts() {
    rejects(
        "type Port { value: Int >= 1 }\ntype Config { port: Port }\nbad(config: Config) (result: Config) { config.port.value = 0\nresult = config }",
    );
    accepts(
        "type Box { value: Int }\nmake() (result: Int >= 1) { original = Box { value = 1 }\nalias = original\noriginal.value = 0\nresult = alias.value }",
    );
    rejects(
        "type Box { value: Int }\nmake() (result: Int >= 1) { original = Box { value = 1 }\noriginal.value = 0\nresult = original.value }",
    );
}

#[test]
fn reassignment_removes_previous_range_guarantees() {
    rejects(
        "type Port { value: Int >= 1 }\nmake(input: Int) (port: Port) { value = 1\nvalue = input\nport = Port { value = value } }",
    );
    rejects(
        "type Box { value: Int }\nmake(box: Box) (result: Int >= 1) { box.value = 0\nresult = box.value }",
    );
}

#[test]
fn decimal_bounds_reuse_checked_numeric_semantics() {
    accepts(
        "type Amount { value: Decimal >= 0.0 and <= 10.0 }\nmake(value: Decimal >= 0.0 and <= 9.0) (result: Amount) { result = Amount { value = value + 1.0 } }\nmake(value: other) (result: Amount) { result = Amount { value = 0.0 } }",
    );
    rejects(
        "type Amount { value: Decimal >= 0.0 }\nmake(value: Decimal) (result: Amount) { result = Amount { value = value } }",
    );
}

#[test]
fn handler_inputs_use_declared_bounds_including_sequence_adapters() {
    accepts(
        "call(step: handler(value: Int >= 1) (result: Int >= 1)) (result: Int >= 1) { result = step(1) }",
    );
    rejects(
        "call(step: handler(value: Int >= 1) (result: Int)) (result: Int) { result = step(0) }",
    );
    rejects("call(step: handler(value: Int >= 1) ()) () { each(step, [0]) }");
}

#[test]
fn proven_comparisons_and_call_domains_have_evidence_without_rejecting_callees() {
    let program = checked("type Port { value: Int >= 1 }\npositive(value: Int >= 1) (result: Bool) { result = true }\npositive(value: other) (result: Bool) { result = false }\ncheck(port: Port) (result: Bool) { compared = port.value < 0\nresult = positive(port.value) and compared }").unwrap();
    let warnings: Vec<_> = program
        .warnings()
        .iter()
        .filter(|warning| warning.code == "W002")
        .collect();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.message.contains("comparison"))
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.message.contains("cannot run"))
    );
    assert!(warnings.iter().all(|warning| !warning.related.is_empty()));
    accepts(
        "positive(value: Int >= 1) (result: Bool) { result = true }\npositive(value: other) (result: Bool) { result = false }",
    );
}

#[test]
fn constant_calls_and_unknown_comparisons_do_not_create_redundancy_noise() {
    let mut sources = SourceMap::default();
    let id = sources.add("main.dever", "positive(value: Int >= 1) (result: Bool) { result = true }\npositive(value: other) (result: Bool) { result = false }\nmain(value: Int) (result: Bool) { known = positive(1)\nresult = value < 0 and known }");
    let program = dever_core::check(&sources).unwrap();
    assert!(
        !program
            .warnings()
            .iter()
            .any(|warning| warning.code == "W002" && warning.primary.source == id)
    );
}

#[test]
fn bool_null_and_choice_facts_reach_call_sites() {
    let mut sources = SourceMap::default();
    let id = sources.add("main.dever", "type Choice { Yes\nNo }\ntype State { enabled: Bool\nchoice: Choice\noptional: Int? }\nflag(value: true) () {}\nflag(value: false) () {}\noption(value: Int) () {}\noption(value: null) () {}\nchoose(value: Choice.Yes) () {}\nchoose(value: Choice.No) () {}\nmain() () { state = State { enabled = true\nchoice = Choice.Yes\noptional = null }\nflag(state.enabled)\noption(state.optional)\nchoose(state.choice) }");
    let program = dever_core::check(&sources).unwrap();
    let warnings: Vec<_> = program
        .warnings()
        .iter()
        .filter(|warning| warning.code == "W002" && warning.primary.source == id)
        .collect();
    assert_eq!(warnings.len(), 3);
}

#[test]
fn arithmetic_does_not_assume_wrapping_or_unproved_division_bounds() {
    rejects(
        "type Positive { value: Int >= 1 }\nmake(input: Int) (result: Positive) { result = Positive { value = input + 1 } }",
    );
    rejects(
        "type Positive { value: Int >= 1 }\nmake(input: Int) (result: Positive) { result = Positive { value = input // 2 } }",
    );
}

#[test]
fn concrete_handler_outputs_are_proved_from_bodies_without_wrapper_annotations() {
    accepts(
        "apply(step: handler(value: Int >= 1) (result: Int >= 1)) (result: Int >= 1) { result = step(1) }\nidentity(value: Int) (result: Int) { result = value }\nmain() (result: Int >= 1) { result = apply(identity) }",
    );
    accepts(
        "apply(step: handler(value: Int >= 1) (result: Int >= 1)) (result: Int >= 1) { result = step(1) }\ngood(value: Int) (result: Int) { result = 2 }\nmain() (result: Int >= 1) { result = apply(good) }",
    );
    rejects(
        "apply(step: handler(value: Int >= 1) (result: Int >= 1)) (result: Int >= 1) { result = step(1) }\nbad(value: Int) (result: Int) { result = 0 }\nmain() (result: Int >= 1) { result = apply(bad) }",
    );
}

#[test]
fn forwarded_handler_inputs_are_contravariant_and_outputs_covariant() {
    accepts(
        "apply(step: handler(value: Int >= 1) (result: Int >= 1)) (result: Int >= 1) { result = step(1) }\nforward(step: handler(value: Int >= 0) (result: Int >= 2)) (result: Int >= 1) { result = apply(step) }",
    );
    for signature in [
        "handler(value: Int >= 2) (result: Int >= 1)",
        "handler(value: Int >= 0) (result: Int >= 0)",
    ] {
        let source = format!(
            "apply(step: handler(value: Int >= 1) (result: Int >= 1)) (result: Int >= 1) {{ result = step(1) }}\nforward(step: {signature}) (result: Int >= 1) {{ result = apply(step) }}"
        );
        let errors = checked(&source).expect_err("unsafe handler variance");
        assert!(
            errors.iter().any(|error| error.code == "C005"),
            "{errors:#?}"
        );
    }
}

#[test]
fn shared_value_graphs_keep_linear_facts_and_independent_mutation_snapshots() {
    const DEPTH: usize = 24;
    let mut declarations = String::from("type Layer0 { value: Int >= 1 }\n");
    for level in 1..=DEPTH {
        declarations.push_str(&format!(
            "type Layer{level} {{ left: Layer{}\nright: Layer{} }}\n",
            level - 1,
            level - 1
        ));
    }
    for (flag, initial) in [("true", 1), ("false", 2)] {
        declarations.push_str(&format!(
            "build(flag: {flag}) (result: Layer{DEPTH}) {{\nv0 = Layer0 {{ value = {initial} }}\n"
        ));
        for level in 1..=DEPTH {
            declarations.push_str(&format!(
                "v{level} = Layer{level} {{ left = v{}\nright = v{} }}\n",
                level - 1,
                level - 1
            ));
        }
        declarations.push_str(&format!("result = v{DEPTH}\n}}\n"));
    }
    let path = format!("{}value", "left.".repeat(DEPTH));
    declarations.push_str(&format!("main() (result: Int >= 1 and <= 2) {{ value = build(true)\nalias = value\nvalue.{path} = 3\nresult = alias.{path}\n}}"));
    accepts(&declarations);
    let mutated = declarations.replace(
        &format!("result = alias.{path}"),
        &format!("result = value.{path}"),
    );
    rejects(&mutated);
}

#[test]
fn excessive_fact_work_is_an_explicit_diagnostic() {
    // A broad but shallow source exercises the budget without allocating an
    // exponentially nested value or risking the host process's memory.
    let mut declarations = String::from("main(input: Int) (result: Int) {\n");
    for _ in 0..5500 {
        declarations.push_str("result = input + 1\n");
    }
    declarations.push('}');
    let errors = checked(&declarations).expect_err("proof budget must be enforced");
    assert!(
        errors.iter().any(|error| error.code == "C015"),
        "{errors:#?}"
    );
}
