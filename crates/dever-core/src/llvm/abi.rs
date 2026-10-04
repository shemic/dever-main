use super::*;

enum AbiOutput<'a> {
    Direct(Option<&'a str>),
    Optional(Option<&'a Type>),
}

pub(super) const DECLARATIONS: &str = "\
declare i32 @dever_rt_v1_time_sleep(i64, ptr, ptr)
declare i32 @dever_rt_v1_text_new(ptr, i64, ptr, ptr)
declare i8 @dever_rt_v1_text_equal(ptr, ptr)
declare i64 @dever_rt_v1_text_hash(ptr)
declare i32 @dever_rt_v1_text_compare(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_concat(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_length(ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_trim(ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_lower(ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_upper(ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_codepoint(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_from_codepoint(i64, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_at(ptr, i64, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_slice(ptr, i64, i64, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_index_of(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_contains(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_starts_with(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_ends_with(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_replace(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_text_split(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_int_parse(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_float_parse(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_decimal_parse(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_int_to_text(i64, ptr)
declare i32 @dever_rt_v1_float_to_text(double, ptr)
declare i32 @dever_rt_v1_decimal_to_text(i64, i64, ptr)
declare i32 @dever_rt_v1_decimal_from_int(i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_add(i64, i64, i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_sub(i64, i64, i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_mul(i64, i64, i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_div(i64, i64, i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_neg(i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_round(i64, i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_decimal_compare(i64, i64, i64, i64, ptr, ptr)
declare i8 @dever_rt_v1_decimal_equal(i64, i64, i64, i64)
declare i8 @dever_rt_v1_bytes_equal(ptr, ptr)
declare i32 @dever_rt_v1_bytes_from_text(ptr, ptr, ptr)
declare i32 @dever_rt_v1_bytes_length(ptr, ptr, ptr)
declare i32 @dever_rt_v1_bytes_at(ptr, i64, ptr, ptr, ptr)
declare i32 @dever_rt_v1_bytes_concat(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_bytes_slice(ptr, i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_bytes_to_text_handle(ptr, ptr, ptr)
declare i32 @dever_rt_v1_bytes_from_ints(ptr, ptr, ptr)
declare i32 @dever_rt_v1_list_new(ptr, ptr, i64, ptr, ptr)
declare i32 @dever_rt_v1_list_append_take(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_list_first_take(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_list_length(ptr, ptr, ptr)
declare i8 @dever_rt_v1_list_equal(ptr, ptr)
declare i32 @dever_rt_v1_list_cursor_take(ptr, ptr, ptr)
declare i32 @dever_rt_v1_list_cursor_next(ptr, ptr, ptr, ptr)
declare void @dever_rt_v1_list_cursor_release(ptr)
declare i32 @dever_rt_v1_map_new(ptr, ptr, ptr, ptr, i64, ptr, ptr)
declare i32 @dever_rt_v1_map_put_take(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_map_remove_take(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_map_get_take(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_map_length(ptr, ptr, ptr)
declare i8 @dever_rt_v1_map_equal(ptr, ptr)
declare i32 @dever_rt_v1_map_cursor_take(ptr, ptr, ptr)
declare i32 @dever_rt_v1_map_cursor_next(ptr, ptr, ptr, ptr, ptr)
declare void @dever_rt_v1_map_cursor_release(ptr)
declare i32 @dever_rt_v1_file_open(ptr, ptr, ptr)
declare i32 @dever_rt_v1_file_create(ptr, ptr, ptr)
declare i32 @dever_rt_v1_file_read(ptr, i64, ptr, ptr, ptr)
declare i32 @dever_rt_v1_file_write(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_file_close(ptr, ptr, ptr)
declare i32 @dever_rt_v1_file_chunks(ptr, i64, ptr, ptr, ptr)
declare i32 @dever_rt_v1_stream_pull(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_stream_close(ptr, ptr, ptr)
declare double @llvm.sqrt.f64(double)
declare double @llvm.sin.f64(double)
declare double @llvm.cos.f64(double)
declare double @llvm.log.f64(double)
declare double @llvm.pow.f64(double, double)
";

impl Module<'_> {
    pub(super) fn text_literal(&mut self, value: &str) -> (String, usize) {
        let len = value.len();
        if len == 0 {
            return ("null".into(), 0);
        }
        let name = format!("@dever_text_literal_{}", self.next_literal);
        self.next_literal += 1;
        let bytes = value
            .as_bytes()
            .iter()
            .map(|byte| format!("\\{byte:02X}"))
            .collect::<String>();
        writeln!(
            self.declarations,
            "{name} = private constant [{len} x i8] c\"{bytes}\""
        )
        .unwrap();
        (name, len)
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn check_runtime_status(&mut self, status: &str, error: &str, span: Span) {
        let failed = self.temp();
        self.line(format!("{failed} = icmp ne i32 {status}, 0"));
        let fault = self.label("runtime_fault");
        let success = self.label("runtime_ok");
        self.line(format!("br i1 {failed}, label %{fault}, label %{success}"));
        self.start(&fault);
        let message = self.temp();
        self.line(format!("{message} = load {{ ptr, i64 }}, ptr {error}"));
        if self.database_errors_override.is_some() {
            self.line(format!("store {{ ptr, i64 }} {message}, ptr %error"));
            self.exit("2");
            self.start(&success);
            return;
        }
        let destination = self.temp();
        self.line(format!(
            "{destination} = getelementptr %dever.fault, ptr %fault, i32 0, i32 4"
        ));
        self.line(format!("store {{ ptr, i64 }} {message}, ptr {destination}"));
        if self.protocol_callback {
            self.exit(status);
        } else {
            self.write_fault(&(FaultCode::RuntimeAbi as i32).to_string(), span);
        }
        self.start(&success);
    }

    pub(super) fn runtime_call(
        &mut self,
        symbol: &str,
        arguments: Vec<String>,
        result_type: &Type,
        nullable: bool,
        physical_type: Option<&str>,
        span: Span,
    ) -> String {
        let output = if nullable {
            assert!(
                physical_type.is_none(),
                "optional ABI output uses its concrete row type"
            );
            AbiOutput::Optional(None)
        } else {
            AbiOutput::Direct(physical_type)
        };
        self.runtime_call_with_output(symbol, arguments, result_type, output, span)
    }

    pub(super) fn runtime_optional_row(
        &mut self,
        symbol: &str,
        arguments: Vec<String>,
        row_type: &Type,
        result_type: &Type,
        span: Span,
    ) -> String {
        self.runtime_call_with_output(
            symbol,
            arguments,
            result_type,
            AbiOutput::Optional(Some(row_type)),
            span,
        )
    }

    fn runtime_call_with_output(
        &mut self,
        symbol: &str,
        mut arguments: Vec<String>,
        result_type: &Type,
        output_kind: AbiOutput<'_>,
        span: Span,
    ) -> String {
        let (value_type, physical_type, nullable) = match output_kind {
            AbiOutput::Direct(physical) => (result_type, physical, false),
            AbiOutput::Optional(row) => {
                let Type::Nullable(inner) = result_type else {
                    unreachable!("nullable ABI result type")
                };
                (row.unwrap_or(inner.as_ref()), None, true)
            }
        };
        let output_type = physical_type
            .map(str::to_owned)
            .unwrap_or_else(|| self.module.ty(value_type));
        let output = self.entry_slot_ir(&output_type);
        arguments.push(format!("ptr {output}"));
        let present = nullable.then(|| {
            let pointer = self.temp();
            writeln!(self.prologue, "  {pointer} = alloca i8").unwrap();
            arguments.push(format!("ptr {pointer}"));
            pointer
        });
        let error = self.temp();
        writeln!(self.prologue, "  {error} = alloca {{ ptr, i64 }}").unwrap();
        arguments.push(format!("ptr {error}"));
        let status = self.temp();
        self.line(format!(
            "{status} = call i32 @{symbol}({})",
            arguments.join(", ")
        ));
        self.check_runtime_status(&status, &error, span);
        if let Some(present) = present {
            self.optional_row(result_type, value_type, &output, &present)
        } else {
            let result = self.temp();
            self.line(format!("{result} = load {output_type}, ptr {output}"));
            if output_type == "i8" && matches!(result_type, Type::Bool) {
                let boolean = self.temp();
                self.line(format!("{boolean} = icmp ne i8 {result}, 0"));
                boolean
            } else {
                result
            }
        }
    }

    /// Presence guards the concrete row; a nullable row is transferred intact.
    /// Collection lookups and Channel receive use the same flattening contract.
    pub(super) fn optional_row(
        &mut self,
        result: &Type,
        row_type: &Type,
        row: &str,
        present: &str,
    ) -> String {
        let slot = self.entry_slot(result);
        self.line(format!(
            "store {} zeroinitializer, ptr {slot}",
            self.module.ty(result)
        ));
        let flag = self.temp();
        self.line(format!("{flag} = load i8, ptr {present}"));
        let has_value = self.temp();
        self.line(format!("{has_value} = icmp ne i8 {flag}, 0"));
        let some = self.label("runtime_some");
        let done = self.label("runtime_optional_done");
        self.line(format!("br i1 {has_value}, label %{some}, label %{done}"));
        self.start(&some);
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {row}",
            self.module.ty(row_type)
        ));
        if row_type == result {
            self.line(format!(
                "store {} {value}, ptr {slot}",
                self.module.ty(result)
            ));
        } else {
            let flag_pointer = self.temp();
            self.line(format!(
                "{flag_pointer} = getelementptr {}, ptr {slot}, i32 0, i32 0",
                self.module.ty(result)
            ));
            self.line(format!("store i1 1, ptr {flag_pointer}"));
            let payload = self.temp();
            self.line(format!(
                "{payload} = getelementptr {}, ptr {slot}, i32 0, i32 1",
                self.module.ty(result)
            ));
            self.line(format!(
                "store {} {value}, ptr {payload}",
                self.module.ty(row_type)
            ));
        }
        self.line(format!("br label %{done}"));
        self.start(&done);
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {slot}",
            self.module.ty(result)
        ));
        value
    }

    pub(super) fn render_text(
        &mut self,
        symbol: &str,
        arguments: Vec<String>,
        span: Span,
    ) -> String {
        let buffer = self.entry_slot_ir("{ ptr, i64 }");
        let mut arguments = arguments;
        arguments.push(format!("ptr {buffer}"));
        let status = self.temp();
        self.line(format!(
            "{status} = call i32 @{symbol}({})",
            arguments.join(", ")
        ));
        self.check_runtime_status(&status, &buffer, span);
        let bytes = self.temp();
        self.line(format!("{bytes} = load {{ ptr, i64 }}, ptr {buffer}"));
        let pointer = self.temp();
        self.line(format!(
            "{pointer} = extractvalue {{ ptr, i64 }} {bytes}, 0"
        ));
        let length = self.temp();
        self.line(format!("{length} = extractvalue {{ ptr, i64 }} {bytes}, 1"));
        let output = self.entry_slot(&Type::Text);
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let created = self.temp();
        self.line(format!("{created} = call i32 @dever_rt_v1_text_new(ptr {pointer}, i64 {length}, ptr {output}, ptr {error})"));
        self.line(format!(
            "call void @dever_rt_v1_buffer_free(ptr {pointer}, i64 {length})"
        ));
        self.check_runtime_status(&created, &error, span);
        let result = self.temp();
        self.line(format!("{result} = load ptr, ptr {output}"));
        result
    }
}
