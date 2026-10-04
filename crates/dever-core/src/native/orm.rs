use std::collections::BTreeSet;
use std::fmt::Write;

use crate::hir::{Expression, ModelCondition, ModelOperation, QueryPlan};
use crate::model::{
    ConnectionSelector, ModelFieldType, ModelRelationKind, ModelSchema, ModelScope,
    ModelSqlCardinality,
};
use crate::source::Span;
use crate::syntax::BinaryOperator;
use crate::types::{DefinitionKind, Shape, Type};

use super::{Emitter, rust_string, rust_type};

impl Emitter<'_> {
    pub(super) fn initialize_models(&self, output: &mut String) {
        let mut model_migrations = Vec::new();
        let mut foreign_key_migrations = Vec::new();
        for (index, model) in self.program.models.iter().enumerate() {
            let span = self.program.types[model.record].span;
            let scope = match model.scope {
                ModelScope::Global => "Global",
                ModelScope::Tenant => "Tenant",
            };
            let (explicit, root) = model_storage_selector(model);
            writeln!(
                output,
                "MODEL_STORAGE_{}.set(dever_runtime::database::StorageBinding::new({explicit}, {}, dever_runtime::database::ModelScope::{scope})).map_err(|_| AppError::fault({}, \"Model storage was initialized more than once\"))?;",
                model.record,
                rust_string(root),
                self.location(span),
            )
            .expect("string formatting");
            let database =
                self.database_failure(&format!("model_database_{}().await", model.record), span);
            writeln!(
                output,
                "let schema_database_{index} = if dever_runtime::config::settings().tenant().is_some() && matches!(dever_runtime::database::ModelScope::{scope}, dever_runtime::database::ModelScope::Tenant) {{ None }} else {{ Some({database}) }};"
            )
            .expect("string formatting");
            let fields = model
                .fields
                .iter()
                .map(|field| {
                    format!(
                        "dever_runtime::database::Field {{ name: String::from({}), ty: String::from({}), nullable: {}, generated: {}, default: {}, rename_from: {} }}",
                        rust_string(&field.name),
                        rust_string(&crate::model::field_type(&field.ty)),
                        field.nullable,
                        field.generated,
                        field.default.as_ref().map(|value| format!("Some(String::from({}))", rust_string(&crate::model::value(value)))).unwrap_or_else(|| "None".into()),
                        field.rename_from.as_ref().map(|name| format!("Some(String::from({}))", rust_string(name))).unwrap_or_else(|| "None".into()),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let indexes = model
                .indexes
                .iter()
                .filter(|candidate| candidate.fields.as_slice() != ["id"])
                .map(|candidate| {
                    let name = index_name(model, candidate);
                    let fields = candidate
                        .fields
                        .iter()
                        .map(|field| format!("String::from({})", rust_string(field)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "dever_runtime::database::Index {{ name: String::from({}), fields: vec![{fields}], unique: {} }}",
                        rust_string(&name),
                        candidate.unique,
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let migrations = model
                .migrations
                .iter()
                .map(|migration| {
                    let drops = migration
                        .drops
                        .iter()
                        .map(|field| format!("String::from({})", rust_string(field)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let revision = migration.revision();
                    format!(
                        "dever_runtime::database::Migration {{ name: String::from({}), revision: String::from({}), drops: vec![{drops}] }}",
                        rust_string(&migration.name),
                        rust_string(&revision),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let data_migrations = model.migrations.iter().flat_map(|migration| {
                migration.statements.iter().map(move |statement| {
                    let phase = match statement.phase {
                        crate::syntax::MigrationPhase::Before => "Before",
                        crate::syntax::MigrationPhase::After => "After",
                    };
                    let parameters = statement.parameters.iter()
                        .map(|(ty, value)| seed_value(value, ty))
                        .collect::<Vec<_>>().join(", ");
                    format!(
                        "dever_runtime::database::DataMigration {{ name: String::from({}), phase: dever_runtime::database::MigrationPhase::{phase}, sql: dever_runtime::database::Sql {{ sqlite: {}, postgres: {} }}, parameters: vec![{parameters}] }}",
                        rust_string(&migration.name), rust_string(&statement.sqlite), rust_string(&statement.postgres),
                    )
                })
            }).collect::<Vec<_>>().join(", ");
            let seeds = model
                .seed
                .iter()
                .map(|seed| {
                    let generated = model
                        .fields
                        .iter()
                        .filter(|field| field.generated && matches!(field.ty, ModelFieldType::Uuid))
                        .collect::<Vec<_>>();
                    let columns = seed
                        .fields
                        .iter()
                        .map(|(field, _)| quoted(field))
                        .chain(generated.iter().map(|field| quoted(&field.name)))
                        .collect::<Vec<_>>();
                    let parameter_types = seed
                        .fields
                        .iter()
                        .map(|(name, _)| {
                            &model
                                .fields
                                .iter()
                                .find(|field| field.name == *name)
                                .expect("validated Seed field")
                                .ty
                        })
                        .chain(generated.iter().map(|field| &field.ty))
                        .collect::<Vec<_>>();
                    let placeholders = parameter_types
                        .iter()
                        .enumerate()
                        .map(|(index, ty)| parameter_sql(index + 1, ty))
                        .collect::<Vec<_>>();
                    let sql = format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING",
                        quoted(&model.table),
                        columns.join(", "),
                        placeholders.join(", "),
                        seed.identity
                            .iter()
                            .map(|field| quoted(field))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    let mut values = seed
                        .fields
                        .iter()
                        .map(|(name, value)| {
                            let field = model
                                .fields
                                .iter()
                                .find(|field| field.name == *name)
                                .expect("validated Seed field");
                            seed_value(value, &field.ty)
                        })
                        .collect::<Vec<_>>();
                    for _ in generated {
                        let uuid =
                            self.database_failure("dever_runtime::orm::Uuid::new_v7()", span);
                        values.push(format!("dever_runtime::orm::Value::Uuid({uuid})"));
                    }
                    format!(
                        "dever_runtime::database::Seed {{ sql: {}, parameters: vec![{values}] }}",
                        database_sql(&sql),
                        values = values.join(", "),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let temporary = format!("_dever_{}_{}", model.table, &model.revision[..8]);
            let table = sqlite_table(model, &self.program.models, &model.table);
            let postgres_constraints = postgres_constraints(model, &self.program.models);
            let postgres_table = postgres_table(model, &postgres_constraints, &model.table);
            let postgres_columns = model
                .fields
                .iter()
                .map(|field| {
                    format!(
                        "dever_runtime::database::PostgresColumn {{ name: String::from({}), definition: String::from({}), sql_type: String::from({}), default: {} }}",
                        rust_string(&field.name),
                        rust_string(&postgres_column(field)),
                        rust_string(&postgres_type(&field.ty)),
                        postgres_field_default(field)
                            .map(|value| format!("Some(String::from({}))", rust_string(&value)))
                            .unwrap_or_else(|| "None".into()),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let postgres_constraints_code = postgres_constraints
                .iter()
                .map(|(name, definition, foreign_key)| {
                    format!(
                        "dever_runtime::database::PostgresConstraint {{ name: String::from({}), definition: String::from({}), foreign_key: {foreign_key} }}",
                        rust_string(name),
                        rust_string(definition),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let foreign_keys = postgres_constraints
                .iter()
                .filter(|(_, _, foreign_key)| *foreign_key)
                .map(|(name, definition, foreign_key)| {
                    format!(
                        "dever_runtime::database::PostgresConstraint {{ name: String::from({}), definition: String::from({}), foreign_key: {foreign_key} }}",
                        rust_string(name),
                        rust_string(definition),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            if !foreign_keys.is_empty() {
                foreign_key_migrations.push((
                    index,
                    span,
                    model.package.clone(),
                    model.table.clone(),
                    foreign_keys.clone(),
                ));
            }
            let temporary_table = sqlite_table(model, &self.program.models, &temporary);
            let create_indexes = sqlite_indexes(model)
                .iter()
                .map(|sql| {
                    format!(
                        "dever_runtime::database::SchemaSql {{ sqlite: String::from({}), postgres: String::from({}) }}",
                        rust_string(sql),
                        rust_string(sql),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let request = format!(
                "dever_runtime::database::Model {{ schema: dever_runtime::database::Schema {{ model: String::from({}), table: String::from({}), revision: String::from({}), seed_revision: String::from({}), fields: vec![{fields}], indexes: vec![{indexes}], migrations: vec![{migrations}] }}, create_table: dever_runtime::database::SchemaSql {{ sqlite: String::from({}), postgres: String::from({}) }}, create_temporary_table: String::from({}), create_indexes: vec![{create_indexes}], postgres_columns: vec![{postgres_columns}], postgres_constraints: vec![{postgres_constraints_code}], seeds: vec![{seeds}], data_migrations: vec![{data_migrations}] }}",
                rust_string(&model.package),
                rust_string(&model.table),
                rust_string(&model.revision),
                rust_string(&model.seed_revision),
                rust_string(&table),
                rust_string(&postgres_table),
                rust_string(&temporary_table),
            );
            writeln!(
                output,
                "fn model_definition_{index}() -> Result<dever_runtime::database::Model, AppError> {{ Ok({request}) }}"
            )
            .expect("string formatting");
            writeln!(
                output,
                "fn model_foreign_keys_{index}() -> Vec<dever_runtime::database::PostgresConstraint> {{ vec![{foreign_keys}] }}"
            )
            .expect("string formatting");
            let migration = self.database_failure(
                &format!("schema_database_{index}.as_ref().expect(\"selected Model database\").migrate(model_definition_{index}()?).await"),
                span,
            );
            model_migrations.push((index, migration));
        }
        self.emit_tenant_migrate(output);
        if model_migrations.is_empty() {
            return;
        }
        output.push_str("let mut schema_migration_order = Vec::new();\n");
        for (index, _) in &model_migrations {
            writeln!(output, "if let Some(database) = schema_database_{index}.as_ref() {{ schema_migration_order.push((database.name(), {index}usize)); }}").expect("string formatting");
        }
        output.push_str("schema_migration_order.sort_unstable();\nfor (_, schema_model) in schema_migration_order.iter().copied() {\nmatch schema_model {\n");
        for (index, migration) in &model_migrations {
            writeln!(output, "{index} => {{ {migration}; }},").expect("string formatting");
        }
        output.push_str(
            "_ => unreachable!(\"schema migration references an unknown Model\"),\n}\n}\n",
        );

        if foreign_key_migrations.is_empty() {
            return;
        }
        output.push_str("for (_, schema_model) in schema_migration_order.iter().copied() {\nmatch schema_model {\n");
        for (index, span, model, table, constraints) in foreign_key_migrations {
            let migration = self.database_failure(
                &format!(
                    "schema_database_{index}.as_ref().expect(\"selected Model database\").migrate_foreign_keys(String::from({}), String::from({}), vec![{constraints}]).await",
                    rust_string(&model),
                    rust_string(&table),
                ),
                span,
            );
            writeln!(output, "{index} => {{ {migration}; }},").expect("string formatting");
        }
        output.push_str("_ => {},\n}\n}\n");
    }

    fn emit_tenant_migrate(&self, output: &mut String) {
        let tenant_models = self
            .program
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| model.scope == ModelScope::Tenant)
            .collect::<Vec<_>>();
        let fingerprint = tenant_fingerprint(&tenant_models, self.program);
        output.push_str("async fn tenant_migrate(tenant_id: i64) -> Result<(), AppError> {\n");
        if tenant_models.is_empty() {
            output.push_str("let _ = tenant_id;\nreturn Err(AppError::fault(\"tenant migration\", \"application has no tenant Model\"));\n}\n");
            return;
        }
        output.push_str("if tenant_id <= 0 { return Err(AppError::fault(\"tenant migration\", \"tenant id must be positive\")); }\n");
        output.push_str("let settings = dever_runtime::config::settings();\nif settings.tenant().is_none() { return Err(AppError::fault(\"tenant migration\", \"tenant storage is not configured\")); }\nlet mut tenant_databases = std::collections::BTreeMap::<String, dever_runtime::database::Database>::new();\n");
        if !self.program.permissions.is_empty() {
            writeln!(output,
                "let authorization_connection = settings.tenant().expect(\"checked tenant settings\").database().to_owned();\nif !tenant_databases.contains_key(&authorization_connection) {{ let database = dever_runtime::tenant::begin_migration(&authorization_connection, tenant_id, {}).await.map_err(|error| AppError::database(\"tenant authorization migration\", error))?; tenant_databases.insert(authorization_connection.clone(), database); }}",
                rust_string(&fingerprint),
            ).expect("string formatting");
        }
        for (index, model) in &tenant_models {
            let (explicit, root) = model_storage_selector(model);
            writeln!(
                output,
                "let tenant_connection_{index} = settings.resolve_database({explicit}, {}).map_err(|error| AppError::fault(\"tenant migration\", error))?.0.to_owned();\nif !tenant_databases.contains_key(&tenant_connection_{index}) {{ let database = dever_runtime::tenant::begin_migration(&tenant_connection_{index}, tenant_id, {}).await.map_err(|error| AppError::database(\"tenant migration\", error))?; tenant_databases.insert(tenant_connection_{index}.clone(), database); }}",
                rust_string(root),
                rust_string(&fingerprint),
            )
            .expect("string formatting");
        }
        writeln!(
            output,
            "let mut tenant_migration_order = [{}];",
            tenant_models
                .iter()
                .map(|(index, _)| format!("(tenant_connection_{index}.as_str(), {index}usize)"))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("string formatting");
        output.push_str("tenant_migration_order.sort_unstable();\nfor (connection, schema_model) in tenant_migration_order.iter().copied() { let database = tenant_databases.get(connection).expect(\"tenant migration database initialized\"); match schema_model {\n");
        for (index, model) in &tenant_models {
            let span = self.program.types[model.record].span;
            let migration = self.database_failure(
                &format!("database.migrate(model_definition_{index}()?).await"),
                span,
            );
            writeln!(output, "{index} => {{ {migration}; }},").expect("string formatting");
        }
        output.push_str(
            "_ => unreachable!(\"tenant migration references an unknown Model\"),\n} }\n",
        );
        output.push_str("for (connection, schema_model) in tenant_migration_order.iter().copied() { let database = tenant_databases.get(connection).expect(\"tenant migration database initialized\"); match schema_model {\n");
        for (index, model) in &tenant_models {
            let constraints = postgres_constraints(model, &self.program.models);
            if constraints.iter().any(|(_, _, foreign_key)| *foreign_key) {
                let span = self.program.types[model.record].span;
                let migration = self.database_failure(
                    &format!(
                        "database.migrate_foreign_keys(String::from({}), String::from({}), model_foreign_keys_{index}()).await",
                        rust_string(&model.package),
                        rust_string(&model.table),
                    ),
                    span,
                );
                writeln!(output, "{index} => {{ {migration}; }},").expect("string formatting");
            }
        }
        output.push_str("_ => {},\n} }\n");
        writeln!(
            output,
            "job_migrate_tenant(tenant_id, {}, &mut tenant_databases).await?;",
            rust_string(&fingerprint)
        )
        .expect("string formatting");
        if !self.program.permissions.is_empty() {
            output.push_str("dever_runtime::auth::store::initialize_roles(tenant_databases.get(&authorization_connection).expect(\"tenant authorization database initialized\").clone()).await.map_err(|error| AppError::database(\"tenant authorization migration\", error))?;\n");
        }
        writeln!(output, "for database in tenant_databases.values() {{ dever_runtime::tenant::mark_database_ready(database, tenant_id, {}).await.map_err(|error| AppError::database(\"tenant migration\", error))?; }}\ndever_runtime::tenant::finish_migration(tenant_id, {}).await.map_err(|error| AppError::database(\"tenant migration\", error))?;\nOk(())\n}}", rust_string(&fingerprint), rust_string(&fingerprint)).expect("string formatting");
        writeln!(output, "if dever_runtime::config::settings().tenant().is_some() {{ dever_runtime::tenant::initialize({}).await.map_err(|error| AppError::database(\"tenant initialization\", error))?; }}", rust_string(&fingerprint)).expect("string formatting");
    }

    pub(super) fn model_decoders(&self, output: &mut String) {
        for model in &self.program.models {
            writeln!(
                output,
                "static MODEL_STORAGE_{}: std::sync::OnceLock<dever_runtime::database::StorageBinding> = std::sync::OnceLock::new();\nasync fn model_database_{}() -> Result<dever_runtime::database::Database, dever_runtime::orm::Error> {{ MODEL_STORAGE_{}.get().expect(\"Model storage is initialized before entry\").database().await }}",
                model.record,
                model.record,
                model.record
            )
            .expect("string formatting");
            writeln!(
                output,
                "fn decode_model_fields_{}(values: &mut std::vec::IntoIter<dever_runtime::orm::Value>) -> Result<T{}, dever_runtime::orm::Error> {{",
                model.record, model.record
            )
            .expect("string formatting");
            let Shape::Record(fields) = &self.program.types[model.record].shape else {
                unreachable!("Model is a record")
            };
            for (index, field) in fields.iter().take(model.fields.len()).enumerate() {
                let column = rust_string(&format!("{}.{}", model.package, field.name));
                let value = format!("dever_runtime::orm::column(values, {column})?");
                writeln!(
                    output,
                    "let f{index} = {};",
                    self.decode_value(&field.ty, &value)
                )
                .expect("string formatting");
            }
            for index in model.fields.len()..fields.len() {
                writeln!(
                    output,
                    "let f{index} = dever_runtime::orm::Related::Unloaded;"
                )
                .expect("string formatting");
            }
            writeln!(
                output,
                "Ok(T{} {{ {} }})\n}}\nfn decode_model_{}(row: dever_runtime::orm::Row) -> Result<T{}, dever_runtime::orm::Error> {{\nlet mut values = row.into_values();\nlet model = decode_model_fields_{}(&mut values)?;\nif !values.as_slice().is_empty() {{ return Err(dever_runtime::orm::Error::invalid_data({})); }}\nOk(model)\n}}",
                model.record,
                (0..fields.len())
                    .map(|index| format!("f{index}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                model.record,
                model.record,
                model.record,
                rust_string(&format!("database row for {} has extra columns", model.package)),
            )
            .expect("string formatting");
        }
        let records = self
            .program
            .models
            .iter()
            .flat_map(|model| model.sql.iter().map(|sql| sql.result.record))
            .filter(|record| self.program.types[*record].kind == DefinitionKind::Regular)
            .collect::<BTreeSet<_>>();
        for record in records {
            let Shape::Record(fields) = &self.program.types[record].shape else {
                unreachable!("checked SQL result is a record")
            };
            writeln!(
                output,
                "fn decode_sql_record_{record}(row: dever_runtime::orm::Row) -> Result<T{record}, dever_runtime::orm::Error> {{\nlet mut values = row.into_values();"
            )
            .expect("string formatting");
            for (index, field) in fields.iter().enumerate() {
                let column = rust_string(&format!(
                    "{}.{}",
                    self.program.types[record].name, field.name
                ));
                let value = format!("dever_runtime::orm::column(&mut values, {column})?");
                writeln!(
                    output,
                    "let f{index} = {};",
                    self.decode_value(&field.ty, &value)
                )
                .expect("string formatting");
            }
            writeln!(
                output,
                "if !values.as_slice().is_empty() {{ return Err(dever_runtime::orm::Error::invalid_data({})); }}\nOk(T{record} {{ {} }})\n}}",
                rust_string(&format!(
                    "database row for {} has extra columns",
                    self.program.types[record].name
                )),
                (0..fields.len())
                    .map(|index| format!("f{index}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            )
            .expect("string formatting");
        }
    }

    pub(super) fn model_operation(
        &self,
        model_index: usize,
        operation: &ModelOperation,
        result: &Expression,
    ) -> String {
        let model = &self.program.models[model_index];
        match operation {
            ModelOperation::Create { values } => self.create(model, values, result),
            ModelOperation::CreateMany { rows } => self.create_many(model, rows, result),
            ModelOperation::Get { id } => self.get(model, id, result),
            ModelOperation::First { query } => self.first(model, query, result),
            ModelOperation::List { query } => self.list(model, query, result),
            ModelOperation::Cursor { query } => self.cursor(model, query, result),
            ModelOperation::Count { query } => self.count(model, query, result),
            ModelOperation::Exists { query } => self.exists(model, query, result),
            ModelOperation::Stream { query } => self.stream(model, query, result),
            ModelOperation::Update { query, values } => self.update(model, query, values, result),
            ModelOperation::Delete { query } => self.delete(model, query, result),
            ModelOperation::Upsert {
                key,
                create,
                update,
            } => self.upsert(model, key, create, update, result),
            ModelOperation::Sql {
                operation,
                arguments,
                ..
            } => self.sql(model, *operation, arguments, result),
        }
    }

    fn sql(
        &self,
        model: &ModelSchema,
        operation: usize,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        let operation = &model.sql[operation];
        let expressions = arguments
            .iter()
            .zip(&operation.parameters)
            .map(|(argument, parameter)| (argument, &parameter.ty))
            .collect::<Vec<_>>();
        let (bindings, parameters) = self.bind_values(&expressions);
        let database = self.database(model, result);
        let maximum = match operation.result.cardinality {
            ModelSqlCardinality::Many => "database.max_page_size()",
            ModelSqlCardinality::One | ModelSqlCardinality::Optional => "1usize",
        };
        let rows = self.database_failure(
            &format!(
                "database.query_bounded(dever_runtime::database::Sql {{ sqlite: {}, postgres: {} }}, vec![{}], {maximum}).await",
                rust_string(&operation.sqlite),
                rust_string(&operation.postgres),
                parameters.join(", ")
            ),
            result.span,
        );
        let decoder = if self.program.types[operation.result.record].kind == DefinitionKind::Model {
            format!("decode_model_{}", operation.result.record)
        } else {
            format!("decode_sql_record_{}", operation.result.record)
        };
        let name = rust_string(&format!("{}.{}", model.package, operation.name));
        let decoded = match operation.result.cardinality {
            ModelSqlCardinality::Many => {
                let rows = self.database_failure(
                    &format!(
                        "rows.into_iter().map({decoder}).collect::<Result<Vec<_>, dever_runtime::orm::Error>>()"
                    ),
                    result.span,
                );
                format!("List::new({rows})")
            }
            ModelSqlCardinality::One => {
                let row = self.database_failure(
                    &format!("dever_runtime::orm::required_row(rows, {name})"),
                    result.span,
                );
                self.database_failure(&format!("{decoder}({row})"), result.span)
            }
            ModelSqlCardinality::Optional => {
                let row = self.database_failure(
                    &format!("dever_runtime::orm::optional_row(rows, {name})"),
                    result.span,
                );
                let decoded = self.database_failure(&format!("{decoder}(row)"), result.span);
                format!("match {row} {{ Some(row) => Some({decoded}), None => None }}")
            }
        };
        format!("{{ {bindings} let database = {database}; let rows = {rows}; {decoded} }}")
    }

    fn create(
        &self,
        model: &ModelSchema,
        values: &[(usize, Expression)],
        result: &Expression,
    ) -> String {
        let generated = model
            .fields
            .iter()
            .filter(|field| field.generated && matches!(field.ty, ModelFieldType::Uuid))
            .collect::<Vec<_>>();
        let columns = values
            .iter()
            .map(|(field, _)| quoted(&model.fields[*field].name))
            .chain(generated.iter().map(|field| quoted(&field.name)))
            .collect::<Vec<_>>();
        let parameter_types = values
            .iter()
            .map(|(field, _)| &model.fields[*field].ty)
            .chain(generated.iter().map(|field| &field.ty))
            .collect::<Vec<_>>();
        let placeholders = parameter_types
            .iter()
            .enumerate()
            .map(|(index, ty)| parameter_sql(index + 1, ty))
            .collect::<Vec<_>>();
        let insertion = if columns.is_empty() {
            "DEFAULT VALUES".into()
        } else {
            format!(
                "({}) VALUES ({})",
                columns.join(", "),
                placeholders.join(", ")
            )
        };
        let sql = format!(
            "INSERT INTO {} {insertion} RETURNING {}",
            quoted(&model.table),
            select_columns(model)
        );
        let expressions = values
            .iter()
            .map(|(field, value)| (value, &model.fields[*field].ty))
            .collect::<Vec<_>>();
        let (mut bindings, mut parameters) = self.bind_values(&expressions);
        for index in 0..generated.len() {
            let uuid = self.database_failure("dever_runtime::orm::Uuid::new_v7()", result.span);
            writeln!(
                bindings,
                "let generated_uuid_{index} = dever_runtime::orm::Value::Uuid({uuid});"
            )
            .expect("string formatting");
            parameters.push(format!("generated_uuid_{index}"));
        }
        let database = self.database(model, result);
        let rows = self.database_failure(
            &format!(
                "database.query({}, vec![{}]).await",
                database_sql(&sql),
                parameters.join(", ")
            ),
            result.span,
        );
        let row = self.database_failure(
            &format!(
                "rows.into_iter().next().ok_or_else(|| dever_runtime::orm::Error::invalid_data({}))",
                rust_string("database INSERT RETURNING produced no row")
            ),
            result.span,
        );
        let decoded = self.database_failure(
            &format!("decode_model_{}({row})", model.record),
            result.span,
        );
        format!("{{ {bindings} let database = {database}; let rows = {rows}; {decoded} }}")
    }

    fn create_many(
        &self,
        model: &ModelSchema,
        rows: &[Vec<(usize, Expression)>],
        result: &Expression,
    ) -> String {
        let generated = model
            .fields
            .iter()
            .filter(|field| field.generated && matches!(field.ty, ModelFieldType::Uuid))
            .collect::<Vec<_>>();
        let columns = rows[0]
            .iter()
            .map(|(field, _)| quoted(&model.fields[*field].name))
            .chain(generated.iter().map(|field| quoted(&field.name)))
            .collect::<Vec<_>>();
        let explicit_per_row = rows[0].len();
        let explicit_total = explicit_per_row * rows.len();
        let values = if columns.is_empty() {
            (0..rows.len())
                .map(|_| "(NULL)".to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            rows.iter()
                .enumerate()
                .map(|(row, values)| {
                    let placeholders = values
                        .iter()
                        .enumerate()
                        .map(|(column, (field, _))| {
                            parameter_sql(
                                row * explicit_per_row + column + 1,
                                &model.fields[*field].ty,
                            )
                        })
                        .chain(generated.iter().enumerate().map(|(column, field)| {
                            parameter_sql(
                                explicit_total + row * generated.len() + column + 1,
                                &field.ty,
                            )
                        }))
                        .collect::<Vec<_>>();
                    format!("({})", placeholders.join(", "))
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (sqlite_sql, postgres_sql) = if columns.is_empty() {
            let sqlite = format!(
                "INSERT INTO {} ({}) VALUES {values}",
                quoted(&model.table),
                quoted("id")
            );
            let defaults = (0..rows.len())
                .map(|_| "(DEFAULT)")
                .collect::<Vec<_>>()
                .join(", ");
            let postgres = format!(
                "INSERT INTO {} ({}) VALUES {defaults}",
                quoted(&model.table),
                quoted("created_at")
            );
            (sqlite, postgres)
        } else {
            let sql = format!(
                "INSERT INTO {} ({}) VALUES {values}",
                quoted(&model.table),
                columns.join(", ")
            );
            (sql.clone(), sql)
        };
        let expressions = rows
            .iter()
            .flat_map(|row| row.iter())
            .map(|(field, value)| (value, &model.fields[*field].ty))
            .collect::<Vec<_>>();
        let (mut bindings, mut parameters) = self.bind_values(&expressions);
        for index in 0..rows.len() * generated.len() {
            let uuid = self.database_failure("dever_runtime::orm::Uuid::new_v7()", result.span);
            writeln!(
                bindings,
                "let generated_uuid_{index} = dever_runtime::orm::Value::Uuid({uuid});"
            )
            .expect("string formatting");
            parameters.push(format!("generated_uuid_{index}"));
        }
        let database = self.database(model, result);
        let affected = self.database_failure(
            &format!(
                "database.execute({}, vec![{}]).await",
                database_sql_pair(&sqlite_sql, &postgres_sql),
                parameters.join(", ")
            ),
            result.span,
        );
        let affected = self.database_failure(
            &format!("dever_runtime::orm::affected({affected})"),
            result.span,
        );
        format!("{{ {bindings} let database = {database}; {affected} }}")
    }

    fn get(&self, model: &ModelSchema, id: &Expression, result: &Expression) -> String {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = ?1 LIMIT 1",
            select_columns(model),
            quoted(&model.table),
            quoted("id")
        );
        let id_type = &model
            .fields
            .iter()
            .find(|field| field.name == "id")
            .expect("synthetic Model id")
            .ty;
        let (bindings, parameters) = self.bind_values(&[(id, id_type)]);
        let database = self.database(model, result);
        let rows = self.database_failure(
            &format!(
                "database.query({}, vec![{}]).await",
                database_sql(&sql),
                parameters.join(", ")
            ),
            result.span,
        );
        let row = self.database_failure(
            &format!(
                "rows.into_iter().next().ok_or_else(|| dever_runtime::orm::Error::not_found({}))",
                rust_string(&model.package)
            ),
            result.span,
        );
        let decoded = self.database_failure(
            &format!("decode_model_{}({row})", model.record),
            result.span,
        );
        format!("{{ {bindings} let database = {database}; let rows = {rows}; {decoded} }}")
    }

    fn first(&self, model: &ModelSchema, query: &QueryPlan, result: &Expression) -> String {
        let mut expressions = Vec::new();
        let joined = has_to_one(model, query);
        let alias = joined.then_some("_dever_parent");
        let where_clause =
            condition_sql_qualified(model, query.condition.as_ref(), &mut expressions, alias);
        let (projection, source) = select_source(model, query, &self.program.models);
        let sql = format!(
            "SELECT {projection} FROM {source}{} ORDER BY {} LIMIT 1",
            where_clause,
            order_sql_qualified(model, &query.order, alias)
        );
        let (bindings, parameters) = self.bind_values(&expressions);
        let database = self.database(model, result);
        let rows = self.database_failure(
            &format!(
                "database.query({}, vec![{}]).await",
                database_sql(&sql),
                parameters.join(", ")
            ),
            result.span,
        );
        let decoded =
            self.database_failure(&self.decode_included_row(model, query, "row"), result.span);
        let load_relations = self.load_to_many(model, query, "items", result);
        format!(
            "{{ {bindings} let database = {database}; let rows = {rows}; let mut items = match rows.into_iter().next() {{ Some(row) => vec![{decoded}], None => Vec::new() }}; {load_relations} items.into_iter().next() }}"
        )
    }

    fn list(&self, model: &ModelSchema, query: &QueryPlan, result: &Expression) -> String {
        let mut expressions = Vec::new();
        let where_clause = condition_sql(model, query.condition.as_ref(), &mut expressions);
        let count_sql = format!(
            "SELECT COUNT(*) FROM {}{}",
            quoted(&model.table),
            where_clause
        );
        let joined = has_to_one(model, query);
        let alias = joined.then_some("_dever_parent");
        let (projection, source) = select_source(model, query, &self.program.models);
        let mut data_expressions = Vec::new();
        let data_where = condition_sql_qualified(
            model,
            query.condition.as_ref(),
            &mut data_expressions,
            alias,
        );
        debug_assert_eq!(data_expressions.len(), expressions.len());
        let data_sql = format!(
            "SELECT {projection} FROM {source}{data_where} ORDER BY {} LIMIT ?{} OFFSET ?{}",
            order_sql_qualified(model, &query.order, alias),
            expressions.len() + 1,
            expressions.len() + 2
        );
        let (bindings, parameters) = self.bind_values(&expressions);
        let page = self.expression(&query.page);
        let size = self.expression(&query.size);
        let database = self.database(model, result);
        let page_info = self.database_failure(
            &format!("dever_runtime::orm::pagination({page}, {size}, database.max_page_size())"),
            result.span,
        );
        let count_rows = self.database_failure(
            &format!(
                "database.query({}, parameters.clone()).await",
                database_sql(&count_sql)
            ),
            result.span,
        );
        let count_row = self.database_failure(
            &format!(
                "count_rows.into_iter().next().ok_or_else(|| dever_runtime::orm::Error::invalid_data({}))",
                rust_string("database COUNT returned no row")
            ),
            result.span,
        );
        let count_value = self.database_failure(
            &format!("dever_runtime::orm::scalar({count_row})"),
            result.span,
        );
        let total = self.database_failure(
            &format!("dever_runtime::orm::int({count_value})"),
            result.span,
        );
        let rows = self.database_failure(
            &format!(
                "database.query({}, {{ parameters.push(dever_runtime::orm::Value::Int(size)); parameters.push(dever_runtime::orm::Value::Int(offset)); parameters }}).await",
                database_sql(&data_sql)
            ),
            result.span,
        );
        let decoded = self.database_failure(
            &self.decode_included_rows(model, query, "rows"),
            result.span,
        );
        let load_relations = self.load_to_many(model, query, "items", result);
        format!(
            "{{ {bindings} let database = {database}; let (page, size, offset) = {page_info}; let mut parameters = vec![{}]; let count_rows = {count_rows}; let total = {total}; let rows = {rows}; let mut items = {decoded}; {load_relations} let items = List::new(items); let pages = total / size + i64::from(total % size != 0); T{} {{ f0: items, f1: page, f2: size, f3: total, f4: pages }} }}",
            parameters.join(", "),
            model.page
        )
    }

    fn cursor(&self, model: &ModelSchema, query: &QueryPlan, result: &Expression) -> String {
        let mut expressions = Vec::new();
        let joined = has_to_one(model, query);
        let alias = joined.then_some("_dever_parent");
        let mut where_clause =
            condition_sql_qualified(model, query.condition.as_ref(), &mut expressions, alias);
        if query.after.is_some() {
            let condition = cursor_condition(model, &query.order, expressions.len() + 1, alias);
            if where_clause.is_empty() {
                where_clause = format!(" WHERE {condition}");
            } else {
                where_clause.push_str(" AND ");
                where_clause.push_str(&condition);
            }
        }
        let limit = expressions.len() + query.after.as_ref().map_or(0, |_| query.order.len()) + 1;
        let (projection, source) = select_source(model, query, &self.program.models);
        let sql = format!(
            "SELECT {projection} FROM {source}{} ORDER BY {} LIMIT ?{limit}",
            where_clause,
            order_sql_qualified(model, &query.order, alias),
        );
        let (mut bindings, mut parameters) = self.bind_values(&expressions);
        if let Some(after) = &query.after {
            writeln!(
                bindings,
                "let cursor_after: Option<T{}> = {};",
                model.record,
                self.expression(after)
            )
            .expect("string formatting");
            let Shape::Record(fields) = &self.program.types[model.record].shape else {
                unreachable!("Model is a record")
            };
            for (index, order) in query.order.iter().enumerate() {
                let field = &fields[order.field];
                let encoded = self.encode_value(
                    &field.ty,
                    &model.fields[order.field].ty,
                    &format!("item.f{}.clone()", order.field),
                    after.span,
                );
                writeln!(
                    bindings,
                    "let cursor_p{index} = match cursor_after.as_ref() {{ Some(item) => {encoded}, None => dever_runtime::orm::Value::Null }};"
                )
                .expect("string formatting");
                parameters.push(format!("cursor_p{index}"));
            }
        }
        let size = self.expression(&query.size);
        let database = self.database(model, result);
        let size = self.database_failure(
            &format!("dever_runtime::orm::cursor_size({size}, database.max_page_size())"),
            result.span,
        );
        let rows = self.database_failure(
            &format!(
                "database.query({}, {{ parameters.push(dever_runtime::orm::Value::Int(size + 1)); parameters }}).await",
                database_sql(&sql)
            ),
            result.span,
        );
        let decoded = self.database_failure(
            &self.decode_included_rows(model, query, "rows"),
            result.span,
        );
        let load_relations = self.load_to_many(model, query, "items", result);
        format!(
            "{{ {bindings} let database = {database}; let size = {size}; let mut parameters = vec![{}]; let rows = {rows}; let mut items = {decoded}; let has_more = items.len() > size as usize; if has_more {{ items.truncate(size as usize); }} {load_relations} let next = if has_more {{ items.last().cloned() }} else {{ None }}; T{} {{ f0: List::new(items), f1: next, f2: has_more }} }}",
            parameters.join(", "),
            model.cursor,
        )
    }

    fn stream(&self, model: &ModelSchema, query: &QueryPlan, result: &Expression) -> String {
        let mut expressions = Vec::new();
        let where_clause = condition_sql(model, query.condition.as_ref(), &mut expressions);
        let sql = format!(
            "SELECT {} FROM {}{} ORDER BY {}",
            select_columns(model),
            quoted(&model.table),
            where_clause,
            order_sql(model, &query.order),
        );
        let (bindings, parameters) = self.bind_values(&expressions);
        let capacity = self.expression(&query.size);
        let database = self.database(model, result);
        let capacity = self.database_failure(
            &format!("dever_runtime::orm::stream_capacity({capacity}, database.max_page_size())"),
            result.span,
        );
        let stream = self.database_failure(
            &format!(
                "database.stream({}, vec![{}], capacity).await",
                database_sql(&sql),
                parameters.join(", ")
            ),
            result.span,
        );
        format!(
            "{{ {bindings} let database = {database}; let capacity = {capacity}; let stream = {stream}; stream.map(decode_model_{}) }}",
            model.record
        )
    }

    fn decode_included_rows(&self, model: &ModelSchema, query: &QueryPlan, rows: &str) -> String {
        format!(
            "{rows}.into_iter().map(|row| {}).collect::<Result<Vec<_>, dever_runtime::orm::Error>>()",
            self.decode_included_row(model, query, "row")
        )
    }

    fn decode_included_row(&self, model: &ModelSchema, query: &QueryPlan, row: &str) -> String {
        let mut code = format!(
            "{{ let mut values = {row}.into_values(); let mut item = decode_model_fields_{}(&mut values)?;",
            model.record
        );
        for relation_index in &query.relations {
            let relation = &model.relations[*relation_index];
            if relation.kind != ModelRelationKind::ToOne {
                continue;
            }
            let target = self
                .program
                .models
                .iter()
                .find(|candidate| candidate.package == relation.model)
                .expect("validated relation Model");
            let record_field = model.fields.len() + relation_index;
            let nullable = model
                .fields
                .iter()
                .find(|field| field.name == relation.field)
                .expect("validated to-one foreign key")
                .nullable;
            if nullable {
                write!(
                    code,
                    " let related = if matches!(values.as_slice().first(), Some(dever_runtime::orm::Value::Null)) {{ for _ in 0..{} {{ dever_runtime::orm::column(&mut values, {})?; }} None }} else {{ Some(decode_model_fields_{}(&mut values)?) }}; item.f{} = dever_runtime::orm::Related::Loaded(Box::new(related));",
                    target.fields.len(),
                    rust_string(&format!("{}.{}", target.package, relation.name)),
                    target.record,
                    record_field,
                )
                .expect("string formatting");
            } else {
                write!(
                    code,
                    " item.f{} = dever_runtime::orm::Related::Loaded(Box::new(decode_model_fields_{}(&mut values)?));",
                    record_field, target.record,
                )
                .expect("string formatting");
            }
        }
        write!(
            code,
            " if !values.as_slice().is_empty() {{ return Err(dever_runtime::orm::Error::invalid_data({})); }} Ok(item) }}",
            rust_string(&format!("database row for {} has extra columns", model.package)),
        )
        .expect("string formatting");
        // Keep decoder ?/return inside the ORM Result boundary, including
        // first(), whose caller is an AppError-returning function.
        format!(
            "(|| -> Result<T{}, dever_runtime::orm::Error> {code})()",
            model.record
        )
    }

    fn load_to_many(
        &self,
        model: &ModelSchema,
        query: &QueryPlan,
        items: &str,
        result: &Expression,
    ) -> String {
        let mut code = String::new();
        for relation_index in &query.relations {
            let relation = &model.relations[*relation_index];
            if relation.kind != ModelRelationKind::ToMany {
                continue;
            }
            let target = self
                .program
                .models
                .iter()
                .find(|candidate| candidate.package == relation.model)
                .expect("validated relation Model");
            let foreign_key = target
                .fields
                .iter()
                .position(|field| field.name == relation.field)
                .expect("validated reverse foreign key");
            let nullable = target.fields[foreign_key].nullable;
            let record_field = model.fields.len() + relation_index;
            let prefix = format!(
                "SELECT {} FROM (SELECT {}, ROW_NUMBER() OVER (PARTITION BY {} ORDER BY {} ASC) AS {} FROM {} WHERE {} IN (",
                select_columns(target),
                select_columns(target),
                quoted(&relation.field),
                quoted("id"),
                quoted("_dever_rank"),
                quoted(&target.table),
                quoted(&relation.field),
            );
            let before_limit = format!(
                ")) AS {} WHERE {} <= ",
                quoted("_dever_children"),
                quoted("_dever_rank"),
            );
            let suffix = format!(
                " ORDER BY {} ASC, {} ASC",
                quoted(&relation.field),
                quoted("id"),
            );
            let query_rows = self.database_failure(
                "database.query_owned(relation_sql, relation_parameters).await",
                result.span,
            );
            let decoded = self.database_failure(
                &format!(
                    "relation_rows.into_iter().map(decode_model_{}).collect::<Result<Vec<_>, _>>()",
                    target.record
                ),
                result.span,
            );
            let relation_limit = self.database_failure(
                "dever_runtime::orm::relation_limit(database.max_page_size())",
                result.span,
            );
            let parent_key = if nullable {
                format!(
                    "match child.f{foreign_key}.as_ref() {{ Some(parent) => parent.0, None => continue }}"
                )
            } else {
                format!("child.f{foreign_key}.0")
            };
            write!(
                code,
                "let mut relation_groups_{relation_index}: std::collections::BTreeMap<i64, Vec<T{}>> = std::collections::BTreeMap::new(); let relation_parent_ids_{relation_index} = {items}.iter().map(|parent| parent.f0.0).collect::<Vec<_>>(); let relation_limit_{relation_index} = {relation_limit}; for relation_ids in relation_parent_ids_{relation_index}.chunks(998) {{ let mut relation_parameters = relation_ids.iter().copied().map(dever_runtime::orm::Value::Int).collect::<Vec<_>>(); relation_parameters.push(dever_runtime::orm::Value::Int(relation_limit_{relation_index})); let relation_sql = dever_runtime::database::relation_sql({}, {}, {}, relation_ids.len()); let relation_rows = {query_rows}; let relation_children = {decoded}; for child in relation_children {{ let parent_id = {parent_key}; relation_groups_{relation_index}.entry(parent_id).or_default().push(child); }} }} for parent in {items}.iter_mut() {{ let children = relation_groups_{relation_index}.remove(&parent.f0.0).unwrap_or_default(); parent.f{record_field} = dever_runtime::orm::Related::Loaded(Box::new(List::new(children))); }}",
                target.record,
                rust_string(&prefix),
                rust_string(&before_limit),
                rust_string(&suffix),
            )
            .expect("string formatting");
        }
        code
    }

    fn count(&self, model: &ModelSchema, query: &QueryPlan, result: &Expression) -> String {
        self.scalar_query(model, query, result, "COUNT(*)", "int")
    }

    fn exists(&self, model: &ModelSchema, query: &QueryPlan, result: &Expression) -> String {
        self.scalar_query(
            model,
            query,
            result,
            &format!("EXISTS(SELECT 1 FROM {}", quoted(&model.table)),
            "boolean",
        )
    }

    fn scalar_query(
        &self,
        model: &ModelSchema,
        query: &QueryPlan,
        result: &Expression,
        projection: &str,
        decoder: &str,
    ) -> String {
        let mut expressions = Vec::new();
        let where_clause = condition_sql(model, query.condition.as_ref(), &mut expressions);
        let sql = if projection.starts_with("EXISTS(") {
            format!("SELECT {projection}{where_clause})")
        } else {
            format!(
                "SELECT {projection} FROM {}{where_clause}",
                quoted(&model.table)
            )
        };
        let (bindings, parameters) = self.bind_values(&expressions);
        let database = self.database(model, result);
        let rows = self.database_failure(
            &format!(
                "database.query({}, vec![{}]).await",
                database_sql(&sql),
                parameters.join(", ")
            ),
            result.span,
        );
        let row = self.database_failure(
            &format!(
                "rows.into_iter().next().ok_or_else(|| dever_runtime::orm::Error::invalid_data({}))",
                rust_string("database scalar query returned no row")
            ),
            result.span,
        );
        let value =
            self.database_failure(&format!("dever_runtime::orm::scalar({row})"), result.span);
        let decoded = self.database_failure(
            &format!("dever_runtime::orm::{decoder}({value})"),
            result.span,
        );
        format!("{{ {bindings} let database = {database}; let rows = {rows}; {decoded} }}")
    }

    fn update(
        &self,
        model: &ModelSchema,
        query: &QueryPlan,
        values: &[(usize, Expression)],
        result: &Expression,
    ) -> String {
        let mut expressions = values
            .iter()
            .map(|(field, value)| (value, &model.fields[*field].ty))
            .collect::<Vec<_>>();
        let assignments = update_assignments(model, values.iter().map(|(field, _)| *field), 1);
        let where_clause = condition_sql(model, query.condition.as_ref(), &mut expressions);
        let sql = format!(
            "UPDATE {} SET {assignments}{where_clause}",
            quoted(&model.table)
        );
        self.write(model, &sql, &expressions, result)
    }

    fn delete(&self, model: &ModelSchema, query: &QueryPlan, result: &Expression) -> String {
        let mut expressions = Vec::new();
        let where_clause = condition_sql(model, query.condition.as_ref(), &mut expressions);
        let sql = format!("DELETE FROM {}{where_clause}", quoted(&model.table));
        self.write(model, &sql, &expressions, result)
    }

    fn upsert(
        &self,
        model: &ModelSchema,
        key: &[(usize, Expression)],
        create: &[(usize, Expression)],
        update: &[(usize, Expression)],
        result: &Expression,
    ) -> String {
        let generated = model
            .fields
            .iter()
            .filter(|field| field.generated && matches!(field.ty, ModelFieldType::Uuid))
            .collect::<Vec<_>>();
        let inserted = key.iter().chain(create);
        let columns = inserted
            .clone()
            .map(|(field, _)| quoted(&model.fields[*field].name))
            .chain(generated.iter().map(|field| quoted(&field.name)))
            .collect::<Vec<_>>();
        let insert_types = key
            .iter()
            .chain(create)
            .map(|(field, _)| &model.fields[*field].ty)
            .chain(generated.iter().map(|field| &field.ty))
            .collect::<Vec<_>>();
        let insert_parameters = insert_types
            .iter()
            .enumerate()
            .map(|(index, ty)| parameter_sql(index + 1, ty))
            .collect::<Vec<_>>();
        let update_offset = insert_parameters.len();
        let assignments = update_assignments(
            model,
            update.iter().map(|(field, _)| *field),
            update_offset + 1,
        );
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {assignments} RETURNING {}",
            quoted(&model.table),
            columns.join(", "),
            insert_parameters.join(", "),
            key.iter()
                .map(|(field, _)| quoted(&model.fields[*field].name))
                .collect::<Vec<_>>()
                .join(", "),
            select_columns(model),
        );
        let expressions = key
            .iter()
            .chain(create)
            .chain(update)
            .map(|(field, value)| (value, &model.fields[*field].ty))
            .collect::<Vec<_>>();
        let (mut bindings, mut parameters) = self.bind_values(&expressions);
        for index in 0..generated.len() {
            let uuid = self.database_failure("dever_runtime::orm::Uuid::new_v7()", result.span);
            writeln!(
                bindings,
                "let generated_uuid_{index} = dever_runtime::orm::Value::Uuid({uuid});"
            )
            .expect("string formatting");
            parameters.insert(
                key.len() + create.len() + index,
                format!("generated_uuid_{index}"),
            );
        }
        let database = self.database(model, result);
        let rows = self.database_failure(
            &format!(
                "database.query({}, vec![{}]).await",
                database_sql(&sql),
                parameters.join(", ")
            ),
            result.span,
        );
        let row = self.database_failure(
            &format!(
                "rows.into_iter().next().ok_or_else(|| dever_runtime::orm::Error::invalid_data({}))",
                rust_string("database UPSERT RETURNING produced no row")
            ),
            result.span,
        );
        let decoded = self.database_failure(
            &format!("decode_model_{}({row})", model.record),
            result.span,
        );
        format!("{{ {bindings} let database = {database}; let rows = {rows}; {decoded} }}")
    }

    fn write(
        &self,
        model: &ModelSchema,
        sql: &str,
        expressions: &[(&Expression, &ModelFieldType)],
        result: &Expression,
    ) -> String {
        let (bindings, parameters) = self.bind_values(expressions);
        let database = self.database(model, result);
        let affected = self.database_failure(
            &format!(
                "database.execute({}, vec![{}]).await",
                database_sql(sql),
                parameters.join(", ")
            ),
            result.span,
        );
        let affected = self.database_failure(
            &format!("dever_runtime::orm::affected({affected})"),
            result.span,
        );
        format!("{{ {bindings} let database = {database}; {affected} }}")
    }

    fn database(&self, model: &ModelSchema, result: &Expression) -> String {
        let pool = self.database_at(model);
        self.database_failure(
            &format!("dever_runtime::database::Executor::new({pool}, _database)"),
            result.span,
        )
    }

    pub(super) fn database_at(&self, model: &ModelSchema) -> String {
        format!("_model_database_{}.clone()", model.record)
    }

    fn bind_values(&self, values: &[(&Expression, &ModelFieldType)]) -> (String, Vec<String>) {
        let mut bindings = String::new();
        let mut parameters = Vec::new();
        for (index, (value, storage)) in values.iter().enumerate() {
            writeln!(
                bindings,
                "let p{index}: {} = {};",
                rust_type(&value.ty),
                self.expression(value)
            )
            .expect("string formatting");
            parameters.push(self.encode_value(
                &value.ty,
                storage,
                &format!("p{index}"),
                value.span,
            ));
        }
        (bindings, parameters)
    }

    pub(super) fn encode_value(
        &self,
        ty: &Type,
        storage: &ModelFieldType,
        value: &str,
        span: Span,
    ) -> String {
        match ty {
            Type::Nullable(inner) => format!(
                "match {value} {{ Some(value) => {}, None => dever_runtime::orm::Value::Null }}",
                self.encode_value(inner, storage, "value", span)
            ),
            Type::Bool => format!("dever_runtime::orm::Value::Bool({value})"),
            Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration => {
                format!("dever_runtime::orm::Value::Int({value})")
            }
            Type::Float => format!("dever_runtime::orm::Value::Float({value})"),
            Type::Decimal => {
                let ModelFieldType::Decimal { precision, scale } = storage else {
                    unreachable!("Decimal expression bound to Decimal Model field")
                };
                self.database_failure(
                    &format!("dever_runtime::orm::decimal_value({value}, {precision}, {scale})"),
                    span,
                )
            }
            Type::Uuid => {
                format!("dever_runtime::orm::Value::Uuid({value})")
            }
            Type::Text | Type::Json => {
                format!("dever_runtime::orm::Value::Text({value})")
            }
            Type::Bytes => {
                format!("dever_runtime::orm::Value::Bytes({value}.into_values().collect())")
            }
            Type::Named(id) if self.program.types[*id].kind == DefinitionKind::ModelId => {
                format!("dever_runtime::orm::Value::Int({value}.0)")
            }
            Type::Named(id) if matches!(&self.program.types[*id].shape, Shape::Choice(variants) if variants.iter().all(|variant| !variant.error && variant.fields.is_empty())) =>
            {
                let Shape::Choice(variants) = &self.program.types[*id].shape else {
                    unreachable!()
                };
                let arms = variants
                    .iter()
                    .enumerate()
                    .map(|(variant, definition)| {
                        format!(
                            "T{id}::V{variant} => dever_runtime::orm::Value::Text(String::from({}))",
                            rust_string(&definition.name)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("match {value} {{ {arms} }}")
            }
            _ => unreachable!("checked Model parameter type"),
        }
    }

    fn decode_value(&self, ty: &Type, value: &str) -> String {
        match ty {
            Type::Nullable(inner) => format!(
                "dever_runtime::orm::nullable({value}, |value| {})?",
                self.decode_result(inner, "value")
            ),
            _ => format!("{}?", self.decode_result(ty, value)),
        }
    }

    fn decode_result(&self, ty: &Type, value: &str) -> String {
        match ty {
            Type::Bool => format!("dever_runtime::orm::boolean({value})"),
            Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration => {
                format!("dever_runtime::orm::int({value})")
            }
            Type::Float => format!("dever_runtime::orm::float({value})"),
            Type::Decimal => format!("dever_runtime::orm::decimal({value})"),
            Type::Text | Type::Json => {
                format!("dever_runtime::orm::text({value})")
            }
            Type::Uuid => format!("dever_runtime::orm::uuid({value})"),
            Type::Bytes => format!("dever_runtime::orm::bytes({value})"),
            Type::Named(id) if self.program.types[*id].kind == DefinitionKind::ModelId => {
                format!("dever_runtime::orm::int({value}).map(T{id})")
            }
            Type::Named(id) if matches!(&self.program.types[*id].shape, Shape::Choice(variants) if variants.iter().all(|variant| !variant.error && variant.fields.is_empty())) =>
            {
                let Shape::Choice(variants) = &self.program.types[*id].shape else {
                    unreachable!()
                };
                let arms = variants
                    .iter()
                    .enumerate()
                    .map(|(variant, definition)| {
                        format!("{} => Ok(T{id}::V{variant})", rust_string(&definition.name))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{{ let choice = dever_runtime::orm::text({value})?; match choice.as_str() {{ {arms}, _ => Err(dever_runtime::orm::Error::invalid_data(format!(\"invalid database choice '{{}}'\", choice))) }} }}"
                )
            }
            Type::Nullable(_) => unreachable!("nullable decoded by wrapper"),
            _ => unreachable!("checked Model field type"),
        }
    }
}

fn model_storage_selector(model: &ModelSchema) -> (String, &str) {
    match &model.connection {
        ConnectionSelector::Explicit(name) => (
            format!("Some({})", rust_string(name)),
            model.package.split('.').next().expect("Model package root"),
        ),
        ConnectionSelector::PackageRoot(root) => ("None".into(), root.as_str()),
    }
}

pub(crate) fn tenant_fingerprint(
    models: &[(usize, &ModelSchema)],
    program: &crate::hir::Program,
) -> String {
    let mut identities = models
        .iter()
        .map(|(_, model)| {
            let connection = match &model.connection {
                ConnectionSelector::Explicit(name) => format!("explicit:{name}"),
                ConnectionSelector::PackageRoot(root) => format!("root:{root}"),
            };
            format!("{}\0{}\0{connection}", model.package, model.revision)
        })
        .collect::<Vec<_>>();
    identities.extend(
        program
            .jobs
            .iter()
            .filter(|job| super::jobs::scope(program, job.function) == ModelScope::Tenant)
            .map(|job| {
                let connection = match &job.connection {
                    ConnectionSelector::Explicit(name) => format!("explicit:{name}"),
                    ConnectionSelector::PackageRoot(root) => format!("root:{root}"),
                };
                format!(
                    "job-v2\0{}\0{}\0{}\0{}\0{}\0{connection}",
                    program.functions[job.function].name,
                    job.payload
                        .as_ref()
                        .map(|schema| schema.fingerprint())
                        .unwrap_or("wire-v1:Job:Unit"),
                    job.attempts,
                    job.timeout_ms,
                    job.schedule.as_deref().unwrap_or(""),
                )
            }),
    );
    crate::model::checksum(&identities.join("\n"))
}

pub(crate) fn sqlite_table(model: &ModelSchema, models: &[ModelSchema], table: &str) -> String {
    let fields = model
        .fields
        .iter()
        .map(|field| sqlite_field(field, model, models))
        .collect::<Vec<_>>()
        .join(",\n  ");
    format!("CREATE TABLE {} (\n  {fields}\n)", quoted(table))
}

pub(crate) fn sqlite_indexes(model: &ModelSchema) -> Vec<String> {
    let mut statements = Vec::new();
    for index in &model.indexes {
        if index.fields.as_slice() == ["id"] {
            continue;
        }
        let name = index_name(model, index);
        statements.push(format!(
            "CREATE {}INDEX IF NOT EXISTS {} ON {} ({})",
            if index.unique { "UNIQUE " } else { "" },
            quoted(&name),
            quoted(&model.table),
            index
                .fields
                .iter()
                .map(|field| quoted(field))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    statements
}

pub(crate) fn index_name(model: &ModelSchema, index: &crate::model::ModelIndex) -> String {
    let identity = format!(
        "{}:{}:{}:{}",
        model.package,
        model.table,
        index.unique,
        index.fields.join("\0")
    );
    format!("_dever_idx_{}", crate::model::checksum(&identity))
}

fn sqlite_field(
    field: &crate::model::ModelField,
    model: &ModelSchema,
    models: &[ModelSchema],
) -> String {
    use crate::model::ModelFieldType;
    if field.name == "id" {
        return format!("{} INTEGER PRIMARY KEY AUTOINCREMENT", quoted("id"));
    }
    let mut definition = format!(
        "{} {}",
        quoted(&field.name),
        match field.ty {
            ModelFieldType::Bool
            | ModelFieldType::Int
            | ModelFieldType::DateTime
            | ModelFieldType::Date
            | ModelFieldType::Time
            | ModelFieldType::Duration
            | ModelFieldType::ModelId(_) => "INTEGER",
            ModelFieldType::Float => "REAL",
            ModelFieldType::Bytes { .. } | ModelFieldType::Uuid => "BLOB",
            ModelFieldType::Decimal { .. }
            | ModelFieldType::Text { .. }
            | ModelFieldType::Json
            | ModelFieldType::Choice(_) => "TEXT",
        }
    );
    if !field.nullable {
        definition.push_str(" NOT NULL");
    }
    if field.name == "created_at" {
        definition.push_str(" DEFAULT (unixepoch() * 1000)");
    } else if let Some(default) = &field.default {
        write!(
            definition,
            " DEFAULT {}",
            sqlite_default(default, &field.ty)
        )
        .expect("string formatting");
    }
    match &field.ty {
        ModelFieldType::Bool => {
            write!(definition, " CHECK ({} IN (0, 1))", quoted(&field.name))
                .expect("string formatting");
        }
        ModelFieldType::Decimal { precision, scale } => {
            let name = quoted(&field.name);
            let length = usize::from(*precision) + 5;
            let suffix = sql_text(&format!(":{scale:02}"));
            write!(
                definition,
                " CHECK ({name} IS NULL OR (typeof({name}) = 'text' AND length({name}) = {length} AND substr({name}, 1, 2) IN ('D0', 'D1') AND substr({name}, 3, {precision}) NOT GLOB '*[^0-9]*' AND substr({name}, {}) = {suffix}))",
                usize::from(*precision) + 3,
            )
            .expect("string formatting");
        }
        ModelFieldType::Uuid => {
            let name = quoted(&field.name);
            write!(
                definition,
                " CHECK ({name} IS NULL OR (typeof({name}) = 'blob' AND length({name}) = 16))"
            )
            .expect("string formatting");
        }
        ModelFieldType::Text { minimum, maximum } | ModelFieldType::Bytes { minimum, maximum } => {
            if *minimum > 0 {
                write!(
                    definition,
                    " CHECK (length({}) >= {minimum})",
                    quoted(&field.name)
                )
                .expect("string formatting");
            }
            if let Some(maximum) = maximum {
                write!(
                    definition,
                    " CHECK (length({}) <= {maximum})",
                    quoted(&field.name)
                )
                .expect("string formatting");
            }
        }
        ModelFieldType::Choice(choice) => {
            let options = model
                .choices
                .iter()
                .find(|candidate| candidate.name == *choice)
                .expect("validated Model choice")
                .options
                .iter()
                .map(|(name, _)| sql_text(name))
                .collect::<Vec<_>>()
                .join(", ");
            write!(
                definition,
                " CHECK ({} IN ({options}))",
                quoted(&field.name)
            )
            .expect("string formatting");
        }
        ModelFieldType::ModelId(target) => {
            if field.logical_reference {
                return definition;
            }
            let table = &models
                .iter()
                .find(|candidate| candidate.package == *target)
                .expect("validated Model foreign key")
                .table;
            write!(
                definition,
                " REFERENCES {} ({})",
                quoted(table),
                quoted("id")
            )
            .expect("string formatting");
        }
        _ => {}
    }
    definition
}

pub(crate) fn postgres_table(
    model: &ModelSchema,
    constraints: &[(String, String, bool)],
    table: &str,
) -> String {
    let mut fields = model.fields.iter().map(postgres_column).collect::<Vec<_>>();
    fields.extend(
        constraints
            .iter()
            .filter(|(_, _, foreign_key)| !foreign_key)
            .map(|(name, definition, _)| format!("CONSTRAINT {} {definition}", quoted(name))),
    );
    format!(
        "CREATE TABLE {} (\n  {}\n)",
        quoted(table),
        fields.join(",\n  ")
    )
}

pub(crate) fn postgres_column(field: &crate::model::ModelField) -> String {
    if field.name == "id" {
        return format!(
            "{} BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY",
            quoted("id")
        );
    }
    let mut definition = format!("{} {}", quoted(&field.name), postgres_type(&field.ty),);
    if !field.nullable {
        definition.push_str(" NOT NULL");
    }
    if let Some(default) = postgres_field_default(field) {
        write!(definition, " DEFAULT {default}").expect("string formatting");
    }
    definition
}

pub(crate) fn postgres_constraints(
    model: &ModelSchema,
    models: &[ModelSchema],
) -> Vec<(String, String, bool)> {
    use crate::model::ModelFieldType;
    let mut constraints = Vec::new();
    for field in &model.fields {
        match &field.ty {
            ModelFieldType::Text { minimum, maximum } => {
                if *minimum > 0 {
                    constraints.push(postgres_length_constraint(
                        model,
                        field,
                        "min",
                        "char_length",
                        ">=",
                        *minimum,
                    ));
                }
                if let Some(maximum) = maximum {
                    constraints.push(postgres_length_constraint(
                        model,
                        field,
                        "max",
                        "char_length",
                        "<=",
                        *maximum,
                    ));
                }
            }
            ModelFieldType::Bytes { minimum, maximum } => {
                if *minimum > 0 {
                    constraints.push(postgres_length_constraint(
                        model,
                        field,
                        "min",
                        "octet_length",
                        ">=",
                        *minimum,
                    ));
                }
                if let Some(maximum) = maximum {
                    constraints.push(postgres_length_constraint(
                        model,
                        field,
                        "max",
                        "octet_length",
                        "<=",
                        *maximum,
                    ));
                }
            }
            ModelFieldType::Choice(choice) => {
                let options = model
                    .choices
                    .iter()
                    .find(|candidate| candidate.name == *choice)
                    .expect("validated Model choice")
                    .options
                    .iter()
                    .map(|(name, _)| sql_text(name))
                    .collect::<Vec<_>>()
                    .join(", ");
                constraints.push((
                    postgres_constraint_name("ck", model, field, "choice"),
                    format!("CHECK ({} IN ({options}))", quoted(&field.name)),
                    false,
                ));
            }
            ModelFieldType::ModelId(target) => {
                if field.generated || field.logical_reference {
                    continue;
                }
                let table = &models
                    .iter()
                    .find(|candidate| candidate.package == *target)
                    .expect("validated Model foreign key")
                    .table;
                constraints.push((
                    postgres_constraint_name("fk", model, field, target),
                    format!(
                        "FOREIGN KEY ({}) REFERENCES {} ({})",
                        quoted(&field.name),
                        quoted(table),
                        quoted("id")
                    ),
                    true,
                ));
            }
            _ => {}
        }
    }
    constraints
}

fn postgres_length_constraint(
    model: &ModelSchema,
    field: &crate::model::ModelField,
    suffix: &str,
    function: &str,
    operator: &str,
    bound: u32,
) -> (String, String, bool) {
    (
        postgres_constraint_name("ck", model, field, suffix),
        format!(
            "CHECK ({function}({}) {operator} {bound})",
            quoted(&field.name)
        ),
        false,
    )
}

fn postgres_constraint_name(
    kind: &str,
    model: &ModelSchema,
    field: &crate::model::ModelField,
    purpose: &str,
) -> String {
    format!(
        "_dever_{kind}_{}",
        crate::model::checksum(&format!(
            "{}:{}:{}:{purpose}",
            model.package, model.table, field.name
        ))
    )
}

pub(crate) fn postgres_type(ty: &crate::model::ModelFieldType) -> String {
    use crate::model::ModelFieldType;
    match ty {
        ModelFieldType::Bool => "BOOLEAN".into(),
        ModelFieldType::Int
        | ModelFieldType::DateTime
        | ModelFieldType::Date
        | ModelFieldType::Time
        | ModelFieldType::Duration
        | ModelFieldType::ModelId(_) => "BIGINT".into(),
        ModelFieldType::Float => "DOUBLE PRECISION".into(),
        ModelFieldType::Bytes { .. } => "BYTEA".into(),
        ModelFieldType::Decimal { precision, scale } => {
            format!("NUMERIC({precision},{scale})")
        }
        ModelFieldType::Uuid => "UUID".into(),
        ModelFieldType::Text { .. } | ModelFieldType::Json | ModelFieldType::Choice(_) => {
            "TEXT".into()
        }
    }
}

pub(crate) fn postgres_field_default(field: &crate::model::ModelField) -> Option<String> {
    if field.name == "created_at" {
        Some("((extract(epoch from clock_timestamp()) * 1000)::bigint)".into())
    } else {
        field.default.as_ref().map(postgres_default)
    }
}

fn postgres_default(value: &crate::model::ModelValue) -> String {
    match value {
        crate::model::ModelValue::Bool(value) => {
            if *value {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        value => sql_default(value),
    }
}

fn sqlite_default(value: &crate::model::ModelValue, ty: &crate::model::ModelFieldType) -> String {
    match (value, ty) {
        (
            crate::model::ModelValue::Number(value),
            crate::model::ModelFieldType::Decimal { precision, scale },
        ) => sql_text(
            &dever_runtime::orm::decimal_storage(value, *precision, *scale)
                .expect("checked Decimal default"),
        ),
        (crate::model::ModelValue::Text(value), crate::model::ModelFieldType::Uuid) => {
            let bytes = dever_runtime::orm::Uuid::parse(value)
                .expect("checked Uuid default")
                .into_bytes()
                .into_iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("X'{bytes}'")
        }
        _ => sql_default(value),
    }
}

fn sql_default(value: &crate::model::ModelValue) -> String {
    use crate::model::ModelValue;
    match value {
        ModelValue::Null => "NULL".into(),
        ModelValue::Bool(value) => i64::from(*value).to_string(),
        ModelValue::Number(value) => value.clone(),
        ModelValue::Text(value) => sql_text(value),
        ModelValue::Choice(value) => sql_text(value.rsplit('.').next().unwrap()),
    }
}

fn seed_value(value: &crate::model::ModelValue, ty: &crate::model::ModelFieldType) -> String {
    use crate::model::{ModelFieldType, ModelValue};
    match value {
        ModelValue::Null => "dever_runtime::orm::Value::Null".into(),
        ModelValue::Bool(value) => format!("dever_runtime::orm::Value::Bool({value})"),
        ModelValue::Number(value) if matches!(ty, ModelFieldType::Float) => {
            format!("dever_runtime::orm::Value::Float({value}f64)")
        }
        ModelValue::Number(value) if matches!(ty, ModelFieldType::Decimal { .. }) => {
            let ModelFieldType::Decimal { precision, scale } = ty else {
                unreachable!()
            };
            let value = dever_runtime::orm::decimal_storage(value, *precision, *scale)
                .expect("checked Decimal Seed value");
            format!(
                "dever_runtime::orm::Value::Decimal(String::from({}))",
                rust_string(&value)
            )
        }
        ModelValue::Number(value) => {
            format!("dever_runtime::orm::Value::Int({value}i64)")
        }
        ModelValue::Text(value) if matches!(ty, ModelFieldType::Uuid) => {
            let bytes = dever_runtime::orm::Uuid::parse(value)
                .expect("checked Uuid Seed value")
                .into_bytes()
                .into_iter()
                .map(|byte| format!("{byte}u8"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "dever_runtime::orm::Value::Uuid(dever_runtime::orm::Uuid::from_bytes([{bytes}]))"
            )
        }
        ModelValue::Text(value) => format!(
            "dever_runtime::orm::Value::Text(String::from({}))",
            rust_string(value)
        ),
        ModelValue::Choice(value) => format!(
            "dever_runtime::orm::Value::Text(String::from({}))",
            rust_string(value.rsplit('.').next().unwrap())
        ),
    }
}

fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub(super) fn database_sql(sqlite: &str) -> String {
    database_sql_pair(sqlite, sqlite)
}

fn database_sql_pair(sqlite: &str, postgres: &str) -> String {
    let (sqlite, postgres) = sql_dialects(sqlite, postgres);
    format!(
        "dever_runtime::database::Sql {{ sqlite: {}, postgres: {} }}",
        rust_string(&sqlite),
        rust_string(&postgres)
    )
}

/// Both native emitters consume the same checked dialect substitutions.
pub(crate) fn sql_dialects(sqlite: &str, postgres: &str) -> (String, String) {
    let postgres = postgres
        .replace("__DEVER_CONTAINS__", "strpos")
        .replace("__DEVER_NOT_EQUAL__", "IS DISTINCT FROM")
        .replace("__DEVER_EQUAL__", "IS NOT DISTINCT FROM");
    let sqlite = sqlite
        .replace("__DEVER_CONTAINS__", "instr")
        .replace("__DEVER_EQUAL__", "IS")
        .replace("__DEVER_NOT_EQUAL__", "IS NOT");
    (
        typed_placeholders(&sqlite, false),
        typed_placeholders(&postgres, true),
    )
}

pub(crate) fn cursor_condition(
    model: &ModelSchema,
    order: &[crate::hir::ModelOrder],
    first_parameter: usize,
    table: Option<&str>,
) -> String {
    let placeholders = order
        .iter()
        .enumerate()
        .map(|(index, order)| parameter_sql(first_parameter + index, &model.fields[order.field].ty))
        .collect::<Vec<_>>();
    let branches = order
        .iter()
        .enumerate()
        .map(|(index, current)| {
            let mut terms = order[..index]
                .iter()
                .enumerate()
                .map(|(previous, prior)| {
                    format!(
                        "{} __DEVER_EQUAL__ {}",
                        qualified(table, &model.fields[prior.field].name),
                        placeholders[previous]
                    )
                })
                .collect::<Vec<_>>();
            terms.push(format!(
                "{} {} {}",
                qualified(table, &model.fields[current.field].name),
                if current.descending { "<" } else { ">" },
                placeholders[index]
            ));
            format!("({})", terms.join(" AND "))
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "(CAST({} AS {}) IS NULL OR ({branches}))",
        placeholders[0],
        postgres_type(&model.fields[order[0].field].ty),
    )
}

fn typed_placeholders(sql: &str, postgres: bool) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut characters = sql.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '?' || !characters.peek().is_some_and(char::is_ascii_digit) {
            output.push(character);
            continue;
        }
        output.push(if postgres { '$' } else { '?' });
        while characters.peek().is_some_and(char::is_ascii_digit) {
            output.push(characters.next().expect("peeked placeholder digit"));
        }
        let marker = characters.next_if(|marker| matches!(marker, 'D' | 'U'));
        if postgres {
            match marker {
                Some('D') => output.push_str("::text::numeric"),
                Some('U') => output.push_str("::uuid"),
                _ => {}
            }
        }
    }
    output
}

pub(crate) fn parameter_sql(index: usize, ty: &ModelFieldType) -> String {
    let marker = match ty {
        ModelFieldType::Decimal { .. } => "D",
        ModelFieldType::Uuid => "U",
        _ => "",
    };
    format!("?{index}{marker}")
}

pub(crate) fn update_assignments(
    model: &ModelSchema,
    fields: impl IntoIterator<Item = usize>,
    first_parameter: usize,
) -> String {
    fields
        .into_iter()
        .enumerate()
        .map(|(offset, field)| {
            format!(
                "{} = {}",
                quoted(&model.fields[field].name),
                parameter_sql(first_parameter + offset, &model.fields[field].ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn select_columns(model: &ModelSchema) -> String {
    select_columns_qualified(model, None)
}

fn select_columns_qualified(model: &ModelSchema, table: Option<&str>) -> String {
    model
        .fields
        .iter()
        .map(|field| qualified(table, &field.name))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn has_to_one(model: &ModelSchema, query: &QueryPlan) -> bool {
    query
        .relations
        .iter()
        .any(|index| model.relations[*index].kind == ModelRelationKind::ToOne)
}

pub(crate) fn select_source(
    model: &ModelSchema,
    query: &QueryPlan,
    models: &[ModelSchema],
) -> (String, String) {
    if !has_to_one(model, query) {
        return (select_columns(model), quoted(&model.table));
    }
    let parent = "_dever_parent";
    let mut projection = vec![select_columns_qualified(model, Some(parent))];
    let mut source = format!("{} AS {}", quoted(&model.table), quoted(parent));
    let mut join_index = 0;
    for relation_index in &query.relations {
        let relation = &model.relations[*relation_index];
        if relation.kind != ModelRelationKind::ToOne {
            continue;
        }
        let target = models
            .iter()
            .find(|candidate| candidate.package == relation.model)
            .expect("validated relation Model");
        let alias = format!("_dever_relation_{join_index}");
        let nullable = model
            .fields
            .iter()
            .find(|field| field.name == relation.field)
            .expect("validated to-one foreign key")
            .nullable;
        write!(
            source,
            " {} JOIN {} AS {} ON {} = {}",
            if nullable { "LEFT" } else { "INNER" },
            quoted(&target.table),
            quoted(&alias),
            qualified(Some(parent), &relation.field),
            qualified(Some(&alias), "id"),
        )
        .expect("string formatting");
        projection.push(select_columns_qualified(target, Some(&alias)));
        join_index += 1;
    }
    (projection.join(", "), source)
}

pub(crate) fn order_sql(model: &ModelSchema, order: &[crate::hir::ModelOrder]) -> String {
    order_sql_qualified(model, order, None)
}

pub(crate) fn order_sql_qualified(
    model: &ModelSchema,
    order: &[crate::hir::ModelOrder],
    table: Option<&str>,
) -> String {
    order
        .iter()
        .map(|order| {
            format!(
                "{} {}",
                qualified(table, &model.fields[order.field].name),
                if order.descending { "DESC" } else { "ASC" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn condition_sql<'a>(
    model: &'a ModelSchema,
    condition: Option<&'a ModelCondition>,
    parameters: &mut Vec<(&'a Expression, &'a ModelFieldType)>,
) -> String {
    condition_sql_qualified(model, condition, parameters, None)
}

pub(crate) fn condition_sql_qualified<'a>(
    model: &'a ModelSchema,
    condition: Option<&'a ModelCondition>,
    parameters: &mut Vec<(&'a Expression, &'a ModelFieldType)>,
    table: Option<&str>,
) -> String {
    condition
        .map(|condition| {
            format!(
                " WHERE {}",
                condition_expression_qualified(model, condition, parameters, table)
            )
        })
        .unwrap_or_default()
}

fn condition_expression_qualified<'a>(
    model: &'a ModelSchema,
    condition: &'a ModelCondition,
    parameters: &mut Vec<(&'a Expression, &'a ModelFieldType)>,
    table: Option<&str>,
) -> String {
    match condition {
        ModelCondition::Compare {
            field,
            operator,
            value,
        } => {
            let field_index = *field;
            let field = qualified(table, &model.fields[field_index].name);
            if matches!(
                value.kind,
                crate::hir::ExpressionKind::Constant(crate::hir::Constant::Null)
            ) {
                return format!(
                    "{field} IS {}NULL",
                    if *operator == BinaryOperator::NotEqual {
                        "NOT "
                    } else {
                        ""
                    }
                );
            }
            let ty = &model.fields[field_index].ty;
            parameters.push((value, ty));
            let operator = match operator {
                BinaryOperator::Equal => "__DEVER_EQUAL__",
                BinaryOperator::NotEqual => "__DEVER_NOT_EQUAL__",
                BinaryOperator::Less => "<",
                BinaryOperator::LessEqual => "<=",
                BinaryOperator::Greater => ">",
                BinaryOperator::GreaterEqual => ">=",
                _ => unreachable!("checked Model comparison"),
            };
            format!("{field} {operator} {}", parameter_sql(parameters.len(), ty))
        }
        ModelCondition::In { field, values } => {
            let ty = &model.fields[*field].ty;
            let placeholders = values
                .iter()
                .map(|value| {
                    parameters.push((value, ty));
                    parameter_sql(parameters.len(), ty)
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{} IN ({placeholders})",
                qualified(table, &model.fields[*field].name)
            )
        }
        ModelCondition::Between {
            field,
            lower,
            upper,
        } => {
            let ty = &model.fields[*field].ty;
            parameters.push((lower, ty));
            let lower = parameter_sql(parameters.len(), ty);
            parameters.push((upper, ty));
            let upper = parameter_sql(parameters.len(), ty);
            format!(
                "{} BETWEEN {lower} AND {upper}",
                qualified(table, &model.fields[*field].name)
            )
        }
        ModelCondition::Text {
            field,
            operation,
            value,
        } => {
            let ty = &model.fields[*field].ty;
            parameters.push((value, ty));
            let parameter = parameter_sql(parameters.len(), ty);
            let field = qualified(table, &model.fields[*field].name);
            match operation {
                crate::hir::TextCondition::Contains => {
                    format!("__DEVER_CONTAINS__({field}, {parameter}) > 0")
                }
                crate::hir::TextCondition::StartsWith => {
                    format!("substr({field}, 1, length({parameter})) = {parameter}")
                }
                crate::hir::TextCondition::EndsWith => format!(
                    "substr({field}, length({field}) - length({parameter}) + 1) = {parameter}"
                ),
            }
        }
        ModelCondition::Not(condition) => {
            format!(
                "NOT ({})",
                condition_expression_qualified(model, condition, parameters, table)
            )
        }
        ModelCondition::And(left, right) | ModelCondition::Or(left, right) => {
            let left = condition_expression_qualified(model, left, parameters, table);
            let right = condition_expression_qualified(model, right, parameters, table);
            let operator = if matches!(condition, ModelCondition::And(_, _)) {
                "AND"
            } else {
                "OR"
            };
            format!("({left} {operator} {right})")
        }
    }
}

pub(crate) fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn qualified(table: Option<&str>, field: &str) -> String {
    match table {
        Some(table) => format!("{}.{}", quoted(table), quoted(field)),
        None => quoted(field),
    }
}
