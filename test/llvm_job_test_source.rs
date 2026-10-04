//! Existing Job and application Test contracts through the LLVM runtime archive.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm_module.rs"]
mod llvm_module;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/sqlite_inspect.rs"]
mod sqlite_inspect;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::{llvm_module, process, sqlite_inspect, temp};
    use dever_backend_bridge::{Target, emit_object};
    use dever_core::{hir::Program, source::SourceMap};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::time::Duration;

    const SQLITE: &str = r#"{"database":{"default":{"type":"sqlite","path":"data/db/case.db","max_connections":2}}}"#;
    const PURE_MODULE: &[(&str, &str)] = &[
        ("main.dever", "main() () {}"),
        (
            "unrelated/account/model.dever",
            "type Account { email: Text }",
        ),
    ];
    const PURE_TESTS: &[(&str, &str)] = &[
        (
            "user/account/success.dever",
            r#"type Pair { left: Int
  right: Text }
type Number { Present(value: Int)
  Missing }
success() () {
  assert(true)
  assert_eq(length("@dever_entry %dever.fault"), 25)
  assert_eq(Pair { left = 1
    right = "@dever_entry %dever.fault" }, Pair { left = 1
    right = "@dever_entry %dever.fault" })
  assert_eq(Number.Present(1), Number.Present(1))
  assert_eq(first([1]), first([1]))
  assert_eq([1, 2], [1, 2])
  assert_eq({ "one" = 1 }, { "one" = 1 })
  dever.test.advance_clock(1000)
  assert_eq(dever.test.drain_jobs(5), 0)
}"#,
        ),
        (
            "user/account/failure.dever",
            "failure() () {\n  assert_eq(1, 2)\n}",
        ),
        (
            "user/account/boolean.dever",
            "boolean() () {\n  assert(false)\n}",
        ),
        (
            "user/account/business.dever",
            "type Outcome { error Denied(message: Text) }\nbusiness() () { fail(Outcome.Denied(\"owned failure payload\")) }",
        ),
    ];

    const JOB_MODULE: &[(&str, &str)] = &[
        ("main.dever", "main() () {}"),
        (
            "content/article/model.dever",
            "type Article { title: Text }",
        ),
        (
            "content/article/app.dever",
            r#"type Publish { title: Text
  article_id: model.id
  tags: List<Text?> }
type DeliveryError { error Failed(message: Text) }
transaction submit(title: Text) (id: Id) {
  article = model.create({ title = title })
  id = dever.job.enqueue(job.publish, Publish { title = title
    article_id = article.id
    tags = ["source", null] }, title)
}
transaction later(title: Text) (id: Id) {
  article = model.create({ title = title })
  id = dever.job.enqueue_at(job.publish, Publish { title = title
    article_id = article.id
    tags = ["source", null] }, title, dever.time.parse_datetime("1970-01-01T00:00:01Z"))
}
transaction abort() () {
  id = submit("rolled-back")
  fail(DeliveryError.Failed("abort the whole transaction"))
}
apply(input: Publish) () {
  original = model.get(input.article_id)
  marker = model.create({ title = "started" })
  article = model.create({ title = port.title(original.title) })
}
count(title: Text) (total: Int) { total = model.count({ where = title == title }) }
arm() (id: Id) { id = dever.job.enqueue(job.cleanup, "manual") }
clean() () { article = model.create({ title = "cron" }) }
"#,
        ),
        (
            "content/article/job.dever",
            "database default\njob publish(input: app.Publish) () retry(2) timeout(1000) { app.apply(input) }\njob cleanup() () retry(2) timeout(1000) { app.clean() }\nschedule cleanup = \"* * * * *\"",
        ),
        (
            "content/article/port.dever",
            "title(value: Text) (title: Text) fails app.DeliveryError",
        ),
        (
            "content/article/adapter.dever",
            "external exec \"worker/must-not-start\" {}",
        ),
    ];

    const JOB_TESTS: &[(&str, &str)] = &[
        (
            "content/article/empty.dever",
            "empty() () { dever.test.advance_clock(1000)\n  assert_eq(dever.test.drain_jobs(5), 0) }",
        ),
        (
            "content/article/first.dever",
            r#"port.title(value: Text) (title: Text) { title = "first" }
first() () {
  assert_eq(app.count("first"), 0)
  first = app.submit("same-key")
  duplicate = app.submit("same-key")
  assert_eq(first, duplicate)
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count("first"), 1)
  assert_eq(app.count("started"), 1)
  dever.test.advance_clock(1000)
  assert_eq(dever.test.drain_jobs(5), 0)
}"#,
        ),
        (
            "content/article/second.dever",
            r#"port.title(value: Text) (title: Text) { title = "second" }
second() () {
  assert_eq(app.count("first"), 0)
  id = app.submit("same-key")
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count("second"), 1)
}"#,
        ),
        (
            "content/article/delayed.dever",
            r#"port.title(value: Text) (title: Text) { title = "delivered" }
delayed() () {
  id = app.later("later")
  assert_eq(dever.test.drain_jobs(5), 0)
  dever.test.advance_clock(999)
  assert_eq(dever.test.drain_jobs(5), 0)
  dever.test.advance_clock(1)
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count("delivered"), 1)
}"#,
        ),
        (
            "content/article/retry.dever",
            r#"port.title(value: Text) (title: Text) { fail(app.DeliveryError.Failed("retry")) }
retry() () {
  id = app.submit("retry-key")
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count("started"), 0)
  assert_eq(dever.test.drain_jobs(5), 0)
  dever.test.advance_clock(1000)
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count("started"), 0)
  dever.test.advance_clock(10000)
  assert_eq(dever.test.drain_jobs(5), 0)
}"#,
        ),
        (
            "content/article/scheduled.dever",
            r#"scheduled() () {
  id = app.arm()
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count("cron"), 1)
  dever.test.advance_clock(60000)
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count("cron"), 2)
  assert_eq(dever.test.drain_jobs(5), 0)
}"#,
        ),
        (
            "content/article/abort.dever",
            "port.title(value: Text) (title: Text) { title = value }\nabort() () { app.abort() }",
        ),
    ];

    fn sources(
        module: &[(&str, &str)],
        tests: &[(&str, &str)],
    ) -> (temp::TemporaryDirectory, SourceMap, Program) {
        let directory = temp::TemporaryDirectory::new();
        for (root, files) in [("module", module), ("test", tests)] {
            fs::create_dir(directory.path().join(root)).unwrap();
            for (path, text) in files {
                let path = directory.path().join(root).join(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, text).unwrap();
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

    struct Suite {
        _directory: temp::TemporaryDirectory,
        executable: PathBuf,
        cases: Vec<(String, bool)>,
    }

    impl Suite {
        fn compile(module: &[(&str, &str)], tests: &[(&str, &str)], iterations: usize) -> Self {
            let (directory, sources, program) = sources(module, tests);
            let ir = dever_core::llvm::emit_test_suite(&program, &sources).unwrap();
            let executable = llvm_module::link_with_driver(
                &directory,
                &ir,
                "one compiled application test suite",
                iterations,
                "test-suite-driver.c",
            );
            let cases = program
                .tests()
                .iter()
                .map(|case| (case.name().to_owned(), case.uses_database()))
                .collect();
            Self {
                _directory: directory,
                executable,
                cases,
            }
        }

        fn stage(&self, setting: Option<&str>) -> temp::TemporaryDirectory {
            let directory = temp::TemporaryDirectory::new();
            fs::copy(&self.executable, directory.path().join("suite")).unwrap();
            if let Some(setting) = setting {
                fs::create_dir(directory.path().join("config")).unwrap();
                fs::write(directory.path().join("config/setting.json"), setting).unwrap();
            }
            directory
        }

        fn run(&self, directory: &Path, name: &str, expected_status: i32) -> Output {
            let index = self
                .cases
                .iter()
                .position(|(case, _)| case == name)
                .expect("checked test identity");
            let stdout = directory.join("stdout");
            let stderr = directory.join("stderr");
            let status = process::status(
                Command::new(directory.join("suite"))
                    .env_clear()
                    .args([index.to_string(), expected_status.to_string()])
                    .stdin(Stdio::null())
                    .stdout(fs::File::create(&stdout).unwrap())
                    .stderr(fs::File::create(&stderr).unwrap()),
                Duration::from_secs(20),
            )
            .unwrap();
            let output = Output {
                status,
                stdout: fs::read(stdout).unwrap(),
                stderr: fs::read(stderr).unwrap(),
            };
            assert!(
                output.status.success(),
                "{name}: {status}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        }
    }

    #[test]
    fn single_suite_keeps_sorted_cases_and_emits_all_six_targets() {
        let (_directory, sources, program) = sources(PURE_MODULE, PURE_TESTS);
        assert_eq!(
            program
                .tests()
                .iter()
                .map(|case| case.name())
                .collect::<Vec<_>>(),
            [
                "user/account/boolean",
                "user/account/business",
                "user/account/failure",
                "user/account/success"
            ]
        );
        assert!(program.tests().iter().all(|case| !case.uses_database()));
        let ir = dever_core::llvm::emit_test_suite(&program, &sources).unwrap();
        for target in Target::ALL {
            assert!(!emit_object(&ir, target).unwrap().is_empty());
        }
    }

    #[test]
    #[ignore = "requires explicit runtime archive; bounded native ownership checks"]
    fn pure_suite_reports_source_failures_without_loading_deployment_settings() {
        let suite = Suite::compile(PURE_MODULE, PURE_TESTS, 64);
        for (case, message) in [
            ("success", None),
            (
                "failure",
                Some("assert_eq failed: actual = 1, expected = 2"),
            ),
            ("boolean", Some("assertion failed")),
            ("business", Some("owned failure payload")),
        ] {
            // A malformed deployment setting must be completely irrelevant here.
            let directory = suite.stage(Some("not deployment JSON"));
            let output = suite.run(
                directory.path(),
                &format!("user/account/{case}"),
                i32::from(message.is_some()),
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            if let Some(message) = message {
                assert_eq!(stderr.matches(message).count(), 65, "{stderr}");
                assert!(
                    stderr.contains(&format!("test/user/account/{case}.dever:2:")),
                    "{stderr}"
                );
            } else {
                assert!(stderr.is_empty(), "{stderr}");
                assert!(output.stdout.is_empty());
            }
            assert!(!directory.path().join("data").exists());
        }
    }

    #[test]
    fn job_cases_preserve_checked_effects_without_reaching_real_libs() {
        let (_directory, sources, program) = sources(JOB_MODULE, JOB_TESTS);
        assert_eq!(
            program
                .tests()
                .iter()
                .filter(|case| case.uses_database())
                .count(),
            6
        );
        let ir = dever_core::llvm::emit_test_suite(&program, &sources).unwrap();
        for target in Target::ALL {
            assert!(!emit_object(&ir, target).unwrap().is_empty());
        }
    }

    fn rows(directory: &Path, sql: &'static str) -> Vec<Vec<dever_runtime::orm::Value>> {
        sqlite_inspect::query(&directory.join("data/db/case.db"), sql)
    }

    #[test]
    #[ignore = "requires explicit runtime archive; isolated SQLite and no production Worker"]
    fn jobs_use_case_fakes_clock_retry_cron_and_atomic_transactions() {
        use dever_runtime::orm::Value;
        let suite = Suite::compile(JOB_MODULE, JOB_TESTS, 0);
        for (case, database) in &suite.cases {
            let directory = suite.stage(database.then_some(SQLITE));
            let failed = case.ends_with("/abort");
            let output = suite.run(directory.path(), case, i32::from(failed));
            if !database {
                assert!(!directory.path().join("data").exists());
                continue;
            }
            if failed {
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("abort the whole transaction")
                );
                assert_eq!(
                    rows(directory.path(), "SELECT COUNT(*) FROM _dever_jobs"),
                    vec![vec![Value::Int(0)]]
                );
                assert_eq!(
                    rows(directory.path(), "SELECT COUNT(*) FROM article"),
                    vec![vec![Value::Int(0)]]
                );
            } else if case.ends_with("/retry") {
                assert_eq!(
                    rows(
                        directory.path(),
                        "SELECT state,attempt,error FROM _dever_jobs"
                    ),
                    vec![vec![
                        Value::Text("dead".into()),
                        Value::Int(2),
                        Value::Text("handler_failed".into())
                    ]]
                );
            } else {
                assert!(
                    rows(
                        directory.path(),
                        "SELECT id FROM _dever_jobs WHERE state!='succeeded'"
                    )
                    .is_empty()
                );
            }
        }
    }

    #[test]
    #[ignore = "requires explicit runtime archive; case-local Port effects and no external Worker"]
    fn suite_keeps_blocking_and_async_fakes_separate() {
        let module = [
            ("main.dever", "main() () {}"),
            (
                "notification/mail/app.dever",
                "read() (value: Int) { value = port.read() }",
            ),
            (
                "notification/mail/port.dever",
                "read() (value: Int) fails dever.time.SleepResult",
            ),
            (
                "notification/mail/adapter.dever",
                "external exec \"worker/must-not-start\" {}",
            ),
        ];
        let tests = [
            (
                "notification/mail/blocking.dever",
                "blocking() () { dever.time.sleep(1)\n  assert_eq(app.read(), 11) }\nport.read() (value: Int) { dever.time.sleep(1)\n  value = 11 }",
            ),
            (
                "notification/mail/suspending.dever",
                "suspending() () { assert_eq(app.read(), 23) }\nport.read() (value: Int) { dever.task.sleep(1)\n  value = 23 }",
            ),
        ];
        let suite = Suite::compile(&module, &tests, 64);
        for (case, database) in &suite.cases {
            assert!(!database);
            let directory = suite.stage(Some("must not load production config"));
            assert!(suite.run(directory.path(), case, 0).stderr.is_empty());
        }
    }

    #[test]
    #[ignore = "requires explicit runtime archive; inspect only this suite's persisted queue"]
    fn job_dispatch_blocks_incompatible_schema_payload_and_policy() {
        use dever_runtime::orm::Value;
        let tests = [
            (
                "content/article/prepare.dever",
                "prepare() () { id = app.submit(\"queued\") }\nport.title(value: Text) (title: Text) { title = value }",
            ),
            (
                "content/article/drain.dever",
                "drain() () { id = app.submit(\"queued\")\n  assert_eq(dever.test.drain_jobs(5), 1)\n  assert_eq(app.count(\"started\"), 0) }\nport.title(value: Text) (title: Text) { title = value }",
            ),
        ];
        let suite = Suite::compile(JOB_MODULE, &tests, 0);
        for (mutation, expected) in [
            (
                "UPDATE _dever_jobs SET schema='old-version'",
                "schema_mismatch",
            ),
            ("UPDATE _dever_jobs SET payload='{}'", "invalid_payload"),
            (
                "UPDATE _dever_jobs SET timeout_ms=3600001",
                "invalid_policy",
            ),
        ] {
            let directory = suite.stage(Some(SQLITE));
            suite.run(directory.path(), "content/article/prepare", 0);
            sqlite_inspect::execute(&directory.path().join("data/db/case.db"), mutation);
            suite.run(directory.path(), "content/article/drain", 0);
            assert_eq!(
                rows(
                    directory.path(),
                    "SELECT state,error,attempt FROM _dever_jobs"
                ),
                [vec![
                    Value::Text("blocked".into()),
                    Value::Text(expected.into()),
                    Value::Int(1)
                ]]
            );
        }
    }

    #[test]
    #[ignore = "requires explicit runtime archive; CMD enters System scope without running worker"]
    fn command_enqueues_system_jobs_and_releases_repeated_application_roots() {
        let mut module = JOB_MODULE.to_vec();
        module
            .iter_mut()
            .find(|(path, _)| *path == "content/article/adapter.dever")
            .unwrap()
            .1 = "port.title(value: Text) (title: Text) { title = value }";
        module.push(("content/article/api.dever", "cmd submit = app.submit"));
        let (_directory, sources, program) = sources(&module, &[]);
        let mut ir = dever_core::llvm::emit_application(&program, &sources).unwrap();
        ir.push_str("\ndefine i32 @dever_test_run() {\nentry:\n  %out = alloca i8\n  %fault = alloca %dever.fault\n  %status = call i32 @dever_application_entry(ptr %out, ptr %fault)\n  call void @dever_outputs_release(ptr %out)\n  call void @dever_fault_release(ptr %fault)\n  ret i32 %status\n}\n");
        let setting = r#"{"database":{"default":{"type":"sqlite","path":"data/db/case.db","max_connections":1}}}"#;
        let output = llvm_module::execute(
            &ir,
            "System CMD enqueue",
            64,
            Duration::from_secs(20),
            &["content.article.submit", r#"{"title":"queued"}"#],
            Some(setting),
        );
        let lines = String::from_utf8(output.stdout).unwrap();
        let replies = lines
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(replies.len(), 65);
        assert!(
            replies
                .iter()
                .all(|reply| reply["code"] == 0 && reply["data"] == replies[0]["data"])
        );
    }

    #[test]
    #[ignore = "requires explicit runtime archive; invalid worker policy must fail before schema I/O"]
    fn invalid_worker_policy_fails_before_creating_database() {
        let module = [
            ("content/article/app.dever", "clean() () {}"),
            (
                "content/article/job.dever",
                "database default\njob cleanup() () retry(2) timeout(1000) { app.clean() }",
            ),
        ];
        let (_source_dir, sources, program) = sources(&module, &[]);
        let mut ir = dever_core::llvm::emit_application(&program, &sources).unwrap();
        let check = llvm_module::fault_check(3, None);
        ir.push_str(&format!("\ndefine i32 @dever_test_run() {{\nentry:\n  %out = alloca i8\n  %fault = alloca %dever.fault\n  %status = call i32 @dever_application_entry(ptr %out, ptr %fault)\n{check}\n  call void @dever_outputs_release(ptr %out)\n  call void @dever_fault_release(ptr %fault)\n  %exit = select i1 %passed, i32 0, i32 1\n  ret i32 %exit\n}}\n"));
        let directory = temp::TemporaryDirectory::new();
        fs::create_dir(directory.path().join("config")).unwrap();
        fs::write(directory.path().join("config/setting.json"), r#"{"database":{"default":{"type":"sqlite","path":"data/db/case.db","max_connections":1}},"runtime":{"mode":"worker","shutdown_ms":1000},"job":{"workers":1,"poll_ms":10,"lease_ms":1000,"retry_base_ms":10,"retry_max_ms":100}}"#).unwrap();
        llvm_module::execute_in(
            &directory,
            &ir,
            "worker lease preflight",
            64,
            Duration::from_secs(10),
            &[],
        );
        assert!(!directory.path().join("data/db/case.db").exists());
    }

    #[test]
    #[ignore = "requires explicit runtime archive; bounded task cancellation inside owned SQLite transactions"]
    fn timed_out_job_drains_children_and_rolls_back_before_retry() {
        use dever_runtime::orm::Value;
        let module = [
            ("main.dever", "main() () {}"),
            (
                "content/article/model.dever",
                "type Article { title: Text }",
            ),
            (
                "content/article/app.dever",
                "submit() (id: Id) { id = dever.job.enqueue(job.publish, \"timeout\") }\napply() () { saved = model.create({ title = \"rollback\" })\n  port.pause() }\ncount() (total: Int) { total = model.count() }",
            ),
            (
                "content/article/port.dever",
                "pause() () fails dever.time.SleepResult",
            ),
            ("content/article/adapter.dever", "port.pause() () {}"),
            (
                "content/article/job.dever",
                "database default\njob publish() () retry(2) timeout(10) { app.apply() }",
            ),
        ];
        let tests = [(
            "content/article/timeout.dever",
            r#"port.pause() () { dever.task.sleep(5000) }
timeout() () {
  id = app.submit()
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count(), 0)
  dever.test.advance_clock(1000)
  assert_eq(dever.test.drain_jobs(5), 1)
  assert_eq(app.count(), 0)
  dever.test.advance_clock(10000)
  assert_eq(dever.test.drain_jobs(5), 0)
}"#,
        )];
        let suite = Suite::compile(&module, &tests, 0);
        let directory = suite.stage(Some(SQLITE));
        suite.run(directory.path(), "content/article/timeout", 0);
        assert_eq!(
            rows(
                directory.path(),
                "SELECT state,error,attempt FROM _dever_jobs"
            ),
            [vec![
                Value::Text("dead".into()),
                Value::Text("handler_timeout".into()),
                Value::Int(2)
            ]]
        );
    }
}
