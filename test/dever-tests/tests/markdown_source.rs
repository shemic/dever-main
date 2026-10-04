mod support;

use std::fs;
use std::path::Path;
use std::process::Command;

use dever_core::diagnostic::Diagnostic;
use dever_core::source::SourceMap;
use support::checked;
use support::temp::TemporaryDirectory;

const MAIN: &str = "main() (answer: Int) { answer = 42 }\n";

fn unstructured_document(blocks: &[&str]) -> String {
    let mut text = String::from("# 程序说明\n\n标题不是 package，表格不是类型。\n");
    for block in blocks {
        text.push_str("\n## 说明\n\n```dever\n");
        text.push_str(block);
        text.push_str("```\n\n这一段不执行。\n");
    }
    text
}

fn documented_list(label: &str, values: &[&str], described: bool) -> String {
    if values.is_empty() {
        return format!("- {label}：无\n");
    }
    let mut text = format!("- {label}：\n");
    for value in values {
        text.push_str(&format!(
            "  - `{value}`{}\n",
            if described { "：测试说明" } else { "" }
        ));
    }
    text
}

fn type_section(title: &str, name: &str, kind: &str, entries: &[&str], code: &str) -> String {
    format!(
        "## {title}\n\n测试类型的用途。\n\n- 类型：`{name}`\n{}\n```dever\n{code}```\n",
        documented_list(kind, entries, true)
    )
}

fn function_section(
    title: &str,
    name: &str,
    inputs: &[&str],
    outputs: &[&str],
    code: &str,
) -> String {
    format!(
        "## {title}\n\n测试函数的用途。\n\n- 函数：`{name}`\n{}{}\n```dever\n{code}```\n",
        documented_list("输入", inputs, true),
        documented_list("输出", outputs, true)
    )
}

fn package_document(
    package: &str,
    public_types: &[&str],
    public_functions: &[&str],
    usages: &[&str],
    sections: &[String],
) -> String {
    let mut text = format!(
        "# 测试程序\n\n测试包的用途。\n\n- 包：`{package}`\n{}{}{}",
        documented_list("公开类型", public_types, false),
        documented_list("公开方法", public_functions, false),
        documented_list("使用", usages, false),
    );
    for section in sections {
        text.push('\n');
        text.push_str(section);
    }
    text
}

fn main_document() -> String {
    package_document(
        "main",
        &[],
        &[],
        &[],
        &[function_section(
            "主函数",
            "main",
            &[],
            &["answer: Int"],
            MAIN,
        )],
    )
}

fn model_section(declaration: &str, code: &str, signature: Option<(&[&str], &[&str])>) -> String {
    let signature = signature.map_or_else(String::new, |(inputs, outputs)| {
        format!(
            "{}{}",
            documented_list("输入", inputs, true),
            documented_list("输出", outputs, true),
        )
    });
    format!(
        "## 存储声明\n\n约束存储结构和初始化行为。\n\n- 声明：`{declaration}`\n{signature}\n```dever\n{code}\n```\n"
    )
}

fn model_document() -> String {
    package_document(
        "app.item.model",
        &["Item"],
        &[],
        &[],
        &[
            model_section("database primary", "database primary", None),
            type_section(
                "项目",
                "Item",
                "字段",
                &["name: Text(1, 64)", "cost: Decimal(5, 2)"],
                "type Item { name: Text(1, 64) unique\n cost: Decimal(5, 2) default 0 }\n",
            ),
            model_section("index(cost, created_at)", "index(cost, created_at)", None),
            model_section("unique(name, cost)", "unique(name, cost)", None),
            model_section(
                "relation children = app.child.model.item_id",
                "relation children = app.child.model.item_id",
                None,
            ),
            model_section("seed", "seed { { name = \"初始\"\n cost = 1.25 } }", None),
            model_section(
                "migrate remove_obsolete",
                "migrate remove_obsolete { drop obsolete }",
                None,
            ),
            model_section(
                "sql by_name",
                concat!(
                    "sql by_name(name: Text) (item: Item?) {\n",
                    " sqlite = \"SELECT id, created_at, name, cost FROM item WHERE name = ?1\"\n",
                    " postgres = \"SELECT id, created_at, name, cost FROM item WHERE name = $1\"\n}\n"
                ),
                Some((&["name: Text"], &["item: Item?"])),
            ),
        ],
    )
}

fn model_sources(document: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("app/item/model.dever.md", document);
    sources.add(
        "app/child/model.dever",
        concat!("", "type Child { item_id: app.item.model.id }\n"),
    );
    sources.add(
        "app/item/app.dever",
        "count() (total: Int) { total = model.count() }\n",
    );
    sources.add(
        "main.dever",
        concat!("", "main() (total: Int) { total = app.item.count() }\n"),
    );
    sources
}

#[test]
fn markdown_models_preserve_all_storage_declarations_and_formatting() {
    let document = model_document();
    let sources = model_sources(&document);
    let program = dever_core::check(&sources)
        .unwrap_or_else(|errors| panic!("{}", render(&errors, &sources)));
    let snapshot = program.model_snapshot();
    assert!(snapshot.contains("model app.item.model name=Item"));
    assert!(snapshot.contains("connection=explicit:primary"));
    assert!(snapshot.contains("field cost Decimal(5,2)"));
    assert!(snapshot.contains("relation to-many children = app.child.model.item_id"));
    assert!(snapshot.contains("migrate remove_obsolete drop(obsolete)"));
    assert!(snapshot.contains("sql by_name"));
    let formatted = dever_core::format::format(&sources.files()[0]).unwrap();
    let reformatted_sources = model_sources(&formatted);
    assert_eq!(
        formatted,
        dever_core::format::format(&reformatted_sources.files()[0]).unwrap(),
    );
    assert_eq!(
        snapshot,
        dever_core::check(&reformatted_sources)
            .unwrap()
            .model_snapshot(),
    );

    let mut plain = SourceMap::default();
    let model = dever_core::parse(&sources.files()[0]).unwrap();
    let mut code = String::from("");
    for declaration in &model.declarations {
        let span = match declaration {
            dever_core::syntax::Declaration::Api(value) => value.span,
            dever_core::syntax::Declaration::Rest(value) => value.span,
            dever_core::syntax::Declaration::External(value) => value.span,
            dever_core::syntax::Declaration::Type(value) => value.span,
            dever_core::syntax::Declaration::Database(value) => value.span,
            dever_core::syntax::Declaration::Schedule(value) => value.span,
            dever_core::syntax::Declaration::ModelIndex(value) => value.span,
            dever_core::syntax::Declaration::Relation(value) => value.span,
            dever_core::syntax::Declaration::Seed(value) => value.span,
            dever_core::syntax::Declaration::Migration(value) => value.span,
            dever_core::syntax::Declaration::ModelSql(value) => value.span,
            dever_core::syntax::Declaration::Function(value) => value.span,
        };
        code.push_str(&document[span.start..span.end]);
        code.push('\n');
    }
    plain.add("app/item/model.dever", code);
    for source in &sources.files()[1..] {
        plain.add(source.path(), source.text());
    }
    let plain_program = dever_core::check(&plain).unwrap();
    assert_eq!(snapshot, plain_program.model_snapshot());
    assert_eq!(program.api_snapshot(), plain_program.api_snapshot());
    dever_core::native::emit(&program, &sources, "main.main").unwrap();
}

#[test]
fn markdown_model_preserves_global_scope() {
    let document = package_document(
        "platform.tenant.model",
        &["Tenant"],
        &[],
        &[],
        &[type_section(
            "租户",
            "Tenant",
            "字段",
            &["name: Text(1, 64)"],
            "global type Tenant { name: Text(1, 64) }\n",
        )],
    );
    let mut sources = SourceMap::default();
    let source = sources.add("platform/tenant/model.dever.md", document);
    let program = dever_core::check(&sources)
        .unwrap_or_else(|errors| panic!("{}", render(&errors, &sources)));
    assert!(
        program.model_snapshot().contains("scope=global"),
        "{}",
        program.model_snapshot(),
    );
    let formatted = dever_core::format::format(sources.get(source)).unwrap();
    assert!(formatted.contains("global type Tenant"), "{formatted}");
}

#[test]
fn markdown_model_metadata_is_checked_and_cannot_skip_documentation() {
    for (before, after, code) in [
        (
            "- 声明：`database primary`",
            "- 声明：`database other`",
            "M006",
        ),
        (
            "- 声明：`index(cost, created_at)`",
            "- 声明：`index(created_at, cost)`",
            "M006",
        ),
        (
            "- 声明：`unique(name, cost)`",
            "- 声明：`unique(cost, name)`",
            "M006",
        ),
        (
            "- 声明：`relation children = app.child.model.item_id`",
            "- 声明：`relation children = app.child.model.id`",
            "M006",
        ),
        ("- 声明：`seed`", "", "M005"),
        (
            "- 声明：`migrate remove_obsolete`",
            "- 声明：`migrate wrong`",
            "M006",
        ),
        ("- 声明：`sql by_name`", "- 声明：`sql wrong`", "M006"),
        (
            "  - `item: Item?`：测试说明",
            "  - `item: Item`：测试说明",
            "M008",
        ),
        (
            "  - `name: Text`：测试说明",
            "  - `name: Int`：测试说明",
            "M007",
        ),
        ("`cost: Decimal(5, 2)`", "`cost: Decimal(6, 2)`", "M008"),
        ("约束存储结构和初始化行为。", "", "M005"),
        ("default 0", "default 0.001", "C014"),
        ("cost = 1.25", "cost = 0.001", "C014"),
    ] {
        let sources = model_sources(&model_document().replace(before, after));
        let errors = dever_core::check(&sources).unwrap_err();
        assert!(
            errors.iter().any(|error| error.code == code),
            "{}",
            render(&errors, &sources),
        );
    }
}

fn markdown_sources(text: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("main.dever.md", text);
    sources
}

fn formatted(text: &str) -> String {
    let sources = markdown_sources(text);
    dever_core::format::format(&sources.files()[0]).unwrap_or_else(|errors| {
        panic!("{}", render(&errors, &sources));
    })
}

fn render(errors: &[Diagnostic], sources: &SourceMap) -> String {
    errors.iter().map(|error| error.render(sources)).collect()
}

fn assert_rejected(text: &str, code: &str) {
    let sources = markdown_sources(text);
    let errors = dever_core::parse(&sources.files()[0]).unwrap_err();
    assert!(
        errors.iter().any(|error| error.code == code),
        "{}",
        render(&errors, &sources)
    );
    assert!(dever_core::format::format(&sources.files()[0]).is_err());
}

fn assert_check_rejected(text: &str, code: &str) -> Vec<Diagnostic> {
    let sources = markdown_sources(text);
    let errors = dever_core::check(&sources).unwrap_err();
    assert!(
        errors.iter().any(|error| error.code == code),
        "{}",
        render(&errors, &sources)
    );
    errors
}

fn execute(sources: &SourceMap) -> std::process::Output {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let native =
        dever_core::native::compile(&checked(sources), sources, "main.main", &rustc).unwrap();
    Command::new(native.executable()).output().unwrap()
}

#[test]
fn markdown_and_plain_sources_have_identical_native_results_and_public_api() {
    let blocks = [
        "",
        "type Pair { original: Int\nchanged: Int }\n",
        "choose(value: true) (answer: Int) { answer = 7 }\nchoose(value: false) (answer: Int) { answer = 9 }\n",
        "main() (answer: Int, pair: Pair) {\nvalue = 2\nvalue = value + 1\nanswer = choose(true) * value\npair = Pair { original = 2\nchanged = value }\n}\n",
    ];
    let markdown_text = package_document(
        "main",
        &[],
        &[],
        &[],
        &[
            type_section(
                "数值对",
                "Pair",
                "字段",
                &["original: Int", "changed: Int"],
                blocks[1],
            ),
            function_section(
                "选择数值",
                "choose",
                &["value: Bool"],
                &["answer: Int"],
                blocks[2],
            ),
            function_section(
                "主函数",
                "main",
                &[],
                &["answer: Int", "pair: Pair"],
                blocks[3],
            ),
        ],
    );
    let markdown = markdown_sources(&markdown_text);
    let plain = support::sources(&blocks.concat());
    let expected = b"answer = 21\npair = main.Pair { original = 2, changed = 3 }\n";
    let left = execute(&plain);
    let right = execute(&markdown);
    assert!(left.status.success() && right.status.success());
    assert!(left.stderr.is_empty() && right.stderr.is_empty());
    assert_eq!(left.stdout, right.stdout);
    assert_eq!(right.stdout, expected);
    assert_eq!(
        checked(&plain).api_snapshot(),
        checked(&markdown).api_snapshot()
    );
}

#[test]
fn real_checkout_example_loads_typescript_highlighted_dever_blocks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/old/markdown");
    let sources = SourceMap::load(&root).unwrap();
    assert_eq!(sources.files().len(), 1);
    let package = dever_core::parse(&sources.files()[0]).unwrap();
    assert_eq!(package.declarations.len(), 6);
}

#[test]
fn markdown_contract_requires_one_code_declaration_per_documented_section() {
    let package_and_function = "# 测试程序\n\n测试包的用途。\n\n- 包：`main`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n```dever\nmain() (answer: Int) { answer = 42 }\n```\n";
    assert_rejected(package_and_function, "M004");

    let mixed_functions = package_document(
        "main",
        &[],
        &[],
        &[],
        &[function_section(
            "两个函数",
            "helper",
            &[],
            &["answer: Int"],
            "helper() (answer: Int) { answer = 1 }\nmain() (answer: Int) { answer = helper() }\n",
        )],
    );
    assert_rejected(&mixed_functions, "M005");

    let split_clauses = package_document(
        "main",
        &[],
        &[],
        &[],
        &[
            function_section(
                "真值分句",
                "choose",
                &["value: Bool"],
                &["answer: Int"],
                "choose(value: true) (answer: Int) { answer = 1 }\n",
            ),
            function_section(
                "假值分句",
                "choose",
                &["value: Bool"],
                &["answer: Int"],
                "choose(value: false) (answer: Int) { answer = 0 }\n",
            ),
        ],
    );
    assert_rejected(&split_clauses, "M005");
}

#[test]
fn markdown_contract_accepts_all_clauses_of_one_function_in_one_section() {
    let text = package_document(
        "main",
        &[],
        &[],
        &[],
        &[function_section(
            "选择数值",
            "choose",
            &["value: Bool"],
            &["answer: Int"],
            "choose(value: true) (answer: Int) { answer = 1 }\nchoose(value: false) (answer: Int) { answer = 0 }\n",
        )],
    );
    checked(&markdown_sources(&text));

    let inconsistent_names = text.replace("choose(value: false)", "choose(flag: false)");
    let errors = assert_check_rejected(&inconsistent_names, "M007");
    assert!(
        errors
            .iter()
            .find(|error| error.code == "M007")
            .is_some_and(|error| !error.related.is_empty())
    );
}

#[test]
fn markdown_contract_checks_package_name_public_api_and_usage_in_order() {
    for text in [
        main_document().replace("- 包：`main`", "- 包：`wrong`"),
        main_document().replace("- 公开方法：无", "- 公开方法：\n  - `main`"),
        main_document().replace("- 使用：无", "- 使用：\n  - `main.main()`"),
    ] {
        let errors = assert_check_rejected(&text, "M006");
        assert!(
            errors
                .iter()
                .filter(|error| error.code == "M006")
                .all(|error| !error.related.is_empty())
        );
    }
}

#[test]
fn markdown_contract_checks_type_fields_variants_and_descriptions() {
    let record = package_document(
        "main",
        &[],
        &[],
        &[],
        &[type_section(
            "数值对",
            "Pair",
            "字段",
            &["left: Int", "right: Int >= 0"],
            "type Pair {\nleft: Int\nright: Int >= 0\n}\n",
        )],
    );
    checked(&markdown_sources(&record));
    let stale_field = record.replace("`right: Int >= 0`", "`right: Int`");
    assert_check_rejected(&stale_field, "M008");
    let missing_description = record.replace("`left: Int`：测试说明", "`left: Int`：");
    assert_rejected(&missing_description, "M005");

    let choice = package_document(
        "main",
        &[],
        &[],
        &[],
        &[type_section(
            "处理结果",
            "Result",
            "分支",
            &["Ok(value: Int)", "error Failed(message: Text)"],
            "type Result {\nOk(value: Int)\nerror Failed(message: Text)\n}\n",
        )],
    );
    checked(&markdown_sources(&choice));
    assert_check_rejected(
        &choice.replace("`Ok(value: Int)`", "`Ok(value: Text)`"),
        "M008",
    );
}

#[test]
fn markdown_contract_checks_resolved_function_inputs_and_outputs() {
    let text = package_document(
        "main",
        &[],
        &[],
        &[],
        &[function_section(
            "选择数值",
            "choose",
            &["value: Bool"],
            &["answer: Int"],
            "choose(value: true) (answer: Int) { answer = 1 }\nchoose(value: false) (answer: Int) { answer = 0 }\n",
        )],
    );
    assert_check_rejected(&text.replace("`value: Bool`", "`value: Int`"), "M007");
    for stale_output in [
        text.replace("`answer: Int`", "`result: Int`"),
        text.replace("`answer: Int`", "`answer: Text`"),
    ] {
        assert_check_rejected(&stale_output, "M008");
    }

    let missing_purpose = main_document().replace("测试函数的用途。\n\n- 函数", "- 函数");
    assert_rejected(&missing_purpose, "M005");
}

#[test]
fn markdown_contract_uses_source_syntax_for_related_types() {
    let document = package_document(
        "main",
        &[],
        &[],
        &[],
        &[function_section(
            "关联名称",
            "name",
            &["value: Related<app.model.user.User>"],
            &["result: Text"],
            concat!(
                "name(value: Related.Unloaded<app.model.user.User>()) (result: Text) { ",
                "result = \"unloaded\" }\n",
                "name(value: Related.Loaded<app.model.user.User>(user)) (result: Text) { ",
                "result = user.name }\n",
            ),
        )],
    );
    let mut sources = markdown_sources(&document);
    sources.add(
        "app/model/user.dever",
        "public type User { name: Text(1, 64) }\n",
    );
    checked(&sources);

    let stale = document.replace(
        "value: Related<app.model.user.User>",
        "value: app.model.user.User",
    );
    let mut stale_sources = markdown_sources(&stale);
    stale_sources.add(
        "app/model/user.dever",
        "public type User { name: Text(1, 64) }\n",
    );
    let errors = dever_core::check(&stale_sources).unwrap_err();
    assert!(errors.iter().any(|error| error.code == "M007"));
}

#[test]
fn loading_ignores_regular_markdown_and_checks_nested_and_duplicate_packages() {
    let directory = TemporaryDirectory::new();
    fs::create_dir_all(directory.path().join("account/user")).unwrap();
    fs::write(
        directory.path().join("README.md"),
        "```dever\nnot a program\n```\n",
    )
    .unwrap();
    fs::write(directory.path().join("example.dever.md.bak"), "bad").unwrap();
    fs::write(
        directory.path().join("main.dever"),
        "main() (answer: Int) { answer = account.user.value() }\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("account/user/app.dever.md"),
        package_document(
            "account.user.app",
            &[],
            &["value"],
            &["account.user.value()"],
            &[function_section(
                "读取数值",
                "value",
                &[],
                &["answer: Int"],
                "value() (answer: Int) { answer = 42 }\n",
            )],
        ),
    )
    .unwrap();
    let sources = SourceMap::load(directory.path()).unwrap();
    assert_eq!(sources.files().len(), 2);
    checked(&sources);

    fs::write(directory.path().join("main.dever.md"), main_document()).unwrap();
    let sources = SourceMap::load(directory.path()).unwrap();
    let errors = dever_core::check(&sources).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == "C002" && error.message.contains("package")),
        "{}",
        render(&errors, &sources)
    );

    let mut wrong_path = SourceMap::default();
    wrong_path.add("wrong.dever.md", main_document());
    assert!(
        dever_core::check(&wrong_path)
            .unwrap_err()
            .iter()
            .any(|error| error.code == "M006")
    );
}

#[test]
fn type_coverage_and_failure_contracts_remain_mandatory() {
    let cases = [
        (
            "C005",
            package_document(
                "main",
                &[],
                &[],
                &[],
                &[function_section(
                    "主函数",
                    "main",
                    &[],
                    &["answer: Int"],
                    "main() (answer: Int) { answer = true }\n",
                )],
            ),
        ),
        (
            "C009",
            package_document(
                "main",
                &[],
                &[],
                &[],
                &[function_section(
                    "主函数",
                    "main",
                    &["value: Bool"],
                    &["answer: Int"],
                    "main(value: true) (answer: Int) { answer = 1 }\n",
                )],
            ),
        ),
        (
            "C012",
            package_document(
                "main",
                &[],
                &[],
                &[],
                &[
                    type_section(
                        "结果",
                        "Result",
                        "分支",
                        &["error Failed"],
                        "type Result { error Failed }\n",
                    ),
                    function_section(
                        "主函数",
                        "main",
                        &["value: Result"],
                        &[],
                        "main(value: Result) () { discarded = value }\n",
                    ),
                ],
            ),
        ),
    ];
    for (code, text) in cases {
        let sources = markdown_sources(&text);
        let errors = dever_core::check(&sources).unwrap_err();
        assert!(
            errors.iter().any(|error| error.code == code),
            "{}",
            render(&errors, &sources)
        );
    }
}

#[test]
fn source_locations_preserve_unicode_and_all_line_endings() {
    for newline in ["\n", "\r\n", "\r"] {
        let text = package_document(
            "main",
            &[],
            &[],
            &[],
            &[function_section(
                "函数",
                "main",
                &[],
                &["answer: Int"],
                "main() (answer: Int) {\n  label = \"中文\"\n  answer = missing(label)\n}\n",
            )],
        )
        .replace('\n', newline);
        let sources = markdown_sources(&text);
        let source = &sources.files()[0];
        assert_eq!(source.text(), text);
        let errors = dever_core::check(&sources).unwrap_err();
        let error = errors
            .iter()
            .find(|error| error.code == "C004")
            .unwrap_or_else(|| panic!("newline {newline:?}: {}", render(&errors, &sources)));
        assert_eq!(error.primary.start, text.find("missing").unwrap());
        assert_eq!(source.position(error.primary.start), (22, 12));
        assert!(error.render(&sources).contains("main.dever.md:"));

        let invalid = text.replace("missing(label)", "\"中文\" @");
        let sources = markdown_sources(&invalid);
        let errors = dever_core::parse(&sources.files()[0]).unwrap_err();
        let error = errors.iter().find(|error| error.code == "L001").unwrap();
        assert_eq!(error.primary.start, invalid.find('@').unwrap());
        assert_eq!(sources.files()[0].position(error.primary.start), (22, 17));
    }
}

#[test]
fn native_fault_points_to_original_markdown_line() {
    let text = package_document(
        "main",
        &[],
        &[],
        &[],
        &[
            function_section(
                "除法",
                "divide",
                &["left: Int", "right: Int"],
                &["answer: Int"],
                "divide(left: Int, right: Int) (answer: Int) {\n  answer = left // right\n}\n",
            ),
            function_section(
                "主函数",
                "main",
                &[],
                &["answer: Int"],
                "main() (answer: Int) { answer = divide(1, 0) }\n",
            ),
        ],
    )
    .replace('\n', "\r\n");
    let output = execute(&markdown_sources(&text));
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("main.dever.md:"), "{error}");
    assert!(error.contains("Int division by zero"), "{error}");
}

#[test]
fn showcase_fences_quotes_lists_html_and_other_languages_never_execute() {
    let examples = [
        "````markdown\n```dever\ninvalid @ program\n```\n````\n",
        "> ```dever\n> invalid @ program\n> ```\n",
        "- Example:\n\n  ```dever\n  invalid @ program\n  ```\n",
        "<pre>\n```dever\ninvalid @ program\n```\n</pre>\n",
        "<!--\n```dever\ninvalid @ program\n```\n-->\n",
        "<div>\n```dever\ninvalid @ program\n```\n</div>\n",
        "```rust\ninvalid @ program\n```\n",
        "```rust dever\ninvalid @ program\n```\n",
        "```\ninvalid @ program\n```\n",
        "    ```dever\n    invalid @ program\n    ```\n",
        "```dever example\ninvalid @ program\n```\n",
        "```&#100;ever\ninvalid @ program\n```\n",
        "~~~dever\\\ninvalid @ program\n~~~\n",
    ];
    for example in examples {
        let text = format!("{example}\n{}", main_document());
        let sources = markdown_sources(&text);
        checked(&sources);
        assert!(formatted(&text).starts_with(example), "{example}");
    }
}

#[test]
fn fences_accept_commonmark_indentation_tildes_and_longer_closures() {
    // After a Markdown list, CommonMark treats two or three spaces as list nesting.
    for indentation in ["", " "] {
        for marker in ["```", "~~~~"] {
            for newline in ["\n", "\r\n", "\r"] {
                let text = format!(
                    "# 说明\n\n测试包的用途。\n\n- 包：`main`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n## 主函数\n\n测试函数的用途。\n\n- 函数：`main`\n- 输入：无\n- 输出：\n  - `answer: Int`：测试说明\n\n\n{indentation}{marker} dever \t\n{MAIN}{indentation}{marker}{}   \n尾文",
                    &marker[..1]
                )
                .replace('\n', newline);
                let sources = markdown_sources(&text);
                if let Err(errors) = dever_core::check(&sources) {
                    panic!(
                        "indentation={indentation:?}, marker={marker:?}, newline={newline:?}: {}",
                        render(&errors, &sources)
                    );
                }
                let output = formatted(&text);
                assert_eq!(formatted(&output), output);
                assert!(output.ends_with(&format!(
                    "{indentation}{marker}{}   {newline}尾文",
                    &marker[..1]
                )));
                assert!(output.contains(&format!("{indentation}  answer = 42{newline}")));
            }
        }
    }
    for indentation in ["  ", "   "] {
        let text = format!("# 说明\n\n说明。\n\n{indentation}```dever\n{indentation}```\n");
        let sources = markdown_sources(&text);
        let errors = dever_core::parse(&sources.files()[0]).unwrap_err();
        assert!(
            errors.iter().any(|error| error.code == "M004"),
            "{}",
            render(&errors, &sources)
        );
        assert!(!errors.iter().any(|error| error.code == "M001"));
    }
}

#[test]
fn no_program_block_and_unclosed_program_fences_are_explicit_errors() {
    for text in ["", "# 只有说明\n", "```text\nexample\n```\n"] {
        assert_rejected(text, "M001");
    }
    for text in [
        "```dever",
        "```dever\n",
        "```dever\n",
        "```dever\n~~~\n",
        "````dever\n```\n",
        "```dever\n``` extra",
        "```dever\n```\t\n",
        "```dever\n```\t",
        "```dever\n    ```\n",
    ] {
        assert_rejected(text, "M002");
    }
    assert_rejected(
        "# 空包\n\n用于测试空包。\n\n- 包：`main`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n```dever\n```",
        "M004",
    );
    assert_rejected("```dever\n```\n", "M004");
}

#[test]
fn declarations_cannot_straddle_program_blocks() {
    for blocks in [
        vec!["main() (answer: Int) {\n", "answer = 42\n}\n"],
        vec!["type Pair {\n", "value: Int\n}\n", MAIN],
        vec!["main(\n", ") (answer: Int) { answer = 42 }\n"],
    ] {
        assert_rejected(&unstructured_document(&blocks), "M003");
    }
}

#[test]
fn lexical_failures_stay_inside_their_own_program_block() {
    let text =
        unstructured_document(&["main() (answer: Text) { answer = \"unfinished\n", "\"\n}\n"]);
    assert_rejected(&text, "L005");
    let sources = markdown_sources(&text);
    for error in dever_core::parse(&sources.files()[0]).unwrap_err() {
        assert!(
            !sources.files()[0]
                .line(sources.files()[0].position(error.primary.start).0)
                .starts_with("这一段")
        );
    }
}

#[test]
fn formatter_preserves_prose_fences_and_block_local_comments() {
    let text = "# 不改标题  \r\n\r\n测试包的用途。\r\n\r\n- 包：`main`\r\n- 公开类型：无\r\n- 公开方法：无\r\n- 使用：无\r\n\r\n## 主函数\r\n\r\n测试函数的用途。\r\n\r\n- 函数：`main`\r\n- 输入：无\r\n- 输出：\r\n  - `answer: Int`：返回值\r\n\r\n~~~dever \t\r\n# 函数前\r\nmain( )(answer:Int){answer=42 # 赋值\r\n# 函数内尾\r\n}\r\n# 函数后\r\n~~~~   \r\n\r\n```rust\r\nleave( this );\r\n```\r\n尾文不加换行";
    let output = formatted(text);
    assert_eq!(formatted(&output), output);
    let expected = text.replace(
        "main( )(answer:Int){answer=42 # 赋值\r\n# 函数内尾\r\n}",
        "main() (answer: Int) {\r\n  answer = 42 # 赋值\r\n# 函数内尾\r\n}",
    );
    assert_eq!(output, expected);
    checked(&markdown_sources(&output));
}
