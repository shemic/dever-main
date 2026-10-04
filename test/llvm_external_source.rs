//! Typed Worker calls and verified executable resources through the LLVM backend.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm_module.rs"]
mod llvm_module;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/sandbox.rs"]
mod sandbox;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::{llvm_module, temp};
    use dever_backend_bridge::{BinaryResource, Target, emit_object, emit_object_with_resources};
    use dever_core::{hir::Program, native::EmbeddedResource, source::SourceMap};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    const READ_APP: &str = r#"type Outcome {
  Done(value: Int)
  error Rejected(reason: Text)
}
read() (value: Int) { value = port.read() }
message(value: Outcome.Done(number)) (text: Text) recover("inspect Worker outcome") { text = "accepted" }
message(value: Outcome.Rejected(reason)) (text: Text) recover("inspect Worker outcome") { text = reason }
capture() (text: Text) { text = message(result(read())) }
"#;
    const READ_PORT: &str = "read() (value: Int) fails app.Outcome";
    const READ_API: &str = "cmd read = app.read\ncmd capture = app.capture";
    const SETTING: &str = "setting {\n mode: Text\n identity: Text?\n marker: Text?\n}\n";

    fn checked(ecosystem: &str, app: &str, port: &str, api: &str) -> (SourceMap, Program) {
        let mut sources = SourceMap::default();
        sources.add("notification/delivery/app.dever", app);
        sources.add("notification/delivery/port.dever", port);
        sources.add("notification/delivery/api.dever", api);
        sources.add(
            "notification/delivery/adapter.dever",
            format!("{SETTING}external {ecosystem} \"worker\" {{ allow network\nallow file }}"),
        );
        let program = dever_core::check(&sources).unwrap_or_else(|errors| {
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

    fn read(ecosystem: &str) -> (SourceMap, Program) {
        checked(ecosystem, READ_APP, READ_PORT, READ_API)
    }

    fn settings(program: &Program, mode: &str) -> String {
        let contracts = program.external_worker_contracts();
        let identity = contracts[0].errors.keys().next().unwrap();
        json!({"adapter":{"notification.delivery":{"setting":{"mode":mode,"identity":identity,"marker":"/data/lifecycle/worker"},"files":{"lifecycle":{"path":"lifecycle","write":true}}}}})
            .to_string()
    }

    fn driver(mut ir: String, fault: Option<(u32, Option<&str>)>) -> String {
        let check = match fault {
            Some((code, message)) => llvm_module::fault_check(code, message),
            None => "  %passed = icmp eq i32 %status, 0".into(),
        };
        ir.push_str(&format!("\ndefine i32 @dever_test_run() {{\nentry:\n  %out = alloca i8\n  %fault = alloca %dever.fault\n  %status = call i32 @dever_application_entry(ptr %out, ptr %fault)\n{check}\n  call void @dever_outputs_release(ptr %out)\n  call void @dever_fault_release(ptr %fault)\n  %exit = select i1 %passed, i32 0, i32 1\n  ret i32 %exit\n}}\n"));
        ir
    }

    fn fixture() -> Vec<u8> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        fs::read(root.join("target/native-release-inputs/component-fixture"))
            .expect("prepare the immutable component-fixture in target/native-release-inputs")
    }

    fn resource(path: impl Into<String>, bytes: Vec<u8>, executable: bool) -> EmbeddedResource {
        EmbeddedResource {
            path: path.into(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
            executable,
        }
    }

    fn resources(program: &Program) -> Vec<EmbeddedResource> {
        let contract = &program.external_worker_contracts()[0];
        let launch = json!({
            "format":"dever-worker-launch-v1", "ecosystem":contract.ecosystem,
            "entry":contract.entry, "executable":"workers/fixture/worker",
            "arguments":[], "working_directory":"workers/fixture"
        });
        let mut resources =
            super::sandbox::assets(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
                .into_iter()
                .map(|asset| resource(asset.path, asset.bytes, asset.executable))
                .collect::<Vec<_>>();
        resources.extend([
            resource(
                format!("{}.dever-worker.json", contract.entry),
                serde_json::to_vec(&launch).unwrap(),
                false,
            ),
            resource("workers/fixture/worker", fixture(), true),
        ]);
        resources
    }

    fn stage(directory: &temp::TemporaryDirectory, setting: &str) {
        fs::create_dir_all(directory.path().join("config")).unwrap();
        fs::create_dir_all(directory.path().join("lifecycle")).unwrap();
        fs::write(directory.path().join("config/setting.json"), setting).unwrap();
    }

    fn run(
        source: &(SourceMap, Program),
        mode: &str,
        command: &str,
        body: &str,
        fault: Option<(u32, Option<&str>)>,
    ) -> std::process::Output {
        let (_, program) = source;
        let directory = temp::TemporaryDirectory::new();
        stage(&directory, &settings(program, mode));
        let resources = resources(program);
        let output = execute(
            source,
            &directory,
            &resources,
            &[command, body],
            fault,
            Duration::from_secs(60),
        );
        if fault.is_none() {
            assert!(
                directory.path().join("lifecycle/worker.shutdown").is_file(),
                "Worker must complete orderly shutdown"
            );
        }
        output
    }

    fn execute(
        source: &(SourceMap, Program),
        directory: &temp::TemporaryDirectory,
        resources: &[EmbeddedResource],
        arguments: &[&str],
        fault: Option<(u32, Option<&str>)>,
        timeout: Duration,
    ) -> std::process::Output {
        let (sources, program) = source;
        let module =
            dever_core::llvm::emit_application_with_resources(program, sources, resources).unwrap();
        let ir = driver(module.ir, fault);
        let attachments = module
            .resources
            .iter()
            .map(|resource| BinaryResource {
                symbol: &resource.symbol,
                bytes: resource.bytes,
            })
            .collect::<Vec<_>>();
        let executable = llvm_module::link_with_resources(
            directory,
            &ir,
            "external Worker",
            64,
            "managed-driver.c",
            &attachments,
        );
        llvm_module::execute_program(&executable, "external Worker", timeout, arguments)
    }

    fn command_source() -> (SourceMap, Program) {
        let mut sources = SourceMap::default();
        sources.add(
            "tools/command/adapter.dever",
            "external command \"bin/tool\" {}",
        );
        sources.add("tools/command/port.dever", "execute(args: List<Text>, stdin: Bytes?) (output: dever.process.Output) fails dever.process.Error");
        sources.add("tools/command/api.dever", "cmd read = app.read");
        sources.add(
            "tools/command/app.dever",
            r#"type Summary {
  code: Int
  binary: Bool
  stderr: Bool
  eof: Bool
}
read() (summary: Summary) {
  input = dever.bytes.from_ints([0, 255, 128, 10])
  echoed = port.execute(["stdin"], input)
  empty = port.execute(["stdin"], null)
  failed = port.execute(["exit", "7"], null)
  summary = Summary {
    code = failed.code
    binary = echoed.stdout == input
    stderr = echoed.stderr == input
    eof = dever.bytes.length(empty.stdout) == 0
  }
}
"#,
        );
        let program = dever_core::check(&sources).unwrap_or_else(|errors| {
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

    #[test]
    fn command_apps_emit_typed_bytes_objects_for_all_targets() {
        let (sources, program) = command_source();
        let ir = dever_core::llvm::emit_application(&program, &sources).unwrap();
        for target in Target::ALL {
            assert!(!emit_object(&ir, target).unwrap().is_empty());
        }
    }

    #[test]
    #[ignore = "requires fresh runtime-external archive, command-fixture and sandbox assets"]
    fn command_binary_io_and_exit_status_use_real_typed_llvm_calls() {
        let source = command_source();
        let contract = &source.1.external_command_contracts()[0];
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut resources = super::sandbox::assets(&workspace)
            .into_iter()
            .map(|asset| resource(asset.path, asset.bytes, asset.executable))
            .collect::<Vec<_>>();
        resources.push(resource(
            "commands/fixture/tool",
            fs::read(workspace.join("target/native-release-inputs/command-fixture"))
                .expect("prepare immutable command-fixture in target/native-release-inputs"),
            true,
        ));
        resources.push(resource(format!("{}.dever-command.json", contract.entry), serde_json::to_vec(&json!({
            "format":"dever-command-launch-v1", "ecosystem":"command", "entry":contract.entry,
            "executable":"commands/fixture/tool", "arguments":[], "working_directory":"commands/fixture"
        })).unwrap(), false));
        let directory = temp::TemporaryDirectory::new();
        stage(&directory, "{}");
        let output = execute(
            &source,
            &directory,
            &resources,
            &["tools.command.read", "{}"],
            None,
            // Each of 65 roots starts three isolated processes and verifies
            // their resources, unlike the persistent single-call Worker probe.
            Duration::from_secs(180),
        );
        result(
            &output,
            json!({"code":7, "binary":true, "stderr":true, "eof":true}),
        );
    }

    fn result(output: &std::process::Output, expected: serde_json::Value) {
        let rows: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![json!({"code":0,"message":"ok","data":expected}); 65]
        );
    }

    #[test]
    fn external_apps_emit_objects_for_all_targets_and_kernels_stay_closed() {
        let (sources, program) = read("exec");
        assert!(
            dever_core::llvm::emit_kernel(&program, &sources, "notification.delivery.read")
                .is_err()
        );
        let ir = dever_core::llvm::emit_application(&program, &sources).unwrap();
        for target in Target::ALL {
            assert!(!emit_object(&ir, target).unwrap().is_empty());
        }
    }

    #[test]
    fn large_embedded_resources_keep_ir_small_and_share_payloads() {
        let (sources, program) = read("exec");
        let bytes = vec![0xA5; 9 * 1024 * 1024];
        let resources = [
            resource("shared/large", bytes.clone(), false),
            resource("shared/alias", bytes, true),
        ];
        let module =
            dever_core::llvm::emit_application_with_resources(&program, &sources, &resources)
                .unwrap();
        assert!(module.ir.len() < 1024 * 1024);
        assert_eq!(module.resources.len(), 1);
        assert_eq!(module.resources[0].bytes, resources[0].bytes);
        assert_eq!(
            module
                .ir
                .matches("= external constant [9437184 x i8]")
                .count(),
            1
        );
        assert_eq!(
            module
                .ir
                .matches("ptr @dever_external_bytes_0, i64 9437184")
                .count(),
            2
        );
    }

    #[test]
    fn embedded_metadata_rejects_bad_hashes_before_linking() {
        let (sources, program) = read("exec");
        let mut entry = resource(
            "module/notification/delivery/worker",
            b"fixture".to_vec(),
            true,
        );
        let resources = [
            entry.clone(),
            resource("shared/same-bytes", entry.bytes.clone(), false),
            resource("shared/empty", Vec::new(), false),
        ];
        let valid =
            dever_core::llvm::emit_application_with_resources(&program, &sources, &resources)
                .unwrap();
        assert_eq!(
            valid.resources.len(),
            2,
            "identical content has one initializer"
        );
        let attachments = valid
            .resources
            .iter()
            .map(|resource| BinaryResource {
                symbol: &resource.symbol,
                bytes: resource.bytes,
            })
            .collect::<Vec<_>>();
        for target in Target::ALL {
            assert!(
                !emit_object_with_resources(&valid.ir, target, &attachments)
                    .unwrap()
                    .is_empty()
            );
        }
        entry.sha256 = "0".repeat(64);
        assert!(
            dever_core::llvm::emit_application_with_resources(&program, &sources, &[entry])
                .is_err()
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-external archive and existing fixture Worker"]
    fn worker_success_and_declared_failure_survive_repeated_roots() {
        let source = read("exec");
        result(
            &run(&source, "normal", "notification.delivery.read", "{}", None),
            json!(7),
        );
        result(
            &run(
                &source,
                "declared_error",
                "notification.delivery.capture",
                "{}",
                None,
            ),
            json!("fixture"),
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-external archive and existing fixture Worker"]
    fn typed_nested_values_and_multiple_outputs_round_trip() {
        let source = checked(
            "exec",
            r#"type Outcome { error Rejected(reason: Text) }
type Payload { name: Text
  tags: List<Text?>
  count: Int
  note: Text? }
echo(value: Payload) (answer: Payload) { answer = port.echo(value) }
"#,
            "echo(value: app.Payload) (value: app.Payload) fails app.Outcome",
            "cmd echo = app.echo",
        );
        let value = json!({"name":"你好 🌍","tags":["first",null,"last"],"count":9223372036854775807_i64,"note":null});
        result(
            &run(
                &source,
                "normal",
                "notification.delivery.echo",
                &json!({"value":value}).to_string(),
                None,
            ),
            value,
        );
        let source = checked(
            "exec",
            "type Outcome { error Rejected(reason: Text) }\ntype Pair { count: Int\n label: Text }\necho(input_count: Int, input_label: Text) (answer: Pair) { reply = port.echo(input_count, input_label)\n answer = Pair { count = reply.count\n label = reply.label } }",
            "echo(count: Int, label: Text) (count: Int, label: Text) fails app.Outcome",
            "cmd echo = app.echo",
        );
        result(
            &run(
                &source,
                "normal",
                "notification.delivery.echo",
                r#"{"input_count":42,"input_label":"owned"}"#,
                None,
            ),
            json!({"count":42,"label":"owned"}),
        );
        let source = checked(
            "exec",
            "type Outcome { error Rejected(reason: Text) }\necho() (done: Bool) { port.echo()\n done = true }",
            "echo() () fails app.Outcome",
            "cmd echo = app.echo",
        );
        result(
            &run(&source, "normal", "notification.delivery.echo", "{}", None),
            json!(true),
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-external archive and existing fixture Worker"]
    fn malformed_output_and_business_error_payloads_are_runtime_faults() {
        let source = read("exec");
        for mode in [
            "missing_output",
            "extra_output",
            "wrong_output",
            "invalid_error",
            "missing_error",
        ] {
            let output = run(
                &source,
                mode,
                "notification.delivery.capture",
                "{}",
                Some((3, None)),
            );
            assert!(
                output.stdout.is_empty(),
                "{mode} must not become a recoverable business error"
            );
        }
        let source = checked(
            "exec",
            "type Outcome { error Rejected(reason: Text) }\nfail() (value: Int) { value = port.fail() }",
            "fail() (value: Int) fails app.Outcome",
            "cmd fail = app.fail",
        );
        let output = run(
            &source,
            "normal",
            "notification.delivery.fail",
            "{}",
            Some((
                3,
                Some("external Adapter returned an undeclared business error"),
            )),
        );
        assert!(output.stdout.is_empty());
        let source = checked(
            "exec",
            "type Outcome { error Rejected(reason: Text) }\nread() (value: Int?) { value = port.read() }",
            "read() (value: Int?) fails app.Outcome",
            "cmd read = app.read",
        );
        assert!(
            run(
                &source,
                "missing_output",
                "notification.delivery.read",
                "{}",
                Some((3, None))
            )
            .stdout
            .is_empty()
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-external archive and existing fixture Worker"]
    fn handshake_and_setting_failures_do_not_leak_a_worker() {
        let source = read("exec");
        for mode in ["duplicate_ready", "capability_mismatch"] {
            assert!(
                run(
                    &source,
                    mode,
                    "notification.delivery.read",
                    "{}",
                    Some((3, None))
                )
                .stdout
                .is_empty()
            );
        }
        let (sources, program) = source;
        let ir = driver(
            dever_core::llvm::emit_application(&program, &sources).unwrap(),
            Some((3, Some("expected wire string"))),
        );
        // No worker exists: typed Setting rejection must happen before process launch.
        assert!(
            llvm_module::execute(
                &ir,
                "invalid external Setting",
                64,
                Duration::from_secs(15),
                &["notification.delivery.read", "{}"],
                Some(r#"{"adapter":{"notification.delivery":{"setting":{"mode":7}}}}"#)
            )
            .stdout
            .is_empty()
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-external archive and existing fixture Worker"]
    fn cancelling_a_call_preserves_the_shared_worker() {
        let source = checked(
            "exec",
            r#"type Outcome { error Rejected(reason: Text) }
pending() () { port.wait() }
read() (value: Int) {
  task = run(pending())
  dever.task.sleep(5)
  stop(task)
  value = port.read()
}
"#,
            "wait() () fails app.Outcome\nread() (value: Int) fails app.Outcome",
            "cmd read = app.read",
        );
        result(
            &run(&source, "normal", "notification.delivery.read", "{}", None),
            json!(7),
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-external archive and existing fixture Worker"]
    fn independent_executables_use_verified_embedded_exec_and_managed_launches() {
        for ecosystem in ["exec", "pip", "npm", "go"] {
            let (sources, program) = read(ecosystem);
            let resources = resources(&program);
            let module =
                dever_core::llvm::emit_application_with_resources(&program, &sources, &resources)
                    .unwrap();
            let ir = driver(module.ir, None);
            let attachments = module
                .resources
                .iter()
                .map(|resource| BinaryResource {
                    symbol: &resource.symbol,
                    bytes: resource.bytes,
                })
                .collect::<Vec<_>>();
            let build = temp::TemporaryDirectory::new();
            let executable = llvm_module::link_with_resources(
                &build,
                &ir,
                ecosystem,
                64,
                "managed-driver.c",
                &attachments,
            );
            let directory = temp::TemporaryDirectory::new();
            let relocated = directory.path().join("program");
            fs::copy(executable, &relocated).unwrap();
            drop(build);
            stage(&directory, &settings(&program, "embedded"));
            result(
                &llvm_module::execute_program(
                    &relocated,
                    ecosystem,
                    Duration::from_secs(120),
                    &["notification.delivery.read", "{}"],
                ),
                json!(7),
            );
            assert!(
                !directory.path().join("module").exists(),
                "only embedded Worker resources may be used"
            );
        }
    }
}
