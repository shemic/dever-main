//! To-many loading batches 998 parent IDs and groups concrete child rows.
use super::*;
use crate::hir::QueryPlan;
use crate::model::{ModelRelationKind, ModelSchema};
use crate::native::orm as sql;

struct RelationBatch<'a> {
    ids: &'a str,
    count: &'a str,
    limit: &'a str,
}

impl FunctionEmitter<'_, '_> {
    fn database_empty_list(&mut self, record: usize, span: Span) -> String {
        self.runtime_call(
            "dever_rt_v1_list_new",
            vec![
                format!(
                    "ptr @dever_type_{}",
                    self.module.type_index(&Type::Named(record))
                ),
                "ptr null".into(),
                "i64 0".into(),
            ],
            &Type::List(Box::new(Type::Named(record))),
            false,
            None,
            span,
        )
    }

    fn database_list_cursor(&mut self, list: &str, span: Span) -> (String, usize) {
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
        (cursor, owner)
    }

    fn database_for_each(
        &mut self,
        list: &str,
        record: usize,
        span: Span,
        mut apply: impl FnMut(&mut Self, &str),
    ) {
        let (cursor, owner) = self.database_list_cursor(list, span);
        let check = self.label("relation_rows_next");
        let body = self.label("relation_rows_body");
        let done = self.label("relation_rows_done");
        self.line(format!("br label %{check}"));
        self.start(&check);
        let ty = Type::Nullable(Box::new(Type::Named(record)));
        let value = self.runtime_call(
            "dever_rt_v1_list_cursor_next",
            vec![format!("ptr {cursor}")],
            &ty,
            true,
            None,
            span,
        );
        let present = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {value}, 0",
            self.module.ty(&ty)
        ));
        self.line(format!("br i1 {present}, label %{body}, label %{done}"));
        self.start(&body);
        let first = self.guards.len();
        let row = self.temp();
        self.line(format!(
            "{row} = extractvalue {} {value}, 1",
            self.module.ty(&ty)
        ));
        self.own_value(&Type::Named(record), &row, false);
        apply(self, &row);
        self.release_from(first);
        self.line(format!("br label %{check}"));
        self.start(&done);
        self.release_guard(owner);
    }

    fn database_append(&mut self, list_slot: &str, record: usize, row: &str, span: Span) {
        let list = self.temp();
        self.line(format!("{list} = load ptr, ptr {list_slot}"));
        let owner = self
            .guards
            .iter()
            .position(|guard| guard.pointer == list_slot)
            .expect("owned relation list slot");
        let pointer = self.row_pointer(&Type::Named(record), row);
        self.disarm(owner);
        let next = self.runtime_call(
            "dever_rt_v1_list_append_take",
            vec![format!("ptr {list}"), format!("ptr {pointer}")],
            &Type::List(Box::new(Type::Named(record))),
            false,
            None,
            span,
        );
        self.line(format!("store ptr {next}, ptr {list_slot}"));
        self.mark_live(owner);
    }

    pub(super) fn model_load_many(
        &mut self,
        index: usize,
        query: &QueryPlan,
        list_slot: &str,
        span: Span,
    ) {
        let model = self.module.program.models[index].clone();
        for relation_index in &query.relations {
            let relation = &model.relations[*relation_index];
            if relation.kind != ModelRelationKind::ToMany {
                continue;
            }
            let target_index = self
                .module
                .program
                .models
                .iter()
                .position(|candidate| candidate.package == relation.model)
                .unwrap();
            let target = self.module.program.models[target_index].clone();
            let foreign_key = target
                .fields
                .iter()
                .position(|field| field.name == relation.field)
                .unwrap();
            let field = model.fields.len() + relation_index;
            let parents = self.temp();
            self.line(format!("{parents} = load ptr, ptr {list_slot}"));
            let (cursor, cursor_owner) = self.database_list_cursor(&parents, span);
            let output = self.database_empty_list(model.record, span);
            let output_ty = Type::List(Box::new(Type::Named(model.record)));
            let output_owner = self.own_value(&output_ty, &output, false);
            let output_slot = self.guards[output_owner].pointer.clone();
            let maximum = self.model_maximum(index, span);
            let limit = self.database_call(
                "db_relation_limit",
                vec![format!("i64 {maximum}")],
                "i64",
                span,
            );
            let ids = self.entry_slot_ir("[998 x i64]");
            let count_slot = self.entry_slot(&Type::Int);
            let done_slot = self.entry_slot_ir("i1");
            self.line(format!("store i1 0, ptr {done_slot}"));
            let batch_start = self.label("relation_batch_start");
            let read = self.label("relation_batch_read");
            let add = self.label("relation_batch_add");
            let exhausted = self.label("relation_batch_exhausted");
            let query_block = self.label("relation_batch_query");
            let queried = self.label("relation_batch_queried");
            let finish = self.label("relation_batches_done");
            self.line(format!("br label %{batch_start}"));
            self.start(&batch_start);
            let batch_first = self.guards.len();
            let batch = self.database_empty_list(model.record, span);
            let batch_owner = self.own_value(&output_ty, &batch, false);
            let batch_slot = self.guards[batch_owner].pointer.clone();
            self.line(format!("store i64 0, ptr {count_slot}"));
            self.line(format!("br label %{read}"));
            self.start(&read);
            let next_ty = Type::Nullable(Box::new(Type::Named(model.record)));
            let parent = self.runtime_call(
                "dever_rt_v1_list_cursor_next",
                vec![format!("ptr {cursor}")],
                &next_ty,
                true,
                None,
                span,
            );
            let present = self.temp();
            self.line(format!(
                "{present} = extractvalue {} {parent}, 0",
                self.module.ty(&next_ty)
            ));
            self.line(format!("br i1 {present}, label %{add}, label %{exhausted}"));
            self.start(&add);
            let parent_row = self.temp();
            self.line(format!(
                "{parent_row} = extractvalue {} {parent}, 1",
                self.module.ty(&next_ty)
            ));
            let parent_owner = self.own_value(&Type::Named(model.record), &parent_row, false);
            let id = self.temp();
            self.line(format!(
                "{id} = extractvalue %T{} {parent_row}, 0",
                model.record
            ));
            let count = self.temp();
            self.line(format!("{count} = load i64, ptr {count_slot}"));
            let id_slot = self.temp();
            self.line(format!(
                "{id_slot} = getelementptr [998 x i64], ptr {ids}, i32 0, i64 {count}"
            ));
            self.line(format!("store i64 {id}, ptr {id_slot}"));
            self.database_append(&batch_slot, model.record, &parent_row, span);
            self.release_guard(parent_owner);
            let next = self.temp();
            self.line(format!("{next} = add i64 {count}, 1"));
            self.line(format!("store i64 {next}, ptr {count_slot}"));
            let full = self.temp();
            self.line(format!("{full} = icmp eq i64 {next}, 998"));
            self.line(format!("br i1 {full}, label %{query_block}, label %{read}"));
            self.start(&exhausted);
            self.line(format!("store i1 1, ptr {done_slot}"));
            let count = self.temp();
            self.line(format!("{count} = load i64, ptr {count_slot}"));
            let empty = self.temp();
            self.line(format!("{empty} = icmp eq i64 {count}, 0"));
            self.line(format!(
                "br i1 {empty}, label %{queried}, label %{query_block}"
            ));
            self.start(&query_block);
            let count = self.temp();
            self.line(format!("{count} = load i64, ptr {count_slot}"));
            let batch = RelationBatch {
                ids: &ids,
                count: &count,
                limit: &limit,
            };
            let rows = self.relation_query(index, &target, &relation.field, batch, span);
            let groups = self.relation_group_rows(&rows, target_index, foreign_key, span);
            let batch = self.temp();
            self.line(format!("{batch} = load ptr, ptr {batch_slot}"));
            self.database_for_each(&batch, model.record, span, |emitter, parent| {
                let id = emitter.temp();
                emitter.line(format!(
                    "{id} = extractvalue %T{} {parent}, 0",
                    model.record
                ));
                let children = emitter.relation_children(&groups, &id, target.record, span);
                let child_ty = Type::List(Box::new(Type::Named(target.record)));
                let child_owner = emitter.own_value(&child_ty, &children, false);
                let pointer = emitter.row_pointer(&child_ty, &children);
                let related = emitter.protocol_call(
                    "dever_rt_v1_db_related_new",
                    vec![
                        format!("ptr @dever_type_{}", emitter.module.type_index(&child_ty)),
                        format!("ptr {pointer}"),
                    ],
                    "ptr",
                    span,
                );
                emitter.protocol_owner("db_related", &related);
                let updated = emitter.temp();
                emitter.line(format!(
                    "{updated} = insertvalue %T{} {parent}, ptr {related}, {field}",
                    model.record
                ));
                emitter.database_append(&output_slot, model.record, &updated, span);
                emitter.release_guard(child_owner);
            });
            self.line(format!("br label %{queried}"));
            self.start(&queried);
            self.release_from(batch_first);
            let exhausted = self.temp();
            self.line(format!("{exhausted} = load i1, ptr {done_slot}"));
            self.line(format!(
                "br i1 {exhausted}, label %{finish}, label %{batch_start}"
            ));
            self.start(&finish);
            self.release_guard(cursor_owner);
            let rebuilt = self.temp();
            self.line(format!("{rebuilt} = load ptr, ptr {output_slot}"));
            self.line(format!(
                "call void @dever_rt_v1_list_release(ptr {parents})"
            ));
            self.line(format!("store ptr {rebuilt}, ptr {list_slot}"));
            self.disarm(output_owner);
        }
    }

    fn relation_query(
        &mut self,
        model: usize,
        target: &ModelSchema,
        field: &str,
        batch: RelationBatch<'_>,
        span: Span,
    ) -> String {
        let RelationBatch { ids, count, limit } = batch;
        let prefix = format!(
            "SELECT {} FROM (SELECT {}, ROW_NUMBER() OVER (PARTITION BY {} ORDER BY {} ASC) AS {} FROM {} WHERE {} IN (",
            sql::select_columns(target),
            sql::select_columns(target),
            sql::quoted(field),
            sql::quoted("id"),
            sql::quoted("_dever_rank"),
            sql::quoted(&target.table),
            sql::quoted(field)
        );
        let before = format!(
            ")) AS {} WHERE {} <= ",
            sql::quoted("_dever_children"),
            sql::quoted("_dever_rank")
        );
        let suffix = format!(
            " ORDER BY {} ASC, {} ASC",
            sql::quoted(field),
            sql::quoted("id")
        );
        let names = [prefix, before, suffix].map(|name| self.module.wire_names(&[name]));
        let database = self.model_database(model);
        let operation = self.database_operation(
            "db_relation_query",
            vec![
                format!("ptr {database}"),
                format!("ptr {}", self.database_context),
                format!("ptr {}", names[0]),
                format!("ptr {}", names[1]),
                format!("ptr {}", names[2]),
                format!("ptr {ids}"),
                format!("i64 {count}"),
                format!("i64 {limit}"),
            ],
            span,
        );
        let rows = self.await_database_handle(&operation, span);
        self.protocol_owner("db_rows", &rows);
        rows
    }

    fn relation_children(&mut self, groups: &str, id: &str, record: usize, span: Span) -> String {
        let retained = self.temp();
        self.line(format!(
            "{retained} = call ptr @dever_rt_v1_map_retain(ptr {groups})"
        ));
        let key = self.row_pointer(&Type::Int, id);
        let list_ty = Type::List(Box::new(Type::Named(record)));
        let optional_ty = Type::Nullable(Box::new(list_ty.clone()));
        let found = self.runtime_call(
            "dever_rt_v1_map_get_take",
            vec![format!("ptr {retained}"), format!("ptr {key}")],
            &optional_ty,
            true,
            None,
            span,
        );
        let present = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {found}, 0",
            self.module.ty(&optional_ty)
        ));
        let slot = self.entry_slot(&list_ty);
        let some = self.label("relation_group_found");
        let missing = self.label("relation_group_empty");
        let done = self.label("relation_group_ready");
        self.line(format!("br i1 {present}, label %{some}, label %{missing}"));
        self.start(&some);
        let list = self.temp();
        self.line(format!(
            "{list} = extractvalue {} {found}, 1",
            self.module.ty(&optional_ty)
        ));
        self.line(format!("store ptr {list}, ptr {slot}"));
        self.line(format!("br label %{done}"));
        self.start(&missing);
        let empty = self.database_empty_list(record, span);
        self.line(format!("store ptr {empty}, ptr {slot}"));
        self.line(format!("br label %{done}"));
        self.start(&done);
        let list = self.temp();
        self.line(format!("{list} = load ptr, ptr {slot}"));
        list
    }

    fn relation_group_rows(
        &mut self,
        rows: &str,
        model: usize,
        foreign_key: usize,
        span: Span,
    ) -> String {
        let target = self.module.program.models[model].clone();
        let list_ty = Type::List(Box::new(Type::Named(target.record)));
        let map_ty = Type::Map(Box::new(Type::Int), Box::new(list_ty.clone()));
        let map = self.runtime_call(
            "dever_rt_v1_map_new",
            vec![
                format!("ptr @dever_type_{}", self.module.type_index(&Type::Int)),
                format!("ptr @dever_type_{}", self.module.type_index(&list_ty)),
                "ptr null".into(),
                "ptr null".into(),
                "i64 0".into(),
            ],
            &map_ty,
            false,
            None,
            span,
        );
        let owner = self.own_value(&map_ty, &map, false);
        let slot = self.guards[owner].pointer.clone();
        let list = self.model_rows_list(model, rows, target.record, None, None, span);
        let list_owner = self.own_value(&list_ty, &list, false);
        self.database_for_each(&list, target.record, span, |emitter, row| {
            let id = emitter.temp();
            emitter.line(format!(
                "{id} = extractvalue %T{} {row}, {foreign_key}",
                target.record
            ));
            let done = emitter.label("relation_child_grouped");
            let id = if target.fields[foreign_key].nullable {
                let present = emitter.temp();
                emitter.line(format!("{present} = extractvalue {{ i1, i64 }} {id}, 0"));
                let group = emitter.label("relation_child_parent");
                emitter.line(format!("br i1 {present}, label %{group}, label %{done}"));
                emitter.start(&group);
                let value = emitter.temp();
                emitter.line(format!("{value} = extractvalue {{ i1, i64 }} {id}, 1"));
                value
            } else {
                id
            };
            let map = emitter.temp();
            emitter.line(format!("{map} = load ptr, ptr {slot}"));
            let children = emitter.relation_children(&map, &id, target.record, span);
            let children_owner = emitter.own_value(&list_ty, &children, false);
            let child_slot = emitter.guards[children_owner].pointer.clone();
            let key = emitter.row_pointer(&Type::Int, &id);
            emitter.disarm(owner);
            let map = emitter.runtime_call(
                "dever_rt_v1_map_remove_take",
                vec![format!("ptr {map}"), format!("ptr {key}")],
                &map_ty,
                false,
                None,
                span,
            );
            emitter.line(format!("store ptr {map}, ptr {slot}"));
            emitter.mark_live(owner);
            emitter.database_append(&child_slot, target.record, row, span);
            emitter.disarm(owner);
            let changed = emitter.runtime_call(
                "dever_rt_v1_map_put_take",
                vec![
                    format!("ptr {map}"),
                    format!("ptr {key}"),
                    format!("ptr {child_slot}"),
                ],
                &map_ty,
                false,
                None,
                span,
            );
            emitter.line(format!("store ptr {changed}, ptr {slot}"));
            emitter.mark_live(owner);
            emitter.release_guard(children_owner);
            emitter.line(format!("br label %{done}"));
            emitter.start(&done);
        });
        self.release_guard(list_owner);
        let groups = self.temp();
        self.line(format!("{groups} = load ptr, ptr {slot}"));
        groups
    }
}
