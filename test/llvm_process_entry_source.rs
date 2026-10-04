//! Checked process-entry lowering; real execution belongs to the CLI regression.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::temp;
    use dever_backend_bridge::{Target, emit_object};
    use dever_core::{hir::Program, llvm, source::SourceMap};
    use std::fs;

    const APPLICATION: &[(&str, &str)] = &[
        (
            "sample/echo/app.dever",
            r#"type Outcome { error Denied(message: Text, labels: List<Text?>) }
echo(value: Text) (answer: Text) {
  dever.task.sleep(1)
  answer = value
}
reject() (answer: Text) { fail(Outcome.Denied("denied 你好", ["first", null])) }
divide(divisor: Int) (answer: Int) { answer = 42 // divisor }
"#,
        ),
        (
            "sample/echo/api.dever",
            "cmd echo = app.echo\ncmd reject = app.reject\ncmd divide = app.divide",
        ),
    ];
    const TESTS: &[(&str, &str)] = &[
        (
            "sample/echo/pass.dever",
            "pass() () { assert_eq(app.echo(\"@dever_entry %dever.fault\"), \"@dever_entry %dever.fault\") }",
        ),
        (
            "sample/echo/assertion.dever",
            "assertion() () {\n  assert_eq([\"actual\"], [\"expected\"])\n}",
        ),
        (
            "sample/echo/reject.dever",
            "reject() () { value = app.reject() }",
        ),
    ];

    fn checked(tests: Option<&[(&str, &str)]>) -> (temp::TemporaryDirectory, SourceMap, Program) {
        let directory = temp::TemporaryDirectory::new();
        for (root, files) in [("module", APPLICATION), ("test", tests.unwrap_or(&[]))] {
            fs::create_dir(directory.path().join(root)).unwrap();
            for (name, contents) in files {
                let path = directory.path().join(root).join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, contents).unwrap();
            }
        }
        let sources = SourceMap::load_project(
            &directory.path().join("module"),
            &directory.path().join("test"),
        )
        .unwrap();
        let program = dever_core::check(&sources).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&sources))
                    .collect::<String>()
            )
        });
        (directory, sources, program)
    }

    #[test]
    fn executable_entries_emit_objects_for_all_six_targets() {
        let (_directory, sources, program) = checked(Some(TESTS));
        for ir in [
            llvm::emit_executable(&program, &sources).unwrap(),
            llvm::emit_test_executable(&program, &sources).unwrap(),
        ] {
            for target in Target::ALL {
                assert!(!emit_object(&ir, target).unwrap().is_empty());
            }
        }
    }

    #[test]
    fn zero_cases_keep_the_existing_compile_boundary() {
        let (_directory, sources, program) = checked(None);
        assert!(program.tests().is_empty());
        assert_eq!(
            llvm::emit_test_executable(&program, &sources).unwrap_err(),
            "cannot compile an empty test suite"
        );
    }

    #[test]
    fn callable_entries_do_not_acquire_a_process_main() {
        let (_directory, sources, program) = checked(Some(TESTS));
        for ir in [
            llvm::emit_application(&program, &sources).unwrap(),
            llvm::emit_test_suite(&program, &sources).unwrap(),
        ] {
            assert!(!ir.contains("define i32 @main("));
            assert!(!ir.contains("@dever_rt_v1_process_test_index("));
            assert!(!ir.contains("@dever_rt_v1_process_init("));
        }
    }
}
