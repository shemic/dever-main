use dever_core::source::SourceMap;

#[path = "support/temp.rs"]
mod temp;

use temp::TemporaryDirectory;

fn sources(model: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("user/profile/model.dever", model);
    sources.add("user/profile/api.dever", "rest\n");
    sources
}

fn diagnostics(sources: &SourceMap) -> String {
    dever_core::check(sources)
        .expect_err("source must fail checking")
        .iter()
        .map(|error| error.render(sources))
        .collect()
}

// Code-generation assertions may inspect unbound HIR; an executable API must
// use the same configured site/auth contract as the CLI.
fn compile_authenticated_rest(mut sources: SourceMap) {
    sources.add("access/user/model.dever", "global type User { name: Text }");
    sources.add(
        "access/tenant/model.dever",
        "global type Tenant { name: Text }",
    );
    sources.add(
        "access/session/app.dever",
        r#"type Identity {
  id: Text
  user_id: access.user.model.id?
  tenant_id: access.tenant.model.id?
}
verify(claims: dever.auth.Claims) (identity: Identity) {
  identity = Identity { id = claims.subject
    user_id = null
    tenant_id = null }
}"#,
    );
    let directory = TemporaryDirectory::new();
    std::fs::create_dir(directory.path().join("config")).unwrap();
    std::fs::write(
        directory.path().join("config/setting.json"),
        r#"{
      "auth": {"providers": {"session": {
        "verify": "access.session.verify",
        "jwtSecret": "0123456789abcdef0123456789abcdef"
      }}},
      "sites": {"admin": {"path": "", "auth": "session"}},
      "database": {"default": {"type": "sqlite", "path": "data/rest.db"}}
    }"#,
    )
    .unwrap();
    let settings = dever_runtime::config::Settings::load_project(directory.path()).unwrap();
    let program = dever_core::check_with_settings(&sources, &settings).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let native = dever_core::native::compile_project(
        &program,
        &sources,
        std::ffi::OsStr::new("rustc"),
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(native.executable().is_file());
}

#[test]
fn rest_derives_the_public_model_without_an_app_wrapper() {
    let sources = sources(
        "type Profile {\n  name: Text(1, 64)\n  bio: Text?\n  score: Decimal(12, 2)?\n  uid: Uuid generated\n  private token: Text?\n}\n",
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
    assert!(
        program
            .model_snapshot()
            .contains("model user.profile.model")
    );
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(generated.contains("transaction.commit().await"));
    assert!(generated.contains("rest_rollback(transaction).await"));
    assert!(generated.contains("dever_runtime::api::text_bounds(\"name\""));
    compile_authenticated_rest(sources);
}

#[test]
fn rest_rejects_models_with_no_public_writable_field() {
    let sources = sources("type Profile {\n  private token: Text?\n}\n");
    assert!(diagnostics(&sources).contains("no writable public fields"));
}

#[test]
fn rest_rejects_private_required_fields_without_a_source() {
    let sources = sources("type Profile {\n  name: Text\n  private token: Text\n}\n");
    assert!(diagnostics(&sources).contains("private and required"));
}

#[test]
fn rest_rejects_fields_without_a_shared_wire_codec() {
    let sources = sources("type Profile {\n  name: Text\n  photo: Bytes\n}\n");
    assert!(diagnostics(&sources).contains("cannot be serialized"));
}

#[test]
fn rest_owner_is_server_bound_and_scopes_every_database_statement() {
    let mut sources = SourceMap::default();
    sources.add(
        "user/account/model.dever",
        "global type Account { email: Text(1, 254) unique }",
    );
    sources.add(
        "user/profile/model.dever",
        r#"type Profile {
  private owner user_id: user.account.model.id = dever.auth.user_id()
  name: Text(1, 64)
  bio: Text?
}
"#,
    );
    sources.add("user/profile/api.dever", "rest\n");
    let program = dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let snapshot = program.model_snapshot();
    assert!(snapshot.contains("owner=auth.user_id"), "{snapshot}");
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(
        generated.contains("dever_runtime::auth::user_id()"),
        "{generated}"
    );
    assert!(
        !generated.contains("inputs.raw_json(\"user_id\")"),
        "{generated}"
    );
    for sql in [
        "SELECT COUNT(*) FROM \\\"profile\\\" WHERE \\\"user_id\\\" = ?1",
        "WHERE \\\"id\\\" = ?1 AND \\\"user_id\\\" = ?2 LIMIT 1",
        "INSERT INTO \\\"profile\\\" (\\\"name\\\", \\\"bio\\\", \\\"user_id\\\")",
        "AND \\\"user_id\\\" = ?4 RETURNING",
        "DELETE FROM \\\"profile\\\" WHERE \\\"id\\\" = ?1 AND \\\"user_id\\\" = ?2",
    ] {
        assert!(generated.contains(sql), "missing {sql}: {generated}");
    }
    compile_authenticated_rest(sources);
}

#[test]
fn rest_owner_accepts_only_one_non_null_integer_identity_source() {
    for (model, expected) in [
        (
            "type Profile { owner user_id: Int? = dever.auth.user_id()\n name: Text }",
            "must be non-null",
        ),
        (
            "type Profile { owner user_id: Text = dever.auth.user_id()\n name: Text }",
            "must use Int or a Model id type",
        ),
        (
            "type Profile { owner user_id: Int default 1 = dever.auth.user_id()\n name: Text }",
            "cannot also be generated or declare a default",
        ),
        (
            "type Profile { owner user_id: Int = dever.api.request_id()\n name: Text }",
            "must be dever.auth.user_id() or dever.auth.tenant_id()",
        ),
        (
            "type Profile { owner user_id: Int = dever.auth.user_id()\n owner tenant_id: Int = dever.auth.tenant_id()\n name: Text }",
            "only one REST owner field",
        ),
    ] {
        let sources = sources(model);
        let rendered = diagnostics(&sources);
        assert!(
            rendered.contains(expected),
            "expected {expected}: {rendered}"
        );
    }
}

#[test]
fn rest_owner_syntax_formats_idempotently() {
    let mut sources = SourceMap::default();
    sources.add(
        "user/profile/model.dever",
        "type Profile {\nprivate owner user_id:Int=dever.auth.user_id()\nname:Text index\n}\n",
    );
    let source = &sources.files()[0];
    let formatted = dever_core::format::format(source).unwrap();
    assert!(
        formatted.contains("private owner user_id: Int = dever.auth.user_id()"),
        "{formatted}"
    );
    let mut roundtrip = SourceMap::default();
    roundtrip.add("user/profile/model.dever", formatted.clone());
    assert_eq!(
        dever_core::format::format(&roundtrip.files()[0]).unwrap(),
        formatted
    );
}

#[test]
fn authenticated_rest_uses_stable_operation_permissions() {
    let directory = TemporaryDirectory::new();
    std::fs::create_dir(directory.path().join("config")).unwrap();
    std::fs::write(
        directory.path().join("config/setting.json"),
        r#"{
          "auth": {"providers": {"session": {
            "verify": "user.profile.verify",
            "jwtSecret": "0123456789abcdef0123456789abcdef"
          }}},
          "sites": {"admin": {"path": "admin", "auth": "session"}}
        }"#,
    )
    .unwrap();
    let mut sources = SourceMap::default();
    sources.add(
        "platform/tenant/model.dever",
        "global type Tenant { name: Text }",
    );
    sources.add(
        "user/profile/model.dever",
        "global type Profile { name: Text }",
    );
    sources.add(
        "user/profile/app.dever",
        r#"type Identity {
  id: Text
  user_id: model.id?
  tenant_id: platform.tenant.model.id?
}

verify(claims: dever.auth.Claims) (identity: Identity) {
  identity = Identity {
    id = claims.subject
    user_id = null
    tenant_id = null
  }
}"#,
    );
    sources.add("user/profile/api/admin/manage.dever", "rest model");
    let settings = dever_runtime::config::Settings::load_project(directory.path()).unwrap();
    let program = dever_core::check_with_settings(&sources, &settings).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    for permission in [
        "user.profile.admin.read",
        "user.profile.admin.create",
        "user.profile.admin.replace",
        "user.profile.admin.delete",
    ] {
        assert!(
            generated.contains(&format!("api_authorize(\"{permission}\", \"admin\")")),
            "{generated}"
        );
    }
}

#[test]
fn rest_field_bindings_are_checked_and_emit_static_search_sql() {
    let mut sources = SourceMap::default();
    sources.add(
        "user/account/model.dever",
        "global type Account { email: Text(1, 254) unique }",
    );
    sources.add(
        "user/profile/domain.dever",
        "normalize(value: Text) (result: Text) pure { result = text.lower(text.trim(value)) }",
    );
    sources.add("user/profile/model.dever", r#"type Profile {
  private owner user_id: user.account.model.id = dever.auth.user_id()
  name: Text(1, 64) create = domain.normalize(input) replace = domain.normalize(input) search = domain.normalize(input)
  private source: Text? create = dever.api.header("x-source") replace = dever.api.header("x-source")
  private direct: Text? create = dever.system.api_header("x-direct") replace = dever.system.api_header("x-direct")
}"#);
    sources.add("user/profile/api.dever", "rest model\n");
    let program = dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    assert!(program.application_errors().unwrap().is_empty());
    assert!(
        program
            .model_snapshot()
            .contains("index plain (user_id,name) implicit=true"),
        "{}",
        program.model_snapshot()
    );
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(
        generated.contains("inputs.raw_json(\"name\")"),
        "{generated}"
    );
    assert!(
        !generated.contains("inputs.raw_json(\"source\")"),
        "{generated}"
    );
    assert!(
        generated.contains("inputs.optional_raw_json(\"name\")"),
        "{generated}"
    );
    assert!(
        generated.contains("WHERE \\\"user_id\\\" = ?1 AND \\\"name\\\" = ?2"),
        "{generated}"
    );
    assert!(
        generated.contains("SELECT COUNT(*) FROM \\\"profile\\\" WHERE \\\"user_id\\\" = ?1"),
        "{generated}"
    );
    compile_authenticated_rest(sources);
}

#[test]
fn rest_field_bindings_reject_unsafe_or_ambiguous_sources() {
    for (model, domain, expected) in [
        (
            "type Profile { private token: Text create = input\n name: Text }",
            "",
            "private REST create binding cannot use request input",
        ),
        (
            "type Profile { name: Text search = \"fixed\" }",
            "",
            "search binding must use input",
        ),
        (
            "type Profile { name: Text search = input\n status: Int search = input }",
            "",
            "at most one search field",
        ),
        (
            "type Profile { name: Text create = domain.normalize(input) }",
            "normalize(value: Text) (result: Text) { result = value }",
            "pure same-domain Domain",
        ),
        (
            "type Profile { created: DateTime create = dever.time.now() }",
            "",
            "pure same-domain Domain",
        ),
        (
            "type Profile { name: Text create = domain.mutate(input) }",
            "mutate(value: Text) (result: Text) pure {\n dever.api.set_header(\"x-test\", value)\n result = value\n}",
            "pure same-domain Domain",
        ),
        (
            "type Profile { count: Int create = model.count() }",
            "",
            "Model operations are private to their owning domain App",
        ),
    ] {
        let mut sources = sources(model);
        if !domain.is_empty() {
            sources.add("user/profile/domain.dever", domain);
        }
        let rendered = diagnostics(&sources);
        assert!(
            rendered.contains(expected),
            "expected {expected}: {rendered}"
        );
    }
}

#[test]
fn rest_binding_modifiers_do_not_change_app_model_operations() {
    let mut sources = SourceMap::default();
    sources.add(
        "user/profile/model.dever",
        "type Profile { name: Text create = text.trim(input) replace = text.lower(input) }",
    );
    sources.add(
        "user/profile/app.dever",
        "create(name: Text) (profile: model.Profile) { profile = model.create({ name = name }) }",
    );
    sources.add("user/profile/api.dever", "post create = app.create");
    let program = dever_core::check(&sources).unwrap();
    let native = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(!native.contains("text_trim"), "{native}");
    assert!(!native.contains("text_lower"), "{native}");
}

#[test]
fn rest_binding_syntax_formats_idempotently() {
    let mut sources = SourceMap::default();
    sources.add("user/profile/model.dever", "type Profile {\nname:Text create=text.trim(input) replace=text.lower(input) search=input\n}\n");
    let formatted = dever_core::format::format(&sources.files()[0]).unwrap();
    assert!(
        formatted.contains(
            "name: Text create = text.trim(input) replace = text.lower(input) search = input"
        ),
        "{formatted}"
    );
    let mut roundtrip = SourceMap::default();
    roundtrip.add("user/profile/model.dever", formatted.clone());
    assert_eq!(
        dever_core::format::format(&roundtrip.files()[0]).unwrap(),
        formatted
    );
}
