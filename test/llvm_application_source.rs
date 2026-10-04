//! Generated application entries, not ordinary kernels, through the real archive.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm_module.rs"]
mod llvm_module;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::llvm_module;
    use dever_backend_bridge::{Target, emit_object};
    use dever_core::{check, hir::Program, source::SourceMap};
    use std::time::Duration;

    const ECHO: &[(&str, &str)] = &[
        (
            "sample/echo/app.dever",
            r#"type Payload {
  name: Text
  count: Int
  tags: List<Text?>
  note: Text?
}
echo(payload: Payload) (value: Payload) { value = payload }
"#,
        ),
        ("sample/echo/api.dever", "cmd echo = app.echo"),
    ];

    const SCALARS: &[(&str, &str)] = &[
        (
            "sample/scalar/app.dever",
            r#"type Scalars {
  enabled: Bool
  number: Float
  money: Decimal
  identity: Id
  uuid: Uuid
  day: Date
  time: Time
  instant: DateTime
  duration: Duration
  raw: Json
}
echo(payload: Scalars) (value: Scalars) { value = payload }
"#,
        ),
        ("sample/scalar/api.dever", "cmd echo = app.echo"),
    ];

    const ASYNC: &[(&str, &str)] = &[
        (
            "sample/echo/app.dever",
            r#"type Payload {
  name: Text
  tags: List<Text?>
}
echo(payload: Payload) (value: Payload) {
  dever.task.sleep(1)
  value = payload
}
"#,
        ),
        ("sample/echo/api.dever", "cmd echo = app.echo"),
    ];

    const PORT: &[(&str, &str)] = &[
        (
            "notification/mail/app.dever",
            r#"read() (value: Int) { value = port.read() }
"#,
        ),
        (
            "notification/mail/port.dever",
            "read() (value: Int) fails dever.time.SleepResult",
        ),
        (
            "notification/mail/adapter/fixed.dever",
            "port.read() (value: Int) { value = 7 }",
        ),
        (
            "notification/mail/adapter/configured.dever",
            r#"setting {
  token: Secret
  value: Int
}
port.read() (value: Int) {
  dever.task.sleep(1)
  value = setting.value
}
"#,
        ),
        ("notification/mail/api.dever", "cmd read = app.read"),
    ];

    const FAILURES: &[(&str, &str)] = &[
        (
            "sample/action/app.dever",
            r#"type Outcome {
  Done(value: Text)
  error Denied(message: Text)
}
load(allowed: true) (value: Text) { value = "accepted" }
load(allowed: false) (value: Text) { fail(Outcome.Denied("owned denial")) }
read(value: Outcome.Done(text)) (answer: Text) recover("return the CMD failure message") { answer = text }
read(value: Outcome.Denied(message)) (answer: Text) recover("return the CMD failure message") { answer = message }
capture(allowed: Bool) (answer: Text) { answer = read(result(load(allowed))) }
divide(divisor: Int) (answer: Int) { answer = 42 // divisor }
"#,
        ),
        (
            "sample/action/api.dever",
            "cmd load = app.load\ncmd capture = app.capture\ncmd divide = app.divide",
        ),
    ];

    fn checked(files: &[(&str, &str)]) -> (SourceMap, Program) {
        let mut sources = SourceMap::default();
        for (path, text) in files {
            sources.add(*path, *text);
        }
        let program = check(&sources).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&sources))
                    .collect::<String>()
            )
        });
        (sources, program)
    }

    fn lower(files: &[(&str, &str)]) -> String {
        let (sources, program) = checked(files);
        dever_core::llvm::emit_application(&program, &sources).unwrap()
    }

    fn execute(
        files: &[(&str, &str)],
        arguments: &[&str],
        setting: Option<&str>,
        fault_code: Option<u32>,
        fault_message: Option<&str>,
    ) -> std::process::Output {
        let mut ir = lower(files);
        let check = match fault_code {
            None => "  %passed = icmp eq i32 %status, 0".to_owned(),
            Some(code) => llvm_module::fault_check(code, fault_message),
        };
        ir.push_str(&format!(
            "\ndefine i32 @dever_test_run() {{\nentry:\n  %out = alloca i8\n  %fault = alloca %dever.fault\n  %status = call i32 @dever_application_entry(ptr %out, ptr %fault)\n{check}\n  call void @dever_outputs_release(ptr %out)\n  call void @dever_fault_release(ptr %fault)\n  %exit = select i1 %passed, i32 0, i32 1\n  ret i32 %exit\n}}\n"
        ));
        llvm_module::execute(
            &ir,
            "checked CMD application",
            64,
            Duration::from_secs(10),
            arguments,
            setting,
        )
    }

    fn success(files: &[(&str, &str)], name: &str, body: &str, setting: Option<&str>, data: &str) {
        let output = execute(files, &[name, body], setting, None, None);
        let expected = format!("{{\"code\":0,\"message\":\"ok\",\"data\":{data}}}\n");
        // One warmup plus 64 calls: every entry must reinitialize and release owners.
        assert_eq!(output.stdout, expected.repeat(65).as_bytes());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn application_fixtures_pass_the_existing_checker() {
        for files in [ECHO, SCALARS, ASYNC, PORT, FAILURES] {
            checked(files);
        }
    }

    #[test]
    fn command_applications_emit_objects_for_all_six_targets() {
        let ir = lower(ECHO);
        for target in Target::ALL {
            assert!(!emit_object(&ir, target).unwrap().is_empty());
        }
    }

    #[test]
    fn ordinary_kernel_is_not_an_application_entry() {
        let (sources, program) = checked(ECHO);
        assert!(dever_core::llvm::emit_kernel(&program, &sources, "sample.echo.echo").is_err());
    }

    #[test]
    fn unsupported_application_metadata_is_rejected_at_its_source() {
        let files = [
            (
                "sample/echo/app.dever",
                "echo() (value: Text) { value = \"okay\" }",
            ),
            (
                "sample/echo/api.dever",
                "public get echo = app.echo\ncmd echo = app.echo",
            ),
        ];
        let (sources, program) = checked(&files);
        let error = dever_core::llvm::emit_application(&program, &sources).unwrap_err();
        assert!(error.contains("sample/echo/api.dever"), "{error}");
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_owned_record_list_unicode_and_missing_nullable() {
        success(
            ECHO,
            "sample.echo.echo",
            r#"{"payload":{"name":"你好 🌍","count":9223372036854775807,"tags":["first",null,"last"]}}"#,
            None,
            r#"{"name":"你好 🌍","count":9223372036854775807,"tags":["first",null,"last"],"note":null}"#,
        );
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_scalar_codecs_preserve_existing_wire_formats() {
        let body = r#"{"payload":{"enabled":true,"number":1.5,"money":"123.45","identity":"external-id","uuid":"01890f3e-7c00-7000-8000-000000000001","day":"2026-10-01","time":"12:34:56.789","instant":"2026-10-01T12:34:56.789Z","duration":-42,"raw":{"large":9999999999999999999999999999,"nested":[true,null,"value"]}}}"#;
        success(
            SCALARS,
            "sample.scalar.echo",
            body,
            None,
            &body[11..body.len() - 1],
        );
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_owned_inputs_survive_the_initial_suspend() {
        success(
            ASYNC,
            "sample.echo.echo",
            r#"{"payload":{"name":"pending-owned","tags":["one",null,"three"]}}"#,
            None,
            r#"{"name":"pending-owned","tags":["one",null,"three"]}"#,
        );
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_input_errors_release_partially_decoded_owners() {
        for body in [
            r#"{"payload":{"name":"owned","count":"bad","tags":["first"],"note":null}}"#,
            r#"{"payload":{"name":"owned","count":1,"tags":["first",false]}}"#,
            r#"{"payload":{"name":"owned","count":1,"tags":[]},"unknown":1}"#,
            r#"{"payload":{"name":"owned","name":"duplicate","count":1,"tags":[]}}"#,
            r#"{"payload":{"name":"owned","tags":[]}}"#,
            r#"{"payload":{"name":"owned","count":9223372036854775808,"tags":[]}}"#,
            r#"[]"#,
        ] {
            let output = execute(ECHO, &["sample.echo.echo", body], None, Some(3), None);
            assert!(output.stdout.is_empty());
        }
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_argument_errors_do_not_run_app() {
        for arguments in [
            &[][..],
            &["sample.echo.echo"][..],
            &["unknown", "{}"][..],
            &["sample.echo.echo", "{}", "extra"][..],
        ] {
            let output = execute(ECHO, arguments, None, Some(3), None);
            assert!(output.stdout.is_empty());
        }
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn port_selects_only_the_configured_adapter_and_releases_settings() {
        for (setting, value) in [
            (r#"{"adapter":{"notification.mail":{"use":"fixed"}}}"#, "7"),
            (
                r#"{"adapter":{"notification.mail":{"use":"configured","setting":{"value":19,"token":"private-token"}}}}"#,
                "19",
            ),
        ] {
            success(PORT, "notification.mail.read", "{}", Some(setting), value);
        }
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn selected_adapter_invalid_setting_fails_closed() {
        for setting in [
            r#"{"adapter":{"notification.mail":{"use":"unknown"}}}"#,
            r#"{"adapter":{"notification.mail":{"use":"configured"}}}"#,
            r#"{"adapter":{"notification.mail":{"use":"configured","setting":{"value":"private-token","token":"private-token"}}}}"#,
            r#"{"adapter":{"notification.mail":{"use":"fixed","use":"configured"}}}"#,
            r#"{"adapter":{"unknown.port":{"use":"fixed"}}}"#,
        ] {
            let output = execute(
                PORT,
                &["notification.mail.read", "{}"],
                Some(setting),
                Some(3),
                None,
            );
            assert!(output.stdout.is_empty());
        }
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_keeps_typed_business_faults_and_explicit_recovery() {
        let output = execute(
            FAILURES,
            &["sample.action.load", r#"{"allowed":false}"#],
            None,
            Some(4),
            None,
        );
        assert!(output.stdout.is_empty());
        success(
            FAILURES,
            "sample.action.capture",
            r#"{"allowed":false}"#,
            None,
            r#""owned denial""#,
        );
        success(
            FAILURES,
            "sample.action.capture",
            r#"{"allowed":true}"#,
            None,
            r#""accepted""#,
        );
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_numeric_fault_is_not_reclassified_as_input_or_business_error() {
        let output = execute(
            FAILURES,
            &["sample.action.divide", r#"{"divisor":0}"#],
            None,
            Some(2),
            None,
        );
        assert!(output.stdout.is_empty());
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_preserves_native_input_diagnostics_and_validation_order() {
        for (body, message) in [
            (r#"{}"#, "input 'payload' is required"),
            (r#"[]"#, "invalid JSON object body: expected wire record"),
            (
                r#"{"payload":{"name":"owned","count":1,"tags":[]},"unknown":false}"#,
                "unknown input 'unknown'",
            ),
            (
                r#"{"payload":{"name":"owned","count":"bad","tags":[]},"unknown":false}"#,
                "expected wire integer",
            ),
        ] {
            let output = execute(
                ECHO,
                &["sample.echo.echo", body],
                None,
                Some(3),
                Some(message),
            );
            assert!(output.stdout.is_empty());
        }
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn selected_setting_diagnostic_is_redacted_after_partial_secret_decode() {
        let setting = r#"{"adapter":{"notification.mail":{"use":"configured","setting":{"value":"private-token","token":"private-token"}}}}"#;
        let output = execute(
            PORT,
            &["notification.mail.read", "{}"],
            Some(setting),
            Some(3),
            Some("expected wire integer"),
        );
        assert!(output.stdout.is_empty());
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn cmd_top_level_nullable_missing_null_and_owned_present() {
        let files = [
            (
                "sample/nullable/app.dever",
                "echo(name: Text?) (value: Text?) { value = name }",
            ),
            ("sample/nullable/api.dever", "cmd echo = app.echo"),
        ];
        for (body, data) in [
            ("{}", "null"),
            (r#"{"name":null}"#, "null"),
            (r#"{"name":"owned nullable"}"#, r#""owned nullable""#),
        ] {
            success(&files, "sample.nullable.echo", body, None, data);
        }
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn single_unconfigured_adapter_needs_no_setting_file() {
        let files = [
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
                "port.read() (value: Int) { value = 7 }",
            ),
            ("notification/mail/api.dever", "cmd read = app.read"),
        ];
        success(&files, "notification.mail.read", "{}", None, "7");
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn task_reads_selected_setting_within_the_application_scope() {
        let mut files = PORT.to_vec();
        files[0].1 = "read() (value: Int) { pending = run(port.read())\n value = wait(pending) }";
        let setting = r#"{"adapter":{"notification.mail":{"use":"configured","setting":{"value":29,"token":"private-task-token"}}}}"#;
        success(&files, "notification.mail.read", "{}", Some(setting), "29");
    }

    #[test]
    #[ignore = "requires explicitly prepared runtime-only archive; no database or service"]
    fn later_adapter_initialization_failure_releases_earlier_settings_and_gate() {
        let mut files = PORT.to_vec();
        files.extend([
            ("notification/sms/app.dever", "read() (value: Int) { value = port.read() }"),
            ("notification/sms/port.dever", "read() (value: Int) fails dever.time.SleepResult"),
            ("notification/sms/adapter.dever", "setting { value: Int }\nport.read() (value: Int) { value = setting.value }"),
            ("notification/send/app.dever", "read() (value: Int) { value = notification.mail.read() + notification.sms.read() }"),
            ("notification/send/api.dever", "cmd read = app.read"),
        ]);
        let setting = r#"{"adapter":{"notification.mail":{"use":"configured","setting":{"value":19,"token":"private-earlier-token"}}}}"#;
        let output = execute(
            &files,
            &["notification.send.read", "{}"],
            Some(setting),
            Some(3),
            Some("selected Adapter requires setting"),
        );
        assert!(output.stdout.is_empty());
    }
}
