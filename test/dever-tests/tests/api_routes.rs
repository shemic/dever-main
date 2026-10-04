#[path = "support/temp.rs"]
mod temp;

use std::fs;

use dever_core::source::SourceMap;
use dever_runtime::api::{self, Inputs};
use dever_runtime::bytes::Bytes;
use dever_runtime::config::Settings;
use dever_runtime::http::{Header, Request};
use temp::TemporaryDirectory;

fn sources(main: &str, api_source: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("main.dever", main);
    sources.add("user/api/hello.dever", api_source);
    sources.add(
        "user/service/greeting.dever",
        "public type Greeting { message: Text }\n\
         public greet(name: Text) (greeting: Greeting) {\n\
           greeting = Greeting { message = \"hello, \" + text.trim(name) }\n\
         }\n",
    );
    sources
}

fn diagnostics(sources: &SourceMap) -> String {
    dever_core::check(sources)
        .expect_err("source must be rejected")
        .iter()
        .map(|error| error.render(sources))
        .collect()
}

#[test]
fn explicit_serve_compiles_recursive_module_routes_and_json_adapter() {
    let mut sources = sources(
        "main() () { dever.api.serve() }",
        "get_greeting(name: Text) (response: user.service.greeting.Greeting) {\n\
           response = user.service.greeting.greet(name)\n\
         }\n\
         post_greeting(name: Text) (response: user.service.greeting.Greeting) {\n\
           response = user.service.greeting.greet(name)\n\
         }",
    );
    sources.add(
        "user/api/admin/health.dever",
        "get_status() (response: Text) { response = \"ready\" }",
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
    let generated = dever_core::native::emit(&program, &sources, "main.main").unwrap();
    assert!(generated.contains("(\"GET\", \"/user/hello/greeting\")"));
    assert!(generated.contains("(\"POST\", \"/user/hello/greeting\")"));
    assert!(generated.contains("(\"GET\", \"/user/admin/health/status\")"));
    assert!(generated.contains("dever_runtime::api::serve(api_route).await"));
    assert!(generated.contains("inputs.finish()"));
    assert!(generated.contains("dever_runtime::api::success(value)"));
}

#[test]
fn generated_api_application_compiles_without_starting_a_listener() {
    let sources = sources(
        "main() () { dever.api.serve() }",
        "get_greeting(name: Text) (response: user.service.greeting.Greeting) {\n\
           response = user.service.greeting.greet(name)\n\
         }\n\
         post_greeting(name: Text) (response: user.service.greeting.Greeting) {\n\
           response = user.service.greeting.greet(name)\n\
         }\n\
         delete_greeting(name: Text) (response: Text) { response = name }",
    );
    let program = dever_core::check(&sources).unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let native = dever_core::native::compile_application(
        &program,
        &sources,
        "main.main",
        &rustc,
        dever_runtime::config::RuntimeProfile::default(),
    )
    .unwrap();
    assert!(native.executable().is_file());
}

#[test]
fn without_serve_api_sources_are_checked_but_not_emitted() {
    let checked_sources = sources(
        "main() (answer: Int) { answer = 42 }",
        "get_greeting(name: Text) (response: user.service.greeting.Greeting) {\n\
           response = user.service.greeting.greet(name)\n\
         }",
    );
    let program = dever_core::check(&checked_sources).unwrap();
    let generated = dever_core::native::emit(&program, &checked_sources, "main.main").unwrap();
    assert!(!generated.contains("fn api_route("));
    assert!(!generated.contains("dever_runtime::api::serve"));
    let invalid = sources(
        "main() () {}",
        "fetch_greeting() (response: Text) { response = \"x\" }",
    );
    assert!(diagnostics(&invalid).contains("API handler names must start with"));
}

#[test]
fn api_only_accepts_private_http_handlers_with_response_output() {
    for (source, expected) in [
        (
            "public get_ok() (response: Text) { response = \"x\" }",
            "private ordinary",
        ),
        (
            "post_create() (result: Text) { result = \"x\" }",
            "named response",
        ),
        (
            "delete_thing(file: dever.system.File) (response: Text) { response = \"x\" }",
            "API inputs must",
        ),
        (
            "get_limit(value: Int >= 1) (response: Int) { response = value }",
            "API inputs must be plain",
        ),
        (
            "type Temporary { value: Text }",
            "only HTTP handler functions",
        ),
    ] {
        let errors = diagnostics(&sources("main() () {}", source));
        assert!(errors.contains(expected), "{source}: {errors}");
    }
}

#[test]
fn query_and_json_body_bind_the_same_typed_parameters() {
    let request = Request {
        method: "GET".into(),
        target: "/user/hello/greeting?name=%E4%B8%AD%E6%96%87&count=12&active=true".into(),
        headers: vec![],
        body: Bytes::from_text(""),
    };
    let mut inputs = Inputs::from_request(&request).unwrap();
    assert_eq!(inputs.text("name").unwrap(), "中文");
    assert_eq!(inputs.int("count").unwrap(), 12);
    assert!(inputs.boolean("active").unwrap());
    inputs.finish().unwrap();

    let request = Request {
        method: "POST".into(),
        target: "/user/hello/greeting".into(),
        headers: vec![Header {
            name: "content-type".into(),
            value: Bytes::from_text("application/json"),
        }],
        body: Bytes::from_text("{\"name\":\"Ada\",\"count\":2,\"active\":false}"),
    };
    let mut inputs = Inputs::from_request(&request).unwrap();
    assert_eq!(inputs.text("name").unwrap(), "Ada");
    assert_eq!(inputs.int("count").unwrap(), 2);
    assert!(!inputs.boolean("active").unwrap());
    inputs.finish().unwrap();
}

#[test]
fn malformed_or_extra_inputs_get_structured_errors() {
    for target in [
        "/user/hello?name=x&name=y",
        "/user/hello?name=%GG",
        "/user/hello?name=x&extra=1",
    ] {
        let request = Request {
            method: "GET".into(),
            target: target.into(),
            headers: vec![],
            body: Bytes::from_text(""),
        };
        let parsed = Inputs::from_request(&request);
        if let Ok(mut inputs) = parsed {
            inputs.text("name").unwrap();
            assert!(inputs.finish().is_err());
        }
    }
    let response = api::invalid_input(api::InputError("bad value".into()));
    assert_eq!(response.status, 400);
    let body: api::Value = serde_json::from_slice(response.body.values()).unwrap();
    assert_eq!(body["code"], 400);
    assert_eq!(body["message"], "bad value");
    assert!(body["data"].is_null());

    for (status, message) in [
        (400, "invalid request"),
        (401, "unauthorized"),
        (403, "forbidden"),
        (404, "not found"),
        (409, "conflict"),
        (429, "too many requests"),
    ] {
        let response = api::standard_error(status);
        assert_eq!(response.status, status);
        let body: api::Value = serde_json::from_slice(response.body.values()).unwrap();
        assert_eq!(body["code"], status);
        assert_eq!(body["message"], message);
        assert!(body["data"].is_null());
    }
}

#[test]
fn listener_settings_are_required_only_for_api_and_come_from_project_config() {
    let directory = TemporaryDirectory::new();
    fs::create_dir(directory.path().join("config")).unwrap();
    let setting = directory.path().join("config/setting.json");
    fs::write(&setting, "{}\n").unwrap();
    let settings = Settings::load_project(directory.path()).unwrap();
    assert!(
        settings
            .http()
            .unwrap_err()
            .contains("http.host and http.port")
    );

    fs::write(&setting, r#"{"http":{"host":"127.0.0.1","port":8181}}"#).unwrap();
    let settings = Settings::load_project(directory.path()).unwrap();
    let http = settings.http().unwrap();
    assert_eq!(http.host, "127.0.0.1");
    assert_eq!(http.port, 8181);

    fs::write(&setting, r#"{"http":{"host":"example.com","port":8181}}"#).unwrap();
    assert!(
        Settings::load_project(directory.path())
            .unwrap()
            .http()
            .is_err()
    );
}
