use dever_core::source::SourceMap;
use dever_core::syntax::{Declaration, InputKind, Package, TypeShape};

fn parsed(source: &str) -> Package {
    let mut sources = SourceMap::default();
    let id = sources.add("main.dever", source);
    dever_core::parse(sources.get(id)).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    })
}

fn formatted(source: &str) -> String {
    let mut sources = SourceMap::default();
    let id = sources.add("main.dever", source);
    dever_core::format::format(sources.get(id)).expect("valid contract syntax")
}

#[test]
fn parses_package_type_and_function_contracts() {
    let package = parsed(
        r#"public type Port {
  private value: Int >= 1 and <= 65535
  scale: Decimal > -1.5
}
public type Result {
  Ready(port: Port)
  error Failed(message: Text)
}
public read(step: handler(value: Int >= 1) (result: Int <= 65535)) (result: Int >= 1) recover("configured fallback") pure {
  result = 8080
}"#,
    );
    let Declaration::Type(port) = &package.declarations[0] else {
        panic!("record")
    };
    let TypeShape::Record(fields) = &port.shape else {
        panic!("record")
    };
    assert!(fields[0].private);
    assert_eq!(fields[0].bounds.len(), 2);
    assert!(!fields[1].private);
    assert_eq!(fields[1].bounds[0].number, "-1.5");
    let Declaration::Type(result) = &package.declarations[1] else {
        panic!("choice")
    };
    let TypeShape::Choice(variants) = &result.shape else {
        panic!("choice")
    };
    assert!(!variants[0].error);
    assert!(variants[1].error);
    assert_eq!(variants[1].name.text, "Failed");
    let Declaration::Function(function) = &package.declarations[2] else {
        panic!("function")
    };
    assert!(function.pure);
    assert_eq!(function.recovery.as_deref(), Some("configured fallback"));
    assert_eq!(function.outputs[0].bounds.len(), 1);
    let InputKind::Handler(handler) = &function.inputs[0].kind else {
        panic!("handler")
    };
    assert_eq!(handler.inputs[0].bounds.len(), 1);
    assert_eq!(handler.outputs[0].bounds.len(), 1);
}

#[test]
fn preserves_contextual_identifiers() {
    let package = parsed(
        "type Record { private: Int }\ntype Choice { error(value: Int) }\ndepends() () {}\npure() () {}\nrecover() () {}",
    );
    let Declaration::Type(record) = &package.declarations[0] else {
        panic!("record")
    };
    let TypeShape::Record(fields) = &record.shape else {
        panic!("record")
    };
    assert!(!fields[0].private);
    assert_eq!(fields[0].name.text, "private");
    let Declaration::Type(choice) = &package.declarations[1] else {
        panic!("choice")
    };
    let TypeShape::Choice(variants) = &choice.shape else {
        panic!("choice")
    };
    assert!(!variants[0].error);
    assert_eq!(variants[0].name.text, "error");
}

#[test]
fn depends_remains_an_ordinary_function_name() {
    let source =
        "depends() (value: Int) { value = 1 }\npublic main() (value: Int) { value = depends() }";
    let output = formatted(source);
    assert_eq!(formatted(&output), output);

    let mut sources = SourceMap::default();
    let id = sources.add("main.dever", output);
    let package = dever_core::parse(sources.get(id)).unwrap();
    assert!(dever_core::check(&sources).is_ok());
    assert!(matches!(package.declarations[0], Declaration::Function(_)));
}

#[test]
fn rejects_removed_package_dependency_clauses() {
    let mut sources = SourceMap::default();
    let id = sources.add("main.dever", "package app exposes () depends ()");
    assert!(dever_core::parse(sources.get(id)).is_err());
}

#[test]
fn rejects_ambiguous_or_invalid_contract_markers() {
    for declaration in [
        "f() () pure pure {}",
        "f() () recover(\"one\") recover(\"two\") {}",
        "f() () recover(\"\") {}",
        "f() () recover(\"  \") {}",
        "f() () recover(1) {}",
        "f() (private result: Int) {}",
        "f(step: handler(private value: Int) ()) () {}",
        "type Result { Ready(private value: Int) }",
        "type Result { private Ready }",
        "type Port { value: Int >= 1 and }",
    ] {
        let mut sources = SourceMap::default();
        let id = sources.add("main.dever", declaration.to_string());
        assert!(
            dever_core::parse(sources.get(id)).is_err(),
            "accepted: {declaration}"
        );
    }
}

#[test]
fn formatting_preserves_contracts_comments_and_canonical_marker_order() {
    let source = r#"public type Port {
private value:Int >= 1 and <=65535 # valid range
}
public type Result {
Ready(value:Int >= 1)
error Failed(message:Text) # failure
}
public main() (result:Int >=1) recover("use \"configured\" port") # recovery
pure # effect
{result=8080}
"#;
    let output = formatted(source);
    assert_eq!(formatted(&output), output);
    assert!(output.contains("private value: Int >= 1 and <= 65535"));
    assert!(output.contains("error Failed("));
    for comment in ["# valid range", "# failure", "# recovery", "# effect"] {
        assert_eq!(output.matches(comment).count(), 1);
    }
    assert!(output.find("pure # effect").unwrap() < output.find("recover(").unwrap());
    let package = parsed(&output);
    let Declaration::Function(function) = &package.declarations[2] else {
        panic!("function")
    };
    assert!(function.pure);
    assert_eq!(
        function.recovery.as_deref(),
        Some("use \"configured\" port")
    );
}
