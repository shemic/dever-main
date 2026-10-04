use std::ffi::OsString;
use std::fs;
use std::process::Command;

use dever_core::source::SourceMap;

const MODEL: &str = r#"
type Status {
  Active
  Disabled
}

type UserSummary {
  status: Status
  total: Int
}

type User {
  email: Text(254) unique
  status: Status default Status.Active
}

sql by_status(status: Status) (users: List<User>) {
  sqlite = "SELECT \"id\", \"created_at\", \"email\", \"status\" FROM \"user\" WHERE \"status\" = ?1 ORDER BY \"id\""
  postgres = "SELECT \"id\", \"created_at\", \"email\", \"status\" FROM \"user\" WHERE \"status\" = $1 ORDER BY \"id\""
}

sql summary(status: Status) (summary: UserSummary) {
  sqlite = "SELECT \"status\", COUNT(*) FROM \"user\" WHERE \"status\" = ?1 GROUP BY \"status\""
  postgres = "SELECT \"status\", COUNT(*) FROM \"user\" WHERE \"status\" = $1 GROUP BY \"status\""
}

sql by_email(email: Text) (user: User?) {
  sqlite = "SELECT \"id\", \"created_at\", \"email\", \"status\" FROM \"user\" WHERE \"email\" = ?1"
  postgres = "SELECT \"id\", \"created_at\", \"email\", \"status\" FROM \"user\" WHERE \"email\" = $1"
}

sql required_by_email(email: Text) (user: User) {
  sqlite = "SELECT \"id\", \"created_at\", \"email\", \"status\" FROM \"user\" WHERE \"email\" = ?1"
  postgres = "SELECT \"id\", \"created_at\", \"email\", \"status\" FROM \"user\" WHERE \"email\" = $1"
}
"#;

const APP: &str = r#"
transaction create_and_read() (user: app.user.model.User) {
  created = model.create({
    email = "transaction@example.com"
  })
  user = model.required_by_email(created.email)
}

email_or_none(user: null) (email: Text) {
  email = "none"
}

email_or_none(user: app.user.model.User) (email: Text) {
  email = user.email
}

main() () {
  first = model.create({
    email = "first@example.com"
  })
  second = model.create({
    email = "second@example.com"
  })
  disabled = model.create({
    email = "disabled@example.com"
    status = app.user.model.Status.Disabled
  })

  active = model.by_status(app.user.model.Status.Active)
  blocking(dever.io.println(int.to_text(length(active))))
  summary = model.summary(app.user.model.Status.Active)
  blocking(dever.io.println(int.to_text(summary.total)))
  missing = model.by_email("missing@example.com")
  blocking(dever.io.println(email_or_none(missing)))
  found = model.by_email(first.email)
  blocking(dever.io.println(email_or_none(found)))
  transaction_user = create_and_read()
  blocking(dever.io.println(transaction_user.email))
}
"#;

fn sources(model: &str, app: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("app/user/model.dever", model);
    sources.add("app/user/app.dever", app);
    sources
}

fn checked(model: &str, app: &str) -> (SourceMap, dever_core::hir::Program) {
    let sources = sources(model, app);
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

fn errors(model: &str) -> String {
    source_errors(&sources(model, APP))
}

fn source_errors(sources: &SourceMap) -> String {
    dever_core::check(sources)
        .unwrap_err()
        .iter()
        .map(|error| error.render(sources))
        .collect()
}

fn compile(model: &str, app: &str) -> dever_core::native::NativeProgram {
    let (sources, program) = checked(model, app);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    dever_core::native::compile_application(
        &program,
        &sources,
        "app.user.main",
        &rustc,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap()
}

fn configure(native: &dever_core::native::NativeProgram) {
    let root = native.executable().parent().unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/setting.json"),
        r#"{
  "database": {
    "default": {
      "type": "sqlite",
      "path": "data/db/app.db",
      "max_connections": 1,
      "max_page_size": 10
    }
  }
}"#,
    )
    .unwrap();
}

#[test]
fn typed_sql_is_static_formatted_and_lowers_to_existing_model_executor() {
    let (sources, program) = checked(MODEL, APP);
    let formatted = dever_core::format::format(&sources.files()[0]).unwrap();
    checked(&formatted, APP);

    let snapshot = program.model_snapshot();
    assert_eq!(
        snapshot.matches("model app.user.model ").count(),
        1,
        "{snapshot}"
    );
    assert!(snapshot.contains("sql by_status"), "{snapshot}");
    assert!(snapshot.contains("Many:app.user.model.User"), "{snapshot}");
    assert!(
        snapshot.contains("One:app.user.model.UserSummary"),
        "{snapshot}"
    );

    let native = dever_core::native::emit(&program, &sources, "app.user.main").unwrap();
    assert!(native.contains("sqlite: \"SELECT \\\"id\\\""), "{native}");
    assert!(native.contains("postgres: \"SELECT \\\"id\\\""), "{native}");
    assert!(native.contains("\\\"status\\\" = ?1"), "{native}");
    assert!(native.contains("\\\"status\\\" = $1"), "{native}");
    assert!(native.contains("decode_sql_record_"), "{native}");
    assert!(native.contains("database.query_bounded("), "{native}");
    assert!(native.contains("database.max_page_size()"), "{native}");
    assert!(!native.contains("query_owned"), "{native}");
}

#[test]
fn typed_sql_rejects_dynamic_text_invalid_shapes_and_unowned_records() {
    let dynamic = MODEL.replace("sqlite = \"SELECT", "sqlite = query\n  query = \"SELECT");
    let rendered = errors(&dynamic);
    assert!(
        rendered.contains("SQL text must be a static Text literal"),
        "{rendered}"
    );

    let wrong_dialect = MODEL.replace(
        "WHERE \\\"status\\\" = $1 ORDER BY",
        "WHERE \\\"status\\\" = ?1 ORDER BY",
    );
    assert!(errors(&wrong_dialect).contains("other dialect's parameter marker"));

    let missing_parameter = MODEL.replace(
        "WHERE \\\"email\\\" = $1\"\n}\n\nsql required_by_email",
        "WHERE \\\"email\\\" = 'fixed'\"\n}\n\nsql required_by_email",
    );
    assert!(errors(&missing_parameter).contains("declared but not bound"));

    let map_result = MODEL.replace("(summary: UserSummary)", "(summary: Map<Text, Int>)");
    assert!(errors(&map_result).contains("concrete Model or record type"));

    let unused_record = MODEL
        .replace("type UserSummary", "type Unused")
        .replace("(summary: UserSummary)", "(summary: User)");
    assert!(errors(&unused_record).contains("auxiliary record in a Model file must be used"));

    let public_record = MODEL.replace("type UserSummary", "public type UserSummary");
    assert!(errors(&public_record).contains("visibility is defined by the source role"));

    let unowned_record = MODEL
        .replace(
            "type UserSummary {\n  status: Status\n  total: Int\n}\n\n",
            "",
        )
        .replace(
            "(summary: UserSummary)",
            "(summary: reporting.user.UserSummary)",
        );
    let mut unowned_sources = sources(&unowned_record, APP);
    unowned_sources.add(
        "reporting/user/app.dever",
        "type UserSummary {\n  status: app.user.model.Status\n  total: Int\n}\n",
    );
    let rendered = source_errors(&unowned_sources);
    assert!(
        rendered.contains("SQL result record must be private to its Model file"),
        "{rendered}"
    );

    let wrong_argument = APP.replace(
        "model.by_status(app.user.model.Status.Active)",
        "model.by_status(1)",
    );
    assert!(dever_core::check(&sources(MODEL, &wrong_argument)).is_err());
}

#[test]
fn typed_sql_can_project_into_an_owning_app_view_without_exporting_private_model_data() {
    let model = r#"
type User {
  name: Text
  private password_hash: Text
}
sql summaries() (users: List<app.UserView>) {
  sqlite = "SELECT name FROM user"
  postgres = "SELECT name FROM user"
}
"#;
    let app = r#"
type UserView {
  name: Text
}
list() (users: List<UserView>) {
  users = model.summaries()
}
"#;
    for result_name in ["app.UserView", "app.user.UserView"] {
        let mut sources = sources(&model.replace("app.UserView", result_name), app);
        sources.add("app/user/api.dever", "cmd list = app.list\n");
        let program = dever_core::check(&sources).unwrap();
        assert!(program.model_snapshot().contains("Many:app.user.UserView"));
        dever_core::native::emit(&program, &sources, "app.user.list").unwrap();
        dever_core::llvm::emit_application(&program, &sources).unwrap();
    }

    // App View 只接收显式存储标量，不能借投影导出私有字段或整个 Model。
    for field in [
        "private name: Text",
        "name: Secret",
        "name: model.User",
        "name: Int >= 1",
    ] {
        let rendered = source_errors(&sources(model, &app.replace("name: Text", field)));
        assert!(
            rendered.contains("SQL result field 'name' must be visible"),
            "{rendered}"
        );
    }
    let domain_result = model.replace("app.UserView", "app.user.domain.UserView");
    let mut invalid = sources(&domain_result, app);
    invalid.add(
        "app/user/domain.dever",
        "type UserView {\n  name: Text\n}\n",
    );
    assert!(source_errors(&invalid).contains("owning-domain App View"));
    assert!(errors(&MODEL.replace("total: Int", "total: Int >= 1")).contains("without bounds"));
}

#[test]
fn typed_sql_placeholder_validation_ignores_sql_lexical_regions() {
    let model = format!(
        r#"{MODEL}

sql lexical(first: Text, second: Text) (users: List<User>) {{
  sqlite = "SELECT \"id\", \"created_at\", \"email\", \"status\" FROM \"user\" WHERE (?2 = ?1 OR ?2 = ?1) AND '?9 $8' = '?7 $6' AND \"name?5$4\" = \"name?3$2\" AND `name?15$14` = [name?13$12] /* ?11 $10 */ -- ?17 $16\n"
  postgres = "SELECT \"id\", \"created_at\", \"email\", \"status\" FROM \"user\" WHERE ($2 = $1 OR $2 = $1) AND '$9 ?8' = '$7 ?6' AND \"name$5?4\" = \"name$3?2\" AND $$ $11 ?10 $$ = $$ $9 ?8 $$ AND $tag$ $7 ?6 $tag$ = $tag$ $5 ?4 $tag$ /* $13 ?12 */ -- $15 ?14\n"
}}
"#
    );
    checked(&model, APP);
    checked(&model.replace("$tag$", "$标签$"), APP);
    checked(
        &model.replace("($2 = $1 OR $2 = $1)", "(name$tag$ = $2 OR name$tag$ = $1)"),
        APP,
    );
}

#[test]
fn sqlite_executes_model_projection_custom_projection_and_transaction_sql() {
    let native = compile(MODEL, APP);
    configure(&native);
    let output = Command::new(native.executable()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "2\n2\nnone\nfirst@example.com\ntransaction@example.com\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn sqlite_reports_missing_extra_and_mismatched_result_columns() {
    let missing = MODEL.replace(
        "SELECT \\\"id\\\", \\\"created_at\\\", \\\"email\\\", \\\"status\\\" FROM \\\"user\\\" WHERE \\\"email\\\" = ?1\"\n  postgres",
        "SELECT \\\"id\\\" FROM \\\"user\\\" WHERE \\\"email\\\" = ?1\"\n  postgres",
    );
    let extra = MODEL.replace(
        "SELECT \\\"id\\\", \\\"created_at\\\", \\\"email\\\", \\\"status\\\" FROM \\\"user\\\" WHERE \\\"email\\\" = ?1\"\n  postgres",
        "SELECT \\\"id\\\", \\\"created_at\\\", \\\"email\\\", \\\"status\\\", 1 FROM \\\"user\\\" WHERE \\\"email\\\" = ?1\"\n  postgres",
    );
    let mismatched = MODEL.replace("COUNT(*)", "'not-an-int'");

    for (model, message) in [
        (missing, "database row is missing column"),
        (extra, "has extra columns"),
        (mismatched, "expected a database Int"),
    ] {
        let native = compile(&model, APP);
        configure(&native);
        let output = Command::new(native.executable()).output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(message), "{stderr}");
    }
}

#[test]
fn singular_sql_result_cardinality_is_strict() {
    use dever_runtime::orm::{Row, bounded_rows, optional_row, required_row};

    assert!(
        required_row(Vec::new(), "required")
            .unwrap_err()
            .to_string()
            .contains("no rows")
    );
    assert!(
        required_row(vec![Row::new(Vec::new()), Row::new(Vec::new())], "required")
            .unwrap_err()
            .to_string()
            .contains("more than one row")
    );
    assert!(optional_row(Vec::new(), "optional").unwrap().is_none());
    assert!(
        optional_row(vec![Row::new(Vec::new()), Row::new(Vec::new())], "optional")
            .unwrap_err()
            .to_string()
            .contains("more than one row")
    );
    assert_eq!(
        bounded_rows(vec![Row::new(Vec::new()), Row::new(Vec::new())], 1)
            .unwrap_err()
            .to_string(),
        "database query returned more than one row"
    );
    assert_eq!(
        bounded_rows((0..11).map(|_| Row::new(Vec::new())).collect(), 10,)
            .unwrap_err()
            .to_string(),
        "database query returned more than 10 rows"
    );
}

#[test]
fn sqlite_bounds_native_sql_rows_before_decoding() {
    let multiple = MODEL.replace(
        "WHERE \\\"email\\\" = ?1\"\n  postgres",
        "WHERE ?1 = ?1\"\n  postgres",
    );
    let many = r#"
main() () {
  one = model.create({email = "one@example.com"})
  two = model.create({email = "two@example.com"})
  three = model.create({email = "three@example.com"})
  four = model.create({email = "four@example.com"})
  five = model.create({email = "five@example.com"})
  six = model.create({email = "six@example.com"})
  seven = model.create({email = "seven@example.com"})
  eight = model.create({email = "eight@example.com"})
  nine = model.create({email = "nine@example.com"})
  ten = model.create({email = "ten@example.com"})
  eleven = model.create({email = "eleven@example.com"})
  users = model.by_status(app.user.model.Status.Active)
  blocking(dever.io.println(int.to_text(length(users))))
}
"#;

    for (model, app, message) in [
        (
            multiple.as_str(),
            APP,
            "database query returned more than one row",
        ),
        (MODEL, many, "database query returned more than 10 rows"),
    ] {
        let native = compile(model, app);
        configure(&native);
        let output = Command::new(native.executable()).output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(message), "{stderr}");
    }
}
