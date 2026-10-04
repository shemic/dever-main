mod support;

use dever_runtime::orm::{ErrorKind, Uuid, Value, decimal, decimal_storage, uuid};
use dever_runtime::sqlite::Database;

#[test]
fn sqlite_invalid_text_preserves_the_column_and_error_kind_in_all_decoders() {
    dever_runtime::task::run_entry(async {
        let root = support::temp::TemporaryDirectory::new();
        let database = Database::open(root.path().join("invalid-text.db"), 1, 4).unwrap();
        let check = |error: dever_runtime::orm::Error| {
            assert_eq!(error.kind(), ErrorKind::Database);
            assert!(error.to_string().contains("index 1"));
        };
        const INVALID_TEXT: &str = "SELECT 7, CAST(x'ff' AS TEXT)";
        check(database.query(INVALID_TEXT, vec![]).await.unwrap_err());
        check(
            database
                .query_bounded(INVALID_TEXT, vec![], 1)
                .await
                .unwrap_err(),
        );
        check(
            database
                .query_owned(INVALID_TEXT.into(), vec![])
                .await
                .unwrap_err(),
        );
        let stream = database.stream(INVALID_TEXT, vec![], 1).await.unwrap();
        check(stream.pull().await.unwrap_err());
        stream.close();
        let rows = database.query("SELECT 1", vec![]).await.unwrap();
        assert_eq!(rows[0].get(0).unwrap(), &Value::Int(1));
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn sqlite_pool_executes_parameterized_queries_and_classifies_constraints() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("app.db"), 1, 50).unwrap();
    assert_eq!(database.max_page_size(), 50);

    database
        .execute_batch(
            "CREATE TABLE user (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                email TEXT NOT NULL UNIQUE, \
                enabled INTEGER NOT NULL\
            )"
            .to_owned(),
        )
        .await
        .unwrap();
    database
        .execute(
            "INSERT INTO user (email, enabled) VALUES (?1, ?2)",
            vec![Value::Text("admin@example.com".into()), Value::Bool(true)],
        )
        .await
        .unwrap();

    let rows = database
        .query(
            "SELECT id, email, enabled FROM user WHERE email = ?1",
            vec![Value::Text("admin@example.com".into())],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get(0).unwrap(), &Value::Int(1));
    assert_eq!(
        rows[0].get(1).unwrap(),
        &Value::Text("admin@example.com".into())
    );
    assert_eq!(rows[0].get(2).unwrap(), &Value::Int(1));

    let duplicate = database
        .execute(
            "INSERT INTO user (email, enabled) VALUES (?1, ?2)",
            vec![Value::Text("admin@example.com".into()), Value::Bool(false)],
        )
        .await
        .unwrap_err();
    assert_eq!(duplicate.kind(), ErrorKind::Constraint);

    let expected_uuid = Uuid::parse("018f08d8-59b0-7cc2-98c4-dc0c0c07398f").unwrap();
    database
        .execute_batch(
            "CREATE TABLE uuid_value (value BLOB NOT NULL CHECK (length(value) = 16))".to_owned(),
        )
        .await
        .unwrap();
    database
        .execute(
            "INSERT INTO uuid_value (value) VALUES (?1)",
            vec![Value::Uuid(expected_uuid)],
        )
        .await
        .unwrap();
    let mut uuid_rows = database
        .query("SELECT value FROM uuid_value", vec![])
        .await
        .unwrap();
    let stored_uuid = uuid(uuid_rows.remove(0).into_values().next().unwrap()).unwrap();
    assert_eq!(stored_uuid, expected_uuid);
}

#[tokio::test]
async fn sqlite_transactions_commit_or_rollback_on_the_owned_connection() {
    let root = support::temp::TemporaryDirectory::new();
    let database = Database::open(root.path().join("transaction.db"), 1, 50).unwrap();
    database
        .execute_batch("CREATE TABLE event (id INTEGER PRIMARY KEY, name TEXT NOT NULL)".to_owned())
        .await
        .unwrap();

    let transaction = database.begin().await.unwrap();
    transaction
        .execute(
            "INSERT INTO event (id, name) VALUES (?1, ?2)",
            vec![Value::Int(1), Value::Text("rolled back".into())],
        )
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert!(
        database
            .query("SELECT id FROM event", vec![])
            .await
            .unwrap()
            .is_empty()
    );

    let transaction = database.begin().await.unwrap();
    transaction
        .execute(
            "INSERT INTO event (id, name) VALUES (?1, ?2)",
            vec![Value::Int(2), Value::Text("committed".into())],
        )
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    let rows = database
        .query("SELECT id, name FROM event", vec![])
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get(0).unwrap(), &Value::Int(2));
    assert_eq!(rows[0].get(1).unwrap(), &Value::Text("committed".into()));

    let abandoned = database.begin().await.unwrap();
    abandoned
        .execute(
            "INSERT INTO event (id, name) VALUES (?1, ?2)",
            vec![Value::Int(3), Value::Text("abandoned".into())],
        )
        .await
        .unwrap();
    drop(abandoned);
    let rows = database
        .query("SELECT id FROM event WHERE id = ?1", vec![Value::Int(3)])
        .await
        .unwrap();
    assert!(rows.is_empty());
}

#[test]
fn decimal_and_uuid_values_have_stable_storage_round_trips() {
    let decimal_values = ["-10", "-2", "-0.01", "0", "0.01", "2", "10"];
    let encoded = decimal_values
        .iter()
        .map(|value| decimal_storage(value, 8, 2).unwrap())
        .collect::<Vec<_>>();
    let mut sorted = encoded.clone();
    sorted.sort();
    assert_eq!(sorted, encoded);
    for (expected, encoded) in decimal_values.into_iter().zip(encoded) {
        assert_eq!(
            decimal(Value::Decimal(encoded)).unwrap(),
            decimal(Value::Text(expected.to_owned())).unwrap()
        );
    }
    assert!(decimal_storage("1.234", 8, 2).is_err());
    assert!(decimal_storage("1000", 3, 0).is_err());
    assert!(decimal_storage("1e2147483647", 8, 2).is_err());
    assert_eq!(
        decimal_storage("0e2147483647", 8, 2).unwrap(),
        "D100000000:02"
    );

    let text = "018f08d8-59b0-7cc2-98c4-dc0c0c07398f";
    let value = Uuid::parse(text).unwrap();
    assert_eq!(value.to_string(), text);
    assert_eq!(uuid(Value::Uuid(value)).unwrap(), value);
    assert!(Uuid::parse("not-a-uuid").is_err());

    let generated = Uuid::new_v7().unwrap();
    let generated_bytes = generated.into_bytes();
    assert_eq!(generated_bytes[6] >> 4, 7);
    assert_eq!(generated_bytes[8] & 0xc0, 0x80);
    assert_eq!(Uuid::parse(&generated.to_string()).unwrap(), generated);
}

#[tokio::test]
async fn cancelled_sqlite_work_does_not_return_an_unknown_connection_to_the_pool() {
    let root = support::temp::TemporaryDirectory::new();
    let database =
        std::sync::Arc::new(Database::open(root.path().join("cancel.db"), 1, 50).unwrap());
    let query_database = database.clone();
    let query = tokio::spawn(async move {
        query_database
            .query(
                "WITH RECURSIVE values_(value) AS (\
                 VALUES(0) UNION ALL SELECT value + 1 FROM values_ WHERE value < 100000000\
                 ) SELECT sum(value) FROM values_",
                vec![],
            )
            .await
    });
    tokio::task::yield_now().await;
    query.abort();
    let _ = query.await;

    let rows = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        database.query("SELECT 1", vec![]),
    )
    .await
    .expect("the pool must replace the cancelled connection")
    .unwrap();
    assert_eq!(rows[0].get(0).unwrap(), &Value::Int(1));
}

#[test]
fn sqlite_row_stream_is_bounded_and_releases_its_connection_when_closed_early_or_dropped() {
    dever_runtime::task::run_entry(async {
        let root = support::temp::TemporaryDirectory::new();
        let database = Database::open(root.path().join("stream.db"), 1, 4).unwrap();
        database
            .execute_batch(
                "CREATE TABLE event (id INTEGER PRIMARY KEY, name TEXT NOT NULL);\
                 INSERT INTO event (id, name) VALUES \
                 (1, 'one'), (2, 'two'), (3, 'three'), (4, 'four'), (5, 'five')"
                    .to_owned(),
            )
            .await
            .unwrap();

        let stream = database
            .stream("SELECT id, name FROM event ORDER BY id", vec![], 2)
            .await
            .unwrap();
        let first = stream.pull().await.unwrap().unwrap();
        assert_eq!(first.get(0).unwrap(), &Value::Int(1));

        let waiting_for_connection = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            database.query("SELECT count(*) FROM event", vec![]),
        )
        .await;
        assert!(
            waiting_for_connection.is_err(),
            "the stream must retain the only connection until it is closed"
        );
        stream.close();

        let rows = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            database.query("SELECT count(*) FROM event", vec![]),
        )
        .await
        .expect("closing the row stream must return its only connection")
        .unwrap();
        assert_eq!(rows[0].get(0).unwrap(), &Value::Int(5));

        let stream = database
            .stream("SELECT id FROM event ORDER BY id", vec![], 1)
            .await
            .unwrap();
        let mut ids = Vec::new();
        while let Some(row) = stream.pull().await.unwrap() {
            ids.push(row.get(0).unwrap().clone());
        }
        assert_eq!(
            ids,
            vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
                Value::Int(5),
            ]
        );

        let stream = database
            .stream("SELECT id FROM event ORDER BY id", vec![], 1)
            .await
            .unwrap();
        assert!(stream.pull().await.unwrap().is_some());
        drop(stream);
        let rows = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            database.query("SELECT count(*) FROM event", vec![]),
        )
        .await
        .expect("dropping the row stream must return its only connection")
        .unwrap();
        assert_eq!(rows[0].get(0).unwrap(), &Value::Int(5));
        Ok(())
    })
    .unwrap();
}
