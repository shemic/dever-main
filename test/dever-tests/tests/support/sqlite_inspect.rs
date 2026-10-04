//! Inspect only an explicitly supplied, test-owned SQLite file between roots.
use dever_runtime::orm::Value;
use std::path::Path;

pub fn query(path: &Path, sql: &'static str) -> Vec<Vec<Value>> {
    dever_runtime::task::run_entry(async {
        let database = dever_runtime::sqlite::Database::open(path, 1, 100)
            .map_err(|error| error.to_string())?;
        let rows = database
            .query(sql, vec![])
            .await
            .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| row.into_values().collect())
            .collect())
    })
    .unwrap()
}

pub fn execute(path: &Path, sql: &'static str) {
    dever_runtime::task::run_entry(async {
        let database = dever_runtime::sqlite::Database::open(path, 1, 100)
            .map_err(|error| error.to_string())?;
        database
            .execute(sql, vec![])
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    })
    .unwrap();
}
