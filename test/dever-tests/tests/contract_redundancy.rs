mod support;

use support::{checked, sources};

fn warnings(source: &str, code: &str) -> Vec<dever_core::diagnostic::Diagnostic> {
    let sources = sources(source);
    let user_source = sources.files()[0].id();
    checked(&sources)
        .warnings()
        .iter()
        .filter(|warning| warning.code == code && warning.primary.source == user_source)
        .cloned()
        .collect()
}

#[test]
fn identical_private_implementations_are_reported_with_source_evidence() {
    let source = "first(value: Int) (answer: Int) { answer = value + 1 }\nsecond(renamed: Int) (answer: Int) { answer = renamed + 1 }";
    let duplicates = warnings(source, "W003");
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0].related.len(), 1);
    assert!(duplicates[0].message.contains("main.first"));
    assert!(duplicates[0].message.contains("main.second"));
    assert_ne!(duplicates[0].primary, duplicates[0].related[0].span);
}

#[test]
fn visibility_contract_and_operator_differences_are_not_duplicates() {
    for source in [
        "public first(value: Int) (answer: Int) { answer = value + 1 }\nsecond(value: Int) (answer: Int) { answer = value + 1 }",
        "first(value: Int) (answer: Int) pure { answer = value + 1 }\nsecond(value: Int) (answer: Int) { answer = value + 1 }",
        "first(value: Int) (answer: Int) { answer = value + 1 }\nsecond(value: Int) (answer: Int) { answer = value - 1 }",
        "first(value: Int) (answer: Int) { answer = value }\nsecond(value: Int) (different: Int) { different = value }",
    ] {
        assert!(warnings(source, "W003").is_empty(), "{source}");
    }
}

#[test]
fn repeated_pure_calls_require_unchanged_inputs_and_an_available_result() {
    let prefix = "calculate(value: Int) (answer: Int) { answer = value + 1 }\n";
    let repeated = format!(
        "{prefix}main(value: Int) (answer: Int) {{ first = calculate(value)\nsecond = calculate(value)\nanswer = first + second }}"
    );
    let diagnostics = warnings(&repeated, "W004");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].related.len(), 1);
    for body in [
        "first = calculate(value)\nvalue = value + 1\nsecond = calculate(value)\nanswer = first + second",
        "first = calculate(value)\nfirst = 0\nsecond = calculate(value)\nanswer = first + second",
    ] {
        assert!(
            warnings(
                &format!("{prefix}main(value: Int) (answer: Int) {{ {body} }}"),
                "W004"
            )
            .is_empty()
        );
    }
}

#[test]
fn field_writes_and_effectful_calls_are_not_reuse_candidates() {
    let field = "public type Counter { value: Int }\ncalculate(value: Int) (answer: Int) { answer = value + 1 }\npublic main(counter: Counter) (answer: Int) { first = calculate(counter.value)\ncounter.value = counter.value + 1\nsecond = calculate(counter.value)\nanswer = first + second }";
    assert!(warnings(field, "W004").is_empty());
    let effects = "noisy(value: Int) (answer: Int) { dever.io.println(\"called\")\nanswer = value }\npublic main(value: Int) (answer: Int) { first = noisy(value)\nsecond = noisy(value)\nanswer = first + second }";
    assert!(warnings(effects, "W004").is_empty());
}

#[test]
fn forwarding_warning_excludes_public_adapters_recovery_and_constrained_outputs() {
    let direct = "calculate(value: Int) (answer: Int) { answer = value + 1 }\nwrapper(value: Int) (answer: Int) { answer = calculate(value) }";
    assert_eq!(warnings(direct, "W005").len(), 1);
    for source in [
        "calculate(value: Int) (answer: Int) { answer = value + 1 }\npublic wrapper(value: Int) (answer: Int) { answer = calculate(value) }",
        "calculate(value: Int) (answer: Int) { answer = value + 1 }\nwrapper(value: Int) (answer: Int) recover(\"application policy\") { answer = calculate(value) }",
        "calculate() (answer: Int >= 1) { answer = 1 }\nwrapper() (answer: Int >= 1) { answer = calculate() }",
        "type PrivateValue { private value: Int }\ncreate(value: Int) (answer: PrivateValue) { answer = PrivateValue { value = value } }\nwrapper(value: Int) (answer: PrivateValue) { answer = create(value) }",
    ] {
        assert!(warnings(source, "W005").is_empty(), "{source}");
    }
}

#[test]
fn existing_unused_write_warning_is_not_duplicated_as_dead_computation() {
    let program = checked(&sources(
        "calculate(value: Int) (answer: Int) { answer = value + 1 }\npublic main() () { unused = calculate(1) }",
    ));
    assert!(
        program
            .warnings()
            .iter()
            .any(|warning| warning.code == "W001")
    );
    assert!(
        !program
            .warnings()
            .iter()
            .any(|warning| warning.code == "W006")
    );
}
