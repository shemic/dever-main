use std::path::Path;

use dever_core::source::SourceMap;

#[path = "../examples/network_bench/response.rs"]
mod benchmark_response;

const CONFIG: &str = r#"
lifetime() (value: Int) { value = 1000 }
idle_sample_ms() (value: Int) { value = 10 }
pending() (value: Int) { value = 4 }
channel_capacity() (value: Int) { value = 4 }
input() (value: Bytes) { value = dever.bytes.from_text("xxxx") }
pending_input() (value: Bytes) { value = dever.bytes.from_text("xxxx") }
orm_input() (value: Bytes) { value = dever.bytes.from_text("xxxx") }
orm_rows() (value: Bytes) { value = dever.bytes.from_text("rrrr") }
orm_concurrency() (value: Int) { value = 2 }
orm_cancel_work() (value: Int) { value = 1000 }
certificate() (value: Bytes) { value = dever.bytes.from_text("") }
key() (value: Bytes) { value = dever.bytes.from_text("") }
limits() (value: dever.http.Limits) { value = dever.http.default_limits() }
http2_limits() (value: dever.http.Limits) { value = dever.http.default_limits() }
large_body() (value: Bytes) { value = dever.bytes.from_text("xxxxxxxxxxxxxxxx") }
"#;

fn checked_fixture(name: &str) -> (SourceMap, dever_core::hir::Program) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../performance/fixtures")
        .join(name);
    let loaded = SourceMap::load(&root).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    });
    let mut sources = SourceMap::default();
    for source in loaded.files() {
        let source_path = source.path().to_str().unwrap();
        let path = match (name, source_path) {
            ("orm", "app.dever") => "benchmark/item/app.dever".to_owned(),
            ("orm", "app/model/item.dever") => "benchmark/item/model.dever".to_owned(),
            ("orm", "app/api.dever") => "benchmark/item/api.dever".to_owned(),
            (_, "app.dever") => match name {
                "runtime" => "benchmark/runtime/app.dever".to_owned(),
                "http" => "benchmark/http/app.dever".to_owned(),
                "live" => "benchmark/live/app.dever".to_owned(),
                _ => unreachable!("unknown performance fixture"),
            },
            ("runtime" | "http" | "live", "port.dever" | "adapter.dever" | "api.dever") => {
                format!("benchmark/{name}/{source_path}")
            }
            _ => unreachable!("unknown performance fixture source"),
        };
        sources.add(path, source.text());
    }
    sources.add(
        "benchmark/bench/app.dever",
        include_str!("../../performance/fixtures/bench.dever"),
    );
    sources.add("benchmark/config/app.dever", CONFIG);
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
fn orm_performance_fixture_checks_every_packaged_entry() {
    let (sources, program) = checked_fixture("orm");
    for entry in [
        "orm_idle",
        "orm_crud",
        "orm_list",
        "orm_cursor",
        "orm_stream",
        "orm_pool",
        "orm_seed",
    ] {
        program
            .application_warnings(&format!("benchmark.item.{entry}"))
            .unwrap_or_else(|error| panic!("benchmark.item.{entry}: {error}"));
    }
    let snapshot = program.api_snapshot();
    assert!(snapshot.contains("function benchmark.item.count()"));
    assert!(snapshot.contains("function benchmark.item.cancel()"));
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(generated.contains("(\"GET\", \"/benchmark/item/count\")"));
    assert!(generated.contains("(\"POST\", \"/benchmark/item/cancel\")"));
    assert!(generated.contains("dever_runtime::api::serve(api_route).await"));
}

#[test]
fn cms_performance_fixture_checks_stock_source_parity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cms");
    let checked = |format| {
        let project = root.join(format);
        let sources =
            SourceMap::load_project(&project.join("module"), &project.join("test")).unwrap();
        dever_core::check(&sources).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&sources))
                    .collect::<String>()
            )
        })
    };
    let plain = checked("dever");
    let markdown = checked("md");
    assert_eq!(plain.api_snapshot(), markdown.api_snapshot());
    assert_eq!(plain.model_snapshot(), markdown.model_snapshot());
}

#[test]
fn orm_http_client_contract_uses_configured_row_count() {
    let _ = benchmark_response::HyperResponses::new().response("/json");
    let _ = benchmark_response::RuntimeResponses::new().response("/json");
    let first = benchmark_response::ExpectedResponse::for_path(
        benchmark_response::ORM_COUNT_PATH,
        Some(17),
    )
    .unwrap();
    assert_eq!(
        first.body.as_ref(),
        br#"{"code":0,"message":"ok","data":17}"#
    );
    assert_eq!(first.content_type, "application/json; charset=utf-8");
    let second = benchmark_response::ExpectedResponse::for_path(
        benchmark_response::ORM_COUNT_PATH,
        Some(23),
    )
    .unwrap();
    assert_ne!(first.body, second.body);
    assert!(
        benchmark_response::ExpectedResponse::for_path(benchmark_response::ORM_COUNT_PATH, None,)
            .is_err()
    );
}

#[test]
fn profile_free_performance_fixtures_still_check() {
    for (fixture, entries) in [
        (
            "runtime",
            &[
                "sync_idle",
                "async_idle",
                "tasks",
                "tasks_on_worker",
                "channels",
                "cancel",
            ][..],
        ),
        ("http", &["http", "https", "http2", "https2"]),
        ("live", &["tcp", "ws", "sse"]),
    ] {
        let (_sources, program) = checked_fixture(fixture);
        for entry in entries {
            program
                .application_warnings(&format!("benchmark.{fixture}.{entry}"))
                .unwrap_or_else(|error| panic!("{fixture} benchmark.{fixture}.{entry}: {error}"));
        }
    }
}
