mod support;

use dever_core::source::SourceMap;
use support::{checked, rejected, sources};

#[path = "../../../crates/dever-cli/src/api.rs"]
mod cli_api;

#[test]
fn application_exports_are_checked_only_with_a_known_entry() {
    let program = checked(&sources(
        "public main() (answer: Int) { answer = helper() }\npublic helper() (answer: Int) { answer = 1 }\npublic unused() () {}",
    ));
    let warnings = program.application_warnings("main.main").unwrap();
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().all(|warning| warning.code == "W007"));
    assert!(program.application_warnings("app.missing").is_err());
    let mut map = SourceMap::default();
    map.add(
        "main.dever",
        "public main() (answer: Int) { answer = utility.read() }",
    );
    map.add(
        "utility.dever",
        "public read() (answer: Int) { answer = 1 }",
    );
    let cross_package_program = checked(&map);
    assert!(
        cross_package_program
            .application_warnings("main.main")
            .unwrap()
            .is_empty()
    );
    assert!(
        !cross_package_program
            .api_snapshot()
            .contains("package app depends")
    );
}

#[test]
fn cli_baseline_is_explicit_never_overwrites_and_blocks_drift() {
    let directory = support::temp::TemporaryDirectory::new();
    let path = directory.path().join("dever.api");
    let original = checked(&sources("public main() (answer: Int) { answer = 1 }"));
    assert!(
        original
            .check_api(
                &original
                    .api_snapshot()
                    .replace("dever-api 5", "dever-api 4")
            )
            .unwrap_err()
            .contains("expected dever-api 5")
    );
    cli_api::check_baseline(&original, directory.path()).unwrap();
    assert!(!path.exists());
    cli_api::write(&original, Some(&path)).unwrap();
    cli_api::check_baseline(&original, directory.path()).unwrap();
    assert!(cli_api::write(&original, Some(&path)).is_err());
    let expanded = checked(&sources(
        "public main() (answer: Int) { answer = 1 }\npublic extra() () {}",
    ));
    assert!(
        cli_api::check_baseline(&expanded, directory.path())
            .unwrap_err()
            .contains("+ function main.extra")
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        original.api_snapshot()
    );
}

#[test]
fn effects_follow_named_and_forwarded_handlers_without_tainting_pure_calls() {
    let shared = r#"
apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }
forward(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = apply(route, value) }
identity(value: Int) (answer: Int) pure { answer = value }
noisy(value: Int) (answer: Int) { dever.io.println("called")
answer = value }
"#;
    checked(&sources(&format!(
        "{shared}\nmain() (answer: Int) pure {{ answer = forward(identity, 1) }}"
    )));
    rejected(
        &format!("{shared}\nmain() (answer: Int) pure {{ answer = forward(noisy, 1) }}"),
        "has effects: stdout",
    );
    rejected(
        "apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) pure { answer = route(value) }",
        "has effects: handler",
    );
    checked(&sources(&format!(
        "{shared}\nignore(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) {{ answer = value }}\nmain() (answer: Int) pure {{ answer = ignore(noisy, 1) }}"
    )));

    let collection = r#"
map_values(route: handler(value: Int) (answer: Int), values: List<Int>) (answers: List<Int>) { answers = each(route, values) }
identity(value: Int) (answer: Int) pure { answer = value }
noisy(value: Int) (answer: Int) { dever.io.println("called")
answer = value }
"#;
    checked(&sources(&format!(
        "{collection}\nmain() (answers: List<Int>) pure {{ answers = map_values(identity, [1]) }}"
    )));
    rejected(
        &format!(
            "{collection}\nmain() (answers: List<Int>) pure {{ answers = map_values(noisy, [1]) }}"
        ),
        "has effects: stdout",
    );
}

#[test]
fn pure_arithmetic_and_text_are_allowed_but_resource_and_clock_effects_are_rejected() {
    checked(&sources(
        "public main() (answer: Text) pure { answer = text.trim(\" x \") }",
    ));
    rejected(
        "clock() (result: Int) pure { result = dever.time.unix_millis() }",
        "has effects: time",
    );
    rejected(
        "close_stream(stream: Stream<Int>) () pure { close(stream) }",
        "has effects: stream",
    );
}

#[test]
fn api_snapshot_detects_expansion_and_effect_changes_but_ignores_private_helpers() {
    let original = checked(&sources("public main() (answer: Int) { answer = 1 }"));
    let baseline = original.api_snapshot();
    assert!(baseline.contains("function main.main"));
    assert!(!baseline.contains("function dever."));
    original.check_api(&baseline).unwrap();
    let helper = checked(&sources(
        "public main() (answer: Int) { answer = helper() }\nhelper() (answer: Int) { answer = 2 }",
    ));
    helper.check_api(&baseline).unwrap();
    let expanded = checked(&sources(
        "public main() (answer: Int) { answer = 1 }\npublic helper() (answer: Int) { answer = 2 }",
    ));
    let difference = expanded.check_api(&baseline).unwrap_err();
    assert!(difference.contains("+ function main.helper"));
    let noisy = checked(&sources(
        "public main() (answer: Int) { dever.io.println(\"x\")\nanswer = 1 }",
    ));
    assert!(
        noisy
            .check_api(&baseline)
            .unwrap_err()
            .contains("effects=[stdout]")
    );
}

#[test]
fn api_includes_constraints_and_private_construction_without_internal_representation() {
    let program = checked(&sources(
        "public type Port { value: Int >= 1 and <= 65535\nprivate secret: Text }",
    ));
    let snapshot = program.api_snapshot();
    assert!(snapshot.contains("construct=private"));
    assert!(snapshot.contains("1..65535"));
    assert!(!snapshot.contains("secret"));
}

#[test]
fn api_tracks_private_transitive_recovery_without_exposing_helper_names() {
    let original = checked(&sources("public main() (answer: Int) pure { answer = 1 }"));
    let changed = checked(&sources(
        "public main() (answer: Int) pure { answer = bridge() }\nbridge() (answer: Int) pure { answer = fallback() }\nfallback() (answer: Int) pure recover(\"missing configuration uses default\") { answer = 1 }",
    ));
    let snapshot = changed.api_snapshot();
    assert!(snapshot.contains("missing configuration uses default"));
    assert!(!snapshot.contains("bridge"));
    assert!(!snapshot.contains("fallback"));
    assert!(changed.check_api(&original.api_snapshot()).is_err());
    assert!(
        snapshot.contains("effects=[]"),
        "recovery itself is not an external effect"
    );
}

#[test]
fn recovery_through_handlers_only_propagates_when_the_handler_is_invoked() {
    let helpers = "fallback(value: Int) (answer: Int) recover(\"use default\") { answer = value }\napply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }\nignore(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = value }\n";
    let invoked = checked(&sources(&format!(
        "{helpers}public main() (answer: Int) pure {{ answer = apply(fallback, 1) }}"
    )));
    assert!(invoked.api_snapshot().contains("use default"));
    let ignored = checked(&sources(&format!(
        "{helpers}public main() (answer: Int) pure {{ answer = ignore(fallback, 1) }}"
    )));
    assert!(!ignored.api_snapshot().contains("use default"));
}

#[test]
fn effect_work_queue_preserves_long_chains_and_unused_handler_cycles() {
    let mut source = String::from("public main() () { step0() }\n");
    for level in 0..64 {
        source.push_str(&format!("step{level}() () {{ step{}() }}\n", level + 1));
    }
    source
        .push_str("step64() () recover(\"report optional event\") { dever.io.println(\"event\") }");
    let snapshot = checked(&sources(&source)).api_snapshot();
    assert!(snapshot.contains("effects=[stdout]"), "{snapshot}");
    assert!(snapshot.contains("report optional event"), "{snapshot}");
    rejected(
        &source.replace("main() () {", "main() () pure {"),
        "has effects: stdout",
    );

    let cycle = "public main() () pure { bridge() }\nbridge() () { ignore(bridge) }\nignore(route: handler() ()) () recover(\"skip optional callback\") {}";
    let snapshot = checked(&sources(cycle)).api_snapshot();
    assert!(snapshot.contains("effects=[]"), "{snapshot}");
    assert!(snapshot.contains("skip optional callback"), "{snapshot}");
    let unused = cycle.replace(
        "bridge() () { ignore(bridge) }",
        "bridge() () { ignore(bridge)\nignore(noisy) }\nnoisy() () recover(\"unused recovery\") { dever.io.println(\"unused\") }",
    );
    let snapshot = checked(&sources(&unused)).api_snapshot();
    assert!(snapshot.contains("effects=[]"), "{snapshot}");
    assert!(!snapshot.contains("unused recovery"), "{snapshot}");
}
