//! Checked Model operations lower to static SQL and concrete parameter/row code.
use super::*;
use crate::hir::{ModelOperation, QueryPlan};
use crate::model::{ModelFieldType, ModelSchema, ModelSqlCardinality};
use crate::native::orm as sql;

pub(super) type DatabaseValues = Vec<(Type, ModelFieldType, String)>;
type ModelWrite = [(usize, Expression)];

impl FunctionEmitter<'_, '_> {
    pub(super) fn model_operation(
        &mut self,
        index: usize,
        operation: &ModelOperation,
        result: &Expression,
    ) -> Result<String, String> {
        let model = self.module.program.models[index].clone();
        match operation {
            ModelOperation::Create { values } => {
                self.model_insert(index, &model, values, None, result)
            }
            ModelOperation::Upsert {
                key,
                create,
                update,
            } => {
                let insert = key.iter().chain(create).cloned().collect::<Vec<_>>();
                self.model_insert(index, &model, &insert, Some((key, update)), result)
            }
            ModelOperation::CreateMany { rows } => {
                self.model_insert_many(index, &model, rows, result)
            }
            ModelOperation::Get { id } => {
                let value = self.expression(id)?;
                let values = vec![(id.ty.clone(), model.fields[0].ty.clone(), value)];
                let query = self.module.planned_sql(&format!(
                    "SELECT {} FROM {} WHERE {} = ?1 LIMIT 1",
                    sql::select_columns(&model),
                    sql::quoted(&model.table),
                    sql::quoted("id")
                ));
                let rows = self.model_query(index, &query, &values, "0", result.span);
                Ok(self.model_first(
                    &rows,
                    &model,
                    None,
                    Some((7, format!("{} was not found", model.package))),
                    result.span,
                ))
            }
            ModelOperation::Update { query, values } => {
                let mut expressions = values
                    .iter()
                    .map(|(field, value)| (value, &model.fields[*field].ty))
                    .collect::<Vec<_>>();
                let assignments =
                    sql::update_assignments(&model, values.iter().map(|(field, _)| *field), 1);
                let condition =
                    sql::condition_sql(&model, query.condition.as_ref(), &mut expressions);
                let values = self.model_bind(&expressions)?;
                let statement = self.module.planned_sql(&format!(
                    "UPDATE {} SET {assignments}{condition}",
                    sql::quoted(&model.table)
                ));
                Ok(self.model_execute(index, &statement, &values, result.span))
            }
            ModelOperation::Delete { query } => {
                let mut expressions = Vec::new();
                let condition =
                    sql::condition_sql(&model, query.condition.as_ref(), &mut expressions);
                let values = self.model_bind(&expressions)?;
                let statement = self.module.planned_sql(&format!(
                    "DELETE FROM {}{condition}",
                    sql::quoted(&model.table)
                ));
                Ok(self.model_execute(index, &statement, &values, result.span))
            }
            ModelOperation::Count { query } | ModelOperation::Exists { query } => {
                let mut expressions = Vec::new();
                let condition =
                    sql::condition_sql(&model, query.condition.as_ref(), &mut expressions);
                let values = self.model_bind(&expressions)?;
                let statement = if matches!(operation, ModelOperation::Count { .. }) {
                    format!(
                        "SELECT COUNT(*) FROM {}{condition}",
                        sql::quoted(&model.table)
                    )
                } else {
                    format!(
                        "SELECT EXISTS(SELECT 1 FROM {}{condition})",
                        sql::quoted(&model.table)
                    )
                };
                let statement = self.module.planned_sql(&statement);
                let rows = self.model_query(index, &statement, &values, "0", result.span);
                Ok(self.model_scalar_rows(&rows, &result.ty, result.span))
            }
            ModelOperation::Sql {
                operation,
                arguments,
                ..
            } => {
                let operation = &model.sql[*operation];
                let expressions = arguments
                    .iter()
                    .zip(&operation.parameters)
                    .map(|(argument, parameter)| (argument, &parameter.ty))
                    .collect::<Vec<_>>();
                let values = self.model_bind(&expressions)?;
                let statement = self
                    .module
                    .database_sql(&operation.sqlite, &operation.postgres);
                let maximum = if operation.result.cardinality == ModelSqlCardinality::Many {
                    self.model_maximum(index, result.span)
                } else {
                    "1".into()
                };
                let rows = self.model_query(index, &statement, &values, &maximum, result.span);
                if operation.result.cardinality == ModelSqlCardinality::Many {
                    return Ok(self.model_rows_list(
                        index,
                        &rows,
                        operation.result.record,
                        None,
                        None,
                        result.span,
                    ));
                }
                let name = self
                    .module
                    .wire_names(&[format!("{}.{}", model.package, operation.name)]);
                let nullable = operation.result.cardinality == ModelSqlCardinality::Optional;
                let slot = self.entry_slot_ir("ptr");
                let present = self.entry_slot_ir("i8");
                let error = self.entry_slot_ir("{ ptr, i64 }");
                let errors = self.database_errors(result.span);
                let status = self.temp();
                let symbol = if nullable { "optional" } else { "required" };
                let extra = if nullable {
                    format!(", ptr {present}")
                } else {
                    String::new()
                };
                self.line(format!("{status} = call i32 @dever_rt_v1_db_{symbol}_row(ptr {rows}, ptr {name}, ptr {slot}{extra}, ptr {errors}, ptr %fault, ptr {error})"));
                self.database_status(&status, &error, result.span);
                let output = self.entry_slot(&result.ty);
                self.line(format!(
                    "store {} zeroinitializer, ptr {output}",
                    self.module.ty(&result.ty)
                ));
                let done = self.label("database_sql_done");
                if nullable {
                    let found = self.temp();
                    self.line(format!("{found} = load i8, ptr {present}"));
                    let has_row = self.temp();
                    self.line(format!("{has_row} = icmp ne i8 {found}, 0"));
                    let decode = self.label("database_sql_decode");
                    self.line(format!("br i1 {has_row}, label %{decode}, label %{done}"));
                    self.start(&decode);
                }
                let row = self.temp();
                self.line(format!("{row} = load ptr, ptr {slot}"));
                let owner = self.protocol_owner("db_row", &row);
                let value =
                    self.database_record(&row, operation.result.record, 0, true, result.span);
                let value = if nullable {
                    self.model_some(&Type::Named(operation.result.record), &value)
                } else {
                    value
                };
                self.line(format!(
                    "store {} {value}, ptr {output}",
                    self.module.ty(&result.ty)
                ));
                self.release_guard(owner);
                self.line(format!("br label %{done}"));
                self.start(&done);
                let value = self.temp();
                self.line(format!(
                    "{value} = load {}, ptr {output}",
                    self.module.ty(&result.ty)
                ));
                Ok(value)
            }
            ModelOperation::First { query } => {
                let (statement, values) = self.model_select(&model, query, " LIMIT 1")?;
                let rows = self.model_query(index, &statement, &values, "0", result.span);
                Ok(self.model_first(&rows, &model, Some(query), None, result.span))
            }
            ModelOperation::List { query } => self.model_page(index, &model, query, result),
            ModelOperation::Cursor { query } => self.model_cursor(index, &model, query, result),
            ModelOperation::Stream { query } => self.model_stream(index, &model, query, result),
        }
    }

    pub(super) fn model_bind(
        &mut self,
        expressions: &[(&Expression, &ModelFieldType)],
    ) -> Result<DatabaseValues, String> {
        expressions
            .iter()
            .map(|(expression, storage)| {
                Ok((
                    expression.ty.clone(),
                    (*storage).clone(),
                    self.expression(expression)?,
                ))
            })
            .collect()
    }

    pub(super) fn model_maximum(&mut self, model: usize, span: Span) -> String {
        let database = self.model_database(model);
        self.protocol_call(
            "dever_rt_v1_db_max_page_size",
            vec![format!("ptr {database}")],
            "i64",
            span,
        )
    }

    pub(super) fn model_query(
        &mut self,
        model: usize,
        query: &str,
        values: &[(Type, ModelFieldType, String)],
        maximum: &str,
        span: Span,
    ) -> String {
        let database = self.model_database(model);
        let parameters = self.database_parameters(values, span);
        let operation = self.database_operation(
            "db_query",
            vec![
                format!("ptr {database}"),
                format!("ptr {}", self.database_context),
                format!("ptr {query}"),
                format!("ptr {parameters}"),
                format!("i64 {}", values.len()),
                format!("i64 {maximum}"),
            ],
            span,
        );
        let rows = self.await_database_handle(&operation, span);
        self.protocol_owner("db_rows", &rows);
        rows
    }

    fn model_execute(
        &mut self,
        model: usize,
        query: &str,
        values: &[(Type, ModelFieldType, String)],
        span: Span,
    ) -> String {
        let database = self.model_database(model);
        let parameters = self.database_parameters(values, span);
        let operation = self.database_operation(
            "db_execute",
            vec![
                format!("ptr {database}"),
                format!("ptr {}", self.database_context),
                format!("ptr {query}"),
                format!("ptr {parameters}"),
                format!("i64 {}", values.len()),
            ],
            span,
        );
        self.await_operation(&operation, &Type::Int, span, false)
    }

    fn model_insert(
        &mut self,
        index: usize,
        model: &ModelSchema,
        insert: &[(usize, Expression)],
        upsert: Option<(&ModelWrite, &ModelWrite)>,
        result: &Expression,
    ) -> Result<String, String> {
        let mut expressions = insert
            .iter()
            .map(|(field, expression)| (expression, &model.fields[*field].ty))
            .collect::<Vec<_>>();
        if let Some((_, update)) = upsert {
            expressions.extend(
                update
                    .iter()
                    .map(|(field, expression)| (expression, &model.fields[*field].ty)),
            );
        }
        let mut values = self.model_bind(&expressions)?;
        let mut columns = insert
            .iter()
            .map(|(field, _)| sql::quoted(&model.fields[*field].name))
            .collect::<Vec<_>>();
        let generated = model
            .fields
            .iter()
            .filter(|field| field.generated && field.ty == ModelFieldType::Uuid)
            .collect::<Vec<_>>();
        for (offset, field) in generated.iter().enumerate() {
            let uuid = self.database_call("db_uuid_new", vec![], "ptr", result.span);
            self.own_value(&Type::Uuid, &uuid, false);
            values.insert(
                insert.len() + offset,
                (Type::Uuid, ModelFieldType::Uuid, uuid),
            );
            columns.push(sql::quoted(&field.name));
        }
        let insert_count = insert.len() + generated.len();
        let placeholders = values[..insert_count]
            .iter()
            .enumerate()
            .map(|(index, (_, storage, _))| sql::parameter_sql(index + 1, storage))
            .collect::<Vec<_>>()
            .join(", ");
        let body = if columns.is_empty() {
            "DEFAULT VALUES".into()
        } else {
            format!("({}) VALUES ({placeholders})", columns.join(", "))
        };
        let conflict = if let Some((key, update)) = upsert {
            let keys = key
                .iter()
                .map(|(field, _)| sql::quoted(&model.fields[*field].name))
                .collect::<Vec<_>>()
                .join(", ");
            let assignments = sql::update_assignments(
                model,
                update.iter().map(|(field, _)| *field),
                insert_count + 1,
            );
            format!(" ON CONFLICT ({keys}) DO UPDATE SET {assignments}")
        } else {
            String::new()
        };
        let query = self.module.planned_sql(&format!(
            "INSERT INTO {} {body}{conflict} RETURNING {}",
            sql::quoted(&model.table),
            sql::select_columns(model)
        ));
        let rows = self.model_query(index, &query, &values, "0", result.span);
        let message = if upsert.is_some() {
            "database UPSERT RETURNING produced no row"
        } else {
            "database INSERT RETURNING produced no row"
        };
        Ok(self.model_first(&rows, model, None, Some((8, message.into())), result.span))
    }

    fn model_insert_many(
        &mut self,
        index: usize,
        model: &ModelSchema,
        rows: &[Vec<(usize, Expression)>],
        result: &Expression,
    ) -> Result<String, String> {
        let expressions = rows
            .iter()
            .flatten()
            .map(|(field, value)| (value, &model.fields[*field].ty))
            .collect::<Vec<_>>();
        let mut values = self.model_bind(&expressions)?;
        let generated = model
            .fields
            .iter()
            .filter(|field| field.generated && field.ty == ModelFieldType::Uuid)
            .collect::<Vec<_>>();
        for _ in 0..rows.len() * generated.len() {
            let uuid = self.database_call("db_uuid_new", vec![], "ptr", result.span);
            self.own_value(&Type::Uuid, &uuid, false);
            values.push((Type::Uuid, ModelFieldType::Uuid, uuid));
        }
        let columns = rows[0]
            .iter()
            .map(|(field, _)| sql::quoted(&model.fields[*field].name))
            .chain(generated.iter().map(|field| sql::quoted(&field.name)))
            .collect::<Vec<_>>();
        let table = sql::quoted(&model.table);
        let (sqlite, postgres) = if columns.is_empty() {
            (
                format!(
                    "INSERT INTO {table} ({}) VALUES {}",
                    sql::quoted("id"),
                    vec!["(NULL)"; rows.len()].join(", ")
                ),
                format!(
                    "INSERT INTO {table} ({}) VALUES {}",
                    sql::quoted("created_at"),
                    vec!["(DEFAULT)"; rows.len()].join(", ")
                ),
            )
        } else {
            let explicit = expressions.len();
            let groups = rows
                .iter()
                .enumerate()
                .map(|(row, fields)| {
                    let placeholders = fields
                        .iter()
                        .enumerate()
                        .map(|(column, (field, _))| {
                            sql::parameter_sql(
                                row * fields.len() + column + 1,
                                &model.fields[*field].ty,
                            )
                        })
                        .chain(generated.iter().enumerate().map(|(column, field)| {
                            sql::parameter_sql(
                                explicit + row * generated.len() + column + 1,
                                &field.ty,
                            )
                        }))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({placeholders})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "INSERT INTO {table} ({}) VALUES {groups}",
                columns.join(", ")
            );
            (query.clone(), query)
        };
        let (sqlite, postgres) = sql::sql_dialects(&sqlite, &postgres);
        let query = self.module.database_sql(&sqlite, &postgres);
        Ok(self.model_execute(index, &query, &values, result.span))
    }

    pub(super) fn model_select(
        &mut self,
        model: &ModelSchema,
        query: &QueryPlan,
        suffix: &str,
    ) -> Result<(String, DatabaseValues), String> {
        let mut expressions = Vec::new();
        let alias = sql::has_to_one(model, query).then_some("_dever_parent");
        let condition =
            sql::condition_sql_qualified(model, query.condition.as_ref(), &mut expressions, alias);
        let (projection, source) = sql::select_source(model, query, &self.module.program.models);
        let order = sql::order_sql_qualified(model, &query.order, alias);
        let statement = self.module.planned_sql(&format!(
            "SELECT {projection} FROM {source}{condition} ORDER BY {order}{suffix}"
        ));
        Ok((statement, self.model_bind(&expressions)?))
    }

    pub(super) fn model_some(&mut self, ty: &Type, value: &str) -> String {
        let optional = self.module.ty(&Type::Nullable(Box::new(ty.clone())));
        let present = self.temp();
        self.line(format!(
            "{present} = insertvalue {optional} zeroinitializer, i1 1, 0"
        ));
        let wrapped = self.temp();
        self.line(format!(
            "{wrapped} = insertvalue {optional} {present}, {} {value}, 1",
            self.module.ty(ty)
        ));
        wrapped
    }

    pub(super) fn model_scalar_rows(&mut self, rows: &str, ty: &Type, span: Span) -> String {
        let length = self.protocol_call(
            "dever_rt_v1_db_rows_len",
            vec![format!("ptr {rows}")],
            "i64",
            span,
        );
        self.model_require_nonempty(&length, 8, "database scalar query returned no row", span);
        let row = self.protocol_call(
            "dever_rt_v1_db_rows_at",
            vec![format!("ptr {rows}"), "i64 0".into()],
            "ptr",
            span,
        );
        let owner = self.protocol_owner("db_row", &row);
        let actual = self.protocol_call(
            "dever_rt_v1_db_row_len",
            vec![format!("ptr {row}")],
            "i64",
            span,
        );
        self.database_column_present(&actual, 0, "scalar", span);
        let value = self.database_scalar(&row, 0, ty, span);
        self.database_row_length(
            &row,
            1,
            "database scalar query returned more than one column",
            span,
        );
        self.release_guard(owner);
        value
    }

    pub(super) fn model_require_nonempty(
        &mut self,
        length: &str,
        kind: u32,
        message: &str,
        span: Span,
    ) {
        let present = self.temp();
        self.line(format!("{present} = icmp sgt i64 {length}, 0"));
        let found = self.label("database_rows_present");
        let missing = self.label("database_rows_empty");
        self.line(format!("br i1 {present}, label %{found}, label %{missing}"));
        self.start(&missing);
        self.database_fault(kind, message, span);
        self.start(&found);
    }
}
