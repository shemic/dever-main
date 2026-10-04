use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dever_core::diagnostic::{Diagnostic, Label};
use dever_core::source::SourceMap;
use dever_core::syntax::{
    BinaryOperator as Op, Declaration, Expression, ExpressionKind as Expr, InputKind, Literal,
    Package, Pattern, StatementKind, TypeShape, UnaryOperator,
};

fn parsed(text: &str) -> Package {
    let mut sources = SourceMap::default();
    let id = sources.add("account/user.dever", text);
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

fn rejected(text: &str) -> (SourceMap, Vec<Diagnostic>) {
    let mut sources = SourceMap::default();
    let id = sources.add("broken.dever", text);
    let errors = dever_core::parse(sources.get(id)).expect_err("invalid syntax must be rejected");
    assert!(!errors.is_empty());
    (sources, errors)
}

fn assigned(expression: &str) -> Expression {
    let package = parsed(&format!(
        "public main() (result: Int) {{\n  result = {expression}\n}}"
    ));
    let Declaration::Function(function) = package.declarations.into_iter().next().unwrap() else {
        panic!("expected function")
    };
    let StatementKind::Assign { value, .. } = function.body.into_iter().next().unwrap().kind else {
        panic!("expected assignment")
    };
    value
}

#[test]
fn parses_the_complete_package_from_the_product_contract() {
    let prd = include_str!("../../../.trellis/tasks/09-04-dever-language-mvp/prd.md");
    let (_, example_section) = prd.split_once("## Complete Package Example\n").unwrap();
    let (_, fenced) = example_section.split_once("```dever\n").unwrap();
    let (source, _) = fenced.split_once("\n```").unwrap();
    let package = parsed(source);
    assert_eq!(
        package
            .name
            .iter()
            .map(|name| name.text.as_str())
            .collect::<Vec<_>>(),
        ["account", "user"]
    );
    assert!(package.declarations.iter().any(|declaration| matches!(
        declaration,
        Declaration::Function(function)
            if function.name.text == "total_score_bonus" && function.public
    )));
    assert_eq!(
        package
            .declarations
            .iter()
            .filter(|declaration| matches!(declaration, Declaration::Type(_)))
            .count(),
        5
    );
    assert_eq!(
        package
            .declarations
            .iter()
            .filter(|declaration| matches!(declaration, Declaration::Function(_)))
            .count(),
        22
    );
    let Declaration::Type(status) = &package.declarations[0] else {
        panic!("expected UserStatus")
    };
    let TypeShape::Choice(variants) = &status.shape else {
        panic!("expected choice")
    };
    assert_eq!(variants[2].name.text, "Disabled");
    assert_eq!(variants[2].payload[0].name.text, "reason");
}

#[test]
fn arithmetic_precedence_and_associativity_follow_the_language_contract() {
    let Expr::Binary {
        left,
        operator: Op::Subtract,
        right,
    } = assigned("10 - 4 - 2 * 3").kind
    else {
        panic!("expected subtraction")
    };
    assert!(matches!(
        left.kind,
        Expr::Binary {
            operator: Op::Subtract,
            ..
        }
    ));
    assert!(matches!(
        right.kind,
        Expr::Binary {
            operator: Op::Multiply,
            ..
        }
    ));
    let Expr::Binary {
        left,
        operator: Op::Or,
        right,
    } = assigned("not enabled and score >= 60 or override").kind
    else {
        panic!("expected or")
    };
    let Expr::Binary {
        left: negated,
        operator: Op::And,
        right: compared,
    } = left.kind
    else {
        panic!("expected and")
    };
    assert!(matches!(
        negated.kind,
        Expr::Unary {
            operator: UnaryOperator::Not,
            ..
        }
    ));
    assert!(matches!(
        compared.kind,
        Expr::Binary {
            operator: Op::GreaterEqual,
            ..
        }
    ));
    assert!(matches!(right.kind, Expr::Name(_)));
}

#[test]
fn integer_division_is_never_a_comment() {
    let Expr::Binary {
        operator, right, ..
    } = assigned("-5 // 2 # actual comment").kind
    else {
        panic!("expected division")
    };
    assert_eq!(operator, Op::IntegerDivide);
    assert!(matches!(right.kind, Expr::Literal(Literal::Number(ref number)) if number == "2"));
}

#[test]
fn numeric_spelling_survives_parsing_without_rounding() {
    for number in [
        "9223372036854775808",
        "0.12345678901234567890123456789012345",
        "1.25e-6",
    ] {
        assert!(
            matches!(assigned(number).kind, Expr::Literal(Literal::Number(ref actual)) if actual == number)
        );
    }
    let Expr::Unary { value, .. } = assigned("-9223372036854775808").kind else {
        panic!("expected unary minus")
    };
    assert!(
        matches!(value.kind, Expr::Literal(Literal::Number(ref number)) if number == "9223372036854775808")
    );
}

#[test]
fn text_escapes_and_comments_preserve_unicode_and_spans() {
    let source = "# 中文注释\npublic main() (text: Text) {\n  text = \"中文\\n\\u{1f600}\\t\\\\\\\"\" # trailing\n}\n# final\n";
    let package = parsed(source);
    assert_eq!(package.comments.len(), 3);
    for comment in &package.comments {
        assert_eq!(&source[comment.span.start..comment.span.end], comment.text);
    }
    let Declaration::Function(function) = &package.declarations[0] else {
        panic!("expected function")
    };
    let StatementKind::Assign { value, .. } = &function.body[0].kind else {
        panic!("expected assignment")
    };
    assert!(
        matches!(&value.kind, Expr::Literal(Literal::Text(text)) if text == "中文\n\u{1f600}\t\\\"")
    );
}

#[test]
fn diagnostic_points_to_the_bad_escape_after_multibyte_text() {
    let source = "main() (text: Text) {\n  text = \"中\\q\"\n}";
    let (sources, errors) = rejected(source);
    assert_eq!(errors[0].code, "L006");
    assert_eq!(
        &source[errors[0].primary.start..errors[0].primary.end],
        "\\q"
    );
    let rendered = errors[0].render(&sources);
    assert!(
        rendered.starts_with("broken.dever:2:12: error[L006]: invalid Text escape\n"),
        "{rendered}"
    );
    assert_eq!(rendered.lines().nth(1), Some("  |   text = \"中\\q\""));
    assert_eq!(rendered.lines().count(), 2);
    assert!(!rendered.contains("Token") && !rendered.contains("rust"));
}

#[test]
fn bad_literals_and_non_ascii_identifiers_are_rejected() {
    for value in [
        "1e",
        "2e+",
        "3suffix",
        "\"\\u{d800}\"",
        "\"\\u{110000}\"",
        "\"\\u{}\"",
        "\"\\x41\"",
        "\"unterminated",
    ] {
        rejected(&format!("main() (result: Text) {{ result = {value} }}"));
    }
    let (_, errors) = rejected("package 示例 exposes ()");
    assert_eq!(errors[0].code, "L001");
}

#[test]
fn lexical_diagnostics_are_sorted_by_source_location() {
    let (_, errors) = rejected("main() (text: Text) { text = \"\\q");
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].code, "L005");
    assert_eq!(errors[1].code, "L006");
    assert!(errors[0].primary.start < errors[1].primary.start);
}

#[test]
fn related_diagnostic_labels_resolve_their_own_source_file() {
    let mut sources = SourceMap::default();
    let first = sources.add("first.dever", "main() () { 1 }");
    let second = sources.add("second.dever", "main() () { 2 }");
    let mut primary = dever_core::parse(sources.get(first)).unwrap_err().remove(0);
    let related = dever_core::parse(sources.get(second))
        .unwrap_err()
        .remove(0);
    primary.related.push(Label {
        span: related.primary,
        message: "related declaration".into(),
    });
    let rendered = primary.render(&sources);
    assert!(rendered.starts_with("first.dever:1:13: error[P002]"));
    assert!(rendered.contains("second.dever:1:13: note: related declaration\n  | main() () { 2 }"));
}

#[test]
fn patterns_cover_ranges_choices_null_and_complement() {
    let package = parsed(
        "grade(score: Int >= 60 and < 90) (result: Text) { result = \"pass\" }\ngrade(score: Int < 60) (result: Text) { result = \"fail\" }\nselect(value: Result.Created(user, _)) (result: Text) { result = user.name }\nselect(value: null) (result: Text) { result = \"none\" }\nselect(value: other) (result: Text) { result = \"other\" }",
    );
    let Declaration::Function(grade) = &package.declarations[0] else {
        panic!("expected function")
    };
    let InputKind::Value(Pattern::Typed { ty, bounds }) = &grade.inputs[0].kind else {
        panic!("expected range")
    };
    assert_eq!(ty.name[0].text, "Int");
    assert_eq!(bounds[0].comparison, Op::GreaterEqual);
    assert_eq!(bounds[1].number, "90");
    let Declaration::Function(select) = &package.declarations[2] else {
        panic!("expected function")
    };
    let InputKind::Value(Pattern::Variant { bindings, .. }) = &select.inputs[0].kind else {
        panic!("expected variant")
    };
    assert_eq!(bindings[1].text, "_");
}

#[test]
fn generic_types_and_all_collection_literal_shapes_parse() {
    let empty = parsed("type Empty {}\nmake() (value: Empty) { value = Empty {} }");
    assert!(
        matches!(&empty.declarations[0], Declaration::Type(declaration) if matches!(&declaration.shape, TypeShape::Record(fields) if fields.is_empty()))
    );
    parsed(
        "main() (users: List<User>, labels: Map<Text, List<Int?>>) {\n users = [User { name = \"a\" }, User { name = \"b\" }]\n labels = {\n  \"a\" = [1, null]\n  \"b\" = []\n }\n labels = {}\n}",
    );
    let Expr::Map(entries) = assigned("{\n\"first\" = 1\n\"second\" = 2\n}").kind else {
        panic!("expected Map")
    };
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[0].key.kind, Expr::Literal(Literal::Text(text)) if text == "first"));
}

#[test]
fn handler_inputs_and_stream_types_have_dedicated_syntax() {
    let package = parsed(
        "serve(route: handler(request: Text) (response: Text), events: Stream<Text>) (result: Text) { result = route(\"ok\") }",
    );
    let Declaration::Function(function) = &package.declarations[0] else {
        panic!("expected function")
    };
    let InputKind::Handler(signature) = &function.inputs[0].kind else {
        panic!("expected handler input")
    };
    assert_eq!(signature.inputs[0].name.text, "request");
    assert_eq!(signature.outputs[0].name.text, "response");
    let InputKind::Value(Pattern::Typed { ty, .. }) = &function.inputs[1].kind else {
        panic!("expected Stream input")
    };
    assert_eq!(ty.name[0].text, "Stream");
    assert_eq!(ty.arguments[0].name[0].text, "Text");
}

#[test]
fn handler_signature_syntax_is_rejected_outside_function_inputs() {
    for source in [
        "type Box { route: handler(value: Int) (answer: Int) }",
        "main() (route: handler(value: Int) (answer: Int)) {}",
    ] {
        rejected(source);
    }
}

#[test]
fn postfix_calls_grouping_and_multiline_arguments_preserve_structure() {
    let Expr::Field { value, name } = assigned("split_name(\n  \"a\" +\n  \"b\"\n).first").kind
    else {
        panic!("expected field access")
    };
    assert_eq!(name.text, "first");
    let Expr::Call { arguments, .. } = value.kind else {
        panic!("expected call")
    };
    assert!(matches!(
        arguments[0].kind,
        Expr::Binary {
            operator: Op::Add,
            ..
        }
    ));
    let Expr::Binary { left, .. } = assigned("(1 + 2) * 3").kind else {
        panic!("expected multiplication")
    };
    assert!(matches!(left.kind, Expr::Group(_)));
}

#[test]
fn excluded_constructs_cannot_silently_become_valid_syntax() {
    for body in [
        "if true {}",
        "return 1",
        "result = null ?? 0",
        "result += 1",
        "result = value?.name",
        "result = values[0]",
        "result = 1;",
        "1 + 2",
        "make().field = 1",
        "result = other",
    ] {
        rejected(&format!("main() (result: Int) {{\n {body}\n}}"));
    }
    for source in [
        "import account.user",
        "package account.user exposes (User)\ntype User { name: Text }",
        "type Mixed {\n name: Text\n Pending\n}",
        "main() (value: Int??) { value = null }",
        "main() { }",
        "main(value: Int >= 0 and <= 5 and < 4) () {}",
    ] {
        rejected(source);
    }
}

#[test]
fn a_statement_requires_a_line_or_structural_boundary() {
    parsed("main() () {\n emit(1)\n emit(2)\n}");
    rejected("main() () { emit(1) emit(2) }");
    rejected("main() (x: Int) { x = 1\n + 2 }");
}

#[test]
fn recovery_reports_independent_declaration_failures_in_source_order() {
    let source = "first() (x: Int) { x = }\nsecond() (x: Int) { x = }";
    let (_, errors) = rejected(source);
    assert_eq!(errors.len(), 2);
    assert!(errors[0].primary.start < errors[1].primary.start);
}

#[test]
fn excessive_nesting_is_a_diagnostic_instead_of_a_stack_overflow() {
    let source = format!(
        "main() (x: Int) {{ x = {}1{} }}",
        "(".repeat(160),
        ")".repeat(160)
    );
    let (_, errors) = rejected(&source);
    assert_eq!(errors[0].code, "P009");
    for expression in [
        format!("{}1", "1 + ".repeat(160)),
        format!("f{}", "()".repeat(160)),
    ] {
        let (_, errors) = rejected(&format!("main() (x: Int) {{ x = {expression} }}"));
        assert_eq!(errors[0].code, "P009");
    }
}

#[test]
fn source_positions_support_crlf_unicode_and_eof() {
    let mut sources = SourceMap::default();
    let id = sources.add("p.dever", "# 中\r\n\r\n");
    let source = sources.get(id);
    assert_eq!(source.position(7), (2, 1));
    assert_eq!(source.position(source.text().len()), (3, 1));
    assert_eq!(source.line(1), "# 中");
    assert_eq!(source.line(3), "");
    let id = sources.add("cr.dever", "# comment\r\r");
    let source = sources.get(id);
    assert_eq!(source.position(10), (2, 1));
    assert_eq!(source.line(1), "# comment");
    assert_eq!(source.position(source.text().len()), (3, 1));
    dever_core::parse(source).unwrap();
}

struct SourceDirectory(PathBuf);

impl SourceDirectory {
    fn create() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dever-source-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for SourceDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove the test-owned source directory");
    }
}

#[test]
fn source_loading_is_sorted_relative_and_ignores_other_file_formats() {
    let directory = SourceDirectory::create();
    fs::create_dir(directory.0.join("account")).unwrap();
    fs::write(directory.0.join("z.dever"), "").unwrap();
    fs::write(directory.0.join("account/user.dever"), "").unwrap();
    fs::write(directory.0.join("ignored.json"), [0xff]).unwrap();
    let sources = SourceMap::load(&directory.0).unwrap();
    assert_eq!(
        sources
            .files()
            .iter()
            .map(|source| source.path().to_str().unwrap())
            .collect::<Vec<_>>(),
        ["account/user.dever", "z.dever"]
    );
    for source in sources.files() {
        dever_core::parse(source).unwrap();
    }
}

#[test]
fn source_loading_reports_invalid_utf8_and_missing_roots() {
    let directory = SourceDirectory::create();
    fs::write(directory.0.join("broken.dever"), [b'#', 0xff]).unwrap();
    let errors = SourceMap::load(&directory.0).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "invalid UTF-8 at byte 1");
    assert!(SourceMap::load(&directory.0.join("missing")).is_err());
    assert!(SourceMap::load(&directory.0.join("broken.dever")).is_err());
}

#[cfg(unix)]
#[test]
fn source_loading_does_not_follow_symbolic_links_outside_the_root() {
    let outside = SourceDirectory::create();
    let directory = SourceDirectory::create();
    fs::write(outside.0.join("outside.dever"), "").unwrap();
    std::os::unix::fs::symlink(&outside.0, directory.0.join("escape")).unwrap();
    let errors = SourceMap::load(&directory.0).unwrap_err();
    assert!(errors[0].message.contains("symbolic links"));
}
