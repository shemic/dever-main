mod support;

use std::fs;
use std::process::Command;

use dever_core::wire::Policy;
use dever_runtime::{time, wire};

#[test]
fn time_calendar_precision_and_checked_arithmetic() {
    for (text, expected) in [
        ("1970-01-01T00:00:00Z", 0),
        ("1969-12-31T23:59:59.999Z", -1),
        ("1970-01-01T08:00:00+08:00", 0),
        ("2000-02-29T00:00:00Z", 951_782_400_000),
    ] {
        let value = time::parse_datetime(text).unwrap();
        assert_eq!(value, expected);
        assert_eq!(
            time::parse_datetime(&time::format_datetime(value).unwrap()).unwrap(),
            value
        );
    }
    assert_eq!(time::format_datetime(0).unwrap(), "1970-01-01T00:00:00Z");
    assert_eq!(
        time::format_datetime(-1).unwrap(),
        "1969-12-31T23:59:59.999Z"
    );
    for text in [
        "1900-02-29T00:00:00Z",
        "2000-02-29",
        "2016-12-31T23:59:60Z",
        "1970-01-01T00:00:00.0001Z",
        "1970-01-01T00:00:00.0000000001Z",
    ] {
        assert!(time::parse_datetime(text).is_err(), "{text}");
    }
    for text in ["0000-01-01", "1969-12-31", "2000-02-29", "9999-12-31"] {
        assert_eq!(
            time::format_date(time::parse_date(text).unwrap()).unwrap(),
            text
        );
    }
    for text in ["1900-02-29", "2024-2-01", "2024-02-30", "+2024-02-01"] {
        assert!(time::parse_date(text).is_err());
    }
    for text in ["00:00:00", "23:59:59.999", "12:30:05.001"] {
        assert_eq!(
            time::format_time(time::parse_time(text).unwrap()).unwrap(),
            text
        );
    }
    for text in ["24:00:00", "23:59:60", "12:00:00.1", "12:00:00.0001"] {
        assert!(time::parse_time(text).is_err());
    }
    assert!(time::format_time(86_400_000).is_err());
    assert!(time::format_time(-1).is_err());
    assert!(time::format_date(1).is_err());
    assert!(time::format_datetime(i64::MAX).is_err());
    assert!(time::format_datetime(i64::MIN).is_err());
    assert!(time::add(i64::MAX, 1).is_err());
    assert!(time::subtract(i64::MIN, 1).is_err());
    assert!(time::difference(i64::MAX, -1).is_err());
    assert_eq!(time::add(-1, time::duration(1)).unwrap(), 0);
    assert_eq!(time::difference(-1, 0).unwrap(), -1);
}

#[test]
fn parser_rejects_ambiguous_and_unbounded_input_without_losing_json_numbers() {
    for text in [
        r#"{"a":{"x":1,"x":2}}"#,
        r#"[{"x":1,"x":2}]"#,
        "true false",
        "NaN",
        "Infinity",
        "[1,]",
        r#""\ud800""#,
    ] {
        assert!(wire::parse(text).is_err(), "{text}");
    }
    let precise =
        r#"{"big":1234567890123456789012345678901234567890,"tiny":1.234567890123456789e-900}"#;
    assert_eq!(wire::parse(precise).unwrap().raw(), precise);
    assert_eq!(
        wire::parse("-9223372036854775808").unwrap().int().unwrap(),
        i64::MIN
    );
    for text in ["9223372036854775808", "1.0", "1e0", "true", "\"1\""] {
        assert!(wire::parse(text).unwrap().int().is_err());
    }
    assert!(wire::parse("1e900").unwrap().float().is_err());
    let at_depth = format!(
        "{}0{}",
        "[".repeat(wire::MAX_DEPTH),
        "]".repeat(wire::MAX_DEPTH)
    );
    assert!(wire::parse(&at_depth).is_ok());
    assert!(wire::parse(&format!("[{at_depth}]")).is_err());
    assert!(wire::parse(&format!("\"{}\"", "x".repeat(wire::MAX_BYTES))).is_err());
    assert!(wire::parse(&format!("[{}]", vec!["0"; wire::MAX_ELEMENTS].join(","))).is_err());
}

#[test]
fn encoder_enforces_limits_and_preserves_raw_json_in_api_envelope() {
    let raw = "1234567890123456789012345678901234567890";
    let mut writer = wire::Encoder::default();
    writer.begin_object().unwrap();
    writer.key("raw").unwrap();
    writer.json(raw).unwrap();
    writer.key("when").unwrap();
    writer.text(&time::format_datetime(-1).unwrap()).unwrap();
    writer.end().unwrap();
    let response = dever_runtime::api::success(writer.finish().unwrap());
    let text = std::str::from_utf8(response.body.values()).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        text,
        format!(
            "{{\"code\":0,\"message\":\"ok\",\"data\":{{\"raw\":{raw},\"when\":\"1969-12-31T23:59:59.999Z\"}}}}"
        )
    );
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(wire::Encoder::default().float(value).is_err());
    }
    let mut writer = wire::Encoder::default();
    writer.begin_array().unwrap();
    for _ in 1..wire::MAX_ELEMENTS {
        writer.null().unwrap();
    }
    assert!(writer.null().is_err());
    assert!(wire::Encoder::default().json(r#"{"x":1,"x":2}"#).is_err());

    let mut writer = wire::Encoder::default();
    writer.begin_object().unwrap();
    writer.key("missing").unwrap();
    assert!(writer.end().is_err());
    assert!(writer.finish().is_err());

    let mut writer = wire::Encoder::default();
    writer.begin_array().unwrap();
    writer.text(&"x".repeat(wire::MAX_BYTES - 3)).unwrap();
    assert!(writer.end().is_err());
    assert!(writer.finish().is_err());

    let mut writer = wire::Encoder::default();
    writer.begin_array().unwrap();
    writer.text(&"x".repeat(wire::MAX_BYTES - 4)).unwrap();
    assert!(writer.int(1).is_err());
    assert!(writer.end().is_err());
    assert!(writer.finish().is_err());
}

const TYPES: &str = r#"
public type Entry { label: Text
  count: Int? }
public type Payload {
  active: Bool
  count: Int
  ratio: Float
  amount: Decimal
  label: Text
  key: Id
  uuid: Uuid
  instant: DateTime
  date: Date
  clock: Time
  elapsed: Duration
  raw: Json
  entries: List<Entry>
}
public type Credentials { password: Secret }
public main() () {}
"#;

#[test]
fn schema_policy_and_field_contracts_have_one_owner() {
    let program = support::checked(&support::sources(TYPES));
    let output = program.wire_schema("main.Payload", Policy::Output).unwrap();
    let input = program
        .wire_schema("main.Payload", Policy::ApiInput)
        .unwrap();
    let job = program.wire_schema("main.Payload", Policy::Job).unwrap();
    assert_ne!(output.fingerprint(), input.fingerprint());
    assert_ne!(output.fingerprint(), job.fingerprint());
    let shifted = support::checked(&support::sources(&format!(
        "public type Unrelated {{ value: Text }}\n{TYPES}"
    )));
    assert_eq!(
        output.fingerprint(),
        shifted
            .wire_schema("main.Payload", Policy::Output)
            .unwrap()
            .fingerprint()
    );
    for policy in [Policy::ApiInput, Policy::SettingInput] {
        let schema = program.wire_schema("main.Credentials", policy).unwrap();
        let code = dever_core::native::wire::emit(&schema, 0);
        assert!(code.contains("Secret::from_input"));
        assert!(!code.contains("_encode"));
    }
    for policy in [Policy::Output, Policy::Job] {
        assert!(program.wire_schema("main.Credentials", policy).is_err());
    }
    for declaration in [
        "public type Bad { private name: Text }",
        "public type Bad { port: Int >= 1 and <= 65535 }",
        "public type Bad { stream: Stream<Int> }",
        "public type Bad { values: Map<Text, Int> }",
        "public type Bad { One\nTwo }",
    ] {
        let program = support::checked(&support::sources(&format!(
            "{declaration}\npublic main() () {{}}"
        )));
        for policy in [
            Policy::Output,
            Policy::ApiInput,
            Policy::SettingInput,
            Policy::Job,
        ] {
            assert!(
                program.wire_schema("main.Bad", policy).is_err(),
                "{declaration}"
            );
        }
    }
    let mut bounded = support::sources("main() () {}");
    bounded.add(
        "counter/api/value.dever",
        "get_value() (response: Int >= 0) { response = 1 }",
    );
    assert!(
        dever_core::check(&bounded)
            .unwrap_err()
            .iter()
            .any(|error| error
                .message
                .contains("wire fields must be public and unconstrained"))
    );
}

#[test]
fn wire_schemas_do_not_change_model_time_storage_identity() {
    let mut sources = support::sources("main() () { calendar.event.save() }");
    sources.add(
        "calendar/event/model.dever",
        "type Event { instant: DateTime\ndate: Date\ntime: Time\nelapsed: Duration }",
    );
    sources.add(
        "calendar/event/app.dever",
        r#"type ClockView { instant: DateTime
      date: Date
      time: Time
      elapsed: Duration }
    save() () {
      stored = model.create({
        instant = dever.time.parse_datetime("1969-12-31T23:59:59.999Z")
        date = dever.time.parse_date("2000-02-29")
        time = dever.time.parse_time("23:59:59.999")
        elapsed = dever.time.duration(-7)
      })
    }"#,
    );
    let program = support::checked(&sources);
    let before = program.model_snapshot();
    for (name, ty) in [
        ("instant", "DateTime"),
        ("date", "Date"),
        ("time", "Time"),
        ("elapsed", "Duration"),
    ] {
        assert!(
            before.contains(&format!("field {name} {ty} nullable=false")),
            "{before}"
        );
    }
    for policy in [
        Policy::Output,
        Policy::ApiInput,
        Policy::SettingInput,
        Policy::Job,
    ] {
        program
            .wire_schema("calendar.event.ClockView", policy)
            .unwrap();
        let id = program
            .wire_schema("calendar.event.model.id", policy)
            .unwrap();
        assert!(
            id.fingerprint()
                .contains("ModelId(\"calendar.event.model.id\")")
        );
        let model = program
            .wire_schema("calendar.event.model.Event", policy)
            .unwrap();
        assert!(
            model
                .fingerprint()
                .contains("ModelId(\"calendar.event.model.id\")")
        );
    }
    assert_eq!(before, program.model_snapshot());
    let generated = dever_core::native::emit(&program, &sources, "main.main").unwrap();
    assert!(generated.contains("f0: i64,\nf1: i64,\nf2: i64,\nf3: i64,"));
    assert_eq!(
        generated
            .matches("dever_runtime::orm::Value::Int(p")
            .count(),
        4
    );
}

#[test]
fn official_time_bridge_is_pure_and_matches_reference() {
    let source = r#"public main() (stamp: Text, date: Text, clock: Text, delta: Duration, invalid: dever.time.DateResult) pure {
  instant = dever.time.parse_datetime("1969-12-31T23:59:59.999Z")
  stamp = dever.time.format_datetime(dever.time.add(instant, dever.time.duration(1)))
  date = dever.time.format_date(dever.time.parse_date("2000-02-29"))
  clock = dever.time.format_time(dever.time.parse_time("23:59:59.999"))
  delta = dever.time.difference(dever.time.subtract(instant, dever.time.duration(1)), instant)
  invalid = result(dever.time.parse_date("1900-02-29"))
}"#;
    let sources = support::sources(source);
    let program = support::checked(&sources);
    let expected = "stamp = 1970-01-01T00:00:00Z\ndate = 2000-02-29\nclock = 23:59:59.999\ndelta = -1\ninvalid = dever.time.DateResult.Failed(invalid calendar Date)\n";
    assert_eq!(support::stdout(source), expected);
    let reference = dever_core::reference::evaluate(&program, "main.main").unwrap();
    assert_eq!(reference.stdout, expected);
    support::rejected(
        "public main() (value: DateTime) pure { value = dever.time.now() }",
        "pure",
    );
}

#[test]
fn time_api_has_equal_plain_and_markdown_contracts() {
    let code = "main() (value: Text) pure { value = dever.time.format_datetime(dever.time.parse_datetime(\"2000-02-29T08:00:00+08:00\")) }";
    let plain = support::sources(code);
    let mut markdown = dever_core::source::SourceMap::default();
    markdown.add("main.dever.md", format!("# 时间转换\n\n验证 UTC 毫秒合同。\n\n- 包：`main`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n## 入口\n\n转换日期时间。\n\n- 函数：`main`\n- 输入：无\n- 输出：\n  - `value: Text`：UTC 时间。\n\n```dever\n{code}\n```\n"));
    let plain_program = support::checked(&plain);
    let markdown_program = support::checked(&markdown);
    assert_eq!(
        plain_program.api_snapshot(),
        markdown_program.api_snapshot()
    );
    let expected = "value = 2000-02-29T00:00:00Z\n";
    for program in [&plain_program, &markdown_program] {
        let output = dever_core::reference::evaluate(program, "main.main").unwrap();
        assert!(output.fault.is_none());
        assert_eq!(output.stdout, expected);
    }
}

#[test]
fn generated_api_uses_shared_time_codec_without_a_listener() {
    let mut sources = support::sources("main() () { dever.api.serve() }");
    sources.add("calendar/api/time.dever", "get_now() (response: DateTime) { response = dever.time.parse_datetime(\"1970-01-01T08:00:00+08:00\") }\nget_again() (response: DateTime) { response = dever.time.parse_datetime(\"1969-12-31T23:59:59.999Z\") }");
    let program = support::checked(&sources);
    let mut code = dever_core::native::emit(&program, &sources, "main.main")
        .unwrap()
        .replace("fn main()", "fn unused_main()");
    assert_eq!(code.matches("fn wire_0_encode").count(), 1);
    assert!(!code.contains("fn wire_1_encode"));
    code.push_str(r#"
fn main() {
    let request = dever_runtime::http::Request {
        method: "GET".into(), target: "/calendar/time/now".into(), headers: vec![], body: Bytes::from_text(""),
    };
    let response = task::run_entry(dever_runtime::api::scoped_request(request, api_route)).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(std::str::from_utf8(response.body.values()).unwrap(), "{\"code\":0,\"message\":\"ok\",\"data\":\"1970-01-01T00:00:00Z\"}");
}
"#);
    compile_and_run(&code);
}

#[test]
fn generated_concrete_codec_roundtrips_and_rejects_bad_records() {
    let sources = support::sources(TYPES);
    let program = support::checked(&sources);
    let mut code = dever_core::native::emit(&program, &sources, "main.main")
        .unwrap()
        .replace("fn main()", "fn unused_main()");
    code.push_str(&dever_core::native::wire::emit(
        &program.wire_schema("main.Payload", Policy::Output).unwrap(),
        0,
    ));
    code.push_str(&dever_core::native::wire::emit(
        &program
            .wire_schema("main.Credentials", Policy::SettingInput)
            .unwrap(),
        1,
    ));
    code.push_str(r###"
fn main() {
    let raw = r#"{"active":true,"count":-9223372036854775808,"ratio":-0.0,"amount":"1234567890123456789012345678901234","label":"你好","key":"9007199254740993","uuid":"12345678-1234-5678-9012-123456789abc","instant":"1969-12-31T23:59:59.999Z","date":"2000-02-29","clock":"23:59:59.999","elapsed":-7,"raw":1234567890123456789012345678901234567890,"entries":[{"label":"entry"}]}"#;
    let decoded = wire_0_decode(raw).unwrap();
    assert_eq!(decoded.f1, i64::MIN);
    assert_eq!(decoded.f2.to_bits(), (-0.0f64).to_bits());
    assert_eq!(decoded.f3, DecimalValue::parse("1234567890123456789012345678901234").unwrap());
    assert_eq!(decoded.f4, "你好");
    assert_eq!(decoded.f5.0, "9007199254740993");
    assert_eq!(decoded.f6.to_string(), "12345678-1234-5678-9012-123456789abc");
    assert_eq!(decoded.f7, -1);
    assert_eq!(decoded.f8, 951782400000);
    assert_eq!(decoded.f9, 86399999);
    assert_eq!(decoded.f10, -7);
    assert!(decoded.f12.values()[0].f1.is_none());
    let encoded = wire_0_encode(&decoded).unwrap();
    assert!(encoded.as_str().contains("1234567890123456789012345678901234567890"));
    assert!(encoded.as_str().contains("1969-12-31T23:59:59.999Z"));
    let again = wire_0_decode(encoded.as_str()).unwrap();
    assert_eq!(encoded.as_str(), wire_0_encode(&again).unwrap().as_str());
    assert!(wire_0_decode(&raw.replace("\"count\":-9223372036854775808,", "")).is_err());
    assert!(wire_0_decode(&raw.replace("\"active\":true", "\"extra\":true,\"active\":true")).is_err());
    assert!(wire_0_decode(&raw.replace("\"label\":\"entry\"", "\"label\":\"entry\",\"label\":\"again\"")).is_err());
    assert!(wire_0_decode(&raw.replace("\"ratio\":-0.0", "\"ratio\":1e900")).is_err());
    assert!(wire_1_decode(r#"{"password":"secret"}"#).is_ok());
    assert!(wire_1_decode(r#"{"password":42}"#).is_err());
}
"###);
    compile_and_run(&code);
}

fn compile_and_run(code: &str) {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let built = Command::new("cargo")
        .current_dir(&workspace)
        .args([
            "build",
            "--locked",
            "--offline",
            "-p",
            "dever-runtime",
            "--features",
            "api",
            "--config",
            "profile.dev.debug=0",
            "--message-format=json",
        ])
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let artifact = String::from_utf8(built.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| {
            value["reason"] == "compiler-artifact" && value["target"]["name"] == "dever_runtime"
        })
        .unwrap();
    let library = artifact["filenames"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .find(|name| name.ends_with(".rlib"))
        .unwrap();
    let directory = support::temp::TemporaryDirectory::new();
    let source = directory.path().join("codec.rs");
    let executable = directory.path().join("codec");
    fs::write(&source, code).unwrap();
    let dependencies = std::path::Path::new(library).parent().unwrap().join("deps");
    let built = Command::new("rustc")
        .args(["--edition=2024", "-C", "debuginfo=0", "--extern"])
        .arg(format!("dever_runtime={library}"))
        .arg("-L")
        .arg(format!("dependency={}", dependencies.display()))
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let result = Command::new(executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
