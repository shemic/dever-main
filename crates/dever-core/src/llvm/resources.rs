//! Resource choices share the existing recoverable Bytes failure channel.
//! File I/O and shared Stream cursor ownership remain in the runtime.

use super::*;

pub(super) struct ResourceRow {
    pub status: String,
    pub output: String,
    pub present: Option<String>,
    pub error: String,
    pub protocol_output: bool,
}

impl Module<'_> {
    pub(super) fn read_event(&mut self, id: usize) -> String {
        let name = format!("@dever_read_event_{id}");
        if !self.read_events.insert(id) {
            return name;
        }
        let Shape::Choice(variants) = &self.program.types[id].shape else {
            unreachable!("checked read event choice")
        };
        let connected = variants.iter().any(|variant| variant.name == "Connected");
        let definitions = [
            if connected {
                ("Connected", Type::Socket, "socket")
            } else {
                ("Chunk", Type::Bytes, "bytes")
            },
            ("Failed", Type::Text, "text"),
        ]
        .map(|(label, ty, kind)| {
            let variant = variants
                .iter()
                .position(|value| value.name == label)
                .expect("checked read event variant");
            (label, ty, kind, variant)
        });
        for (label, ty, kind, variant) in definitions {
            let instance = self
                .instances
                .iter()
                .next()
                .expect("reachable entry")
                .clone();
            let mut emitter = FunctionEmitter::new(self, &instance);
            emitter.start("entry");
            let retained = emitter.temp();
            emitter.line(format!(
                "{retained} = call ptr @dever_rt_v1_{kind}_retain(ptr %handle)"
            ));
            emitter.write_variant(id, variant, &[(ty, retained)], "%out");
            emitter.line("ret void");
            let callback = format!(
                "define internal void @dever_read_{label}_{id}(ptr %handle, ptr %out) {{\nentry:\n{} }}\n",
                emitter.body
            );
            self.declarations.push_str(&callback);
        }
        let success = if connected { "Connected" } else { "Chunk" };
        writeln!(self.declarations,
            "{name} = private constant {{ ptr, ptr, ptr }} {{ ptr @dever_type_{}, ptr @dever_read_{success}_{id}, ptr @dever_read_Failed_{id} }}",
            self.type_index(&Type::Named(id))).unwrap();
        name
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn file_result(
        &mut self,
        operation: Intrinsic,
        mut arguments: Vec<String>,
        result: &Expression,
    ) -> String {
        use Intrinsic::*;
        let (symbol, success, end) = match operation {
            FileOpen => ("file_open", "Opened", None),
            FileCreate => ("file_create", "Opened", None),
            FileRead => ("file_read", "Read", Some("End")),
            FileWrite => ("file_write", "Done", None),
            FileClose => ("file_close", "Closed", None),
            FileChunks => {
                let Type::Named(id) = result.ty else {
                    unreachable!("checked chunks choice")
                };
                let Shape::Choice(variants) = &self.module.program.types[id].shape else {
                    unreachable!("checked chunks choice")
                };
                let variant = variants
                    .iter()
                    .find(|value| value.name == "Streaming")
                    .expect("checked Streaming variant");
                let Type::Stream(element) = &variant.fields[0].ty else {
                    unreachable!("checked chunks stream")
                };
                let Type::Named(event) = element.as_ref() else {
                    unreachable!("checked read event identity")
                };
                let descriptor = self.module.read_event(*event);
                arguments.push(format!("ptr {descriptor}"));
                ("file_chunks", "Streaming", None)
            }
            _ => unreachable!("checked File intrinsic"),
        };
        self.recoverable_call(
            &format!("dever_rt_v1_{symbol}"),
            arguments,
            result,
            success,
            end,
        )
    }

    /// Only runtime status 1 is recoverable. Invalid ABI input never becomes a
    /// user business error; an absent row never reads the uninitialized out slot.
    pub(super) fn recoverable_call(
        &mut self,
        symbol: &str,
        mut arguments: Vec<String>,
        result: &Expression,
        success_name: &str,
        end_name: Option<&str>,
    ) -> String {
        let Type::Named(id) = result.ty else {
            unreachable!("checked resource choice")
        };
        let Shape::Choice(variants) = &self.module.program.types[id].shape else {
            unreachable!("checked resource choice")
        };
        let success = variants
            .iter()
            .position(|value| value.name == success_name)
            .expect("checked resource success");
        let value_type = variants[success]
            .fields
            .first()
            .map(|field| field.ty.clone());
        let output = match &value_type {
            Some(ty) => self.entry_slot(ty),
            None => self.entry_slot_ir("i8"),
        };
        arguments.push(format!("ptr {output}"));
        let present = end_name.map(|_| {
            let slot = self.entry_slot_ir("i8");
            arguments.push(format!("ptr {slot}"));
            slot
        });
        let error = self.entry_slot_ir("{ ptr, i64 }");
        arguments.push(format!("ptr {error}"));
        let status = self.temp();
        self.line(format!(
            "{status} = call i32 @{symbol}({})",
            arguments.join(", ")
        ));
        self.resource_row(
            result,
            success_name,
            end_name,
            ResourceRow {
                status,
                output,
                present,
                error,
                protocol_output: false,
            },
        )
    }

    pub(super) fn resource_row(
        &mut self,
        result: &Expression,
        success_name: &str,
        end_name: Option<&str>,
        row: ResourceRow,
    ) -> String {
        let ResourceRow {
            status,
            output,
            present,
            error,
            protocol_output,
        } = row;
        let Type::Named(id) = result.ty else {
            unreachable!("checked resource choice")
        };
        let Shape::Choice(variants) = &self.module.program.types[id].shape else {
            unreachable!()
        };
        let success = variants
            .iter()
            .position(|variant| variant.name == success_name)
            .expect("checked resource success");
        let failed = variants
            .iter()
            .position(|variant| variant.name == "Failed")
            .expect("checked resource failure");
        let end = end_name.map(|name| {
            variants
                .iter()
                .position(|variant| variant.name == name)
                .expect("checked resource end")
        });
        let value_type = variants[success]
            .fields
            .first()
            .map(|field| field.ty.clone());
        let choice = self.entry_slot(&result.ty);
        let success_block = self.label("resource_success");
        let failure_block = self.label("resource_failed");
        let done = self.label("resource_done");
        let ok = self.temp();
        self.line(format!("{ok} = icmp eq i32 {status}, 0"));
        let not_ok = self.label("resource_status");
        self.line(format!(
            "br i1 {ok}, label %{success_block}, label %{not_ok}"
        ));
        self.start(&not_ok);
        let business = self.temp();
        self.line(format!("{business} = icmp eq i32 {status}, 1"));
        let abi_fault = self.label("resource_abi_fault");
        self.line(format!(
            "br i1 {business}, label %{failure_block}, label %{abi_fault}"
        ));
        self.start(&abi_fault);
        self.check_runtime_status(&status, &error, result.span);
        self.line("unreachable");
        self.start(&success_block);
        if let (Some(end), Some(present)) = (end, present) {
            let flag = self.temp();
            self.line(format!("{flag} = load i8, ptr {present}"));
            let has_value = self.temp();
            self.line(format!("{has_value} = icmp ne i8 {flag}, 0"));
            let available = self.label("resource_present");
            let absent = self.label("resource_end");
            self.line(format!(
                "br i1 {has_value}, label %{available}, label %{absent}"
            ));
            self.start(&absent);
            self.write_variant(id, end, &[], &choice);
            self.line(format!("br label %{done}"));
            self.start(&available);
        }
        let fields = value_type
            .map(|ty| {
                let value = self.temp();
                self.line(format!(
                    "{value} = load {}, ptr {output}",
                    if protocol_output {
                        "ptr".into()
                    } else {
                        self.module.ty(&ty)
                    }
                ));
                let value = if protocol_output {
                    self.protocol_to_language(&ty, &value, result.span)
                } else {
                    value
                };
                (ty, value)
            })
            .into_iter()
            .collect::<Vec<_>>();
        self.write_variant(id, success, &fields, &choice);
        self.line(format!("br label %{done}"));
        self.start(&failure_block);
        let text = self.error_text(&error, result.span);
        self.write_variant(id, failed, &[(Type::Text, text)], &choice);
        self.line(format!("br label %{done}"));
        self.start(&done);
        let value = self.temp();
        self.line(format!("{value} = load %T{id}, ptr {choice}"));
        value
    }

    pub(super) fn error_text(&mut self, error: &str, span: Span) -> String {
        let buffer = self.temp();
        self.line(format!("{buffer} = load {{ ptr, i64 }}, ptr {error}"));
        let bytes = self.temp();
        self.line(format!("{bytes} = extractvalue {{ ptr, i64 }} {buffer}, 0"));
        let length = self.temp();
        self.line(format!(
            "{length} = extractvalue {{ ptr, i64 }} {buffer}, 1"
        ));
        let message = self.entry_slot(&Type::Text);
        let conversion_error = self.entry_slot_ir("{ ptr, i64 }");
        let converted = self.temp();
        self.line(format!("{converted} = call i32 @dever_rt_v1_text_new(ptr {bytes}, i64 {length}, ptr {message}, ptr {conversion_error})"));
        self.line(format!(
            "call void @dever_rt_v1_buffer_free(ptr {bytes}, i64 {length})"
        ));
        self.check_runtime_status(&converted, &conversion_error, span);
        let text = self.temp();
        self.line(format!("{text} = load ptr, ptr {message}"));
        text
    }
}
