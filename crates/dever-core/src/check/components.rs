use std::collections::BTreeSet;

use crate::diagnostic::Diagnostic;
use crate::hir::Program;
use crate::model::{DatabaseOwner, ModelScope};
use crate::specialize::{self, Specialization};

pub(super) fn register(program: &mut Program, errors: &mut Vec<Diagnostic>) {
    let mut tenant_components = program
        .models
        .iter()
        .filter(|model| model.scope == ModelScope::Tenant)
        .filter_map(|model| model.package.split('.').next().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    for job in &program.jobs {
        if job_uses_tenant_storage(program, job.function)
            && let Some(component) = function_component(program, job.function)
        {
            tenant_components.insert(component.to_owned());
        }
    }
    program.tenant_components = tenant_components.iter().cloned().collect();

    let route_components = program
        .api_routes
        .iter()
        .map(|route| reachable_components(program, route.function, &tenant_components))
        .collect::<Vec<_>>();
    for (route, components) in program.api_routes.iter_mut().zip(route_components) {
        if route.anonymous && !components.is_empty() {
            errors.push(Diagnostic::error(
                "C014",
                format!(
                    "public API cannot reach tenant components [{}]; tenant access requires an authenticated API",
                    components.join(", ")
                ),
                route.span,
            ));
        }
        route.components = components;
    }

    let command_components = program
        .api_commands
        .iter()
        .map(|command| reachable_components(program, command.function, &tenant_components))
        .collect::<Vec<_>>();
    for (command, components) in program.api_commands.iter_mut().zip(command_components) {
        command.components = components;
    }

    for rest in &mut program.api_rest {
        let component = program.models[rest.model]
            .package
            .split('.')
            .next()
            .filter(|component| tenant_components.contains(*component))
            .map(str::to_owned);
        rest.components = component.into_iter().collect();
    }

    let job_components = program
        .jobs
        .iter()
        .map(|job| reachable_components(program, job.function, &tenant_components))
        .collect::<Vec<_>>();
    for (job, components) in program.jobs.iter_mut().zip(job_components) {
        job.components = components;
    }
}

fn reachable_components(
    program: &Program,
    function: usize,
    tenant_components: &BTreeSet<String>,
) -> Vec<String> {
    let root = Specialization {
        function,
        handlers: Vec::new(),
    };
    let Ok(reachable) = specialize::execution_reachable(&program.functions, [root]) else {
        return Vec::new();
    };
    let mut components = BTreeSet::new();
    for instance in reachable {
        if let Some(component) = function_component(program, instance.function)
            && tenant_components.contains(component)
        {
            components.insert(component.to_owned());
        }
        for owner in &program.database_effects[instance.function] {
            match owner {
                DatabaseOwner::Model(model)
                    if program.models[*model].scope == ModelScope::Tenant =>
                {
                    if let Some(component) = program.models[*model].package.split('.').next() {
                        components.insert(component.to_owned());
                    }
                }
                DatabaseOwner::Job(job) if job_uses_tenant_storage(program, *job) => {
                    if let Some(component) = function_component(program, *job) {
                        components.insert(component.to_owned());
                    }
                }
                _ => {}
            }
        }
    }
    components.into_iter().collect()
}

fn job_uses_tenant_storage(program: &Program, function: usize) -> bool {
    program.database_effects[function].iter().any(|owner| {
        matches!(owner, DatabaseOwner::Model(model) if program.models[*model].scope == ModelScope::Tenant)
    })
}

fn function_component(program: &Program, function: usize) -> Option<&str> {
    program.packages[program.functions[function].owner]
        .domain
        .as_ref()
        .map(|(component, _)| component.as_str())
}
