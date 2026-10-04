//! Typed database boundary. SQL values never enter source-language evaluation.
use super::*;

pub(super) const DECLARATIONS: &str = "\
%dever.db_error = type { ptr, ptr, ptr }
%dever.db_value = type { i32, i32, i32, i64, double, { i64, i64 }, ptr }
%dever.db_sql = type { { ptr, i64 }, { ptr, i64 } }
declare void @dever_rt_v1_db_cause_append(ptr, ptr)
declare i32 @dever_rt_v1_db_application_root(ptr, ptr, ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_session_new(ptr, i64, ptr, i64, ptr, ptr)
declare void @dever_rt_v1_db_session_release(ptr)
declare ptr @dever_rt_v1_db_prepare(ptr, ptr, ptr)
declare ptr @dever_rt_v1_db_resolve_scoped(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_select(ptr, ptr, ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_db_retain(ptr)
declare void @dever_rt_v1_db_release(ptr)
declare i32 @dever_rt_v1_db_max_page_size(ptr, ptr, ptr)
declare ptr @dever_rt_v1_db_begin(ptr, ptr, ptr)
declare ptr @dever_rt_v1_db_transaction_retain(ptr)
declare void @dever_rt_v1_db_transaction_release(ptr)
declare ptr @dever_rt_v1_db_finish(ptr, i8, ptr, ptr)
declare ptr @dever_rt_v1_db_rollback_take(ptr, ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_db_execute(ptr, ptr, ptr, ptr, i64, ptr, ptr)
declare ptr @dever_rt_v1_db_query(ptr, ptr, ptr, ptr, i64, i64, ptr, ptr)
declare ptr @dever_rt_v1_db_relation_query(ptr, ptr, ptr, ptr, ptr, ptr, i64, i64, ptr, ptr)
declare ptr @dever_rt_v1_db_stream(ptr, ptr, ptr, ptr, i64, i64, ptr, ptr, ptr)
declare ptr @dever_rt_v1_db_stream_retain(ptr)
declare void @dever_rt_v1_db_stream_release(ptr)
declare i32 @dever_rt_v1_db_stream_close(ptr, ptr, ptr)
declare ptr @dever_rt_v1_db_stream_pull(ptr, ptr, ptr)
declare void @dever_rt_v1_db_rows_release(ptr)
declare i32 @dever_rt_v1_db_rows_len(ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_rows_at(ptr, i64, ptr, ptr)
declare i32 @dever_rt_v1_db_required_row(ptr, ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_optional_row(ptr, ptr, ptr, ptr, ptr, ptr, ptr)
declare void @dever_rt_v1_db_row_release(ptr)
declare i32 @dever_rt_v1_db_row_len(ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_related_new(ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_db_related_retain(ptr)
declare void @dever_rt_v1_db_related_release(ptr)
declare i8 @dever_rt_v1_db_related_equal(ptr, ptr)
declare i32 @dever_rt_v1_db_related_get(ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_row_null(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_row_bool(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_row_int(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_row_float(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_row_decimal(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_row_text(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_row_bytes(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_row_uuid(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_uuid_new(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_error(i32, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_pagination(i64, i64, i64, ptr, ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_cursor_size(i64, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_stream_capacity(i64, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_db_relation_limit(i64, ptr, ptr, ptr, ptr)
";

impl Module<'_> {
    pub(super) fn database_enabled(&self) -> bool {
        self.application.as_ref().is_some_and(|app| match app.test {
            Some(index) => self.program.tests[index].uses_database(),
            None => !self.program.models.is_empty() || !self.program.jobs.is_empty(),
        })
    }

    pub(super) fn database_function(&self, instance: &Specialization) -> bool {
        !specialize::database_effects(self.program, instance).is_empty()
    }

    pub(super) fn database_error(&mut self, instance: &Specialization, span: Span) -> String {
        let location = self.location(span);
        let name = self.names[instance].clone();
        let symbol = format!("@dever_db_error_{name}_{location}");
        if !self.database_errors.insert((name.clone(), location)) {
            return symbol;
        }
        let id = crate::capture::database_errors(&self.program.types)[0].ty;
        let Shape::Choice(variants) = &self.program.types[id].shape else {
            unreachable!()
        };
        let kinds = [
            "Pool",
            "PoolExhausted",
            "Connection",
            "Timeout",
            "Cancelled",
            "Database",
            "Constraint",
            "NotFound",
            "InvalidData",
            "Migration",
        ];
        let variants = kinds
            .iter()
            .map(|name| {
                variants
                    .iter()
                    .position(|variant| variant.name == *name)
                    .expect("bundled database error kind")
            })
            .collect::<Vec<_>>();
        writeln!(self.declarations, "{symbol} = private constant %dever.db_error {{ ptr @dever_fault_owned_{name}, ptr @dever_db_pack_{name}_{location}, ptr @dever_db_append_cause }}").unwrap();
        writeln!(self.declarations, "define internal void @dever_db_pack_{name}_{location}(i32 %kind, ptr %message, ptr %fault) {{\nentry:\n  store %dever.fault zeroinitializer, ptr %fault\n  %owned = call ptr @dever_rt_v1_text_retain(ptr %message)\n  %payload = getelementptr %dever.fault, ptr %fault, i32 0, i32 7").unwrap();
        for (index, value) in [(0, 4), (1, location), (5, id + 1)] {
            writeln!(self.declarations, "  %f{index} = getelementptr %dever.fault, ptr %fault, i32 0, i32 {index}\n  store i32 {value}, ptr %f{index}").unwrap();
        }
        let arms = variants
            .iter()
            .enumerate()
            .map(|(kind, variant)| format!("i32 {kind}, label %kind{variant}"))
            .collect::<Vec<_>>()
            .join(" ");
        writeln!(
            self.declarations,
            "  switch i32 %kind, label %invalid [{arms}]\ninvalid:\n  unreachable"
        )
        .unwrap();
        for variant in variants {
            writeln!(self.declarations, "kind{variant}:\n  %tag{variant} = getelementptr %T{id}, ptr %payload, i32 0, i32 0\n  store i32 {variant}, ptr %tag{variant}\n  %body{variant} = getelementptr %T{id}, ptr %payload, i32 0, i32 1\n  store ptr %owned, ptr %body{variant}\n  %variant{variant} = getelementptr %dever.fault, ptr %fault, i32 0, i32 6\n  store i32 {variant}, ptr %variant{variant}\n  ret void").unwrap();
        }
        self.declarations.push_str("}\n");
        symbol
    }

    pub(super) fn database_sql(&mut self, sqlite: &str, postgres: &str) -> String {
        let (sqlite, slen) = self.text_literal(sqlite);
        let (postgres, plen) = self.text_literal(postgres);
        let name = format!("@dever_db_sql_{}", self.next_literal);
        self.next_literal += 1;
        writeln!(self.declarations, "{name} = private constant %dever.db_sql {{ {{ ptr, i64 }} {{ ptr {sqlite}, i64 {slen} }}, {{ ptr, i64 }} {{ ptr {postgres}, i64 {plen} }} }}").unwrap();
        name
    }

    pub(super) fn planned_sql(&mut self, sql: &str) -> String {
        let (sqlite, postgres) = crate::native::orm::sql_dialects(sql, sql);
        self.database_sql(&sqlite, &postgres)
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn await_database_handle(&mut self, operation: &str, span: Span) -> String {
        let (status, output, _, error) = self.poll_raw_layout(operation, "ptr");
        let status = self.poll_status(&status, &error, span);
        self.propagate_status(&status, span, false);
        let handle = self.temp();
        self.line(format!("{handle} = load ptr, ptr {output}"));
        handle
    }
    pub(super) fn database_errors(&mut self, span: Span) -> String {
        self.database_errors_override
            .clone()
            .unwrap_or_else(|| self.module.database_error(self.instance, span))
    }
    pub(super) fn database_status(&mut self, status: &str, error: &str, span: Span) {
        let typed = self.temp();
        let failed = self.label("database_fault");
        let checked = self.label("database_status");
        self.line(format!("{typed} = icmp eq i32 {status}, 1"));
        self.line(format!("br i1 {typed}, label %{failed}, label %{checked}"));
        self.start(&failed);
        self.exit("1");
        self.start(&checked);
        self.check_runtime_status(status, error, span);
    }

    pub(super) fn database_call(
        &mut self,
        symbol: &str,
        mut arguments: Vec<String>,
        layout: &str,
        span: Span,
    ) -> String {
        let output = self.entry_slot_ir(layout);
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let errors = self.database_errors(span);
        arguments.extend([
            format!("ptr {output}"),
            format!("ptr {errors}"),
            "ptr %fault".into(),
            format!("ptr {error}"),
        ]);
        let status = self.temp();
        self.line(format!(
            "{status} = call i32 @dever_rt_v1_{symbol}({})",
            arguments.join(", ")
        ));
        self.database_status(&status, &error, span);
        let value = self.temp();
        self.line(format!("{value} = load {layout}, ptr {output}"));
        value
    }

    pub(super) fn database_operation(
        &mut self,
        symbol: &str,
        mut arguments: Vec<String>,
        span: Span,
    ) -> String {
        let errors = self.database_errors(span);
        arguments.push(format!("ptr {errors}"));
        self.async_operation(symbol, arguments, span)
    }

    pub(super) fn database_fault(&mut self, kind: u32, message: &str, span: Span) {
        let text = self.application_text(message, span);
        let errors = self.database_errors(span);
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_db_error(i32 {kind}, ptr {text}, ptr {errors}, ptr %fault, ptr {error})"));
        self.database_status(&status, &error, span);
        self.line("unreachable");
    }

    pub(super) fn model_database(&mut self, model: usize) -> String {
        if self.module.api_enabled() {
            let span = self.module.program.functions[self.instance.function].span;
            let session = self.temp();
            self.line(format!("{session} = load ptr, ptr @dever_database_session"));
            let binding = self.module.database_binding(model);
            let slot = self.entry_slot_ir("%dever.db_binding");
            self.line(format!("store %dever.db_binding {binding}, ptr {slot}"));
            let operation = self.database_operation(
                "db_resolve_scoped",
                vec![format!("ptr {session}"), format!("ptr {slot}")],
                span,
            );
            let database = self.await_database_handle(&operation, span);
            self.protocol_owner("db", &database);
            return database;
        }
        let database = self.temp();
        self.line(format!(
            "{database} = load ptr, ptr @dever_database_{model}"
        ));
        database
    }
}
