#[path = "support/orm_sqlite.rs"]
mod orm_sqlite;
mod support;

use dever_runtime::orm::{ErrorKind, Value};
use dever_runtime::sqlite::Database;
use orm_sqlite::{
    TestField, bool_default, bytes_field, created_at, database_state, id, index, migration, model,
    text, text_field,
};

fn text_model(
    revision: &str,
    text_maximum: usize,
    bytes_maximum: usize,
) -> dever_runtime::database::Model {
    model(
        revision,
        "seed-empty",
        vec![
            id(),
            text_field("name", text_maximum, false),
            bytes_field("payload", bytes_maximum, false),
            created_at(),
        ],
        vec![],
        vec![],
        vec![],
    )
}

#[tokio::test]
async fn new_database_uses_only_the_final_schema_and_records_real_catalog() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("new.db"), 1, 50).unwrap();
    let final_model = model(
        "1000000000000001",
        "seed-empty",
        vec![
            id(),
            text_field("display_name", 128, false).renamed_from("nickname"),
            bytes_field("payload", 16, true),
            bool_default("enabled"),
            created_at(),
        ],
        vec![index(
            "_dever_idx_user_display_name",
            &["display_name"],
            true,
        )],
        vec![],
        vec![],
    );

    database.migrate(final_model).await.unwrap();
    let state = database_state(&database).await;

    let columns = state
        .columns
        .iter()
        .map(|row| text(row, 0))
        .collect::<Vec<_>>();
    assert_eq!(
        columns,
        ["id", "display_name", "payload", "enabled", "created_at"]
    );
    assert!(!columns.contains(&"nickname"));
    assert_eq!(state.history.len(), 3);
    assert!(
        state
            .history
            .iter()
            .any(|row| { text(row, 0) == "schema" && text(row, 2) == "1000000000000001" })
    );
    assert!(state.catalog.iter().any(|row| {
        text(row, 0) == "table"
            && text(row, 2).contains("length(\"display_name\") <= 128")
            && text(row, 2).contains("DEFAULT 1")
    }));
    assert!(
        database
            .query(
                "SELECT name FROM sqlite_schema WHERE type = 'table' AND name LIKE '_dever_user_%'",
                vec![],
            )
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rename_widen_and_add_fields_preserve_existing_data_and_are_idempotent() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("automatic.db"), 1, 50).unwrap();
    let initial = model(
        "2000000000000001",
        "seed-empty",
        vec![
            id(),
            text_field("nickname", 64, false),
            bytes_field("payload", 8, false),
            created_at(),
        ],
        vec![],
        vec![],
        vec![],
    );
    database.migrate(initial).await.unwrap();
    database
        .execute_batch(
            "INSERT INTO user (nickname, payload) VALUES ('Administrator', X'0102')".into(),
        )
        .await
        .unwrap();

    let upgraded = || {
        model(
            "2000000000000002",
            "seed-empty",
            vec![
                id(),
                text_field("display_name", 128, false).renamed_from("nickname"),
                bytes_field("payload", 16, false),
                text_field("bio", 32, true),
                bool_default("enabled"),
                created_at(),
            ],
            vec![],
            vec![],
            vec![],
        )
    };
    database.migrate(upgraded()).await.unwrap();
    let once = database_state(&database).await;
    database.migrate(upgraded()).await.unwrap();
    assert_eq!(database_state(&database).await, once);

    let rows = database
        .query(
            "SELECT display_name, payload, bio, enabled FROM user",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        rows[0].get(0).unwrap(),
        &Value::Text("Administrator".into())
    );
    assert_eq!(rows[0].get(1).unwrap(), &Value::Bytes(vec![1, 2]));
    assert_eq!(rows[0].get(2).unwrap(), &Value::Null);
    assert_eq!(rows[0].get(3).unwrap(), &Value::Int(1));
}

#[tokio::test]
async fn text_and_bytes_tightening_succeeds_or_rolls_back_as_one_unit() {
    let root = support::temp::TemporaryDirectory::new();
    let valid = Database::open(root.path().join("narrow-valid.db"), 1, 50).unwrap();
    valid
        .migrate(text_model("3000000000000001", 16, 8))
        .await
        .unwrap();
    valid
        .execute_batch("INSERT INTO user (name, payload) VALUES ('short', X'0102')".into())
        .await
        .unwrap();
    valid
        .migrate(text_model("3000000000000002", 5, 2))
        .await
        .unwrap();
    let valid_state = database_state(&valid).await;
    assert!(valid_state.catalog.iter().any(|row| {
        text(row, 0) == "table"
            && text(row, 2).contains("length(\"name\") <= 5")
            && text(row, 2).contains("length(\"payload\") <= 2")
    }));

    let invalid = Database::open(root.path().join("narrow-invalid.db"), 1, 50).unwrap();
    invalid
        .migrate(text_model("3000000000000001", 16, 8))
        .await
        .unwrap();
    invalid
        .execute_batch("INSERT INTO user (name, payload) VALUES ('too-long', X'010203')".into())
        .await
        .unwrap();
    let before = database_state(&invalid).await;
    assert_eq!(
        invalid
            .migrate(text_model("3000000000000002", 5, 2))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Constraint
    );
    assert_eq!(database_state(&invalid).await, before);
}

#[tokio::test]
async fn not_null_and_unique_changes_validate_existing_rows_before_commit() {
    let root = support::temp::TemporaryDirectory::new();

    let nullable = |revision: &str, required: bool| {
        model(
            revision,
            "seed-empty",
            vec![
                id(),
                text_field("code", 16, false),
                text_field("note", 32, !required),
                created_at(),
            ],
            vec![],
            vec![],
            vec![],
        )
    };
    let nullable_valid = Database::open(root.path().join("nullable-valid.db"), 1, 50).unwrap();
    nullable_valid
        .migrate(nullable("4000000000000001", false))
        .await
        .unwrap();
    nullable_valid
        .execute_batch("INSERT INTO user (code, note) VALUES ('one', 'value')".into())
        .await
        .unwrap();
    nullable_valid
        .migrate(nullable("4000000000000002", true))
        .await
        .unwrap();
    assert_eq!(
        orm_sqlite::integer(&database_state(&nullable_valid).await.columns[2], 2),
        1
    );

    let nullable_invalid = Database::open(root.path().join("nullable-invalid.db"), 1, 50).unwrap();
    nullable_invalid
        .migrate(nullable("4000000000000001", false))
        .await
        .unwrap();
    nullable_invalid
        .execute_batch("INSERT INTO user (code, note) VALUES ('one', NULL)".into())
        .await
        .unwrap();
    let before = database_state(&nullable_invalid).await;
    assert_eq!(
        nullable_invalid
            .migrate(nullable("4000000000000002", true))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Constraint
    );
    assert_eq!(database_state(&nullable_invalid).await, before);

    let unique = |revision: &str, enabled: bool| {
        model(
            revision,
            "seed-empty",
            vec![id(), text_field("code", 16, false), created_at()],
            enabled
                .then(|| index("_dever_idx_user_code", &["code"], true))
                .into_iter()
                .collect(),
            vec![],
            vec![],
        )
    };
    let unique_valid = Database::open(root.path().join("unique-valid.db"), 1, 50).unwrap();
    unique_valid
        .migrate(unique("4000000000000011", false))
        .await
        .unwrap();
    unique_valid
        .execute_batch("INSERT INTO user (code) VALUES ('one'), ('two')".into())
        .await
        .unwrap();
    unique_valid
        .migrate(unique("4000000000000012", true))
        .await
        .unwrap();
    assert!(
        database_state(&unique_valid)
            .await
            .catalog
            .iter()
            .any(|row| {
                text(row, 1) == "_dever_idx_user_code"
                    && text(row, 2).starts_with("CREATE UNIQUE INDEX")
            })
    );

    let unique_invalid = Database::open(root.path().join("unique-invalid.db"), 1, 50).unwrap();
    unique_invalid
        .migrate(unique("4000000000000011", false))
        .await
        .unwrap();
    unique_invalid
        .execute_batch("INSERT INTO user (code) VALUES ('same'), ('same')".into())
        .await
        .unwrap();
    let before = database_state(&unique_invalid).await;
    assert_eq!(
        unique_invalid
            .migrate(unique("4000000000000012", true))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Constraint
    );
    assert_eq!(database_state(&unique_invalid).await, before);
}

#[tokio::test]
async fn adding_a_foreign_key_accepts_valid_rows_and_rolls_back_orphans() {
    let root = support::temp::TemporaryDirectory::new();
    let schema = |revision: &str, foreign_key: bool| {
        let owner = if foreign_key {
            TestField::new(
                "owner_id",
                "id:app.model.team",
                false,
                "\"owner_id\" INTEGER NOT NULL REFERENCES \"team\" (\"id\")",
            )
        } else {
            TestField::new("owner_id", "Int", false, "\"owner_id\" INTEGER NOT NULL")
        };
        model(
            revision,
            "seed-empty",
            vec![id(), owner, created_at()],
            vec![],
            vec![],
            vec![],
        )
    };

    let valid = Database::open(root.path().join("foreign-key-valid.db"), 1, 50).unwrap();
    valid
        .execute_batch(
            "CREATE TABLE team (id INTEGER PRIMARY KEY); INSERT INTO team (id) VALUES (1)".into(),
        )
        .await
        .unwrap();
    valid
        .migrate(schema("5000000000000001", false))
        .await
        .unwrap();
    valid
        .execute_batch("INSERT INTO user (owner_id) VALUES (1)".into())
        .await
        .unwrap();
    valid
        .migrate(schema("5000000000000002", true))
        .await
        .unwrap();
    let foreign_keys = valid
        .query(
            "SELECT \"table\", \"from\", \"to\" FROM pragma_foreign_key_list('user')",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(foreign_keys.len(), 1);
    assert_eq!(text(&foreign_keys[0], 0), "team");
    assert_eq!(text(&foreign_keys[0], 1), "owner_id");
    assert_eq!(text(&foreign_keys[0], 2), "id");
    drop(valid);

    let invalid = Database::open(root.path().join("foreign-key-invalid.db"), 1, 50).unwrap();
    invalid
        .execute_batch("CREATE TABLE team (id INTEGER PRIMARY KEY)".into())
        .await
        .unwrap();
    invalid
        .migrate(schema("5000000000000001", false))
        .await
        .unwrap();
    invalid
        .execute_batch("INSERT INTO user (owner_id) VALUES (99)".into())
        .await
        .unwrap();
    let before = database_state(&invalid).await;
    assert_eq!(
        invalid
            .migrate(schema("5000000000000002", true))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&invalid).await, before);
    drop(invalid);

    let cross_family = Database::open(root.path().join("foreign-key-type.db"), 1, 50).unwrap();
    cross_family
        .execute_batch("CREATE TABLE team (id INTEGER PRIMARY KEY)".into())
        .await
        .unwrap();
    cross_family
        .migrate(model(
            "5000000000000011",
            "seed-empty",
            vec![
                id(),
                TestField::new("owner_id", "Float", false, "\"owner_id\" REAL NOT NULL"),
                created_at(),
            ],
            vec![],
            vec![],
            vec![],
        ))
        .await
        .unwrap();
    cross_family
        .execute_batch("INSERT INTO user (owner_id) VALUES (1.0)".into())
        .await
        .unwrap();
    let before = database_state(&cross_family).await;
    assert_eq!(
        cross_family
            .migrate(schema("5000000000000012", true))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&cross_family).await, before);
}

#[tokio::test]
async fn named_drop_runs_once_and_applied_content_cannot_change_or_disappear() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("named-drop.db"), 1, 50).unwrap();
    let initial = || {
        model(
            "6000000000000001",
            "seed-empty",
            vec![
                id(),
                text_field("code", 16, false),
                text_field("legacy", 32, true),
                created_at(),
            ],
            vec![],
            vec![],
            vec![],
        )
    };
    database.migrate(initial()).await.unwrap();
    database
        .execute_batch("INSERT INTO user (code, legacy) VALUES ('one', 'old')".into())
        .await
        .unwrap();

    let dropped = |migration_revision: &str, include_migration: bool| {
        model(
            "6000000000000002",
            "seed-empty",
            vec![id(), text_field("code", 16, false), created_at()],
            vec![],
            include_migration
                .then(|| migration("remove_legacy", migration_revision, &["legacy"]))
                .into_iter()
                .collect(),
            vec![],
        )
    };
    let before_drop = database_state(&database).await;
    assert_eq!(
        database
            .migrate(dropped("remove-legacy-v1", false))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&database).await, before_drop);

    database
        .migrate(dropped("remove-legacy-v1", true))
        .await
        .unwrap();
    let applied = database_state(&database).await;
    assert_eq!(
        applied
            .columns
            .iter()
            .map(|row| text(row, 0))
            .collect::<Vec<_>>(),
        ["id", "code", "created_at"]
    );
    assert!(applied.history.iter().any(|row| {
        text(row, 0) == "migration"
            && text(row, 1) == "remove_legacy"
            && text(row, 2) == "remove-legacy-v1"
    }));

    database
        .migrate(dropped("remove-legacy-v1", true))
        .await
        .unwrap();
    assert_eq!(database_state(&database).await, applied);
    assert_eq!(
        database
            .migrate(dropped("remove-legacy-v2", true))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&database).await, applied);
    assert_eq!(
        database
            .migrate(dropped("remove-legacy-v1", false))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&database).await, applied);
}

#[tokio::test]
async fn catalog_drift_is_rejected_without_rewriting_history_or_data() {
    let root = support::temp::TemporaryDirectory::new();

    let indexed = || {
        model(
            "8000000000000001",
            "seed-empty",
            vec![id(), text_field("code", 16, false), created_at()],
            vec![index("_dever_idx_user_code", &["code"], true)],
            vec![],
            vec![],
        )
    };
    let index_drift = Database::open(root.path().join("index-drift.db"), 1, 50).unwrap();
    index_drift.migrate(indexed()).await.unwrap();
    index_drift
        .execute_batch(
            "INSERT INTO user (code) VALUES ('one'); \
             DROP INDEX _dever_idx_user_code; \
             CREATE INDEX _dever_idx_user_code ON user (code)"
                .into(),
        )
        .await
        .unwrap();
    let before = database_state(&index_drift).await;
    assert_eq!(
        index_drift.migrate(indexed()).await.unwrap_err().kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&index_drift).await, before);

    let table_drift = Database::open(root.path().join("table-drift.db"), 1, 50).unwrap();
    table_drift.migrate(indexed()).await.unwrap();
    table_drift
        .execute_batch(
            "INSERT INTO user (code) VALUES ('one'); ALTER TABLE user ADD COLUMN rogue TEXT".into(),
        )
        .await
        .unwrap();
    let before = database_state(&table_drift).await;
    assert_eq!(
        table_drift.migrate(indexed()).await.unwrap_err().kind(),
        ErrorKind::Migration
    );
    assert_eq!(database_state(&table_drift).await, before);
}
