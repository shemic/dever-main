//! Durable Job metadata and concrete payload/transaction adapters.
use super::*;
use crate::model::{DatabaseOwner, ModelScope};

const CLOCK_DECLARATIONS: &str = "\
declare i32 @dever_rt_v1_job_clock_new(i8, ptr, ptr)
declare i32 @dever_rt_v1_job_clock_now(ptr, ptr, ptr)
declare i32 @dever_rt_v1_job_clock_advance(ptr, i64, ptr)
declare void @dever_rt_v1_job_clock_release(ptr)
@dever_job_clock = private global ptr null
";

const DECLARATIONS: &str = "\
%dever.job = type { { ptr, i64 }, { ptr, i64 }, i32, i32, ptr, ptr, ptr, i64, ptr }
%dever.job_auth = type { { ptr, i64 }, ptr }
declare i32 @dever_rt_v1_job_session_new(ptr, ptr, ptr, i64, ptr, i64, ptr, i64, ptr, i64, ptr, ptr)
declare void @dever_rt_v1_job_session_release(ptr)
declare i32 @dever_rt_v1_job_configure(ptr, i8, i8, ptr)
declare i32 @dever_rt_v1_job_decode_unit(ptr, ptr)
declare ptr @dever_rt_v1_job_initialize(ptr, ptr, ptr)
declare ptr @dever_rt_v1_job_enqueue(ptr, i64, ptr, ptr, ptr, ptr, i64, ptr, ptr)
declare ptr @dever_rt_v1_job_drain(ptr, i64, ptr)
declare ptr @dever_rt_v1_job_system_scope(i64, i8, ptr, ptr, ptr)
declare ptr @dever_rt_v1_api_serve_with_jobs(ptr, ptr, i64, ptr, i64, ptr, ptr)
declare ptr @dever_rt_v1_api_migrate_application(ptr, ptr, ptr, ptr, i64, ptr, i64, ptr)
@dever_job_session = private global ptr null
";

impl Module<'_> {
    pub(super) fn jobs_enabled(&self) -> bool {
        self.application.is_some() && !self.program.jobs.is_empty()
    }

    pub(super) fn clock_enabled(&self) -> bool {
        self.jobs_enabled()
            || self
                .application
                .as_ref()
                .is_some_and(|app| app.test.is_some())
    }

    pub(super) fn owner_binding(&mut self, owner: DatabaseOwner) -> String {
        if let DatabaseOwner::Model(model) = owner {
            return self.database_binding(model);
        }
        let DatabaseOwner::Job(target) = owner else {
            unreachable!()
        };
        let (explicit, root) = self.program.database_selector(owner);
        let explicit = explicit.map(str::to_owned);
        let root = root.to_owned();
        let explicit = explicit
            .map(|value| self.wire_names(&[value]))
            .unwrap_or_else(|| "null".into());
        let root = self.api_name(&root);
        let tenant =
            u8::from(crate::native::jobs::scope(self.program, target) == ModelScope::Tenant);
        format!("{{ ptr {explicit}, {root}, i8 {tenant} }}")
    }

    pub(super) fn emit_job_descriptors(&mut self) {
        if self.clock_enabled() {
            self.declarations.push_str(CLOCK_DECLARATIONS);
        }
        if !self.jobs_enabled() {
            return;
        }
        self.declarations.push_str(DECLARATIONS);
        let jobs = self.program.jobs.clone();
        let mut descriptors = Vec::new();
        for (index, job) in jobs.iter().enumerate() {
            let binding = self.owner_binding(DatabaseOwner::Job(job.function));
            writeln!(
                self.declarations,
                "@dever_job_binding_{index} = private constant %dever.db_binding {binding}"
            )
            .unwrap();
            let target = self.api_name(&self.program.functions[job.function].name.clone());
            let schema = self.api_name(
                job.payload
                    .as_ref()
                    .map(|schema| schema.fingerprint())
                    .unwrap_or("wire-v1:Job:Unit"),
            );
            let schedule = job
                .schedule
                .as_ref()
                .map(|cron| self.wire_names(std::slice::from_ref(cron)))
                .unwrap_or_else(|| "null".into());
            let components = self.wire_names(&job.components);
            let handler = self.job_handler(index);
            descriptors.push(format!("%dever.job {{ {target}, {schema}, i32 {}, i32 {}, ptr {schedule}, ptr @dever_job_binding_{index}, ptr {components}, i64 {}, ptr {handler} }}", job.attempts, job.timeout_ms, job.components.len()));
        }
        writeln!(
            self.declarations,
            "@dever_jobs = private constant [{} x %dever.job] [{}]",
            jobs.len(),
            descriptors.join(", ")
        )
        .unwrap();
        let mut hooks = Vec::new();
        for (index, hook) in self.program.auth.clone().iter().enumerate() {
            let provider = self.api_name(&hook.provider);
            hooks.push(format!(
                "%dever.job_auth {{ {provider}, ptr @dever_api_auth_{index} }}"
            ));
        }
        writeln!(
            self.declarations,
            "@dever_job_auth = private constant [{} x %dever.job_auth] [{}]",
            hooks.len(),
            hooks.join(", ")
        )
        .unwrap();
    }

    fn job_handler(&mut self, index: usize) -> String {
        let instance = Specialization {
            function: self.application.as_ref().unwrap().job_handlers[index],
            handlers: Vec::new(),
        };
        let function = self.program.functions[instance.function].clone();
        let input = asynchronous::input_type(&function);
        let span = function.span;
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.asynchronous = false;
        emitter.protocol_callback = true;
        emitter.start("entry");
        if let Some(parameter) = function.parameters.first() {
            let ty = parameter.value_type().unwrap();
            let decoder = emitter
                .module
                .wire_callback(ty, crate::wire::Policy::Job, false);
            let payload = emitter.runtime_call(
                "dever_rt_v1_wire_decode",
                vec![
                    "ptr %payload".into(),
                    format!("ptr @dever_type_{}", emitter.module.type_index(ty)),
                    format!("ptr @{decoder}"),
                ],
                ty,
                false,
                None,
                span,
            );
            let row = emitter.temp();
            emitter.line(format!(
                "{row} = insertvalue {} zeroinitializer, {} {payload}, 0",
                emitter.module.ty(&input),
                emitter.module.ty(ty)
            ));
            emitter.line(format!(
                "store {} {row}, ptr %out",
                emitter.module.ty(&input)
            ));
        } else {
            emitter.wire_write("dever_rt_v1_job_decode_unit", vec!["ptr %payload".into()]);
        }
        let decode = format!("dever_job_decode_{index}");
        let body = emitter
            .finish_protocol_callback(&decode, "ptr %payload, i64 %unused, ptr %out, ptr %error");
        self.declarations.push_str(&body);
        let descriptor = format!("@dever_job_handler_{index}");
        writeln!(self.declarations, "{descriptor} = private constant %dever.api_handler {{ ptr @dever_async_{}, ptr null, ptr @dever_owned_{}, ptr @{decode}, ptr @dever_api_fault_status, ptr null }}", self.names[&instance], self.type_index(&input)).unwrap();
        descriptor
    }

    pub(super) fn emit_job_body(
        &mut self,
        instance: &Specialization,
        index: usize,
    ) -> Result<(), String> {
        let function = self.program.functions[instance.function].clone();
        let target = Specialization {
            function: self.program.jobs[index].function,
            handlers: Vec::new(),
        };
        let mut emitter = FunctionEmitter::new(self, instance);
        let parameters = emitter.begin_generated_function(&function);
        let arguments = function
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
        let transaction = emitter.begin_command_transaction(&target, function.span);
        if specialize::suspends(emitter.module.program, &target) {
            let (status, _) =
                emitter.invoke_values(&target, &arguments, &Type::Unit, function.span);
            emitter.propagate_status(&status, function.span, false);
        } else {
            emitter.invoke_sync_boundary(
                &target,
                &arguments,
                "async_blocking",
                &Type::Unit,
                function.span,
            );
        }
        if transaction {
            emitter.commit_command_transaction();
        }
        emitter.exit("0");
        let name = emitter.module.names[instance].clone();
        let body = emitter.finish_function(&name, &parameters);
        self.functions.push_str(&body);
        Ok(())
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn initialize_clock(&mut self, span: Span) {
        if !self.module.clock_enabled() {
            return;
        }
        let test = u8::from(self.module.application.as_ref().unwrap().test.is_some());
        let clock = self.protocol_call(
            "dever_rt_v1_job_clock_new",
            vec![format!("i8 {test}")],
            "ptr",
            span,
        );
        self.line(format!("store ptr {clock}, ptr @dever_job_clock"));
    }

    pub(super) fn register_jobs(&mut self, span: Span) {
        if !self.module.jobs_enabled() {
            return;
        }
        let session = self.temp();
        self.line(format!("{session} = load ptr, ptr @dever_api_session"));
        let clock = self.temp();
        self.line(format!("{clock} = load ptr, ptr @dever_job_clock"));
        let manifest = self
            .module
            .wire_names(&self.module.program.tenant_components.clone());
        let jobs = self.protocol_call(
            "dever_rt_v1_job_session_new",
            vec![
                format!("ptr {session}"),
                format!("ptr {clock}"),
                "ptr @dever_jobs".into(),
                format!("i64 {}", self.module.program.jobs.len()),
                "ptr @dever_job_auth".into(),
                format!("i64 {}", self.module.program.auth.len()),
                "ptr @dever_permissions".into(),
                format!("i64 {}", self.module.program.permissions.len()),
                format!("ptr {manifest}"),
                format!("i64 {}", self.module.program.tenant_components.len()),
            ],
            "ptr",
            span,
        );
        self.line(format!("store ptr {jobs}, ptr @dever_job_session"));
    }

    pub(super) fn initialize_jobs(&mut self, span: Span) {
        if !self.module.jobs_enabled() {
            return;
        }
        let jobs = self.temp();
        self.line(format!("{jobs} = load ptr, ptr @dever_job_session"));
        let operation =
            self.database_operation("job_initialize", vec![format!("ptr {jobs}")], span);
        self.await_operation(&operation, &Type::Unit, span, false);
    }

    pub(super) fn owner_database(&mut self, owner: DatabaseOwner, span: Span) -> String {
        if let DatabaseOwner::Model(model) = owner {
            return self.model_database(model);
        }
        let session = self.temp();
        self.line(format!("{session} = load ptr, ptr @dever_database_session"));
        let binding = self.module.owner_binding(owner);
        let slot = self.entry_slot_ir("%dever.db_binding");
        self.line(format!("store %dever.db_binding {binding}, ptr {slot}"));
        let operation = self.database_operation(
            "db_resolve_scoped",
            vec![format!("ptr {session}"), format!("ptr {slot}")],
            span,
        );
        let database = self.await_database_handle(&operation, span);
        self.protocol_owner("db", &database);
        database
    }

    pub(super) fn enqueue_job(
        &mut self,
        target: usize,
        arguments: &[Expression],
        scheduled: bool,
        span: Span,
    ) -> Result<String, String> {
        let index = self
            .module
            .program
            .jobs
            .iter()
            .position(|job| job.function == target)
            .expect("reachable Job");
        let has_payload = self.module.program.jobs[index].payload.is_some();
        let values = arguments
            .iter()
            .map(|argument| self.expression(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let payload = if has_payload {
            let ty = &arguments[0].ty;
            let encoder = self
                .module
                .wire_callback(ty, crate::wire::Policy::Job, true);
            let row = self.row_pointer(ty, &values[0]);
            let payload = self.runtime_call(
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
            );
            self.own_value(&Type::Text, &payload, false);
            payload
        } else {
            self.application_text("null", span)
        };
        let key = usize::from(has_payload);
        let run_at = if scheduled {
            values[key + 1].clone()
        } else {
            let clock = self.temp();
            self.line(format!("{clock} = load ptr, ptr @dever_job_clock"));
            self.protocol_call(
                "dever_rt_v1_job_clock_now",
                vec![format!("ptr {clock}")],
                "i64",
                span,
            )
        };
        let database = self.owner_database(DatabaseOwner::Job(target), span);
        let jobs = self.temp();
        self.line(format!("{jobs} = load ptr, ptr @dever_job_session"));
        let operation = self.database_operation(
            "job_enqueue",
            vec![
                format!("ptr {jobs}"),
                format!("i64 {index}"),
                format!("ptr {database}"),
                format!("ptr {}", self.database_context),
                format!("ptr {payload}"),
                format!("ptr {}", values[key]),
                format!("i64 {run_at}"),
            ],
            span,
        );
        Ok(self.await_operation(&operation, &Type::Id, span, false))
    }

    pub(super) fn invoke_system(
        &mut self,
        target: &Specialization,
        input: &str,
        tenant: &str,
        span: Span,
    ) {
        let present = self.temp();
        self.line(format!("{present} = icmp sgt i64 {tenant}, 0"));
        let flag = self.temp();
        self.line(format!("{flag} = zext i1 {present} to i8"));
        let name = self.module.names[target].clone();
        let operation = self.async_operation(
            "job_system_scope",
            vec![
                format!("i64 {tenant}"),
                format!("i8 {flag}"),
                format!("ptr @dever_async_{name}"),
                format!("ptr {input}"),
            ],
            span,
        );
        self.await_operation(&operation, &Type::Unit, span, false);
    }
}
