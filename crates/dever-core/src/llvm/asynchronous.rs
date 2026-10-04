//! Coroutine frames, typed async descriptors, and polling at actual suspension points.
use super::*;

pub(super) fn input_type(function: &crate::hir::Function) -> Type {
    Type::Outputs(
        function
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                parameter.value_type().map(|ty| crate::types::Field {
                    name: format!("p{index}"),
                    ty: ty.clone(),
                    private: false,
                    bounds: Vec::new(),
                })
            })
            .collect(),
    )
}

const DECLARATIONS: &str = "\
%dever.owned_type = type { i64, i64, ptr, ptr, i8 }
%dever.async_function = type { ptr, ptr, ptr, ptr, ptr, ptr, i8 }
%dever.sync_function = type { ptr, ptr, ptr, ptr, i8 }
declare token @llvm.coro.id(i32, ptr, ptr, ptr)
declare i64 @llvm.coro.size.i64()
declare i64 @llvm.coro.align.i64()
declare ptr @llvm.coro.begin(token, ptr)
declare i8 @llvm.coro.suspend(token, i1)
declare ptr @llvm.coro.free(token, ptr)
declare i1 @llvm.coro.end(ptr, i1, token)
declare void @llvm.coro.resume(ptr)
declare void @llvm.coro.destroy(ptr)
declare i1 @llvm.coro.done(ptr)
declare ptr @dever_rt_v1_async_frame_alloc(i64, i64)
declare void @dever_rt_v1_async_frame_free(ptr, i64, i64)
declare i32 @dever_rt_v1_async_root(ptr, ptr, ptr, ptr, ptr)
declare void @dever_rt_v1_async_complete(ptr, i32)
declare i32 @dever_rt_v1_async_op_poll(ptr, ptr, ptr, ptr, ptr, ptr)
declare void @dever_rt_v1_async_op_release(ptr)
declare ptr @dever_rt_v1_async_call(ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_sleep(i64)
declare ptr @dever_rt_v1_async_blocking(ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_parallel(ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_task_run(ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_task_run_sync(ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_task_wait(ptr)
declare ptr @dever_rt_v1_async_task_stop(ptr)
declare ptr @dever_rt_v1_async_task_retain(ptr)
declare void @dever_rt_v1_async_task_release(ptr)
declare i32 @dever_rt_v1_async_group_new(i64, ptr, ptr)
declare ptr @dever_rt_v1_async_group_run(ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_group_run_sync(ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_group_wait(ptr)
declare ptr @dever_rt_v1_async_group_stop(ptr)
declare ptr @dever_rt_v1_async_group_retain(ptr)
declare void @dever_rt_v1_async_group_release(ptr)
declare i32 @dever_rt_v1_async_channel_new(i64, ptr, ptr)
declare ptr @dever_rt_v1_async_channel_retain(ptr)
declare void @dever_rt_v1_async_channel_release(ptr)
declare ptr @dever_rt_v1_async_channel_send(ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_channel_receive(ptr)
declare ptr @dever_rt_v1_async_channel_close(ptr)
";

impl Module<'_> {
    pub(super) fn validate_async_boundaries(&self, sources: &SourceMap) -> Result<(), String> {
        for instance in &self.instances {
            let function = &self.program.functions[instance.function];
            if matches!(function.kind, FunctionKind::Job { .. }) && !self.send_safe(instance) {
                return Err(located(
                    sources,
                    function.span,
                    "LLVM Job boundary requires transferable input, frame, and typed failures",
                ));
            }
            let bindings = specialize::handler_bindings(function, instance);
            let mut rejected = None;
            crate::check::visit(function, |expression| {
                let target = match &expression.kind {
                    ExpressionKind::RunCall {
                        target, arguments, ..
                    }
                    | ExpressionKind::ParallelCall { target, arguments }
                    | ExpressionKind::BlockingCall { target, arguments } => match target {
                        CallTarget::Function(function) => Some(specialize::call_specialization(
                            *function, arguments, &bindings,
                        )),
                        CallTarget::Handler(handler) => Some(Specialization {
                            function: specialize::resolve_handler(*handler, &bindings),
                            handlers: Vec::new(),
                        }),
                    },
                    ExpressionKind::Collection {
                        operation: CollectionOp::ParallelEach,
                        handler: Some(handler),
                        ..
                    } => Some(Specialization {
                        function: specialize::resolve_handler(*handler, &bindings),
                        handlers: Vec::new(),
                    }),
                    ExpressionKind::Intrinsic {
                        operation:
                            Intrinsic::HttpServe
                            | Intrinsic::HttpServeTls
                            | Intrinsic::HttpServeLive
                            | Intrinsic::HttpServeLiveTls,
                        handler: Some(handler),
                        ..
                    } => Some(Specialization {
                        function: specialize::resolve_handler(*handler, &bindings),
                        handlers: Vec::new(),
                    }),
                    _ => None,
                };
                if let Some(target) = target
                    && !self.send_safe(&target)
                {
                    rejected.get_or_insert(expression.span);
                }
            });
            if let Some(span) = rejected {
                return Err(located(
                    sources,
                    span,
                    "LLVM task or blocking boundary requires a transferable frame, outputs, and typed failures",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn emit_async_types(&mut self) {
        let parallel = self.instances.iter().any(|instance| {
            let mut found = false;
            crate::check::visit(&self.program.functions[instance.function], |expression| {
                found |= matches!(
                    expression.kind,
                    ExpressionKind::Collection {
                        operation: CollectionOp::ParallelEach,
                        ..
                    }
                );
            });
            found
        });
        if !self
            .instances
            .iter()
            .any(|instance| specialize::suspends(self.program, instance))
            && !self.value_types.iter().any(|ty| {
                matches!(
                    ty,
                    Type::Task(_)
                        | Type::Group
                        | Type::Channel(_)
                        | Type::AsyncStream(_)
                        | Type::Socket
                        | Type::Listener
                )
            })
            && !parallel
        {
            return;
        }
        self.declarations.push_str(DECLARATIONS);
        self.declarations.push_str(super::network::DECLARATIONS);
        self.declarations.push_str(super::parallel::DECLARATIONS);
        self.declarations.push_str(concat!(
            "define internal void @dever_coro_resume(ptr %frame) {\nentry:\n  call void @llvm.coro.resume(ptr %frame)\n  ret void\n}\n",
            "define internal void @dever_coro_destroy(ptr %frame) {\nentry:\n  call void @llvm.coro.destroy(ptr %frame)\n  ret void\n}\n",
            "define internal i8 @dever_coro_done(ptr %frame) {\nentry:\n  %done = call i1 @llvm.coro.done(ptr %frame)\n  %result = zext i1 %done to i8\n  ret i8 %result\n}\n",
        ));
        for (index, ty) in self.value_types.iter().enumerate() {
            let llvm = self.ty(ty);
            writeln!(self.declarations, "define internal void @dever_move_{index}(ptr %source, ptr %destination) {{\nentry:\n  %value = load {llvm}, ptr %source\n  store {llvm} %value, ptr %destination\n  store {llvm} zeroinitializer, ptr %source\n  ret void\n}}").unwrap();
            writeln!(self.declarations, "@dever_owned_{index} = private constant %dever.owned_type {{ i64 {}, i64 {}, ptr @dever_move_{index}, ptr @dever_drop_{index}, i8 {} }}", self.bytes(ty), self.alignment(ty), u8::from(self.boundary_transferable(ty))).unwrap();
        }
        self.emit_fault_move();
        for instance in self.instances.clone() {
            self.emit_async_descriptor(&instance);
        }
    }

    // Fault payloads retain their checked nominal type. Never memcpy an erased
    // payload or treat a business error as its diagnostic text.
    fn emit_fault_move(&mut self) {
        self.declarations.push_str("define internal void @dever_fault_move(ptr %source, ptr %destination) {\nentry:\n  store %dever.fault zeroinitializer, ptr %destination\n");
        let capacity = self.instances.len() + 1;
        for (index, ty) in [
            "i32".into(),
            "i32".into(),
            "i32".into(),
            format!("[{capacity} x i32]"),
            "{ ptr, i64 }".into(),
            "i32".into(),
            "i32".into(),
        ]
        .iter()
        .enumerate()
        {
            writeln!(self.declarations, "  %s{index} = getelementptr %dever.fault, ptr %source, i32 0, i32 {index}\n  %d{index} = getelementptr %dever.fault, ptr %destination, i32 0, i32 {index}\n  %v{index} = load {ty}, ptr %s{index}\n  store {ty} %v{index}, ptr %d{index}").unwrap();
        }
        self.declarations.push_str("  %scause = getelementptr %dever.fault, ptr %source, i32 0, i32 8\n  %dcause = getelementptr %dever.fault, ptr %destination, i32 0, i32 8\n  %cause = load ptr, ptr %scause\n  store ptr %cause, ptr %dcause\n");
        let failures = self
            .instances
            .iter()
            .flat_map(|instance| specialize::failures(self.program, instance))
            .map(|failure| failure.ty)
            .collect::<BTreeSet<_>>();
        let arms = failures
            .iter()
            .map(|id| format!("i32 {}, label %payload_{id}", id + 1))
            .collect::<Vec<_>>()
            .join(" ");
        writeln!(self.declarations, "  switch i32 %v5, label %moved [{arms}]").unwrap();
        for id in failures {
            writeln!(self.declarations, "payload_{id}:\n  %sp{id} = getelementptr %dever.fault, ptr %source, i32 0, i32 7\n  %dp{id} = getelementptr %dever.fault, ptr %destination, i32 0, i32 7\n  %p{id} = load %T{id}, ptr %sp{id}\n  store %T{id} %p{id}, ptr %dp{id}\n  br label %moved").unwrap();
        }
        self.declarations
            .push_str("moved:\n  store %dever.fault zeroinitializer, ptr %source\n  ret void\n}\n");
    }

    fn fault_transferable(&self, instance: &Specialization) -> bool {
        specialize::failures(self.program, instance)
            .iter()
            .all(|failure| {
                let Shape::Choice(variants) = &self.program.types[failure.ty].shape else {
                    unreachable!()
                };
                variants[failure.variant]
                    .fields
                    .iter()
                    .all(|field| field.ty.transferable(&self.program.types))
            })
    }

    fn frame_send(&self, ty: &Type) -> bool {
        match ty {
            Type::Unit
            | Type::Task(_)
            | Type::Group
            | Type::Socket
            | Type::Listener
            | Type::Upload => true,
            Type::Stream(_) => false,
            Type::Related(_) | Type::RowStream(_) => ty.transferable(&self.program.types),
            Type::List(inner)
            | Type::Nullable(inner)
            | Type::Channel(inner)
            | Type::AsyncStream(inner) => self.frame_send(inner),
            Type::Map(key, value) | Type::MapEntry(key, value) => {
                self.frame_send(key) && self.frame_send(value)
            }
            Type::Outputs(fields) => fields.iter().all(|field| self.frame_send(&field.ty)),
            Type::Named(id) => match &self.program.types[*id].shape {
                Shape::Record(fields) => fields.iter().all(|field| self.frame_send(&field.ty)),
                Shape::Choice(variants) => variants
                    .iter()
                    .flat_map(|variant| &variant.fields)
                    .all(|field| self.frame_send(&field.ty)),
            },
            _ => ty.transferable(&self.program.types),
        }
    }

    fn boundary_transferable(&self, ty: &Type) -> bool {
        // Unit and Outputs are compiler-internal ABI rows. HTTP may transfer
        // its affine Upload owner; source spawn checks still forbid Upload.
        match ty {
            Type::Unit | Type::Upload => true,
            Type::Outputs(fields) => fields
                .iter()
                .all(|field| self.boundary_transferable(&field.ty)),
            _ => ty.transferable(&self.program.types),
        }
    }

    fn send_safe(&self, instance: &Specialization) -> bool {
        let function = &self.program.functions[instance.function];
        let boundary = function
            .parameters
            .iter()
            .filter_map(Parameter::value_type)
            .all(|ty| self.boundary_transferable(ty))
            && function
                .outputs
                .iter()
                .all(|field| field.ty.transferable(&self.program.types))
            && self.fault_transferable(instance);
        if !boundary || !specialize::suspends(self.program, instance) {
            return boundary;
        }
        // A Send caller also carries the futures of ordinary async callees.
        specialize::reachable(&self.program.functions, [instance.clone()])
            .expect("validated acyclic calls")
            .iter()
            .all(|callee| {
                if !specialize::suspends(self.program, callee) {
                    return true;
                }
                let function = &self.program.functions[callee.function];
                let mut expressions_send = true;
                crate::check::visit(function, |expression| {
                    expressions_send &= self.frame_send(&expression.ty)
                });
                expressions_send
                    && function
                        .parameters
                        .iter()
                        .filter_map(Parameter::value_type)
                        .all(|ty| self.frame_send(ty))
                    && function
                        .clauses
                        .iter()
                        .flat_map(|clause| &clause.locals)
                        .all(|ty| self.frame_send(ty))
                    && function
                        .outputs
                        .iter()
                        .all(|field| self.frame_send(&field.ty))
                    && self.fault_transferable(callee)
            })
    }

    fn emit_async_descriptor(&mut self, instance: &Specialization) {
        let function = &self.program.functions[instance.function];
        let name = self.names[instance].clone();
        let input = input_type(function);
        let input_id = self.type_index(&input);
        let output_id = self.type_index(&output_type(&function.outputs));
        writeln!(self.declarations, "@dever_fault_owned_{name} = private constant %dever.owned_type {{ i64 ptrtoint (ptr getelementptr (%dever.fault, ptr null, i32 1) to i64), i64 8, ptr @dever_fault_move, ptr @dever_fault_release, i8 {} }}", u8::from(self.fault_transferable(instance))).unwrap();
        let asynchronous = specialize::suspends(self.program, instance);
        let mut wrapper = format!(
            "define internal {} @dever_{}_create_{name}({}) {{\nentry:\n",
            if asynchronous { "ptr" } else { "i32" },
            if asynchronous { "async" } else { "sync" },
            if asynchronous {
                "ptr %state, ptr %input, ptr %out, ptr %fault"
            } else {
                "ptr %input, ptr %out, ptr %fault"
            }
        );
        wrapper.push_str("  store %dever.fault zeroinitializer, ptr %fault\n");
        let mut args = Vec::new();
        for (position, (index, ty)) in function
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| parameter.value_type().map(|ty| (index, ty)))
            .enumerate()
        {
            writeln!(wrapper, "  %a{index} = getelementptr {}, ptr %input, i32 0, i32 {position}\n  %p{index} = load {}, ptr %a{index}", self.ty(&input), self.ty(ty)).unwrap();
            args.push(format!("{} %p{index}", self.ty(ty)));
        }
        let database = self.database_function(instance);
        let direct_arguments = args.clone();
        if database {
            args.push("ptr null".into());
        }
        args.extend(["ptr %out".into(), "ptr %fault".into()]);
        if asynchronous {
            args.push("ptr %state".into());
        }
        writeln!(
            wrapper,
            "  %result = call {} @{name}({})",
            if asynchronous { "ptr" } else { "i32" },
            args.join(", ")
        )
        .unwrap();
        writeln!(
            wrapper,
            "  ret {} %result\n}}",
            if asynchronous { "ptr" } else { "i32" }
        )
        .unwrap();
        self.declarations.push_str(&wrapper);
        let send_safe = u8::from(self.send_safe(instance));
        if asynchronous {
            writeln!(self.declarations, "@dever_async_{name} = private constant %dever.async_function {{ ptr @dever_owned_{output_id}, ptr @dever_fault_owned_{name}, ptr @dever_async_create_{name}, ptr @dever_coro_resume, ptr @dever_coro_destroy, ptr @dever_coro_done, i8 {send_safe} }}").unwrap();
            if database {
                let layout = format!("{{ {}, ptr }}", self.ty(&input));
                writeln!(self.declarations, "define internal ptr @dever_async_context_create_{name}(ptr %state, ptr %input, ptr %out, ptr %fault) {{\nentry:\n  %context_ptr = getelementptr {layout}, ptr %input, i32 0, i32 1\n  %context = load ptr, ptr %context_ptr\n  store %dever.fault zeroinitializer, ptr %fault").unwrap();
                for (position, (index, ty)) in function
                    .parameters
                    .iter()
                    .enumerate()
                    .filter_map(|(index, parameter)| parameter.value_type().map(|ty| (index, ty)))
                    .enumerate()
                {
                    writeln!(self.declarations, "  %a{index} = getelementptr {}, ptr %input, i32 0, i32 {position}\n  %p{index} = load {}, ptr %a{index}", self.ty(&input), self.ty(ty)).unwrap();
                }
                let mut arguments = direct_arguments;
                arguments.extend([
                    "ptr %context".into(),
                    "ptr %out".into(),
                    "ptr %fault".into(),
                    "ptr %state".into(),
                ]);
                writeln!(self.declarations, "  %result = call ptr @{name}({})\n  ret ptr %result\n}}\n@dever_async_context_{name} = private constant %dever.async_function {{ ptr @dever_owned_{output_id}, ptr @dever_fault_owned_{name}, ptr @dever_async_context_create_{name}, ptr @dever_coro_resume, ptr @dever_coro_destroy, ptr @dever_coro_done, i8 {send_safe} }}", arguments.join(", ")).unwrap();
            }
        } else {
            writeln!(self.declarations, "@dever_sync_{name} = private constant %dever.sync_function {{ ptr @dever_owned_{input_id}, ptr @dever_owned_{output_id}, ptr @dever_fault_owned_{name}, ptr @dever_sync_create_{name}, i8 {send_safe} }}").unwrap();
        }
    }

    pub(super) fn emit_async_entry(&mut self, root: &Specialization) {
        let name = self.names[root].clone();
        let location = self.location(self.program.functions[root.function].span);
        let (symbol, extra) = if self.api_enabled() {
            (
                "api_application_root",
                ", ptr @dever_api_session, ptr @dever_api_append_cause".into(),
            )
        } else if self.database_enabled() {
            let errors = self.database_error(root, self.program.functions[root.function].span);
            (
                "db_application_root",
                format!(", ptr @dever_database_session, ptr {errors}"),
            )
        } else if self.external_enabled() {
            ("external_root", ", ptr @dever_external_append_cause".into())
        } else {
            ("async_root", String::new())
        };
        writeln!(self.functions, "  %error = alloca {{ ptr, i64 }}\n  %input = alloca {{}}\n  %async_status = call i32 @dever_rt_v1_{symbol}(ptr @dever_async_{name}, ptr %input, ptr %out, ptr %fault{extra}, ptr %error)\n  switch i32 %async_status, label %root_error [i32 1, label %root_success i32 2, label %root_fault]\nroot_error:\n  %message = load {{ ptr, i64 }}, ptr %error\n  %message_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 4\n  store {{ ptr, i64 }} %message, ptr %message_ptr\n  %code = getelementptr %dever.fault, ptr %fault, i32 0, i32 0\n  store i32 3, ptr %code\n  %origin = getelementptr %dever.fault, ptr %fault, i32 0, i32 1\n  store i32 {location}, ptr %origin\n  br label %root_fault\nroot_success:\n  br label %root_return\nroot_fault:\n  br label %root_return\nroot_return:\n  %status = phi i32 [0, %root_success], [1, %root_fault]").unwrap();
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn parameter(&self, index: usize) -> String {
        self.parameters
            .get(&index)
            .cloned()
            .unwrap_or_else(|| format!("%p{index}"))
    }

    pub(super) fn begin_coroutine(&mut self, function: &crate::hir::Function) {
        self.prologue.push_str(concat!(
            "  %cancelling = alloca i1\n  store i1 0, ptr %cancelling\n",
            "  %coro_id = call token @llvm.coro.id(i32 0, ptr null, ptr null, ptr null)\n",
            "  %coro_size = call i64 @llvm.coro.size.i64()\n  %coro_align = call i64 @llvm.coro.align.i64()\n",
            "  %coro_memory = call ptr @dever_rt_v1_async_frame_alloc(i64 %coro_size, i64 %coro_align)\n",
            "  %coro_frame = call ptr @llvm.coro.begin(token %coro_id, ptr %coro_memory)\n",
        ));
        // create() receives borrowed arguments. Acquire frame ownership before
        // the initial suspend returns to its caller, not on the first poll.
        for (index, parameter) in function.parameters.iter().enumerate() {
            let Some(ty) = parameter.value_type() else {
                continue;
            };
            if contains_owned(self.module.program, ty) {
                let guard = self.own_value(ty, &format!("%p{index}"), true);
                let owned = self.load_owned_value(ty, guard);
                self.parameters.insert(index, owned);
            }
        }
        if self.database_context != "null" {
            let retained = self.temp();
            self.line(format!(
                "{retained} = call ptr @dever_rt_v1_db_transaction_retain(ptr {})",
                self.database_context
            ));
            self.protocol_owner("db_transaction", &retained);
            self.database_context = retained;
        }
        self.suspend("async_start", false);
        self.start("async_start");
    }

    fn suspend(&mut self, resume: &str, final_suspend: bool) {
        let result = self.temp();
        self.line(format!(
            "{result} = call i8 @llvm.coro.suspend(token none, i1 {})",
            u8::from(final_suspend)
        ));
        let arms = if final_suspend {
            "i8 1, label %free_frame".into()
        } else {
            format!("i8 0, label %{resume} i8 1, label %destroy_frame")
        };
        self.line(format!(
            "switch i8 {result}, label %suspend_return [{arms}]"
        ));
    }

    pub(super) fn end_coroutine(&mut self, status: &str) {
        let cancelling = self.temp();
        self.line(format!("{cancelling} = load i1, ptr %cancelling"));
        self.line(format!(
            "br i1 {cancelling}, label %free_frame, label %complete_frame"
        ));
        self.start("complete_frame");
        let okay = self.temp();
        self.line(format!("{okay} = icmp eq i32 {status}, 0"));
        let completion = self.temp();
        self.line(format!("{completion} = select i1 {okay}, i32 1, i32 2"));
        self.line(format!(
            "call void @dever_rt_v1_async_complete(ptr %state, i32 {completion})"
        ));
        self.suspend("", true);
        self.start("destroy_frame");
        self.line("store i1 1, ptr %cancelling");
        self.line("br label %cleanup");
        self.start("free_frame");
        self.line("%allocation = call ptr @llvm.coro.free(token %coro_id, ptr %coro_frame)");
        self.line("call void @dever_rt_v1_async_frame_free(ptr %allocation, i64 %coro_size, i64 %coro_align)");
        self.line("br label %suspend_return");
        self.start("suspend_return");
        self.line("call i1 @llvm.coro.end(ptr %coro_frame, i1 false, token none)");
        self.line("ret ptr %coro_frame");
    }

    pub(super) fn pack_input(
        &mut self,
        instance: &Specialization,
        values: &[(Type, String)],
        owned: bool,
    ) -> (String, Option<usize>) {
        let ty = input_type(&self.module.program.functions[instance.function]);
        let source = self.entry_slot(&ty);
        self.line(format!(
            "store {} zeroinitializer, ptr {source}",
            self.module.ty(&ty)
        ));
        for (index, (field, value)) in values.iter().enumerate() {
            let pointer = self.temp();
            self.line(format!(
                "{pointer} = getelementptr {}, ptr {source}, i32 0, i32 {index}",
                self.module.ty(&ty)
            ));
            self.line(format!(
                "store {} {value}, ptr {pointer}",
                self.module.ty(field)
            ));
        }
        if !owned {
            return (source, None);
        }
        let destination = self.entry_slot(&ty);
        self.line(format!(
            "call void @dever_clone_{}(ptr {source}, ptr {destination})",
            self.module.type_index(&ty)
        ));
        let guard = self.register_guard(&ty, destination.clone());
        self.mark_live(guard);
        (destination, Some(guard))
    }

    pub(super) fn async_operation(
        &mut self,
        symbol: &str,
        mut arguments: Vec<String>,
        span: Span,
    ) -> String {
        let error = self.entry_slot_ir("{ ptr, i64 }");
        arguments.push(format!("ptr {error}"));
        let operation = self.temp();
        self.line(format!(
            "{operation} = call ptr @dever_rt_v1_{symbol}({})",
            arguments.join(", ")
        ));
        let missing = self.temp();
        self.line(format!("{missing} = icmp eq ptr {operation}, null"));
        let failed = self.label("operation_error");
        let ready = self.label("operation_created");
        self.line(format!("br i1 {missing}, label %{failed}, label %{ready}"));
        self.start(&failed);
        self.check_runtime_status("1", &error, span);
        self.line("unreachable");
        self.start(&ready);
        operation
    }

    pub(super) fn poll_raw(
        &mut self,
        operation: &str,
        row: &Type,
    ) -> (String, String, String, String) {
        self.poll_raw_layout(operation, &self.module.ty(row))
    }

    pub(super) fn poll_raw_layout(
        &mut self,
        operation: &str,
        layout: &str,
    ) -> (String, String, String, String) {
        debug_assert!(
            self.asynchronous,
            "only checked suspension points poll operations"
        );
        let operation_slot = self.entry_slot_ir("ptr");
        let guard = self.register_owned(OwnedKind::AsyncOp, operation_slot.clone());
        self.line(format!("store ptr {operation}, ptr {operation_slot}"));
        self.mark_live(guard);
        let output = self.entry_slot_ir(layout);
        self.line(format!("store {layout} zeroinitializer, ptr {output}"));
        let present = self.entry_slot_ir("i8");
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let poll = self.label("poll_operation");
        let waiting = self.label("pending_operation");
        let ready = self.label("ready_operation");
        self.line(format!("br label %{poll}"));
        self.start(&poll);
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_async_op_poll(ptr %state, ptr {operation}, ptr {output}, ptr {present}, ptr %fault, ptr {error})"));
        let pending = self.temp();
        self.line(format!("{pending} = icmp eq i32 {status}, 0"));
        self.line(format!("br i1 {pending}, label %{waiting}, label %{ready}"));
        self.start(&waiting);
        self.suspend(&poll, false);
        self.start(&ready);
        self.release_guard(guard);
        (status, output, present, error)
    }

    pub(super) fn poll_operation(
        &mut self,
        operation: &str,
        output: &Type,
        span: Span,
    ) -> (String, String) {
        let (status, value, _, error) = self.poll_raw(operation, output);
        let legacy = self.poll_status(&status, &error, span);
        (legacy, value)
    }

    pub(super) fn poll_status(&mut self, status: &str, error: &str, span: Span) -> String {
        let error_status = self.temp();
        self.line(format!("{error_status} = icmp ugt i32 {status}, 2"));
        let failed = self.label("poll_error");
        let done = self.label("poll_done");
        self.line(format!(
            "br i1 {error_status}, label %{failed}, label %{done}"
        ));
        self.start(&failed);
        self.check_runtime_status(status, error, span);
        self.line("unreachable");
        self.start(&done);
        let legacy = self.temp();
        self.line(format!("{legacy} = sub i32 {status}, 1"));
        legacy
    }

    pub(super) fn await_operation(
        &mut self,
        operation: &str,
        output: &Type,
        span: Span,
        trace: bool,
    ) -> String {
        let (status, value) = self.poll_operation(operation, output, span);
        self.propagate_status(&status, span, trace);
        let result = self.temp();
        self.line(format!(
            "{result} = load {}, ptr {value}",
            self.module.ty(output)
        ));
        result
    }

    pub(super) fn propagate_status(&mut self, status: &str, span: Span, trace: bool) {
        let failed = self.temp();
        self.line(format!("{failed} = icmp ne i32 {status}, 0"));
        let error = self.label("await_fault");
        let done = self.label("await_done");
        self.line(format!("br i1 {failed}, label %{error}, label %{done}"));
        self.start(&error);
        if trace {
            self.append_frame(span);
        }
        self.exit(status);
        self.start(&done);
    }
}
