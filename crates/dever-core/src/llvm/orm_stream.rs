//! RowStream pulls decode at the consumer's error location and own no borrowed row.
use super::*;
use crate::hir::QueryPlan;
use crate::model::ModelSchema;
use crate::native::orm as sql;

impl Module<'_> {
    pub(super) fn database_row_decoder(
        &mut self,
        record: usize,
        instance: &Specialization,
    ) -> String {
        let symbol = format!("@dever_db_decoder_{record}");
        if !self.database_decoders.insert(record) {
            return symbol;
        }
        let index = self.type_index(&Type::Named(record));
        writeln!(self.declarations, "{symbol} = private constant {{ ptr, ptr }} {{ ptr @dever_owned_{index}, ptr @dever_db_decode_{record} }}").unwrap();
        let span = self.program.types[record].span;
        let mut emitter = FunctionEmitter::new(self, instance);
        emitter.asynchronous = false;
        emitter.database_errors_override = Some("%errors".into());
        emitter.start("entry");
        emitter
            .prologue
            .push_str("  %exit_status = alloca i32\n  store i32 0, ptr %exit_status\n");
        let value = emitter.database_record("%row", record, 0, true, span);
        emitter.line(format!("store %T{record} {value}, ptr %out"));
        emitter.exit("0");
        emitter.start("cleanup");
        emitter.release_from(0);
        let status = emitter.temp();
        emitter.line(format!("{status} = load i32, ptr %exit_status"));
        emitter.line(format!("ret i32 {status}"));
        let body = format!(
            "define internal i32 @dever_db_decode_{record}(ptr %row, ptr %out, ptr %errors, ptr %fault, ptr %error) {{\nentry:\n{}{} }}\n",
            emitter.prologue, emitter.body
        );
        self.declarations.push_str(&body);
        symbol
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn model_stream(
        &mut self,
        index: usize,
        model: &ModelSchema,
        query: &QueryPlan,
        result: &Expression,
    ) -> Result<String, String> {
        let mut expressions = Vec::new();
        let condition = sql::condition_sql(model, query.condition.as_ref(), &mut expressions);
        let values = self.model_bind(&expressions)?;
        let capacity = self.expression(&query.size)?;
        let maximum = self.model_maximum(index, result.span);
        let capacity = self.database_call(
            "db_stream_capacity",
            vec![format!("i64 {capacity}"), format!("i64 {maximum}")],
            "i64",
            result.span,
        );
        let statement = self.module.planned_sql(&format!(
            "SELECT {} FROM {}{condition} ORDER BY {}",
            sql::select_columns(model),
            sql::quoted(&model.table),
            sql::order_sql(model, &query.order)
        ));
        let parameters = self.database_parameters(&values, result.span);
        let database = self.model_database(index);
        let decoder = self
            .module
            .database_row_decoder(model.record, self.instance);
        let operation = self.database_operation(
            "db_stream",
            vec![
                format!("ptr {database}"),
                format!("ptr {}", self.database_context),
                format!("ptr {statement}"),
                format!("ptr {parameters}"),
                format!("i64 {}", values.len()),
                format!("i64 {capacity}"),
                format!("ptr {decoder}"),
            ],
            result.span,
        );
        Ok(self.await_database_handle(&operation, result.span))
    }
}
