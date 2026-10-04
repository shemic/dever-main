use dever_core::source::SourceMap;
use std::path::Path;

const RUNTIME_CONFIG: &str = r#"
lifetime() (value: Int) { value = 1000 }
idle_sample_ms() (value: Int) { value = 10 }
pending() (value: Int) { value = 4 }
channel_capacity() (value: Int) { value = 4 }
input() (value: Bytes) { value = dever.bytes.from_text("xxxx") }
pending_input() (value: Bytes) { value = dever.bytes.from_text("xxxx") }
"#;

const HTTP_CONFIG: &str = r#"
lifetime() (value: Int) { value = 1000 }
certificate() (value: Bytes) { value = dever.bytes.from_text("") }
key() (value: Bytes) { value = dever.bytes.from_text("") }
limits() (value: dever.http.Limits) { value = dever.http.default_limits() }
http2_limits() (value: dever.http.Limits) { value = dever.http.default_limits() }
large_body() (value: Bytes) { value = dever.bytes.from_text("xxxxxxxxxxxxxxxx") }
"#;

fn check_fixture(fixture: &str, entries: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../performance/fixtures")
        .join(fixture);
    let loaded = SourceMap::load(&root).expect("load performance fixture");
    let mut sources = SourceMap::default();
    for source in loaded.files() {
        sources.add(
            format!("benchmark/{fixture}/{}", source.path().display()),
            source.text(),
        );
    }
    match fixture {
        "runtime" => {
            sources.add(
                "benchmark/bench/app.dever",
                include_str!("../../performance/fixtures/bench.dever"),
            );
            sources.add("benchmark/config/app.dever", RUNTIME_CONFIG);
        }
        "http" => {
            sources.add("benchmark/config/app.dever", HTTP_CONFIG);
        }
        "live" => {}
        _ => unreachable!("unknown performance fixture"),
    }
    let program = dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    for entry in entries {
        program
            .application_warnings(&format!("benchmark.{fixture}.{entry}"))
            .unwrap_or_else(|error| panic!("{fixture}.{entry}: {error}"));
    }
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile::default(),
    )
    .unwrap_or_else(|error| panic!("{fixture} native emission: {error}"));
    for entry in entries {
        assert!(
            generated.contains(&format!("\"benchmark.{fixture}.{entry}\" => {{")),
            "missing production CMD root benchmark.{fixture}.{entry}"
        );
    }
}

#[test]
fn runtime_port_fixture_checks_entries() {
    check_fixture(
        "runtime",
        &[
            "sync_idle",
            "async_idle",
            "tasks",
            "tasks_on_worker",
            "channels",
            "cancel",
        ],
    );
}

#[test]
fn http_port_fixture_checks_entries() {
    check_fixture("http", &["http", "https", "http2", "https2"]);
}

#[test]
fn live_port_fixture_checks_entries() {
    check_fixture("live", &["tcp", "ws", "sse"]);
}
