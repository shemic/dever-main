use dever_runtime::database::{
    DataMigration, Field, Migration, MigrationPhase, Model, PostgresColumn, Schema, SchemaSql, Sql,
};
use dever_runtime::orm::{ErrorKind, Value};
use dever_runtime::postgres::Database;

const MODEL: &str = "app.model.migration_probe";
const TABLE: &str = "migration_probe";
const BEFORE: &str = "UPDATE migration_probe SET name = $1 WHERE name = $2";
const AFTER: &str = "UPDATE migration_probe SET tag = $1";

fn field(name: &str, nullable: bool) -> Field {
    Field {
        name: name.into(),
        ty: "Text".into(),
        nullable,
        generated: false,
        default: None,
        rename_from: None,
    }
}

fn column(name: &str, nullable: bool) -> PostgresColumn {
    PostgresColumn {
        name: name.into(),
        definition: format!("\"{name}\" TEXT{}", if nullable { "" } else { " NOT NULL" }),
        sql_type: "TEXT".into(),
        default: None,
    }
}

fn statement(phase: MigrationPhase, sql: &'static str, parameters: Vec<Value>) -> DataMigration {
    DataMigration {
        name: "backfill_tag".into(),
        phase,
        sql: Sql {
            sqlite: "",
            postgres: sql,
        },
        parameters,
    }
}

fn model(upgraded: bool, after_sql: &'static str) -> Model {
    let mut fields = vec![field("name", false)];
    let mut columns = vec![column("name", false)];
    if upgraded {
        fields.push(field("tag", true));
        columns.push(column("tag", true));
    }
    let data_migrations = if upgraded {
        vec![
            statement(
                MigrationPhase::Before,
                BEFORE,
                vec![
                    Value::Text("short".into()),
                    Value::Text("legacy-name".into()),
                ],
            ),
            statement(
                MigrationPhase::After,
                after_sql,
                vec![Value::Text("tagged".into())],
            ),
        ]
    } else {
        vec![]
    };
    Model {
        schema: Schema {
            model: MODEL.into(),
            table: TABLE.into(),
            revision: if upgraded { "probe-v2" } else { "probe-v1" }.into(),
            seed_revision: "probe-seed".into(),
            fields,
            indexes: vec![],
            migrations: if upgraded {
                vec![Migration {
                    name: "backfill_tag".into(),
                    revision: "probe-v1".into(),
                    drops: vec![],
                }]
            } else {
                vec![]
            },
        },
        create_table: SchemaSql {
            sqlite: String::new(),
            postgres: if upgraded {
                "CREATE TABLE migration_probe (\"name\" TEXT NOT NULL, \"tag\" TEXT)"
            } else {
                "CREATE TABLE migration_probe (\"name\" TEXT NOT NULL)"
            }
            .into(),
        },
        create_temporary_table: String::new(),
        create_indexes: vec![],
        postgres_columns: columns,
        postgres_constraints: vec![],
        seeds: vec![],
        data_migrations,
    }
}

pub async fn assert_named_data_migration(database: &Database) {
    database.migrate(model(false, AFTER)).await.unwrap();
    database
        .execute(
            "INSERT INTO migration_probe (name) VALUES ($1)",
            vec![Value::Text("legacy-name".into())],
        )
        .await
        .unwrap();

    let failure = database
        .migrate(model(true, "UPDATE missing_probe SET tag = $1"))
        .await
        .unwrap_err();
    assert_eq!(failure.kind(), ErrorKind::Database, "{failure}");
    let names = database
        .query("SELECT name FROM migration_probe", vec![])
        .await
        .unwrap();
    assert_eq!(names[0].get(0).unwrap(), &Value::Text("legacy-name".into()));
    let columns = database
        .query(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'migration_probe' ORDER BY ordinal_position",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(columns.len(), 1);

    database.migrate(model(true, AFTER)).await.unwrap();
    let rows = database
        .query("SELECT name, tag FROM migration_probe", vec![])
        .await
        .unwrap();
    assert_eq!(rows[0].get(0).unwrap(), &Value::Text("short".into()));
    assert_eq!(rows[0].get(1).unwrap(), &Value::Text("tagged".into()));
    database
        .execute(
            "UPDATE migration_probe SET tag = $1",
            vec![Value::Text("hand-edited".into())],
        )
        .await
        .unwrap();
    database.migrate(model(true, AFTER)).await.unwrap();
    let rows = database
        .query("SELECT tag FROM migration_probe", vec![])
        .await
        .unwrap();
    assert_eq!(rows[0].get(0).unwrap(), &Value::Text("hand-edited".into()));
    let history = database
        .query(
            "SELECT COUNT(*) FROM _dever_history WHERE model = $1 AND kind = 'migration' AND name = 'backfill_tag'",
            vec![Value::Text(MODEL.into())],
        )
        .await
        .unwrap();
    assert_eq!(history[0].get(0).unwrap(), &Value::Int(1));
}
