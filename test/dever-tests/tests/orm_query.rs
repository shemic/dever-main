use dever_core::source::SourceMap;
use dever_runtime::config::RuntimeProfile;

const MODEL: &str = r#"
public type UserStatus {
  Active = "启用"
  Disabled = "禁用"
}

public type User {
  external_uuid: Uuid default "018f08d8-59b0-7cc2-98c4-dc0c0c07398f"
  email: Text(254) unique
  display_name: Text(1, 64)
  status: UserStatus default UserStatus.Active index
  enabled: Bool default true
  age: Int default 0
  alias: Text?
}
"#;

const SERVICE: &str = r#"
public register() (user: user.model.user.User) {
  user = user.create({
    external_uuid = "018f08d8-59b0-7cc2-98c4-dc0c0c07398f"
    email = "reader@example.com"
    display_name = "Reader"
  })
}

public active() (users: user.model.user.Page) {
  users = user.list({
    where = status == user.model.user.UserStatus.Active and enabled == true
    order = [created_at.desc]
    page = 2
    size = 10
  })
}

public find(id: user.model.user.id) (user: user.model.user.User) {
  user = user.get(id)
}

public disable(id: user.model.user.id) (changed: Int) {
  changed = user.update(id, {
    status = user.model.user.UserStatus.Disabled
    enabled = false
  })
}

public remove_disabled() (changed: Int) {
  changed = user.delete({
    where = status == user.model.user.UserStatus.Disabled
  })
}

public bulk() (changed: Int) {
  changed = user.create_many([
    {
      email = "one@example.com"
      display_name = "One"
    },
    {
      email = "two@example.com"
      display_name = "Two"
    }
  ])
}

public advanced() (users: user.model.user.Page) {
  users = user.list({
    where = not contains(display_name, "bot") and in(status, [user.model.user.UserStatus.Active, user.model.user.UserStatus.Disabled]) and between(age, 18, 65) and starts_with(email, "reader") and ends_with(email, ".com") and alias == null
  })
}

public page_after() (users: user.model.user.Cursor) {
  users = user.cursor({
    after = null
    order = [created_at.desc, id.desc]
    size = 10
  })
}

show(user: user.model.user.User) () {}

public scan() () {
  users = user.stream({ size = 4 })
  each(show, users)
}

public save() (saved: user.model.user.User) {
  saved = user.upsert(
    { email = "reader@example.com" },
    { display_name = "Reader" },
    { display_name = "Updated" }
  )
}
"#;

fn sources(service: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("user/model/user.dever", MODEL);
    sources.add("user/service/account.dever", service);
    sources
}

fn errors(service: &str) -> String {
    let sources = sources(service);
    dever_core::check(&sources)
        .unwrap_err()
        .iter()
        .map(|error| error.render(&sources))
        .collect()
}

#[test]
fn model_operations_build_static_query_plans_and_parameterized_sql() {
    let sources = sources(SERVICE);
    let program = dever_core::check(&sources).unwrap();
    let api = program.api_snapshot();
    assert!(api.contains("function user.service.account.register"));
    assert!(api.contains("effects=[database]"), "{api}");

    let create =
        dever_core::native::emit(&program, &sources, "user.service.account.register").unwrap();
    assert!(create.contains("INSERT INTO \\\"user\\\""), "{create}");
    assert!(create.contains("VALUES (?1, ?2, ?3)"), "{create}");
    assert!(
        create.contains("dever_runtime::orm::Uuid::from_bytes"),
        "{create}"
    );
    assert!(
        create.contains("dever_runtime::orm::Value::Uuid"),
        "{create}"
    );
    assert!(create.contains("$1::uuid"), "{create}");
    assert!(create.contains("decode_model_"), "{create}");

    let list = dever_core::native::emit(&program, &sources, "user.service.account.active").unwrap();
    assert!(list.contains("SELECT COUNT(*) FROM \\\"user\\\""), "{list}");
    assert!(
        list.contains("ORDER BY \\\"created_at\\\" DESC, \\\"id\\\" DESC"),
        "{list}"
    );
    assert!(list.contains("LIMIT ?3 OFFSET ?4"), "{list}");

    let advanced =
        dever_core::native::emit(&program, &sources, "user.service.account.advanced").unwrap();
    assert!(
        advanced.contains("NOT (instr(\\\"display_name\\\", ?1) > 0)"),
        "{advanced}"
    );
    assert!(
        advanced.contains("\\\"status\\\" IN (?2, ?3)"),
        "{advanced}"
    );
    assert!(
        advanced.contains("\\\"age\\\" BETWEEN ?4 AND ?5"),
        "{advanced}"
    );
    assert!(advanced.contains("\\\"alias\\\" IS NULL"), "{advanced}");

    let cursor =
        dever_core::native::emit(&program, &sources, "user.service.account.page_after").unwrap();
    assert!(
        cursor.contains("(CAST(?1 AS BIGINT) IS NULL OR ((\\\"created_at\\\" < ?1) OR (\\\"created_at\\\" IS ?1 AND \\\"id\\\" < ?2)))"),
        "{cursor}"
    );
    assert!(
        cursor.contains("(CAST($1 AS BIGINT) IS NULL OR ((\\\"created_at\\\" < $1) OR (\\\"created_at\\\" IS NOT DISTINCT FROM $1 AND \\\"id\\\" < $2)))"),
        "{cursor}"
    );
    assert!(cursor.contains("LIMIT ?3"), "{cursor}");
    assert!(!cursor.contains("COUNT(*)"), "{cursor}");

    let bulk = dever_core::native::emit(&program, &sources, "user.service.account.bulk").unwrap();
    assert!(bulk.contains("VALUES (?1, ?2), (?3, ?4)"), "{bulk}");

    let stream = dever_core::native::emit(&program, &sources, "user.service.account.scan").unwrap();
    assert!(stream.contains("database.stream("), "{stream}");
    assert!(stream.contains("ORDER BY \\\"id\\\" DESC"), "{stream}");
    assert!(
        !stream.contains("collect::<Result<Vec<_>, _>>()"),
        "{stream}"
    );

    let upsert = dever_core::native::emit(&program, &sources, "user.service.account.save").unwrap();
    assert!(
        upsert.contains("ON CONFLICT (\\\"email\\\") DO UPDATE"),
        "{upsert}"
    );
}

#[test]
fn model_operations_reject_unknown_fields_and_invalid_query_shapes() {
    let unknown = SERVICE.replace(
        "email = \"reader@example.com\"",
        "unknown = \"reader@example.com\"",
    );
    assert!(errors(&unknown).contains("unknown Model field 'unknown'"));

    let generated = SERVICE.replace(
        "email = \"reader@example.com\"",
        "id = \"reader@example.com\"",
    );
    assert!(errors(&generated).contains("generated Model field 'id'"));

    let unknown_option = SERVICE.replace("page = 2", "offset = 2");
    assert!(errors(&unknown_option).contains("unknown query option 'offset'"));

    let invalid_in = SERVICE.replace(
        "in(status, [user.model.user.UserStatus.Active, user.model.user.UserStatus.Disabled])",
        "in(status, [])",
    );
    assert!(errors(&invalid_in).contains("between 1 and 100 values"));

    let invalid_text = SERVICE.replace("contains(display_name, \"bot\")", "contains(age, \"bot\")");
    assert!(errors(&invalid_text).contains("requires a Text field"));

    let invalid_arity =
        SERVICE.replace("contains(display_name, \"bot\")", "contains(display_name)");
    assert!(errors(&invalid_arity).contains("where 'contains' expects 2 arguments"));

    let invalid_field = SERVICE.replace(
        "contains(display_name, \"bot\")",
        "contains(\"bot\", display_name)",
    );
    assert!(errors(&invalid_field).contains("the first 'contains' argument must be a Model field"));

    let outside_where = SERVICE.replace(
        "user = user.get(id)",
        "value = contains(\"reader\", \"read\")\n  user = user.get(id)",
    );
    let outside_where_errors = errors(&outside_where);
    assert!(
        outside_where_errors.contains("unknown function 'contains' with 2 argument(s)"),
        "{outside_where_errors}"
    );

    let invalid_cursor = SERVICE.replace("order = [created_at.desc, id.desc]", "order = alias.asc");
    assert!(errors(&invalid_cursor).contains("does not support nullable Model fields"));
}

#[test]
fn empty_create_many_uses_one_valid_statement_per_dialect() {
    let mut sources = SourceMap::default();
    sources.add("app/event/model.dever", "type Event {}");
    sources.add(
        "app/event/app.dever",
        "create() (created: Int) { created = model.create_many([{}, {}]) }",
    );
    let program = dever_core::check(&sources).unwrap();
    let native = dever_core::native::emit(&program, &sources, "app.event.create").unwrap();
    assert!(
        native.contains("sqlite: \"INSERT INTO \\\"event\\\" (\\\"id\\\") VALUES (NULL), (NULL)\""),
        "{native}"
    );
    assert!(
        native.contains("postgres: \"INSERT INTO \\\"event\\\" (\\\"created_at\\\") VALUES (DEFAULT), (DEFAULT)\""),
        "{native}"
    );
}

#[test]
fn transaction_functions_generate_one_shared_database_boundary() {
    let service = SERVICE.replacen("register()", "transaction register()", 1);
    let sources = sources(&service);
    let program = dever_core::check(&sources).unwrap();
    let native =
        dever_core::native::emit(&program, &sources, "user.service.account.register").unwrap();

    assert!(
        native.contains("transaction_database.begin().await"),
        "{native}"
    );
    assert!(native.contains("transaction.commit().await"), "{native}");
    assert!(native.contains("transaction.rollback().await"), "{native}");
    assert!(native.contains("database::Executor::new"), "{native}");
    assert!(
        native.contains("postgres: \"INSERT INTO \\\"user\\\""),
        "{native}"
    );
}

#[test]
fn database_effects_flow_through_helpers_and_nested_transactions() {
    let helper = r#"
create_user() (user: user.model.user.User) {
  user = user.create({
    email = "reader@example.com"
    display_name = "Reader"
  })
}

public transaction register() (user: user.model.user.User) {
  user = create_user()
}
"#;
    let helper_sources = sources(helper);
    let helper_program = dever_core::check(&helper_sources).unwrap();
    let helper_native = dever_core::native::emit(
        &helper_program,
        &helper_sources,
        "user.service.account.register",
    )
    .unwrap();
    assert!(
        helper_native
            .matches("Option<&dever_runtime::database::Transaction>")
            .count()
            >= 2,
        "{helper_native}"
    );
    assert!(helper_native.contains("(None).await"), "{helper_native}");
    assert!(
        helper_native.contains("(_database).await"),
        "{helper_native}"
    );

    let nested = helper.replacen(
        "create_user() (user: user.model.user.User)",
        "transaction create_user() (user: user.model.user.User)",
        1,
    );
    let nested_sources = sources(&nested);
    let nested_program = dever_core::check(&nested_sources).unwrap();
    let nested_native = dever_core::native::emit(
        &nested_program,
        &nested_sources,
        "user.service.account.register",
    )
    .unwrap();
    assert_eq!(
        nested_native
            .matches("transaction_database.begin().await")
            .count(),
        2,
        "{nested_native}"
    );
    assert!(
        nested_native.contains("(_database).await"),
        "{nested_native}"
    );
}

#[test]
fn transaction_functions_require_database_work_and_reject_concurrency() {
    let mut empty = SourceMap::default();
    empty.add("main.dever", "public transaction save() () {}");
    let empty_error = dever_core::check(&empty)
        .unwrap_err()
        .iter()
        .map(|error| error.render(&empty))
        .collect::<String>();
    assert!(
        empty_error.contains("must perform a database operation"),
        "{empty_error}"
    );

    let concurrent = r#"work() () {}
public transaction register() (user: user.model.user.User) {
  task = run(work())
  user = user.create({
    email = "reader@example.com"
    display_name = "Reader"
  })
  wait(task)
}
"#;
    assert!(errors(concurrent).contains("transaction functions cannot start concurrent work"));

    let streaming = r#"public transaction scan() () {
  users = user.stream({ size = 1 })
  close(users)
}
"#;
    assert!(errors(streaming).contains("transaction functions cannot start concurrent work"));

    let structured_blocking = r#"hash(password: Secret) (value: Text) {
  value = blocking(dever.crypto.password_hash(password))
}

public transaction register(password: Secret) (user: user.model.user.User) {
  password_hash = hash(password)
  user = user.create({
    email = password_hash
    display_name = "Reader"
  })
}
"#;
    assert!(
        errors(structured_blocking)
            .contains("transaction functions cannot perform password hashing or verification")
    );

    let short_transaction = r#"hash(password: Secret) (value: Text) {
  value = blocking(dever.crypto.password_hash(password))
}

transaction create_user(email: Text) (user: user.model.user.User) {
  user = user.create({
    email = email
    display_name = "Reader"
  })
}

public register(password: Secret) (user: user.model.user.User) {
  password_hash = hash(password)
  user = create_user(password_hash)
}
"#;
    let sources = sources(short_transaction);
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

#[test]
fn nullable_comparisons_use_null_safe_database_operators() {
    let service = r#"
find_alias(value: Text?) (users: user.model.user.Page) {
  users = user.list({ where = alias == value })
}

public main() (users: user.model.user.Page) {
  users = find_alias(null)
}
"#;
    let sources = sources(service);
    let program = dever_core::check(&sources).unwrap();
    let native = dever_core::native::emit(&program, &sources, "user.service.account.main").unwrap();

    assert!(native.contains("\\\"alias\\\" IS ?1"), "{native}");
    assert!(
        native.contains("\\\"alias\\\" IS NOT DISTINCT FROM $1"),
        "{native}"
    );
}

#[test]
fn database_effects_follow_bound_handlers_inside_transactions() {
    let service = r#"
apply(route: handler() (user: user.model.user.User)) (user: user.model.user.User) {
  user = route()
}

forward(route: handler() (user: user.model.user.User)) (user: user.model.user.User) {
  user = apply(route)
}

create_user() (user: user.model.user.User) {
  user = user.create({
    email = "reader@example.com"
    display_name = "Reader"
  })
}

public transaction register() (user: user.model.user.User) {
  user = forward(create_user)
}
"#;
    let sources = sources(service);
    let program = dever_core::check(&sources).unwrap();
    let native =
        dever_core::native::emit(&program, &sources, "user.service.account.register").unwrap();

    assert!(
        native.contains("transaction_database.begin().await"),
        "{native}"
    );
    assert!(native.contains("(_database).await"), "{native}");

    let concurrent = service.replace(
        "create_user() (user: user.model.user.User) {",
        "work() () {}\n\ncreate_user() (user: user.model.user.User) {\n  task = run(work())\n  wait(task)",
    );
    assert!(errors(&concurrent).contains("transaction functions cannot start concurrent work"));
}

#[test]
fn project_models_generate_scoped_storage_and_controlled_tenant_migration() {
    let mut sources = SourceMap::default();
    sources.add(
        "platform/role/model.dever",
        "global type Role { name: Text(1, 32) }",
    );
    sources.add(
        "user/profile/model.dever",
        "type Profile {\n  role_id: platform.role.model.id\n  display_name: Text(1, 64)\n}",
    );
    sources.add(
        "user/profile/app.dever",
        "count() (value: Int) { value = model.count() }",
    );
    sources.add("user/profile/api.dever", "cmd count = app.count");

    let program = dever_core::check(&sources).unwrap();
    let native = dever_core::native::emit_project(
        &program,
        &sources,
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();

    assert!(!native.contains("MODEL_DATABASE_"), "{native}");
    assert!(native.contains("MODEL_STORAGE_"), "{native}");
    assert!(
        native.contains("OnceLock<dever_runtime::database::StorageBinding>"),
        "{native}"
    );
    assert!(
        native.contains("dever_runtime::database::ModelScope::Global"),
        "{native}"
    );
    assert!(
        native.contains("dever_runtime::database::ModelScope::Tenant"),
        "{native}"
    );
    assert!(
        native.contains("dever_runtime::config::bootstrap_scoped"),
        "{native}"
    );
    assert!(
        native.contains("async fn tenant_migrate(tenant_id: i64)"),
        "{native}"
    );
    assert!(native.contains("--dever-tenant-migrate"), "{native}");
    assert!(
        native.contains("dever_runtime::tenant::begin_migration"),
        "{native}"
    );
    assert!(
        native.contains("dever_runtime::tenant::mark_database_ready"),
        "{native}"
    );
    assert!(
        native.contains("dever_runtime::tenant::finish_migration"),
        "{native}"
    );
    assert!(!native.contains("REFERENCES \\\"role\\\""), "{native}");
}
