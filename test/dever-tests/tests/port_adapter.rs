#[path = "support/temp.rs"]
mod temp;

use dever_core::source::SourceMap;
use std::{fs, process::Command};
use temp::TemporaryDirectory;

const APP: &str = "type ReadResult {\n Read(value: Int)\n error Unavailable(message: Text)\n}\nread() (value: Int) { value = port.read() }\n";
const PORT: &str = "read() (value: Int) fails app.ReadResult\n";
const ADAPTER: &str = "port.read() (value: Int) { value = 7 }";
const COMMAND_PORT: &str = "execute(args: List<Text>, stdin: Bytes?) (output: dever.process.Output) fails dever.process.Error";
const COMMAND_APP: &str = "read() (value: Int) { output = port.execute([\"argument with spaces\"], null)\n value = output.code }";

fn command_sources(port: &str, adapter: &str, app: &str) -> SourceMap {
    sources(port, adapter, &[("notification/mail/app.dever", app)])
}

#[test]
fn command_contract_selects_only_its_bytes_policy_and_both_typed_backends() {
    use dever_core::wire::Policy;

    let mut source = command_sources(
        COMMAND_PORT,
        "external command \"bin/tool\" {}",
        COMMAND_APP,
    );
    source.add("notification/mail/api.dever", "cmd read = app.read");
    let program = checked(&source);
    assert!(program.has_external_adapters());
    assert!(program.external_worker_contracts().is_empty());
    assert!(program.external_worker_entries().is_empty());
    assert!(program.external_lib_requests().is_empty());
    let contracts = program.external_command_contracts();
    assert_eq!(contracts.len(), 1);
    let contract = &contracts[0];
    assert_eq!(contract.ecosystem, "command");
    assert_eq!(contract.operations, ["execute"]);
    assert_eq!(contract.entry, "module/notification/mail/bin/tool");
    assert_eq!(
        contract.inputs["execute"]["fields"]["stdin"]["value"],
        serde_json::json!({"type":"bytes","encoding":"base64"})
    );
    assert_eq!(
        contract.outputs["execute"]["fields"]["output"]["name"],
        "dever.process.Output"
    );
    assert_eq!(
        contract.errors.keys().collect::<Vec<_>>(),
        [&"dever.process.Error.Failed".to_owned()]
    );
    assert!(
        program
            .wire_schema("dever.process.Output", Policy::Command)
            .is_ok()
    );
    for policy in [
        Policy::Output,
        Policy::ApiInput,
        Policy::SettingInput,
        Policy::Job,
    ] {
        assert!(
            program
                .wire_schema("dever.process.Output", policy)
                .unwrap_err()
                .contains("Bytes")
        );
    }
    let native = dever_core::native::emit(&program, &source, "notification.mail.read").unwrap();
    assert!(native.contains("node.bytes()"));
    assert!(native.contains("writer.bytes(value)"));
    let llvm = dever_core::llvm::emit_application(&program, &source).unwrap();
    assert!(llvm.contains("call i32 @dever_rt_v1_wire_node_bytes("));
    assert!(llvm.contains("call i32 @dever_rt_v1_wire_encoder_bytes("));

    checked(&command_sources(
        &COMMAND_PORT
            .replace(", stdin: Bytes?", "")
            .replace("execute", "convert"),
        "external command \"bin/tool\" { allow file }",
        "read() (value: Int) { output = port.convert([])\n value = output.code }",
    ));
}

#[test]
fn command_contract_rejects_signature_drift_worker_options_and_paths() {
    for (port, message) in [
        (
            COMMAND_PORT.replace("args:", "arguments:"),
            "inputs must be",
        ),
        (COMMAND_PORT.replace("List<Text>", "Text"), "inputs must be"),
        (COMMAND_PORT.replace("stdin:", "input:"), "inputs must be"),
        (COMMAND_PORT.replace("Bytes?", "Bytes"), "inputs must be"),
        (
            COMMAND_PORT.replace("Bytes?", "Bytes?, extra: Int"),
            "inputs must be",
        ),
        (
            COMMAND_PORT.replace("output:", "result:"),
            "must return output",
        ),
        (
            COMMAND_PORT.replace("dever.process.Output", "Int"),
            "must return output",
        ),
        (
            COMMAND_PORT.replace("dever.process.Error", "dever.process.ArgumentsResult"),
            "must declare fails",
        ),
        (
            format!(
                "{COMMAND_PORT}\n{}",
                COMMAND_PORT.replace("execute", "convert")
            ),
            "exactly one Port operation",
        ),
    ] {
        rejected(
            &command_sources(
                &port,
                "external command \"bin/tool\" {}",
                "read() (value: Int) { value = 0 }",
            ),
            message,
        );
    }
    for (adapter, message) in [
        (
            "setting { value: Int }\nexternal command \"bin/tool\" {}",
            "does not accept setting or lib",
        ),
        (
            "external command \"bin/tool\" { lib \"pip:example@1.0\" }",
            "does not accept setting or lib",
        ),
        ("external command \"../tool\" {}", "portable relative path"),
        ("external command \"/tool\" {}", "portable relative path"),
        (
            "external command \"bin/\\u{0}tool\" {}",
            "portable relative path",
        ),
        (
            "external command \"bin/tool\" { allow unknown }",
            "unknown external capability",
        ),
    ] {
        rejected(
            &command_sources(COMMAND_PORT, adapter, COMMAND_APP),
            message,
        );
    }
    rejected(
        &command_sources(COMMAND_PORT, "external exec \"bin/tool\" {}", COMMAND_APP),
        "unsupported wire type Bytes",
    );
    let mut api = command_sources(
        COMMAND_PORT,
        "external command \"bin/tool\" {}",
        "read() (output: dever.process.Output) { output = port.execute([], null) }",
    );
    api.add("notification/mail/api.dever", "cmd read = app.read");
    rejected(&api, "Bytes");
}

#[test]
fn command_adapter_formats_plain_and_markdown_sources_idempotently() {
    let adapter = "external command \"bin/tool\"{allow file\nallow process}";
    let source = command_sources(COMMAND_PORT, adapter, COMMAND_APP);
    checked(&source);
    let plain = source
        .files()
        .iter()
        .find(|file| file.path().ends_with("adapter.dever"))
        .unwrap();
    let formatted = dever_core::format::format(plain).unwrap();
    assert_eq!(
        formatted,
        "external command \"bin/tool\" {\n  allow file\n  allow process\n}\n"
    );
    checked(&command_sources(COMMAND_PORT, &formatted, COMMAND_APP));

    let markdown = format!(
        "# 命令适配器\n\n执行普通二进制。\n\n- 包：`notification.mail.adapter`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n## 外部程序\n\n传递参数和标准输入。\n\n- 声明：`external command bin/tool`\n\n```dever\n{adapter}\n```\n"
    );
    let mut source = SourceMap::default();
    source.add("notification/mail/app.dever", COMMAND_APP);
    source.add("notification/mail/port.dever", COMMAND_PORT);
    let id = source.add("notification/mail/adapter.dever.md", &markdown);
    checked(&source);
    let formatted = dever_core::format::format(source.get(id)).unwrap();
    assert!(formatted.contains("传递参数和标准输入。"));
    let mut round_trip = SourceMap::default();
    let id = round_trip.add("notification/mail/adapter.dever.md", formatted.clone());
    assert_eq!(
        dever_core::format::format(round_trip.get(id)).unwrap(),
        formatted
    );
}

#[test]
fn command_effects_and_test_fakes_reuse_the_port_boundary() {
    rejected(
        &command_sources(
            COMMAND_PORT,
            "external command \"bin/tool\" {}",
            &COMMAND_APP.replace("(value: Int)", "(value: Int) pure"),
        ),
        "pure",
    );
    let root = TemporaryDirectory::new();
    for (path, text) in [
        ("module/notification/mail/app.dever", COMMAND_APP),
        ("module/notification/mail/port.dever", COMMAND_PORT),
        (
            "module/notification/mail/adapter.dever",
            "external command \"bin/missing-program\" { allow process }",
        ),
        (
            "test/notification/mail/first.dever",
            "first() () { assert_eq(app.read(), 23) }\nport.execute(args: List<Text>, stdin: Bytes?) (output: dever.process.Output) { output = dever.process.Output { code = 23\n stdout = dever.bytes.from_text(\"fake\")\n stderr = dever.bytes.from_text(\"\") } }",
        ),
    ] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let source =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    let program = checked(&source);
    let llvm = dever_core::llvm::emit_test_suite(&program, &source).unwrap();
    assert!(!llvm.contains("@dever_rt_v1_external_start"));
    assert!(!llvm.contains("bin/missing-program"));
}

#[cfg(feature = "component")]
#[test]
fn command_bytes_wire_is_lossless_strict_and_bounded() {
    use dever_runtime::{bytes::Bytes, wire};
    for (bytes, json) in [(vec![], "\"\""), (vec![0, 255, 128, 10], "\"AP+ACg==\"")] {
        let bytes = Bytes::new(bytes);
        let mut writer = wire::Encoder::default();
        writer.bytes(&bytes).unwrap();
        let encoded = writer.finish().unwrap();
        assert_eq!(encoded.as_str(), json);
        assert_eq!(wire::parse(json).unwrap().bytes().unwrap(), bytes);
    }
    for json in [
        "null",
        "[0,255]",
        "\"_w==\"",
        "\"/w\"",
        "\"/x==\"",
        "\" /w==\"",
    ] {
        assert!(wire::parse(json).unwrap().bytes().is_err(), "{json}");
    }
    let oversized = Bytes::new(vec![0; wire::MAX_BYTES * 3 / 4]);
    assert!(wire::Encoder::default().bytes(&oversized).is_err());
}

fn sources(port: &str, adapter: &str, extra: &[(&str, &str)]) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("notification/mail/port.dever", port);
    sources.add("notification/mail/adapter.dever", adapter);
    if !extra
        .iter()
        .any(|(path, _)| *path == "notification/mail/app.dever")
    {
        sources.add("notification/mail/app.dever", APP);
    }
    for (path, text) in extra {
        sources.add(*path, *text);
    }
    sources
}

fn checked(sources: &SourceMap) -> dever_core::hir::Program {
    dever_core::check(sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(sources))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

fn generated_external_schema(port: &str, app: &str, adapter: &str) -> String {
    let source = sources(
        port,
        adapter,
        &[
            ("notification/mail/app.dever", app),
            (
                "main.dever",
                "main() (value: Int) { value = notification.mail.read() }",
            ),
        ],
    );
    let program = checked(&source);
    let generated = dever_core::native::emit(&program, &source, "main.main").unwrap();
    let definition = generated
        .split("dever_runtime::component::Definition {")
        .nth(1)
        .expect("external component definition");
    let schema = definition
        .split("schema: \"")
        .nth(1)
        .expect("external schema literal");
    schema
        .split('"')
        .next()
        .expect("external schema value")
        .to_owned()
}

fn rejected(sources: &SourceMap, message: &str) {
    let errors = dever_core::check(sources).expect_err("must reject invalid Port boundary");
    assert!(
        errors.iter().any(|error| error.message.contains(message)),
        "{errors:?}"
    );
}

#[test]
fn checks_bodyless_contract_implementation_and_setting_schema() {
    checked(&sources(PORT, ADAPTER, &[]));
    checked(&sources(
        PORT,
        "setting { value: Int }\nport.read() (value: Int) { value = setting.value }",
        &[],
    ));
    rejected(
        &sources(
            &PORT.replace("fails app.ReadResult", "{ value = 1 }"),
            ADAPTER,
            &[],
        ),
        "bodyless",
    );
    rejected(
        &sources(PORT, "port.read() (value: Text) { value = \"wrong\" }", &[]),
        "preserve Port",
    );
    rejected(
        &sources(
            PORT,
            "type Other { error Broken(message: Text) }\nport.read() (value: Int) { fail(Other.Broken(\"failed\")) }",
            &[],
        ),
        "failure set exceeds",
    );
    rejected(
        &sources(
            PORT,
            "setting { value: Int }\nport.read() (value: Int) {\n setting.value = 1\n value = 7\n}",
            &[],
        ),
        "read-only",
    );
    checked(&sources(
        PORT,
        ADAPTER,
        &[(
            "notification/mail/app.dever",
            &format!("{APP}\nsetting() (value: Int) {{ value = 1 }}"),
        )],
    ));
}

#[test]
fn port_contract_types_include_nested_app_dtos() {
    let port = "type Message { recipient: app.Recipient }\nread(message: Message) (value: Int) fails app.ReadResult";
    let adapter = "port.read(message: port.Message) (value: Int) { value = message.recipient.id }";
    let app = "type Recipient { id: Int }\ntype Unrelated { value: Int }\ntype ReadResult { error Unavailable(message: Text) }\nread() (value: Int) { value = port.read(port.Message { recipient = Recipient { id = 1 } }) }";
    let extra = [("notification/mail/app.dever", app)];
    checked(&sources(port, adapter, &extra));
    rejected(
        &sources(
            port,
            &format!("{adapter}\ntype Private {{ unrelated: app.Unrelated }}"),
            &extra,
        ),
        "outside the current domain boundary",
    );
}

#[test]
fn rejects_role_bypasses_and_private_port_failure_exports() {
    rejected(
        &sources(
            "type Error { error Failed(message: Text) }\nread() (value: Int) fails Error",
            ADAPTER,
            &[],
        ),
        "public App error choice",
    );
    rejected(
        &sources(
            PORT,
            "read() (value: Int) { value = 1 }\nport.read() (value: Int) { value = read() }",
            &[(
                "notification/mail/app.dever",
                "type ReadResult { error Unavailable(message: Text) }\nread() (value: Int) { value = adapter.read() }",
            )],
        ),
        "not callable",
    );
    rejected(
        &sources(
            PORT,
            ADAPTER,
            &[(
                "notification/mail/domain.dever",
                "read() (value: Int) { value = port.read() }",
            )],
        ),
        "not callable",
    );
    rejected(
        &sources(PORT, "port.read() (value: Int) { value = app.read() }", &[]),
        "not callable",
    );
}

#[test]
fn formats_port_and_qualified_adapter_without_changing_contracts() {
    for (path, text) in [
        ("notification/mail/port.dever", PORT),
        (
            "notification/mail/adapter.dever",
            "setting { value: Int }\nport.read() (value: Int) { value = setting.value }",
        ),
    ] {
        let mut sources = SourceMap::default();
        let id = sources.add(path, text);
        let formatted = dever_core::format::format(sources.get(id)).unwrap();
        let mut formatted_sources = SourceMap::default();
        let id = formatted_sources.add(path, &formatted);
        assert_eq!(
            formatted,
            dever_core::format::format(formatted_sources.get(id)).unwrap()
        );
    }
}

#[test]
fn checks_and_formats_external_exec_adapter_contract() {
    let adapter = "external exec \"worker/mail\" {\n allow network\n allow gpu\n}";
    let source = sources(PORT, adapter, &[]);
    checked(&source);
    let id = source
        .files()
        .iter()
        .find(|file| file.path().to_string_lossy().contains("adapter"))
        .expect("Adapter source")
        .id();
    let formatted = dever_core::format::format(source.get(id)).unwrap();
    assert_eq!(
        formatted,
        "external exec \"worker/mail\" {\n  allow network\n  allow gpu\n}\n"
    );
    let mut round_trip = SourceMap::default();
    round_trip.add("notification/mail/adapter.dever", &formatted);
    round_trip.add("notification/mail/app.dever", APP);
    round_trip.add("notification/mail/port.dever", PORT);
    checked(&round_trip);

    checked(&sources(
        PORT,
        "setting {\n endpoint: Text\n token: Secret\n}\nexternal exec \"worker/mail\" { allow network }",
        &[],
    ));

    rejected(
        &sources(PORT, "external exec \"../worker\" {}", &[]),
        "portable relative path",
    );
    rejected(
        &sources(PORT, "external exec \"worker\" { allow unknown }", &[]),
        "unknown external capability",
    );
    rejected(
        &sources(
            PORT,
            "external exec \"worker\" {\n allow file\n allow file\n}",
            &[],
        ),
        "duplicate external capability",
    );
    rejected(
        &sources(
            PORT,
            "external exec \"worker\" {}\nport.read() (value: Int) { value = 1 }",
            &[],
        ),
        "may contain only",
    );

    let mut missing = SourceMap::default();
    missing.add(
        "notification/mail/adapter.dever",
        "external exec \"worker\" {}",
    );
    rejected(&missing, "requires one Port identity");

    let mut ambiguous = SourceMap::default();
    ambiguous.add("notification/mail/app.dever", APP);
    ambiguous.add("notification/mail/port/primary.dever", PORT);
    ambiguous.add(
        "notification/mail/port/secondary.dever",
        "send() () fails app.ReadResult",
    );
    ambiguous.add(
        "notification/mail/adapter.dever",
        "external exec \"worker\" {}",
    );
    rejected(&ambiguous, "multiple Port identities");

    let mut wrong_role = SourceMap::default();
    let id = wrong_role.add("notification/mail/app.dever", "external exec \"worker\" {}");
    assert!(
        dever_core::parse(wrong_role.get(id)).is_err(),
        "external must stay contextual to Adapter sources"
    );
}

#[test]
fn external_adapter_exposes_exact_lib_requests_to_the_checked_program() {
    let source = sources(
        PORT,
        "external exec \"worker/mail\" {\n  lib \"pip:fixture@1.0.0\"\n  lib \"npm:fixture@1.0.0\"\n  allow network\n}",
        &[(
            "main.dever",
            "main() (value: Int) { value = notification.mail.read() }",
        )],
    );
    let program = checked(&source);
    assert_eq!(
        program.external_lib_requests(),
        vec![
            "npm:fixture@1.0.0".to_owned(),
            "pip:fixture@1.0.0".to_owned(),
        ]
    );
    let adapter = source
        .files()
        .iter()
        .find(|file| file.path().to_string_lossy().contains("adapter"))
        .unwrap();
    let formatted = dever_core::format::format(source.get(adapter.id())).unwrap();
    assert!(formatted.contains("lib \"pip:fixture@1.0.0\""));
    assert!(formatted.contains("lib \"npm:fixture@1.0.0\""));
}

#[test]
fn external_schema_binds_setting_and_top_level_wire_names() {
    let adapter = |field: &str| {
        format!(
            "setting {{ endpoint: {field} }}\nexternal exec \"worker/mail\" {{ allow network }}"
        )
    };
    let base = generated_external_schema(PORT, APP, &adapter("Text"));
    let changed_setting = generated_external_schema(PORT, APP, &adapter("Int"));
    let renamed_output = generated_external_schema(
        "read() (count: Int) fails app.ReadResult\n",
        "type ReadResult {\n Read(value: Int)\n error Unavailable(message: Text)\n}\nread() (count: Int) { count = port.read() }\n",
        &adapter("Text"),
    );
    assert_ne!(
        base, changed_setting,
        "setting schema must participate in the protocol identity"
    );
    assert_ne!(
        base, renamed_output,
        "top-level wire field names must participate in the protocol identity"
    );
}

#[test]
fn managed_external_adapters_expose_checked_typed_worker_contracts() {
    let port = "type Address { city: Text }\ntype Message {\n recipient: Address\n tags: List<Text>\n}\nread(message: Message) (value: Int?) fails app.ReadResult";
    let app = "type ReadResult { error Unavailable(message: Text, retry_after: Int?) }";
    for (ecosystem, entry) in [
        ("pip", "python/main.py"),
        ("npm", "javascript/main.mjs"),
        ("go", "go/main.go"),
    ] {
        let adapter = format!(
            "setting {{ token: Secret }}\nexternal {ecosystem} \"{entry}\" {{\n lib \"sample@1.2.3\"\n allow network\n}}"
        );
        let source = sources(port, &adapter, &[("notification/mail/app.dever", app)]);
        let contracts = checked(&source).external_worker_contracts();
        let [contract] = contracts.as_slice() else {
            panic!("one checked Worker")
        };
        assert_eq!(contract.ecosystem, ecosystem);
        assert_eq!(contract.libs, [format!("{ecosystem}:sample@1.2.3")]);
        assert_eq!(
            contract.setting.as_ref().unwrap()["fields"]["token"]["type"],
            "secret"
        );
        assert_eq!(
            contract.inputs["read"]["fields"]["message"]["fields"]["recipient"]["fields"]["city"]["type"],
            "text"
        );
        assert_eq!(
            contract.inputs["read"]["fields"]["message"]["fields"]["tags"]["type"],
            "list"
        );
        assert_eq!(
            contract.outputs["read"]["fields"]["value"]["type"],
            "nullable"
        );
        let errors = contract.errors.iter().collect::<Vec<_>>();
        let [(identity, error)] = errors.as_slice() else {
            panic!("one declared error")
        };
        assert!(identity.ends_with("ReadResult.Unavailable"));
        assert_eq!(error["fields"]["retry_after"]["type"], "nullable");
        assert_eq!(contract.sdk_manifest()["schema"], contract.schema);
        let adapter_source = source
            .files()
            .iter()
            .find(|file| file.path().to_string_lossy().contains("adapter"))
            .unwrap();
        let formatted = dever_core::format::format(source.get(adapter_source.id())).unwrap();
        assert!(formatted.contains(&format!("external {ecosystem} \"{entry}\"")));
    }
    rejected(
        &sources(
            PORT,
            "external pip \"python/main.py\" { lib \"npm:other@1.0\" }",
            &[],
        ),
        "exact name@version",
    );

    let source = sources(
        PORT,
        "external pip \"python/main.py\" { lib \"sample@1.2.3\" }",
        &[(
            "main.dever",
            "main() (value: Int) { value = notification.mail.read() }",
        )],
    );
    let generated = dever_core::native::emit(&checked(&source), &source, "main.main").unwrap();
    assert!(generated.contains("ecosystem: \"pip\""));
}

#[test]
fn markdown_contract_accepts_external_exec_declaration() {
    let mut source = SourceMap::default();
    source.add("notification/mail/app.dever", APP);
    source.add("notification/mail/port.dever", PORT);
    let adapter = "# 外部邮件适配器\n\n通过受管 Worker 实现邮件边界。\n\n- 包：`notification.mail.adapter`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n## Worker\n\n声明外部 Worker。\n\n- 声明：`external exec worker/mail`\n\n```dever\nexternal exec \"worker/mail\" {\n  allow network\n}\n```\n";
    source.add("notification/mail/adapter.dever.md", adapter);
    checked(&source);

    let mut invalid = SourceMap::default();
    invalid.add("notification/mail/app.dever", APP);
    invalid.add("notification/mail/port.dever", PORT);
    invalid.add(
        "notification/mail/adapter.dever.md",
        adapter.replace("external exec worker/mail`", "external exec worker/other`"),
    );
    rejected(&invalid, "documented Model declaration");
}

#[test]
fn external_exec_adapter_emits_dispatch_and_shutdown_cleanup() {
    let source = sources(
        PORT,
        "external exec \"worker/mail\" { allow network }",
        &[(
            "main.dever",
            "main() (value: Int) { value = notification.mail.read() }",
        )],
    );
    let program = checked(&source);
    let generated = dever_core::native::emit(&program, &source, "main.main").unwrap();
    assert!(generated.contains("dever_runtime::component::call("));
    assert!(generated.contains("dever_runtime::component::shutdown().await"));
    assert!(generated.contains("match (application, cleanup)"));
}

#[test]
fn markdown_contract_includes_declared_failures_and_qualified_implementation() {
    let mut source = SourceMap::default();
    source.add("notification/mail/app.dever", APP);
    let port = "# 邮件边界\n\n读取邮件计数。\n\n- 包：`notification.mail.port`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n## 读取计数\n\n声明读取合同。\n\n- 函数：`read`\n- 输入：无\n- 输出：\n  - `value: Int`：计数。\n- 允许失败：`app.ReadResult`\n\n```dever\nread() (value: Int) fails app.ReadResult\n```\n";
    let adapter = "# 邮件实现\n\n提供固定计数。\n\n- 包：`notification.mail.adapter`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n## 读取计数\n\n实现读取合同。\n\n- 函数：`port.read`\n- 输入：无\n- 输出：\n  - `value: Int`：计数。\n\n```dever\nport.read() (value: Int) { value = 7 }\n```\n";
    source.add("notification/mail/port.dever.md", port);
    source.add("notification/mail/adapter.dever.md", adapter);
    checked(&source);
    let mut invalid = SourceMap::default();
    invalid.add("notification/mail/app.dever", APP);
    invalid.add(
        "notification/mail/port.dever.md",
        port.replace("允许失败：`app.ReadResult`", "允许失败：`app.Other`"),
    );
    invalid.add("notification/mail/adapter.dever", ADAPTER);
    rejected(&invalid, "documented Port failure choice");
}

#[test]
fn effects_union_and_direct_io_boundaries_are_conservative() {
    let mut source = SourceMap::default();
    source.add("notification/mail/app.dever", "pause() () { port.pause() }");
    source.add(
        "notification/mail/port.dever",
        "pause() () fails dever.time.SleepResult",
    );
    source.add(
        "notification/mail/adapter/blocking.dever",
        "port.pause() () { dever.time.sleep(1) }",
    );
    source.add(
        "notification/mail/adapter/suspending.dever",
        "port.pause() () { dever.task.sleep(1) }",
    );
    rejected(&source, "blocking");

    let mut source = SourceMap::default();
    source.add(
        "notification/mail/app.dever",
        "open() (file: dever.system.File) { file = dever.io.open(\"not-opened-by-check\") }",
    );
    rejected(&source, "I/O must pass through a Port");

    let source = sources(
        PORT,
        "port.read() (value: Int) { fail(app.ReadResult.Unavailable(\"offline\")) }",
        &[],
    );
    let program = checked(&source);
    assert!(program.api_snapshot().contains("Unavailable"));
    let source = sources(
        PORT,
        ADAPTER,
        &[(
            "notification/mail/app.dever",
            "type ReadResult {\n Read(value: Int)\n error Unavailable(message: Text)\n}\ncapture() (response: ReadResult) { response = result(port.read()) }",
        )],
    );
    checked(&source);
}

#[test]
fn native_selection_decodes_only_selected_setting_and_rejects_invalid_config() {
    let mut source = SourceMap::default();
    source.add(
        "main.dever",
        "main() (value: Int) { value = notification.mail.read() }",
    );
    source.add("notification/mail/app.dever", APP);
    source.add("notification/mail/port.dever", PORT);
    source.add("notification/mail/adapter/fixed.dever", ADAPTER);
    source.add("notification/mail/adapter/configured.dever", "setting {\n value: Int\n token: Secret\n}\nport.read() (value: Int) { value = setting.value }");
    let program = checked(&source);
    let native = dever_core::native::compile(
        &program,
        &source,
        "main.main",
        std::ffi::OsStr::new("rustc"),
    )
    .unwrap();
    let root = TemporaryDirectory::new();
    let executable = root.path().join("program");
    native.save(&executable).unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    let config = root.path().join("config/setting.json");
    for (text, expected) in [
        (
            r#"{"adapter":{"notification.mail":{"use":"fixed"}}}"#,
            Some("value = 7\n"),
        ),
        (
            r#"{"adapter":{"notification.mail":{"use":"configured","setting":{"value":19,"token":"never-print-this-secret"}}}}"#,
            Some("value = 19\n"),
        ),
        (
            r#"{"adapter":{"notification.mail":{"use":"unknown"}}}"#,
            None,
        ),
        (r#"{"adapter":{"notification.mail":{}}}"#, None),
        (
            r#"{"adapter":{"notification.mail":{"use":"configured"}}}"#,
            None,
        ),
        (
            r#"{"adapter":{"notification.mail":{"use":"configured","setting":{"value":19,"extra":1}}}}"#,
            None,
        ),
        (
            r#"{"adapter":{"notification.mail":{"use":"configured","setting":{"value":"never-print-this-secret"}}}}"#,
            None,
        ),
        (
            r#"{"adapter":{"notification.mail":{"use":"fixed","use":"configured"}}}"#,
            None,
        ),
        (r#"{"adapter":{"unknown.port":{"use":"fixed"}}}"#, None),
        (r#"{"adapter":null}"#, None),
    ] {
        fs::write(&config, text).unwrap();
        let result = Command::new(&executable).output().unwrap();
        assert_eq!(
            result.status.success(),
            expected.is_some(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if let Some(expected) = expected {
            assert_eq!(String::from_utf8_lossy(&result.stdout), expected);
        }
        assert!(!String::from_utf8_lossy(&result.stderr).contains("never-print-this-secret"));
    }
}

#[test]
fn single_adapter_executes_without_settings() {
    let source = sources(
        PORT,
        ADAPTER,
        &[(
            "main.dever",
            "main() (value: Int) { value = notification.mail.read() }",
        )],
    );
    let program = checked(&source);
    let native = dever_core::native::compile(
        &program,
        &source,
        "main.main",
        std::ffi::OsStr::new("rustc"),
    )
    .unwrap();
    let root = TemporaryDirectory::new();
    let executable = root.path().join("program");
    native.save(&executable).unwrap();
    let result = Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "value = 7\n");
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/setting.json"),
        r#"{"adapter":null}"#,
    )
    .unwrap();
    let result = Command::new(&executable).output().unwrap();
    assert!(
        !result.status.success(),
        "explicit null must not become an omitted binding"
    );
}

#[test]
fn test_suite_uses_each_case_fake_without_production_settings() {
    let root = TemporaryDirectory::new();
    for (path, text) in [
        ("module/main.dever", "main() () {}"),
        ("module/notification/mail/app.dever", APP),
        ("module/notification/mail/port.dever", PORT),
        (
            "module/notification/mail/adapter.dever",
            "setting { value: Int }\nport.read() (value: Int) { value = setting.value }",
        ),
        (
            "test/notification/mail/first.dever",
            "first() () { assert_eq(app.read(), 11) }\nport.read() (value: Int) { value = 11 }",
        ),
        (
            "test/notification/mail/second.dever",
            "second() () { assert_eq(app.read(), 23) }\nport.read() (value: Int) { value = 23 }",
        ),
        (
            "config/setting.json",
            "invalid production setting must not be read",
        ),
    ] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let source =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    let program = checked(&source);
    let native =
        dever_core::native::compile_test_suite(&program, &source, std::ffi::OsStr::new("rustc"))
            .unwrap();
    for index in 0..2 {
        let result = Command::new(native.executable())
            .arg(index.to_string())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fs::write(
        root.path().join("test/notification/mail/first.dever"),
        "first() () { assert_eq(app.read(), 11) }",
    )
    .unwrap();
    let source =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    rejected(&source, "requires a fake");
}

#[test]
fn test_suite_keeps_blocking_and_suspending_fakes_case_local() {
    let root = TemporaryDirectory::new();
    for (path, text) in [
        ("module/main.dever", "main() () {}"),
        (
            "module/notification/mail/app.dever",
            "pause() () { port.pause() }",
        ),
        (
            "module/notification/mail/port.dever",
            "pause() () fails dever.time.SleepResult",
        ),
        (
            "module/notification/mail/adapter.dever",
            "port.pause() () {}",
        ),
        (
            "test/notification/mail/first.dever",
            "first() () {\n dever.time.sleep(1)\n app.pause()\n}\nport.pause() () { dever.time.sleep(1) }",
        ),
        (
            "test/notification/mail/second.dever",
            "second() () { app.pause() }\nport.pause() () { dever.task.sleep(1) }",
        ),
    ] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let source =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    let program = checked(&source);
    let native =
        dever_core::native::compile_test_suite(&program, &source, std::ffi::OsStr::new("rustc"))
            .unwrap();
    for index in 0..2 {
        let result = Command::new(native.executable())
            .arg(index.to_string())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let second = root.path().join("test/notification/mail/second.dever");
    fs::write(&second, "second() () {\n dever.time.sleep(1)\n app.pause()\n}\nport.pause() () { dever.task.sleep(1) }").unwrap();
    let source =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    rejected(&source, "blocking");
    fs::write(&second, "type Other { error Broken(message: Text) }\nsecond() () {}\nport.pause() () { fail(Other.Broken(\"unused fake still must match\")) }").unwrap();
    let source =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    rejected(&source, "failure set exceeds");
    fs::write(
        &second,
        "second() () {}\nunused(action: handler() ()) () pure { dever.time.sleep(1) }",
    )
    .unwrap();
    let source =
        SourceMap::load_project(&root.path().join("module"), &root.path().join("test")).unwrap();
    rejected(&source, "pure function");
}
