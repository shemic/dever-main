#[path = "support/temp.rs"]
mod temp;

use std::fs;

use dever_core::source::SourceMap;
use dever_runtime::config::Settings;

fn sources(files: &[(&str, &str)]) -> SourceMap {
    let mut sources = SourceMap::default();
    for (path, source) in files {
        sources.add(*path, *source);
    }
    sources
}

fn check(files: &[(&str, &str)]) {
    let sources = sources(files);
    dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
}

fn reject(files: &[(&str, &str)], expected: &str) {
    let sources = sources(files);
    let errors = dever_core::check(&sources).expect_err("source must be rejected");
    let rendered = errors
        .iter()
        .map(|error| error.render(&sources))
        .collect::<String>();
    assert!(
        rendered.contains(expected),
        "expected {expected}: {rendered}"
    );
}

fn auth_settings() -> (temp::TemporaryDirectory, Settings) {
    let root = temp::TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/setting.json"),
        r#"{
          "auth": {
            "providers": {
              "session": {
                "verify": "user.account.verify",
                "jwtSecret": "0123456789abcdef0123456789abcdef"
              }
            }
          },
          "sites": {
            "admin": {"path": "admin", "auth": "session"}
          },
          "database": {
            "default": {
              "type": "sqlite",
              "path": "data/control.db",
              "tenant_directory": "data/tenants"
            }
          },
          "tenant": {
            "database": "default"
          }
        }"#,
    )
    .unwrap();
    let settings = Settings::load_project(root.path()).unwrap();
    (root, settings)
}

fn multi_provider_settings() -> (temp::TemporaryDirectory, Settings) {
    let (root, _) = auth_settings();
    let setting_path = root.path().join("config/setting.json");
    let mut settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&setting_path).unwrap()).unwrap();
    settings["auth"]["providers"]["other"] = serde_json::json!({
        "verify": "admin.principal.verify",
        "jwtSecret": "0123456789abcdef0123456789abcdef",
    });
    settings["sites"]["other"] = serde_json::json!({"path": "other", "auth": "other"});
    fs::write(&setting_path, settings.to_string()).unwrap();
    let settings = Settings::load_project(root.path()).unwrap();
    (root, settings)
}

fn identity_sources() -> [(&'static str, &'static str); 3] {
    [
        (
            "user/account/model.dever",
            "global type Account { email: Text(1, 254) }",
        ),
        (
            "platform/tenant/model.dever",
            "global type Tenant { name: Text(1, 64) }",
        ),
        (
            "user/account/app.dever",
            r#"type Identity {
  id: Text
  user_id: user.account.model.id?
  tenant_id: platform.tenant.model.id?
}
verify(claims: dever.auth.Claims) (identity: Identity) {
  identity = Identity {
    id = claims.subject
    user_id = null
    tenant_id = null
  }
}
profile() (response: Text) {
  response = dever.site.key()
}"#,
        ),
    ]
}

fn configured_check(
    files: &[(&str, &str)],
    settings: &Settings,
) -> Result<dever_core::hir::Program, String> {
    let sources = sources(files);
    dever_core::check_with_settings(&sources, settings).map_err(|errors| {
        errors
            .iter()
            .map(|error| error.render(&sources))
            .collect::<String>()
    })
}

#[test]
fn api_bindings_inherit_app_signatures_without_response_name_requirement() {
    check(&[
        (
            "user/account/app.dever",
            "greet(name: Text) (greeting: Text) { greeting = text.trim(name) }",
        ),
        (
            "user/account/api.dever",
            "get greeting = app.greet\npost greeting = app.greet\nput greeting = app.greet\ndelete greeting = app.greet\ncmd greeting = app.greet",
        ),
    ]);
}

#[test]
fn configured_sites_bind_nested_api_directories_to_checked_auth_hooks() {
    let (_root, settings) = auth_settings();
    let mut files = identity_sources().to_vec();
    files.push((
        "user/account/api/admin/profile.dever",
        "public get profile = app.profile",
    ));

    let source_map = sources(&files);
    let program =
        dever_core::check_with_settings(&source_map, &settings).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&source_map))
                    .collect::<String>()
            )
        });
    let generated = dever_core::native::emit_project(
        &program,
        &source_map,
        dever_runtime::config::RuntimeProfile::default(),
    )
    .unwrap();
    assert!(generated.contains("dever_runtime::auth::prepare(&[\"admin\"]"));
    assert!(generated.contains("dever_runtime::auth::scope(auth_prepared, auth_identity"));
}

#[test]
fn configured_sites_reject_unmapped_api_directories_and_invalid_verify_records() {
    let (_root, settings) = auth_settings();
    let mut unmapped = identity_sources().to_vec();
    unmapped.push((
        "user/account/api/front/profile.dever",
        "get profile = app.profile",
    ));
    let error = configured_check(&unmapped, &settings).unwrap_err();
    assert!(
        error.contains("not assigned to a configured site"),
        "{error}"
    );

    let invalid_app = identity_sources()[2]
        .1
        .replace("tenant_id: platform.tenant.model.id?", "tenant_id: Int?");
    let mut invalid = identity_sources().to_vec();
    invalid[2] = ("user/account/app.dever", &invalid_app);
    invalid.push((
        "user/account/api/admin/profile.dever",
        "get profile = app.profile",
    ));
    let error = configured_check(&invalid, &settings).unwrap_err();
    assert!(
        error.contains("identity must be an App-owned public record"),
        "{error}"
    );
}

#[test]
fn verify_is_read_only() {
    let (_root, settings) = auth_settings();
    let writing_verify = identity_sources()[2].1.replace(
        "identity = Identity {",
        "account = model.create({ email = claims.subject })\n  identity = Identity {",
    );
    let mut writing = identity_sources().to_vec();
    writing[2] = ("user/account/app.dever", &writing_verify);
    writing.push((
        "user/account/api/admin/profile.dever",
        "get profile = app.profile",
    ));
    let error = configured_check(&writing, &settings).unwrap_err();
    assert!(
        error.contains("authentication verify cannot write"),
        "{error}"
    );
}

#[test]
fn trusted_identity_getters_preserve_nominal_ids_and_nullable_representation() {
    let (_root, settings) = auth_settings();
    let mut files = identity_sources().to_vec();
    files.extend([
        (
            "news/reader/app.dever",
            r#"
type Profile {
  user: user.account.model.id?
  tenant: platform.tenant.model.id?
  raw: Int?
}
profile() (profile: Profile) {
  profile = Profile {
    user = dever.auth.user_id()
    tenant = dever.auth.tenant_id()
    raw = dever.auth.user_id()
  }
}
current_user() (user: user.account.model.id) {
  user = require_user(dever.auth.user_id())
}
require_user(value: user.account.model.id) (user: user.account.model.id) {
  user = value
}
require_user(value: null) (user: user.account.model.id) {
  fail(dever.api.Error.Unauthorized)
}
owns(id: user.account.model.id) (owned: Bool) {
  owned = dever.auth.owns_user(id)
}
"#,
        ),
        (
            "news/reader/api/admin/profile.dever",
            "get profile = app.profile\nget current = app.current_user\nget owns = app.owns",
        ),
    ]);
    let source_map = sources(&files);
    dever_core::check(&source_map).expect("source-only checking defers provider binding");
    let program =
        dever_core::check_with_settings(&source_map, &settings).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&source_map))
                    .collect::<String>()
            )
        });
    let native = dever_core::native::emit_project(
        &program,
        &source_map,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(native.contains("dever_runtime::auth::user_id().map(|value| value.map(T"));
    assert!(native.contains("dever_runtime::auth::tenant_id().map(|value| value.map(T"));
    let llvm = dever_core::llvm::emit_application(&program, &source_map).unwrap();
    assert!(llvm.contains("@dever_rt_v1_auth_user_id"));
    assert!(llvm.contains("@dever_rt_v1_auth_tenant_id"));
}

#[test]
fn trusted_identity_getters_reject_wrong_ids_and_implicit_casts() {
    let (_root, settings) = auth_settings();
    for (app, message) in [
        (
            "profile() (value: platform.tenant.model.id?) { value = dever.auth.user_id() }",
            "nominal ID must match provider",
        ),
        (
            "profile() (value: user.account.model.id?) { value = dever.auth.tenant_id() }",
            "nominal ID must match provider",
        ),
        (
            "profile(id: platform.tenant.model.id) (owned: Bool) { owned = dever.auth.owns_user(id) }",
            "nominal ID must match provider",
        ),
        (
            "profile() (value: user.account.model.id) { value = dever.auth.user_id() }",
            "C005",
        ),
        (
            "profile(raw: Int?) (value: user.account.model.id?) { value = raw }",
            "C005",
        ),
        (
            "profile() (value: user.account.model.id?) { value = 1 }",
            "C005",
        ),
        (
            "profile() (value: user.account.model.id?) { raw = dever.auth.user_id()\nvalue = raw }",
            "C005",
        ),
    ] {
        let mut files = identity_sources().to_vec();
        files.extend([
            ("news/reader/app.dever", app),
            (
                "news/reader/api/admin/profile.dever",
                "get profile = app.profile",
            ),
        ]);
        let error = configured_check(&files, &settings).unwrap_err();
        assert!(error.contains(message), "{app}: {error}");
    }
}

#[test]
fn trusted_identity_getters_require_matching_providers_at_every_route() {
    let (_root, settings) = multi_provider_settings();
    let other_provider = identity_sources()[2]
        .1
        .replace("user.account.model.id?", "admin.principal.model.id?");
    for app in [
        "profile() (value: user.account.model.id?) { value = dever.auth.user_id() }",
        "profile(id: user.account.model.id) (owned: Bool) { owned = dever.auth.owns_user(id) }",
    ] {
        let mut files = identity_sources().to_vec();
        files.extend([
            (
                "admin/principal/model.dever",
                "global type Principal { name: Text }",
            ),
            ("admin/principal/app.dever", other_provider.as_str()),
            ("news/reader/app.dever", app),
            (
                "news/reader/api/admin/profile.dever",
                "get profile = app.profile",
            ),
            (
                "news/reader/api/other/profile.dever",
                "get profile = app.profile",
            ),
        ]);
        let error = configured_check(&files, &settings).unwrap_err();
        assert!(
            error.contains("C006") && error.contains("provider 'other'"),
            "{error}"
        );
    }
}

#[test]
fn rest_identity_bindings_preserve_matching_nominal_ids() {
    let (_root, settings) = auth_settings();
    let mut files = identity_sources().to_vec();
    files.extend([
        (
            "news/article/model.dever",
            r#"type Article {
  private owner author: user.account.model.id = dever.auth.user_id()
  title: Text
  linked_user: user.account.model.id? create = dever.auth.user_id() replace = dever.auth.user_id()
  linked_tenant: platform.tenant.model.id? create = dever.auth.tenant_id() replace = dever.auth.tenant_id()
}"#,
        ),
        ("news/article/api/admin/manage.dever", "rest model"),
    ]);
    check(&files);
    let program = configured_check(&files, &settings).unwrap();
    let source_map = sources(&files);
    let native = dever_core::native::emit_project(
        &program,
        &source_map,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(native.contains("dever_runtime::auth::user_id().map(|value| value.map(T"));
    assert!(native.contains("dever_runtime::auth::tenant_id().map(|value| value.map(T"));
    let llvm = dever_core::llvm::emit_application(&program, &source_map).unwrap();
    assert!(llvm.contains("@dever_rt_v1_auth_user_id"));
    assert!(llvm.contains("@dever_rt_v1_auth_tenant_id"));
}

#[test]
fn rest_identity_bindings_reject_wrong_provider_ids() {
    let (_root, settings) = auth_settings();
    for field in [
        "linked: platform.tenant.model.id? create = dever.auth.user_id()",
        "linked: platform.tenant.model.id? replace = dever.auth.user_id()",
        "linked: user.account.model.id? create = dever.auth.tenant_id()",
        "linked: user.account.model.id? replace = dever.auth.tenant_id()",
        "private owner linked: platform.tenant.model.id = dever.auth.user_id()",
        "private owner linked: user.account.model.id = dever.auth.tenant_id()",
    ] {
        let model = format!("type Article {{\n title: Text\n {field}\n}}");
        let mut files = identity_sources().to_vec();
        files.extend([
            ("news/article/model.dever", model.as_str()),
            ("news/article/api/admin/manage.dever", "rest model"),
        ]);
        check(&files);
        let error = configured_check(&files, &settings).unwrap_err();
        assert!(
            error.contains("C006") && error.contains("provider 'session'"),
            "{field}: {error}"
        );
    }
}

#[test]
fn rest_identity_bindings_match_each_sites_provider() {
    let (_root, settings) = multi_provider_settings();
    let other_provider = identity_sources()[2]
        .1
        .replace("user.account.model.id?", "admin.principal.model.id?");
    for field in [
        "linked: user.account.model.id? create = dever.auth.user_id()",
        "linked: user.account.model.id? replace = dever.auth.user_id()",
        "private owner linked: user.account.model.id = dever.auth.user_id()",
    ] {
        let model = format!("type Article {{\n title: Text\n {field}\n}}");
        let mut files = identity_sources().to_vec();
        files.extend([
            (
                "admin/principal/model.dever",
                "global type Principal { name: Text }",
            ),
            ("admin/principal/app.dever", other_provider.as_str()),
            ("news/article/model.dever", model.as_str()),
            ("news/article/api/admin/manage.dever", "rest model"),
        ]);
        configured_check(&files, &settings).expect("first site's provider matches");
        files.push(("news/article/api/other/manage.dever", "rest model"));
        let error = configured_check(&files, &settings).unwrap_err();
        assert!(
            error.contains("C006") && error.contains("provider 'other'"),
            "{field}: {error}"
        );
    }
}

#[test]
fn nominal_identity_getters_do_not_gain_public_cmd_or_job_context() {
    let (_root, settings) = auth_settings();
    let app = "profile() (value: user.account.model.id?) { value = dever.auth.user_id() }";
    for (path, declaration, message) in [
        (
            "news/reader/api/admin/profile.dever",
            "public get profile = app.profile",
            "requires a protected API",
        ),
        (
            "news/reader/api.dever",
            "cmd profile = app.profile",
            "not available from CMD or Job",
        ),
        (
            "news/reader/job.dever",
            "database default\njob sample() () retry(3) timeout(1000) { value = app.profile() }",
            "not available from CMD or Job",
        ),
    ] {
        let mut files = identity_sources().to_vec();
        files.extend([("news/reader/app.dever", app), (path, declaration)]);
        let error = configured_check(&files, &settings).unwrap_err();
        assert!(error.contains("C006") && error.contains(message), "{error}");
    }
}

#[test]
fn nominal_identity_getters_remain_unavailable_to_ordinary_tests() {
    let (_root, settings) = auth_settings();
    let mut files = identity_sources().to_vec();
    files.push((
        "news/reader/app.dever",
        "profile() (value: user.account.model.id?) { value = dever.auth.user_id() }",
    ));
    let mut inputs = sources(&files).compilation_sources().unwrap();
    inputs.push(dever_core::source::CompilationSource {
        logical_path: "news/reader/probe.dever".into(),
        is_test: true,
        text: "probe() () { value = app.profile() }".into(),
    });
    let source_map = SourceMap::from_compilation_sources(&inputs).unwrap();
    let errors = dever_core::check_with_settings(&source_map, &settings).unwrap_err();
    let rendered = errors
        .iter()
        .map(|error| error.render(&source_map))
        .collect::<String>();
    assert!(
        rendered.contains("C006") && rendered.contains("not available from tests"),
        "{rendered}"
    );
}

#[test]
fn protected_api_permissions_are_stable_and_unique_across_paths_and_methods() {
    let (_root, settings) = auth_settings();
    let mut files = identity_sources().to_vec();
    files.push((
        "user/account/api/admin/profile.dever",
        "get profile = app.profile",
    ));
    let program = configured_check(&files, &settings).unwrap();
    assert!(program.api_snapshot().contains(
        "permission user.account.admin.profile method=GET component=user domain=account site=admin action=profile"
    ));

    files.push((
        "user/account/api/admin/alternate.dever",
        "post profile = app.profile",
    ));
    let error = configured_check(&files, &settings).unwrap_err();
    assert!(
        error.contains("duplicate API permission 'user.account.admin.profile'"),
        "{error}"
    );
}

#[test]
fn public_visibility_and_identity_stay_on_http_app_bindings() {
    reject(
        &[
            (
                "user/account/app.dever",
                "profile() (response: Text) { response = \"ok\" }",
            ),
            (
                "user/account/api/admin/profile.dever",
                "public cmd profile = app.profile",
            ),
        ],
        "application visibility is defined by the source role",
    );
    reject(
        &[(
            "user/account/domain.dever",
            "profile() (response: Text) { response = dever.auth.id() }",
        )],
        "available only from App functions",
    );
    reject(
        &[
            (
                "user/account/app.dever",
                "manage() (ok: Bool) { dever.auth.disable_role(\"reader\")\nok = true }",
            ),
            (
                "user/account/api/admin/manage.dever",
                "public post manage = app.manage",
            ),
        ],
        "public API call chains cannot administer authorization",
    );
}

#[test]
fn public_api_cannot_reach_a_tenant_component() {
    let (_root, settings) = auth_settings();
    let mut files = identity_sources().to_vec();
    files.extend([
        (
            "news/article/model.dever",
            "type Article { title: Text(1, 160) }",
        ),
        (
            "news/article/app.dever",
            "count() (value: Int) { value = model.count() }",
        ),
        (
            "news/article/api/admin/browse.dever",
            "public get count = app.count",
        ),
    ]);
    let error = configured_check(&files, &settings).unwrap_err();
    assert!(
        error.contains("public API cannot reach tenant components [news]"),
        "{error}"
    );
}

#[test]
fn json_bindings_accept_wire_records_but_query_bindings_require_scalars() {
    let app = "type Input { name: Text }\nupdate(input: Input) (value: Text) { value = text.trim(input.name) }";
    check(&[
        ("user/account/app.dever", app),
        (
            "user/account/api.dever",
            "put profile = app.update\ncmd update = app.update",
        ),
    ]);
    reject(
        &[
            ("user/account/app.dever", app),
            ("user/account/api.dever", "get profile = app.update"),
        ],
        "wire-safe scalars",
    );
}

#[test]
fn api_bindings_reject_legacy_bodies_cross_domain_and_ambiguous_targets() {
    reject(
        &[(
            "user/account/api.dever",
            "get_name() (response: Text) { response = \"x\" }",
        )],
        "handler function bodies and legacy prefixes",
    );
    reject(
        &[("user/account/api.dever", "post save = user.profile.save")],
        "same domain",
    );
    reject(
        &[("user/account/api.dever", "post save = app.missing")],
        "unambiguous same-domain",
    );
    reject(
        &[
            (
                "user/account/app.dever",
                "save() (value: Text) { value = \"x\" }\nsave(name: Text) (value: Text) { value = text.trim(name) }",
            ),
            ("user/account/api.dever", "post save = app.save"),
        ],
        "unambiguous same-domain",
    );
}

#[test]
fn duplicate_routes_and_commands_are_rejected() {
    let app = "greet() (value: Text) { value = \"hello\" }";
    for (declaration, diagnostic) in [
        ("get hello = app.greet", "duplicate HTTP route"),
        ("cmd hello = app.greet", "duplicate command"),
    ] {
        let api = format!("{declaration}\n{declaration}");
        reject(
            &[
                ("user/account/app.dever", app),
                ("user/account/api.dever", &api),
            ],
            diagnostic,
        );
    }
    reject(
        &[
            ("user/account/app.dever", app),
            ("user/account/api.dever", "cmd hello = app.greet"),
            (
                "user/account/api/maintenance.dever",
                "cmd hello = app.greet",
            ),
        ],
        "duplicate command",
    );
}

fn sql_projection_files(sqlite: &str, postgres: &str, method: &str) -> [(&'static str, String); 3] {
    [
        (
            "news/article/model.dever",
            format!(
                "type Article {{\n  name: Text\n  private draft: Bool default true\n}}\n\
                 sql lookup(name: Text, size: Int) (rows: List<app.View>) {{\n\
                   sqlite = {sqlite:?}\n  postgres = {postgres:?}\n}}\n"
            ),
        ),
        (
            "news/article/app.dever",
            "type View { name: Text }\n\
             read(name: Text, size: Int) (rows: List<View>) {\n\
               rows = model.lookup(name, size)\n}\n"
                .into(),
        ),
        (
            "news/article/api.dever",
            format!("{method} read = app.read\n"),
        ),
    ]
}

#[test]
fn get_accepts_only_proven_single_model_sql_projections() {
    for sqlite in [
        "SELECT name FROM article WHERE name = ?1 ORDER BY id DESC LIMIT ?2",
        "select \"name\" from \"article\" where \"name\" = ?1 AND name = ?1 order by id desc, name asc limit ?2;",
    ] {
        let postgres = sqlite.replace('?', "$");
        let files = sql_projection_files(sqlite, &postgres, "get");
        check(
            &files
                .iter()
                .map(|(path, source)| (*path, source.as_str()))
                .collect::<Vec<_>>(),
        );
    }
}

#[test]
fn unproven_sql_keeps_its_write_effect_in_each_dialect() {
    let plain = "SELECT name FROM article WHERE name = ?1 ORDER BY id DESC LIMIT ?2";
    for unproven in [
        "SELECT name FROM other_article WHERE name = ?1 ORDER BY id DESC LIMIT ?2",
        "SELECT missing FROM article WHERE name = ?1 ORDER BY id DESC LIMIT ?2",
        "SELECT lower(name) FROM article WHERE name = ?1 ORDER BY id DESC LIMIT ?2",
        "WITH changed AS (UPDATE article SET name = ?1 WHERE id = ?2 RETURNING name) SELECT name FROM changed",
        "SELECT name FROM article WHERE name = ?1 LIMIT ?2; DELETE FROM article",
        "UPDATE article SET name = ?1 WHERE id = ?2 RETURNING name",
        "SELECT name INTO copied_article FROM article WHERE name = ?1 LIMIT ?2",
        "SELECT name FROM article WHERE name = ?1 LIMIT ?2 FOR UPDATE",
        "SELECT name FROM article WHERE name = ?1 LIMIT ?2 -- unproved comment",
    ] {
        let plain_postgres = plain.replace('?', "$");
        let unproven_postgres = unproven.replace('?', "$");
        for (sqlite, postgres) in [
            (unproven, plain_postgres.as_str()),
            (plain, unproven_postgres.as_str()),
        ] {
            let files = sql_projection_files(sqlite, postgres, "get");
            reject(
                &files
                    .iter()
                    .map(|(path, source)| (*path, source.as_str()))
                    .collect::<Vec<_>>(),
                "GET App call chains cannot write",
            );
            let files = sql_projection_files(sqlite, postgres, "post");
            check(
                &files
                    .iter()
                    .map(|(path, source)| (*path, source.as_str()))
                    .collect::<Vec<_>>(),
            );
        }
    }
}

#[test]
fn get_rejects_writes_reached_through_cross_domain_app_calls() {
    let writer = "create(name: Text) (value: Text) { account = model.create({ name = text.trim(name) })\nvalue = account.name }";
    reject(
        &[
            (
                "user/account/model.dever",
                "type Account { name: Text(1, 64) }",
            ),
            ("user/account/app.dever", writer),
            (
                "user/report/app.dever",
                "greet(name: Text) (value: Text) { value = \"hello \" + user.account.create(name) }",
            ),
            ("user/report/api.dever", "get greeting = app.greet"),
        ],
        "GET App call chains cannot write",
    );
    check(&[
        (
            "user/account/model.dever",
            "type Account { name: Text(1, 64) }",
        ),
        ("user/account/app.dever", writer),
        ("user/account/api.dever", "post create = app.create"),
    ]);
    check(&[
        (
            "user/account/model.dever",
            "type Account { name: Text(1, 64) }",
        ),
        (
            "user/account/app.dever",
            "count() (value: Text) { value = int.to_text(model.count()) }",
        ),
        ("user/account/api.dever", "get count = app.count"),
    ]);
    reject(
        &[
            (
                "user/account/app.dever",
                "disable() (ok: Bool) { dever.auth.disable_role(\"reader\")\nok = true }",
            ),
            ("user/account/api.dever", "get disable = app.disable"),
        ],
        "GET App call chains cannot write database or authorization state",
    );
}

#[test]
fn rest_requires_unambiguous_same_domain_model_and_rejects_duplicate_registration() {
    let model = "type Account { name: Text(1, 64) }";
    check(&[
        ("user/account/model.dever", model),
        ("user/account/api.dever", "rest"),
    ]);
    check(&[
        ("user/account/model.dever", model),
        ("user/account/api.dever", "rest model"),
    ]);
    reject(
        &[("user/account/api.dever", "rest")],
        "requires one same-domain Model",
    );
    reject(
        &[
            ("user/account/model.dever", model),
            ("user/account/api.dever", "rest\nrest"),
        ],
        "duplicate HTTP route",
    );
}

#[test]
fn rest_selector_disambiguates_main_and_topic_models() {
    let main = (
        "user/account/model.dever",
        "type Account { name: Text(1, 64) }",
    );
    let detail = (
        "user/account/model/detail.dever",
        "type Detail { title: Text(1, 64) }",
    );
    reject(
        &[main, detail, ("user/account/api.dever", "rest")],
        "requires one same-domain Model",
    );
    check(&[
        main,
        detail,
        ("user/account/api.dever", "rest model.detail"),
    ]);
    let alternate = (
        "user/account/model/archive.dever",
        "type Archive { name: Text(1, 64) }",
    );
    reject(
        &[detail, alternate, ("user/account/api.dever", "rest")],
        "requires one same-domain Model",
    );
    reject(
        &[main, ("user/account/api.dever", "rest user.profile.model")],
        "requires one same-domain Model",
    );
}

#[test]
fn generated_rest_and_custom_bindings_cannot_register_the_same_route() {
    reject(
        &[
            (
                "user/account/model.dever",
                "type Account { name: Text(1, 64) }",
            ),
            (
                "user/account/app.dever",
                "details() (value: Text) { value = \"details\" }",
            ),
            ("user/account/api.dever", "get details = app.details"),
            ("user/account/api/details.dever", "rest model"),
        ],
        "duplicate HTTP route",
    );
}

#[test]
fn api_declarations_format_idempotently() {
    let mut sources = SourceMap::default();
    sources.add("user/account/api.dever", "# Actions\npost  publish=app.publish # publish article\ncmd rebuild =app.rebuild\nrest model\n");
    let source = sources.files().first().unwrap();
    let formatted = dever_core::format::format(source).unwrap();
    assert!(formatted.contains("post publish = app.publish"));
    assert!(formatted.contains("cmd rebuild = app.rebuild"));
    assert!(formatted.contains("rest model"));
    assert!(formatted.contains("# Actions"));
    assert!(formatted.contains("# publish article"));
    let mut roundtrip = SourceMap::default();
    roundtrip.add("user/account/api.dever", formatted.clone());
    assert_eq!(
        dever_core::format::format(roundtrip.files().first().unwrap()).unwrap(),
        formatted
    );
}

#[test]
fn markdown_api_declarations_share_plain_source_contracts() {
    let markdown = "# 账户接口\n\n绑定账户业务能力。\n\n- 包：`user.account.api`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n## 问候\n\n提供账户问候接口。\n\n- 声明：`get greeting`\n\n```dever\nget greeting = app.greet\n```\n";
    check(&[
        (
            "user/account/app.dever",
            "greet() (value: Text) { value = \"hello\" }",
        ),
        ("user/account/api.dever.md", markdown),
    ]);
    let invalid = markdown.replace("`get greeting`", "`post greeting`");
    reject(
        &[
            (
                "user/account/app.dever",
                "greet() (value: Text) { value = \"hello\" }",
            ),
            ("user/account/api.dever.md", &invalid),
        ],
        "documented Model declaration",
    );
}
