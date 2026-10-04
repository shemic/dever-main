use std::collections::{BTreeMap, BTreeSet};

use deadpool_sqlite::rusqlite::{Connection, OptionalExtension, params};

use crate::database::{Field, Migration, MigrationPhase, Model, Schema};
use crate::orm::Error;

use super::{database_error, interact_error, sqlite_values};

const CREATE_HISTORY: &str = "\
CREATE TABLE IF NOT EXISTS _dever_history (\
  model TEXT NOT NULL,\
  kind TEXT NOT NULL,\
  name TEXT NOT NULL,\
  revision TEXT NOT NULL,\
  definition TEXT NOT NULL,\
  applied_at INTEGER NOT NULL DEFAULT (unixepoch()),\
  PRIMARY KEY (model, kind, name)\
)";

pub(super) async fn apply(connection: &deadpool_sqlite::Object, model: Model) -> Result<(), Error> {
    connection
        .interact(move |connection| transaction(connection, model))
        .await
        .map_err(interact_error)?
}

fn transaction(connection: &mut Connection, model: Model) -> Result<(), Error> {
    connection
        .execute_batch("BEGIN IMMEDIATE; PRAGMA defer_foreign_keys = ON;")
        .map_err(database_error)?;
    let result = apply_locked(connection, &model);
    let result = match result {
        Ok(()) => match connection.execute_batch("COMMIT") {
            Ok(()) => Ok(()),
            Err(commit) => rollback(connection, database_error(commit)),
        },
        Err(error) => rollback(connection, error),
    };
    connection.flush_prepared_statement_cache();
    result
}

fn rollback(connection: &Connection, error: Error) -> Result<(), Error> {
    match connection.execute_batch("ROLLBACK") {
        Ok(()) => Err(error),
        Err(rollback) => Err(error.cause(rollback)),
    }
}

fn apply_locked(connection: &Connection, model: &Model) -> Result<(), Error> {
    connection
        .execute_batch(CREATE_HISTORY)
        .map_err(database_error)?;
    let table_exists = table_exists(connection, &model.schema.table)?;
    let previous = load_schema(connection, &model.schema.model)?;
    match (table_exists, previous.as_ref()) {
        (false, None) => create_model(connection, model)?,
        (true, Some(previous)) => update_model(connection, model, previous)?,
        (true, None) => {
            return Err(Error::migration(format!(
                "table '{}' exists without Dever schema history",
                model.schema.table
            )));
        }
        (false, Some(_)) => {
            return Err(Error::migration(format!(
                "Dever schema history exists but table '{}' is missing",
                model.schema.table
            )));
        }
    }
    apply_seed(connection, model, previous.as_ref())?;
    store_history(connection, &model.schema)?;
    Ok(())
}

fn create_model(connection: &Connection, model: &Model) -> Result<(), Error> {
    connection
        .execute_batch(&model.create_table.sqlite)
        .map_err(database_error)?;
    create_indexes(connection, &model.create_indexes)
}

fn update_model(connection: &Connection, model: &Model, previous: &Schema) -> Result<(), Error> {
    if previous.table != model.schema.table {
        return Err(Error::migration(format!(
            "Model '{}' cannot rename table '{}' to '{}' automatically",
            model.schema.model, previous.table, model.schema.table
        )));
    }
    validate_migrations(connection, &model.schema)?;
    crate::database::validate_migration_order(previous, &model.schema)?;
    validate_catalog(connection, previous)?;
    let pending = unapplied_migrations(connection, &model.schema)?;
    apply_data(connection, model, &pending, MigrationPhase::Before)?;
    if previous.revision == model.schema.revision {
        create_indexes(connection, &model.create_indexes)?;
    } else if fields_match(&previous.fields, &model.schema.fields) {
        synchronize_indexes(connection, previous, model)?;
    } else {
        rebuild_table(connection, previous, model, &pending)?;
    }
    apply_data(connection, model, &pending, MigrationPhase::After)
}

fn apply_data(
    connection: &Connection,
    model: &Model,
    pending: &[Migration],
    phase: MigrationPhase,
) -> Result<(), Error> {
    for statement in &model.data_migrations {
        if statement.phase != phase
            || !pending
                .iter()
                .any(|migration| migration.name == statement.name)
        {
            continue;
        }
        let parameters = sqlite_values(statement.parameters.clone());
        connection
            .execute(
                statement.sql.sqlite,
                deadpool_sqlite::rusqlite::params_from_iter(parameters),
            )
            .map_err(database_error)?;
    }
    Ok(())
}

fn rebuild_table(
    connection: &Connection,
    previous: &Schema,
    model: &Model,
    new_migrations: &[Migration],
) -> Result<(), Error> {
    let allowed_drops = new_migrations
        .iter()
        .flat_map(|migration| migration.drops.iter().cloned())
        .collect::<BTreeSet<_>>();
    let previous_fields = previous
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let mut copied = Vec::new();
    let mut consumed = BTreeSet::new();
    for target in &model.schema.fields {
        let source = previous_fields
            .get(target.name.as_str())
            .copied()
            .or_else(|| {
                target
                    .rename_from
                    .as_deref()
                    .and_then(|name| previous_fields.get(name).copied())
            });
        let Some(source) = source else {
            continue;
        };
        if !compatible_types(&source.ty, &target.ty) {
            return Err(Error::migration(format!(
                "field '{}.{}' cannot change type from '{}' to '{}' automatically",
                model.schema.model, target.name, source.ty, target.ty
            )));
        }
        if !consumed.insert(source.name.as_str()) {
            return Err(Error::migration(format!(
                "field '{}.{}' reuses migration source '{}'",
                model.schema.model, target.name, source.name
            )));
        }
        copied.push((target.name.as_str(), source.name.as_str()));
    }
    for source in &previous.fields {
        if !consumed.contains(source.name.as_str()) && !allowed_drops.contains(&source.name) {
            return Err(Error::migration(format!(
                "field '{}.{}' was removed without a named migrate drop",
                model.schema.model, source.name
            )));
        }
    }

    connection
        .execute_batch(&model.create_temporary_table)
        .map_err(database_error)?;
    let targets = copied
        .iter()
        .map(|(target, _)| quote(target))
        .collect::<Vec<_>>()
        .join(", ");
    let sources = copied
        .iter()
        .map(|(_, source)| quote(source))
        .collect::<Vec<_>>()
        .join(", ");
    let temporary = temporary_table(&model.schema);
    connection
        .execute_batch(&format!(
            "INSERT INTO {} ({targets}) SELECT {sources} FROM {};\nDROP TABLE {};\nALTER TABLE {} RENAME TO {};",
            quote(&temporary),
            quote(&model.schema.table),
            quote(&model.schema.table),
            quote(&temporary),
            quote(&model.schema.table),
        ))
        .map_err(database_error)?;
    create_indexes(connection, &model.create_indexes)?;
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(database_error)?;
    if foreign_keys
        .query([])
        .map_err(database_error)?
        .next()
        .map_err(database_error)?
        .is_some()
    {
        return Err(Error::migration(format!(
            "Model '{}' migration would leave invalid foreign keys",
            model.schema.model
        )));
    }
    Ok(())
}

fn synchronize_indexes(
    connection: &Connection,
    previous: &Schema,
    model: &Model,
) -> Result<(), Error> {
    for index in &previous.indexes {
        connection
            .execute_batch(&format!("DROP INDEX IF EXISTS {}", quote(&index.name)))
            .map_err(database_error)?;
    }
    create_indexes(connection, &model.create_indexes)
}

fn create_indexes(
    connection: &Connection,
    indexes: &[crate::database::SchemaSql],
) -> Result<(), Error> {
    for index in indexes {
        connection
            .execute_batch(&index.sqlite)
            .map_err(database_error)?;
    }
    Ok(())
}

fn apply_seed(
    connection: &Connection,
    model: &Model,
    previous: Option<&Schema>,
) -> Result<(), Error> {
    if previous.is_some_and(|schema| schema.seed_revision == model.schema.seed_revision) {
        return Ok(());
    }
    for seed in &model.seeds {
        let parameters = sqlite_values(seed.parameters.clone());
        connection
            .execute(
                seed.sql.sqlite,
                deadpool_sqlite::rusqlite::params_from_iter(parameters),
            )
            .map_err(database_error)?;
    }
    Ok(())
}

fn validate_catalog(connection: &Connection, schema: &Schema) -> Result<(), Error> {
    let recorded = load_catalog(connection, &schema.model)?.ok_or_else(|| {
        Error::migration(format!(
            "Model '{}' has no recorded database catalog",
            schema.model
        ))
    })?;
    if catalog(connection, &schema.table)? != recorded {
        return Err(Error::migration(format!(
            "table '{}' does not match its recorded Dever schema",
            schema.table
        )));
    }
    Ok(())
}

fn validate_migrations(connection: &Connection, schema: &Schema) -> Result<(), Error> {
    let applied = load_migrations(connection, &schema.model)?;
    let desired = schema
        .migrations
        .iter()
        .map(|migration| (migration.name.as_str(), migration))
        .collect::<BTreeMap<_, _>>();
    for (name, revision) in applied {
        let Some(migration) = desired.get(name.as_str()) else {
            return Err(Error::migration(format!(
                "applied migration '{}.{}' was removed from source",
                schema.model, name
            )));
        };
        if migration.revision != revision {
            return Err(Error::migration(format!(
                "applied migration '{}.{}' was modified",
                schema.model, name
            )));
        }
    }
    Ok(())
}

fn unapplied_migrations(connection: &Connection, schema: &Schema) -> Result<Vec<Migration>, Error> {
    let applied = load_migrations(connection, &schema.model)?;
    Ok(schema
        .migrations
        .iter()
        .filter(|migration| !applied.contains_key(&migration.name))
        .cloned()
        .collect())
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, Error> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(database_error)
}

fn load_schema(connection: &Connection, model: &str) -> Result<Option<Schema>, Error> {
    let definition = connection
        .query_row(
            "SELECT definition FROM _dever_history WHERE model = ?1 AND kind = 'schema' AND name = 'current'",
            [model],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?;
    definition
        .map(|definition| {
            serde_json::from_str(&definition)
                .map_err(|error| Error::migration(format!("invalid Dever schema history: {error}")))
        })
        .transpose()
}

fn load_catalog(connection: &Connection, model: &str) -> Result<Option<String>, Error> {
    connection
        .query_row(
            "SELECT definition FROM _dever_history WHERE model = ?1 AND kind = 'catalog' AND name = 'current'",
            [model],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
}

fn catalog(connection: &Connection, table: &str) -> Result<String, Error> {
    let table_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get::<_, String>(0),
        )
        .map_err(database_error)?;
    let mut statement = connection
        .prepare(
            "SELECT name, sql FROM sqlite_schema WHERE type = 'index' AND tbl_name = ?1 AND substr(name, 1, 11) = '_dever_idx_' ORDER BY name",
        )
        .map_err(database_error)?;
    let indexes = statement
        .query_map([table], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    serde_json::to_string(&(table_sql, indexes))
        .map_err(|error| Error::migration(format!("cannot encode SQLite catalog: {error}")))
}

fn load_migrations(
    connection: &Connection,
    model: &str,
) -> Result<BTreeMap<String, String>, Error> {
    let mut statement = connection
        .prepare(
            "SELECT name, revision FROM _dever_history WHERE model = ?1 AND kind = 'migration'",
        )
        .map_err(database_error)?;
    statement
        .query_map([model], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(database_error)?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(database_error)
}

fn store_history(connection: &Connection, schema: &Schema) -> Result<(), Error> {
    let definition = serde_json::to_string(schema).map_err(|error| {
        Error::migration(format!("cannot encode Dever schema history: {error}"))
    })?;
    store_history_row(
        connection,
        &schema.model,
        "schema",
        "current",
        &schema.revision,
        &definition,
    )?;
    for migration in &schema.migrations {
        store_history_row(
            connection,
            &schema.model,
            "migration",
            &migration.name,
            &migration.revision,
            "",
        )?;
    }
    store_history_row(
        connection,
        &schema.model,
        "seed",
        "current",
        &schema.seed_revision,
        "",
    )?;
    store_history_row(
        connection,
        &schema.model,
        "catalog",
        "current",
        &schema.revision,
        &catalog(connection, &schema.table)?,
    )
}

fn store_history_row(
    connection: &Connection,
    model: &str,
    kind: &str,
    name: &str,
    revision: &str,
    definition: &str,
) -> Result<(), Error> {
    connection
        .execute(
            "INSERT INTO _dever_history (model, kind, name, revision, definition) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (model, kind, name) DO UPDATE SET revision = excluded.revision, definition = excluded.definition, applied_at = unixepoch() WHERE _dever_history.kind <> 'migration'",
            params![model, kind, name, revision, definition],
        )
        .map(|_| ())
        .map_err(database_error)
}

fn fields_match(previous: &[Field], desired: &[Field]) -> bool {
    previous == desired
}

fn compatible_types(previous: &str, desired: &str) -> bool {
    previous == desired
        || (previous.starts_with("Text") && desired.starts_with("Text"))
        || (previous.starts_with("Bytes") && desired.starts_with("Bytes"))
        || (previous == "Int" && desired == "Float")
        || (sqlite_integer_reference(previous) && sqlite_integer_reference(desired))
}

fn sqlite_integer_reference(ty: &str) -> bool {
    ty == "Int" || ty.starts_with("id:")
}

pub fn temporary_table(schema: &Schema) -> String {
    format!("_dever_{}_{}", schema.table, &schema.revision[..8])
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
