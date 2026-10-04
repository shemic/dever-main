#[path = "support/orm_sqlite.rs"]
mod orm_sqlite;
mod support;

use std::fs;
use std::process::Command;

use dever_core::source::SourceMap;
use dever_runtime::database::{DataMigration, MigrationPhase, Model, Sql};
use dever_runtime::orm::{ErrorKind, Value};
use dever_runtime::sqlite::Database;
use orm_sqlite::{created_at, database_state, id, migration, model, seed, text, text_field};

const SOURCE: &str = r#"type User { name: Text(1, 8) unique }
migrate normalize_names {
  before {
    sqlite = "UPDATE user SET name = ?1 WHERE name = ?2"
    postgres = "UPDATE user SET name = $1 WHERE name = $2"
    parameters = ["short", "too-long-name"]
  }
  drop legacy
  after {
    parameters = ["final"]
    postgres = "UPDATE user SET name = $1"
    sqlite = "UPDATE user SET name = ?1"
  }
}
"#;

fn sources(source: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("app/user/model.dever", source);
    sources.add(
        "app/user/app.dever",
        "main() (total: Int) { total = model.count() }\n",
    );
    sources
}

fn snapshot(source: &str) -> String {
    let sources = sources(source);
    support::checked(&sources).model_snapshot()
}

#[test]
fn migration_sql_formats_and_hashes_all_execution_inputs() {
    let source_map = sources(SOURCE);
    let formatted = dever_core::format::format(&source_map.files()[0]).unwrap();
    let formatted_map = sources(&formatted);
    assert_eq!(
        formatted,
        dever_core::format::format(&formatted_map.files()[0]).unwrap()
    );
    let original = snapshot(SOURCE);
    assert_eq!(original, snapshot(&formatted));
    for (before, after) in [
        (
            "[\"short\", \"too-long-name\"]",
            "[\"other\", \"too-long-name\"]",
        ),
        (
            "UPDATE user SET name = ?1 WHERE name = ?2",
            "UPDATE user SET name = ?1 WHERE name != ?2",
        ),
        (
            "UPDATE user SET name = $1 WHERE name = $2",
            "UPDATE user SET name = $1 WHERE name != $2",
        ),
        ("before {", "after {"),
    ] {
        assert_ne!(original, snapshot(&SOURCE.replace(before, after)));
    }
}

#[test]
fn migration_sql_rejects_unbound_nonliteral_and_non_dml_operations() {
    for sql in [
        "COMMIT",
        "PRAGMA foreign_keys = OFF",
        "DROP TABLE user",
        "SELECT 1",
        "UPDATE user SET name = ?1; DELETE FROM user WHERE name = ?2",
        "UPDATE user SET name = ?1; /* hidden boundary */ COMMIT",
        "UPDATE user SET name = ?3 WHERE name = ?2",
        "UPDATE user SET name = $1 WHERE name = $2",
        "-- only a comment",
        "UPDATE _dever_history SET revision = ?1 WHERE name = ?2",
        "DELETE FROM main.[_dever_history] WHERE name IN (?1, ?2)",
        "UPDATE '_dever_history' SET revision = ?1 WHERE name = ?2",
        "UPDATE `_Dever_history` SET revision = ?1 WHERE name = ?2",
        "UPDATE sqlite_master SET name = ?1 WHERE name = ?2",
        "UPDATE user SET name = '你好",
        "UPDATE user SET name = '",
        "UPDATE [你好 SET name = ?1 WHERE name = ?2",
    ] {
        let source = SOURCE.replacen("UPDATE user SET name = ?1 WHERE name = ?2", sql, 1);
        let errors = dever_core::check(&sources(&source)).unwrap_err();
        assert!(
            errors.iter().any(|error| error.code == "C014"),
            "{sql}: {errors:?}"
        );
    }
    for sql in [
        r#"UPDATE \"_dever_history\" SET revision = $1 WHERE name = $2"#,
        r#"UPDATE U&\"\\005fdever_history\" SET revision = $1 WHERE name = $2"#,
        "UPDATE pg_catalog.pg_class SET relname = $1 WHERE relname = $2",
    ] {
        let source = SOURCE.replacen("UPDATE user SET name = $1 WHERE name = $2", sql, 1);
        assert!(
            dever_core::check(&sources(&source))
                .unwrap_err()
                .iter()
                .any(|error| error.code == "C014")
        );
    }
    for parameter in ["1 + 2", "name", "1.25", "9223372036854775808", "1e999"] {
        let source = SOURCE.replacen(
            "[\"short\", \"too-long-name\"]",
            &format!("[{parameter}, \"old\"]"),
            1,
        );
        assert!(
            dever_core::check(&sources(&source))
                .unwrap_err()
                .iter()
                .any(|error| error.code == "C014")
        );
    }
    for omitted in [
        "    parameters = [\"short\", \"too-long-name\"]\n",
        "    sqlite = \"UPDATE user SET name = ?1 WHERE name = ?2\"\n",
        "    postgres = \"UPDATE user SET name = $1 WHERE name = $2\"\n",
    ] {
        assert!(
            dever_core::check(&sources(&SOURCE.replacen(omitted, "", 1)))
                .unwrap_err()
                .iter()
                .any(|error| error.code == "P001")
        );
    }
}

#[test]
fn migration_sql_scanner_preserves_quoted_and_commented_boundaries() {
    let source = SOURCE.replace(
        "UPDATE user SET name = ?1 WHERE name = ?2",
        "/* prefix */ UPDATE user SET name = ?1 WHERE name = ?2 AND '; ?90' = '; ?90'; -- tail",
    ).replace(
        "UPDATE user SET name = $1 WHERE name = $2",
        "/* prefix /* nested */ */ UPDATE user SET name = $1 WHERE name = $2 AND $quoted$; $90$quoted$ = $quoted$; $90$quoted$; -- tail",
    );
    snapshot(&source);
    snapshot(&SOURCE.replace("UPDATE user SET", "UPDATE my_pg_notes SET"));
}

#[test]
fn migration_sql_rejects_unclosed_postgres_dollar_quotes_without_panicking() {
    for sql in [
        "UPDATE user SET name = $tag",
        "UPDATE user SET name = $tag$unfinished",
        "UPDATE user SET name = $$unfinished",
        "UPDATE user SET name = $tag$你好",
    ] {
        let source = format!(
            "type User {{ name: Text }}\n\
             migrate change_name {{ after {{\n\
             sqlite = \"UPDATE user SET name = 'updated'\"\n\
             postgres = \"{sql}\"\nparameters = []\n}} }}\n"
        );
        let errors = dever_core::check(&sources(&source)).unwrap_err();
        assert!(
            errors.iter().any(|error| error.code == "C014"),
            "{sql}: {errors:?}"
        );
    }
}

fn initial() -> Model {
    model(
        "a000000000000001",
        "empty",
        vec![
            id(),
            text_field("name", 64, false),
            text_field("legacy", 64, true),
            created_at(),
        ],
        vec![],
        vec![],
        vec![],
    )
}

fn statement(phase: MigrationPhase, sql: &'static str, parameters: Vec<Value>) -> DataMigration {
    DataMigration {
        name: "normalize_names".into(),
        phase,
        sql: Sql {
            sqlite: sql,
            postgres: "unused SQLite test dialect",
        },
        parameters,
    }
}

fn upgraded(after: &'static str) -> Model {
    let mut model = model(
        "a000000000000002",
        "with-seed",
        vec![id(), text_field("name", 8, false), created_at()],
        vec![],
        vec![migration("normalize_names", "transform-v1", &["legacy"])],
        vec![seed(
            "INSERT INTO user (name) VALUES (?1)",
            vec![Value::Text("seed".into())],
        )],
    );
    model.data_migrations = vec![
        statement(
            MigrationPhase::Before,
            "UPDATE user SET name = ?1 WHERE name = ?2",
            vec![
                Value::Text("short".into()),
                Value::Text("too-long-name".into()),
            ],
        ),
        statement(
            MigrationPhase::After,
            after,
            vec![Value::Text("final".into())],
        ),
    ];
    model
}

#[tokio::test]
async fn data_migrations_surround_ddl_run_once_and_skip_fresh_databases() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("existing.db"), 1, 50).unwrap();
    database.migrate(initial()).await.unwrap();
    database
        .execute_batch("INSERT INTO user (name, legacy) VALUES ('too-long-name', 'old')".into())
        .await
        .unwrap();
    database
        .migrate(upgraded("UPDATE user SET name = ?1"))
        .await
        .unwrap();
    let state = database_state(&database).await;
    assert_eq!(text(&state.data[0], 1), "final");
    assert_eq!(text(&state.data[1], 1), "seed");
    assert_eq!(
        state
            .columns
            .iter()
            .map(|row| text(row, 0))
            .collect::<Vec<_>>(),
        ["id", "name", "created_at"]
    );
    database
        .migrate(upgraded("UPDATE user SET name = ?1"))
        .await
        .unwrap();
    assert_eq!(database_state(&database).await, state);
    let mut changed = upgraded("UPDATE user SET name = ?1");
    changed.schema.migrations[0].revision = "transform-v2".into();
    assert_eq!(
        database.migrate(changed).await.unwrap_err().kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&database).await, state);
    let mut removed = upgraded("UPDATE user SET name = ?1");
    removed.schema.migrations.clear();
    assert_eq!(
        database.migrate(removed).await.unwrap_err().kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&database).await, state);

    let fresh = Database::open(root.path().join("fresh.db"), 1, 50).unwrap();
    fresh
        .migrate(upgraded("UPDATE missing_historical_table SET name = ?1"))
        .await
        .unwrap();
    assert_eq!(text(&database_state(&fresh).await.data[0], 1), "seed");
}

#[tokio::test]
async fn data_ddl_seed_and_history_roll_back_together_and_catalog_is_checked_first() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("rollback.db"), 1, 50).unwrap();
    database.migrate(initial()).await.unwrap();
    database
        .execute_batch("INSERT INTO user (name, legacy) VALUES ('too-long-name', 'old')".into())
        .await
        .unwrap();
    let before = database_state(&database).await;
    assert!(
        database
            .migrate(upgraded("UPDATE missing_table SET name = ?1"))
            .await
            .is_err()
    );
    assert_eq!(database_state(&database).await, before);
    let mut bad_seed = upgraded("UPDATE user SET name = ?1");
    bad_seed.seeds[0].parameters[0] = Value::Text("long-seed-name".into());
    assert_eq!(
        database.migrate(bad_seed).await.unwrap_err().kind(),
        ErrorKind::Constraint
    );
    assert_eq!(database_state(&database).await, before);
    database
        .execute_batch("ALTER TABLE user ADD COLUMN rogue TEXT".into())
        .await
        .unwrap();
    drop(database);
    let database = Database::open(root.path().join("rollback.db"), 1, 50).unwrap();
    let drifted = database_state(&database).await;
    assert_eq!(
        database
            .migrate(upgraded("UPDATE user SET name = ?1"))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&database).await, drifted);
}

#[tokio::test]
async fn data_only_steps_keep_source_order_and_applied_history_is_immutable() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("data-only.db"), 1, 50).unwrap();
    database.migrate(initial()).await.unwrap();
    database
        .execute_batch("INSERT INTO user (name) VALUES ('')".into())
        .await
        .unwrap();
    let upgrade = || {
        let mut model = initial();
        model.schema.revision = "a000000000000003".into();
        model.schema.migrations = vec![
            migration("normalize_names", "data-only-v1", &[]),
            migration("keep_order", "data-only-v2", &[]),
        ];
        model.data_migrations = vec![
            statement(
                MigrationPhase::Before,
                "UPDATE user SET name = name || ?1",
                vec![Value::Text("first';--".into())],
            ),
            statement(
                MigrationPhase::Before,
                "UPDATE user SET name = name || ?1",
                vec![Value::Text("second".into())],
            ),
            statement(
                MigrationPhase::After,
                "UPDATE user SET name = name || ?1",
                vec![Value::Text("after".into())],
            ),
        ];
        model
    };
    database.migrate(upgrade()).await.unwrap();
    assert_eq!(
        text(&database_state(&database).await.data[0], 1),
        "first';--secondafter"
    );
    database
        .execute_batch("UPDATE _dever_history SET applied_at = 1 WHERE kind = 'migration'".into())
        .await
        .unwrap();
    database.migrate(upgrade()).await.unwrap();
    let history = database
        .query(
            "SELECT applied_at FROM _dever_history WHERE kind = 'migration'",
            vec![],
        )
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .all(|row| row.get(0).unwrap() == &Value::Int(1))
    );
    let before = database_state(&database).await;
    let mut reordered = upgrade();
    reordered.schema.migrations.reverse();
    assert_eq!(
        database.migrate(reordered).await.unwrap_err().kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&database).await, before);
}

#[test]
fn native_data_migration_executes_bound_values_during_upgrade() {
    let root = support::temp::TemporaryDirectory::new();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/setting.json"),
        r#"{"database":{"default":{"type":"sqlite","path":"data/app.db","max_connections":1}}}"#,
    )
    .unwrap();
    let original = "type User { name: Text(64) unique\n legacy: Text? }\nseed { { name = \"too-long-name\"\n legacy = \"old\" } }\n";
    for (name, source) in [("initial", original), ("upgraded", SOURCE)] {
        let sources = sources(source);
        let program = support::checked(&sources);
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let native = dever_core::native::compile_application(
            &program,
            &sources,
            "app.user.main",
            &compiler,
            dever_runtime::config::RuntimeProfile {
                sqlite: true,
                postgres: false,
            },
        )
        .unwrap();
        let executable = root.path().join(name);
        fs::copy(native.executable(), &executable).unwrap();
        for _ in 0..2 {
            let output = Command::new(&executable).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, b"total = 1\n");
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let database = Database::open(root.path().join("data/app.db"), 1, 50).unwrap();
        assert_eq!(text(&database_state(&database).await.data[0], 2), "final");
    });
}
