use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::process::Command;

use dever_core::hir::Program;
use dever_core::source::SourceMap;

const HELLO: &str = "greet() () {\n  dever.io.println(\"hello\")\n}\n";

fn sources(files: &[(&str, &str)]) -> SourceMap {
    let mut sources = SourceMap::default();
    for (path, text) in files {
        sources.add(path, *text);
    }
    sources
}

fn checked(sources: &SourceMap) -> Program {
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

fn compile(sources: &SourceMap) -> dever_core::native::NativeProgram {
    compile_entry(sources, "app.user.main")
}

fn compile_entry(sources: &SourceMap, entry: &str) -> dever_core::native::NativeProgram {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    dever_core::native::compile(&checked(sources), sources, entry, &rustc).unwrap()
}

#[test]
fn hello_example_is_a_native_executable_with_exact_stdout_and_cleanup() {
    let sources = sources(&[("app/user/app.dever", HELLO)]);
    let native = compile_entry(&sources, "app.user.greet");
    let path = native.executable().to_owned();
    let output = Command::new(&path).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(output.stdout, b"hello\n");
    assert!(output.stderr.is_empty());

    let saved = path.parent().unwrap().join("saved-program");
    native.save(&saved).unwrap();
    let original = fs::read(&saved).unwrap();
    assert_eq!(
        native.save(&saved).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&saved).unwrap(), original);
    assert_eq!(Command::new(&saved).output().unwrap().stdout, b"hello\n");
    drop(native);
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn text_calls_resolve_forward_across_packages_and_preserve_value_semantics() {
    let sources = sources(&[
        (
            "app/user/app.dever",
            r#"main() () {
  message = "first"
  saved = message
  message = "second"
  show(message)
  show(saved)
  echo.change(message)
  show(message)
  self("quote: \" slash: \\ newline: \n tab: \t null: \u{0} unicode: \u{4e2d}")
  show()
}
show(message: Text) () {
  echo.say(message)
}
show() () {
  echo.say("")
}
self(value: Text) () {
  echo.say(value)
}
"#,
        ),
        (
            "app/echo/app.dever",
            r#"say(message: Text) () {
  dever.io.println(message)
}
change(message: Text) () {
  message = "changed"
  say(message)
}
"#,
        ),
    ]);
    let native = compile(&sources);
    let output = Command::new(native.executable()).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "second\nfirst\nchanged\nsecond\nquote: \" slash: \\ newline: \n tab: \t null: \0 unicode: \u{4e2d}\n\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn invalid_calls_and_expressions_fail_before_native_compilation() {
    for (body, code, message) in [
        ("missing(\"hello\")", "C004", "unknown function"),
        ("dever.io.println(1)", "C005", "expected Text"),
        ("dever.io.println(true)", "C005", "expected Text"),
        ("dever.io.println()", "C004", "unknown function"),
        ("dever.io.println(\"a\", \"b\")", "C004", "unknown function"),
        ("dever.io.println(message)", "C004", "uninitialized local"),
        ("message = message", "C004", "uninitialized local"),
        ("message = \"a\"\nmessage = 1", "C005", "expected Text"),
        (
            "message = dever.io.println(\"a\")",
            "C005",
            "zero-output action cannot be assigned",
        ),
        ("dever.io.println(\"a\" + 1)", "C005", "numeric operands"),
        ("message = \"a\"\nmessage()", "C005", "not callable"),
        ("message.value = \"a\"", "C004", "uninitialized local"),
    ] {
        let source = format!("main() () {{\n  {body}\n}}\n");
        let sources = sources(&[("app/user/app.dever", &source)]);
        let errors = dever_core::check(&sources).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == code && error.message.contains(message)),
            "{body}: {errors:?}"
        );
        assert!(errors[0].render(&sources).contains("app/user/app.dever:"));
    }
}

#[test]
fn diagnostics_point_to_the_actual_invalid_argument() {
    let sources = sources(&[(
        "app/user/app.dever",
        "main() () {\n  dever.io.println(42)\n}\n",
    )]);
    let errors = dever_core::check(&sources).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]
            .render(&sources)
            .starts_with("app/user/app.dever:2:20: error[C005]: expected Text, found Int\n")
    );
}

#[test]
fn invalid_declarations_are_rejected_even_when_unused() {
    for (declarations, expected) in [
        ("unused() (result: Text) {}", "uninitialized"),
        ("unused(value: Unknown) () {}", "unknown type"),
        ("unused(value: null) () {}", "cannot infer"),
        ("unused(value: \"a\") () {}", "do not cover"),
        ("unused(value: Text, value: Text) () {}", "duplicate local"),
        ("unused() () {}\nunused() () {}", "overlap"),
        ("type User { name: Missing }", "unknown type"),
        ("unused() () { missing() }", "unknown function"),
    ] {
        let source = format!("main() () {{}}\n{declarations}\n");
        let sources = sources(&[("app/user/app.dever", &source)]);
        let errors = dever_core::check(&sources).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "{declarations}: {errors:?}"
        );
    }
}

#[test]
fn package_paths_namespaces_and_exposures_are_checked() {
    for (path, source, message) in [
        ("dever/io/app.dever", "main() () {}", "reserved"),
        ("text.dever", "main() () {}", "reserved"),
        (
            "app/user/app.dever",
            "exposes (main)\nmain() () {}",
            "package/exposes declarations were removed",
        ),
        (
            "app/user/app.dever",
            "public main() () {}",
            "visibility is defined by the source role",
        ),
    ] {
        let sources = sources(&[(path, source)]);
        let errors = dever_core::check(&sources).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(message)),
            "{errors:?}"
        );
    }
    let loose = sources(&[("user.dever", "main() () {}")]);
    let application =
        SourceMap::from_compilation_sources(&loose.compilation_sources().unwrap()).unwrap();
    assert!(
        dever_core::check(&application)
            .unwrap_err()
            .iter()
            .any(|error| error.message.contains("application source must be"))
    );
    let sources = sources(&[("app/user/app.dever", ""), ("app/user/app.dever", "")]);
    assert!(
        dever_core::check(&sources)
            .unwrap_err()
            .iter()
            .any(|error| error.message == "duplicate package")
    );
}

#[test]
fn private_functions_cannot_be_called_from_another_package() {
    let sources = sources(&[
        (
            "app/user/app.dever",
            "main() () { app.echo.domain.hidden() }",
        ),
        ("app/echo/domain.dever", "hidden() () {}"),
    ]);
    assert!(
        dever_core::check(&sources)
            .unwrap_err()
            .iter()
            .any(|error| error.code == "C006")
    );
}

#[test]
fn recursion_and_package_cycles_are_rejected() {
    for body in ["main()", "next()"] {
        let source = format!("main() () {{ {body} }}\nnext() () {{ main() }}");
        let sources = sources(&[("app/user/app.dever", &source)]);
        assert!(
            dever_core::check(&sources)
                .unwrap_err()
                .iter()
                .any(|error| error.message.contains("recursive"))
        );
    }
    let sources = sources(&[
        (
            "app/first/app.dever",
            "start() () { second.middle() }\nfinish() () {}",
        ),
        ("app/second/app.dever", "middle() () { first.finish() }"),
    ]);
    let errors = dever_core::check(&sources).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("cyclic package"));
}

#[test]
fn entry_requires_a_public_zero_input_function() {
    let sources = sources(&[
        (
            "app/user/app.dever",
            "main() () {}\nwith_input(message: Text) () {}",
        ),
        ("app/user/domain.dever", "hidden() () {}"),
    ]);
    let program = checked(&sources);
    assert!(dever_core::native::emit(&program, &sources, "app.user.main").is_ok());
    for (entry, message) in [
        ("app.user.domain.hidden", "must be public"),
        ("app.user.with_input", "zero-input"),
        ("missing.main", "zero-input"),
    ] {
        assert!(
            dever_core::native::emit(&program, &sources, entry)
                .unwrap_err()
                .contains(message)
        );
    }
}

#[test]
fn missing_toolchain_reports_an_actionable_error() {
    let sources = sources(&[("app/user/app.dever", "main() () {}")]);
    let result = dever_core::native::compile(
        &checked(&sources),
        &sources,
        "app.user.main",
        std::ffi::OsStr::new("/nonexistent-dever-test/rustc"),
    );
    let Err(error) = result else {
        panic!("missing compiler succeeded")
    };
    assert!(error.contains("set RUSTC"));
}

struct FailingOutput {
    fail_write: bool,
}

impl Write for FailingOutput {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(buffer.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::Other.into())
    }
}

#[test]
fn runtime_preserves_utf8_and_propagates_write_and_flush_failures() {
    let mut bytes = Vec::new();
    dever_runtime::write_line(&mut bytes, "hello \u{4e2d}\n").unwrap();
    assert_eq!(bytes, "hello \u{4e2d}\n\n".as_bytes());
    for (fail_write, expected) in [
        (true, io::ErrorKind::BrokenPipe),
        (false, io::ErrorKind::Other),
    ] {
        assert_eq!(
            dever_runtime::write_line(&mut FailingOutput { fail_write }, "hello")
                .unwrap_err()
                .kind(),
            expected
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn native_output_failure_has_dever_location_and_nonzero_exit() {
    let sources = sources(&[("app/user/app.dever", HELLO)]);
    let native = compile_entry(&sources, "app.user.greet");
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = Command::new(native.executable())
        .stdout(full)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("app/user/app.dever:2:3:"), "{error}");
    assert!(error.contains("standard output failed"), "{error}");
}
