use std::fmt::Write;

use crate::hir::{ApiRest, Program};
use crate::model::{
    ModelFieldType, ModelSchema, RestContract, RestOwnerSource, RestValueSource, RestWriteField,
};
use crate::types::{Shape, Type};
use crate::wire::{Policy, Schema};

use super::orm::{database_sql, parameter_sql, quoted, select_columns};
use super::rust_string;

pub(super) fn emit_helpers(program: &Program, output: &mut String) {
    let mut emitted = std::collections::BTreeSet::new();
    for route in &program.api_rest {
        let model = &program.models[route.model];
        if !emitted.insert(model.record) {
            continue;
        }
        let contract = &route.contract;
        let Shape::Record(fields) = &program.types[model.record].shape else {
            unreachable!()
        };
        for &index in &contract.read {
            let schema = Schema::build(&fields[index].ty, &program.types, Policy::Output)
                .expect("checked REST output field");
            output.push_str(&super::wire::emit_named(
                &schema,
                &format!("rest_{}_f{index}_out", model.record),
            ));
        }
        let inputs = contract
            .create
            .iter()
            .chain(&contract.replace)
            .filter_map(|write| match write.source {
                RestValueSource::Input
                | RestValueSource::Binding {
                    uses_input: true, ..
                } => Some(write.field),
                RestValueSource::Binding {
                    uses_input: false, ..
                } => None,
            })
            .chain(contract.search.iter().map(|search| search.field))
            .collect::<std::collections::BTreeSet<_>>();
        for index in inputs {
            let schema = Schema::build(&fields[index].ty, &program.types, Policy::ApiInput)
                .expect("checked REST input field");
            output.push_str(&super::wire::emit_named(
                &schema,
                &format!("rest_{}_f{index}_in", model.record),
            ));
        }
        writeln!(
            output,
            "fn rest_{}_encode(value: &T{}) -> Result<dever_runtime::wire::Encoded, String> {{",
            model.record, model.record
        )
        .expect("format");
        output.push_str(
            "let mut writer = dever_runtime::wire::Encoder::default(); writer.begin_object()?;\n",
        );
        for &index in &contract.read {
            writeln!(output, "writer.key({})?; writer.json(rest_{}_f{index}_out_encode(&value.f{index})?.as_str())?;", rust_string(&model.fields[index].name), model.record).expect("format");
        }
        output.push_str("writer.end()?; writer.finish()\n}\n");
    }
}

/// Emit a REST match arm body. The caller owns routing and supplies the checked method/detail pair.
pub(super) fn emit_handler(
    emitter: &super::Emitter<'_>,
    route: &ApiRest,
    method: &str,
    detail: bool,
    output: &mut String,
) {
    let program = emitter.program;
    let model = &program.models[route.model];
    let contract = &route.contract;
    let table = quoted(&model.table);
    let id_condition = format!("{} = ?1", quoted("id"));
    if detail {
        writeln!(output, "let rest_id = match path.strip_prefix({}).and_then(|id| id.parse::<i64>().ok()).filter(|id| *id > 0) {{ Some(id) => id, None => return Ok(dever_runtime::api::not_found()) }};", rust_string(&format!("{}/", route.path))).expect("format");
    }
    match (method, detail) {
        ("GET", false) => {
            output.push_str("let mut inputs = match dever_runtime::api::Inputs::from_request(&request) { Ok(inputs) => inputs, Err(error) => return Ok(dever_runtime::api::invalid_input(error)) };\n");
            output.push_str("let page = match inputs.optional_int(\"page\") { Ok(value) => value.unwrap_or(1), Err(error) => return Ok(dever_runtime::api::invalid_input(error)) };\n");
            output.push_str("let size = match inputs.optional_int(\"size\") { Ok(value) => value.unwrap_or(20), Err(error) => return Ok(dever_runtime::api::invalid_input(error)) };\n");
            parse_search(emitter, model, contract, output);
            finish_inputs(output);
            owner_value(output, contract);
            database(output, model);
            output.push_str("let (page, size, offset) = match dever_runtime::orm::pagination(page, size, database.max_page_size()) { Ok(value) => value, Err(error) => return Ok(dever_runtime::api::invalid_input(dever_runtime::api::InputError(error.to_string()))) };\n");
            let (owner_filter, count_parameters, page_parameters, limit_slot) = match contract.owner
            {
                Some((index, _)) => (
                    format!(" WHERE {} = ?1", quoted(&model.fields[index].name)),
                    "vec![dever_runtime::orm::Value::Int(rest_owner_id)]",
                    "vec![dever_runtime::orm::Value::Int(rest_owner_id), dever_runtime::orm::Value::Int(size), dever_runtime::orm::Value::Int(offset)]",
                    2,
                ),
                None => (
                    String::new(),
                    "vec![]",
                    "vec![dever_runtime::orm::Value::Int(size), dever_runtime::orm::Value::Int(offset)]",
                    1,
                ),
            };
            let count = format!("SELECT COUNT(*) FROM {table}{owner_filter}");
            let list = format!(
                "SELECT {} FROM {table}{owner_filter} ORDER BY {} DESC LIMIT ?{limit_slot} OFFSET ?{}",
                select_columns(model),
                quoted("id"),
                limit_slot + 1
            );
            if let Some(search) = &contract.search {
                let column = quoted(&model.fields[search.field].name);
                let search_slot = usize::from(contract.owner.is_some()) + 1;
                let search_parameter = parameter_sql(search_slot, &model.fields[search.field].ty);
                let search_filter = if contract.owner.is_some() {
                    format!("{owner_filter} AND {column} = {search_parameter}")
                } else {
                    format!(" WHERE {column} = {search_parameter}")
                };
                let search_count = format!("SELECT COUNT(*) FROM {table}{search_filter}");
                let search_limit = search_slot + 1;
                let search_list = format!(
                    "SELECT {} FROM {table}{search_filter} ORDER BY {} DESC LIMIT ?{search_limit} OFFSET ?{}",
                    select_columns(model),
                    quoted("id"),
                    search_limit + 1
                );
                let search_count_parameters = if contract.owner.is_some() {
                    "vec![dever_runtime::orm::Value::Int(rest_owner_id), rest_search_parameter.clone()]"
                } else {
                    "vec![rest_search_parameter.clone()]"
                };
                let search_page_parameters = if contract.owner.is_some() {
                    "vec![dever_runtime::orm::Value::Int(rest_owner_id), rest_search_parameter, dever_runtime::orm::Value::Int(size), dever_runtime::orm::Value::Int(offset)]"
                } else {
                    "vec![rest_search_parameter, dever_runtime::orm::Value::Int(size), dever_runtime::orm::Value::Int(offset)]"
                };
                writeln!(output, "let (count_sql, list_sql, count_parameters, page_parameters) = if let Some(rest_search_parameter) = rest_search_parameter {{ ({}, {}, {search_count_parameters}, {search_page_parameters}) }} else {{ ({}, {}, {count_parameters}, {page_parameters}) }};", database_sql(&search_count), database_sql(&search_list), database_sql(&count), database_sql(&list)).expect("format");
                output.push_str("let count_rows = match database.query(count_sql, count_parameters).await { Ok(rows) => rows, Err(error) => return Ok(rest_database_error(error)) };\n");
            } else {
                writeln!(output, "let count_rows = match database.query({}, {count_parameters}).await {{ Ok(rows) => rows, Err(error) => return Ok(rest_database_error(error)) }};", database_sql(&count)).expect("format");
            }
            output.push_str("let total = match count_rows.into_iter().next().ok_or_else(|| dever_runtime::orm::Error::invalid_data(\"REST count returned no row\")).and_then(dever_runtime::orm::scalar).and_then(dever_runtime::orm::int) { Ok(value) => value, Err(error) => return Ok(rest_database_error(error)) };\n");
            if contract.search.is_some() {
                output.push_str("let rows = match database.query(list_sql, page_parameters).await { Ok(rows) => rows, Err(error) => return Ok(rest_database_error(error)) };\n");
            } else {
                writeln!(output, "let rows = match database.query({}, {page_parameters}).await {{ Ok(rows) => rows, Err(error) => return Ok(rest_database_error(error)) }};", database_sql(&list)).expect("format");
            }
            writeln!(output, "let mut writer = dever_runtime::wire::Encoder::default(); let value: Result<_, String> = (|| {{ writer.begin_object()?; writer.key(\"items\")?; writer.begin_array()?; for row in rows {{ let model = decode_model_{}(row).map_err(|error| error.to_string())?; writer.json(rest_{}_encode(&model)?.as_str())?; }} writer.end()?; writer.key(\"page\")?; writer.int(page)?; writer.key(\"size\")?; writer.int(size)?; writer.key(\"total\")?; writer.int(total)?; writer.end()?; writer.finish() }})();", model.record, model.record).expect("format");
            success_value(output);
        }
        ("GET", true) => {
            output.push_str("let inputs = match dever_runtime::api::Inputs::from_request(&request) { Ok(inputs) => inputs, Err(error) => return Ok(dever_runtime::api::invalid_input(error)) };\n");
            finish_inputs(output);
            owner_value(output, contract);
            database(output, model);
            let (owner_filter, parameters) = owner_filter(model, contract, 2);
            let select = format!(
                "SELECT {} FROM {table} WHERE {id_condition}{owner_filter} LIMIT 1",
                select_columns(model)
            );
            writeln!(output, "let rows = match database.query({}, vec![dever_runtime::orm::Value::Int(rest_id){parameters}]).await {{ Ok(rows) => rows, Err(error) => return Ok(rest_database_error(error)) }};", database_sql(&select)).expect("format");
            output.push_str("let Some(row) = rows.into_iter().next() else { return Ok(dever_runtime::api::not_found()) };\n");
            encode_row(output, model, false);
        }
        ("POST", false) => {
            parse_fields(emitter, model, &contract.create, output);
            owner_value(output, contract);
            database(output, model);
            let generated = model
                .fields
                .iter()
                .enumerate()
                .filter(|(_, field)| field.generated && matches!(field.ty, ModelFieldType::Uuid))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            let mut write_fields = contract
                .create
                .iter()
                .map(|write| write.field)
                .collect::<Vec<_>>();
            if let Some((owner, _)) = contract.owner {
                write_fields.push(owner);
            }
            write_fields.extend(generated.iter().copied());
            let columns = write_fields
                .iter()
                .map(|&index| quoted(&model.fields[index].name))
                .collect::<Vec<_>>();
            let sql = if columns.is_empty() {
                format!(
                    "INSERT INTO {table} DEFAULT VALUES RETURNING {}",
                    select_columns(model)
                )
            } else {
                let values = write_fields
                    .iter()
                    .enumerate()
                    .map(|(slot, &index)| parameter_sql(slot + 1, &model.fields[index].ty))
                    .collect::<Vec<_>>();
                format!(
                    "INSERT INTO {table} ({}) VALUES ({}) RETURNING {}",
                    columns.join(", "),
                    values.join(", "),
                    select_columns(model)
                )
            };
            let mut parameters = values(program, model, &contract.create, output);
            if contract.owner.is_some() {
                parameters.push_str("dever_runtime::orm::Value::Int(rest_owner_id), ");
            }
            for index in generated {
                writeln!(output, "let generated_{index} = match dever_runtime::orm::Uuid::new_v7() {{ Ok(value) => value, Err(error) => return Ok(rest_database_error(error)) }};").expect("format");
                write!(
                    parameters,
                    "dever_runtime::orm::Value::Uuid(generated_{index}), "
                )
                .expect("format");
            }
            begin_transaction(output);
            writeln!(output, "let rows = match executor.query({}, vec![{parameters}]).await {{ Ok(rows) => rows, Err(error) => {{ drop(executor); rest_rollback(transaction).await; return Ok(rest_database_error(error)); }} }};", database_sql(&sql)).expect("format");
            output.push_str("let Some(row) = rows.into_iter().next() else { drop(executor); rest_rollback(transaction).await; return Ok(dever_runtime::api::internal_error()) };\n");
            encode_row(output, model, true);
        }
        ("PUT", true) => {
            parse_fields(emitter, model, &contract.replace, output);
            owner_value(output, contract);
            database(output, model);
            let assignments = super::orm::update_assignments(
                model,
                contract.replace.iter().map(|write| write.field),
                1,
            );
            let id_slot = contract.replace.len() + 1;
            let (owner_filter, owner_parameters) = owner_filter(model, contract, id_slot + 1);
            let sql = format!(
                "UPDATE {table} SET {assignments} WHERE {} = ?{id_slot}{owner_filter} RETURNING {}",
                quoted("id"),
                select_columns(model)
            );
            let parameters = values(program, model, &contract.replace, output);
            begin_transaction(output);
            writeln!(output, "let rows = match executor.query({}, vec![{parameters}dever_runtime::orm::Value::Int(rest_id){owner_parameters}]).await {{ Ok(rows) => rows, Err(error) => {{ drop(executor); rest_rollback(transaction).await; return Ok(rest_database_error(error)); }} }};", database_sql(&sql)).expect("format");
            output.push_str("let Some(row) = rows.into_iter().next() else { drop(executor); rest_rollback(transaction).await; return Ok(dever_runtime::api::not_found()) };\n");
            encode_row(output, model, true);
        }
        ("DELETE", true) => {
            output.push_str("let inputs = match dever_runtime::api::Inputs::from_request(&request) { Ok(inputs) => inputs, Err(error) => return Ok(dever_runtime::api::invalid_input(error)) };\n");
            finish_inputs(output);
            owner_value(output, contract);
            database(output, model);
            let (owner_filter, owner_parameters) = owner_filter(model, contract, 2);
            let sql = format!("DELETE FROM {table} WHERE {id_condition}{owner_filter}");
            begin_transaction(output);
            writeln!(output, "let affected = match executor.execute({}, vec![dever_runtime::orm::Value::Int(rest_id){owner_parameters}]).await {{ Ok(affected) => affected, Err(error) => {{ drop(executor); rest_rollback(transaction).await; return Ok(rest_database_error(error)); }} }};", database_sql(&sql)).expect("format");
            output.push_str("drop(executor); if affected == 0 { rest_rollback(transaction).await; return Ok(dever_runtime::api::not_found()); } if let Err(error) = transaction.commit().await { return Ok(rest_database_error(error)); } Ok(dever_runtime::api::success(dever_runtime::wire::Encoded::null()))\n");
        }
        _ => unreachable!("checked REST route"),
    }
}

pub(super) fn emit_database_error(output: &mut String) {
    output.push_str("fn rest_database_error(error: dever_runtime::orm::Error) -> dever_runtime::http::Response { dever_runtime::log::error(\"REST database operation failed\", Vec::new()); if error.kind() == dever_runtime::orm::ErrorKind::NotFound { dever_runtime::api::not_found() } else { dever_runtime::api::internal_error() } }\n");
    output.push_str("async fn rest_rollback(transaction: dever_runtime::database::Transaction) { if transaction.rollback().await.is_err() { dever_runtime::log::error(\"REST rollback failed\", Vec::new()); } }\n");
}

fn parse_fields(
    emitter: &super::Emitter<'_>,
    model: &ModelSchema,
    writes: &[RestWriteField],
    output: &mut String,
) {
    let program = emitter.program;
    output.push_str("let mut inputs = match dever_runtime::api::Inputs::from_request(&request) { Ok(inputs) => inputs, Err(error) => return Ok(dever_runtime::api::invalid_input(error)) };\n");
    let Shape::Record(fields) = &program.types[model.record].shape else {
        unreachable!()
    };
    for write in writes {
        let index = write.field;
        let reads_input = matches!(
            write.source,
            RestValueSource::Input
                | RestValueSource::Binding {
                    uses_input: true,
                    ..
                }
        );
        if !reads_input {
            continue;
        }
        let name = rust_string(&model.fields[index].name);
        let prefix = format!("rest_{}_f{index}_in", model.record);
        let nullable = matches!(fields[index].ty, Type::Nullable(_));
        let read = if nullable {
            "optional_raw_json"
        } else {
            "raw_json"
        };
        let absent = if nullable {
            ".unwrap_or_else(|| \"null\".into())"
        } else {
            ""
        };
        writeln!(output, "let raw_{index} = match inputs.{read}({name}) {{ Ok(value) => value{absent}, Err(error) => return Ok(dever_runtime::api::invalid_input(error)) }};").expect("format");
        writeln!(output, "let value_{index} = match {prefix}_decode(&raw_{index}) {{ Ok(value) => value, Err(error) => return Ok(dever_runtime::api::invalid_input(dever_runtime::api::InputError(error))) }};").expect("format");
        if let ModelFieldType::Text { minimum, maximum } = model.fields[index].ty {
            let value = if nullable {
                format!("value_{index}.as_deref()")
            } else {
                format!("Some(value_{index}.as_str())")
            };
            let maximum =
                maximum.map_or_else(|| "None".to_owned(), |value| format!("Some({value})"));
            writeln!(output, "if let Err(error) = dever_runtime::api::text_bounds({name}, {value}, {minimum}, {maximum}) {{ return Ok(dever_runtime::api::invalid_input(error)); }}").expect("format");
        }
    }
    finish_inputs(output);
    for write in writes {
        let RestValueSource::Binding {
            expression,
            uses_input,
        } = &write.source
        else {
            continue;
        };
        let index = write.field;
        if *uses_input {
            writeln!(output, "let _v0 = value_{index};").expect("format");
        }
        writeln!(output, "let value_{index} = match (|| -> Result<_, AppError> {{ Ok({}) }})() {{ Ok(value) => value, Err(error) => return Ok(api_failure_response(error)) }};", emitter.expression(expression)).expect("format");
        if let ModelFieldType::Text { minimum, maximum } = model.fields[index].ty {
            let nullable = matches!(fields[index].ty, Type::Nullable(_));
            let value = if nullable {
                format!("value_{index}.as_deref()")
            } else {
                format!("Some(value_{index}.as_str())")
            };
            let maximum =
                maximum.map_or_else(|| "None".to_owned(), |value| format!("Some({value})"));
            let failure = if *uses_input {
                "return Ok(dever_runtime::api::invalid_input(error))"
            } else {
                "dever_runtime::log::error(\"REST server field binding violated Model bounds\", Vec::new()); return Ok(dever_runtime::api::internal_error())"
            };
            writeln!(output, "if let Err(error) = dever_runtime::api::text_bounds({}, {value}, {minimum}, {maximum}) {{ {failure}; }}", rust_string(&model.fields[index].name)).expect("format");
        }
    }
}

fn parse_search(
    emitter: &super::Emitter<'_>,
    model: &ModelSchema,
    contract: &RestContract,
    output: &mut String,
) {
    let Some(search) = &contract.search else {
        return;
    };
    let program = emitter.program;
    let Shape::Record(fields) = &program.types[model.record].shape else {
        unreachable!()
    };
    let index = search.field;
    let name = rust_string(&model.fields[index].name);
    let prefix = format!("rest_{}_f{index}_in", model.record);
    writeln!(output, "let raw_search = match inputs.optional_raw_json({name}) {{ Ok(value) => value, Err(error) => return Ok(dever_runtime::api::invalid_input(error)) }};").expect("format");
    writeln!(output, "let rest_search_value = match raw_search {{ Some(raw) => match {prefix}_decode(&raw) {{ Ok(value) => Some(value), Err(error) => return Ok(dever_runtime::api::invalid_input(dever_runtime::api::InputError(error))) }}, None => None }};").expect("format");
    if let ModelFieldType::Text { minimum, maximum } = model.fields[index].ty {
        let maximum = maximum.map_or_else(|| "None".to_owned(), |value| format!("Some({value})"));
        writeln!(output, "if let Err(error) = dever_runtime::api::text_bounds({name}, rest_search_value.as_deref(), {minimum}, {maximum}) {{ return Ok(dever_runtime::api::invalid_input(error)); }}").expect("format");
    }
    writeln!(output, "let rest_search_value = match rest_search_value {{ Some(value) => {{ let _v0 = value; match (|| -> Result<_, AppError> {{ Ok({}) }})() {{ Ok(value) => Some(value), Err(error) => return Ok(api_failure_response(error)) }} }}, None => None }};", emitter.expression(&search.expression)).expect("format");
    if let ModelFieldType::Text { minimum, maximum } = model.fields[index].ty {
        let maximum = maximum.map_or_else(|| "None".to_owned(), |value| format!("Some({value})"));
        writeln!(output, "if let Err(error) = dever_runtime::api::text_bounds({name}, rest_search_value.as_deref(), {minimum}, {maximum}) {{ return Ok(dever_runtime::api::invalid_input(error)); }}").expect("format");
    }
    let value = value_expression(&fields[index].ty, &model.fields[index].ty, "value");
    writeln!(output, "let rest_search_parameter = match rest_search_value {{ Some(value) => match {value} {{ Ok(value) => Some(value), Err(error) => return Ok(dever_runtime::api::invalid_input(dever_runtime::api::InputError(error.to_string()))) }}, None => None }};").expect("format");
}

fn values(
    program: &Program,
    model: &ModelSchema,
    writes: &[RestWriteField],
    output: &mut String,
) -> String {
    let Shape::Record(fields) = &program.types[model.record].shape else {
        unreachable!()
    };
    writes.iter().map(|write| {
        let index = write.field;
        let value = value_expression(&fields[index].ty, &model.fields[index].ty, &format!("value_{index}"));
        writeln!(output, "let parameter_{index} = match {value} {{ Ok(value) => value, Err(error) => return Ok(dever_runtime::api::invalid_input(dever_runtime::api::InputError(error.to_string()))) }};").expect("format");
        format!("parameter_{index}, ")
    }).collect()
}

fn value_expression(ty: &Type, storage: &ModelFieldType, name: &str) -> String {
    let value = "dever_runtime::orm::Value";
    match ty {
        Type::Nullable(inner) => format!(
            "match {name} {{ Some(value) => {}, None => Ok::<_, dever_runtime::orm::Error>({value}::Null) }}",
            value_expression(inner, storage, "value")
        ),
        Type::Bool => format!("Ok::<_, dever_runtime::orm::Error>({value}::Bool({name}))"),
        Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration => {
            format!("Ok::<_, dever_runtime::orm::Error>({value}::Int({name}))")
        }
        Type::Float => format!("Ok::<_, dever_runtime::orm::Error>({value}::Float({name}))"),
        Type::Text | Type::Json => {
            format!("Ok::<_, dever_runtime::orm::Error>({value}::Text({name}))")
        }
        Type::Uuid => format!("Ok::<_, dever_runtime::orm::Error>({value}::Uuid({name}))"),
        Type::Named(_) => format!("Ok::<_, dever_runtime::orm::Error>({value}::Int({name}.0))"),
        Type::Decimal => {
            let ModelFieldType::Decimal { precision, scale } = storage else {
                unreachable!()
            };
            format!("dever_runtime::orm::decimal_value({name}, {precision}, {scale})")
        }
        _ => unreachable!("checked REST wire field"),
    }
}

fn database(output: &mut String, model: &ModelSchema) {
    writeln!(output, "let database = match model_database_{}().await {{ Ok(database) => database, Err(error) => return Ok(rest_database_error(error)) }};", model.record).expect("format");
}

fn begin_transaction(output: &mut String) {
    output.push_str("let transaction = match database.begin().await { Ok(transaction) => transaction, Err(error) => return Ok(rest_database_error(error)) };\n");
    output.push_str("let executor = match dever_runtime::database::Executor::new(database, Some(&transaction)) { Ok(executor) => executor, Err(error) => return Ok(rest_database_error(error)) };\n");
}

fn finish_inputs(output: &mut String) {
    output.push_str("if let Err(error) = inputs.finish() { return Ok(dever_runtime::api::invalid_input(error)); }\n");
}

fn owner_value(output: &mut String, contract: &RestContract) {
    let Some((_, source)) = contract.owner else {
        return;
    };
    let read = match source {
        RestOwnerSource::User => "dever_runtime::auth::user_id()",
        RestOwnerSource::Tenant => "dever_runtime::auth::tenant_id()",
    };
    writeln!(
        output,
        "let rest_owner_id = match {read} {{ Ok(Some(value)) => value, Ok(None) => return Ok(dever_runtime::api::standard_error(403)), Err(error) => return Ok(dever_runtime::api::auth_error(error)) }};"
    )
    .expect("format");
}

fn owner_filter(
    model: &ModelSchema,
    contract: &RestContract,
    parameter: usize,
) -> (String, &'static str) {
    match contract.owner {
        Some((index, _)) => (
            format!(" AND {} = ?{parameter}", quoted(&model.fields[index].name)),
            ", dever_runtime::orm::Value::Int(rest_owner_id)",
        ),
        None => (String::new(), ""),
    }
}

fn encode_row(output: &mut String, model: &ModelSchema, transactional: bool) {
    let rollback = if transactional {
        "drop(executor); rest_rollback(transaction).await; "
    } else {
        ""
    };
    writeln!(output, "let model = match decode_model_{}(row) {{ Ok(model) => model, Err(error) => {{ {rollback}return Ok(rest_database_error(error)); }} }};", model.record).expect("format");
    writeln!(output, "let value = match rest_{}_encode(&model) {{ Ok(value) => value, Err(_error) => {{ {rollback}return Ok(dever_runtime::api::internal_error()); }} }};", model.record).expect("format");
    if transactional {
        output.push_str("drop(executor); if let Err(error) = transaction.commit().await { return Ok(rest_database_error(error)); }\n");
    }
    output.push_str("Ok(dever_runtime::api::success(value))\n");
}

fn success_value(output: &mut String) {
    output.push_str("match value { Ok(value) => Ok(dever_runtime::api::success(value)), Err(_error) => Ok(dever_runtime::api::internal_error()) }\n");
}
