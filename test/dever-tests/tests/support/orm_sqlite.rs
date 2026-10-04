#![allow(dead_code)]

use dever_runtime::database::{Field, Index, Migration, Model, Schema, SchemaSql, Seed, Sql};
use dever_runtime::orm::{Row, Value};
use dever_runtime::sqlite::Database;

pub const MODEL_NAME: &str = "app.model.user";
pub const TABLE_NAME: &str = "user";

#[derive(Clone)]
pub struct TestField {
    schema: Field,
    sqlite: String,
}

impl TestField {
    pub fn new(name: &str, ty: &str, nullable: bool, sqlite: &str) -> Self {
        Self {
            schema: Field {
                name: name.into(),
                ty: ty.into(),
                nullable,
                generated: name == "id" || name == "created_at",
                default: (name == "created_at").then(|| "generated:created_at".into()),
                rename_from: None,
            },
            sqlite: sqlite.into(),
        }
    }

    pub fn renamed_from(mut self, source: &str) -> Self {
        self.schema.rename_from = Some(source.into());
        self
    }

    pub fn default(mut self, value: &str) -> Self {
        self.schema.default = Some(value.into());
        self
    }
}

pub fn id() -> TestField {
    TestField::new(
        "id",
        "Int",
        false,
        "\"id\" INTEGER PRIMARY KEY AUTOINCREMENT",
    )
}

pub fn created_at() -> TestField {
    TestField::new(
        "created_at",
        "DateTime",
        false,
        "\"created_at\" INTEGER NOT NULL DEFAULT (unixepoch() * 1000)",
    )
}

pub fn text_field(name: &str, maximum: usize, nullable: bool) -> TestField {
    TestField::new(
        name,
        &format!("Text({maximum})"),
        nullable,
        &format!(
            "\"{name}\" TEXT {}CHECK (length(\"{name}\") <= {maximum})",
            if nullable { "" } else { "NOT NULL " }
        ),
    )
}

pub fn bytes_field(name: &str, maximum: usize, nullable: bool) -> TestField {
    TestField::new(
        name,
        &format!("Bytes({maximum})"),
        nullable,
        &format!(
            "\"{name}\" BLOB {}CHECK (length(\"{name}\") <= {maximum})",
            if nullable { "" } else { "NOT NULL " }
        ),
    )
}

pub fn bool_default(name: &str) -> TestField {
    TestField::new(
        name,
        "Bool",
        false,
        &format!("\"{name}\" INTEGER NOT NULL DEFAULT 1 CHECK (\"{name}\" IN (0, 1))"),
    )
    .default("true")
}

pub fn index(name: &str, fields: &[&str], unique: bool) -> Index {
    Index {
        name: name.into(),
        fields: fields.iter().map(|field| (*field).into()).collect(),
        unique,
    }
}

pub fn migration(name: &str, revision: &str, drops: &[&str]) -> Migration {
    Migration {
        name: name.into(),
        revision: revision.into(),
        drops: drops.iter().map(|field| (*field).into()).collect(),
    }
}

pub fn seed(sqlite: &'static str, parameters: Vec<Value>) -> Seed {
    Seed {
        sql: Sql {
            sqlite,
            postgres: "",
        },
        parameters,
    }
}

pub fn model(
    revision: &str,
    seed_revision: &str,
    fields: Vec<TestField>,
    indexes: Vec<Index>,
    migrations: Vec<Migration>,
    seeds: Vec<Seed>,
) -> Model {
    let temporary_table = format!("_dever_user_{}", &revision[..8]);
    let column_sql = fields
        .iter()
        .map(|field| field.sqlite.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let create_indexes = indexes
        .iter()
        .map(|index| {
            let fields = index
                .fields
                .iter()
                .map(|field| quote(field))
                .collect::<Vec<_>>()
                .join(", ");
            SchemaSql {
                sqlite: format!(
                    "CREATE {}INDEX IF NOT EXISTS {} ON {} ({fields})",
                    if index.unique { "UNIQUE " } else { "" },
                    quote(&index.name),
                    quote(TABLE_NAME),
                ),
                postgres: String::new(),
            }
        })
        .collect();

    Model {
        schema: Schema {
            model: MODEL_NAME.into(),
            table: TABLE_NAME.into(),
            revision: revision.into(),
            seed_revision: seed_revision.into(),
            fields: fields.into_iter().map(|field| field.schema).collect(),
            indexes,
            migrations,
        },
        create_table: SchemaSql {
            sqlite: format!("CREATE TABLE {} ({column_sql})", quote(TABLE_NAME)),
            postgres: String::new(),
        },
        create_temporary_table: format!("CREATE TABLE {} ({column_sql})", quote(&temporary_table)),
        create_indexes,
        postgres_columns: vec![],
        postgres_constraints: vec![],
        seeds,
        data_migrations: vec![],
    }
}

#[derive(Debug, PartialEq)]
pub struct DatabaseState {
    pub catalog: Vec<Row>,
    pub columns: Vec<Row>,
    pub history: Vec<Row>,
    pub data: Vec<Row>,
}

pub async fn database_state(database: &Database) -> DatabaseState {
    DatabaseState {
        catalog: database
            .query(
                "SELECT type, name, sql FROM sqlite_schema WHERE (type = 'table' AND name = 'user') OR (type = 'index' AND tbl_name = 'user') ORDER BY type, name",
                vec![],
            )
            .await
            .unwrap(),
        columns: database
            .query(
                "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info('user') ORDER BY cid",
                vec![],
            )
            .await
            .unwrap(),
        history: database
            .query(
                "SELECT kind, name, revision, definition FROM _dever_history WHERE model = 'app.model.user' ORDER BY kind, name",
                vec![],
            )
            .await
            .unwrap(),
        data: database
            .query("SELECT * FROM user ORDER BY id", vec![])
            .await
            .unwrap(),
    }
}

pub fn text(row: &Row, index: usize) -> &str {
    match row.get(index).unwrap() {
        Value::Text(value) => value,
        value => panic!("expected Text at column {index}, got {value:?}"),
    }
}

pub fn integer(row: &Row, index: usize) -> i64 {
    match row.get(index).unwrap() {
        Value::Int(value) => *value,
        value => panic!("expected Int at column {index}, got {value:?}"),
    }
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
