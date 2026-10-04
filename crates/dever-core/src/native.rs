use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write;

use crate::hir::{self, Atom, Constant, Program, Projection, Statement};
use crate::source::{SourceMap, Span};
use crate::specialize::{self, Specialization};
use crate::types::{Parameter, Shape, Type, output_type};

mod api;
mod build;
mod collections;
mod expressions;
mod http;
mod intrinsics;
pub(crate) mod jobs;
pub(crate) mod liveness;
pub(crate) mod orm;
mod ports;
mod records;
mod rest;
mod sequences;
pub(crate) mod updates;
pub mod wire;
pub use build::{
    CleanupSummary, NativeProgram, OPTIMIZATION_ARGS, clean_project_artifacts, compile,
    compile_application, compile_artifact, compile_project, compile_project_with_resources,
    compile_test_suite, compile_with_cache,
};

struct Emitter<'a> {
    program: &'a Program,
    sources: &'a SourceMap,
    names: &'a BTreeMap<Specialization, String>,
    bindings: BTreeMap<usize, usize>,
    moves: HashSet<*const hir::Expression>,
    async_context: bool,
}

type EmittedDefinitions = (
    String,
    BTreeSet<Specialization>,
    BTreeMap<Specialization, String>,
);

/// Verified external Worker bytes supplied by the offline packager.  The core
/// compiler only serializes them into the generated native program; it does
/// not resolve registries or inspect a machine cache.
#[derive(Clone, Debug)]
pub struct EmbeddedResource {
    pub path: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub executable: bool,
}

#[derive(Clone, Copy)]
enum DatabaseContext {
    Inherit,
    Detached,
}

/// Generated Rust is a disposable backend artifact, not a public source format.
pub fn emit(program: &Program, sources: &SourceMap, entry: &str) -> Result<String, String> {
    emit_with_profile(program, sources, entry, None)
}

fn emit_with_profile(
    program: &Program,
    sources: &SourceMap,
    entry: &str,
    profile: Option<dever_runtime::config::RuntimeProfile>,
) -> Result<String, String> {
    let entry = program.entry(entry)?;
    emit_entry(program, sources, entry, profile)
}

pub fn emit_project(
    program: &Program,
    sources: &SourceMap,
    profile: dever_runtime::config::RuntimeProfile,
) -> Result<String, String> {
    emit_project_with_resources(program, sources, profile, &[])
}

pub fn emit_project_with_resources(
    program: &Program,
    sources: &SourceMap,
    profile: dever_runtime::config::RuntimeProfile,
    resources: &[EmbeddedResource],
) -> Result<String, String> {
    if !program.permissions.is_empty() && !profile.sqlite && !profile.postgres {
        return Err("protected API permissions require a configured database".into());
    }
    let serve_api = !program.api_routes.is_empty() || !program.api_rest.is_empty();
    let serve_jobs = !program.jobs.is_empty();
    if !serve_api && !serve_jobs && program.api_commands.is_empty() {
        return Err("application has no API, Job, or CMD entry".into());
    }
    let span = program
        .api_rest
        .first()
        .map(|entry| entry.span)
        .or_else(|| {
            program
                .api_routes
                .first()
                .map(|entry| program.functions[entry.function].span)
        })
        .or_else(|| {
            program
                .api_commands
                .first()
                .map(|entry| program.functions[entry.function].span)
        })
        .or_else(|| {
            program
                .jobs
                .first()
                .map(|entry| program.functions[entry.function].span)
        })
        .expect("application entry exists");
    let roots = program
        .api_routes
        .iter()
        .map(|route| route.function)
        .chain(program.api_commands.iter().map(|command| command.function))
        .chain(program.jobs.iter().map(|job| job.function))
        .chain(program.auth.iter().map(|hook| hook.function))
        .chain(program.rest_binding_roots())
        .map(|function| Specialization {
            function,
            handlers: Vec::new(),
        })
        .collect();
    let (mut output, instances, names) =
        emit_definitions(program, sources, roots, Some(profile), serve_api)?;
    let emitter = Emitter {
        program,
        sources,
        names: &names,
        bindings: BTreeMap::new(),
        moves: HashSet::new(),
        async_context: false,
    };
    api::emit_commands(program, &mut output);
    emit_command_tenant_scopes(program, &mut output);
    output.push_str("async fn entry() -> Result<(), AppError> {\nlet args = std::env::args().skip(1).collect::<Vec<_>>();\n");
    let cleanup_database = profile.sqlite || profile.postgres;
    let cleanup_components = ports::has_external(program, &instances);
    if cleanup_database || cleanup_components {
        output.push_str("let application: Result<(), AppError> = async {\n");
    }
    output.push_str("let service_start = args.is_empty();\n");
    if !resources.is_empty() {
        output.push_str("dever_runtime::external::prepare(&dever_runtime::config::executable_directory()?, DEVER_EXTERNAL_BUNDLE, DEVER_EXTERNAL_RESOURCES).map_err(|error| AppError::fault(\"external resources\", error))?;\n");
    }
    emit_database_setup(
        &emitter,
        &mut output,
        &instances,
        profile,
        span,
        Some((serve_api, serve_jobs)),
    );
    if !program.permissions.is_empty() {
        output.push_str("if service_start { authorization_initialize().await?; }\n");
    }
    output.push_str("if args.first().is_some_and(|argument| argument == \"--dever-tenant-migrate\") {\nif args.len() != 2 { return Err(AppError::fault(\"tenant migration\", \"expected --dever-tenant-migrate and one positive tenant id\")); }\nlet tenant_id = args[1].parse::<i64>().map_err(|_| AppError::fault(\"tenant migration\", \"tenant id must be a positive integer\"))?;\nif tenant_id <= 0 { return Err(AppError::fault(\"tenant migration\", \"tenant id must be positive\")); }\n");
    if profile.sqlite || profile.postgres {
        output.push_str("tenant_migrate(tenant_id).await?;\nreturn Ok(());\n");
    } else {
        output.push_str("return Err(AppError::fault(\"tenant migration\", \"application has no database runtime\"));\n");
    }
    output.push_str("}\n");
    if !program.permissions.is_empty() {
        output.push_str("if args.first().is_some_and(|argument| argument == \"--dever-tenant-owner\") {\nif args.len() != 4 { return Err(AppError::fault(\"tenant owner\", \"expected --dever-tenant-owner, one positive tenant id, one site, and one positive user id\")); }\nlet tenant_id = args[1].parse::<i64>().map_err(|_| AppError::fault(\"tenant owner\", \"tenant id must be a positive integer\"))?;\nlet user_id = args[3].parse::<i64>().map_err(|_| AppError::fault(\"tenant owner\", \"user id must be a positive integer\"))?;\nif tenant_id <= 0 || user_id <= 0 { return Err(AppError::fault(\"tenant owner\", \"tenant id and user id must be positive\")); }\ntenant_owner(tenant_id, &args[2], user_id).await?;\nreturn Ok(());\n}\n");
    }
    output.push_str("if args.first().is_some_and(|argument| argument == \"--dever-tenant-component\") {\nif args.len() != 4 { return Err(AppError::fault(\"tenant component\", \"expected --dever-tenant-component, one positive tenant id, enable or disable, and one component\")); }\nlet tenant_id = args[1].parse::<i64>().map_err(|_| AppError::fault(\"tenant component\", \"tenant id must be a positive integer\"))?;\nif tenant_id <= 0 { return Err(AppError::fault(\"tenant component\", \"tenant id must be positive\")); }\n");
    if program.tenant_components.is_empty() {
        output.push_str("return Err(AppError::fault(\"tenant component\", \"application has no tenant components\"));\n");
    } else {
        output.push_str("match args[2].as_str() {\n\"enable\" => dever_runtime::tenant::enable_component(tenant_id, &args[3], TENANT_COMPONENTS).await.map_err(|error| AppError::database(\"tenant component\", error))?,\n\"disable\" => dever_runtime::tenant::disable_component(tenant_id, &args[3], TENANT_COMPONENTS).await.map_err(|error| AppError::database(\"tenant component\", error))?,\n_ => return Err(AppError::fault(\"tenant component\", \"operation must be enable or disable\")),\n}\nreturn Ok(());\n");
    }
    output.push_str("}\n");
    ports::initialize(&emitter, &mut output, &instances, span)?;
    output.push_str("if service_start {\n");
    if serve_api || serve_jobs {
        let service = match (serve_api, serve_jobs) {
            (true, true) => {
                "dever_runtime::lifecycle::serve_both(dever_runtime::api::serve(api_route), job_serve()).await"
            }
            (true, false) => "dever_runtime::api::serve(api_route).await",
            (false, true) => "job_serve().await",
            (false, false) => unreachable!(),
        };
        writeln!(output, "dever_runtime::lifecycle::run(async move {{ {service}.map_err(AppError::from) }}).await?;")
            .expect("string formatting");
    } else {
        output.push_str("return Err(AppError::fault(\"command\", \"application has no service entry; select a CMD and JSON input\"));\n");
    }
    output.push_str("} else {\n");
    if !program.api_commands.is_empty() {
        output.push_str("let data = if args.first().is_some_and(|argument| argument == \"--dever-tenant-cmd\") {\n");
        output.push_str("if args.len() != 4 { return Err(AppError::fault(\"command\", \"expected --dever-tenant-cmd, one positive tenant id, CMD name, and one JSON object\")); }\n");
        output.push_str("let tenant_id = args[1].parse::<i64>().map_err(|_| AppError::fault(\"command\", \"tenant id must be a positive integer\"))?;\nif tenant_id <= 0 { return Err(AppError::fault(\"command\", \"tenant id must be a positive integer\")); }\n");
        output.push_str("match cmd_requires_tenant(&args[2]) {\nSome(true) => {},\nSome(false) => return Err(AppError::fault(\"command\", \"selected CMD does not use tenant storage; remove --tenant\")),\nNone => return Err(AppError::fault(\"command\", format!(\"unknown CMD '{}'\", args[2]))),\n}\n");
        output.push_str("if dever_runtime::config::settings().tenant().is_none() { return Err(AppError::fault(\"command\", \"tenant storage is not configured\")); }\n");
        if !cleanup_database {
            output.push_str("return Err(AppError::fault(\"command\", \"application has no tenant database runtime\"))\n");
        } else if program.jobs.is_empty() {
            output.push_str("dever_runtime::tenant::scope(tenant_id, cmd_dispatch(&args[2], &args[3])).await?\n");
        } else {
            output.push_str("dever_runtime::job::scope_system(Some(tenant_id), dever_runtime::tenant::scope(tenant_id, cmd_dispatch(&args[2], &args[3]))).await?\n");
        }
        output.push_str("} else {\n");
        output.push_str("if args.len() != 2 { return Err(AppError::fault(\"command\", \"expected CMD name and one JSON object\")); }\n");
        output.push_str("match cmd_requires_tenant(&args[0]) {\nSome(true) => return Err(AppError::fault(\"command\", \"selected CMD requires an explicit tenant; use dever run --tenant\")),\nSome(false) => {},\nNone => return Err(AppError::fault(\"command\", format!(\"unknown CMD '{}'\", args[0]))),\n}\n");
        if program.jobs.is_empty() {
            output.push_str("cmd_dispatch(&args[0], &args[1]).await?\n};\n");
        } else {
            output.push_str("dever_runtime::job::scope_system(None, cmd_dispatch(&args[0], &args[1])).await?\n};\n");
        }
        output.push_str("dever_runtime::println(&format!(\"{{\\\"code\\\":0,\\\"message\\\":\\\"ok\\\",\\\"data\\\":{}}}\", data.as_str())).map_err(|error| AppError::fault(\"command\", error))?;\n");
    } else {
        output.push_str(
            "return Err(AppError::fault(\"command\", \"application has no CMD entry\"));\n",
        );
    }
    output.push_str("}\nOk(())\n");
    if cleanup_database || cleanup_components {
        emit_cleanup(
            &emitter,
            &mut output,
            span,
            cleanup_database,
            cleanup_components,
        );
    }
    output.push_str("}\n");
    if !resources.is_empty() {
        output.push_str(&embedded_resource_definitions(resources)?);
    }
    output.push_str("fn main() -> std::process::ExitCode {\nmatch task::run_entry_typed(entry())");
    emit_main_result(&mut output);
    Ok(output)
}

fn embedded_resource_definitions(resources: &[EmbeddedResource]) -> Result<String, String> {
    validate_embedded_resources(resources)?;
    let digest = embedded_bundle_digest(resources);
    let inputs = resource_inputs(resources);
    let definitions = inputs
        .unique
        .iter()
        .enumerate()
        .map(|(index, _)| {
            format!(
                "static DEVER_EXTERNAL_BYTES_{index}: &[u8] = include_bytes!({});\n",
                rust_string(&resource_source_filename(index))
            )
        })
        .collect::<String>();
    let entries = resources.iter().zip(&inputs.indices).map(|(resource, index)| {
        format!(
            "dever_runtime::external::Resource {{ path: {}, bytes: DEVER_EXTERNAL_BYTES_{index}, sha256: {}, executable: {} }}",
            rust_string(&resource.path), rust_string(&resource.sha256), resource.executable,
        )
    }).collect::<Vec<_>>().join(", ");
    Ok(format!(
        "\n{definitions}static DEVER_EXTERNAL_RESOURCES: &[dever_runtime::external::Resource<'static>] = &[{entries}];\nconst DEVER_EXTERNAL_BUNDLE: &str = {};\n",
        rust_string(&digest)
    ))
}

pub(crate) fn validate_embedded_resources(resources: &[EmbeddedResource]) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    for resource in resources {
        let actual = Sha256::digest(&resource.bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if actual != resource.sha256 {
            return Err(format!(
                "external resource '{}' has an invalid digest",
                resource.path
            ));
        }
    }
    Ok(())
}

pub(crate) struct ResourceInputs<'a> {
    pub unique: Vec<&'a EmbeddedResource>,
    pub indices: Vec<usize>,
}

pub(crate) fn resource_inputs(resources: &[EmbeddedResource]) -> ResourceInputs<'_> {
    let mut by_digest = BTreeMap::new();
    let mut unique = Vec::new();
    let mut indices = Vec::with_capacity(resources.len());
    for resource in resources {
        let index = *by_digest
            .entry(resource.sha256.as_str())
            .or_insert_with(|| {
                let index = unique.len();
                unique.push(resource);
                index
            });
        indices.push(index);
    }
    ResourceInputs { unique, indices }
}

fn resource_source_filename(index: usize) -> String {
    format!("external-resource-{index}.bin")
}

pub(crate) fn embedded_bundle_digest(resources: &[EmbeddedResource]) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for resource in resources {
        digest.update(resource.path.as_bytes());
        digest.update([0]);
        digest.update(resource.sha256.as_bytes());
        digest.update([0]);
        digest.update((resource.bytes.len() as u64).to_le_bytes());
        digest.update([u8::from(resource.executable)]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn emit_entry(
    program: &Program,
    sources: &SourceMap,
    entry: usize,
    profile: Option<dever_runtime::config::RuntimeProfile>,
) -> Result<String, String> {
    let root = Specialization {
        function: entry,
        handlers: Vec::new(),
    };
    let serve_api = api_enabled(program, entry)?;
    let serve_jobs = job_enabled(program, entry)?;
    if serve_jobs && program.jobs.is_empty() {
        return Err("dever.job.serve() requires at least one Job declaration".into());
    }
    let roots = std::iter::once(root.clone())
        .chain(
            program
                .api_routes
                .iter()
                .filter(|_| serve_api)
                .map(|route| Specialization {
                    function: route.function,
                    handlers: Vec::new(),
                }),
        )
        .chain(
            program
                .jobs
                .iter()
                .filter(|_| serve_jobs)
                .map(|job| Specialization {
                    function: job.function,
                    handlers: Vec::new(),
                }),
        )
        .collect::<Vec<_>>();
    let (mut output, instances, names) =
        emit_definitions(program, sources, roots, profile, serve_api)?;
    let emitter = Emitter {
        program,
        sources,
        names: &names,
        bindings: BTreeMap::new(),
        moves: HashSet::new(),
        async_context: false,
    };
    let root_suspends = specialize::suspends(program, &root);
    let schema_suspends = profile
        .is_some_and(|profile| profile.postgres || (profile.sqlite && !program.models.is_empty()));
    let async_entry = root_suspends || schema_suspends;
    output.push_str(if async_entry {
        "async fn entry() -> Result<(), AppError> {\n"
    } else {
        "fn entry() -> Result<(), AppError> {\n"
    });
    let cleanup_database =
        async_entry && profile.is_some_and(|profile| profile.sqlite || profile.postgres);
    let cleanup_components = ports::has_external(program, &instances);
    if cleanup_database || cleanup_components {
        output.push_str("let application: Result<(), AppError> = async {\n");
    }
    ports::initialize(
        &emitter,
        &mut output,
        &instances,
        program.functions[entry].span,
    )?;
    if let Some(profile) = profile {
        emit_database_setup(
            &emitter,
            &mut output,
            &instances,
            profile,
            program.functions[entry].span,
            None,
        );
    }
    let invocation = entry_invocation(program, &names, &root);
    let invocation = if serve_api || serve_jobs {
        format!("dever_runtime::lifecycle::run(async move {{ {invocation} }}).await")
    } else {
        invocation
    };
    writeln!(output, "let result = {invocation}?;").expect("string formatting");
    for (index, field) in program.functions[entry].outputs.iter().enumerate() {
        let value = if program.functions[entry].outputs.len() == 1 {
            "result".into()
        } else {
            format!("result.{index}")
        };
        writeln!(
            output,
            "let mut rendered{index} = String::from({}); rendered{index}.push_str(\" = \"); {value}.render_to(&mut rendered{index}); rendered{index}.push('\\n'); resource::stdout_write(&rendered{index})?;",
            rust_string(&field.name),
        )
        .expect("string formatting");
    }
    output.push_str("Ok(())\n");
    if cleanup_database || cleanup_components {
        emit_cleanup(
            &emitter,
            &mut output,
            program.functions[entry].span,
            cleanup_database,
            cleanup_components,
        );
    }
    output.push_str("}\nfn main() -> std::process::ExitCode {\nmatch ");
    output.push_str(if async_entry {
        "task::run_entry_typed(entry())"
    } else {
        "entry()"
    });
    emit_main_result(&mut output);
    Ok(output)
}

fn emit_test_suite(
    program: &Program,
    sources: &SourceMap,
    profile: Option<dever_runtime::config::RuntimeProfile>,
) -> Result<String, String> {
    if program.tests.is_empty() {
        return Err("cannot compile an empty test suite".into());
    }
    let mut output = String::from("#![forbid(unsafe_code)]\n");
    for index in 0..program.tests.len() {
        let case = crate::check::test_program(program, index).map_err(|errors| {
            errors
                .iter()
                .map(|error| error.render(sources))
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        writeln!(output, "mod case_{index} {{").expect("string formatting");
        output.push_str(&emit_test_case(&case, sources, index, profile)?);
        output.push_str("}\n");
    }
    writeln!(output, "fn main() -> std::process::ExitCode {{\nmatch std::env::args().nth(1).and_then(|value| value.parse::<usize>().ok()) {{").expect("string formatting");
    for index in 0..program.tests.len() {
        writeln!(output, "Some({index}) => case_{index}::run(),").expect("string formatting");
    }
    output.push_str("_ => { eprintln!(\"test program requires a valid case index\"); std::process::ExitCode::FAILURE },\n}\n}\n");
    Ok(output)
}

fn emit_test_case(
    program: &Program,
    sources: &SourceMap,
    index: usize,
    profile: Option<dever_runtime::config::RuntimeProfile>,
) -> Result<String, String> {
    let test = &program.tests[index];
    let root = Specialization {
        function: test.function,
        handlers: Vec::new(),
    };
    let (mut output, instances, names) =
        emit_definitions(program, sources, vec![root.clone()], profile, false)?;
    let emitter = Emitter {
        program,
        sources,
        names: &names,
        bindings: BTreeMap::new(),
        moves: HashSet::new(),
        async_context: false,
    };
    let async_entry = specialize::suspends(program, &root)
        || profile.is_some_and(|profile| profile.sqlite && test.uses_database());
    output.push_str(if async_entry {
        "async fn entry() -> Result<(), AppError> {\n"
    } else {
        "fn entry() -> Result<(), AppError> {\n"
    });
    let cleanup_database = async_entry && test.uses_database();
    if cleanup_database {
        output.push_str("let application: Result<(), AppError> = async {\n");
    }
    jobs::initialize_clock(&emitter, &mut output, &instances, test.span);
    if let Some(profile) = profile.filter(|_| test.uses_database()) {
        emit_database_setup(&emitter, &mut output, &instances, profile, test.span, None);
    }
    let invocation = entry_invocation(program, &names, &root);
    if jobs::targets(program, &instances).is_empty() {
        writeln!(output, "let _ = {invocation}?;\nOk(())").expect("string formatting");
    } else {
        writeln!(output, "dever_runtime::job::scope_system(None, async {{ let _ = {invocation}?; Ok(()) }}).await")
            .expect("string formatting");
    }
    if cleanup_database {
        emit_cleanup(&emitter, &mut output, test.span, true, false);
    }
    output.push_str("}\n");
    writeln!(
        output,
        "pub fn run() -> std::process::ExitCode {{\nmatch {}",
        if async_entry {
            "task::run_entry_typed(entry())"
        } else {
            "entry()"
        },
    )
    .expect("string formatting");
    emit_main_result(&mut output);
    Ok(output)
}

fn emit_cleanup(
    emitter: &Emitter<'_>,
    output: &mut String,
    span: Span,
    database: bool,
    component: bool,
) {
    output.push_str("}.await;\nlet mut cleanup: Option<AppError> = None;\n");
    if component {
        writeln!(output, "if let Err(error) = dever_runtime::component::shutdown().await {{ cleanup = Some(AppError::fault({}, error)); }}", emitter.location(span)).expect("string formatting");
    }
    if database {
        writeln!(output, "if let Err(error) = dever_runtime::database::shutdown().await {{ cleanup = Some(match cleanup {{ Some(previous) => previous.cause(error), None => AppError::database({}, error) }}); }}", emitter.location(span)).expect("string formatting");
    }
    output.push_str("match (application, cleanup) {\n(Ok(()), None) => Ok(()),\n(Err(error), None) => Err(error),\n(Ok(()), Some(cleanup)) => Err(cleanup),\n(Err(error), Some(cleanup)) => Err(error.cause(cleanup)),\n}\n");
}

fn emit_definitions(
    program: &Program,
    sources: &SourceMap,
    roots: Vec<Specialization>,
    profile: Option<dever_runtime::config::RuntimeProfile>,
    serve_api: bool,
) -> Result<EmittedDefinitions, String> {
    let instances = specialize::execution_reachable(&program.functions, roots)
        .map_err(|_| "recursive specialized handler calls are not allowed".to_owned())?;
    let names: BTreeMap<_, _> = instances
        .iter()
        .cloned()
        .map(|instance| {
            let name = if instance.handlers.is_empty() {
                format!("f{}", instance.function)
            } else {
                format!(
                    "f{}h{}",
                    instance.function,
                    instance
                        .handlers
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join("h")
                )
            };
            (instance, name)
        })
        .collect();
    let emitter = Emitter {
        program,
        sources,
        names: &names,
        bindings: BTreeMap::new(),
        moves: HashSet::new(),
        async_context: false,
    };
    let mut output = String::from(
        "#![forbid(unsafe_code)]\n#![allow(dead_code, unused_mut, unused_variables, unused_parens)]\nuse dever_runtime::{number::{self, DecimalValue}, collections::{List, Map, MapEntry}, bytes::Bytes, channel, resource, render::Render, task};\n",
    );
    writeln!(
        output,
        "static TENANT_COMPONENTS: &[&str] = &[{}];",
        program
            .tenant_components
            .iter()
            .map(|component| rust_string(component))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .expect("string formatting");
    emitter.types(&mut output);
    if profile.is_some() {
        emitter.model_decoders(&mut output);
    }
    emitter.errors(&mut output);
    ports::definitions(&emitter, &mut output, &instances);
    jobs::definitions(&emitter, &mut output, &instances);
    jobs::tenant_migration(
        &emitter,
        &mut output,
        &instances,
        profile.is_some_and(|profile| profile.sqlite || profile.postgres),
    );
    for instance in &instances {
        let emitter = Emitter {
            program,
            sources,
            names: &names,
            bindings: specialize::handler_bindings(&program.functions[instance.function], instance),
            moves: HashSet::new(),
            async_context: specialize::suspends(program, instance),
        };
        emitter.function(&mut output, instance);
    }
    if serve_api {
        api::emit_routes(&emitter, &mut output);
    }
    Ok((output, instances, names))
}

fn emit_database_setup(
    emitter: &Emitter<'_>,
    output: &mut String,
    instances: &BTreeSet<Specialization>,
    profile: dever_runtime::config::RuntimeProfile,
    entry_span: Span,
    application_services: Option<(bool, bool)>,
) {
    let program = emitter.program;
    let mut bindings = program
        .models
        .iter()
        .map(|model| match &model.connection {
            crate::model::ConnectionSelector::Explicit(name) => (
                Some(name.as_str()),
                model.package.split('.').next().unwrap(),
                model.scope == crate::model::ModelScope::Tenant,
            ),
            crate::model::ConnectionSelector::PackageRoot(root) => (
                None,
                root.as_str(),
                model.scope == crate::model::ModelScope::Tenant,
            ),
        })
        .collect::<Vec<_>>();
    bindings.extend(jobs::targets(program, instances).into_iter().map(|target| {
        let (explicit, root) = program.database_selector(crate::model::DatabaseOwner::Job(target));
        (
            explicit,
            root,
            jobs::scope(program, target) == crate::model::ModelScope::Tenant,
        )
    }));
    bindings.sort_unstable();
    bindings.dedup();
    let bindings = bindings
        .iter()
        .map(|(explicit, root, tenant)| {
            format!(
                "({}, {}, {tenant})",
                explicit
                    .map(|name| format!("Some({})", rust_string(name)))
                    .unwrap_or_else(|| "None".into()),
                rust_string(root)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut transaction_roots = instances
        .iter()
        .filter(|instance| {
            program.functions[instance.function].kind == crate::syntax::FunctionKind::Transaction
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if application_services.is_some() {
        for function in program
            .api_routes
            .iter()
            .map(|route| route.function)
            .chain(program.api_commands.iter().map(|command| command.function))
            .chain(program.jobs.iter().map(|job| job.function))
        {
            if function_writes_database(program, function) {
                transaction_roots.insert(Specialization {
                    function,
                    handlers: Vec::new(),
                });
            }
        }
    }
    let transactions = transaction_roots
        .iter()
        .map(|instance| {
            let bindings = specialize::database_effects(program, instance)
                .iter()
                .map(|owner| {
                    let (explicit, root) = program.database_selector(*owner);
                    let tenant = match owner {
                        crate::model::DatabaseOwner::Model(index) => {
                            program.models[*index].scope == crate::model::ModelScope::Tenant
                        }
                        crate::model::DatabaseOwner::Job(target) => {
                            jobs::scope(program, *target) == crate::model::ModelScope::Tenant
                        }
                    };
                    (explicit, root, tenant)
                })
                .map(|(explicit, root, tenant)| {
                    format!(
                        "({}, {}, {tenant})",
                        explicit
                            .map(|name| format!("Some({})", rust_string(name)))
                            .unwrap_or_else(|| "None".into()),
                        rust_string(root)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("&[{bindings}]")
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "dever_runtime::config::bootstrap_scoped(dever_runtime::config::RuntimeProfile {{ sqlite: {}, postgres: {} }}, &[{bindings}], &[{transactions}])?;",
        profile.sqlite,
        profile.postgres,
    )
    .expect("string formatting");
    let serve_api = application_services.map_or_else(
        || {
            instances.iter().any(|instance| {
                function_starts_service(&program.functions[instance.function], false)
            })
        },
        |services| services.0,
    );
    let serve_jobs = application_services.map_or_else(
        || {
            instances.iter().any(|instance| {
                function_starts_service(&program.functions[instance.function], true)
            })
        },
        |services| services.1,
    );
    if serve_api || serve_jobs {
        if application_services.is_some() {
            output.push_str("if service_start {\n");
        }
        writeln!(
            output,
            "dever_runtime::lifecycle::configure({serve_api}, {serve_jobs})?;"
        )
        .expect("string formatting");
        if application_services.is_some() {
            output.push_str("}\n");
        }
    }
    jobs::preflight(emitter, output, instances, entry_span);
    if profile.postgres {
        let prepare =
            emitter.database_failure("dever_runtime::database::prepare().await", entry_span);
        writeln!(output, "{prepare};").expect("string formatting");
    }
    if profile.sqlite || profile.postgres {
        emitter.initialize_models(output);
        jobs::initialize(emitter, output, instances, entry_span);
    }
}

fn entry_invocation(
    program: &Program,
    names: &BTreeMap<Specialization, String>,
    root: &Specialization,
) -> String {
    let root_database = if specialize::database_effects(program, root).is_empty() {
        ""
    } else {
        "None"
    };
    if specialize::suspends(program, root) {
        format!("{}({root_database}).await", names[root])
    } else {
        format!("{}({root_database})", names[root])
    }
}

fn emit_main_result(output: &mut String) {
    output.push_str(" {\nOk(()) => { dever_runtime::log::flush(); std::process::ExitCode::SUCCESS },\nErr(error) => { dever_runtime::log::error(error.to_string(), vec![]); dever_runtime::log::flush(); std::process::ExitCode::FAILURE }\n}\n}\n");
}

fn api_enabled(program: &Program, entry: usize) -> Result<bool, String> {
    service_enabled(program, entry, false)
}

fn service_enabled(program: &Program, entry: usize, worker: bool) -> Result<bool, String> {
    let instances = specialize::reachable(
        &program.functions,
        [Specialization {
            function: entry,
            handlers: Vec::new(),
        }],
    )
    .map_err(|_| "recursive specialized handler calls are not allowed".to_owned())?;
    Ok(instances
        .iter()
        .any(|instance| function_starts_service(&program.functions[instance.function], worker)))
}

fn job_enabled(program: &Program, entry: usize) -> Result<bool, String> {
    service_enabled(program, entry, true)
}

fn function_starts_service(function: &crate::hir::Function, worker: bool) -> bool {
    let mut starts = false;
    crate::check::visit(function, |expression| {
        if let crate::hir::ExpressionKind::Intrinsic { operation, .. } = expression.kind {
            starts |= if worker {
                matches!(operation, crate::intrinsic::Intrinsic::JobServe)
            } else {
                matches!(operation, crate::intrinsic::Intrinsic::ApiServe)
            };
        }
    });
    starts
}

fn function_writes_database(program: &Program, function: usize) -> bool {
    let root = Specialization {
        function,
        handlers: Vec::new(),
    };
    specialize::writes_database(program, &root)
}

fn emit_command_tenant_scopes(program: &Program, output: &mut String) {
    if program.api_commands.is_empty() {
        return;
    }
    output.push_str("fn cmd_requires_tenant(name: &str) -> Option<bool> {\nmatch name {\n");
    for command in &program.api_commands {
        let tenant = !command.components.is_empty();
        writeln!(output, "{} => Some({tenant}),", rust_string(&command.name))
            .expect("string formatting");
    }
    output.push_str("_ => None,\n}\n}\n");
}

fn transaction_database(program: &Program, function: usize) -> Option<String> {
    if !function_writes_database(program, function) {
        return None;
    }
    let root = Specialization {
        function,
        handlers: Vec::new(),
    };
    let owner = specialize::database_effects(program, &root)
        .into_iter()
        .next()?;
    Some(match owner {
        crate::model::DatabaseOwner::Model(index) => {
            format!("model_database_{}().await", program.models[index].record,)
        }
        crate::model::DatabaseOwner::Job(index) => format!("job_database_{index}().await",),
    })
}

impl Emitter<'_> {
    fn function(&self, output: &mut String, instance: &Specialization) {
        let id = instance.function;
        let function = &self.program.functions[id];
        let suspends = specialize::suspends(self.program, instance);
        let mut parameters = function
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                parameter
                    .value_type()
                    .map(|ty| format!("_p{index}: {}", rust_type(ty)))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let database_effects = specialize::database_effects(self.program, instance);
        if !database_effects.is_empty() {
            if !parameters.is_empty() {
                parameters.push_str(", ");
            }
            parameters.push_str("_database: Option<&dever_runtime::database::Transaction>");
        }
        writeln!(
            output,
            "{}fn {}({parameters}) -> Result<{}, AppError> {{",
            if suspends { "async " } else { "" },
            self.names[instance],
            rust_type(&output_type(&function.outputs))
        )
        .expect("string formatting");
        if ports::function(self, output, instance) {
            return;
        }
        for owner in &database_effects {
            match owner {
                crate::model::DatabaseOwner::Model(index) => {
                    let model = &self.program.models[*index];
                    let database = self.database_failure(
                        &format!("model_database_{}().await", model.record),
                        function.span,
                    );
                    writeln!(output, "let _model_database_{} = {database};", model.record)
                        .expect("string formatting");
                }
                crate::model::DatabaseOwner::Job(target) => {
                    let database = self
                        .database_failure(&format!("job_database_{target}().await"), function.span);
                    writeln!(output, "let _job_database_{target} = {database};")
                        .expect("string formatting");
                }
            }
        }
        let transaction = function.kind == crate::syntax::FunctionKind::Transaction;
        if transaction {
            let owner = *database_effects
                .iter()
                .next()
                .expect("validated transaction database effect");
            let database = self.database_owner_at(owner);
            let begin = self.database_failure("transaction_database.begin().await", function.span);
            writeln!(
                output,
                "let transaction_owner = if _database.is_none() {{ let transaction_database = {database}; Some({begin}) }} else {{ None }};\nlet _database = _database.or(transaction_owner.as_ref());\nlet transaction_result: Result<{}, AppError> = async {{",
                rust_type(&output_type(&function.outputs))
            )
            .expect("string formatting");
        }
        for clause in &function.clauses {
            let mut optimized = clause.clone();
            self.inline_sequences(&mut optimized);
            updates::coalesce(&mut optimized, &self.program.types);
            updates::snapshot_scalars(&mut optimized, &self.program.types);
            let clause = &optimized;
            let clause_emitter = Emitter {
                program: self.program,
                sources: self.sources,
                names: self.names,
                bindings: self.bindings.clone(),
                moves: liveness::last_uses(clause),
                async_context: suspends,
            };
            let mut domains = clause.patterns.iter();
            let guard = function
                .parameters
                .iter()
                .enumerate()
                .filter_map(|(index, parameter)| {
                    let Parameter::Value(ty) = parameter else {
                        return None;
                    };
                    let domain = domains.next().expect("checked value parameter domain");
                    Some(format!(
                        "({})",
                        domain
                            .iter()
                            .map(|atom| self.atom(atom, ty, &format!("_p{index}")))
                            .collect::<Vec<_>>()
                            .join(" || ")
                    ))
                })
                .collect::<Vec<_>>()
                .join(" && ");
            debug_assert!(domains.next().is_none());
            writeln!(
                output,
                "if {} {{",
                if guard.is_empty() { "true" } else { &guard }
            )
            .expect("string formatting");
            for (slot, ty) in clause.locals.iter().enumerate() {
                writeln!(output, "let mut _v{slot}: {};", rust_type(ty))
                    .expect("string formatting");
            }
            for binding in &clause.bindings {
                self.binding(
                    output,
                    binding,
                    function.parameters[binding.input]
                        .value_type()
                        .expect("value binding"),
                    clause
                        .bindings
                        .iter()
                        .filter(|other| other.input == binding.input)
                        .count()
                        == 1,
                );
            }
            for statement in &clause.body {
                match statement {
                    Statement::Assign {
                        slot,
                        fields,
                        value,
                    } => {
                        let mut target = format!("_v{slot}");
                        let mut ty = clause.locals[*slot].clone();
                        for index in fields {
                            target.push_str(&field_suffix(&ty, *index));
                            ty = self.field_type(&ty, *index);
                        }
                        writeln!(output, "{target} = {};", clause_emitter.expression(value))
                            .expect("string formatting");
                    }
                    Statement::Call(value) => {
                        writeln!(output, "{};", clause_emitter.expression(value))
                            .expect("string formatting");
                    }
                }
            }
            if clause.terminates {
                writeln!(output, "}}\n").expect("string formatting");
                continue;
            }
            let result = match clause.outputs.as_slice() {
                [] => "()".into(),
                [slot] => format!("_v{slot}"),
                slots => format!(
                    "({})",
                    slots
                        .iter()
                        .map(|slot| format!("_v{slot}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            writeln!(output, "return Ok({result});\n}}").expect("string formatting");
        }
        output.push_str("unreachable!(\"checked exhaustive clauses\")\n");
        if transaction {
            writeln!(
                output,
                "}}.await;\nif let Some(transaction) = transaction_owner {{\nreturn match transaction_result {{\nOk(value) => {{ transaction.commit().await.map_err(|error| AppError::database({}, error))?; Ok(value) }},\nErr(error) => match transaction.rollback().await {{ Ok(()) => Err(error), Err(rollback) => Err(error.cause(rollback)) }}\n}};\n}}\ntransaction_result",
                self.location(function.span),
            )
            .expect("string formatting");
        }
        output.push_str("}\n");
    }

    fn invoke(
        &self,
        function: &Specialization,
        mut arguments: String,
        database: DatabaseContext,
    ) -> String {
        if !specialize::database_effects(self.program, function).is_empty() {
            if !arguments.is_empty() {
                arguments.push_str(", ");
            }
            arguments.push_str(match database {
                DatabaseContext::Inherit => "_database",
                DatabaseContext::Detached => "None",
            });
        }
        format!("{}({arguments})", self.names[function])
    }

    fn binding(&self, output: &mut String, binding: &hir::Binding, ty: &Type, unique: bool) {
        let input = format!("_p{}", binding.input);
        let value = match binding.projection {
            Projection::Whole if unique && ty.movable(&self.program.types) => input.clone(),
            Projection::Whole => format!("{input}.clone()"),
            Projection::NonNull if unique && ty.movable(&self.program.types) => {
                format!("{input}.expect(\"checked non-null clause\")")
            }
            Projection::NonNull => {
                format!("{input}.as_ref().expect(\"checked non-null clause\").clone()")
            }
            Projection::Payload { variant, field } => {
                if let Type::Related(_) = ty.base() {
                    debug_assert_eq!(variant, 1);
                    debug_assert_eq!(field, 0);
                    let input = if matches!(ty, Type::Nullable(_)) {
                        format!("{input}.as_ref().expect(\"checked non-null variant\")")
                    } else {
                        format!("&{input}")
                    };
                    writeln!(
                        output,
                        "_v{} = match {input} {{ dever_runtime::orm::Related::Loaded(payload) => payload.as_ref().clone(), _ => unreachable!(\"checked Related variant\") }};",
                        binding.slot,
                    )
                    .expect("string formatting");
                    return;
                }
                let Type::Named(id) = ty.base() else {
                    unreachable!()
                };
                let Shape::Choice(variants) = &self.program.types[*id].shape else {
                    unreachable!()
                };
                let fields = (0..variants[variant].fields.len())
                    .map(|index| if index == field { "payload" } else { "_" })
                    .collect::<Vec<_>>()
                    .join(", ");
                let input = if matches!(ty, Type::Nullable(_)) {
                    format!("{input}.as_ref().expect(\"checked non-null variant\")")
                } else {
                    format!("&{input}")
                };
                format!(
                    "match {input} {{ T{id}::V{variant}({fields}) => payload.clone(), _ => unreachable!(\"checked variant\") }}"
                )
            }
        };
        writeln!(output, "_v{} = {value};", binding.slot).expect("string formatting");
    }

    fn atom(&self, atom: &Atom, ty: &Type, value: &str) -> String {
        if matches!(atom, Atom::Null) {
            return format!("{value}.is_none()");
        }
        if let Type::Nullable(base) = ty {
            return format!(
                "{value}.as_ref().is_some_and(|_x| {})",
                self.atom(atom, base, "(*_x)")
            );
        }
        match atom {
            Atom::Any => "true".into(),
            Atom::Bool(boolean) => format!("{value} == {boolean}"),
            Atom::Variant(variant) => {
                if matches!(ty, Type::Related(_)) {
                    return format!(
                        "matches!({value}, dever_runtime::orm::Related::{})",
                        if *variant == 0 {
                            "Unloaded"
                        } else {
                            "Loaded(..)"
                        }
                    );
                }
                let Type::Named(id) = ty else { unreachable!() };
                let Shape::Choice(variants) = &self.program.types[*id].shape else {
                    unreachable!()
                };
                format!(
                    "matches!({value}, T{id}::V{variant}{})",
                    if variants[*variant].fields.is_empty() {
                        ""
                    } else {
                        "(..)"
                    }
                )
            }
            Atom::Int(lo, hi) => range_guard(
                value,
                (*lo != i64::MIN).then(|| format!("{lo}i64")),
                (*hi != i64::MAX).then(|| format!("{hi}i64")),
            ),
            Atom::Decimal(lo, hi) => range_guard(
                value,
                (*lo != dever_runtime::number::DecimalValue::minimum())
                    .then(|| constant(&Constant::Decimal(*lo))),
                (*hi != dever_runtime::number::DecimalValue::maximum())
                    .then(|| constant(&Constant::Decimal(*hi))),
            ),
            Atom::Literal(literal) => format!("{value} == {}", typed_constant(literal, ty)),
            Atom::Remainder(literals) => {
                if literals.is_empty() {
                    "true".into()
                } else {
                    literals
                        .iter()
                        .map(|literal| format!("{value} != {}", typed_constant(literal, ty)))
                        .collect::<Vec<_>>()
                        .join(" && ")
                }
            }
            Atom::Null => unreachable!(),
        }
    }

    fn field_type(&self, ty: &Type, index: usize) -> Type {
        match ty {
            Type::Named(id) => match &self.program.types[*id].shape {
                Shape::Record(fields) => fields[index].ty.clone(),
                _ => unreachable!(),
            },
            Type::Outputs(fields) => fields[index].ty.clone(),
            Type::MapEntry(key, value) => {
                if index == 0 {
                    *key.clone()
                } else {
                    *value.clone()
                }
            }
            _ => unreachable!("checked field access"),
        }
    }

    fn failure(&self, expression: &str, span: Span) -> String {
        format!(
            "({expression}).map_err(|error| AppError::fault({}, error))?",
            self.location(span)
        )
    }

    fn database_failure(&self, expression: &str, span: Span) -> String {
        format!(
            "({expression}).map_err(|error| AppError::database({}, error))?",
            self.location(span)
        )
    }

    fn call_failure(&self, expression: &str, span: Span) -> String {
        format!(
            "({expression}).map_err(|error| error.at({}))?",
            self.location(span)
        )
    }

    fn location(&self, span: Span) -> String {
        let source = self.sources.get(span.source);
        let (line, column) = source.position(span.start);
        let location = format!("{}:{line}:{column}", source.path().display());
        rust_string(&location)
    }
}

// 类型已保证整个 Int/有限 Decimal 域，只有分句实际缩小的边界才需运行时比较。
fn range_guard(value: &str, low: Option<String>, high: Option<String>) -> String {
    let checks: Vec<_> = low
        .into_iter()
        .map(|low| format!("{value} >= {low}"))
        .chain(high.into_iter().map(|high| format!("{value} <= {high}")))
        .collect();
    if checks.is_empty() {
        "true".into()
    } else {
        checks.join(" && ")
    }
}

fn optional_result(expression: &str, value: &Type) -> String {
    if matches!(value, Type::Nullable(_)) {
        format!("({expression}).flatten()")
    } else {
        expression.into()
    }
}

fn rust_type(ty: &Type) -> String {
    match ty {
        Type::Unit => "()".into(),
        Type::Bool => "bool".into(),
        Type::Int => "i64".into(),
        Type::Decimal => "DecimalValue".into(),
        Type::Float => "f64".into(),
        Type::Text => "String".into(),
        Type::Id => "dever_runtime::Id".into(),
        Type::Bytes => "Bytes".into(),
        Type::Secret => "dever_runtime::secret::Secret".into(),
        Type::Upload => "dever_runtime::api::upload::Upload".into(),
        Type::Uuid => "dever_runtime::orm::Uuid".into(),
        Type::Json => "String".into(),
        Type::DateTime | Type::Date | Type::Time | Type::Duration => "i64".into(),
        Type::File => "resource::FileHandle".into(),
        Type::Socket => "dever_runtime::net::Socket".into(),
        Type::Listener => "dever_runtime::net::Listener".into(),
        Type::HttpReply => "dever_runtime::http::HttpReply".into(),
        Type::WebSocket => "dever_runtime::websocket::WebSocket".into(),
        Type::ClientTls => "dever_runtime::tls::ClientTls".into(),
        Type::ServerTls => "dever_runtime::tls::ServerTls".into(),
        Type::HttpClient => "dever_runtime::http::HttpClient".into(),
        Type::Stream(element) => format!("resource::Stream<{}>", rust_type(element)),
        Type::AsyncStream(element) => format!(
            "dever_runtime::async_stream::AsyncStream<{}>",
            rust_type(element)
        ),
        Type::RowStream(element) => {
            format!("dever_runtime::orm::RowStream<{}>", rust_type(element))
        }
        Type::Related(element) => format!("dever_runtime::orm::Related<{}>", rust_type(element)),
        Type::Channel(element) => format!("channel::Channel<{}>", rust_type(element)),
        Type::Task(outputs) => {
            format!("task::Task<{}, AppError>", rust_type(&output_type(outputs)))
        }
        Type::Group => "task::Group<AppError>".into(),
        Type::Named(id) => format!("T{id}"),
        Type::Nullable(base) => format!("Option<{}>", rust_type(base)),
        Type::List(base) => format!("List<{}>", rust_type(base)),
        Type::Map(key, value) => format!("Map<{}, {}>", rust_type(key), rust_type(value)),
        Type::MapEntry(key, value) => format!("MapEntry<{}, {}>", rust_type(key), rust_type(value)),
        Type::Outputs(fields) => format!(
            "({})",
            fields
                .iter()
                .map(|field| rust_type(&field.ty))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn field_suffix(ty: &Type, index: usize) -> String {
    match ty {
        Type::Outputs(_) => format!(".{index}"),
        Type::MapEntry(_, _) => {
            if index == 0 {
                ".key".into()
            } else {
                ".value".into()
            }
        }
        _ => format!(".f{index}"),
    }
}

fn constant(value: &Constant) -> String {
    match value {
        Constant::Null => "None".into(),
        Constant::Bool(value) => value.to_string(),
        Constant::Int(value) => format!("{value}i64"),
        Constant::Decimal(value) => format!(
            "DecimalValue::from_bytes({:?}).expect(\"checked Decimal literal\")",
            value.to_bytes()
        ),
        Constant::Float(value) => format!("f64::from_bits({}u64)", value.to_bits()),
        Constant::Text(value) => format!("String::from({})", rust_string(value)),
    }
}

fn typed_constant(value: &Constant, ty: &Type) -> String {
    if let (Constant::Text(value), Type::Uuid) = (value, ty) {
        let bytes = dever_runtime::orm::Uuid::parse(value)
            .expect("checked Uuid literal")
            .into_bytes();
        format!("dever_runtime::orm::Uuid::from_bytes({bytes:?})")
    } else {
        constant(value)
    }
}

fn rust_string(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .chars()
            .flat_map(char::escape_default)
            .collect::<String>()
    )
}
