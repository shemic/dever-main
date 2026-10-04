use dever_core::source::SourceMap;

#[path = "support/temp.rs"]
mod temp;

const USER: &str = r#"
database report

public type UserStatus {
  Active = "启用"
  Disabled = "禁用"
}

public type User {
  uuid: Uuid generated unique
  external_uuid: Uuid default "018f08d8-59b0-7cc2-98c4-dc0c0c07398f"
  email: Text(254) unique
  display_name: Text(1, 64) from nickname
  status: UserStatus default UserStatus.Active index
  enabled: Bool default true
  score: Float?
  balance: Decimal(20, 4) default 0
  avatar: Bytes(1048576)?
  birthday: Date?
  wake_at: Time?
  active_for: Duration default 0
  profile: Json?
}

index(status, created_at)

relation articles = news.model.news.author_id

seed {
  {
    email = "admin@example.com"
    display_name = "Admin"
    status = UserStatus.Active
    enabled = true
    balance = 0
    active_for = 0
  }
}

migrate remove_legacy_code {
  drop legacy_code
}

public status_options() (options: Map<UserStatus, Text>) pure {
  options = UserStatus.options
}
"#;

const NEWS: &str = r#"
public type News {
  author_id: user.model.user.id
  title: Text(1, 160)
  published_at: DateTime?
}
"#;

fn sources(user: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("user/model/user.dever", user);
    sources.add("news/model/news.dever", NEWS);
    sources
}

fn errors(user: &str) -> String {
    let sources = sources(user);
    match dever_core::check(&sources) {
        Ok(_) => panic!("Model source must be rejected"),
        Err(errors) => errors.iter().map(|error| error.render(&sources)).collect(),
    }
}

#[test]
fn model_syntax_formats_idempotently_and_builds_a_stable_schema() {
    let mut source_map = SourceMap::default();
    let id = source_map.add("user/model/user.dever", USER);
    let formatted = dever_core::format::format(source_map.get(id)).unwrap();
    let mut reformatted_map = SourceMap::default();
    let reformatted_id = reformatted_map.add("user/model/user.dever", &formatted);
    assert_eq!(
        dever_core::format::format(reformatted_map.get(reformatted_id)).unwrap(),
        formatted
    );

    let program = dever_core::check(&sources(USER)).unwrap();
    let snapshot = program.model_snapshot();
    assert!(
        snapshot.contains("model user.model.user name=User table=user connection=explicit:report")
    );
    assert!(snapshot.contains("field id id:user.model.user nullable=false generated=true"));
    assert!(snapshot.contains("field created_at DateTime nullable=false generated=true"));
    assert!(snapshot.contains(
        "field display_name Text(1,64) nullable=false generated=false default=- from=nickname"
    ));
    assert!(snapshot.contains("field balance Decimal(20,4)"));
    assert!(snapshot.contains("field external_uuid Uuid nullable=false generated=false"));
    assert!(
        snapshot.contains("choice user.model.user.UserStatus [Active=\"启用\",Disabled=\"禁用\"]")
    );
    assert!(snapshot.contains("index unique (email) implicit=false"));
    assert!(snapshot.contains("index plain (author_id) implicit=true"));
    assert!(snapshot.contains("relation to-many articles = news.model.news.author_id"));
    assert!(snapshot.contains("migrate remove_legacy_code drop(legacy_code)"));

    let native =
        dever_core::native::emit(&program, &sources(USER), "user.model.user.status_options")
            .unwrap();
    assert!(native.contains("Map::<T"));
    assert!(native.contains("from_unique"));

    let formatted_program = dever_core::check(&sources(&formatted)).unwrap();
    assert_eq!(program.model_snapshot(), formatted_program.model_snapshot());
}

#[test]
fn model_rest_binding_modifiers_parse_and_format_in_source_order() {
    let mut sources = SourceMap::default();
    let source = sources.add(
        "user/profile/model.dever",
        "type Profile {\nname:Text create=text.trim(input) replace=text.lower(input) search=input\n}\n",
    );
    let formatted = dever_core::format::format(sources.get(source)).unwrap();
    assert!(
        formatted.contains(
            "name: Text create = text.trim(input) replace = text.lower(input) search = input"
        ),
        "{formatted}"
    );
    let checked = dever_core::check(&sources).unwrap();
    assert!(
        checked
            .model_snapshot()
            .contains("index plain (name) implicit=true")
    );
}

#[test]
fn global_type_is_model_only_and_part_of_the_schema_identity() {
    let mut sources = SourceMap::default();
    let global = sources.add(
        "platform/tenant/model.dever",
        "global type Tenant { name: Text(1, 64) }\n",
    );
    sources.add(
        "news/article/model.dever",
        "type Article { tenant_id: platform.tenant.model.id\n tenant_ref: platform.tenant.model.id\n title: Text(1, 160) }\n",
    );
    let formatted = dever_core::format::format(sources.get(global)).unwrap();
    assert!(formatted.starts_with("global type Tenant"), "{formatted}");

    let program = dever_core::check(&sources).unwrap();
    let snapshot = program.model_snapshot();
    assert!(
        snapshot.contains("model platform.tenant.model name=Tenant table=tenant connection=package:platform scope=global"),
        "{snapshot}",
    );
    assert!(
        snapshot.contains("model news.article.model name=Article table=article connection=package:news scope=tenant"),
        "{snapshot}",
    );
    assert!(
        snapshot.contains("relation logical to-one tenant = platform.tenant.model.tenant_id"),
        "{snapshot}",
    );
    assert!(
        snapshot.contains("field tenant_ref id:platform.tenant.model nullable=false generated=false default=- from=- reference=logical"),
        "{snapshot}",
    );

    sources.add(
        "news/article/app.dever",
        "list() (page: model.Page) { page = model.list({ with = [tenant] }) }\n",
    );
    let errors = dever_core::check(&sources).unwrap_err();
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("logical cross-scope relation 'tenant' cannot be loaded")
    }));

    let mut regular = SourceMap::default();
    regular.add(
        "user/account/app.dever",
        "global type Identity { value: Text }\n",
    );
    let errors = dever_core::check(&regular).unwrap_err();
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("allowed only for the primary record")
    }));

    let mut reverse = SourceMap::default();
    reverse.add(
        "platform/tenant/model.dever",
        "global type Tenant { article: news.article.model.id }\n",
    );
    reverse.add("news/article/model.dever", "type Article { title: Text }\n");
    let errors = dever_core::check(&reverse).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("cannot reference tenant Model"))
    );
}

#[test]
fn authentication_verify_can_read_only_global_models() {
    let root = temp::TemporaryDirectory::new();
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::write(
        root.path().join("config/setting.json"),
        r#"{
          "auth": {"providers": {"session": {
            "verify": "user.account.verify",
            "jwtSecret": "0123456789abcdef0123456789abcdef"
          }}},
          "sites": {"admin": {"path": "admin", "auth": "session"}}
        }"#,
    )
    .unwrap();
    let settings = dever_runtime::config::Settings::load_project(root.path()).unwrap();
    let app = r#"type Identity {
  id: Text
  user_id: user.account.model.id?
  tenant_id: platform.tenant.model.id?
}
verify(claims: dever.auth.Claims) (identity: Identity) {
  identity = Identity {
    id = claims.subject + ":" + int.to_text(model.count())
    user_id = null
    tenant_id = null
  }
}
profile() (value: Text) { value = "ok" }
"#;
    let checked = |account: &str| {
        let mut sources = SourceMap::default();
        sources.add("user/account/model.dever", account);
        sources.add(
            "platform/tenant/model.dever",
            "global type Tenant { name: Text }\n",
        );
        sources.add("user/account/app.dever", app);
        sources.add(
            "user/account/api/admin/profile.dever",
            "get profile = app.profile\n",
        );
        (
            dever_core::check_with_settings(&sources, &settings),
            sources,
        )
    };

    let (result, sources) = checked("type Account { email: Text }\n");
    let errors = result.unwrap_err();
    let rendered = errors
        .iter()
        .map(|error| error.render(&sources))
        .collect::<String>();
    assert!(
        rendered.contains("authentication verify may access only global Models"),
        "{rendered}"
    );

    let (result, sources) = checked("global type Account { email: Text }\n");
    result.unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
}

#[test]
fn model_contracts_reject_ambiguous_or_unsafe_declarations() {
    let regular = {
        let mut sources = SourceMap::default();
        sources.add("main.dever", "public type Value { name: Text(64) }");
        sources
    };
    let regular_errors = dever_core::check(&regular).unwrap_err();
    assert!(
        regular_errors
            .iter()
            .any(|error| error.message.contains("only allowed on Model fields"))
    );

    let invalid_generated = USER.replace(
        "uuid: Uuid generated unique",
        "uuid: Uuid? generated unique",
    );
    assert!(errors(&invalid_generated).contains("requires a non-null Uuid field"));

    let invalid_uuid = USER.replace("018f08d8-59b0-7cc2-98c4-dc0c0c07398f", "not-a-uuid");
    assert!(errors(&invalid_uuid).contains("canonical 8-4-4-4-12 form"));

    let missing_key = USER.replace("email = \"admin@example.com\"\n", "");
    assert!(errors(&missing_key).contains("unique key"));

    let nullable_key = USER
        .replace("email: Text(254) unique", "email: Text(254)? unique")
        .replace("email = \"admin@example.com\"", "email = null");
    assert!(errors(&nullable_key).contains("non-null values"));

    let wrong_name = USER
        .replace("type User {", "type Account {")
        .replace("  User\n", "  Account\n");
    assert!(errors(&wrong_name).contains("must be named 'User'"));

    let fake_id = USER.replace(
        "status_options() (options: Map<UserStatus, Text>) pure {",
        "fake_id() (value: user.model.user.id) {\n  value = user.model.user.id {}\n}\n\nstatus_options() (options: Map<UserStatus, Text>) pure {",
    );
    assert!(errors(&fake_id).contains("created only by database operations"));
}

#[test]
fn model_defaults_and_seeds_validate_storage_constants_before_emission() {
    let cases = [
        ("Int", "9223372036854775807", true),
        ("Int", "-9223372036854775808", true),
        ("Int", "1.5", false),
        ("Int", "9223372036854775808", false),
        ("Int", "-9223372036854775809", false),
        ("Int", "1e2", false),
        ("Float", "1e308", true),
        ("Float", "-1e308", true),
        ("Float", "1e309", false),
        ("Float", "-1e309", false),
        ("Decimal(3, 2)", "9.99", true),
        ("Decimal(3, 2)", "-9.99", true),
        ("Decimal(3, 2)", "1.2300", true),
        ("Decimal(3, 2)", "10", false),
        ("Decimal(3, 2)", "0.001", false),
        ("Decimal(3, 2)", "1234.5678", false),
        ("Decimal(34, 0)", "9999999999999999999999999999999999", true),
        ("Text(1, 3)", "\"中😀文\"", true),
        ("Text(1, 3)", "\"\"", false),
        ("Text(1, 3)", "\"中😀文字\"", false),
        ("DateTime", "1.5", false),
        ("Date", "9223372036854775808", false),
        ("Time", "1e2", false),
        ("Duration", "-9223372036854775808", true),
    ];
    for (ty, value, valid) in cases {
        for seed in [false, true] {
            let field = if seed {
                ty.to_owned()
            } else {
                format!("{ty} default {value}")
            };
            let seed = if seed {
                format!("seed {{ {{ key = \"one\"\n value = {value} }} }}")
            } else {
                String::new()
            };
            let text = format!(
                "public type Item {{ key: Text unique\n value: {field} }}\n{seed}\npublic total_count() (total: Int) {{ total = app.model.item.count() }}\n"
            );
            let mut sources = SourceMap::default();
            sources.add("app/model/item.dever", text);
            match dever_core::check(&sources) {
                Ok(program) => {
                    assert!(valid, "accepted {ty} {value}, {seed}");
                    dever_core::native::emit(&program, &sources, "app.model.item.total_count")
                        .unwrap();
                }
                Err(errors) => {
                    assert!(!valid, "rejected {ty} {value}: {errors:?}");
                    assert!(
                        errors.iter().any(|error| error.code == "C014"),
                        "{errors:?}",
                    );
                }
            }
        }
    }
}
