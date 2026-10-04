use dever_core::diagnostic::Diagnostic;
use dever_core::source::SourceMap;
use dever_core::syntax::Declaration;

fn sources(files: &[(&str, &str)]) -> SourceMap {
    let mut sources = SourceMap::default();
    for (path, text) in files {
        sources.add(*path, *text);
    }
    sources
}

fn checked(files: &[(&str, &str)]) -> dever_core::hir::Program {
    let sources = sources(files);
    dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

fn rejected(files: &[(&str, &str)], code: &str, message: &str) -> Vec<Diagnostic> {
    let sources = sources(files);
    let errors = dever_core::check(&sources).expect_err("source must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.code == code && error.message.contains(message)),
        "expected {code}: {message}; got {errors:?}"
    );
    errors
}

fn markdown(package: &str, code: &str) -> String {
    format!(
        "# 问候\n\n提供问候文本。\n\n- 包：`{package}`\n- 公开类型：无\n- 公开方法：\n  - `greet`\n- 使用：\n  - `{package}.greet()`\n\n## 问候方法\n\n返回问候。\n\n- 函数：`greet`\n- 输入：无\n- 输出：\n  - `response: Text`：问候文本。\n\n```dever\n{code}\n```\n"
    )
}

#[test]
fn source_paths_define_packages_in_both_formats() {
    let document = markdown(
        "user.service.greeting",
        "public greet() (response: Text) { response = \"你好\" }",
    );
    for (path, text) in [
        (
            "user/service/greeting.dever",
            "public greet() (response: Text) { response = \"你好\" }",
        ),
        ("user/service/greeting.dever.md", document.as_str()),
    ] {
        let sources = sources(&[(path, text)]);
        let package = dever_core::parse(&sources.files()[0]).unwrap();
        assert_eq!(
            package
                .name
                .iter()
                .map(|name| name.text.as_str())
                .collect::<Vec<_>>(),
            ["user", "service", "greeting"]
        );
        checked(&[(path, text)]);
    }
    let sources = sources(&[("main.dever", "main() () {}")]);
    assert_eq!(
        dever_core::parse(&sources.files()[0]).unwrap().name[0].text,
        "main"
    );
}

#[test]
fn private_helpers_are_local_and_public_declarations_are_cross_package() {
    let service = "public type Greeting { message: Text }\npublic greet() (response: Greeting) { response = Greeting { message = helper() } }\nhelper() (response: Text) { response = \"hello\" }\n";
    let caller = "main() (response: user.service.greeting.Greeting) { response = user.service.greeting.greet() }";
    let program = checked(&[
        ("user/service/greeting.dever", service),
        ("main.dever", caller),
    ]);
    let snapshot = program.api_snapshot();
    assert!(snapshot.contains("function user.service.greeting.greet"));
    assert!(snapshot.contains("type user.service.greeting.Greeting"));
    assert!(!snapshot.contains("helper"));
    assert!(!snapshot.contains("function main.main"));
    rejected(
        &[
            ("user/service/greeting.dever", service),
            (
                "main.dever",
                "main() (response: Text) { response = user.service.greeting.helper() }",
            ),
        ],
        "C006",
        "private",
    );
    rejected(
        &[
            ("secret.dever", "type Hidden { value: Int }"),
            ("main.dever", "main(value: secret.Hidden) () {}"),
        ],
        "C006",
        "private",
    );
}

#[test]
fn public_visibility_must_match_every_clause_of_one_signature() {
    rejected(
        &[(
            "sample.dever",
            "public choose(value: true) (response: Int) { response = 1 }\nchoose(value: false) (response: Int) { response = 0 }",
        )],
        "C006",
        "same public visibility",
    );
    checked(&[(
        "sample.dever",
        "public choose(value: true) (response: Int) { response = 1 }\npublic choose(value: false) (response: Int) { response = 0 }",
    )]);
}

#[test]
fn public_boundaries_cannot_publish_private_types() {
    for declaration in [
        "public identity(value: Hidden) (response: Hidden) { response = value }",
        "public identity(value: List<Hidden>) (response: List<Hidden>) { response = value }",
        "public apply(action: handler(value: Hidden) (response: Hidden), value: Hidden) (response: Hidden) { response = action(value) }",
        "public type Published { value: Hidden }",
    ] {
        let source = format!("type Hidden {{ value: Int }}\n{declaration}");
        rejected(&[("sample.dever", &source)], "C006", "private type");
    }
    let program = checked(&[(
        "sample.dever",
        "type Hidden { value: Int }\npublic type Published { private hidden: Hidden }",
    )]);
    assert!(!program.api_snapshot().contains("sample.Hidden"));
}

#[test]
fn model_records_require_public_but_sql_result_records_remain_private() {
    rejected(
        &[("news/model/article.dever", "type Article { title: Text }")],
        "C014",
        "declared public",
    );
    checked(&[(
        "news/model/article.dever",
        "public type Article { title: Text }",
    )]);
    rejected(
        &[(
            "news/model/article.dever",
            "public type Article { title: Text }\npublic type Summary { count: Int }",
        )],
        "C014",
        "SQL result record must be private",
    );
}

#[test]
fn legacy_headers_and_non_declaration_public_modifiers_are_rejected() {
    for text in [
        "package app exposes (main)\nmain() () {}",
        "exposes(main)\nmain() () {}",
    ] {
        rejected(
            &[("app.dever", text)],
            "P001",
            "package/exposes declarations were removed",
        );
    }
    rejected(
        &[(
            "news/model/article.dever",
            "public database default\npublic type Article { title: Text }",
        )],
        "P001",
        "only on types and functions",
    );
}

#[test]
fn source_roots_and_standard_namespaces_cannot_be_spoofed() {
    for path in ["../outside.dever", "/outside.dever", "source.txt"] {
        rejected(&[(path, "main() () {}")], "C003", "relative .dever");
    }
    rejected(
        &[("dever/spoof.dever", "main() () {}")],
        "C003",
        "reserved standard",
    );
    rejected(
        &[("WrongName.dever", "main() () {}")],
        "C010",
        "lower_snake_case",
    );
    let document = markdown(
        "sample",
        "public greet() (response: Text) { response = \"hi\" }",
    );
    rejected(
        &[("sample.dever", ""), ("sample.dever.md", &document)],
        "C002",
        "duplicate package",
    );
}

#[test]
fn formatter_preserves_public_transaction_and_comment_positions() {
    let code = "# 公共接口\npublic type Result{private value:Int}\npublic transaction write(value:Int)(response:Int){response=value}\nhelper()(){ }\n";
    let sources = sources(&[("sample.dever", code)]);
    let formatted = dever_core::format::format(&sources.files()[0]).unwrap();
    assert!(formatted.starts_with("# 公共接口\npublic type Result"));
    assert!(formatted.contains("public transaction write(value: Int) (response: Int)"));
    assert!(!formatted.contains("package "));
    let mut second = SourceMap::default();
    second.add("sample.dever", &formatted);
    assert_eq!(
        dever_core::format::format(&second.files()[0]).unwrap(),
        formatted
    );
    let package = dever_core::parse(&second.files()[0]).unwrap();
    assert!(matches!(&package.declarations[0], Declaration::Type(ty) if ty.public));
    assert!(matches!(&package.declarations[1], Declaration::Function(function) if function.public));
    assert!(
        matches!(&package.declarations[2], Declaration::Function(function) if !function.public)
    );
}

#[test]
fn markdown_public_contract_and_source_locations_follow_headerless_code() {
    let document = markdown("sample", "public greet()(response:Text){response=\"你好\"}");
    checked(&[("sample.dever.md", &document)]);
    let sources = sources(&[("sample.dever.md", &document)]);
    let formatted = dever_core::format::format(&sources.files()[0]).unwrap();
    let prefix = document.split("```dever").next().unwrap();
    assert!(formatted.starts_with(prefix));
    checked(&[("sample.dever.md", &formatted)]);
    let private = document.replace("public greet", "greet");
    rejected(&[("sample.dever.md", &private)], "M006", "public methods");
    let invalid = document.replace("\"你好\"", "unknown");
    let errors = rejected(&[("sample.dever.md", &invalid)], "C004", "unknown");
    assert!(
        errors
            .iter()
            .any(|error| &invalid[error.primary.start..error.primary.end] == "unknown")
    );
    let h1_code = document.replace(
        "## 问候方法",
        "```dever\n# 不允许在 H1 下写代码\n```\n\n## 问候方法",
    );
    rejected(&[("sample.dever.md", &h1_code)], "M004", "0 top-level");
}

#[test]
fn markdown_cannot_split_a_public_declaration_across_blocks() {
    let document = markdown(
        "sample",
        "public\n```\n\n```dever\ngreet() (response: Text) { response = \"hi\" }",
    );
    rejected(&[("sample.dever.md", &document)], "P001", "identifier");
    let document = markdown(
        "sample",
        "public greet() (response: Text) {\n```\n\n```dever\nresponse = \"hi\"\n}",
    );
    rejected(
        &[("sample.dever.md", &document)],
        "M003",
        "complete within one",
    );
}
