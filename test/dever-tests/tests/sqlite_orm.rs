#[path = "support/orm_fixture.rs"]
mod orm_fixture;
mod support;

use std::fs;

use dever_runtime::config::RuntimeProfile;
use dever_runtime::sqlite::Database;
use orm_fixture::DatabaseState;

#[test]
fn shared_orm_fixture_remains_valid_without_opening_a_database() {
    let sources = orm_fixture::sources();
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
            .contains("model app.user.model name=User")
    );
}

async fn database_state(database: &Database) -> DatabaseState {
    let columns = database
        .query(
            "SELECT name FROM pragma_table_info('user') ORDER BY cid",
            vec![],
        )
        .await
        .unwrap();
    let managed_index_count = database
        .query(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index' AND tbl_name = 'user' AND name LIKE '_dever_idx_%'",
            vec![],
        )
        .await
        .unwrap();
    let history = database
        .query(
            "SELECT kind, revision FROM _dever_history WHERE model = 'app.user.model' AND kind IN ('schema', 'seed') ORDER BY kind",
            vec![],
        )
        .await
        .unwrap();
    let users = database
        .query(
            "SELECT email, display_name, status, age, alias FROM user ORDER BY email",
            vec![],
        )
        .await
        .unwrap();
    orm_fixture::database_state(columns, managed_index_count, history, users)
}

#[tokio::test]
async fn sqlite_executes_the_shared_orm_fixture() {
    let profile = RuntimeProfile {
        sqlite: true,
        postgres: false,
    };
    let main = orm_fixture::compile("app.user.main", profile);
    let rollback = orm_fixture::compile("app.user.rollback", profile);
    assert_eq!(main.revisions, rollback.revisions);

    let root = main.native.executable().parent().unwrap();
    let setting = r#"{
  "database": {
    "default": {
      "type": "sqlite",
      "path": "data/db/app.db",
      "max_connections": 1,
      "max_page_size": 10
    }
  }
}"#;
    orm_fixture::configure(main.native.executable(), setting);
    orm_fixture::assert_main_output(orm_fixture::run(main.native.executable()));

    let database = Database::open(root.join("data/db/app.db"), 1, 10).unwrap();
    let committed = database_state(&database).await;
    orm_fixture::assert_committed_state(&committed, &main.revisions);
    drop(database);

    let rollback_executable = root.join("rollback");
    fs::copy(rollback.native.executable(), &rollback_executable).unwrap();
    let output = orm_fixture::run(&rollback_executable);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("UNIQUE constraint failed"), "{stderr}");

    let database = Database::open(root.join("data/db/app.db"), 1, 10).unwrap();
    assert_eq!(database_state(&database).await, committed);
}

#[test]
fn database_failures_are_captured_after_transaction_rollback_and_task_wait() {
    let mut sources = dever_core::source::SourceMap::default();
    sources.add(
        "app/item/model.dever",
        r#"type Item { name: Text unique }
"#,
    );
    sources.add(
        "app/item/app.dever",
        r#"type ReadResult {
  Found(value: app.item.model.Item)
  error Failed(error: dever.database.Error)
}
transaction missing() (value: app.item.model.Item) {
  saved = model.create({ name = "temporary" })
  removed = model.delete(saved.id)
  value = model.get(saved.id)
}
transaction create_duplicate() (value: app.item.model.Item) {
  saved = model.create({ name = "duplicate" })
  value = model.create({ name = "duplicate" })
}
task_missing() (value: app.item.model.Item) {
  task = run(missing())
  value = wait(task)
}
main() (missing: ReadResult, duplicate: ReadResult, count: Int) {
  missing = result(task_missing())
  duplicate = result(create_duplicate())
  count = model.count()
}
"#,
    );
    let program = support::checked(&sources);
    let snapshot = program.api_snapshot();
    assert!(
        snapshot.contains("dever.database.Error.NotFound"),
        "{snapshot}"
    );
    let native = dever_core::native::compile_application(
        &program,
        &sources,
        "app.item.main",
        &std::ffi::OsString::from("rustc"),
        RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    orm_fixture::configure(
        native.executable(),
        r#"{
      "database": { "default": { "type": "sqlite", "path": "data/db/errors.db", "max_connections": 1 } }
    }"#,
    );
    let result = orm_fixture::run(native.executable());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.contains("app.item.ReadResult.Failed(dever.database.Error.NotFound("),
        "{output}"
    );
    assert!(
        output.contains("app.item.ReadResult.Failed(dever.database.Error.Constraint("),
        "{output}"
    );
    assert!(output.ends_with("count = 0\n"), "{output}");
}
