use super::{Emitter, rust_string};
use crate::hir::Expression;
use crate::model::{DatabaseOwner, ModelScope};
use crate::source::Span;
use std::fmt::Write;

pub(super) fn targets(
    program: &crate::hir::Program,
    instances: &std::collections::BTreeSet<crate::specialize::Specialization>,
) -> std::collections::BTreeSet<usize> {
    let mut targets = std::collections::BTreeSet::new();
    for instance in instances {
        if matches!(
            program.functions[instance.function].kind,
            crate::syntax::FunctionKind::Job { .. }
        ) {
            targets.insert(instance.function);
        }
        crate::check::visit(&program.functions[instance.function], |expression| {
            if let crate::hir::ExpressionKind::JobEnqueue { target, .. } = expression.kind {
                targets.insert(target);
            }
        });
    }
    targets
}

pub(crate) fn scope(program: &crate::hir::Program, target: usize) -> ModelScope {
    let instance = crate::specialize::Specialization {
        function: target,
        handlers: Vec::new(),
    };
    let component_is_tenant = program.packages[program.functions[target].owner]
        .domain
        .as_ref()
        .is_some_and(|(component, _)| program.tenant_components.contains(component));
    if component_is_tenant || crate::specialize::database_effects(program, &instance).iter().any(|owner| {
        matches!(owner, DatabaseOwner::Model(model) if program.models[*model].scope == ModelScope::Tenant)
    }) {
        ModelScope::Tenant
    } else {
        ModelScope::Global
    }
}

fn scope_name(scope: ModelScope) -> &'static str {
    match scope {
        ModelScope::Global => "Global",
        ModelScope::Tenant => "Tenant",
    }
}

pub(super) fn definitions(
    emitter: &Emitter<'_>,
    output: &mut String,
    instances: &std::collections::BTreeSet<crate::specialize::Specialization>,
) {
    let targets = targets(emitter.program, instances);
    if targets.is_empty() && !uses_test_controls(emitter.program, instances) {
        return;
    }
    output.push_str("static JOB_CLOCK: std::sync::OnceLock<dever_runtime::time::Clock> = std::sync::OnceLock::new();\n");
    if targets.is_empty() {
        output.push_str("async fn job_drain(limit: i64) -> Result<i64, String> { if !(1..=10000).contains(&limit) { return Err(\"test Job drain limit must be between 1 and 10000\".into()); } Ok(0) }\n");
        return;
    }
    for target in &targets {
        let target = *target;
        let job = emitter
            .program
            .jobs
            .iter()
            .find(|job| job.function == target)
            .expect("checked Job target");
        if let Some(schema) = &job.payload {
            output.push_str(&super::wire::emit_named(schema, &format!("job_{target}")));
        }
        writeln!(output, "static JOB_STORAGE_{target}: std::sync::OnceLock<dever_runtime::database::StorageBinding> = std::sync::OnceLock::new();\nasync fn job_database_{target}() -> Result<dever_runtime::database::Database, dever_runtime::orm::Error> {{ JOB_STORAGE_{target}.get().expect(\"Job storage initialized before entry\").database().await }}\nstatic JOB_SPEC_{target}: dever_runtime::job::Spec = dever_runtime::job::Spec {{ target: {}, schema: {}, attempts: {}, timeout_ms: {} }};",
            rust_string(&emitter.program.functions[target].name), rust_string(job.payload.as_ref().map(|schema| schema.fingerprint()).unwrap_or("wire-v1:Job:Unit")), job.attempts, job.timeout_ms).expect("string formatting");
    }
    output.push_str("fn job_bindings() -> Vec<dever_runtime::job::worker::Binding> { vec![\n");
    for target in &targets {
        let job = emitter
            .program
            .jobs
            .iter()
            .find(|job| job.function == *target)
            .expect("checked Job");
        let schedule = job
            .schedule
            .as_ref()
            .map(|cron| {
                format!(
                    "Some(dever_runtime::cron::Cron::parse({}).expect(\"checked cron\"))",
                    rust_string(cron)
                )
            })
            .unwrap_or_else(|| "None".into());
        let scope = scope_name(scope(emitter.program, *target));
        writeln!(output, "dever_runtime::job::worker::Binding::scoped(JOB_STORAGE_{target}.get().expect(\"Job storage initialized before entry\"), dever_runtime::database::ModelScope::{scope}, &JOB_SPEC_{target}, {schedule}),").expect("string formatting");
    }
    output.push_str("] }\n");
    emit_identity_failure(emitter.program, output);
    output.push_str("async fn job_dispatch_verified(claim: dever_runtime::job::store::Claim) -> dever_runtime::job::store::Outcome {\nuse dever_runtime::job::store::Outcome;\nmatch claim.target.as_str() {\n");
    for target in &targets {
        let job = emitter
            .program
            .jobs
            .iter()
            .find(|job| job.function == *target)
            .expect("checked Job");
        writeln!(output, "{} => {{ if claim.schema != JOB_SPEC_{target}.schema {{ return Outcome::SchemaMismatch; }}", rust_string(&emitter.program.functions[*target].name)).expect("string formatting");
        let mut arguments = Vec::new();
        if job.payload.is_some() {
            writeln!(output, "let payload = match job_{target}_decode(&claim.payload) {{ Ok(payload) => payload, Err(_) => return Outcome::InvalidPayload }};").expect("string formatting");
            arguments.push("payload");
        } else {
            output.push_str("if !dever_runtime::wire::parse(&claim.payload).is_ok_and(|value| value.is_null()) { return Outcome::InvalidPayload; }\n");
        }
        let instance = crate::specialize::Specialization {
            function: *target,
            handlers: Vec::new(),
        };
        let transaction_database = super::transaction_database(emitter.program, *target);
        if let Some(database) = &transaction_database {
            writeln!(output, "let transaction_database = match {database} {{ Ok(database) => database, Err(_) => return Outcome::Retry }};\nlet transaction = match transaction_database.begin().await {{ Ok(value) => value, Err(_) => return Outcome::Retry }};")
                .expect("string formatting");
        }
        if !crate::specialize::database_effects(emitter.program, &instance).is_empty() {
            arguments.push(if transaction_database.is_some() {
                "Some(&transaction)"
            } else {
                "None"
            });
        }
        let call = format!("{}({})", emitter.names[&instance], arguments.join(", "));
        let call = if crate::specialize::suspends(emitter.program, &instance) {
            format!("{call}.await")
        } else {
            format!("task::blocking_typed(move || {call}).await")
        };
        if transaction_database.is_some() {
            writeln!(output, "match {call} {{ Ok(()) => match transaction.commit().await {{ Ok(()) => Outcome::Success, Err(_) => Outcome::Retry }}, Err(_) => {{ let _ = transaction.rollback().await; Outcome::Retry }} }} }},").expect("string formatting");
        } else {
            writeln!(
                output,
                "match {call} {{ Ok(()) => Outcome::Success, Err(_) => Outcome::Retry }} }},"
            )
            .expect("string formatting");
        }
    }
    output.push_str("_ => Outcome::UnknownTarget,\n}\n}\n");
    emit_component_check(emitter.program, output);
    emit_dispatch(emitter.program, output);
    output.push_str("async fn job_drain(limit: i64) -> Result<i64, String> { dever_runtime::job::worker::drain(job_bindings(), JOB_CLOCK.get().expect(\"clock initialized\").clone(), limit, job_dispatch).await }\nasync fn job_serve() -> Result<(), String> { if !dever_runtime::lifecycle::worker_enabled() { return Ok(()); } dever_runtime::job::worker::serve(job_bindings(), JOB_CLOCK.get().expect(\"clock initialized\").clone(), dever_runtime::config::settings().jobs()?, job_dispatch).await }\n");
}

fn emit_identity_failure(program: &crate::hir::Program, output: &mut String) {
    let failures = program
        .failures
        .iter()
        .flat_map(|failures| failures.iter())
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let unauthorized = program
        .types
        .iter()
        .enumerate()
        .find(|(_, definition)| definition.name == "dever.api.Error")
        .and_then(|(id, definition)| match &definition.shape {
            crate::types::Shape::Choice(variants) => Some((id, variants)),
            _ => None,
        });
    output.push_str("fn job_identity_failure(error: AppError) -> dever_runtime::job::store::Outcome {\nuse dever_runtime::job::store::Outcome;\nmatch &error.kind {\n");
    if let Some((id, variants)) = unauthorized {
        for name in ["Unauthorized", "Forbidden"] {
            if let Some(index) = variants.iter().position(|variant| variant.name == name)
                && failures.contains(&crate::hir::Failure {
                    ty: id,
                    variant: index,
                })
            {
                writeln!(
                    output,
                    "AppErrorKind::E{id}V{index} => Outcome::IdentityRejected,"
                )
                .expect("string formatting");
            }
        }
    }
    output.push_str("_ => { dever_runtime::log::error(\"Job identity verification failed\", Vec::new()); Outcome::Retry }\n}\n}\n");
}

fn emit_dispatch(program: &crate::hir::Program, output: &mut String) {
    output.push_str("async fn job_dispatch(claim: dever_runtime::job::store::Claim) -> dever_runtime::job::store::Outcome {\nuse dever_runtime::job::store::Outcome;\nmatch claim.execution.clone() {\ndever_runtime::job::ExecutionIdentity::System { .. } => { if let Err(outcome) = job_require_components(&claim).await { return outcome; } job_dispatch_verified(claim).await },\ndever_runtime::job::ExecutionIdentity::User { tenant_id, provider, site, subject, session, tenant_claim, permission_key } => {\n");
    if program.auth.is_empty() || program.permissions.is_empty() {
        output.push_str("let _ = (tenant_id, provider, site, subject, session, tenant_claim, permission_key);\nOutcome::InvalidIdentity\n");
    } else {
        output.push_str("if dever_runtime::auth::validate_job_source(&provider, &site).is_err() { return Outcome::InvalidIdentity; }\nmatch provider.as_str() {\n");
        for hook in &program.auth {
            let instance = crate::specialize::Specialization {
                function: hook.function,
                handlers: Vec::new(),
            };
            writeln!(output, "{} => {{", rust_string(&hook.provider)).expect("string formatting");
            writeln!(output,
                "let auth_claims = T{} {{ f0: subject.clone(), f1: session.clone(), f2: tenant_claim.clone(), f3: site.clone() }};",
                hook.claims,
            ).expect("string formatting");
            let mut arguments = vec!["auth_claims".to_owned()];
            if !crate::specialize::database_effects(program, &instance).is_empty() {
                arguments.push("None".into());
            }
            let call = format!(
                "f{}({}){}",
                hook.function,
                arguments.join(", "),
                if crate::specialize::suspends(program, &instance) {
                    ".await"
                } else {
                    ""
                }
            );
            writeln!(output,
                "let identity: T{} = match {call} {{ Ok(identity) => identity, Err(error) => return job_identity_failure(error), }};\nlet verified_identity = dever_runtime::auth::Identity {{ id: identity.f0, user_id: identity.f1.map(|value| value.0), tenant_id: identity.f2.map(|value| value.0) }};\nlet verified_claims = dever_runtime::auth::Claims {{ subject, session, tenant: tenant_claim, site: site.clone() }};\nif dever_runtime::auth::validate_job_identity(&verified_claims, &verified_identity).is_err() || verified_identity.tenant_id != tenant_id {{ return Outcome::IdentityRejected; }}\nif !AUTHORIZATION_PERMISSIONS.iter().any(|permission| permission.key == permission_key && permission.site == site) {{ return Outcome::IdentityRejected; }}\nif let Err(outcome) = job_require_components(&claim).await {{ return outcome; }}\nlet Some(user_id) = verified_identity.user_id else {{ return Outcome::IdentityRejected; }};\nlet authorization_database = if tenant_id.is_some() {{ let Some(tenant) = dever_runtime::config::settings().tenant() else {{ return Outcome::InvalidIdentity; }}; match dever_runtime::tenant::database(tenant.database()).await {{ Ok(database) => database, Err(_) => return Outcome::Retry }} }} else {{ match authorization_control_database() {{ Ok(database) => database, Err(_) => return Outcome::Retry }} }};\nmatch dever_runtime::auth::store::authorize(authorization_database, user_id, &site, &permission_key).await {{ Ok(true) => job_dispatch_verified(claim).await, Ok(false) => Outcome::IdentityRejected, Err(_) => Outcome::Retry }}\n}},",
                hook.identity,
            ).expect("string formatting");
        }
        output.push_str("_ => Outcome::InvalidIdentity,\n}\n");
    }
    output.push_str("}\n}\n}\n");
}

fn emit_component_check(program: &crate::hir::Program, output: &mut String) {
    output.push_str("async fn job_require_components(claim: &dever_runtime::job::store::Claim) -> Result<(), dever_runtime::job::store::Outcome> {\nmatch claim.target.as_str() {\n");
    for job in &program.jobs {
        if job.components.is_empty() {
            continue;
        }
        writeln!(output,
            "{} => dever_runtime::job::require_components(&claim.execution, &[{}], TENANT_COMPONENTS).await?,",
            rust_string(&program.functions[job.function].name),
            job.components.iter().map(|component| rust_string(component)).collect::<Vec<_>>().join(", "),
        ).expect("string formatting");
    }
    output.push_str("_ => {},\n}\nOk(())\n}\n");
}

pub(super) fn initialize(
    emitter: &Emitter<'_>,
    output: &mut String,
    instances: &std::collections::BTreeSet<crate::specialize::Specialization>,
    span: Span,
) {
    if targets(emitter.program, instances).is_empty() {
        return;
    }
    if !emitter.sources.get(span.source).is_test() {
        initialize_clock(emitter, output, instances, span);
    }
    let mut connections = std::collections::BTreeSet::new();
    for target in targets(emitter.program, instances) {
        let (explicit, root) = emitter
            .program
            .database_selector(DatabaseOwner::Job(target));
        let explicit = explicit
            .map(|name| format!("Some({})", rust_string(name)))
            .unwrap_or_else(|| "None".into());
        let job_scope = scope(emitter.program, target);
        let scope = scope_name(job_scope);
        writeln!(output, "JOB_STORAGE_{target}.set(dever_runtime::database::StorageBinding::new({explicit}, {}, dever_runtime::database::ModelScope::{scope})).map_err(|_| AppError::fault({}, \"Job storage initialized twice\"))?;\nlet job_database_{target} = if dever_runtime::config::settings().tenant().is_some() && matches!(dever_runtime::database::ModelScope::{scope}, dever_runtime::database::ModelScope::Tenant) {{ None }} else {{ Some({}) }};", rust_string(root), emitter.location(span), emitter.database_failure(&format!("job_database_{target}().await"), span)).expect("string formatting");
        if connections.insert((explicit, root, job_scope == ModelScope::Tenant)) {
            let initialize = emitter.database_failure(&format!("dever_runtime::job::store::initialize(job_database_{target}.as_ref().expect(\"selected Job database\").clone()).await"), span);
            writeln!(
                output,
                "if job_database_{target}.is_some() {{ {initialize}; }}"
            )
            .expect("string formatting");
        }
        let job = emitter
            .program
            .jobs
            .iter()
            .find(|job| job.function == target)
            .expect("checked Job");
        if let Some(cron) = &job.schedule {
            let validate = emitter.database_failure(&format!("dever_runtime::job::store::validate_schedule(job_database_{target}.as_ref().expect(\"selected Job database\").clone(), &JOB_SPEC_{target}, &dever_runtime::cron::Cron::parse({}).expect(\"checked cron\")).await", rust_string(cron)), span);
            writeln!(
                output,
                "if job_database_{target}.is_some() {{ {validate}; }}"
            )
            .expect("string formatting");
        }
    }
}

pub(super) fn tenant_migration(
    emitter: &Emitter<'_>,
    output: &mut String,
    instances: &std::collections::BTreeSet<crate::specialize::Specialization>,
    database_enabled: bool,
) {
    if !database_enabled {
        return;
    }
    let targets = targets(emitter.program, instances);
    if !targets
        .iter()
        .any(|target| scope(emitter.program, *target) == ModelScope::Tenant)
    {
        // ORM tenant migration calls the Job migration hook unconditionally.
        // Keep that generated contract total for applications without tenant
        // Jobs instead of leaving an unresolved function reference.
        output.push_str("async fn job_migrate_tenant(_tenant_id: i64, _schema_fingerprint: &str, _databases: &mut std::collections::BTreeMap<String, dever_runtime::database::Database>) -> Result<(), AppError> { Ok(()) }\n");
        return;
    }
    output.push_str("async fn job_migrate_tenant(tenant_id: i64, schema_fingerprint: &str, databases: &mut std::collections::BTreeMap<String, dever_runtime::database::Database>) -> Result<(), AppError> {\nlet settings = dever_runtime::config::settings();\n");
    let span = targets
        .iter()
        .next()
        .map(|target| emitter.program.functions[*target].span);
    let mut connections = std::collections::BTreeMap::new();
    for target in targets
        .into_iter()
        .filter(|target| scope(emitter.program, *target) == ModelScope::Tenant)
    {
        let (explicit, root) = emitter
            .program
            .database_selector(DatabaseOwner::Job(target));
        let explicit_code = explicit
            .map(|name| format!("Some({})", rust_string(name)))
            .unwrap_or_else(|| "None".into());
        let identity = (explicit.map(str::to_owned), root.to_owned());
        let connection_target = *connections.entry(identity).or_insert(target);
        if connection_target == target {
            writeln!(output, "let job_connection_{target} = settings.resolve_database({explicit_code}, {}).map_err(|error| AppError::fault(\"tenant Job migration\", error))?.0.to_owned();\nif !databases.contains_key(&job_connection_{target}) {{ let database = dever_runtime::tenant::begin_migration(&job_connection_{target}, tenant_id, schema_fingerprint).await.map_err(|error| AppError::database(\"tenant Job migration\", error))?; databases.insert(job_connection_{target}.clone(), database); }}\n{};", rust_string(root), emitter.database_failure(&format!("dever_runtime::job::store::initialize(databases.get(&job_connection_{target}).expect(\"tenant Job database initialized\").clone()).await"), span.expect("tenant Job has source span"))).expect("string formatting");
        }
        let job = emitter
            .program
            .jobs
            .iter()
            .find(|job| job.function == target)
            .expect("checked Job");
        if let Some(cron) = &job.schedule {
            let validate = emitter.database_failure(&format!("dever_runtime::job::store::validate_schedule(databases.get(&job_connection_{connection_target}).expect(\"tenant Job database initialized\").clone(), &JOB_SPEC_{target}, &dever_runtime::cron::Cron::parse({}).expect(\"checked cron\")).await", rust_string(cron)), span.expect("tenant Job has source span"));
            writeln!(output, "{validate};").expect("string formatting");
        }
    }
    output.push_str("Ok(())\n}\n");
}

fn uses_test_controls(
    program: &crate::hir::Program,
    instances: &std::collections::BTreeSet<crate::specialize::Specialization>,
) -> bool {
    instances.iter().any(|instance| {
        let mut found = false;
        crate::check::visit(&program.functions[instance.function], |expression| {
            if let crate::hir::ExpressionKind::Intrinsic {
                operation:
                    crate::intrinsic::Intrinsic::TestJobDrain
                    | crate::intrinsic::Intrinsic::TestClockAdvance,
                ..
            } = expression.kind
            {
                found = true;
            }
        });
        found
    })
}

pub(super) fn initialize_clock(
    emitter: &Emitter<'_>,
    output: &mut String,
    instances: &std::collections::BTreeSet<crate::specialize::Specialization>,
    span: Span,
) {
    if targets(emitter.program, instances).is_empty()
        && !uses_test_controls(emitter.program, instances)
    {
        return;
    }
    let clock = if emitter.sources.get(span.source).is_test() {
        "for_test"
    } else {
        "default"
    };
    writeln!(output, "JOB_CLOCK.set(dever_runtime::time::Clock::{clock}()).map_err(|_| AppError::fault({}, \"Job clock initialized twice\"))?;", emitter.location(span)).expect("string formatting");
}

pub(super) fn preflight(
    emitter: &Emitter<'_>,
    output: &mut String,
    instances: &std::collections::BTreeSet<crate::specialize::Specialization>,
    span: Span,
) {
    for target in targets(emitter.program, instances) {
        writeln!(
            output,
            "{};",
            emitter.database_failure(&format!("JOB_SPEC_{target}.validate()"), span)
        )
        .expect("string formatting");
        let (explicit, root) = emitter
            .program
            .database_selector(DatabaseOwner::Job(target));
        writeln!(output, "if dever_runtime::lifecycle::worker_enabled() {{ dever_runtime::config::settings().validate_job_connection({}, {}, JOB_SPEC_{target}.timeout_ms)?; }}", explicit.map(|name| format!("Some({})", rust_string(name))).unwrap_or_else(|| "None".into()), rust_string(root)).expect("string formatting");
    }
}

impl Emitter<'_> {
    pub(super) fn database_owner_at(&self, owner: DatabaseOwner) -> String {
        match owner {
            DatabaseOwner::Model(id) => self.database_at(&self.program.models[id]),
            DatabaseOwner::Job(function) => format!("_job_database_{function}.clone()"),
        }
    }

    pub(super) fn enqueue_job(
        &self,
        target: usize,
        arguments: &[Expression],
        scheduled: bool,
        span: Span,
    ) -> String {
        let job = self
            .program
            .jobs
            .iter()
            .find(|job| job.function == target)
            .expect("checked Job");
        let mut output = String::from("{");
        for (index, argument) in arguments.iter().enumerate() {
            write!(output, "let j{index} = {};", self.expression(argument))
                .expect("string formatting");
        }
        let payload = if job.payload.is_some() {
            self.failure(&format!("job_{target}_encode(&j0)"), span)
        } else {
            "dever_runtime::wire::Encoded::null()".into()
        };
        let key = usize::from(job.payload.is_some());
        let run_at = if scheduled {
            format!("j{}", key + 1)
        } else {
            self.database_failure("JOB_CLOCK.get().expect(\"clock initialized\").now()", span)
        };
        let database = self.database_failure(&format!("job_database_{target}().await"), span);
        let call = format!(
            "dever_runtime::job::enqueue({database}, _database, &JOB_SPEC_{target}, {payload}, &j{key}, {run_at}).await"
        );
        output.push_str(&self.database_failure(&call, span));
        output.push('}');
        output
    }
}
