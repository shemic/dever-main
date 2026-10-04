mod support;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use dever_core::source::SourceMap;
use dever_runtime::database::{Field, Model, Schema, SchemaSql};
use dever_runtime::orm::{Row, Value};
use dever_runtime::sqlite::Database;

const GUARD_MODEL: &str = r#"
database z

public type Guard {
  code: Text(16)
}
"#;

#[derive(Debug, PartialEq)]
struct ConnectionState {
    tables: Vec<Row>,
    history: Vec<Row>,
    data: Vec<Vec<Row>>,
}

#[derive(Debug, PartialEq)]
struct CommittedConnections {
    a: ConnectionState,
    default: ConnectionState,
    report: ConnectionState,
    root: ConnectionState,
}

fn model_source(name: &str, database: Option<&str>, seed: &str) -> String {
    let database = database
        .map(|name| format!("database {name}\n\n"))
        .unwrap_or_default();
    format!(
        "{database}public type {name} {{\n  code: Text(16) unique\n}}\n\nseed {{\n  {{\n    code = \"{seed}\"\n  }}\n}}\n"
    )
}

fn sources() -> SourceMap {
    let mut sources = SourceMap::default();
    // The z-bound Model is deliberately first. Runtime migration order must not follow this order.
    sources.add("zfirst/model/guard.dever", GUARD_MODEL);
    sources.add(
        "later/model/omega.dever",
        model_source("Omega", Some("a"), "omega"),
    );
    sources.add(
        "earlier/model/beta.dever",
        model_source("Beta", Some("a"), "beta"),
    );
    sources.add(
        "fallback/model/defaulted.dever",
        model_source("Defaulted", None, "default"),
    );
    sources.add(
        "root/model/rooted.dever",
        model_source("Rooted", None, "root"),
    );
    sources.add(
        "analytics/model/reported.dever",
        model_source("Reported", Some("report"), "report"),
    );
    sources.add("main.dever", "public main() () {}\n");
    sources
}

#[test]
fn named_connection_models_are_valid_without_opening_a_database() {
    let sources = sources();
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
            .contains("model zfirst.model.guard name=Guard")
    );
}

fn compile() -> (dever_core::native::NativeProgram, String, String) {
    let sources = sources();
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
    let guard = snapshot
        .lines()
        .find(|line| line.starts_with("model zfirst.model.guard "))
        .expect("guard Model snapshot");
    let revision = snapshot_value(guard, "revision");
    let seed_revision = snapshot_value(guard, "seed");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let native = dever_core::native::compile_application(
        &program,
        &sources,
        "main.main",
        &rustc,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    (native, revision, seed_revision)
}

fn snapshot_value(line: &str, name: &str) -> String {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{name}=")))
        .expect("Model snapshot value")
        .to_owned()
}

fn configure(root: &Path) {
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("data/db")).unwrap();
    fs::write(
        root.join("config/setting.json"),
        r#"{
  "database": {
    "default": {"type":"sqlite","path":"data/db/default.db","max_connections":1,"max_page_size":20},
    "a": {"type":"sqlite","path":"data/db/a.db","max_connections":1,"max_page_size":20},
    "analytics": {"type":"sqlite","path":"data/db/analytics.db","max_connections":1,"max_page_size":20},
    "report": {"type":"sqlite","path":"data/db/report.db","max_connections":1,"max_page_size":20},
    "root": {"type":"sqlite","path":"data/db/root.db","max_connections":1,"max_page_size":20},
    "z": {"type":"sqlite","path":"data/db/z.db","max_connections":1,"max_page_size":20}
  }
}"#,
    )
    .unwrap();
}

fn database_path(root: &Path, name: &str) -> PathBuf {
    root.join("data/db").join(format!("{name}.db"))
}

fn run(native: &dever_core::native::NativeProgram) -> Output {
    Command::new(native.executable()).output().unwrap()
}

async fn execute(path: &Path, sql: String) {
    let database = Database::open(path, 1, 20).unwrap();
    database.execute_batch(sql).await.unwrap();
}

fn guard_table(table: &str) -> String {
    format!(
        "CREATE TABLE \"{table}\" (\n  \"id\" INTEGER PRIMARY KEY AUTOINCREMENT,\n  \"code\" TEXT NOT NULL CHECK (length(\"code\") <= 16),\n  \"created_at\" INTEGER NOT NULL DEFAULT (unixepoch() * 1000)\n)"
    )
}

async fn prepare_guard_catalog_drift(path: &Path, revision: String, seed_revision: String) {
    let database = Database::open(path, 1, 20).unwrap();
    database
        .migrate(Model {
            schema: Schema {
                model: "zfirst.model.guard".into(),
                table: "guard".into(),
                revision,
                seed_revision,
                fields: vec![
                    Field {
                        name: "id".into(),
                        ty: "id:zfirst.model.guard".into(),
                        nullable: false,
                        generated: true,
                        default: None,
                        rename_from: None,
                    },
                    Field {
                        name: "code".into(),
                        ty: "Text(16)".into(),
                        nullable: false,
                        generated: false,
                        default: None,
                        rename_from: None,
                    },
                    Field {
                        name: "created_at".into(),
                        ty: "DateTime".into(),
                        nullable: false,
                        generated: true,
                        default: Some("generated:created_at".into()),
                        rename_from: None,
                    },
                ],
                indexes: vec![],
                migrations: vec![],
            },
            create_table: SchemaSql {
                sqlite: guard_table("guard"),
                postgres: String::new(),
            },
            create_temporary_table: guard_table("_dever_guard_test"),
            create_indexes: vec![],
            postgres_columns: vec![],
            postgres_constraints: vec![],
            seeds: vec![],
            data_migrations: vec![],
        })
        .await
        .unwrap();
    database
        .execute_batch("ALTER TABLE guard ADD COLUMN rogue TEXT".into())
        .await
        .unwrap();
}

async fn connection_state(path: &Path, queries: &[&'static str]) -> ConnectionState {
    if !path.exists() {
        return ConnectionState {
            tables: vec![],
            history: vec![],
            data: vec![],
        };
    }
    let database = Database::open(path, 1, 20).unwrap();
    let tables = database
        .query(
            "SELECT name, sql FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != '_dever_history' ORDER BY name",
            vec![],
        )
        .await
        .unwrap();
    let has_history = database
        .query(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name = '_dever_history'",
            vec![],
        )
        .await
        .unwrap();
    let history = if has_history.is_empty() {
        vec![]
    } else {
        database
            .query(
                "SELECT model, kind, name, revision, definition FROM _dever_history ORDER BY model, kind, name",
                vec![],
            )
            .await
            .unwrap()
    };
    let mut data = Vec::new();
    for query in queries {
        data.push(database.query(query, vec![]).await.unwrap());
    }
    ConnectionState {
        tables,
        history,
        data,
    }
}

async fn committed_connections(
    a: &Path,
    default: &Path,
    report: &Path,
    root: &Path,
) -> CommittedConnections {
    CommittedConnections {
        a: connection_state(
            a,
            &[
                "SELECT code FROM omega ORDER BY id",
                "SELECT code FROM beta ORDER BY id",
            ],
        )
        .await,
        default: connection_state(default, &["SELECT code FROM defaulted ORDER BY id"]).await,
        report: connection_state(report, &["SELECT code FROM reported ORDER BY id"]).await,
        root: connection_state(root, &["SELECT code FROM rooted ORDER BY id"]).await,
    }
}

fn text(row: &Row, index: usize) -> &str {
    match row.get(index).unwrap() {
        Value::Text(value) => value,
        value => panic!("expected text at column {index}, got {value:?}"),
    }
}

fn table_names(state: &ConnectionState) -> Vec<&str> {
    state.tables.iter().map(|row| text(row, 0)).collect()
}

fn history_models(state: &ConnectionState) -> Vec<&str> {
    state
        .history
        .iter()
        .filter(|row| text(row, 1) == "schema")
        .map(|row| text(row, 0))
        .collect()
}

fn assert_complete_history(state: &ConnectionState, models: &[&str]) {
    let expected = models
        .iter()
        .flat_map(|model| ["catalog", "schema", "seed"].map(|kind| (*model, kind, "current")))
        .collect::<Vec<_>>();
    let actual = state
        .history
        .iter()
        .map(|row| (text(row, 0), text(row, 1), text(row, 2)))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn native_sqlite_connections_bind_and_recover_in_stable_order() {
    let (native, guard_revision, guard_seed_revision) = compile();
    let root = native.executable().parent().unwrap();
    configure(root);
    let a = database_path(root, "a");
    let default = database_path(root, "default");
    let analytics = database_path(root, "analytics");
    let report = database_path(root, "report");
    let root_database = database_path(root, "root");
    let z = database_path(root, "z");

    execute(
        &a,
        "CREATE TABLE beta (id INTEGER PRIMARY KEY, code INTEGER NOT NULL)".into(),
    )
    .await;
    prepare_guard_catalog_drift(&z, guard_revision, guard_seed_revision).await;

    let first = run(&native);
    assert!(!first.status.success());
    let first_error = String::from_utf8(first.stderr).unwrap();
    assert!(
        first_error.contains("table 'beta' exists without Dever schema history"),
        "{first_error}"
    );
    let first_a = connection_state(&a, &["SELECT code FROM omega ORDER BY id"]).await;
    assert_eq!(table_names(&first_a), ["beta", "omega"]);
    assert_eq!(history_models(&first_a), ["later.model.omega"]);
    assert_complete_history(&first_a, &["later.model.omega"]);
    assert_eq!(text(&first_a.data[0][0], 0), "omega");
    for pending in [&default, &report, &root_database] {
        assert_eq!(
            connection_state(pending, &[]).await,
            ConnectionState {
                tables: vec![],
                history: vec![],
                data: vec![],
            }
        );
    }

    execute(&a, "DROP TABLE beta".into()).await;
    let second = run(&native);
    assert!(!second.status.success());
    let second_error = String::from_utf8(second.stderr).unwrap();
    assert!(
        second_error.contains("table 'guard' does not match its recorded Dever schema"),
        "{second_error}"
    );

    let second_connections = committed_connections(&a, &default, &report, &root_database).await;
    assert_eq!(table_names(&second_connections.a), ["beta", "omega"]);
    assert_eq!(
        history_models(&second_connections.a),
        ["earlier.model.beta", "later.model.omega"]
    );
    assert_complete_history(
        &second_connections.a,
        &["earlier.model.beta", "later.model.omega"],
    );
    assert_eq!(table_names(&second_connections.default), ["defaulted"]);
    assert_eq!(
        history_models(&second_connections.default),
        ["fallback.model.defaulted"]
    );
    assert_complete_history(&second_connections.default, &["fallback.model.defaulted"]);
    assert_eq!(table_names(&second_connections.report), ["reported"]);
    assert_eq!(
        history_models(&second_connections.report),
        ["analytics.model.reported"]
    );
    assert_complete_history(&second_connections.report, &["analytics.model.reported"]);
    assert_eq!(table_names(&second_connections.root), ["rooted"]);
    assert_eq!(
        history_models(&second_connections.root),
        ["root.model.rooted"]
    );
    assert_complete_history(&second_connections.root, &["root.model.rooted"]);
    assert_eq!(text(&second_connections.a.data[0][0], 0), "omega");
    assert_eq!(text(&second_connections.a.data[1][0], 0), "beta");
    assert_eq!(text(&second_connections.default.data[0][0], 0), "default");
    assert_eq!(text(&second_connections.report.data[0][0], 0), "report");
    assert_eq!(text(&second_connections.root.data[0][0], 0), "root");
    assert_eq!(
        connection_state(&analytics, &[]).await,
        ConnectionState {
            tables: vec![],
            history: vec![],
            data: vec![],
        }
    );

    execute(&z, format!("DROP TABLE guard; {}", guard_table("guard"))).await;
    let third = run(&native);
    assert!(
        third.status.success(),
        "{}",
        String::from_utf8_lossy(&third.stderr)
    );
    let third_connections = committed_connections(&a, &default, &report, &root_database).await;
    let third_z = connection_state(&z, &["SELECT code FROM guard ORDER BY id"]).await;
    assert_eq!(third_connections, second_connections);
    assert_eq!(table_names(&third_z), ["guard"]);
    assert_eq!(history_models(&third_z), ["zfirst.model.guard"]);
    assert_complete_history(&third_z, &["zfirst.model.guard"]);
    assert!(third_z.data[0].is_empty());

    let fourth = run(&native);
    assert!(
        fourth.status.success(),
        "{}",
        String::from_utf8_lossy(&fourth.stderr)
    );
    assert_eq!(
        committed_connections(&a, &default, &report, &root_database).await,
        third_connections
    );
    assert_eq!(
        connection_state(&z, &["SELECT code FROM guard ORDER BY id"]).await,
        third_z
    );
}
