use std::collections::{BTreeMap, BTreeSet};

use tokio_postgres::types::ToSql;

use crate::database::{MigrationPhase, Model, PostgresConstraint, Schema};
use crate::orm::Error;

use super::{database_error, execute_on};

const CREATE_HISTORY: &str = "\
CREATE TABLE IF NOT EXISTS _dever_history (\
  model TEXT NOT NULL,\
  kind TEXT NOT NULL,\
  name TEXT NOT NULL,\
  revision TEXT NOT NULL,\
  definition TEXT NOT NULL,\
  applied_at BIGINT NOT NULL DEFAULT ((extract(epoch from clock_timestamp()) * 1000)::bigint),\
  PRIMARY KEY (model, kind, name)\
)";

pub(super) async fn apply(
    connection: &deadpool_postgres::Object,
    model: Model,
) -> Result<(), Error> {
    connection
        .batch_execute("BEGIN")
        .await
        .map_err(database_error)?;
    let result = apply_locked(connection, &model).await;
    match result {
        Ok(()) => connection
            .batch_execute("COMMIT")
            .await
            .map_err(database_error),
        Err(error) => match connection.batch_execute("ROLLBACK").await {
            Ok(()) => Err(error),
            Err(rollback) => Err(error.cause(rollback)),
        },
    }
}

pub(super) async fn apply_foreign_keys(
    connection: &deadpool_postgres::Object,
    model: &str,
    table: &str,
    constraints: &[PostgresConstraint],
) -> Result<(), Error> {
    connection
        .batch_execute("BEGIN")
        .await
        .map_err(database_error)?;
    let result = apply_foreign_keys_locked(connection, model, table, constraints).await;
    match result {
        Ok(()) => connection
            .batch_execute("COMMIT")
            .await
            .map_err(database_error),
        Err(error) => match connection.batch_execute("ROLLBACK").await {
            Ok(()) => Err(error),
            Err(rollback) => Err(error.cause(rollback)),
        },
    }
}

async fn apply_foreign_keys_locked(
    connection: &deadpool_postgres::Object,
    model: &str,
    table: &str,
    constraints: &[PostgresConstraint],
) -> Result<(), Error> {
    connection
        .query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            &[&model],
        )
        .await
        .map_err(database_error)?;
    for constraint in constraints {
        let exists = connection
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class rel ON rel.oid = con.conrelid JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace WHERE nsp.nspname = current_schema() AND rel.relname = $1 AND con.conname = $2 AND con.contype = 'f')",
                &[&table, &constraint.name],
            )
            .await
            .map_err(database_error)?
            .get::<_, bool>(0);
        if !exists {
            alter_table(
                connection,
                table,
                &format!(
                    "ADD CONSTRAINT {} {}",
                    quote(&constraint.name),
                    constraint.definition
                ),
            )
            .await?;
        }
    }
    store_catalog_history(connection, model, table).await
}

async fn apply_locked(connection: &deadpool_postgres::Object, model: &Model) -> Result<(), Error> {
    connection
        .query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            &[&model.schema.model],
        )
        .await
        .map_err(database_error)?;
    connection
        .batch_execute(CREATE_HISTORY)
        .await
        .map_err(database_error)?;
    let table_exists = table_exists(connection, &model.schema.table).await?;
    let previous = load_schema(connection, &model.schema.model).await?;
    let mut pending = BTreeSet::new();
    match (table_exists, previous.as_ref()) {
        (false, None) => {
            connection
                .batch_execute(&model.create_table.postgres)
                .await
                .map_err(database_error)?;
        }
        (true, Some(previous)) => {
            validate_migrations(connection, &model.schema).await?;
            crate::database::validate_migration_order(previous, &model.schema)?;
            validate_catalog(connection, previous).await?;
            let applied = load_migrations(connection, &model.schema.model).await?;
            pending = model
                .schema
                .migrations
                .iter()
                .filter(|migration| !applied.contains_key(&migration.name))
                .map(|migration| migration.name.as_str())
                .collect();
            apply_data(connection, model, &pending, MigrationPhase::Before).await?;
            if previous.revision != model.schema.revision {
                migrate_schema(connection, previous, model).await?;
            }
        }
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
    for index in &model.create_indexes {
        connection
            .batch_execute(&index.postgres)
            .await
            .map_err(database_error)?;
    }
    apply_data(connection, model, &pending, MigrationPhase::After).await?;
    if previous
        .as_ref()
        .is_none_or(|schema| schema.seed_revision != model.schema.seed_revision)
    {
        for seed in &model.seeds {
            execute_on(connection, seed.sql.postgres, seed.parameters.clone()).await?;
        }
    }
    store_history(connection, &model.schema).await
}

async fn apply_data(
    connection: &deadpool_postgres::Object,
    model: &Model,
    pending: &BTreeSet<&str>,
    phase: MigrationPhase,
) -> Result<(), Error> {
    for statement in &model.data_migrations {
        if statement.phase == phase && pending.contains(statement.name.as_str()) {
            execute_on(
                connection,
                statement.sql.postgres,
                statement.parameters.clone(),
            )
            .await?;
        }
    }
    Ok(())
}

async fn migrate_schema(
    connection: &deadpool_postgres::Object,
    previous: &Schema,
    model: &Model,
) -> Result<(), Error> {
    let applied = load_migrations(connection, &model.schema.model).await?;
    let allowed_drops = model
        .schema
        .migrations
        .iter()
        .filter(|migration| !applied.contains_key(&migration.name))
        .flat_map(|migration| migration.drops.iter().cloned())
        .collect::<BTreeSet<_>>();
    let previous_fields = previous
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let columns = model
        .postgres_columns
        .iter()
        .map(|column| (column.name.as_str(), column))
        .collect::<BTreeMap<_, _>>();
    let mut mapped = Vec::new();
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
        if let Some(source) = source {
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
        }
        mapped.push((target, source));
    }
    for source in &previous.fields {
        if !consumed.contains(source.name.as_str()) && !allowed_drops.contains(&source.name) {
            return Err(Error::migration(format!(
                "field '{}.{}' was removed without a named migrate drop",
                model.schema.model, source.name
            )));
        }
    }

    drop_dever_constraints(connection, &model.schema.table).await?;
    for (target, source) in &mapped {
        if let Some(source) = source
            && source.name != target.name
        {
            alter_table(
                connection,
                &model.schema.table,
                &format!(
                    "RENAME COLUMN {} TO {}",
                    quote(&source.name),
                    quote(&target.name)
                ),
            )
            .await?;
        }
    }
    for source in &previous.fields {
        if !consumed.contains(source.name.as_str()) {
            alter_table(
                connection,
                &model.schema.table,
                &format!("DROP COLUMN {}", quote(&source.name)),
            )
            .await?;
        }
    }
    for (target, source) in mapped {
        let column = columns
            .get(target.name.as_str())
            .expect("generated PostgreSQL column");
        let Some(source) = source else {
            alter_table(
                connection,
                &model.schema.table,
                &format!("ADD COLUMN {}", column.definition),
            )
            .await?;
            continue;
        };
        let previous_type = postgres_type(&source.ty);
        if previous_type != column.sql_type {
            alter_table(
                connection,
                &model.schema.table,
                &format!(
                    "ALTER COLUMN {} TYPE {} USING {}::{}",
                    quote(&target.name),
                    column.sql_type,
                    quote(&target.name),
                    column.sql_type,
                ),
            )
            .await?;
        }
        if source.nullable != target.nullable {
            alter_table(
                connection,
                &model.schema.table,
                &format!(
                    "ALTER COLUMN {} {} NOT NULL",
                    quote(&target.name),
                    if target.nullable { "DROP" } else { "SET" },
                ),
            )
            .await?;
        }
        if source.default != target.default || source.generated != target.generated {
            let action = column
                .default
                .as_ref()
                .map(|default| format!("SET DEFAULT {default}"))
                .unwrap_or_else(|| "DROP DEFAULT".into());
            alter_table(
                connection,
                &model.schema.table,
                &format!("ALTER COLUMN {} {action}", quote(&target.name)),
            )
            .await?;
        }
    }
    for constraint in model
        .postgres_constraints
        .iter()
        .filter(|constraint| !constraint.foreign_key)
    {
        alter_table(
            connection,
            &model.schema.table,
            &format!(
                "ADD CONSTRAINT {} {}",
                quote(&constraint.name),
                constraint.definition
            ),
        )
        .await?;
    }
    for index in &previous.indexes {
        connection
            .batch_execute(&format!("DROP INDEX IF EXISTS {}", quote(&index.name)))
            .await
            .map_err(database_error)?;
    }
    Ok(())
}

async fn drop_dever_constraints(
    connection: &deadpool_postgres::Object,
    table: &str,
) -> Result<(), Error> {
    let constraints = connection
        .query(
            "SELECT con.conname FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class rel ON rel.oid = con.conrelid JOIN pg_catalog.pg_namespace nsp ON nsp.oid = rel.relnamespace WHERE nsp.nspname = current_schema() AND rel.relname = $1 AND left(con.conname, 7) = '_dever_'",
            &[&table],
        )
        .await
        .map_err(database_error)?;
    for row in constraints {
        alter_table(
            connection,
            table,
            &format!("DROP CONSTRAINT {}", quote(row.get::<_, &str>(0))),
        )
        .await?;
    }
    Ok(())
}

async fn alter_table(
    connection: &deadpool_postgres::Object,
    table: &str,
    operation: &str,
) -> Result<(), Error> {
    connection
        .batch_execute(&format!("ALTER TABLE {} {operation}", quote(table)))
        .await
        .map_err(database_error)
}

fn compatible_types(previous: &str, desired: &str) -> bool {
    previous == desired
        || (previous.starts_with("Text") && desired.starts_with("Text"))
        || (previous.starts_with("Bytes") && desired.starts_with("Bytes"))
        || (previous == "Int" && desired == "Float")
}

fn postgres_type(logical: &str) -> String {
    if logical == "Bool" {
        "BOOLEAN".into()
    } else if logical == "Float" {
        "DOUBLE PRECISION".into()
    } else if logical.starts_with("Bytes") {
        "BYTEA".into()
    } else if let Some(decimal) = logical
        .strip_prefix("Decimal(")
        .and_then(|value| value.strip_suffix(')'))
    {
        format!("NUMERIC({decimal})")
    } else if logical == "Uuid" {
        "UUID".into()
    } else if logical == "Int"
        || logical == "DateTime"
        || logical == "Date"
        || logical == "Time"
        || logical == "Duration"
        || logical.starts_with("id:")
    {
        "BIGINT".into()
    } else {
        "TEXT".into()
    }
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

async fn table_exists(connection: &deadpool_postgres::Object, table: &str) -> Result<bool, Error> {
    connection
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = current_schema() AND c.relname = $1 AND c.relkind = 'r')",
            &[&table],
        )
        .await
        .map(|row| row.get(0))
        .map_err(database_error)
}

async fn load_schema(
    connection: &deadpool_postgres::Object,
    model: &str,
) -> Result<Option<Schema>, Error> {
    let row = connection
        .query_opt(
            "SELECT definition FROM _dever_history WHERE model = $1 AND kind = 'schema' AND name = 'current'",
            &[&model],
        )
        .await
        .map_err(database_error)?;
    row.map(|row| {
        serde_json::from_str(row.get(0))
            .map_err(|error| Error::migration(format!("invalid Dever schema history: {error}")))
    })
    .transpose()
}

async fn validate_catalog(
    connection: &deadpool_postgres::Object,
    schema: &Schema,
) -> Result<(), Error> {
    let recorded = load_catalog(connection, &schema.model)
        .await?
        .ok_or_else(|| {
            Error::migration(format!(
                "Model '{}' has no recorded database catalog",
                schema.model
            ))
        })?;
    if catalog(connection, &schema.table).await? != recorded {
        return Err(Error::migration(format!(
            "table '{}' does not match its recorded Dever schema",
            schema.table
        )));
    }
    Ok(())
}

async fn load_catalog(
    connection: &deadpool_postgres::Object,
    model: &str,
) -> Result<Option<String>, Error> {
    connection
        .query_opt(
            "SELECT definition FROM _dever_history WHERE model = $1 AND kind = 'catalog' AND name = 'current'",
            &[&model],
        )
        .await
        .map(|row| row.map(|row| row.get(0)))
        .map_err(database_error)
}

async fn catalog(connection: &deadpool_postgres::Object, table: &str) -> Result<String, Error> {
    let columns = connection
        .query(
            "SELECT attribute.attname, pg_catalog.format_type(attribute.atttypid, attribute.atttypmod), attribute.attnotnull, attribute.attidentity::text, pg_catalog.pg_get_expr(default_value.adbin, default_value.adrelid) FROM pg_catalog.pg_attribute attribute JOIN pg_catalog.pg_class relation ON relation.oid = attribute.attrelid JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace LEFT JOIN pg_catalog.pg_attrdef default_value ON default_value.adrelid = relation.oid AND default_value.adnum = attribute.attnum WHERE namespace.nspname = current_schema() AND relation.relname = $1 AND attribute.attnum > 0 AND NOT attribute.attisdropped ORDER BY attribute.attnum",
            &[&table],
        )
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            (
                row.get::<_, String>(0),
                row.get::<_, String>(1),
                row.get::<_, bool>(2),
                row.get::<_, String>(3),
                row.get::<_, Option<String>>(4),
            )
        })
        .collect::<Vec<_>>();
    let constraints = connection
        .query(
            "SELECT constraint_.conname, constraint_.contype::text, pg_catalog.pg_get_constraintdef(constraint_.oid, false) FROM pg_catalog.pg_constraint constraint_ JOIN pg_catalog.pg_class relation ON relation.oid = constraint_.conrelid JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace WHERE namespace.nspname = current_schema() AND relation.relname = $1 ORDER BY constraint_.conname",
            &[&table],
        )
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            (
                row.get::<_, String>(0),
                row.get::<_, String>(1),
                row.get::<_, String>(2),
            )
        })
        .collect::<Vec<_>>();
    let indexes = connection
        .query(
            "SELECT index_relation.relname, pg_catalog.pg_get_indexdef(index_relation.oid) FROM pg_catalog.pg_index index_ JOIN pg_catalog.pg_class table_relation ON table_relation.oid = index_.indrelid JOIN pg_catalog.pg_class index_relation ON index_relation.oid = index_.indexrelid JOIN pg_catalog.pg_namespace namespace ON namespace.oid = table_relation.relnamespace WHERE namespace.nspname = current_schema() AND table_relation.relname = $1 AND left(index_relation.relname, 11) = '_dever_idx_' ORDER BY index_relation.relname",
            &[&table],
        )
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
        .collect::<Vec<_>>();
    serde_json::to_string(&(columns, constraints, indexes))
        .map_err(|error| Error::migration(format!("cannot encode PostgreSQL catalog: {error}")))
}

async fn validate_migrations(
    connection: &deadpool_postgres::Object,
    schema: &Schema,
) -> Result<(), Error> {
    let applied = load_migrations(connection, &schema.model).await?;
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

async fn load_migrations(
    connection: &deadpool_postgres::Object,
    model: &str,
) -> Result<BTreeMap<String, String>, Error> {
    connection
        .query(
            "SELECT name, revision FROM _dever_history WHERE model = $1 AND kind = 'migration'",
            &[&model],
        )
        .await
        .map_err(database_error)
        .map(|rows| {
            rows.into_iter()
                .map(|row| (row.get(0), row.get(1)))
                .collect()
        })
}

async fn store_history(
    connection: &deadpool_postgres::Object,
    schema: &Schema,
) -> Result<(), Error> {
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
    )
    .await?;
    for migration in &schema.migrations {
        store_history_row(
            connection,
            &schema.model,
            "migration",
            &migration.name,
            &migration.revision,
            "",
        )
        .await?;
    }
    store_history_row(
        connection,
        &schema.model,
        "seed",
        "current",
        &schema.seed_revision,
        "",
    )
    .await?;
    store_catalog_history(connection, &schema.model, &schema.table).await
}

async fn store_catalog_history(
    connection: &deadpool_postgres::Object,
    model: &str,
    table: &str,
) -> Result<(), Error> {
    store_history_row(
        connection,
        model,
        "catalog",
        "current",
        "actual",
        &catalog(connection, table).await?,
    )
    .await
}

async fn store_history_row(
    connection: &deadpool_postgres::Object,
    model: &str,
    kind: &str,
    name: &str,
    revision: &str,
    definition: &str,
) -> Result<(), Error> {
    let parameters: [&(dyn ToSql + Sync); 5] = [&model, &kind, &name, &revision, &definition];
    connection
        .execute(
            "INSERT INTO _dever_history (model, kind, name, revision, definition) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (model, kind, name) DO UPDATE SET revision = EXCLUDED.revision, definition = EXCLUDED.definition, applied_at = (extract(epoch from clock_timestamp()) * 1000)::bigint WHERE _dever_history.kind <> 'migration'",
            &parameters,
        )
        .await
        .map(|_| ())
        .map_err(database_error)
}
