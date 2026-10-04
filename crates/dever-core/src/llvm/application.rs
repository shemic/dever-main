//! Compiler-owned application roots. Settings live until the root Scope drains.
use super::*;

/// Textual code and borrowed binary initializers form one compilation input.
pub struct ResourceModule<'a> {
    pub ir: String,
    pub resources: Vec<BinaryResource<'a>>,
}

pub struct BinaryResource<'a> {
    pub symbol: String,
    pub bytes: &'a [u8],
}

#[derive(Clone)]
pub(super) struct Application {
    pub root: usize,
    pub ports: Vec<usize>,
    pub adapters: Vec<usize>,
    pub api_handlers: Vec<usize>,
    pub command_handlers: Vec<usize>,
    pub rest_handlers: Vec<rest::Handler>,
    pub rest_error: Option<usize>,
    pub job_handlers: Vec<usize>,
    pub test: Option<usize>,
    pub resources: Option<(String, usize)>,
    pub executable: bool,
}

/// Emit the checked production application entry, independently of ordinary kernels.
/// The caller supplies Unit output and the module's fault slot, then releases
/// both using the same generated ownership exports as other LLVM entries.
pub fn emit_application(program: &Program, sources: &SourceMap) -> Result<String, String> {
    emit_application_with_resources(program, sources, &[]).map(|module| module.ir)
}

/// Embed already prepared Worker resources; registry resolution stays with the packager.
pub fn emit_application_with_resources<'a>(
    program: &Program,
    sources: &SourceMap,
    resources: &'a [crate::native::EmbeddedResource],
) -> Result<ResourceModule<'a>, String> {
    emit_program_with_resources(program, sources, None, resources, false)
}

/// Emit a complete production executable with the C ABI process entry.
pub fn emit_executable(program: &Program, sources: &SourceMap) -> Result<String, String> {
    emit_executable_with_resources(program, sources, &[]).map(|module| module.ir)
}

/// The same application entry with already verified, immutable Worker resources.
pub fn emit_executable_with_resources<'a>(
    program: &Program,
    sources: &SourceMap,
    resources: &'a [crate::native::EmbeddedResource],
) -> Result<ResourceModule<'a>, String> {
    emit_program_with_resources(program, sources, None, resources, true)
}

pub(super) fn emit_program(
    program: &Program,
    sources: &SourceMap,
    test: Option<usize>,
) -> Result<String, String> {
    emit_program_with_resources(program, sources, test, &[], false).map(|module| module.ir)
}

fn emit_program_with_resources<'a>(
    program: &Program,
    sources: &SourceMap,
    test: Option<usize>,
    resources: &'a [crate::native::EmbeddedResource],
    executable: bool,
) -> Result<ResourceModule<'a>, String> {
    crate::native::validate_embedded_resources(resources)?;
    validate_http_bindings(program, sources)?;
    let span = test
        .map(|index| program.tests[index].span)
        .or_else(|| {
            program
                .api_routes
                .first()
                .map(|route| route.span)
                .or_else(|| program.api_rest.first().map(|route| route.span))
                .or_else(|| {
                    program
                        .api_commands
                        .first()
                        .map(|command| program.functions[command.function].span)
                })
                .or_else(|| {
                    program
                        .jobs
                        .first()
                        .map(|job| program.functions[job.function].span)
                })
        })
        .ok_or_else(|| "LLVM application has no API, REST, CMD, or Job entry".to_owned())?;
    let mut checked = program.clone();
    let (rest_handlers, rest_error) = rest::prepare(&mut checked);
    let roots = program
        .api_commands
        .iter()
        .map(|command| command.function)
        .chain(program.api_routes.iter().map(|route| route.function))
        .chain(program.auth.iter().map(|hook| hook.function))
        .chain(rest_handlers.iter().map(|handler| handler.function))
        .chain(
            program
                .jobs
                .iter()
                .filter(|_| test.is_none())
                .map(|job| job.function),
        )
        .chain(test.map(|index| program.tests[index].function))
        .map(|function| Specialization {
            function,
            handlers: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut instances = specialize::execution_reachable(&checked.functions, roots)
        .map_err(|span| located(sources, span, "recursive specialized call"))?;
    checked.jobs.retain(|job| {
        instances
            .iter()
            .any(|instance| instance.function == job.function)
    });
    for instance in &instances {
        if instance.function < program.functions.len() {
            validate_function(&checked, sources, instance, true)?;
        }
    }
    let ports = instances
        .iter()
        .filter_map(|instance| {
            checked.functions[instance.function]
                .port
                .as_ref()
                .map(|_| instance.function)
        })
        .collect::<Vec<_>>();
    let owners = ports
        .iter()
        .map(|port| program.functions[*port].owner)
        .collect::<BTreeSet<_>>();
    let adapters = program
        .adapters
        .iter()
        .enumerate()
        .filter_map(|(id, adapter)| {
            (test.is_none() && !adapter.fake && owners.contains(&adapter.port_owner)).then_some(id)
        })
        .collect::<Vec<_>>();
    for owner in owners {
        if test.is_some() {
            continue;
        }
        if !adapters
            .iter()
            .any(|id| program.adapters[*id].port_owner == owner)
        {
            return Err(located(
                sources,
                span,
                "reachable Port has no production Adapter",
            ));
        }
    }
    // The synthetic root has no source-callable identity. Its body is generated
    // below; the existing descriptors still own all concrete entry callees.
    let mut command_handlers = Vec::new();
    for (index, command) in program.api_commands.iter().enumerate() {
        let original = &program.functions[command.function];
        let mut function = generated_function(format!("<generated CMD {index}>"), original.span);
        function.parameters = vec![Parameter::Value(Type::Text)];
        let id = append_handler(&mut checked, command.function, function);
        command_handlers.push(id);
        instances.insert(Specialization {
            function: id,
            handlers: Vec::new(),
        });
    }
    let mut api_handlers = Vec::new();
    for (index, route) in program.api_routes.iter().enumerate() {
        let original = &program.functions[route.function];
        let mut function = generated_function(format!("<generated API {index}>"), route.span);
        function.parameters = original.parameters.clone();
        function.outputs = vec![crate::types::Field {
            name: "encoded".into(),
            ty: Type::Text,
            private: false,
            bounds: Vec::new(),
        }];
        let id = append_handler(&mut checked, route.function, function);
        api_handlers.push(id);
        instances.insert(Specialization {
            function: id,
            handlers: Vec::new(),
        });
    }
    let mut job_handlers = Vec::new();
    for job in checked.jobs.clone() {
        let original = checked.functions[job.function].clone();
        let mut function =
            generated_function(format!("<generated Job {}>", job.function), original.span);
        function.parameters = original.parameters.clone();
        let id = append_handler(&mut checked, job.function, function);
        job_handlers.push(id);
        instances.insert(Specialization {
            function: id,
            handlers: Vec::new(),
        });
    }
    let root = append_generated(
        &mut checked,
        generated_function("<generated application entry>".into(), span),
    );
    checked.failures[root] = instances
        .iter()
        .flat_map(|instance| specialize::failures(&checked, instance))
        .collect();
    let database = test.map_or(
        !checked.models.is_empty() || !checked.jobs.is_empty(),
        |index| checked.tests[index].uses_database(),
    );
    if database {
        checked.failures[root].extend(crate::capture::database_errors(&checked.types));
    }
    let root_instance = Specialization {
        function: root,
        handlers: Vec::new(),
    };
    instances.insert(root_instance.clone());
    let mut module = Module::new(&checked, sources, instances);
    module.collect_external_types();
    for handler in &rest_handlers {
        let decoded = Type::Outputs(
            handler.inputs[usize::from(handler.detail)..]
                .iter()
                .enumerate()
                .map(|(index, input)| crate::types::Field {
                    name: format!("p{index}"),
                    ty: input.ty.clone(),
                    private: false,
                    bounds: Vec::new(),
                })
                .collect(),
        );
        collect_owned_types(&checked, &decoded, &mut module.value_types);
    }
    for model in checked.models.iter().filter(|_| database) {
        for id in [model.record, model.page, model.cursor]
            .into_iter()
            .chain(model.sql.iter().map(|sql| sql.result.record))
        {
            collect_types(&checked, &Type::Named(id), &mut module.type_ids);
            collect_owned_types(&checked, &Type::Named(id), &mut module.value_types);
        }
        for ty in [
            Type::List(Box::new(Type::Named(model.record))),
            Type::Nullable(Box::new(Type::Named(model.record))),
            Type::Map(
                Box::new(Type::Int),
                Box::new(Type::List(Box::new(Type::Named(model.record)))),
            ),
        ] {
            collect_owned_types(&checked, &ty, &mut module.value_types);
        }
    }
    for ty in [
        Type::Text,
        Type::Json,
        Type::Bool,
        Type::Int,
        Type::List(Box::new(Type::Text)),
        Type::Nullable(Box::new(Type::Text)),
    ] {
        collect_owned_types(&checked, &ty, &mut module.value_types);
    }
    for adapter in &adapters {
        if let Some(id) = checked.adapters[*adapter].setting {
            let ty = Type::Named(id);
            collect_types(&checked, &ty, &mut module.type_ids);
            collect_owned_types(&checked, &ty, &mut module.value_types);
        }
    }
    module.application = Some(Application {
        root,
        ports,
        adapters,
        api_handlers,
        command_handlers,
        rest_handlers,
        rest_error,
        job_handlers,
        test,
        resources: None,
        executable,
    });
    module.validate_async_boundaries(sources)?;
    module.emit_types()?;
    module.declarations.push_str(wire::DECLARATIONS);
    module.declarations.push_str(ports::DECLARATIONS);
    let inputs = crate::native::resource_inputs(resources);
    let attachments = inputs
        .unique
        .iter()
        .enumerate()
        .map(|(index, resource)| BinaryResource {
            symbol: format!("dever_external_bytes_{index}"),
            bytes: resource.bytes.as_slice(),
        })
        .collect::<Vec<_>>();
    module.emit_external_descriptors(resources, &inputs.indices, &attachments);
    if module.api_enabled() {
        module.declarations.push_str(api::DECLARATIONS);
        module.declarations.push_str(api_context::DECLARATIONS);
        module.emit_api_descriptors();
    }
    if module.database_enabled() {
        module.declarations.push_str(database::DECLARATIONS);
        module.emit_database_slots();
    }
    module.emit_job_descriptors();
    module.declarations.push_str("declare i32 @dever_rt_v1_text_copy_utf8(ptr, i64, ptr)\ndeclare i32 @dever_rt_v1_text_export(ptr, ptr)\n");
    module.emit_port_slots();
    module.emit_functions()?;
    module.emit_named_entry(&root_instance, "dever_application_run", true);
    module.emit_application_entry(&root_instance);
    if test.is_some() {
        module.emit_test_exports(&root_instance);
    }
    if executable {
        module.emit_process_entry(&root_instance);
    }
    Ok(ResourceModule {
        ir: module.finish(),
        resources: attachments,
    })
}

fn validate_http_bindings(program: &Program, sources: &SourceMap) -> Result<(), String> {
    for route in &program.api_routes {
        if route.auth.is_none() || (!route.anonymous && route.permission.is_none()) {
            return Err(located(
                sources,
                route.span,
                "LLVM API application requires a configured site, auth hook, and protected-route permission; check the application with settings",
            ));
        }
    }
    for route in &program.api_rest {
        if route.auth.is_none()
            || [
                route.permissions.read,
                route.permissions.create,
                route.permissions.replace,
                route.permissions.delete,
            ]
            .iter()
            .any(Option::is_none)
        {
            return Err(located(
                sources,
                route.span,
                "LLVM REST application requires a configured site, auth hook, and all four permissions; check the application with settings",
            ));
        }
    }
    Ok(())
}

pub(super) fn generated_function(name: String, span: Span) -> crate::hir::Function {
    crate::hir::Function {
        name,
        owner: 0,
        public: false,
        parameters: Vec::new(),
        outputs: Vec::new(),
        clauses: Vec::new(),
        span,
        suspends: true,
        pure: false,
        recovery: None,
        kind: FunctionKind::Ordinary,
        port: None,
        implementation: false,
        setting: None,
        external: None,
    }
}

pub(super) fn append_generated(program: &mut Program, function: crate::hir::Function) -> usize {
    let id = program.functions.len();
    program.functions.push(function);
    program.effects.push(BTreeSet::new());
    program.recoveries.push(BTreeSet::new());
    program.suspension_handlers.push(BTreeSet::new());
    program.failure_handlers.push(BTreeSet::new());
    program.database_effects.push(BTreeSet::new());
    program.failures.push(BTreeSet::new());
    id
}

fn append_handler(
    program: &mut Program,
    target: usize,
    mut function: crate::hir::Function,
) -> usize {
    let original = &program.functions[target];
    function.owner = original.owner;
    // Generated bodies keep their original checked callee's transitive
    // Send, failure, and database effects through this analysis-only edge.
    function.clauses = generated_call_edge(original, target);
    let id = append_generated(program, function);
    program.effects[id] = program.effects[target].clone();
    program.failures[id] = program.failures[target].clone();
    program.database_effects[id] = program.database_effects[target].clone();
    id
}

fn generated_call_edge(function: &crate::hir::Function, target: usize) -> Vec<crate::hir::Clause> {
    vec![crate::hir::Clause {
        patterns: Vec::new(),
        bindings: Vec::new(),
        locals: Vec::new(),
        outputs: Vec::new(),
        terminates: false,
        body: vec![Statement::Call(Expression {
            kind: ExpressionKind::Call {
                target: CallTarget::Function(target),
                arguments: function
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(index, parameter)| {
                        CallArgument::Value(Expression {
                            kind: ExpressionKind::Local(index),
                            ty: parameter.value_type().expect("entry value input").clone(),
                            span: function.span,
                        })
                    })
                    .collect(),
            },
            ty: output_type(&function.outputs),
            span: function.span,
        })],
    }]
}

impl Module<'_> {
    pub(super) fn emit_command_body(
        &mut self,
        instance: &Specialization,
        index: usize,
    ) -> Result<(), String> {
        let function = self.program.functions[instance.function].clone();
        let command = self.program.api_commands[index].clone();
        let mut emitter = FunctionEmitter::new(self, instance);
        let parameters = emitter.begin_generated_function(&function);
        let body = emitter.parameter(0);
        emitter.emit_command(&command, index, &body)?;
        let name = emitter.module.names[instance].clone();
        let body = emitter.finish_function(&name, &parameters);
        self.functions.push_str(&body);
        Ok(())
    }
    pub(super) fn emit_application_body(
        &mut self,
        instance: &Specialization,
    ) -> Result<(), String> {
        let function = self.program.functions[instance.function].clone();
        let commands = self.program.api_commands.clone();
        let mut emitter = FunctionEmitter::new(self, instance);
        emitter.start("entry");
        emitter
            .prologue
            .push_str("  %exit_status = alloca i32\n  store i32 0, ptr %exit_status\n");
        emitter.begin_coroutine(&function);
        let span = function.span;
        emitter.initialize_clock(span);
        emitter.initialize_external_resources(span);
        let arguments_ty = Type::List(Box::new(Type::Text));
        let arguments = emitter.runtime_call(
            "dever_rt_v1_process_arguments",
            vec![format!(
                "ptr @dever_type_{}",
                emitter.module.type_index(&Type::Text)
            )],
            &arguments_ty,
            false,
            None,
            span,
        );
        emitter.own_value(&arguments_ty, &arguments, false);
        let length = emitter.runtime_call(
            "dever_rt_v1_list_length",
            vec![format!("ptr {arguments}")],
            &Type::Int,
            false,
            None,
            span,
        );
        let empty = emitter.temp();
        emitter.line(format!("{empty} = icmp eq i64 {length}, 0"));
        let migration = emitter.initialize_database(span, &empty);
        emitter.line(format!(
            "br i1 {empty}, label %command_empty, label %command_check_arity"
        ));
        emitter.start("command_empty");
        emitter.initialize_ports(span);
        if !emitter.module.program.api_routes.is_empty()
            || !emitter.module.program.api_rest.is_empty()
            || emitter.module.jobs_enabled()
        {
            emitter.serve_api(span);
            emitter.exit("0");
        } else {
            emitter.application_fault(
                "application has no service entry; select a CMD and JSON input",
                span,
            );
        }
        emitter.start("command_check_arity");
        let retained = emitter.temp();
        emitter.line(format!(
            "{retained} = call ptr @dever_rt_v1_list_retain(ptr {arguments})"
        ));
        let cursor = emitter.protocol_call(
            "dever_rt_v1_list_cursor_take",
            vec![format!("ptr {retained}")],
            "ptr",
            span,
        );
        let cursor_slot = emitter.entry_slot_ir("ptr");
        emitter.line(format!("store ptr {cursor}, ptr {cursor_slot}"));
        let guard = emitter.register_owned(OwnedKind::ListCursor, cursor_slot);
        emitter.mark_live(guard);
        let first = emitter.application_argument(&cursor, span);
        let (name, body, tenant) = emitter.application_command_arguments(
            &first,
            &cursor,
            &length,
            migration.as_ref(),
            span,
        )?;
        emitter.release_guard(guard);
        emitter.initialize_ports(span);
        for (index, command) in commands.iter().enumerate() {
            let command_name = emitter.application_text(&command.name, span);
            let equal = emitter.equals(&Type::Text, &name, &command_name)?;
            emitter.line(format!(
                "br i1 {equal}, label %command_{index}, label %command_next_{index}"
            ));
            emitter.start(&format!("command_{index}"));
            let function = emitter
                .module
                .application
                .as_ref()
                .unwrap()
                .command_handlers[index];
            let target = Specialization {
                function,
                handlers: Vec::new(),
            };
            emitter.application_invoke_command(command, &target, &body, &tenant, span);
            emitter.exit("0");
            emitter.start(&format!("command_next_{index}"));
        }
        let prefix = emitter.application_text("unknown CMD '", span);
        let suffix = emitter.application_text("'", span);
        let message = emitter.application_concat(&prefix, &name, span);
        let message = emitter.application_concat(&message, &suffix, span);
        emitter.application_fault_text(&message, span);
        let name = emitter.module.names[instance].clone();
        let body = emitter.finish_function(&name, "");
        self.functions.push_str(&body);
        Ok(())
    }

    fn emit_application_entry(&mut self, root: &Specialization) {
        let app = self.application.clone().unwrap();
        let gated = !app.ports.is_empty()
            || self.database_enabled()
            || self.api_enabled()
            || self.clock_enabled();
        let span = self.program.functions[root.function].span;
        self.functions.push_str("define i32 @dever_application_entry(ptr %out, ptr %fault) {\nentry:\n  store {} zeroinitializer, ptr %out\n  store %dever.fault zeroinitializer, ptr %fault\n");
        if gated {
            self.declarations
                .push_str("@dever_application_active = private global i8 0\n");
            let (message, length) = self.text_literal("application entry is already active");
            let location = self.location(span);
            writeln!(self.functions, "  %gate = cmpxchg ptr @dever_application_active, i8 0, i8 1 acq_rel acquire\n  %entered = extractvalue {{ i8, i1 }} %gate, 1\n  br i1 %entered, label %run, label %busy\nbusy:\n  %message = getelementptr %dever.fault, ptr %fault, i32 0, i32 4\n  %copied = call i32 @dever_rt_v1_text_copy_utf8(ptr {message}, i64 {length}, ptr %message)\n  %code = getelementptr %dever.fault, ptr %fault, i32 0, i32 0\n  store i32 3, ptr %code\n  %origin = getelementptr %dever.fault, ptr %fault, i32 0, i32 1\n  store i32 {location}, ptr %origin\n  ret i32 1\nrun:").unwrap();
        }
        self.functions
            .push_str("  %status = call i32 @dever_application_run(ptr %out, ptr %fault)\n");
        // async_root has drained every child before these immutable globals can
        // be destroyed or another entry can publish a new Adapter selection.
        for adapter in app.adapters {
            let adapter = &self.program.adapters[adapter];
            if let Some(id) = adapter.setting {
                let owner = adapter.owner;
                let ty = self.type_index(&Type::Named(id));
                writeln!(self.functions, "  %live{owner} = load i1, ptr @dever_setting_live_{owner}\n  br i1 %live{owner}, label %drop_setting{owner}, label %setting_done{owner}\ndrop_setting{owner}:\n  call void @dever_drop_{ty}(ptr @dever_setting_{owner})\n  store %T{id} zeroinitializer, ptr @dever_setting_{owner}\n  store i1 false, ptr @dever_setting_live_{owner}\n  br label %setting_done{owner}\nsetting_done{owner}:").unwrap();
            }
        }
        for port in app.ports {
            writeln!(self.functions, "  store i64 0, ptr @dever_port_{port}").unwrap();
        }
        if self.clock_enabled() {
            self.functions.push_str("  %job_clock = load ptr, ptr @dever_job_clock\n  call void @dever_rt_v1_job_clock_release(ptr %job_clock)\n  store ptr null, ptr @dever_job_clock\n");
        }
        if self.jobs_enabled() {
            self.functions.push_str("  %jobs = load ptr, ptr @dever_job_session\n  call void @dever_rt_v1_job_session_release(ptr %jobs)\n  store ptr null, ptr @dever_job_session\n");
        }
        if self.database_enabled() {
            for index in 0..self.program.models.len() {
                writeln!(self.functions, "  %database{index} = load ptr, ptr @dever_database_{index}\n  call void @dever_rt_v1_db_release(ptr %database{index})\n  store ptr null, ptr @dever_database_{index}").unwrap();
            }
            self.functions.push_str("  %session = load ptr, ptr @dever_database_session\n  call void @dever_rt_v1_db_session_release(ptr %session)\n  store ptr null, ptr @dever_database_session\n");
        }
        if self.api_enabled() {
            self.functions.push_str("  %api_session = load ptr, ptr @dever_api_session\n  call void @dever_rt_v1_api_session_release(ptr %api_session)\n  store ptr null, ptr @dever_api_session\n");
        }
        if gated {
            self.functions
                .push_str("  store atomic i8 0, ptr @dever_application_active release, align 1\n");
        }
        self.functions.push_str("  ret i32 %status\n}\n");
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn application_text(&mut self, text: &str, span: Span) -> String {
        let (pointer, length) = self.module.text_literal(text);
        let value = self.runtime_call(
            "dever_rt_v1_text_new",
            vec![format!("ptr {pointer}"), format!("i64 {length}")],
            &Type::Text,
            false,
            None,
            span,
        );
        self.own_value(&Type::Text, &value, false);
        value
    }

    fn application_concat(&mut self, left: &str, right: &str, span: Span) -> String {
        let value = self.runtime_call(
            "dever_rt_v1_text_concat",
            vec![format!("ptr {left}"), format!("ptr {right}")],
            &Type::Text,
            false,
            None,
            span,
        );
        self.own_value(&Type::Text, &value, false);
        value
    }

    pub(super) fn application_fault(&mut self, message: &str, span: Span) {
        let message = self.application_text(message, span);
        self.application_fault_text(&message, span);
    }

    pub(super) fn application_fault_text(&mut self, message: &str, span: Span) {
        let buffer = self.entry_slot_ir("{ ptr, i64 }");
        let status = self.temp();
        self.line(format!(
            "{status} = call i32 @dever_rt_v1_text_export(ptr {message}, ptr {buffer})"
        ));
        self.check_runtime_status(&status, &buffer, span);
        self.check_runtime_status("1", &buffer, span);
        self.line("unreachable");
    }

    pub(super) fn application_argument(&mut self, cursor: &str, span: Span) -> String {
        let value = self.runtime_call(
            "dever_rt_v1_list_cursor_next",
            vec![format!("ptr {cursor}")],
            &Type::Nullable(Box::new(Type::Text)),
            true,
            None,
            span,
        );
        let argument = self.temp();
        self.line(format!(
            "{argument} = extractvalue {{ i1, ptr }} {value}, 1"
        ));
        self.own_value(&Type::Text, &argument, false);
        argument
    }

    fn emit_command(
        &mut self,
        command: &crate::hir::ApiCommand,
        index: usize,
        body: &str,
    ) -> Result<(), String> {
        let instance = Specialization {
            function: command.function,
            handlers: Vec::new(),
        };
        let function = self.module.program.functions[command.function].clone();
        let span = function.span;
        self.application_require_components(&command.components, span);
        let input_ty = asynchronous::input_type(&function);
        let mut fields = Vec::new();
        for (name, parameter) in command.inputs.iter().zip(&function.parameters) {
            let ty = parameter
                .value_type()
                .expect("checked CMD value parameter")
                .clone();
            let callback = self
                .module
                .wire_callback(&ty, crate::wire::Policy::ApiInput, false);
            fields.push((name.clone(), ty, callback));
        }
        let callback_name = format!("dever_command_decode_{index}");
        let mut decoder = FunctionEmitter::new(self.module, &instance);
        decoder.asynchronous = false;
        decoder.start("entry");
        let row = decoder.decode_fields(&input_ty, &fields, "%inputs", wire::FieldInput::Command);
        decoder.line(format!(
            "store {} {row}, ptr %out",
            decoder.module.ty(&input_ty)
        ));
        let callback =
            decoder.finish_protocol_callback(&callback_name, "ptr %inputs, ptr %out, ptr %error");
        self.module.declarations.push_str(&callback);
        let input = self.runtime_call(
            "dever_rt_v1_cmd_decode",
            vec![
                format!("ptr {body}"),
                format!("ptr @dever_type_{}", self.module.type_index(&input_ty)),
                format!("ptr @{callback_name}"),
            ],
            &input_ty,
            false,
            None,
            span,
        );
        self.own_value(&input_ty, &input, false);
        let mut args = Vec::new();
        for (index, (_, ty, _)) in fields.iter().enumerate() {
            let value = self.temp();
            self.line(format!(
                "{value} = extractvalue {} {input}, {index}",
                self.module.ty(&input_ty)
            ));
            args.push((ty.clone(), value));
        }
        let output_ty = output_type(&function.outputs);
        let transactional = self.begin_command_transaction(&instance, span);
        let (status, row) = self.invoke_values(&instance, &args, &output_ty, span);
        self.propagate_status(&status, span, false);
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {row}",
            self.module.ty(&output_ty)
        ));
        self.own_value(&output_ty, &value, false);
        let encoder = self
            .module
            .wire_callback(&output_ty, crate::wire::Policy::Output, true);
        let encoded = self.runtime_call(
            "dever_rt_v1_wire_encode",
            vec![
                format!("ptr {row}"),
                format!("ptr @dever_type_{}", self.module.type_index(&output_ty)),
                format!("ptr @{encoder}"),
            ],
            &Type::Text,
            false,
            None,
            span,
        );
        self.own_value(&Type::Text, &encoded, false);
        if transactional {
            self.commit_command_transaction();
        }
        let prefix = self.application_text("{\"code\":0,\"message\":\"ok\",\"data\":", span);
        let suffix = self.application_text("}\n", span);
        let text = self.application_concat(&prefix, &encoded, span);
        let text = self.application_concat(&text, &suffix, span);
        self.runtime_call(
            "dever_rt_v1_stdout_write",
            vec![format!("ptr {text}")],
            &Type::Bool,
            false,
            Some("i8"),
            span,
        );
        self.exit("0");
        Ok(())
    }
}
