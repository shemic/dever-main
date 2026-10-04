mod support;

use support::{checked, rejected, sources};

const ERRORS: &str = r#"public type ComputeResult {
  Done(value: Int)
  error Invalid(value: Int)
}
compute(value: 0) (answer: Int) {
  fail(ComputeResult.Invalid(value))
}
compute(value: other) (answer: Int) {
  answer = value + 1
}
render(result: ComputeResult.Done(value)) (text: Text) recover("render handled compute errors") {
  text = int.to_text(value)
}
render(result: ComputeResult.Invalid(value)) (text: Text) recover("render handled compute errors") {
  text = "invalid " + int.to_text(value)
}
"#;

#[test]
fn fail_and_result_have_stable_syntax_and_formatting() {
    let source = format!("{ERRORS}main() (text: Text) {{ text = render(result(compute(0))) }}");
    let mut map = sources(&source);
    let formatted = dever_core::format::format(map.get(map.files()[0].id())).unwrap();
    map = sources(&formatted);
    let reformatted = dever_core::format::format(map.get(map.files()[0].id())).unwrap();
    assert_eq!(formatted, reformatted);
    assert!(formatted.contains("fail(ComputeResult.Invalid(value))"));
    assert!(formatted.contains("result(compute(0))"));
}

#[test]
fn error_sets_propagate_and_are_visible_in_the_public_api() {
    let source = format!(
        "{ERRORS}forward(value: Int) (answer: Int) {{ answer = compute(value) }}\npublic main() (answer: Int) {{ answer = forward(1) }}"
    );
    let program = checked(&sources(&source));
    let snapshot = program.api_snapshot();
    assert!(
        snapshot.contains("function main.main() (answer: Int [])")
            && snapshot.contains("errors=[main.ComputeResult.Invalid]"),
        "{snapshot}"
    );
}

#[test]
fn result_capture_requires_matching_success_and_error_variants() {
    checked(&sources(&format!(
        "{ERRORS}main() (text: Text) {{ text = render(result(compute(0))) }}"
    )));
    rejected(
        &format!(
            "{ERRORS}type Wrong {{ Done(value: Int)\nerror Other(value: Int) }}\nwrong(result: Wrong.Done(value)) () recover(\"handled\") {{}}\nwrong(result: Wrong.Other(value)) () recover(\"handled\") {{}}\nmain() () {{ wrong(result(compute(0))) }}"
        ),
        "result capture errors do not match the call",
    );
    rejected(
        &format!(
            "{ERRORS}type Wrong {{ Done\nerror Invalid(value: Int) }}\nwrong(result: Wrong) () recover(\"handled\") {{}}\nmain() () {{ wrong(result(compute(0))) }}"
        ),
        "success payload must match",
    );
}

#[test]
fn result_capture_does_not_introduce_a_concurrency_effect() {
    checked(&sources(&format!(
        "{ERRORS}main() (result: ComputeResult) pure {{ result = result(compute(0)) }}"
    )));
}

#[test]
fn fail_must_be_an_error_variant_and_end_its_clause() {
    rejected(
        "type Outcome { Done\nerror Failed }\npublic main() () { fail(Outcome.Done) }",
        "fail requires an error choice variant",
    );
    rejected(
        "type Outcome { error Failed }\npublic main() () { fail(Outcome.Failed)\ndever.io.println(\"unreachable\") }",
        "fail must be the final statement",
    );
}

#[test]
fn native_default_propagation_reaches_the_cli_boundary_once() {
    let output = support::run(
        "type Outcome { error Failed(value: Int) }\nstep() () { fail(Outcome.Failed(7)) }\npublic main() () { step() }",
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.contains("main.Outcome.Failed(7)"), "{stderr}");
    assert!(stderr.contains("main.dever:3:"), "{stderr}");
}

#[test]
fn native_result_capture_converts_success_and_business_failure_to_choice_values() {
    for (value, expected) in [("0", "invalid 0"), ("4", "5")] {
        let output = support::stdout(&format!(
            "{ERRORS}main() (text: Text) {{ text = render(result(compute({value}))) }}"
        ));
        assert_eq!(output, format!("text = {expected}\n"));
    }
}

#[test]
fn task_wait_preserves_business_error_identity() {
    let output = support::run(
        "type Outcome { error Failed(value: Int) }\nstep() () { fail(Outcome.Failed(9)) }\npublic main() () { task = run(step())\nwait(task) }",
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("main.Outcome.Failed(9)"), "{stderr}");
    assert!(stderr.contains("main.dever:4:"), "{stderr}");
}

#[test]
fn capture_wraps_a_shared_error_choice_without_losing_its_identity() {
    let source = r#"public type Failure {
  error Missing(message: Text)
  error Conflict(message: Text)
}
public type Outcome {
  Done(value: Int)
  error Failed(error: Failure)
}
load(conflict: true) (value: Int) { fail(Failure.Conflict("duplicate")) }
load(conflict: false) (value: Int) { fail(Failure.Missing("absent")) }
public main() (outcome: Outcome) { outcome = result(load(false)) }
"#;
    assert_eq!(
        support::stdout(source),
        "outcome = main.Outcome.Failed(main.Failure.Missing(absent))\n"
    );
    #[cfg(feature = "reference")]
    {
        let sources = sources(source);
        let result = dever_core::reference::evaluate(&checked(&sources), "main.main").unwrap();
        assert!(result.fault.is_none(), "{:?}", result.fault);
        assert_eq!(
            result.stdout,
            "outcome = main.Outcome.Failed(main.Failure.Missing(absent))\n"
        );
    }
    rejected(
        &source.replace(
            "error Failed(error: Failure)",
            "error Failed(error: Failure)\nerror Again(error: Failure)",
        ),
        "overlapping error wrappers",
    );
    rejected(
        &source.replace(
            "outcome = result(load(false))",
            "outcome = result(load(false))\noutcome = Outcome.Done(0)",
        ),
        "failure is not handled",
    );
    let wrapped = source.replace(
        "public main() (outcome: Outcome) { outcome = result(load(false)) }",
        r#"
rewrap(captured: Outcome.Done(number)) (answer: Int) { answer = number }
rewrap(captured: Outcome.Failed(error)) (answer: Int) { fail(Outcome.Failed(error)) }
load_again() (answer: Int) { answer = rewrap(result(load(false))) }
public main() (outcome: Outcome) { outcome = result(load_again()) }
"#,
    );
    assert_eq!(
        support::stdout(&wrapped),
        "outcome = main.Outcome.Failed(main.Failure.Missing(absent))\n"
    );
}
