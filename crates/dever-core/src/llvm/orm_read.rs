//! Query result assembly, pagination, and association loading use concrete rows.
use super::*;
use crate::hir::QueryPlan;
use crate::model::{ModelFieldType, ModelRelationKind, ModelSchema};
use crate::native::orm as sql;

impl FunctionEmitter<'_, '_> {
    pub(super) fn model_first(
        &mut self,
        rows: &str,
        model: &ModelSchema,
        query: Option<&QueryPlan>,
        required: Option<(u32, String)>,
        span: Span,
    ) -> String {
        let index = self
            .module
            .program
            .models
            .iter()
            .position(|candidate| candidate.record == model.record)
            .unwrap();
        let list = self.model_rows_list(index, rows, model.record, query, None, span);
        let ty = Type::Nullable(Box::new(Type::Named(model.record)));
        let value = self.runtime_call(
            "dever_rt_v1_list_first_take",
            vec![format!("ptr {list}")],
            &ty,
            true,
            None,
            span,
        );
        let Some((kind, message)) = required else {
            return value;
        };
        let present = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {value}, 0",
            self.module.ty(&ty)
        ));
        let found = self.label("database_first_found");
        let missing = self.label("database_first_missing");
        self.line(format!("br i1 {present}, label %{found}, label %{missing}"));
        self.start(&missing);
        self.database_fault(kind, &message, span);
        self.start(&found);
        let record = self.temp();
        self.line(format!(
            "{record} = extractvalue {} {value}, 1",
            self.module.ty(&ty)
        ));
        record
    }

    pub(super) fn model_rows_list(
        &mut self,
        model: usize,
        rows: &str,
        record: usize,
        query: Option<&QueryPlan>,
        limit: Option<&str>,
        span: Span,
    ) -> String {
        let ty = Type::List(Box::new(Type::Named(record)));
        let list = self.runtime_call(
            "dever_rt_v1_list_new",
            vec![
                format!(
                    "ptr @dever_type_{}",
                    self.module.type_index(&Type::Named(record))
                ),
                "ptr null".into(),
                "i64 0".into(),
            ],
            &ty,
            false,
            None,
            span,
        );
        let owner = self.own_value(&ty, &list, false);
        let list_slot = self.guards[owner].pointer.clone();
        let length = self.protocol_call(
            "dever_rt_v1_db_rows_len",
            vec![format!("ptr {rows}")],
            "i64",
            span,
        );
        let index_slot = self.entry_slot(&Type::Int);
        self.line(format!("store i64 0, ptr {index_slot}"));
        let check = self.label("database_rows_check");
        let body = self.label("database_rows_decode");
        let done = self.label("database_rows_done");
        self.line(format!("br label %{check}"));
        self.start(&check);
        let index = self.temp();
        self.line(format!("{index} = load i64, ptr {index_slot}"));
        let more = self.temp();
        self.line(format!("{more} = icmp slt i64 {index}, {length}"));
        self.line(format!("br i1 {more}, label %{body}, label %{done}"));
        self.start(&body);
        let first = self.guards.len();
        let row = self.protocol_call(
            "dever_rt_v1_db_rows_at",
            vec![format!("ptr {rows}"), format!("i64 {index}")],
            "ptr",
            span,
        );
        self.protocol_owner("db_row", &row);
        let value = if let Some(query) = query {
            self.model_included_row(&row, model, query, span)
        } else {
            self.database_record(&row, record, 0, true, span)
        };
        self.own_value(&Type::Named(record), &value, false);
        let advance = self.label("database_rows_advance");
        if let Some(limit) = limit {
            let keep = self.temp();
            self.line(format!("{keep} = icmp slt i64 {index}, {limit}"));
            let append = self.label("database_rows_append");
            self.line(format!("br i1 {keep}, label %{append}, label %{advance}"));
            self.start(&append);
        }
        let pointer = self.row_pointer(&Type::Named(record), &value);
        let list = self.temp();
        self.line(format!("{list} = load ptr, ptr {list_slot}"));
        self.disarm(owner);
        let appended = self.runtime_call(
            "dever_rt_v1_list_append_take",
            vec![format!("ptr {list}"), format!("ptr {pointer}")],
            &ty,
            false,
            None,
            span,
        );
        self.line(format!("store ptr {appended}, ptr {list_slot}"));
        self.mark_live(owner);
        self.line(format!("br label %{advance}"));
        self.start(&advance);
        self.release_from(first);
        let next = self.temp();
        self.line(format!("{next} = add i64 {index}, 1"));
        self.line(format!("store i64 {next}, ptr {index_slot}"));
        self.line(format!("br label %{check}"));
        self.start(&done);
        if let Some(query) = query {
            self.model_load_many(model, query, &list_slot, span);
        }
        let value = self.temp();
        self.line(format!("{value} = load ptr, ptr {list_slot}"));
        self.disarm(owner);
        value
    }

    fn model_included_row(
        &mut self,
        row: &str,
        index: usize,
        query: &QueryPlan,
        span: Span,
    ) -> String {
        let model = self.module.program.models[index].clone();
        let mut value = self.database_record(row, model.record, 0, false, span);
        let owner = self.own_value(&Type::Named(model.record), &value, false);
        let record_slot = self.guards[owner].pointer.clone();
        let mut offset = model.fields.len();
        let Shape::Record(fields) = self.module.program.types[model.record].shape.clone() else {
            unreachable!()
        };
        for relation_index in &query.relations {
            let relation = &model.relations[*relation_index];
            if relation.kind != ModelRelationKind::ToOne {
                continue;
            }
            let target = self
                .module
                .program
                .models
                .iter()
                .find(|target| target.package == relation.model)
                .unwrap()
                .clone();
            let field = model.fields.len() + relation_index;
            let Type::Related(inner) = &fields[field].ty else {
                unreachable!()
            };
            let payload = self.entry_slot(inner);
            self.line(format!(
                "store {} zeroinitializer, ptr {payload}",
                self.module.ty(inner)
            ));
            let done = self.label("database_relation_decoded");
            if matches!(inner.as_ref(), Type::Nullable(_)) {
                let actual = self.protocol_call(
                    "dever_rt_v1_db_row_len",
                    vec![format!("ptr {row}")],
                    "i64",
                    span,
                );
                self.database_column_present(
                    &actual,
                    offset,
                    &format!("{}.{}", target.package, target.fields[0].name),
                    span,
                );
                let absent = self.database_call(
                    "db_row_null",
                    vec![format!("ptr {row}"), format!("i64 {offset}")],
                    "i8",
                    span,
                );
                let absent_bool = self.temp();
                self.line(format!("{absent_bool} = icmp ne i8 {absent}, 0"));
                let decode = self.label("database_relation_decode");
                let null = self.label("database_relation_null");
                self.line(format!(
                    "br i1 {absent_bool}, label %{null}, label %{decode}"
                ));
                self.start(&null);
                self.database_column_present(
                    &actual,
                    offset + target.fields.len() - 1,
                    &format!("{}.{}", target.package, relation.name),
                    span,
                );
                self.line(format!("br label %{done}"));
                self.start(&decode);
            }
            let decoded = self.database_record(row, target.record, offset, false, span);
            let decoded = if matches!(inner.as_ref(), Type::Nullable(_)) {
                self.model_some(&Type::Named(target.record), &decoded)
            } else {
                decoded
            };
            self.line(format!(
                "store {} {decoded}, ptr {payload}",
                self.module.ty(inner)
            ));
            self.line(format!("br label %{done}"));
            self.start(&done);
            let temporary = self.register_guard(inner, payload.clone());
            self.mark_live(temporary);
            let related = self.protocol_call(
                "dever_rt_v1_db_related_new",
                vec![
                    format!("ptr @dever_type_{}", self.module.type_index(inner)),
                    format!("ptr {payload}"),
                ],
                "ptr",
                span,
            );
            let destination = self.temp();
            self.line(format!(
                "{destination} = getelementptr %T{}, ptr {record_slot}, i32 0, i32 {field}",
                model.record
            ));
            self.line(format!("store ptr {related}, ptr {destination}"));
            self.release_guard(temporary);
            offset += target.fields.len();
        }
        self.database_row_length(
            row,
            offset,
            &format!("database row for {} has extra columns", model.package),
            span,
        );
        value = self.temp();
        self.line(format!(
            "{value} = load %T{}, ptr {record_slot}",
            model.record
        ));
        self.disarm(owner);
        value
    }

    pub(super) fn model_page(
        &mut self,
        index: usize,
        model: &ModelSchema,
        query: &QueryPlan,
        result: &Expression,
    ) -> Result<String, String> {
        let mut expressions = Vec::new();
        let condition = sql::condition_sql(model, query.condition.as_ref(), &mut expressions);
        let mut values = self.model_bind(&expressions)?;
        let page = self.expression(&query.page)?;
        let size = self.expression(&query.size)?;
        let maximum = self.model_maximum(index, result.span);
        let page_slot = self.entry_slot(&Type::Int);
        let size_slot = self.entry_slot(&Type::Int);
        let offset_slot = self.entry_slot(&Type::Int);
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let errors = self.database_errors(result.span);
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_db_pagination(i64 {page}, i64 {size}, i64 {maximum}, ptr {page_slot}, ptr {size_slot}, ptr {offset_slot}, ptr {errors}, ptr %fault, ptr {error})"));
        self.database_status(&status, &error, result.span);
        let offset = self.temp();
        self.line(format!("{offset} = load i64, ptr {offset_slot}"));
        let count_sql = self.module.planned_sql(&format!(
            "SELECT COUNT(*) FROM {}{condition}",
            sql::quoted(&model.table)
        ));
        let count_rows = self.model_query(index, &count_sql, &values, "0", result.span);
        let total = self.model_scalar_rows(&count_rows, &Type::Int, result.span);
        let alias = sql::has_to_one(model, query).then_some("_dever_parent");
        let mut ignored = Vec::new();
        let condition =
            sql::condition_sql_qualified(model, query.condition.as_ref(), &mut ignored, alias);
        let (projection, source) = sql::select_source(model, query, &self.module.program.models);
        let order = sql::order_sql_qualified(model, &query.order, alias);
        let statement = self.module.planned_sql(&format!(
            "SELECT {projection} FROM {source}{condition} ORDER BY {order} LIMIT ?{} OFFSET ?{}",
            values.len() + 1,
            values.len() + 2
        ));
        values.extend([
            (Type::Int, ModelFieldType::Int, size.clone()),
            (Type::Int, ModelFieldType::Int, offset),
        ]);
        let rows = self.model_query(index, &statement, &values, "0", result.span);
        let items =
            self.model_rows_list(index, &rows, model.record, Some(query), None, result.span);
        let quotient = self.temp();
        let remainder = self.temp();
        let extra = self.temp();
        let extra_integer = self.temp();
        let pages = self.temp();
        self.line(format!("{quotient} = sdiv i64 {total}, {size}"));
        self.line(format!("{remainder} = srem i64 {total}, {size}"));
        self.line(format!("{extra} = icmp ne i64 {remainder}, 0"));
        self.line(format!("{extra_integer} = zext i1 {extra} to i64"));
        self.line(format!("{pages} = add i64 {quotient}, {extra_integer}"));
        Ok(self.model_aggregate(
            model.page,
            &[
                ("ptr", items),
                ("i64", page),
                ("i64", size),
                ("i64", total),
                ("i64", pages),
            ],
        ))
    }

    pub(super) fn model_cursor(
        &mut self,
        index: usize,
        model: &ModelSchema,
        query: &QueryPlan,
        result: &Expression,
    ) -> Result<String, String> {
        let mut expressions = Vec::new();
        let alias = sql::has_to_one(model, query).then_some("_dever_parent");
        let mut condition =
            sql::condition_sql_qualified(model, query.condition.as_ref(), &mut expressions, alias);
        let mut values = self.model_bind(&expressions)?;
        if let Some(after) = &query.after {
            let cursor = sql::cursor_condition(model, &query.order, values.len() + 1, alias);
            condition.push_str(if condition.is_empty() {
                " WHERE "
            } else {
                " AND "
            });
            condition.push_str(&cursor);
            let after_value = self.expression(after)?;
            let present = self.temp();
            self.line(format!(
                "{present} = extractvalue {} {after_value}, 0",
                self.module.ty(&after.ty)
            ));
            let Shape::Record(fields) = self.module.program.types[model.record].shape.clone()
            else {
                unreachable!()
            };
            for order in &query.order {
                let field = &fields[order.field];
                let ty = Type::Nullable(Box::new(field.ty.clone()));
                let slot = self.entry_slot(&ty);
                self.line(format!(
                    "store {} zeroinitializer, ptr {slot}",
                    self.module.ty(&ty)
                ));
                let some = self.label("database_cursor_after");
                let done = self.label("database_cursor_bound");
                self.line(format!("br i1 {present}, label %{some}, label %{done}"));
                self.start(&some);
                let record = self.temp();
                self.line(format!(
                    "{record} = extractvalue {} {after_value}, 1",
                    self.module.ty(&after.ty)
                ));
                let value = self.temp();
                self.line(format!(
                    "{value} = extractvalue %T{} {record}, {}",
                    model.record, order.field
                ));
                let wrapped = self.model_some(&field.ty, &value);
                self.line(format!(
                    "store {} {wrapped}, ptr {slot}",
                    self.module.ty(&ty)
                ));
                self.line(format!("br label %{done}"));
                self.start(&done);
                let value = self.temp();
                self.line(format!(
                    "{value} = load {}, ptr {slot}",
                    self.module.ty(&ty)
                ));
                values.push((ty, model.fields[order.field].ty.clone(), value));
            }
        }
        let size = self.expression(&query.size)?;
        let maximum = self.model_maximum(index, result.span);
        let size = self.database_call(
            "db_cursor_size",
            vec![format!("i64 {size}"), format!("i64 {maximum}")],
            "i64",
            result.span,
        );
        let limit = self.temp();
        self.line(format!("{limit} = add i64 {size}, 1"));
        let (projection, source) = sql::select_source(model, query, &self.module.program.models);
        let order = sql::order_sql_qualified(model, &query.order, alias);
        let statement = self.module.planned_sql(&format!(
            "SELECT {projection} FROM {source}{condition} ORDER BY {order} LIMIT ?{}",
            values.len() + 1
        ));
        values.push((Type::Int, ModelFieldType::Int, limit));
        let rows = self.model_query(index, &statement, &values, "0", result.span);
        let length = self.protocol_call(
            "dever_rt_v1_db_rows_len",
            vec![format!("ptr {rows}")],
            "i64",
            result.span,
        );
        let more = self.temp();
        self.line(format!("{more} = icmp sgt i64 {length}, {size}"));
        let items = self.model_rows_list(
            index,
            &rows,
            model.record,
            Some(query),
            Some(&size),
            result.span,
        );
        let owner = self.own_value(
            &Type::List(Box::new(Type::Named(model.record))),
            &items,
            false,
        );
        let next_ty = Type::Nullable(Box::new(Type::Named(model.record)));
        let next = self.entry_slot(&next_ty);
        self.line(format!(
            "store {} zeroinitializer, ptr {next}",
            self.module.ty(&next_ty)
        ));
        let some = self.label("database_cursor_next");
        let done = self.label("database_cursor_ready");
        self.line(format!("br i1 {more}, label %{some}, label %{done}"));
        self.start(&some);
        self.model_last(&items, model.record, &next, result.span);
        self.line(format!("br label %{done}"));
        self.start(&done);
        let next_value = self.temp();
        self.line(format!(
            "{next_value} = load {}, ptr {next}",
            self.module.ty(&next_ty)
        ));
        self.disarm(owner);
        let layout = self.module.ty(&next_ty);
        Ok(self.model_aggregate(
            model.cursor,
            &[("ptr", items), (&layout, next_value), ("i1", more)],
        ))
    }

    fn model_last(&mut self, list: &str, record: usize, output: &str, span: Span) {
        let retained = self.temp();
        self.line(format!(
            "{retained} = call ptr @dever_rt_v1_list_retain(ptr {list})"
        ));
        let cursor = self.protocol_call(
            "dever_rt_v1_list_cursor_take",
            vec![format!("ptr {retained}")],
            "ptr",
            span,
        );
        let slot = self.entry_slot_ir("ptr");
        self.line(format!("store ptr {cursor}, ptr {slot}"));
        let owner = self.register_owned(OwnedKind::ListCursor, slot);
        self.mark_live(owner);
        let ty = Type::Nullable(Box::new(Type::Named(record)));
        let last_owner = self.register_guard(&ty, output.into());
        self.mark_live(last_owner);
        let check = self.label("database_cursor_last");
        let found = self.label("database_cursor_last_found");
        let done = self.label("database_cursor_last_done");
        self.line(format!("br label %{check}"));
        self.start(&check);
        let row = self.runtime_call(
            "dever_rt_v1_list_cursor_next",
            vec![format!("ptr {cursor}")],
            &ty,
            true,
            None,
            span,
        );
        let present = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {row}, 0",
            self.module.ty(&ty)
        ));
        self.line(format!("br i1 {present}, label %{found}, label %{done}"));
        self.start(&found);
        self.release_guard(last_owner);
        self.line(format!("store {} {row}, ptr {output}", self.module.ty(&ty)));
        self.mark_live(last_owner);
        self.line(format!("br label %{check}"));
        self.start(&done);
        self.release_guard(owner);
        self.disarm(last_owner);
    }

    fn model_aggregate(&mut self, id: usize, fields: &[(&str, String)]) -> String {
        let mut value = "zeroinitializer".to_string();
        for (index, (ty, field)) in fields.iter().enumerate() {
            let next = self.temp();
            self.line(format!(
                "{next} = insertvalue %T{id} {value}, {ty} {field}, {index}"
            ));
            value = next;
        }
        value
    }
}
