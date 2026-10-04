#[path = "support/temp.rs"]
mod temp;

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;

use dever_core::diagnostic::Diagnostic;
use dever_core::source::SourceMap;
use temp::TemporaryDirectory;

fn project(module: &[(&str, &str)], tests: &[(&str, &str)]) -> (TemporaryDirectory, SourceMap) {
    let root = TemporaryDirectory::new();
    write_sources(&root.path().join("module"), module);
    if !tests.is_empty() {
        write_sources(&root.path().join("test"), tests);
    }
    let sources =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    (root, sources)
}

fn write_sources(root: &Path, files: &[(&str, &str)]) {
    fs::create_dir_all(root).unwrap();
    for (relative, text) in files {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

fn checked(sources: &SourceMap) -> dever_core::hir::Program {
    dever_core::check(sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(sources))
                .collect::<String>()
        )
    })
}

fn rejected(sources: &SourceMap, code: &str, message: &str) -> Vec<Diagnostic> {
    let errors = dever_core::check(sources).expect_err("source must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.code == code && error.message.contains(message)),
        "expected {code}: {message}; got {}",
        errors
            .iter()
            .map(|error| error.render(sources))
            .collect::<String>()
    );
    errors
}

#[test]
fn discovers_filename_entries_in_stable_order_and_allows_no_tests() {
    let (_root, sources) = project(&[("main.dever", "main() () {}")], &[]);
    assert!(checked(&sources).tests().is_empty());

    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[
            (
                "user/account/register.dever",
                "register() () { assert(true) }",
            ),
            (
                "news/article/publish.dever",
                "publish() () { assert_eq(1, 1) }",
            ),
        ],
    );
    assert_eq!(
        checked(&sources)
            .tests()
            .iter()
            .map(|test| test.name())
            .collect::<Vec<_>>(),
        ["news/article/publish", "user/account/register"]
    );
}

#[test]
fn rejects_invalid_test_paths_and_entry_contracts() {
    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[("user/account/nested/register.dever", "register() () {}")],
    );
    rejected(&sources, "C003", "test source must be");

    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[("user/account/register.dever", "helper_entry() () {}")],
    );
    rejected(
        &sources,
        "C005",
        "must declare the ordinary 'register() ()' entry",
    );

    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[("user/account/register.dever", "register(value: Int) () {}")],
    );
    rejected(&sources, "C005", "zero-input, zero-output");
}

#[test]
fn accepts_documented_markdown_tests_and_rejects_duplicate_topics() {
    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[(
            "user/account/register.dever.md",
            r#"# 注册账户测试

验证账户注册行为。

- 包：`user.account.register`
- 公开类型：无
- 公开方法：无
- 使用：无

## 注册账户

执行注册断言。

- 函数：`register`
- 输入：无
- 输出：无

```dever
register() () { assert(true) }
```
"#,
        )],
    );
    assert_eq!(checked(&sources).tests()[0].name(), "user/account/register");

    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[
            ("user/account/register.dever", "register() () {}"),
            (
                "user/account/register.dever.md",
                r#"# 注册账户测试

验证账户注册行为。

- 包：`user.account.register`
- 公开类型：无
- 公开方法：无
- 使用：无

## 注册账户

执行注册断言。

- 函数：`register`
- 输入：无
- 输出：无

```dever
register() () {}
```
"#,
            ),
        ],
    );
    rejected(&sources, "C002", "duplicate package");
}

#[test]
fn test_visibility_uses_domain_boundaries_without_exposing_test_helpers() {
    let (_root, sources) = project(
        &[
            ("main.dever", "main() () {}"),
            (
                "user/account/domain.dever",
                "normalize(value: Text) (result: Text) { result = text.lower(text.trim(value)) }",
            ),
            (
                "user/account/app.dever",
                "normalize(value: Text) (result: Text) { result = domain.normalize(value) }",
            ),
            (
                "news/article/app.dever",
                "label() (result: Text) { result = \"article\" }",
            ),
        ],
        &[(
            "user/account/normalize.dever",
            r#"normalize() () {
  assert_eq(app.normalize(" Editor "), "editor")
  assert_eq(domain.normalize(" ADMIN "), "admin")
  assert_eq(news.article.label(), "article")
  helper()
}
helper() () {}"#,
        )],
    );
    assert_eq!(checked(&sources).tests().len(), 1);

    let (_root, sources) = project(
        &[
            ("main.dever", "main() () {}"),
            (
                "news/article/domain.dever",
                "secret() (result: Text) { result = \"private\" }",
            ),
        ],
        &[(
            "user/account/steal.dever",
            "steal() () { assert_eq(news.article.domain.secret(), \"private\") }",
        )],
    );
    rejected(&sources, "C006", "not callable from this source role");

    let (_root, sources) = project(
        &[
            ("main.dever", "main() () {}"),
            ("user/account/model.dever", "type Account { email: Text }"),
        ],
        &[(
            "user/account/count.dever",
            "count() () { assert_eq(model.count(), 0) }",
        )],
    );
    rejected(&sources, "C006", "Model operations are private");

    let (_root, sources) = project(
        &[
            ("main.dever", "main() () {}"),
            (
                "user/account/app.dever",
                "leak() () { user.account.register.helper() }",
            ),
        ],
        &[(
            "user/account/register.dever",
            "register() () {}\nhelper() () {}",
        )],
    );
    rejected(&sources, "C006", "not callable from this source role");

    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[
            (
                "user/account/register.dever",
                "register() () {}\nhelper() () {}",
            ),
            (
                "user/account/profile.dever",
                "profile() () { user.account.register.helper() }",
            ),
        ],
    );
    rejected(&sources, "C006", "not callable from this source role");
}

#[test]
fn assertions_are_test_only_exact_and_comparable() {
    let (_root, sources) = project(&[("main.dever", "main() () { assert(true) }")], &[]);
    rejected(&sources, "C004", "unknown function 'assert'");

    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[("user/account/value.dever", "value() () { assert(1) }")],
    );
    rejected(&sources, "C005", "expected Bool");

    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[(
            "user/account/types.dever",
            "types() () { assert_eq(1, 1.0) }",
        )],
    );
    rejected(&sources, "C005", "same type on both sides");

    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[(
            "user/account/stream.dever",
            "stream() () { assert_eq(stream([1]), stream([1])) }",
        )],
    );
    rejected(&sources, "C005", "assert_eq does not support");
}

#[test]
fn tests_cannot_start_the_application_api() {
    let (_root, sources) = project(
        &[("main.dever", "main() () {}")],
        &[(
            "user/account/server.dever",
            "server() () { dever.api.serve() }",
        )],
    );
    rejected(&sources, "C006", "cannot start the application HTTP API");
}

#[test]
fn tests_use_existing_result_choices_for_business_failures() {
    let (_root, sources) = project(
        &[
            ("main.dever", "main() () {}"),
            (
                "user/account/app.dever",
                r#"type RegistrationResult {
  Created(account: Int)
  error Invalid(message: Text)
}
register(value: 0) (account: Int) {
  fail(RegistrationResult.Invalid("invalid account"))
}
register(value: other) (account: Int) {
  account = value
}"#,
            ),
        ],
        &[(
            "user/account/register.dever",
            r#"register() () {
  verify(result(app.register(0)))
}
verify(result: app.RegistrationResult.Invalid(message)) () recover("test verifies expected failure") {
  assert_eq(message, "invalid account")
}
verify(result: app.RegistrationResult.Created(account)) () recover("test verifies expected failure") {
  assert(false)
}"#,
        )],
    );
    assert_eq!(checked(&sources).tests().len(), 1);
}

#[test]
fn native_test_suite_dispatches_cases_reports_locations_and_reuses_cache() {
    let (_root, sources) = project(
        &[
            ("main.dever", "main() () {}"),
            ("user/profile/model.dever", "type Profile { name: Text }"),
        ],
        &[
            (
                "news/article/publish.dever",
                "publish() () { assert_eq(1, 1) }",
            ),
            (
                "user/account/register.dever",
                r#"type Pair {
  left: Int
  right: Int
}
type Number {
  Present(value: Int)
  Missing
}
register() () {
  actual = Pair {
    left = 1
    right = 2
  }
  expected = Pair {
    left = 1
    right = 2
  }
  assert_eq(actual, expected)
  assert_eq(Number.Present(1), Number.Present(1))
  assert_eq(first([1]), first([1]))
  assert_eq(missing(), null)
  assert_eq([1, 2], [1, 2])
  values = { "one" = 1 }
  assert_eq(values, { "one" = 1 })
  assert_eq(entries(values), entries({ "one" = 1 }))
  assert_eq(1, 2)
}
missing() (value: Int?) {
  value = null
}"#,
            ),
        ],
    );
    let program = checked(&sources);
    assert!(!program.tests()[0].uses_database());
    assert!(!program.tests()[1].uses_database());
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let native = dever_core::native::compile_test_suite(&program, &sources, &rustc).unwrap();
    let success = Command::new(native.executable()).arg("0").output().unwrap();
    assert!(success.status.success());
    let output = Command::new(native.executable()).arg("1").output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("test/user/account/register.dever:26:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("assert_eq failed: actual = 1, expected = 2"),
        "{stderr}"
    );
    drop(native);
    let cached = dever_core::native::compile_test_suite(&program, &sources, &rustc).unwrap();
    assert!(cached.cache_hit());
}
