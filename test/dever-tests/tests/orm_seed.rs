#[path = "support/orm_sqlite.rs"]
mod orm_sqlite;
mod support;

use dever_runtime::orm::{ErrorKind, Value};
use dever_runtime::sqlite::Database;
use orm_sqlite::{TestField, created_at, database_state, id, index, model, seed, text, text_field};

const INSERT_SEED: &str = "INSERT INTO \"user\" (\"code\", \"email\", \"label\") VALUES (?1, ?2, ?3) ON CONFLICT (\"code\") DO NOTHING";

fn seed_row(code: &str, email: &str, label: &str) -> dever_runtime::database::Seed {
    seed(
        INSERT_SEED,
        vec![
            Value::Text(code.into()),
            Value::Text(email.into()),
            Value::Text(label.into()),
        ],
    )
}

fn seed_model(seed_revision: &str, rows: &[(&str, &str, &str)]) -> dever_runtime::database::Model {
    model(
        "7000000000000001",
        seed_revision,
        vec![
            id(),
            text_field("code", 32, false),
            text_field("email", 128, false),
            text_field("label", 64, false),
            created_at(),
        ],
        vec![
            index("_dever_idx_user_code", &["code"], true),
            index("_dever_idx_user_email", &["email"], true),
        ],
        vec![],
        rows.iter()
            .map(|(code, email, label)| seed_row(code, email, label))
            .collect(),
    )
}

fn initial_seed() -> dever_runtime::database::Model {
    seed_model("seed-1", &[("pending", "pending@seed.test", "Pending")])
}

fn expanded_seed() -> dever_runtime::database::Model {
    seed_model(
        "seed-2",
        &[
            ("pending", "pending@changed.test", "Changed by source"),
            ("done", "done@seed.test", "Done"),
        ],
    )
}

#[tokio::test]
async fn changed_seed_appends_missing_rows_preserves_business_values_and_fast_skips() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("seed.db"), 1, 50).unwrap();
    database.migrate(initial_seed()).await.unwrap();
    database
        .execute_batch(
            "UPDATE user SET email = 'business@test.local', label = 'Business value' WHERE code = 'pending'"
                .into(),
        )
        .await
        .unwrap();

    database.migrate(expanded_seed()).await.unwrap();
    let rows = database
        .query("SELECT code, email, label FROM user ORDER BY code", vec![])
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(text(&rows[0], 0), "done");
    assert_eq!(text(&rows[0], 1), "done@seed.test");
    assert_eq!(text(&rows[1], 0), "pending");
    assert_eq!(text(&rows[1], 1), "business@test.local");
    assert_eq!(text(&rows[1], 2), "Business value");

    database
        .execute_batch(
            "CREATE TABLE seed_probe (attempt INTEGER NOT NULL); \
             CREATE TRIGGER observe_seed BEFORE INSERT ON user BEGIN \
               INSERT INTO seed_probe (attempt) VALUES (1); \
             END"
            .into(),
        )
        .await
        .unwrap();
    let before = database_state(&database).await;
    database.migrate(expanded_seed()).await.unwrap();
    assert_eq!(database_state(&database).await, before);
    let attempts = database
        .query("SELECT COUNT(*) FROM seed_probe", vec![])
        .await
        .unwrap();
    assert_eq!(attempts[0].get(0).unwrap(), &Value::Int(0));
    assert!(
        before
            .history
            .iter()
            .any(|row| { text(row, 0) == "seed" && text(row, 2) == "seed-2" })
    );
}

#[tokio::test]
async fn non_target_seed_constraint_failure_rolls_back_rows_and_history() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("seed-rollback.db"), 1, 50).unwrap();
    database.migrate(expanded_seed()).await.unwrap();
    database
        .execute_batch("UPDATE user SET label = 'Business done' WHERE code = 'done'".into())
        .await
        .unwrap();
    let before = database_state(&database).await;

    let invalid = seed_model(
        "seed-3",
        &[
            ("pending", "pending@changed.test", "Pending"),
            ("done", "done@seed.test", "Done"),
            ("archived", "archived@seed.test", "Archived"),
            ("queued", "done@seed.test", "Queued"),
        ],
    );
    assert_eq!(
        database.migrate(invalid).await.unwrap_err().kind(),
        ErrorKind::Constraint
    );
    assert_eq!(database_state(&database).await, before);
    assert!(
        before
            .history
            .iter()
            .any(|row| { text(row, 0) == "seed" && text(row, 2) == "seed-2" })
    );
    assert!(!before.data.iter().any(|row| text(row, 1) == "archived"));
    assert!(
        before
            .data
            .iter()
            .any(|row| { text(row, 1) == "done" && text(row, 3) == "Business done" })
    );
}

#[tokio::test]
async fn deferred_constraint_commit_failure_rolls_back_and_releases_the_connection() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("seed-commit-rollback.db"), 1, 50).unwrap();
    database
        .execute_batch(
            "CREATE TABLE team (id INTEGER PRIMARY KEY); INSERT INTO team (id) VALUES (1)".into(),
        )
        .await
        .unwrap();

    let seeded = |revision: &str, owner_id: i64| {
        model(
            "7100000000000001",
            revision,
            vec![
                id(),
                TestField::new(
                    "owner_id",
                    "id:app.model.team",
                    false,
                    "\"owner_id\" INTEGER NOT NULL REFERENCES \"team\" (\"id\") DEFERRABLE INITIALLY DEFERRED",
                ),
                created_at(),
            ],
            vec![index("_dever_idx_user_owner_id", &["owner_id"], true)],
            vec![],
            vec![seed(
                "INSERT INTO \"user\" (\"owner_id\") VALUES (?1) ON CONFLICT (\"owner_id\") DO NOTHING",
                vec![Value::Int(owner_id)],
            )],
        )
    };

    assert_eq!(
        database
            .migrate(seeded("seed-invalid", 99))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Constraint
    );
    assert!(
        database
            .query(
                "SELECT name FROM sqlite_schema WHERE name IN ('user', '_dever_history', '_dever_idx_user_owner_id')",
                vec![],
            )
            .await
            .unwrap()
            .is_empty()
    );

    database.migrate(seeded("seed-valid", 1)).await.unwrap();
    let state = database_state(&database).await;
    assert_eq!(state.data.len(), 1);
    assert_eq!(state.data[0].get(1).unwrap(), &Value::Int(1));
    assert!(
        state
            .history
            .iter()
            .any(|row| { text(row, 0) == "seed" && text(row, 2) == "seed-valid" })
    );
}
