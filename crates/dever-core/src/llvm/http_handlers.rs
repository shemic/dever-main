//! Static HTTP handlers and typed upload/download callbacks.
use super::*;

impl Module<'_> {
    pub(super) fn upload_decoder(&mut self, id: usize) -> String {
        let name = format!("@dever_upload_decode_{id}");
        if !self.upload_decoders.insert(id) {
            return name;
        }
        let Shape::Choice(variants) = &self.program.types[id].shape else {
            unreachable!()
        };
        let mut callback = format!(
            "define internal i32 {name}(ptr %event, ptr %out) {{\nentry:\n  %tag_ptr = getelementptr %T{id}, ptr %event, i32 0, i32 0\n  %tag = load i32, ptr %tag_ptr\n  %payload_ptr = getelementptr %T{id}, ptr %event, i32 0, i32 1\n  %payload = load ptr, ptr %payload_ptr\n  switch i32 %tag, label %invalid ["
        );
        for (index, _) in variants.iter().enumerate() {
            write!(callback, "i32 {index}, label %variant{index} ").unwrap();
        }
        callback.push_str("]\ninvalid:\n  unreachable\n");
        for (index, variant) in variants.iter().enumerate() {
            let (kind, status) = match variant.name.as_str() {
                "Chunk" => ("bytes", 0),
                "Failed" => ("text", 1),
                _ => unreachable!(),
            };
            writeln!(callback, "variant{index}:\n  %owned{index} = call ptr @dever_rt_v1_{kind}_retain(ptr %payload)\n  store ptr %owned{index}, ptr %out\n  ret i32 {status}").unwrap();
        }
        callback.push_str("}\n");
        self.declarations.push_str(&callback);
        name
    }

    pub(super) fn websocket_event(&mut self, id: usize) -> String {
        let descriptor = format!("@dever_ws_event_{id}");
        if !self.websocket_events.insert(id) {
            return descriptor;
        }
        let Shape::Choice(variants) = self.program.types[id].shape.clone() else {
            unreachable!()
        };
        let read = variants
            .iter()
            .position(|variant| variant.name == "Read")
            .unwrap();
        let failed = variants
            .iter()
            .position(|variant| variant.name == "Failed")
            .unwrap();
        let Type::Named(message) = variants[read].fields[0].ty else {
            unreachable!()
        };
        let Shape::Choice(messages) = self.program.types[message].shape.clone() else {
            unreachable!()
        };
        let instance = self.instances.first().unwrap().clone();
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.start("entry");
        emitter.line("switch i32 %kind, label %invalid [i32 0, label %kind0 i32 1, label %kind1 i32 2, label %kind2 i32 3, label %kind3]");
        emitter.start("invalid");
        emitter.line("unreachable");
        for (kind, name) in ["Text", "Binary", "Ping", "Pong"].iter().enumerate() {
            emitter.start(&format!("kind{kind}"));
            let (variant, definition) = messages
                .iter()
                .enumerate()
                .find(|(_, variant)| variant.name == *name)
                .unwrap();
            let symbol = if kind == 0 { "text" } else { "bytes" };
            let owned = emitter.temp();
            emitter.line(format!(
                "{owned} = call ptr @dever_rt_v1_{symbol}_retain(ptr %payload)"
            ));
            let slot = emitter.entry_slot(&Type::Named(message));
            emitter.write_variant(
                message,
                variant,
                &[(definition.fields[0].ty.clone(), owned)],
                &slot,
            );
            let value = emitter.temp();
            emitter.line(format!("{value} = load %T{message}, ptr {slot}"));
            emitter.write_variant(id, read, &[(Type::Named(message), value)], "%out");
            emitter.line("ret void");
        }
        let callback = format!(
            "define internal void @dever_ws_read_{id}(i32 %kind, ptr %payload, ptr %out) {{\nentry:\n{}{} }}\n",
            emitter.prologue, emitter.body
        );
        self.declarations.push_str(&callback);
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.start("entry");
        let text = emitter.temp();
        emitter.line(format!(
            "{text} = call ptr @dever_rt_v1_text_retain(ptr %payload)"
        ));
        emitter.write_variant(id, failed, &[(Type::Text, text)], "%out");
        emitter.line("ret void");
        let callback = format!(
            "define internal void @dever_ws_failed_{id}(ptr %payload, ptr %out) {{\nentry:\n{}{} }}\n",
            emitter.prologue, emitter.body
        );
        self.declarations.push_str(&callback);
        writeln!(self.declarations, "{descriptor} = private constant {{ ptr, ptr, ptr }} {{ ptr @dever_type_{}, ptr @dever_ws_read_{id}, ptr @dever_ws_failed_{id} }}", self.type_index(&Type::Named(id))).unwrap();
        descriptor
    }

    fn http_handler(&mut self, instance: &Specialization, live: bool) -> String {
        let name = self.names[instance].clone();
        let descriptor = format!("@dever_http_handler_{name}");
        if !self.http_handlers.insert(instance.clone()) {
            return descriptor;
        }
        let function = self.program.functions[instance.function].clone();
        let input = asynchronous::input_type(&function);
        let Type::Outputs(fields) = &input else {
            unreachable!()
        };
        let context_index = if live { 2 } else { 1 };
        let context = fields.get(context_index).map_or("null".into(), |field| {
            format!("@dever_type_{}", self.type_index(&field.ty))
        });
        let mut emitter = FunctionEmitter::new(self, instance);
        emitter.asynchronous = false;
        emitter.start("entry");
        let request = emitter.temp();
        emitter.line(format!(
            "{request} = call ptr @dever_rt_v1_http_request_retain(ptr %request)"
        ));
        let value = emitter.protocol_to_language(&fields[0].ty, &request, function.span);
        let request_guard = emitter.own_value(&fields[0].ty, &value, false);
        let request_row = emitter.guards[request_guard].pointer.clone();
        for (index, field) in fields.iter().enumerate() {
            let target = emitter.temp();
            emitter.line(format!(
                "{target} = getelementptr {}, ptr %out, i32 0, i32 {index}",
                emitter.module.ty(&input)
            ));
            if index == 0 {
                emitter.line(format!(
                    "call void @dever_clone_{}(ptr {request_row}, ptr {target})",
                    emitter.module.type_index(&field.ty)
                ));
            } else if live && index == 1 {
                let reply = emitter.temp();
                emitter.line(format!(
                    "{reply} = call ptr @dever_rt_v1_http_reply_retain(ptr %reply)"
                ));
                emitter.line(format!("store ptr {reply}, ptr {target}"));
            } else {
                emitter.line(format!(
                    "call void @dever_clone_{}(ptr %context, ptr {target})",
                    emitter.module.type_index(&field.ty)
                ));
            }
        }
        let callback = emitter.finish_protocol_callback(
            &format!("dever_http_pack_{name}"),
            "ptr %request, ptr %reply, ptr %context, ptr %out, ptr %error",
        );
        self.declarations.push_str(&callback);
        let response = if live {
            "null".into()
        } else {
            let mut emitter = FunctionEmitter::new(self, instance);
            emitter.asynchronous = false;
            emitter.start("entry");
            let output_ty = &function.outputs[0].ty;
            let output = emitter.temp();
            emitter.line(format!(
                "{output} = load {}, ptr %input",
                emitter.module.ty(output_ty)
            ));
            let handle = emitter.protocol_to_runtime(output_ty, &output, function.span);
            emitter.line(format!("store ptr {handle}, ptr %out"));
            emitter.disarm(emitter.guards.len() - 1);
            let callback = emitter.finish_protocol_callback(
                &format!("dever_http_response_{name}"),
                "ptr %input, ptr %out, ptr %error",
            );
            self.declarations.push_str(&callback);
            format!("@dever_http_response_{name}")
        };
        let message = self.http_fault_message(instance);
        let (asynchronous, synchronous) = if specialize::suspends(self.program, instance) {
            (format!("@dever_async_{name}"), "null".into())
        } else {
            ("null".into(), format!("@dever_sync_{name}"))
        };
        writeln!(self.declarations, "{descriptor} = private constant %dever.http_handler {{ ptr {asynchronous}, ptr {synchronous}, ptr @dever_owned_{}, ptr {context}, ptr @dever_http_pack_{name}, ptr {response}, ptr {message} }}", self.type_index(&input)).unwrap();
        descriptor
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn finish_protocol_callback(mut self, name: &str, arguments: &str) -> String {
        self.exit("0");
        self.start("cleanup");
        self.release_from(0);
        let status = self.temp();
        self.line(format!("{status} = load i32, ptr %exit_status"));
        let message = self.temp();
        self.line(format!(
            "{message} = getelementptr %dever.fault, ptr %fault, i32 0, i32 4"
        ));
        let buffer = self.temp();
        self.line(format!("{buffer} = load {{ ptr, i64 }}, ptr {message}"));
        self.line(format!("store {{ ptr, i64 }} {buffer}, ptr %error"));
        self.line(format!("ret i32 {status}"));
        format!(
            "define internal i32 @{name}({arguments}) {{\nentry:\n  %exit_status = alloca i32\n  store i32 0, ptr %exit_status\n  %fault = alloca %dever.fault\n  store %dever.fault zeroinitializer, ptr %fault\n{}{} }}\n",
            self.prologue, self.body
        )
    }

    pub(super) fn http_serve(
        &mut self,
        operation: Intrinsic,
        handler: crate::hir::HandlerTarget,
        arguments: &[Expression],
        values: &[String],
        result: &Expression,
    ) -> String {
        let live = matches!(
            operation,
            Intrinsic::HttpServeLive | Intrinsic::HttpServeLiveTls
        );
        let tls = matches!(
            operation,
            Intrinsic::HttpServeTls | Intrinsic::HttpServeLiveTls
        );
        let instance = Specialization {
            function: specialize::resolve_handler(handler, &self.bindings),
            handlers: Vec::new(),
        };
        let descriptor = self.module.http_handler(&instance, live);
        let limits = self.http_limits(&arguments[1].ty, &values[1]);
        let live_limits = if live {
            self.protocol_limits(
                &arguments[2].ty,
                &values[2],
                &["chunk_bytes", "idle_ms", "heartbeat_ms"],
            )
        } else {
            "null".into()
        };
        let tls_index = if live { 3 } else { 2 };
        let tls_value = if tls {
            values[tls_index].clone()
        } else {
            "null".into()
        };
        let context_index = tls_index + usize::from(tls);
        let context = arguments
            .get(context_index)
            .map_or("null".into(), |argument| {
                self.row_pointer(&argument.ty, &values[context_index])
            });
        let operation = self.async_operation(
            "async_http_serve",
            vec![
                format!("ptr {}", values[0]),
                format!("ptr {limits}"),
                format!("ptr {live_limits}"),
                format!("ptr {tls_value}"),
                format!("ptr {descriptor}"),
                format!("ptr {context}"),
            ],
            result.span,
        );
        self.await_operation(&operation, &Type::Unit, result.span, true)
    }
}
