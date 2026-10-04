use super::*;

impl FunctionEmitter<'_, '_> {
    pub(super) fn call_instance(
        &self,
        target: CallTarget,
        arguments: &[CallArgument],
    ) -> Specialization {
        match target {
            CallTarget::Function(function) => {
                specialize::call_specialization(function, arguments, &self.bindings)
            }
            CallTarget::Handler(target) => Specialization {
                function: specialize::resolve_handler(target, &self.bindings),
                handlers: Vec::new(),
            },
        }
    }

    pub(super) fn invoke(
        &mut self,
        instance: &Specialization,
        arguments: &[CallArgument],
        output: &Type,
        span: Span,
    ) -> Result<(String, String), String> {
        let mut values = Vec::new();
        for argument in arguments {
            if let CallArgument::Value(value) = argument {
                let result = self.expression(value)?;
                values.push((value.ty.clone(), result));
            }
        }
        Ok(self.invoke_values(instance, &values, output, span))
    }

    pub(super) fn invoke_values(
        &mut self,
        instance: &Specialization,
        arguments: &[(Type, String)],
        output: &Type,
        span: Span,
    ) -> (String, String) {
        if specialize::suspends(self.module.program, instance) {
            let (input, _) = self.pack_input(instance, arguments, false);
            let (input, descriptor) = if self.module.database_function(instance) {
                let ty =
                    asynchronous::input_type(&self.module.program.functions[instance.function]);
                let layout = format!("{{ {}, ptr }}", self.module.ty(&ty));
                let extended = self.entry_slot_ir(&layout);
                let value = self.temp();
                self.line(format!(
                    "{value} = load {}, ptr {input}",
                    self.module.ty(&ty)
                ));
                self.line(format!(
                    "store {} {value}, ptr {extended}",
                    self.module.ty(&ty)
                ));
                let context = self.temp();
                self.line(format!(
                    "{context} = getelementptr {layout}, ptr {extended}, i32 0, i32 1"
                ));
                self.line(format!(
                    "store ptr {}, ptr {context}",
                    self.database_context
                ));
                (
                    extended,
                    format!("@dever_async_context_{}", self.module.names[instance]),
                )
            } else {
                (
                    input,
                    format!("@dever_async_{}", self.module.names[instance]),
                )
            };
            let operation = self.async_operation(
                "async_call",
                vec![format!("ptr {descriptor}"), format!("ptr {input}")],
                span,
            );
            return self.poll_operation(&operation, output, span);
        }
        let result = self.entry_slot(output);
        self.line(format!(
            "store {} zeroinitializer, ptr {result}",
            self.module.ty(output)
        ));
        let mut values = arguments
            .iter()
            .map(|(ty, value)| format!("{} {value}", self.module.ty(ty)))
            .collect::<Vec<_>>();
        if self.module.database_function(instance) {
            values.push(format!("ptr {}", self.database_context));
        }
        values.push(format!("ptr {result}"));
        values.push("ptr %fault".into());
        let status = self.temp();
        self.line(format!(
            "{status} = call i32 @{}({})",
            self.module.names[instance],
            values.join(", ")
        ));
        (status, result)
    }

    pub(super) fn fail(&mut self, error: &Expression, span: Span) -> Result<String, String> {
        let Type::Named(id) = error.ty else {
            unreachable!("checked fail choice")
        };
        let ExpressionKind::Variant { variant, .. } = error.kind else {
            unreachable!("checked direct failure variant")
        };
        let value = self.expression(error)?;
        let payload = self.temp();
        self.line(format!(
            "{payload} = getelementptr %dever.fault, ptr %fault, i32 0, i32 7"
        ));
        self.line(format!("store %T{id} {value}, ptr {payload}"));
        if contains_owned(self.module.program, &error.ty) {
            self.disarm(self.guards.len() - 1);
        }
        for (index, number) in [(5, id + 1), (6, variant)] {
            let pointer = self.temp();
            self.line(format!(
                "{pointer} = getelementptr %dever.fault, ptr %fault, i32 0, i32 {index}"
            ));
            self.line(format!("store i32 {number}, ptr {pointer}"));
        }
        self.write_fault(&(FaultCode::Business as i32).to_string(), span);
        let dead = self.label("after_fail");
        self.start(&dead);
        Ok("zeroinitializer".into())
    }

    pub(super) fn capture_result(
        &mut self,
        target: CallTarget,
        arguments: &[CallArgument],
        choice: usize,
        success: usize,
        expression: &Expression,
    ) -> Result<String, String> {
        let instance = self.call_instance(target, arguments);
        let function = &self.module.program.functions[instance.function];
        let output = output_type(&function.outputs);
        let failures = specialize::failures(self.module.program, &instance);
        let targets = crate::capture::targets(&self.module.program.types, choice, &failures)
            .expect("checked result capture mapping");
        let (status, output_slot) = self.invoke(&instance, arguments, &output, expression.span)?;
        let result_slot = self.entry_slot(&expression.ty);
        let success_block = self.label("capture_success");
        let failure_block = self.label("capture_failure");
        let done = self.label("capture_done");
        let ok = self.temp();
        self.line(format!("{ok} = icmp eq i32 {status}, 0"));
        self.line(format!(
            "br i1 {ok}, label %{success_block}, label %{failure_block}"
        ));

        self.start(&success_block);
        let success_fields = match &self.module.program.types[choice].shape {
            Shape::Choice(variants) => variants[success].fields.clone(),
            _ => unreachable!("checked capture choice"),
        };
        let values = success_fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let value = self.temp();
                if success_fields.len() == 1 {
                    self.line(format!(
                        "{value} = load {}, ptr {output_slot}",
                        self.module.ty(&field.ty)
                    ));
                } else {
                    let aggregate = self.temp();
                    self.line(format!(
                        "{aggregate} = load {}, ptr {output_slot}",
                        self.module.ty(&output)
                    ));
                    self.line(format!(
                        "{value} = extractvalue {} {aggregate}, {index}",
                        self.module.ty(&output)
                    ));
                }
                (field.ty.clone(), value)
            })
            .collect::<Vec<_>>();
        self.write_variant(choice, success, &values, &result_slot);
        self.line(format!("br label %{done}"));

        self.start(&failure_block);
        let kind_ptr = self.temp();
        self.line(format!(
            "{kind_ptr} = getelementptr %dever.fault, ptr %fault, i32 0, i32 0"
        ));
        let kind = self.temp();
        self.line(format!("{kind} = load i32, ptr {kind_ptr}"));
        let business = self.temp();
        self.line(format!(
            "{business} = icmp eq i32 {kind}, {}",
            FaultCode::Business as i32
        ));
        let dispatch = self.label("capture_dispatch");
        let propagate = self.label("capture_propagate");
        self.line(format!(
            "br i1 {business}, label %{dispatch}, label %{propagate}"
        ));
        self.start(&dispatch);
        let type_ptr = self.temp();
        self.line(format!(
            "{type_ptr} = getelementptr %dever.fault, ptr %fault, i32 0, i32 5"
        ));
        let type_id = self.temp();
        self.line(format!("{type_id} = load i32, ptr {type_ptr}"));
        let variant_ptr = self.temp();
        self.line(format!(
            "{variant_ptr} = getelementptr %dever.fault, ptr %fault, i32 0, i32 6"
        ));
        let variant_id = self.temp();
        self.line(format!("{variant_id} = load i32, ptr {variant_ptr}"));
        for (failure, target) in targets {
            if !failures.contains(&failure) {
                continue;
            }
            let matches_type = self.temp();
            self.line(format!(
                "{matches_type} = icmp eq i32 {type_id}, {}",
                failure.ty + 1
            ));
            let matches_variant = self.temp();
            self.line(format!(
                "{matches_variant} = icmp eq i32 {variant_id}, {}",
                failure.variant
            ));
            let matches = self.temp();
            self.line(format!(
                "{matches} = and i1 {matches_type}, {matches_variant}"
            ));
            let captured = self.label("capture_match");
            let next = self.label("capture_next");
            self.line(format!("br i1 {matches}, label %{captured}, label %{next}"));
            self.start(&captured);
            let payload = self.temp();
            self.line(format!(
                "{payload} = getelementptr %dever.fault, ptr %fault, i32 0, i32 7"
            ));
            let inner = self.temp();
            self.line(format!("{inner} = load %T{}, ptr {payload}", failure.ty));
            if target.wrapped {
                self.write_variant(
                    choice,
                    target.variant,
                    &[(Type::Named(failure.ty), inner)],
                    &result_slot,
                );
            } else {
                self.line(format!("store %T{choice} {inner}, ptr {result_slot}"));
            }
            if self.module.uses_runtime() {
                let cause_ptr = self.temp();
                self.line(format!(
                    "{cause_ptr} = getelementptr %dever.fault, ptr %fault, i32 0, i32 8"
                ));
                let cause = self.temp();
                self.line(format!("{cause} = load ptr, ptr {cause_ptr}"));
                self.line(format!("call void @dever_rt_v1_text_release(ptr {cause})"));
            }
            self.line("store %dever.fault zeroinitializer, ptr %fault");
            self.line(format!("br label %{done}"));
            self.start(&next);
        }
        self.line(format!("br label %{propagate}"));
        self.start(&propagate);
        self.append_frame(expression.span);
        self.exit(&status);
        self.start(&done);
        let result = self.temp();
        self.line(format!("{result} = load %T{choice}, ptr {result_slot}"));
        Ok(result)
    }

    pub(super) fn write_variant(
        &mut self,
        id: usize,
        variant: usize,
        fields: &[(Type, String)],
        slot: &str,
    ) {
        self.line(format!("store %T{id} zeroinitializer, ptr {slot}"));
        let tag = self.temp();
        self.line(format!(
            "{tag} = getelementptr %T{id}, ptr {slot}, i32 0, i32 0"
        ));
        self.line(format!("store i32 {variant}, ptr {tag}"));
        let payload = self.temp();
        self.line(format!(
            "{payload} = getelementptr %T{id}, ptr {slot}, i32 0, i32 1"
        ));
        for (index, (ty, value)) in fields.iter().enumerate() {
            let field = self.temp();
            self.line(format!(
                "{field} = getelementptr %T{id}V{variant}, ptr {payload}, i32 0, i32 {index}"
            ));
            self.line(format!("store {} {value}, ptr {field}", self.module.ty(ty)));
        }
    }
}
