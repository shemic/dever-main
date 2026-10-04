mod support;

use support::{checked, rejected, sources};

const RESULT: &str = r#"type Result { Ready(value: Int)
error Failed(message: Text) }
fail_value(message: Text) (value: Int) { fail(Result.Failed(message)) }
failure(message: Text) (value: Result) { value = result(fail_value(message)) }
"#;

#[test]
fn failure_cannot_be_discarded_overwritten_logged_or_laundered() {
    for code in [
        "run() () { result = failure(\"bad\") }",
        "run() (result: Result) { result = failure(\"bad\")\nresult = Result.Ready(0) }",
        "run(result: Result.Ready(value)) (out: Int) { out = value }\nrun(result: Result.Failed(message)) (out: Int) { dever.io.println(message)\nout = 0 }",
        "ignore(result: Result) () {}\nrun() () { ignore(failure(\"bad\")) }",
    ] {
        rejected(&format!("{RESULT}{code}"), "failure is not handled");
    }
}

#[test]
fn errors_can_be_returned_translated_or_explicitly_recovered() {
    for code in [
        "run() (result: Result) { result = failure(\"bad\") }",
        "run(result: Result) (out: Result) { out = result }",
        "run(result: Result.Ready(value)) (out: Result) { out = Result.Ready(value) }\nrun(result: Result.Failed(_)) (out: Result) { fail(Result.Failed(\"translated\")) }",
        "run(result: Result.Ready(value)) (out: Int) recover(\"optional configuration uses zero\") { out = value }\nrun(result: Result.Failed(_)) (out: Int) recover(\"optional configuration uses zero\") { out = 0 }",
    ] {
        checked(&sources(&format!("{RESULT}{code}")));
    }
}

#[test]
fn nested_failure_survives_value_copies_and_projection() {
    rejected(
        &format!(
            "{RESULT}type Box {{ result: Result\nnumber: Int }}\nrun() (out: Int) {{ box = Box {{ result = failure(\"bad\")\nnumber = 3 }}\nout = box.number }}"
        ),
        "failure is not handled",
    );
    checked(&sources(&format!(
        "{RESULT}type Box {{ result: Result }}\nrun() (out: Box) {{ box = Box {{ result = failure(\"bad\") }}\nout = box }}"
    )));
}

#[test]
fn possibly_empty_or_null_outputs_cannot_hide_another_failure() {
    for (ty, extra) in [
        ("List<Problem>", ""),
        ("Box?", "public type Box { problem: Problem }"),
    ] {
        rejected(
            &format!(
                "public type Problem {{ error Bad }}\n{extra}\npublic run(problem: Problem.Bad, replacement: {ty}) (result: {ty}) {{ result = replacement }}"
            ),
            "failure is not handled",
        );
    }
    checked(&sources(
        "public type Problem { error Bad }\npublic type Box { problem: Problem }\npublic run(box: Box, replacement: Problem) (result: Problem) { result = replacement }",
    ));
}

#[test]
fn empty_traversal_does_not_handle_context_and_singleton_projection_transfers_failure() {
    rejected(
        "public type Problem { error Bad }\nstep(value: Int, problem: Problem) () recover(\"report optional diagnostic\") {}\npublic run(problem: Problem) () { each(step, [], problem) }",
        "failure is not handled",
    );
    checked(&sources(
        "public type Problem { error Bad }\npublic run(problem: Problem) (result: Problem?) { result = first([problem]) }",
    ));
    rejected(
        "public type Problem { error Bad }\npublic run(first_problem: Problem, second_problem: Problem) (result: Problem?) { result = first([first_problem, second_problem]) }",
        "failure is not handled",
    );
    rejected(
        "public type Result { Ready\nerror Failed }\npublic run(problem: Result) (result: Map<Text, Result>) { old = {\"key\" = problem}\nresult = put(old, \"key\", Result.Ready) }",
        "failure is not handled",
    );
    rejected(
        "public type Result { Ready\nerror Failed }\npublic run(problem: Result, key: Text) (result: Map<Text, Result>) { result = {key = problem\n\"key\" = Result.Ready} }",
        "failure is not handled",
    );
}

#[test]
fn short_circuit_does_not_claim_a_skipped_handler_consumes_failures() {
    for left in ["false", "condition"] {
        rejected(
            &format!(
                "public type Problem {{ error Bad }}\nreport(problem: Problem) (result: Bool) recover(\"report optional failure\") {{ result = true }}\npublic run(problem: Problem, condition: Bool) (result: Bool) {{ result = {left} and report(problem) }}"
            ),
            "failure is not handled",
        );
    }
}

#[test]
fn empty_reduce_preserves_the_initial_failure_obligation() {
    let declarations = "type Problem { error Bad }\nstep(value: Int, state: Problem) (next: Problem) { next = state }\n";
    rejected(
        &format!(
            "{declarations}run(problem: Problem) () {{ discarded = reduce(step, [], problem) }}"
        ),
        "failure is not handled",
    );
    checked(&sources(&format!(
        "{declarations}run(problem: Problem) (result: Problem) {{ result = reduce(step, [], problem) }}"
    )));
}

#[test]
fn shared_type_graphs_do_not_expand_without_bounded_failure_work() {
    for error in [false, true] {
        let mut source = String::from("");
        source.push_str(if error {
            "type T0 { error Bad }\n"
        } else {
            "type T0 { value: Int }\n"
        });
        for level in 1..=24 {
            source.push_str(&format!(
                "type T{level} {{ left: T{}\nright: T{} }}\n",
                level - 1,
                level - 1
            ));
        }
        source.push_str("accept(value: T24) (result: T24) { result = value }");
        if error {
            rejected(&source, "failure analysis exceeds its structural limit");
        } else {
            checked(&sources(&source));
        }
    }
}

#[test]
fn nominal_depth_is_checked_before_recursive_contract_analysis() {
    let mut source = String::from("type T0 { error Bad }\n");
    for level in 1..=256 {
        source.push_str(&format!("type T{level} {{ value: T{} }}\n", level - 1));
    }
    source.push_str("accept(value: T256) (result: T256) { result = value }");
    rejected(
        &source,
        "contract type nesting exceeds the structural depth limit",
    );
}

#[test]
fn shared_nominal_properties_preserve_equality_and_worker_resource_boundaries() {
    const DEPTH: usize = 48;
    for (leaf, comparable, transferable) in [
        ("Text", true, true),
        ("dever.system.File", false, true),
        ("dever.system.Socket", false, true),
        ("dever.system.Listener", false, true),
        ("Stream<Int>", false, false),
    ] {
        let mut declarations = format!("type T0 {{ value: {leaf} }}\n");
        // Declare callers before their dependencies, so source order cannot stand in
        // for the checked type graph. Each level shares the previous type twice.
        for level in (1..=DEPTH).rev() {
            declarations.push_str(&format!(
                "type T{level} {{ left: T{}\nright: T{} }}\n",
                level - 1,
                level - 1
            ));
        }
        let equality = format!(
            "{declarations}run(left: T{DEPTH}, right: T{DEPTH}) (same: Bool) {{ same = left == right }}"
        );
        if comparable {
            checked(&sources(&equality));
        } else {
            rejected(&equality, "equality requires the same comparable type");
        }
        let parallel = format!(
            "{declarations}accept(value: T{DEPTH}) () {{}}\nrun(values: List<T{DEPTH}>) () {{ parallel_each(accept, values, 2) }}"
        );
        if transferable {
            checked(&sources(&parallel));
        } else {
            rejected(&parallel, "cannot contain Stream");
        }
    }
}

#[test]
fn failure_summaries_follow_long_calls_and_unused_handler_dependency_cycles() {
    // 已有失败 payload 保留“必定携带失败”的摘要；叶子允许恢复，负向只检验调用者的原失败。
    let prefix = "type Problem { error Bad }\ntype Result { Ready(problem: Problem)\nEmpty }\n";
    let mut declarations = String::from(prefix);
    for level in 0..64 {
        declarations.push_str(&format!(
            "step{level}(replacement: Problem) (result: Result) {{ result = step{}(replacement) }}\n",
            level + 1
        ));
    }
    declarations
        .push_str("step64(replacement: Problem) (result: Result) recover(\"replacement policy\") { result = Result.Ready(replacement) }\n");
    declarations.push_str("run(problem: Problem, replacement: Problem) (result: Result) { result = step0(replacement) }");
    checked(&sources(&declarations));
    rejected(
        &declarations.replace("Result.Ready(replacement)", "Result.Empty"),
        "failure is not handled",
    );

    let cycle = format!(
        "{prefix}run(problem: Problem, value: Problem) (result: Result) {{ result = replacement(value) }}\nreplacement(value: Problem) (result: Result) {{ result = ignore(replacement, value) }}\nignore(route: handler(value: Problem) (result: Result), value: Problem) (result: Result) recover(\"replacement policy\") {{ result = Result.Ready(value) }}"
    );
    checked(&sources(&cycle));
    rejected(
        &cycle.replace("Result.Ready(value)", "Result.Empty"),
        "failure is not handled",
    );
}

#[test]
fn private_fields_protect_access_mutation_and_construction() {
    for expression in [
        "out = box.value",
        "box.value = 2\nout = 0",
        "box = secret.Box { value = 2 }\nout = 0",
    ] {
        let mut source = sources(&format!(
            "public run(box: secret.Box) (out: Int) {{ {expression} }}"
        ));
        source.add("secret.dever", "public type Box { private value: Int }");
        let diagnostics = dever_core::check(&source).unwrap_err();
        assert!(diagnostics.iter().any(|error| error.code == "C006"));
    }
}
