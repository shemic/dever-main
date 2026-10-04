//! Static request adapters call concrete App functions inside the runtime request scope.
use super::*;

struct RouteDescriptor<'a> {
    method: &'a str,
    path: &'a str,
    directory: &'a [String],
    components: &'a [String],
    permission: Option<usize>,
    auth: Option<usize>,
    handler: &'a str,
    anonymous: bool,
    multipart: bool,
    detail: bool,
}

pub(super) const DECLARATIONS: &str = "\
%dever.api_handler = type { ptr, ptr, ptr, ptr, ptr, ptr }
%dever.api_auth = type { ptr, ptr, ptr, ptr, ptr, ptr }
%dever.api_permission = type { { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, { ptr, i64 } }
%dever.api_route = type { { ptr, i64 }, { ptr, i64 }, ptr, i64, ptr, i64, ptr, ptr, ptr, i8, i8, i8 }
declare i32 @dever_rt_v1_api_input_field(ptr, ptr, i8, i32, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_input_upload(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_input_raw_field(ptr, ptr, i8, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_text_bounds(ptr, ptr, i64, i64, i8, ptr)
declare i32 @dever_rt_v1_api_pagination(i64, i64, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_api_session_new(ptr, i64, ptr, i64, ptr, ptr)
declare i32 @dever_rt_v1_api_session_database(ptr, ptr, ptr)
declare void @dever_rt_v1_api_session_release(ptr)
declare i32 @dever_rt_v1_api_application_root(ptr, ptr, ptr, ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_api_initialize(ptr, ptr, ptr, i64, i8, ptr)
declare void @dever_rt_v1_api_cause_append(ptr, ptr)
declare ptr @dever_rt_v1_api_serve(ptr, ptr, i64, ptr, i64, ptr)
declare i32 @dever_rt_v1_api_has_tenant(ptr, ptr, ptr)
declare ptr @dever_rt_v1_api_tenant_scope(i64, ptr, ptr, ptr)
declare ptr @dever_rt_v1_api_require_components(ptr, ptr, i64, ptr, i64, ptr)
declare ptr @dever_rt_v1_api_tenant_owner(ptr, i64, ptr, i64, ptr)
declare ptr @dever_rt_v1_api_tenant_component(ptr, i64, ptr, i8, ptr, i64, ptr)
@dever_api_session = private global ptr null
";

impl Module<'_> {
    pub(super) fn api_enabled(&self) -> bool {
        if self
            .application
            .as_ref()
            .is_some_and(|app| app.test.is_some())
        {
            return self.jobs_enabled();
        }
        self.application.is_some()
            && (!self.program.jobs.is_empty()
                || !self.program.api_routes.is_empty()
                || !self.program.api_rest.is_empty()
                || !self.program.auth.is_empty()
                || !self.program.tenant_components.is_empty()
                || self.instances.iter().any(|instance| {
                    let mut scoped_capability = false;
                    crate::check::visit(&self.program.functions[instance.function], |expression| {
                        if let ExpressionKind::Intrinsic { operation, .. } = expression.kind {
                            // Logs read invocation Settings even without HTTP routes.
                            scoped_capability |= api_context::supported(operation)
                                || matches!(
                                    operation,
                                    Intrinsic::LogDebug
                                        | Intrinsic::LogInfo
                                        | Intrinsic::LogWarn
                                        | Intrinsic::LogError
                                );
                        }
                    });
                    scoped_capability
                }))
    }

    pub(super) fn api_name(&mut self, value: &str) -> String {
        let (pointer, length) = self.text_literal(value);
        format!("{{ ptr, i64 }} {{ ptr {pointer}, i64 {length} }}")
    }

    pub(super) fn emit_api_descriptors(&mut self) {
        if !self.database_enabled() {
            self.declarations
                .push_str("%dever.db_binding = type { ptr, { ptr, i64 }, i8 }\n");
        }
        self.declarations.push_str("define internal void @dever_api_append_cause(ptr %fault, ptr %message) {\nentry:\n  %cause = getelementptr %dever.fault, ptr %fault, i32 0, i32 8\n  call void @dever_rt_v1_api_cause_append(ptr %cause, ptr %message)\n  ret void\n}\n");
        self.api_fault_status(false);
        self.api_fault_status(true);
        self.rest_fault_message();
        for (index, hook) in self.program.auth.clone().iter().enumerate() {
            self.emit_auth_descriptor(index, hook);
        }
        let permissions = self.program.permissions.clone();
        let mut entries = Vec::new();
        for (index, permission) in permissions.iter().enumerate() {
            let fields = [
                &permission.key,
                &permission.component,
                &permission.domain,
                &permission.site,
                &permission.action,
                permission.method,
            ]
            .iter()
            .map(|value| self.api_name(value))
            .collect::<Vec<_>>()
            .join(", ");
            writeln!(
                self.declarations,
                "@dever_permission_{index} = private constant %dever.api_permission {{ {fields} }}"
            )
            .unwrap();
            entries.push(format!("%dever.api_permission {{ {fields} }}"));
        }
        writeln!(
            self.declarations,
            "@dever_permissions = private constant [{} x %dever.api_permission] [{}]",
            entries.len(),
            entries.join(", ")
        )
        .unwrap();
        let mut routes = Vec::new();
        for (index, route) in self.program.api_routes.clone().iter().enumerate() {
            let handler = self.api_handler(index);
            routes.push(self.api_route_value(RouteDescriptor {
                method: route.method,
                path: &route.path,
                directory: &route.directory,
                components: &route.components,
                permission: route.permission,
                auth: route.auth,
                handler: &handler,
                anonymous: route.anonymous,
                multipart: route.multipart,
                detail: false,
            }));
        }
        for (index, handler) in self
            .application
            .as_ref()
            .unwrap()
            .rest_handlers
            .clone()
            .iter()
            .enumerate()
        {
            let route = self.program.api_rest[handler.route].clone();
            let permission = match handler.method {
                "GET" => route.permissions.read,
                "POST" => route.permissions.create,
                "PUT" => route.permissions.replace,
                "DELETE" => route.permissions.delete,
                _ => unreachable!(),
            };
            let descriptor = self.rest_handler(index, handler);
            routes.push(self.api_route_value(RouteDescriptor {
                method: handler.method,
                path: &route.path,
                directory: &route.directory,
                components: &route.components,
                permission,
                auth: route.auth,
                handler: &descriptor,
                anonymous: false,
                multipart: false,
                detail: handler.detail,
            }));
        }
        writeln!(
            self.declarations,
            "@dever_api_routes = private constant [{} x %dever.api_route] [{}]",
            routes.len(),
            routes.join(", ")
        )
        .unwrap();
    }

    fn api_route_value(&mut self, route: RouteDescriptor<'_>) -> String {
        let RouteDescriptor {
            method,
            path,
            directory,
            components,
            permission,
            auth,
            handler,
            anonymous,
            multipart,
            detail,
        } = route;
        let method = self.api_name(method);
        let path = self.api_name(path);
        let names = self.wire_names(directory);
        let components_pointer = self.wire_names(components);
        let permission = permission.map_or_else(
            || "null".into(),
            |index| format!("@dever_permission_{index}"),
        );
        let auth = auth.map_or_else(|| "null".into(), |index| format!("@dever_api_auth_{index}"));
        format!(
            "%dever.api_route {{ {method}, {path}, ptr {names}, i64 {}, ptr {components_pointer}, i64 {}, ptr {permission}, ptr {auth}, ptr {handler}, i8 {}, i8 {}, i8 {} }}",
            directory.len(),
            components.len(),
            u8::from(anonymous),
            u8::from(multipart),
            u8::from(detail)
        )
    }

    fn emit_auth_descriptor(&mut self, index: usize, hook: &crate::hir::AuthHook) {
        let instance = Specialization {
            function: hook.function,
            handlers: Vec::new(),
        };
        let input = asynchronous::input_type(&self.program.functions[hook.function]);
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.asynchronous = false;
        emitter.protocol_callback = true;
        emitter.start("entry");
        let Shape::Record(fields) = emitter.module.program.types[hook.claims].shape.clone() else {
            unreachable!();
        };
        let mut record = "zeroinitializer".to_owned();
        for (field, pointer) in ["%subject", "%session", "%tenant", "%site"]
            .iter()
            .enumerate()
        {
            let value = if field == 2 {
                let present = emitter.temp();
                emitter.line(format!("{present} = icmp ne ptr {pointer}, null"));
                let flag = emitter.temp();
                let row = emitter.temp();
                emitter.line(format!(
                    "{flag} = insertvalue {{ i1, ptr }} zeroinitializer, i1 {present}, 0"
                ));
                emitter.line(format!(
                    "{row} = insertvalue {{ i1, ptr }} {flag}, ptr {pointer}, 1"
                ));
                row
            } else {
                pointer.to_string()
            };
            let row = emitter.temp();
            emitter.line(format!(
                "{row} = insertvalue %T{} {record}, {} {value}, {field}",
                hook.claims,
                emitter.module.ty(&fields[field].ty)
            ));
            record = row;
        }
        let source = emitter.row_pointer(&Type::Named(hook.claims), &record);
        emitter.line(format!(
            "call void @dever_clone_{}(ptr {source}, ptr %out)",
            emitter.module.type_index(&Type::Named(hook.claims))
        ));
        let callback = format!("dever_auth_pack_{index}");
        let body = emitter.finish_protocol_callback(
            &callback,
            "ptr %subject, ptr %session, ptr %tenant, ptr %site, ptr %out, ptr %error",
        );
        self.declarations.push_str(&body);
        // Claims is the only input field, so its layout equals the one-field
        // input tuple; all owned Text fields were cloned by the typed helper.
        let mut unpack = format!(
            "define internal i32 @dever_auth_unpack_{index}(ptr %input, ptr %id, ptr %user, ptr %user_present, ptr %tenant, ptr %tenant_present, ptr %error) {{\nentry:\n  %identity = load %T{}, ptr %input\n  %subject = extractvalue %T{} %identity, 0\n  %owned = call ptr @dever_rt_v1_text_retain(ptr %subject)\n  store ptr %owned, ptr %id\n",
            hook.identity, hook.identity
        );
        for (field, name) in [(1, "user"), (2, "tenant")] {
            writeln!(unpack, "  %{name}_value = extractvalue %T{} %identity, {field}\n  %{name}_flag = extractvalue {{ i1, i64 }} %{name}_value, 0\n  %{name}_number = extractvalue {{ i1, i64 }} %{name}_value, 1\n  %{name}_byte = zext i1 %{name}_flag to i8\n  store i64 %{name}_number, ptr %{name}\n  store i8 %{name}_byte, ptr %{name}_present", hook.identity).unwrap();
        }
        unpack.push_str("  store { ptr, i64 } zeroinitializer, ptr %error\n  ret i32 0\n}\n");
        self.declarations.push_str(&unpack);
        let name = &self.names[&instance];
        let (asynchronous, synchronous) = if specialize::suspends(self.program, &instance) {
            (format!("@dever_async_{name}"), "null".into())
        } else {
            ("null".into(), format!("@dever_sync_{name}"))
        };
        writeln!(self.declarations, "@dever_api_auth_{index} = private constant %dever.api_auth {{ ptr {asynchronous}, ptr {synchronous}, ptr @dever_owned_{}, ptr @{callback}, ptr @dever_auth_unpack_{index}, ptr @dever_api_fault_status }}", self.type_index(&input)).unwrap();
    }
    pub(super) fn emit_api_body(
        &mut self,
        instance: &Specialization,
        index: usize,
    ) -> Result<(), String> {
        let route = self.program.api_routes[index].clone();
        let wrapper = self.program.functions[instance.function].clone();
        let app = self.program.functions[route.function].clone();
        let target = Specialization {
            function: route.function,
            handlers: Vec::new(),
        };
        let mut emitter = FunctionEmitter::new(self, instance);
        let parameters = emitter.begin_generated_function(&wrapper);
        let password = [
            Intrinsic::CryptoPasswordHash,
            Intrinsic::CryptoPasswordVerify,
        ]
        .iter()
        .any(|operation| specialize::uses_intrinsic(emitter.module.program, &target, *operation));
        let transaction = !password && emitter.begin_command_transaction(&target, route.span);
        let arguments = app
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                (
                    parameter.value_type().unwrap().clone(),
                    emitter.parameter(index),
                )
            })
            .collect::<Vec<_>>();
        let output = output_type(&app.outputs);
        let (status, row) = emitter.invoke_values(&target, &arguments, &output, route.span);
        emitter.propagate_status(&status, route.span, false);
        let value = emitter.temp();
        emitter.line(format!(
            "{value} = load {}, ptr {row}",
            emitter.module.ty(&output)
        ));
        emitter.own_value(&output, &value, false);
        let encoded = emitter.encode_api_output(&output, &row, route.span);
        let guard = emitter.own_value(&Type::Text, &encoded, false);
        if transaction {
            emitter.commit_command_transaction();
        }
        emitter.line(format!("store ptr {encoded}, ptr %out"));
        emitter.disarm(guard);
        emitter.exit("0");
        let name = emitter.module.names[instance].clone();
        let body = emitter.finish_function(&name, &parameters);
        self.functions.push_str(&body);
        Ok(())
    }

    pub(super) fn api_fault_status(&mut self, rest: bool) -> String {
        let name = if rest {
            "dever_rest_fault_status"
        } else {
            "dever_api_fault_status"
        };
        let mut cases = Vec::new();
        for (id, definition) in self.program.types.iter().enumerate() {
            let Shape::Choice(variants) = &definition.shape else {
                continue;
            };
            for (index, variant) in variants.iter().enumerate() {
                let status = match (definition.name.as_str(), variant.name.as_str()) {
                    ("dever.api.Error", "Invalid") => 400,
                    ("dever.api.Error", "Unauthorized") => 401,
                    ("dever.api.Error", "Forbidden") => 403,
                    ("dever.api.Error", "NotFound") => 404,
                    ("dever.api.Error", "Conflict") => 409,
                    ("dever.api.Error", "TooManyRequests") => 429,
                    ("dever.database.Error", "NotFound") if rest => 404,
                    ("<generated REST Error>", "Invalid") => 400,
                    ("<generated REST Error>", "Forbidden") => 403,
                    ("<generated REST Error>", "NotFound") => 404,
                    _ => continue,
                };
                cases.push((id + 1, index, status));
            }
        }
        let mut body = format!(
            "define internal i64 @{name}(ptr %fault) {{\nentry:\n  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0\n  %code = load i32, ptr %code_ptr\n  %business = icmp eq i32 %code, 4\n  br i1 %business, label %identity, label %internal\nidentity:\n  %type_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 5\n  %type = load i32, ptr %type_ptr\n  %variant_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 6\n  %variant = load i32, ptr %variant_ptr\n"
        );
        for (index, (id, variant, status)) in cases.iter().enumerate() {
            writeln!(body, "  %type{index} = icmp eq i32 %type, {id}\n  %variant{index} = icmp eq i32 %variant, {variant}\n  %matches{index} = and i1 %type{index}, %variant{index}\n  br i1 %matches{index}, label %status{index}, label %next{index}\nstatus{index}:\n  ret i64 {status}\nnext{index}:").unwrap();
        }
        body.push_str("  br label %internal\ninternal:\n  ret i64 500\n}\n");
        self.declarations.push_str(&body);
        format!("@{name}")
    }

    pub(super) fn api_handler(&mut self, index: usize) -> String {
        let route = self.program.api_routes[index].clone();
        let function = self.application.as_ref().unwrap().api_handlers[index];
        let instance = Specialization {
            function,
            handlers: Vec::new(),
        };
        let input = asynchronous::input_type(&self.program.functions[function]);
        let parameters = self.program.functions[function].parameters.clone();
        let fields = route
            .inputs
            .iter()
            .zip(parameters)
            .map(|(name, parameter)| {
                let ty = parameter.value_type().unwrap().clone();
                let callback = if ty == Type::Upload {
                    String::new()
                } else {
                    self.wire_callback(&ty, crate::wire::Policy::ApiInput, false)
                };
                (name.clone(), ty, callback)
            })
            .collect::<Vec<_>>();
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.asynchronous = false;
        emitter.protocol_callback = true;
        emitter.start("entry");
        let value = emitter.decode_fields(&input, &fields, "%inputs", wire::FieldInput::Api);
        emitter.line(format!(
            "store {} {value}, ptr %out",
            emitter.module.ty(&input)
        ));
        let callback = format!("dever_api_decode_{index}");
        let body = emitter.finish_protocol_callback(
            &callback,
            "ptr %inputs, i64 %detail_id, ptr %out, ptr %error",
        );
        self.declarations.push_str(&body);
        let name = &self.names[&instance];
        let descriptor = format!("@dever_api_handler_{index}");
        writeln!(self.declarations, "{descriptor} = private constant %dever.api_handler {{ ptr @dever_async_{name}, ptr null, ptr @dever_owned_{}, ptr @{callback}, ptr @dever_api_fault_status, ptr null }}", self.type_index(&input)).unwrap();
        descriptor
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn initialize_api(
        &mut self,
        bindings: &str,
        binding_count: usize,
        transactions: &str,
        transaction_count: usize,
        service_start: &str,
        span: Span,
    ) -> String {
        let session = self.protocol_call(
            "dever_rt_v1_api_session_new",
            vec![
                format!("ptr {bindings}"),
                format!("i64 {binding_count}"),
                format!("ptr {transactions}"),
                format!("i64 {transaction_count}"),
            ],
            "ptr",
            span,
        );
        self.line(format!("store ptr {session}, ptr @dever_api_session"));
        if self.module.jobs_enabled() && self.module.application.as_ref().unwrap().test.is_none() {
            let configure = self.label("application_configure_services");
            let ready = self.label("application_resources_ready");
            self.line(format!(
                "br i1 {service_start}, label %{configure}, label %{ready}"
            ));
            self.start(&configure);
            self.wire_write(
                "dever_rt_v1_job_configure",
                vec![
                    format!("ptr {session}"),
                    format!(
                        "i8 {}",
                        u8::from(
                            !self.module.program.api_routes.is_empty()
                                || !self.module.program.api_rest.is_empty()
                        )
                    ),
                    "i8 1".into(),
                ],
            );
            self.line(format!("br label %{ready}"));
            self.start(&ready);
        }
        self.register_jobs(span);
        let tenant_models = self
            .module
            .program
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| model.scope == crate::model::ModelScope::Tenant)
            .collect::<Vec<_>>();
        let fingerprint =
            crate::native::orm::tenant_fingerprint(&tenant_models, self.module.program);
        let fingerprint = self.module.wire_names(&[fingerprint]);
        let service = self.temp();
        self.line(format!("{service} = zext i1 {service_start} to i8"));
        let operation = self.async_operation(
            "api_initialize",
            vec![
                format!("ptr {session}"),
                format!("ptr {fingerprint}"),
                "ptr @dever_permissions".into(),
                format!("i64 {}", self.module.program.permissions.len()),
                format!("i8 {service}"),
            ],
            span,
        );
        self.await_operation(&operation, &Type::Unit, span, true);
        session
    }

    pub(super) fn serve_api(&mut self, span: Span) {
        let session = self.temp();
        self.line(format!("{session} = load ptr, ptr @dever_api_session"));
        let components = self
            .module
            .wire_names(&self.module.program.tenant_components.clone());
        let mut arguments = vec![
            format!("ptr {session}"),
            "ptr @dever_api_routes".into(),
            format!(
                "i64 {}",
                self.module.program.api_routes.len()
                    + self
                        .module
                        .application
                        .as_ref()
                        .unwrap()
                        .rest_handlers
                        .len()
            ),
            format!("ptr {components}"),
            format!("i64 {}", self.module.program.tenant_components.len()),
        ];
        let symbol = if self.module.jobs_enabled() {
            let jobs = self.temp();
            self.line(format!("{jobs} = load ptr, ptr @dever_job_session"));
            arguments.push(format!("ptr {jobs}"));
            "api_serve_with_jobs"
        } else {
            "api_serve"
        };
        let operation = self.async_operation(symbol, arguments, span);
        self.await_operation(&operation, &Type::Unit, span, true);
    }

    pub(super) fn begin_generated_function(&mut self, function: &crate::hir::Function) -> String {
        let mut parameters = function
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                format!(
                    "{} %p{index}, ",
                    self.module.ty(parameter.value_type().unwrap())
                )
            })
            .collect::<String>();
        if self.module.database_function(self.instance) {
            parameters.push_str("ptr %database, ");
            self.database_context = "%database".into();
        }
        self.start("entry");
        self.prologue
            .push_str("  %exit_status = alloca i32\n  store i32 0, ptr %exit_status\n");
        self.begin_coroutine(function);
        parameters
    }

    pub(super) fn encode_api_output(&mut self, ty: &Type, row: &str, span: Span) -> String {
        let encoder = self
            .module
            .wire_callback(ty, crate::wire::Policy::Output, true);
        self.runtime_call(
            "dever_rt_v1_wire_encode",
            vec![
                format!("ptr {row}"),
                format!("ptr @dever_type_{}", self.module.type_index(ty)),
                format!("ptr @{encoder}"),
            ],
            &Type::Text,
            false,
            None,
            span,
        )
    }
}
