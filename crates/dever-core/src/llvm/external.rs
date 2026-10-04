//! Typed external operation envelopes over the shared invocation-owned supervisor.
use super::*;
use crate::hir::AdapterImplementation;
use crate::native::EmbeddedResource;
use crate::types::Field;

const DECLARATIONS: &str = "\
%dever.external_resource = type { { ptr, i64 }, ptr, i64, { ptr, i64 }, i8 }
%dever.external_definition = type { { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, ptr, i64, ptr, i64, i64 }
declare i32 @dever_rt_v1_external_prepare(ptr, ptr, i64, ptr)
declare ptr @dever_rt_v1_external_start(ptr, ptr, ptr)
declare ptr @dever_rt_v1_external_call(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_external_reply_take(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_external_root(ptr, ptr, ptr, ptr, ptr, ptr)
declare void @dever_rt_v1_external_cause_append(ptr, ptr)
";

fn input_fields(program: &Program, function: usize) -> Vec<Field> {
    let function = &program.functions[function];
    let external = function.external.expect("external operation");
    let contract = program.functions[external.contract].port.as_ref().unwrap();
    contract
        .input_names
        .iter()
        .zip(&function.parameters)
        .map(|(name, parameter)| Field {
            name: name.clone(),
            ty: parameter
                .value_type()
                .expect("checked Port value input")
                .clone(),
            private: false,
            bounds: Vec::new(),
        })
        .collect()
}

impl Module<'_> {
    pub(super) fn external_enabled(&self) -> bool {
        self.application
            .as_ref()
            .is_some_and(|app| app.resources.is_some())
            || self
                .instances
                .iter()
                .any(|instance| self.program.functions[instance.function].external.is_some())
    }

    pub(super) fn collect_external_types(&mut self) {
        for instance in self.instances.clone() {
            let function = &self.program.functions[instance.function];
            let Some(external) = function.external else {
                continue;
            };
            let mut rows = vec![
                input_fields(self.program, instance.function),
                function.outputs.clone(),
            ];
            let contract = self.program.functions[external.contract]
                .port
                .as_ref()
                .unwrap();
            for failure in &contract.failures {
                let Shape::Choice(variants) = &self.program.types[failure.ty].shape else {
                    unreachable!()
                };
                rows.push(variants[failure.variant].fields.clone());
            }
            for fields in rows {
                collect_owned_types(self.program, &Type::Outputs(fields), &mut self.value_types);
            }
        }
    }

    pub(super) fn emit_external_descriptors(
        &mut self,
        resources: &[EmbeddedResource],
        indices: &[usize],
        attachments: &[BinaryResource<'_>],
    ) {
        if !self.external_enabled() && resources.is_empty() {
            return;
        }
        self.declarations.push_str(DECLARATIONS);
        self.declarations.push_str("define internal void @dever_external_append_cause(ptr %fault, ptr %message) {\nentry:\n  %cause = getelementptr %dever.fault, ptr %fault, i32 0, i32 8\n  call void @dever_rt_v1_external_cause_append(ptr %cause, ptr %message)\n  ret void\n}\n");
        let adapters = self.application.as_ref().unwrap().adapters.clone();
        for id in adapters {
            let adapter = self.program.adapters[id].clone();
            let AdapterImplementation::External(external) = &adapter.implementation else {
                continue;
            };
            let names = [
                format!("{}:{}", adapter.identity, adapter.name),
                external.ecosystem.keyword().to_owned(),
                external.entry.clone(),
                adapter.identity.clone(),
                adapter.name.clone(),
                external.schema.clone(),
            ];
            let metadata = names
                .iter()
                .map(|name| self.api_name(name))
                .collect::<Vec<_>>()
                .join(", ");
            let capabilities = self.wire_names(
                &external
                    .capabilities
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect::<Vec<_>>(),
            );
            let operations = adapter
                .operations
                .keys()
                .map(|port| {
                    self.program.functions[*port]
                        .port
                        .as_ref()
                        .unwrap()
                        .operation
                        .clone()
                })
                .collect::<Vec<_>>();
            let operation_names = self.wire_names(&operations);
            writeln!(self.declarations, "@dever_external_{id} = private constant %dever.external_definition {{ {metadata}, ptr {capabilities}, i64 {}, ptr {operation_names}, i64 {}, i64 30000 }}", external.capabilities.len(), operations.len()).unwrap();
        }
        if resources.is_empty() {
            return;
        }
        for resource in attachments {
            writeln!(
                self.declarations,
                "@{} = external constant [{} x i8]",
                resource.symbol,
                resource.bytes.len()
            )
            .unwrap();
        }
        let mut entries = Vec::new();
        for (resource, index) in resources.iter().zip(indices) {
            let path = self.api_name(&resource.path);
            let digest = self.api_name(&resource.sha256);
            entries.push(format!("%dever.external_resource {{ {path}, ptr @dever_external_bytes_{index}, i64 {}, {digest}, i8 {} }}", resource.bytes.len(), u8::from(resource.executable)));
        }
        writeln!(
            self.declarations,
            "@dever_external_resources = private constant [{} x %dever.external_resource] [{}]",
            resources.len(),
            entries.join(", ")
        )
        .unwrap();
        let digest = self.wire_names(&[crate::native::embedded_bundle_digest(resources)]);
        self.application.as_mut().unwrap().resources = Some((digest, resources.len()));
    }
}

impl FunctionEmitter<'_, '_> {
    fn external_wire_policy(&self) -> crate::wire::Policy {
        let function = &self.module.program.functions[self.instance.function];
        let operation = function.external.expect("external operation");
        let AdapterImplementation::External(external) =
            &self.module.program.adapters[operation.adapter].implementation
        else {
            unreachable!("external operation Adapter")
        };
        crate::wire::Policy::external(external.ecosystem)
    }

    pub(super) fn initialize_external_resources(&mut self, span: Span) {
        let Some((digest, count)) = self
            .module
            .application
            .as_ref()
            .and_then(|app| app.resources.clone())
        else {
            return;
        };
        self.wire_write_at(
            "dever_rt_v1_external_prepare",
            vec![
                format!("ptr {digest}"),
                "ptr @dever_external_resources".into(),
                format!("i64 {count}"),
            ],
            span,
        );
    }

    pub(super) fn start_external_adapter(
        &mut self,
        adapter: &crate::hir::Adapter,
        setting: &str,
        span: Span,
    ) {
        if !matches!(adapter.implementation, AdapterImplementation::External(_)) {
            return;
        }
        let id = self
            .module
            .program
            .adapters
            .iter()
            .position(|value| value.owner == adapter.owner)
            .unwrap();
        let operation = self.async_operation(
            "external_start",
            vec![
                format!("ptr @dever_external_{id}"),
                format!("ptr {setting}"),
            ],
            span,
        );
        self.await_operation(&operation, &Type::Unit, span, false);
    }

    pub(super) fn emit_external_body(
        &mut self,
        function: &crate::hir::Function,
    ) -> Result<bool, String> {
        let Some(external) = function.external else {
            return Ok(false);
        };
        let span = function.span;
        let adapter = self.module.program.adapters[external.adapter].clone();
        let contract = self.module.program.functions[external.contract]
            .port
            .clone()
            .unwrap();
        let payload = self.encode_external_inputs(span);
        let key = self.application_text(&format!("{}:{}", adapter.identity, adapter.name), span);
        let operation = self.application_text(&contract.operation, span);
        let (tag, identity, body) = self.call_external(&key, &operation, &payload, span);
        let success = self.label("external_result");
        let failure = self.label("external_failure");
        let ok = self.temp();
        self.line(format!("{ok} = icmp eq i8 {tag}, 0"));
        self.line(format!("br i1 {ok}, label %{success}, label %{failure}"));
        self.start(&success);
        self.return_external_result(&function.outputs, &body, span);
        self.start(&failure);
        self.fail_external(&contract.failures, &identity, &body, span)?;
        Ok(true)
    }

    fn encode_external_inputs(&mut self, span: Span) -> String {
        let fields = input_fields(self.module.program, self.instance.function);
        let input_ty = Type::Outputs(fields.clone());
        let mut input = "zeroinitializer".to_owned();
        for (index, field) in fields.iter().enumerate() {
            let value = self.temp();
            self.line(format!(
                "{value} = insertvalue {} {input}, {} {}, {index}",
                self.module.ty(&input_ty),
                self.module.ty(&field.ty),
                self.parameter(index)
            ));
            input = value;
        }
        let row = self.row_pointer(&input_ty, &input);
        let callback = self.module.wire_fields_callback(
            &fields,
            &format!("dever_external_inputs_{}", self.instance.function),
            self.external_wire_policy(),
            true,
        );
        let payload = self.runtime_call(
            "dever_rt_v1_wire_encode",
            vec![
                format!("ptr {row}"),
                format!("ptr @dever_type_{}", self.module.type_index(&input_ty)),
                format!("ptr @{callback}"),
            ],
            &Type::Text,
            false,
            None,
            span,
        );
        self.own_value(&Type::Text, &payload, false);
        payload
    }

    fn call_external(
        &mut self,
        key: &str,
        operation_name: &str,
        payload: &str,
        span: Span,
    ) -> (String, String, String) {
        let operation = self.async_operation(
            "external_call",
            vec![
                format!("ptr {key}"),
                format!("ptr {operation_name}"),
                format!("ptr {payload}"),
            ],
            span,
        );
        let (status, reply_row, _, error) = self.poll_raw_layout(&operation, "ptr");
        let status = self.poll_status(&status, &error, span);
        self.propagate_status(&status, span, false);
        let reply = self.temp();
        self.line(format!("{reply} = load ptr, ptr {reply_row}"));
        let kind = self.entry_slot_ir("i8");
        let identity = self.entry_slot(&Type::Text);
        let body = self.entry_slot(&Type::Text);
        self.wire_write_at(
            "dever_rt_v1_external_reply_take",
            vec![
                format!("ptr {reply}"),
                format!("ptr {kind}"),
                format!("ptr {identity}"),
                format!("ptr {body}"),
            ],
            span,
        );
        let identity_value = self.temp();
        let body_value = self.temp();
        self.line(format!("{identity_value} = load ptr, ptr {identity}"));
        self.line(format!("{body_value} = load ptr, ptr {body}"));
        self.own_value(&Type::Text, &identity_value, false);
        self.own_value(&Type::Text, &body_value, false);
        let tag = self.temp();
        self.line(format!("{tag} = load i8, ptr {kind}"));
        (tag, identity_value, body_value)
    }

    fn return_external_result(&mut self, fields: &[Field], body: &str, span: Span) {
        let value = self.decode_external_fields(fields, body, "outputs", span);
        let output = output_type(fields);
        let result = match fields.len() {
            0 => "zeroinitializer".into(),
            1 => {
                let result = self.temp();
                self.line(format!(
                    "{result} = extractvalue {} {value}, 0",
                    self.module.ty(&Type::Outputs(fields.to_vec()))
                ));
                result
            }
            _ => value,
        };
        self.line(format!(
            "store {} {result}, ptr %out",
            self.module.ty(&output)
        ));
        self.exit("0");
    }

    fn fail_external(
        &mut self,
        failures: &BTreeSet<crate::hir::Failure>,
        identity: &str,
        body: &str,
        span: Span,
    ) -> Result<(), String> {
        for declared in failures {
            let definition = self.module.program.types[declared.ty].clone();
            let Shape::Choice(variants) = &definition.shape else {
                unreachable!()
            };
            let variant = &variants[declared.variant];
            let expected =
                self.application_text(&format!("{}.{}", definition.name, variant.name), span);
            let matches = self.equals(&Type::Text, identity, &expected)?;
            let matched = self.label("external_declared_error");
            let next = self.label("external_next_error");
            self.line(format!("br i1 {matches}, label %{matched}, label %{next}"));
            self.start(&matched);
            let values = self.decode_external_fields(
                &variant.fields,
                body,
                &format!("error_{}_{}", declared.ty, declared.variant),
                span,
            );
            let row_ty = Type::Outputs(variant.fields.clone());
            let mut fields = Vec::new();
            for (index, field) in variant.fields.iter().enumerate() {
                let value = self.temp();
                self.line(format!(
                    "{value} = extractvalue {} {values}, {index}",
                    self.module.ty(&row_ty)
                ));
                fields.push((field.ty.clone(), value));
            }
            let payload = self.temp();
            self.line(format!(
                "{payload} = getelementptr %dever.fault, ptr %fault, i32 0, i32 7"
            ));
            self.write_variant(declared.ty, declared.variant, &fields, &payload);
            for (index, value) in [(5, declared.ty + 1), (6, declared.variant)] {
                let pointer = self.temp();
                self.line(format!(
                    "{pointer} = getelementptr %dever.fault, ptr %fault, i32 0, i32 {index}"
                ));
                self.line(format!("store i32 {value}, ptr {pointer}"));
            }
            self.write_fault(&(FaultCode::Business as i32).to_string(), span);
            self.start(&next);
        }
        self.application_fault(
            "external Adapter returned an undeclared business error",
            span,
        );
        Ok(())
    }

    /// The wire callback transfers a complete row; callers immediately move it
    /// into their output or fault, with no intervening fallible operation.
    fn decode_external_fields(
        &mut self,
        fields: &[Field],
        body: &str,
        suffix: &str,
        span: Span,
    ) -> String {
        let ty = Type::Outputs(fields.to_vec());
        let callback = self.module.wire_fields_callback(
            fields,
            &format!("dever_external_{suffix}_{}", self.instance.function),
            self.external_wire_policy(),
            false,
        );
        self.runtime_call(
            "dever_rt_v1_wire_decode",
            vec![
                format!("ptr {body}"),
                format!("ptr @dever_type_{}", self.module.type_index(&ty)),
                format!("ptr @{callback}"),
            ],
            &ty,
            false,
            None,
            span,
        )
    }
}
