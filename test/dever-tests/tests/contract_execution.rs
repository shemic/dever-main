mod support;

use dever_core::source::SourceMap;

#[test]
fn cms_formats_share_source_model_api_and_configuration_contracts() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cms");
    let plain = SourceMap::load(&root.join("dever/module")).unwrap();
    let markdown = SourceMap::load(&root.join("md/module")).unwrap();
    let plain_settings =
        dever_runtime::config::Settings::load_project(&root.join("dever")).unwrap();
    let markdown_settings =
        dever_runtime::config::Settings::load_project(&root.join("md")).unwrap();
    let plain_program =
        dever_core::check_with_settings(&plain, &plain_settings).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&plain))
                    .collect::<String>()
            )
        });
    let markdown_program = dever_core::check_with_settings(&markdown, &markdown_settings)
        .unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&markdown))
                    .collect::<String>()
            )
        });
    assert_no_application_errors(&plain_program, &plain);
    assert_no_application_errors(&markdown_program, &markdown);
    assert_eq!(
        plain_program.model_snapshot(),
        markdown_program.model_snapshot(),
    );
    assert_eq!(
        plain_program.api_snapshot(),
        markdown_program.api_snapshot()
    );
    // 应用测试也是双源码合同的一部分，避免 Markdown 测试退化为空断言。
    let plain_with_tests =
        SourceMap::load_project(&root.join("dever/module"), &root.join("dever/test")).unwrap();
    let markdown_with_tests =
        SourceMap::load_project(&root.join("md/module"), &root.join("md/test")).unwrap();
    assert_eq!(
        canonical_cms_sources(&plain_with_tests),
        canonical_cms_sources(&markdown_with_tests),
    );

    let configuration = |format: &str| -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(root.join(format).join("config/setting.json")).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(configuration("dever"), configuration("md"));
    for (format, program, settings) in [
        ("dever", &plain_program, &plain_settings),
        ("md", &markdown_program, &markdown_settings),
    ] {
        let profile = program.validate_database_settings(settings).unwrap();
        assert!(profile.sqlite && !profile.postgres, "{format}");
        let sources = if format == "dever" { &plain } else { &markdown };
        let generated = dever_core::native::emit_project(program, sources, profile).unwrap();
        assert!(generated.contains(
            "dever_runtime::job::scope_system(None, cmd_dispatch(&args[0], &args[1])).await"
        ));
        assert!(generated.contains(
            "dever_runtime::lifecycle::serve_both(dever_runtime::api::serve(api_route), job_serve()).await"
        ));
    }
}

fn assert_no_application_errors(program: &dever_core::hir::Program, sources: &SourceMap) {
    let errors = program.application_errors().unwrap();
    assert!(
        errors.is_empty(),
        "{}",
        errors
            .iter()
            .map(|error| error.render(sources))
            .collect::<String>()
    );
}

fn canonical_cms_sources(sources: &SourceMap) -> std::collections::BTreeMap<String, String> {
    use dever_core::syntax::Declaration;

    let mut declarations = std::collections::BTreeMap::new();
    for source in sources.files() {
        // 使用编译器 AST 的边界抽取声明，避免测试另写 Markdown fence 解析器。
        let package = dever_core::parse(source).unwrap();
        let name = package
            .name
            .iter()
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let mut code = String::new();
        for declaration in package.declarations {
            let span = match declaration {
                Declaration::Type(value) => value.span,
                Declaration::Function(value) => value.span,
                Declaration::External(value) => value.span,
                Declaration::Api(value) => value.span,
                Declaration::Rest(value) => value.span,
                Declaration::Database(value) => value.span,
                Declaration::Schedule(value) => value.span,
                Declaration::ModelIndex(value) => value.span,
                Declaration::Relation(value) => value.span,
                Declaration::Seed(value) => value.span,
                Declaration::Migration(value) => value.span,
                Declaration::ModelSql(value) => value.span,
            };
            code.push_str(&source.text()[span.start..span.end]);
            code.push('\n');
        }
        let mut normalized = SourceMap::default();
        normalized.add(format!("{}.dever", name.replace('.', "/")), code);
        let canonical = dever_core::format::format(&normalized.files()[0]).unwrap();
        assert!(declarations.insert(name, canonical).is_none());
    }
    declarations
}

#[test]
fn maintained_examples_pass_the_same_contract_checker() {
    for root in example_roots() {
        let source_root = root.join("module");
        let program = support::checked(&SourceMap::load(&source_root).unwrap());
        let baseline = root.join("dever.api");
        if baseline.exists() {
            program
                .check_api(&std::fs::read_to_string(baseline).unwrap())
                .unwrap();
        }
    }
}

fn examples_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/old/dever")
}

fn example_roots() -> Vec<std::path::PathBuf> {
    let examples = examples_root()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    vec![examples.join("cms/dever"), examples.join("cms/md")]
}

fn archived_sources(root: &std::path::Path) -> SourceMap {
    copy_archived_sources(root, false)
}

fn archived_rejected_sources(root: &std::path::Path) -> SourceMap {
    copy_archived_sources(root, true)
}

fn copy_archived_sources(root: &std::path::Path, remap_main: bool) -> SourceMap {
    let loaded = SourceMap::load(root).unwrap();
    let mut sources = SourceMap::default();
    for source in loaded.files() {
        let path = if remap_main && source.path() == std::path::Path::new("main.dever") {
            std::path::Path::new("app.dever")
        } else {
            source.path()
        };
        sources.add(path, source.text());
    }
    sources
}

#[test]
fn rejected_examples_demonstrate_the_documented_diagnostic() {
    for (name, code) in [
        ("range", "C011"),
        ("failure", "C012"),
        ("pure", "C013"),
        ("coverage", "C009"),
        ("private", "C006"),
    ] {
        let sources =
            archived_rejected_sources(&examples_root().join("rejected").join(name).join("module"));
        let errors = dever_core::check(&sources).unwrap_err();
        assert!(
            errors.iter().any(|error| error.code == code),
            "{name}: {errors:#?}"
        );
    }
}

#[test]
fn constraints_and_explicit_recovery_preserve_native_execution() {
    assert_eq!(
        run_example("contracts", "main.main"),
        "configured = domain.Port { value = 8080 }\ninvalid = domain.PortResult.Failed(port must be between 1 and 65535)\nfallback = domain.Port { value = 8080 }\n"
    );
}

fn run_example(name: &str, entry: &str) -> String {
    let sources = archived_sources(&examples_root().join(name).join("module"));
    let program = support::checked(&sources);
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let native = dever_core::native::compile(&program, &sources, entry, &compiler).unwrap();
    let output = std::process::Command::new(native.executable())
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .arg("sample")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn source_roots_and_sequence_examples_preserve_expected_native_results() {
    for (name, entry, expected) in [
        ("hello", "main.main", "hello\n"),
        ("privacy", "main.main", "before = 0\nafter = 1\n"),
        (
            "sequences",
            "main.main",
            "mapped = [2, 4, 6]\ntotal = 6\nstopped = main.Summary { total = 6, count = 3 }\nempty_total = 0\nbytes_total = 131\nfirst_value = 2\nmissing = null\n",
        ),
        ("stream", "main.main", "total = 208\n"),
        (
            "library",
            "main.main",
            "json = {\"message\":\"你好 Dever\",\"total\":6}\n",
        ),
    ] {
        assert_eq!(run_example(name, entry), expected, "{name}");
    }
}

#[test]
fn text_example_preserves_unicode_null_and_explicit_conversion_failures() {
    let output = run_example("text", "main.main");
    assert!(output.starts_with("cleaned = dever\ncharacter = 😀\nslice = 中😀\npieces = [a, , b]\njoined = a||b\nparsed = 42\ninvalid = null\nexact = 1.25\nrounded = 2.34\nutf8 = 你好\n"), "{output}");
    assert!(
        output.contains("invalid_utf8 = dever.bytes.DecodeResult.Failed("),
        "{output}"
    );
    assert!(
        output.contains("invalid_byte = dever.bytes.BufferResult.Failed("),
        "{output}"
    );
}

#[test]
fn parallel_example_joins_every_named_task_before_done() {
    let output = run_example("parallel", "main.main");
    let mut lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.pop(), Some("done"));
    lines.sort();
    assert_eq!(lines, ["alpha", "beta", "gamma"]);
}

#[test]
fn async_example_owns_tasks_groups_and_channels_until_completion() {
    assert_eq!(
        run_example("async", "main.main"),
        "task_value = 7\ngrouped_value = 21\ncomputed = 144\n"
    );
}

#[test]
fn system_example_reads_child_inputs_and_returns_invalid_sleep_as_failure() {
    let output = run_example("system", "main.main");
    assert!(output.contains("arguments = [sample]"), "{output}");
    assert!(output.contains("clock = "), "{output}");
    assert!(
        output.contains("invalid_sleep = dever.time.SleepResult.Failed("),
        "{output}"
    );
}
