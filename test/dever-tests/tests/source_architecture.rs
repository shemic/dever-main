#[path = "support/temp.rs"]
mod temp;

use std::fs;

use dever_core::diagnostic::Diagnostic;
use dever_core::source::SourceMap;
use temp::TemporaryDirectory;

fn sources(files: &[(&str, &str)]) -> SourceMap {
    let mut sources = SourceMap::default();
    for (path, text) in files {
        sources.add(*path, *text);
    }
    sources
}

fn checked(files: &[(&str, &str)]) -> dever_core::hir::Program {
    let sources = sources(files);
    dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

fn rejected(files: &[(&str, &str)], code: &str, message: &str) -> Vec<Diagnostic> {
    let sources = sources(files);
    let errors = dever_core::check(&sources).expect_err("source must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.code == code && error.message.contains(message)),
        "expected {code}: {message}; got {errors:?}"
    );
    errors
}

#[test]
fn app_role_exports_domain_capabilities_without_exposing_topic_files() {
    let program = checked(&[
        (
            "main.dever",
            "main() (response: user.account.Greeting) { response = user.account.greet() }",
        ),
        (
            "user/account/app/commands.dever",
            "type Greeting { message: Text }\ngreet() (response: Greeting) { response = Greeting { message = int.to_text(count()) } }",
        ),
        (
            "user/account/app/queries.dever",
            "count() (response: Int) { response = 1 }",
        ),
    ]);
    let snapshot = program.api_snapshot();
    assert!(snapshot.contains("function user.account.greet"));
    assert!(snapshot.contains("function user.account.count"));
    assert!(snapshot.contains("type user.account.Greeting"));
    assert!(!snapshot.contains("commands"));
    assert!(!snapshot.contains("queries"));
    let warnings = program.application_warnings("main.main").unwrap();
    assert!(warnings.iter().any(|warning| {
        warning.code == "W007" && warning.message.contains("user.account.count")
    }));
}

#[test]
fn source_roles_replace_public_and_enforce_cohesive_layouts() {
    rejected(
        &[(
            "user/account/app.dever",
            "public greet() (response: Text) { response = \"hello\" }",
        )],
        "P001",
        "source role",
    );
    rejected(
        &[(
            "user/account/app/commands.dever",
            "greet() (response: Text) { response = \"hello\" }",
        )],
        "C003",
        "at least two cohesive topic files",
    );
    rejected(
        &[
            (
                "user/account/app.dever",
                "greet() (response: Text) { response = \"hello\" }",
            ),
            (
                "user/account/app/queries.dever",
                "count() (response: Int) { response = 1 }",
            ),
        ],
        "C003",
        "mutually exclusive",
    );
    rejected(
        &[(
            "user/common/app.dever",
            "greet() (response: Text) { response = \"hello\" }",
        )],
        "C003",
        "generic source bucket",
    );
}

#[test]
fn model_root_and_topic_keep_independent_persisted_identities() {
    let root = ("user/account/model.dever", "type Account { name: Text }");
    let topic = (
        "user/account/model/session.dever",
        "type Session { token: Text }",
    );
    let root_schema = checked(&[root]).model_snapshot();
    let topic_schema = checked(&[topic]).model_snapshot();
    let combined_schema = checked(&[root, topic]).model_snapshot();

    assert!(root_schema.contains("model user.account.model name=Account table=account "));
    assert!(topic_schema.contains("model user.account.model.session name=Session table=session "));
    assert_eq!(
        combined_schema,
        format!(
            "{root_schema}{}",
            topic_schema.strip_prefix("dever-model 1\n").unwrap()
        )
    );
}

#[test]
fn api_paths_allow_one_topic_with_or_without_a_root_file() {
    // Empty declarations isolate layout checks from API declaration syntax.
    let root = ("user/account/api.dever", "");
    let topic = ("user/account/api/session/history.dever", "");
    checked(&[topic]);
    checked(&[root, topic]);
}

#[test]
fn business_role_topics_still_require_cohesive_exclusive_directories() {
    for role in ["app", "domain", "port", "adapter", "job"] {
        let root = format!("user/account/{role}.dever");
        let topic = format!("user/account/{role}/registration.dever");
        rejected(&[(&topic, "")], "C003", "at least two cohesive topic files");
        let errors = rejected(&[(&root, ""), (&topic, "")], "C003", "mutually exclusive");
        let conflict = errors
            .iter()
            .find(|error| error.message.contains("mutually exclusive"))
            .unwrap();
        assert_eq!(conflict.related.len(), 1);
        assert_eq!(conflict.related[0].message, "role file declared here");
    }
}

#[test]
fn model_and_api_exceptions_preserve_topic_name_and_depth_checks() {
    rejected(
        &[("user/account/model/session/history.dever", "")],
        "C003",
        "topic directories are flat",
    );
    for role in ["model", "api"] {
        let topic = format!("user/account/{role}/helpers.dever");
        rejected(&[(&topic, "")], "C003", "generic source bucket");
    }
}

#[test]
fn strict_project_loading_rejects_the_removed_service_layout() {
    checked(&[
        ("main.dever", "main() () {}"),
        (
            "user/service/greeting.dever",
            "greet() (response: Text) { response = \"hello\" }",
        ),
    ]);
    let root = TemporaryDirectory::new();
    fs::create_dir_all(root.path().join("user/service")).unwrap();
    fs::write(root.path().join("main.dever"), "main() () {}\n").unwrap();
    fs::write(
        root.path().join("user/service/greeting.dever"),
        "greet() (response: Text) { response = \"hello\" }\n",
    )
    .unwrap();
    let sources = SourceMap::load(root.path()).unwrap();
    let errors = dever_core::check(&sources).unwrap_err();
    assert!(errors.iter().any(|error| {
        error.code == "C003" && error.message.contains("application source must be")
    }));
}

#[test]
fn only_app_crosses_domain_boundaries() {
    checked(&[
        (
            "user/account/domain.dever",
            "normalize(value: Text) (response: Text) { response = text.trim(value) }",
        ),
        (
            "user/account/app.dever",
            "normalize(value: Text) (response: Text) { response = domain.normalize(value) }",
        ),
        (
            "news/article/app.dever",
            "author(value: Text) (response: Text) { response = user.account.normalize(value) }",
        ),
        (
            "user/profile/app.dever",
            "label(value: Text) (response: Text) { response = account.normalize(value) }",
        ),
        (
            "main.dever",
            "main() (response: Text) { response = news.article.author(\" editor \" ) }",
        ),
    ]);
    rejected(
        &[
            (
                "user/account/domain.dever",
                "normalize(value: Text) (response: Text) { response = text.trim(value) }",
            ),
            (
                "news/article/app.dever",
                "steal(value: Text) (response: Text) { response = user.account.domain.normalize(value) }",
            ),
        ],
        "C006",
        "not callable from this source role",
    );
}

#[test]
fn app_failures_must_use_app_owned_error_types() {
    rejected(
        &[
            (
                "user/account/domain.dever",
                "type RegistrationError { error InvalidName }\nreject() () { fail(RegistrationError.InvalidName) }",
            ),
            (
                "user/account/app.dever",
                "register() () { domain.reject() }",
            ),
        ],
        "C006",
        "cannot expose private error type",
    );
    checked(&[
        (
            "user/account/app.dever",
            "type RegistrationError { error InvalidName }\nregister() () { domain.reject() }",
        ),
        (
            "user/account/domain.dever",
            "reject() () { fail(app.RegistrationError.InvalidName) }",
        ),
    ]);
}

#[test]
fn model_operations_belong_to_the_owning_domain_app() {
    checked(&[
        ("user/account/model.dever", "type Account { name: Text }"),
        (
            "user/account/app.dever",
            "count() (total: Int) { total = model.count() }",
        ),
        (
            "main.dever",
            "main() (total: Int) { total = user.account.count() }",
        ),
    ]);
    rejected(
        &[
            ("user/account/model.dever", "type Account { name: Text }"),
            (
                "main.dever",
                "main() (total: Int) { total = user.account.model.count() }",
            ),
        ],
        "C006",
        "Model operations are private",
    );
}

#[test]
fn api_is_an_http_entry_and_not_a_source_capability() {
    let sources = sources(&[
        (
            "user/account/app.dever",
            "greet(name: Text) (response: Text) { response = \"hello, \" + name }",
        ),
        ("user/account/api.dever", "get greeting = app.greet"),
    ]);
    let program = dever_core::check(&sources).unwrap();
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile::default(),
    )
    .unwrap();
    assert!(generated.contains("(\"GET\", \"/user/account/greeting\")"));

    rejected(
        &[
            ("user/account/api.dever", "get greeting = app.greet"),
            (
                "user/account/app.dever",
                "greet() (response: Text) { response = \"hello\" }",
            ),
            (
                "news/article/app.dever",
                "steal() (response: Text) { response = user.account.api.greeting() }",
            ),
        ],
        "C004",
        "unknown",
    );
}

#[test]
fn main_contains_only_the_boot_function_and_port_rejects_business_bodies() {
    let root = TemporaryDirectory::new();
    fs::write(
        root.path().join("main.dever"),
        "main() () {}\nhelper() () {}\n",
    )
    .unwrap();
    let sources = SourceMap::load(root.path()).unwrap();
    let errors = dever_core::check(&sources).unwrap_err();
    assert!(
        errors.iter().any(|error| {
            error.code == "C005" && error.message.contains("only the ordinary main")
        })
    );
    rejected(
        &[(
            "user/account/port.dever",
            "load() (response: Text) { response = \"value\" }",
        )],
        "C005",
        "bodyless",
    );
}
