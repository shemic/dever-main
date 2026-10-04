use std::fs;
use std::path::{Path as FilePath, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use dever_core::source::{SourceMap, Span};
use dever_core::syntax::*;

#[path = "../../../crates/dever-cli/src/format.rs"]
mod cli_format;

fn parsed_at(path: &FilePath, text: &str) -> Package {
    let mut sources = SourceMap::default();
    let id = sources.add(path, text);
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

fn formatted_at(path: &FilePath, text: &str) -> String {
    let mut sources = SourceMap::default();
    let id = sources.add(path, text);
    dever_core::format::format(sources.get(id)).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    })
}

fn assert_roundtrip(source: &str) -> String {
    assert_roundtrip_at(FilePath::new("main.dever"), source)
}

fn assert_roundtrip_at(path: &FilePath, source: &str) -> String {
    let output = formatted_at(path, source);
    assert_eq!(
        shape(parsed_at(path, source)),
        shape(parsed_at(path, &output)),
        "{output}"
    );
    assert_eq!(
        formatted_at(path, &output),
        output,
        "formatter must be idempotent"
    );
    output
}

#[test]
fn canonical_layout_preserves_field_evaluation_order_and_groups() {
    let source = "type Pair{a:Int\nb:Int}\npublic main( )(result:Pair){result=Pair{b=second()\na=first()}}\n";
    assert_eq!(
        assert_roundtrip(source),
        "type Pair {\n  a: Int\n  b: Int\n}\n\npublic main() (result: Pair) {\n  result = Pair {\n    b = second()\n    a = first()\n  }\n}\n"
    );
}

#[test]
fn all_syntax_and_literal_forms_roundtrip_without_semantic_checking() {
    let source = r#"type Empty {}
type Event {Ready(bytes: Bytes, code: Int)
End()
Failed(message: Text)}
public run(route: handler(value: Int) (answer: Int), stream: Stream<Event>, limit: Int >= -2 and < 8) (result: List<MapEntry<Text, Int?>>) {
values = [1, 2, -3, 1.20, 3e-2, 5.0]
mapping = {"z" = 1
"a" = 2}
result = []
empty = {}
truth = not false and true or false
maths = -(-2) + 3 * (4 - 5) / 6 // 7 % 8
compare = (1 < 2) == (3 <= 4) != (5 > 6) == (7 >= 8)
literal = "引号\" 斜杠\\ 换行\n回车\r制表\t零\u{0}控制\u{1f}字符\u{4e2d}"
maybe = null
record = app.Pair {b = route(2)
a = route(1)}
record.a = route(3)
call = (route(1)).answer
each(consume, stream)
}
pick(event: Event.Ready(bytes, code)) (value: Int) {value=code}
pick(event: other) (value: Int) {value=-1}
constant(value: -2) (result: Int) {result=2}
empty_generic(value: List<>) () {}
"#;
    let output = assert_roundtrip(source);
    assert!(output.contains("\\u{0}"));
    assert!(output.contains("b = route(2)\n    a = route(1)"));
    assert!(output.contains("End()"));
}

#[test]
fn application_declarations_roundtrip_with_full_structure() {
    for (path, source) in [
        ("system/health/api.dever", "cmd ping=app.ping\n"),
        ("news/article/api.dever", "rest model\n"),
        (
            "notification/mail/adapter/worker.dever",
            "external exec \"worker\" {lib \"pip:fixture@1.0.0\"\nallow network}\n",
        ),
        ("user/session/job.dever", "schedule cleanup=\"0 * * * *\"\n"),
    ] {
        assert_roundtrip_at(FilePath::new(path), source);
    }
}

#[test]
fn comments_inside_expressions_signatures_and_eof_are_preserved() {
    let source = r#"# 模块说明
# 类型说明
type Empty { # 空类型
# 保留空类型内部注释
}
public main( # 输入说明
value: handler( # 回调输入
value: Int # 数值
) (answer: Int), # 回调输出
values: List< # 元素类型
Int
>) (result: Int) { # 正文
  result=( # 分组
    1 # 第一项
    + # 运算符
    2
  ) # 结果
  list=[ # 列表
    1, # 成员
    # 下一成员
    2 # 最后一项
  ]
  call=f(1 # 逗号之前
  ,2)
  mapping={ # 映射
    "key"=2 # 映射值
    # 结束映射
  }
  # 结束正文
} # 结束函数
# 文件结尾
# 无换行结尾"#;
    let output = assert_roundtrip(source);
    assert!(output.starts_with("# 模块说明\n# 类型说明\ntype Empty"));
    assert!(output.ends_with("# 文件结尾\n# 无换行结尾\n"));
}

#[test]
fn long_positional_lists_wrap_with_two_space_indentation() {
    let source = "public main() () {send(\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\", \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\")}\n";
    let output = assert_roundtrip(source);
    assert!(output.contains("  send(\n    \"aaaaaaaa"));
    assert!(output.contains("\",\n    \"bbbbbbbb"));
    assert!(output.ends_with("\n  )\n}\n"));
}

#[test]
fn line_endings_comment_whitespace_and_unicode_are_canonical() {
    for newline in ["\n", "\r\n", "\r"] {
        let source = [
            "# 中文注释  ",
            "",
            "main() (value: Text) {value=\"# 😀\\u{2028}\\u{7f}\"} # 尾注释\t",
            "",
        ]
        .join(newline);
        let output = assert_roundtrip(&source);
        assert!(!output.contains('\r'));
        assert!(output.starts_with("# 中文注释\n"));
        assert!(output.ends_with("} # 尾注释\n"));
    }
}

#[test]
fn current_examples_official_packages_and_complete_contract_roundtrip() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for relative in ["library", "examples"] {
        let sources = SourceMap::load(&root.join(relative)).unwrap();
        for source in sources.files() {
            let path = source_root_relative(source.path());
            assert_roundtrip_at(&path, source.text());
        }
    }
    let prd = include_str!("../../../.trellis/tasks/09-04-dever-language-mvp/prd.md");
    let (_, example) = prd.split_once("## Complete Package Example\n").unwrap();
    let (_, fenced) = example.split_once("```dever\n").unwrap();
    assert_roundtrip(fenced.split_once("\n```").unwrap().0);
}

fn source_root_relative(path: &FilePath) -> PathBuf {
    let segments = path.iter().collect::<Vec<_>>();
    let start = segments
        .iter()
        .enumerate()
        .filter_map(|(index, segment)| {
            matches!(segment.to_str(), Some("module" | "test")).then_some(index + 1)
        })
        .next_back()
        .unwrap_or(0);
    segments[start..].iter().collect()
}

#[test]
fn invalid_source_never_produces_formatted_output() {
    for text in ["package", "main() () {x =}", "# comment\n$"] {
        let mut sources = SourceMap::default();
        let id = sources.add("broken.dever", text);
        assert!(dever_core::format::format(sources.get(id)).is_err());
    }
}

#[test]
fn cli_prepares_all_files_before_writes_and_check_is_read_only() {
    let temporary = Temporary::new();
    let module = temporary.0.join("module");
    fs::create_dir(&module).unwrap();
    let valid = "";
    fs::write(module.join("main.dever"), valid).unwrap();
    fs::write(module.join("broken.dever"), "package").unwrap();
    fs::write(module.join("unrelated.txt"), "unchanged").unwrap();
    let sources = SourceMap::load_project(&module, &temporary.0.join("test")).unwrap();
    assert!(cli_format::execute(&temporary.0, &sources, false).is_err());
    assert_eq!(
        fs::read_to_string(module.join("main.dever")).unwrap(),
        valid
    );
    fs::remove_file(module.join("broken.dever")).unwrap();
    let sources = SourceMap::load_project(&module, &temporary.0.join("test")).unwrap();
    assert_eq!(
        cli_format::execute(&temporary.0, &sources, true).unwrap(),
        ExitCode::FAILURE
    );
    assert_eq!(
        fs::read_to_string(module.join("main.dever")).unwrap(),
        valid
    );
    assert_eq!(
        cli_format::execute(&temporary.0, &sources, false).unwrap(),
        ExitCode::SUCCESS
    );
    let updated = SourceMap::load_project(&module, &temporary.0.join("test")).unwrap();
    assert_eq!(
        cli_format::execute(&temporary.0, &updated, true).unwrap(),
        ExitCode::SUCCESS
    );
    assert_eq!(
        fs::read_to_string(module.join("unrelated.txt")).unwrap(),
        "unchanged"
    );
    assert_eq!(fs::read_dir(&module).unwrap().count(), 2);
}

#[test]
fn source_path_set_changes_prevent_all_rewrites() {
    let temporary = Temporary::new();
    let module = temporary.0.join("module");
    fs::create_dir(&module).unwrap();
    let original = "";
    let existing = module.join("main.dever");
    let added = module.join("added.dever");
    fs::write(&existing, original).unwrap();
    let snapshot = SourceMap::load_project(&module, &temporary.0.join("test")).unwrap();
    fs::write(&added, "package").unwrap();
    assert!(cli_format::execute(&temporary.0, &snapshot, false).is_err());
    assert_eq!(fs::read_to_string(&existing).unwrap(), original);
    assert_eq!(fs::read_to_string(&added).unwrap(), "package");

    fs::remove_file(&added).unwrap();
    fs::rename(&existing, &added).unwrap();
    assert!(cli_format::execute(&temporary.0, &snapshot, false).is_err());
    assert_eq!(fs::read_to_string(&added).unwrap(), original);
    assert!(!existing.exists());
}

#[test]
fn changed_snapshot_and_read_only_files_prevent_all_rewrites() {
    let temporary = Temporary::new();
    let module = temporary.0.join("module");
    fs::create_dir(&module).unwrap();
    let original = "";
    let first = module.join("first.dever");
    let second = module.join("second.dever");
    fs::write(&first, original).unwrap();
    fs::write(&second, original).unwrap();
    let sources = SourceMap::load_project(&module, &temporary.0.join("test")).unwrap();
    fs::write(&second, "edited").unwrap();
    assert!(cli_format::execute(&temporary.0, &sources, false).is_err());
    assert_eq!(fs::read_to_string(&first).unwrap(), original);
    fs::write(&second, original).unwrap();
    let permissions = fs::metadata(&second).unwrap().permissions();
    let mut read_only = permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&second, read_only).unwrap();
    let result = cli_format::execute(&temporary.0, &sources, false);
    fs::set_permissions(&second, permissions).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&first).unwrap(), original);
    assert_eq!(fs::read_dir(&module).unwrap().count(), 2);
}

#[test]
fn module_and_test_sources_are_prepared_before_any_rewrite() {
    let temporary = Temporary::new();
    let module = temporary.0.join("module");
    let tests = temporary.0.join("test/user/account");
    fs::create_dir(&module).unwrap();
    fs::create_dir_all(&tests).unwrap();
    let main = module.join("main.dever");
    let test = tests.join("register.dever");
    let unformatted_main = "main()(){ }";
    let unformatted_test = "register()(){assert(true)}";
    fs::write(&main, unformatted_main).unwrap();
    fs::write(&test, unformatted_test).unwrap();
    let sources = SourceMap::load_project(&module, &temporary.0.join("test")).unwrap();

    let permissions = fs::metadata(&test).unwrap().permissions();
    let mut read_only = permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&test, read_only).unwrap();
    let result = cli_format::execute(&temporary.0, &sources, false);
    fs::set_permissions(&test, permissions).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&main).unwrap(), unformatted_main);
    assert_eq!(fs::read_to_string(&test).unwrap(), unformatted_test);

    cli_format::execute(&temporary.0, &sources, false).unwrap();
    assert_eq!(fs::read_to_string(&main).unwrap(), "main() () {}\n");
    assert_eq!(
        fs::read_to_string(&test).unwrap(),
        "register() () {\n  assert(true)\n}\n"
    );
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_rejected_and_permissions_are_preserved() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temporary = Temporary::new();
    let module = temporary.0.join("module");
    fs::create_dir(&module).unwrap();
    let original = "";
    let file = module.join("main.dever");
    fs::write(&file, original).unwrap();
    symlink(&file, module.join("linked.dever")).unwrap();
    assert!(SourceMap::load_project(&module, &temporary.0.join("test")).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
    fs::remove_file(module.join("linked.dever")).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    let sources = SourceMap::load_project(&module, &temporary.0.join("test")).unwrap();
    cli_format::execute(&temporary.0, &sources, false).unwrap();
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

struct Temporary(PathBuf);

impl Temporary {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dever-formatter-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

// Compare full syntax structure and decoded literals independently of source positions.
fn shape(mut package: Package) -> Package {
    let span = Span {
        start: 0,
        end: 0,
        ..package.span
    };
    package.span = span;
    normalize_path(&mut package.name, span);
    for comment in &mut package.comments {
        comment.span = span;
        comment.text = comment.text.trim_end().to_owned();
    }
    for declaration in &mut package.declarations {
        match declaration {
            Declaration::Api(binding) => {
                binding.span = span;
                binding.action.span = span;
                normalize_path(&mut binding.target, span);
            }
            Declaration::Rest(rest) => {
                rest.span = span;
                if let Some(model) = &mut rest.model {
                    normalize_path(model, span);
                }
            }
            Declaration::External(external) => {
                external.span = span;
                external.entry_span = span;
                for name in external.libs.iter_mut().chain(&mut external.capabilities) {
                    name.span = span;
                }
            }
            Declaration::Type(declaration) => {
                declaration.span = span;
                declaration.name.span = span;
                match &mut declaration.shape {
                    TypeShape::Record(fields) => normalize_fields(fields, span),
                    TypeShape::Choice(variants) => {
                        for variant in variants {
                            variant.span = span;
                            variant.name.span = span;
                            normalize_fields(&mut variant.payload, span);
                        }
                    }
                }
            }
            Declaration::Function(function) => {
                function.span = span;
                function.name.span = span;
                normalize_fields(&mut function.outputs, span);
                if let Some(failure) = &mut function.fails {
                    normalize_type(failure, span);
                }
                for input in &mut function.inputs {
                    input.span = span;
                    input.name.span = span;
                    match &mut input.kind {
                        InputKind::Handler(signature) => {
                            signature.span = span;
                            normalize_fields(&mut signature.inputs, span);
                            normalize_fields(&mut signature.outputs, span);
                        }
                        InputKind::Value(pattern) => match pattern {
                            Pattern::Typed { ty, bounds } => {
                                normalize_type(ty, span);
                                for bound in bounds {
                                    bound.span = span;
                                }
                            }
                            Pattern::Variant {
                                name,
                                arguments,
                                bindings,
                                span: pattern_span,
                            } => {
                                normalize_path(name, span);
                                for argument in arguments {
                                    normalize_type(argument, span);
                                }
                                normalize_path(bindings, span);
                                *pattern_span = span;
                            }
                            Pattern::Literal {
                                span: pattern_span, ..
                            }
                            | Pattern::Other(pattern_span) => *pattern_span = span,
                        },
                    }
                }
                for statement in &mut function.body {
                    statement.span = span;
                    match &mut statement.kind {
                        StatementKind::Assign { target, value } => {
                            normalize_path(target, span);
                            normalize_expression(value, span);
                        }
                        StatementKind::Call(call) => normalize_expression(call, span),
                    }
                }
            }
            Declaration::Database(binding) => {
                binding.span = span;
                binding.name.span = span;
            }
            Declaration::Schedule(schedule) => {
                schedule.span = span;
                schedule.target.span = span;
            }
            Declaration::ModelIndex(index) => {
                index.span = span;
                normalize_path(&mut index.fields, span);
            }
            Declaration::Relation(relation) => {
                relation.span = span;
                relation.name.span = span;
                normalize_path(&mut relation.field, span);
            }
            Declaration::Seed(seed) => {
                seed.span = span;
                for row in &mut seed.rows {
                    for field in row {
                        field.span = span;
                        field.name.span = span;
                        normalize_expression(&mut field.value, span);
                    }
                }
            }
            Declaration::Migration(migration) => {
                migration.span = span;
                migration.name.span = span;
                for operation in &mut migration.operations {
                    match operation {
                        MigrationOperation::Drop(field) => field.span = span,
                        MigrationOperation::Sql(sql) => {
                            sql.span = span;
                            sql.sqlite.span = span;
                            sql.postgres.span = span;
                            for parameter in &mut sql.parameters {
                                normalize_expression(parameter, span);
                            }
                        }
                    }
                }
            }
            Declaration::ModelSql(sql) => {
                sql.span = span;
                sql.name.span = span;
                normalize_fields(&mut sql.inputs, span);
                normalize_fields(&mut sql.outputs, span);
                sql.sqlite.span = span;
                sql.postgres.span = span;
            }
        }
    }
    package
}

fn normalize_path(path: &mut Path, span: Span) {
    for name in path {
        name.span = span;
    }
}

fn normalize_fields(fields: &mut [Field], span: Span) {
    for field in fields {
        field.span = span;
        field.name.span = span;
        normalize_type(&mut field.ty, span);
        for bound in &mut field.bounds {
            bound.span = span;
        }
        if let Some(value) = &mut field.storage.default {
            normalize_expression(value, span);
        }
        if let Some(from) = &mut field.storage.from {
            from.span = span;
        }
        for expression in [
            &mut field.storage.owner,
            &mut field.storage.create,
            &mut field.storage.replace,
            &mut field.storage.search,
        ]
        .into_iter()
        .flatten()
        {
            normalize_expression(expression, span);
        }
    }
}

fn normalize_type(ty: &mut TypeRef, span: Span) {
    ty.span = span;
    normalize_path(&mut ty.name, span);
    for argument in &mut ty.arguments {
        normalize_type(argument, span);
    }
    for parameter in &mut ty.parameters {
        parameter.span = span;
    }
}

fn normalize_expression(expression: &mut Expression, span: Span) {
    expression.span = span;
    match &mut expression.kind {
        ExpressionKind::Literal(_) => {}
        ExpressionKind::Name(path) => normalize_path(path, span),
        ExpressionKind::Group(value)
        | ExpressionKind::Fail(value)
        | ExpressionKind::CaptureResult(value)
        | ExpressionKind::Unary { value, .. } => normalize_expression(value, span),
        ExpressionKind::Contextual(ContextualExpression::Channel { element, capacity }) => {
            normalize_type(element, span);
            normalize_expression(capacity, span);
        }
        ExpressionKind::Field { value, name } => {
            normalize_expression(value, span);
            name.span = span;
        }
        ExpressionKind::Call {
            function,
            arguments,
        } => {
            normalize_expression(function, span);
            for argument in arguments {
                normalize_expression(argument, span);
            }
        }
        ExpressionKind::Record { name, fields } => {
            normalize_path(name, span);
            for field in fields {
                field.span = span;
                field.name.span = span;
                normalize_expression(&mut field.value, span);
            }
        }
        ExpressionKind::List(values) => {
            for value in values {
                normalize_expression(value, span);
            }
        }
        ExpressionKind::Map(entries) => {
            for entry in entries {
                entry.span = span;
                normalize_expression(&mut entry.key, span);
                normalize_expression(&mut entry.value, span);
            }
        }
        ExpressionKind::Binary { left, right, .. } => {
            normalize_expression(left, span);
            normalize_expression(right, span);
        }
    }
}
