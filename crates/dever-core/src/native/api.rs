use std::fmt::Write;

use crate::hir::{ApiRoute, Program};
use crate::specialize::{self, Specialization};
use crate::wire::{Node, Policy, Schema};

use super::rust_string;

pub(super) fn emit_routes(emitter: &super::Emitter<'_>, output: &mut String) {
    let program = emitter.program;
    emit_authorization(program, output);
    let mut codecs = std::collections::BTreeMap::new();
    let mut route_codecs = Vec::new();
    for (route_index, route) in program.api_routes.iter().enumerate() {
        let schema = Schema::from_field(
            &program.functions[route.function].outputs[0],
            &program.types,
            Policy::Output,
        )
        .expect("checked API wire schema");
        let next = codecs.len();
        let index = *codecs
            .entry(schema.fingerprint().to_owned())
            .or_insert_with(|| {
                output.push_str(&super::wire::emit(&schema, next));
                next
            });
        route_codecs.push(index);
        for (input_index, parameter) in program.functions[route.function]
            .parameters
            .iter()
            .enumerate()
        {
            if parameter.value_type() == Some(&crate::types::Type::Upload) {
                continue;
            }
            let schema = Schema::build(
                parameter.value_type().expect("checked API input"),
                &program.types,
                Policy::ApiInput,
            )
            .expect("checked API input schema");
            output.push_str(&super::wire::emit_named(
                &schema,
                &format!("api_input_{route_index}_{input_index}"),
            ));
        }
    }
    if !program.api_rest.is_empty() {
        super::rest::emit_helpers(program, output);
        super::rest::emit_database_error(output);
    }
    emit_failure_mapper(program, output);
    output.push_str("async fn api_route(mut request: dever_runtime::http::Request) -> Result<dever_runtime::http::Response, String> {\n");
    output.push_str("let path = request.target.split('?').next().unwrap_or(\"\").to_owned();\nmatch (request.method.as_str(), path.as_str()) {\n");
    for (route_index, (route, codec)) in program.api_routes.iter().zip(route_codecs).enumerate() {
        writeln!(
            output,
            "({}, {}) => {{",
            rust_string(route.method),
            rust_string(&route.path)
        )
        .expect("string formatting");
        emit_auth_begin(
            program,
            route.auth,
            &route.directory,
            route.anonymous,
            output,
        );
        emit_components(&route.components, output);
        emit_permission(program, route.permission, output);
        emit_handler(program, route, route_index, codec, output);
        emit_auth_end(route.auth, output);
        output.push_str("},\n");
    }
    for rest in &program.api_rest {
        for method in ["GET", "POST"] {
            writeln!(
                output,
                "({}, {}) => {{",
                rust_string(method),
                rust_string(&rest.path)
            )
            .expect("string formatting");
            emit_auth_begin(program, rest.auth, &rest.directory, false, output);
            emit_components(&rest.components, output);
            emit_permission(program, rest_permission(rest, method), output);
            emit_read_body(output);
            super::rest::emit_handler(emitter, rest, method, false, output);
            emit_auth_end(rest.auth, output);
            output.push_str("},\n");
        }
    }
    output.push_str("(_, path) => {\n");
    let mut paths = std::collections::BTreeMap::<&str, Vec<&str>>::new();
    for route in &program.api_routes {
        paths.entry(&route.path).or_default().push(route.method);
    }
    for rest in &program.api_rest {
        paths.entry(&rest.path).or_default().extend(["GET", "POST"]);
    }
    for rest in &program.api_rest {
        writeln!(
            output,
            "if path.starts_with({}) {{ return match request.method.as_str() {{",
            rust_string(&format!("{}/", rest.path))
        )
        .expect("string formatting");
        for method in ["GET", "PUT", "DELETE"] {
            writeln!(output, "{} => {{", rust_string(method)).expect("string formatting");
            emit_auth_begin(program, rest.auth, &rest.directory, false, output);
            emit_components(&rest.components, output);
            emit_permission(program, rest_permission(rest, method), output);
            emit_read_body(output);
            super::rest::emit_handler(emitter, rest, method, true, output);
            emit_auth_end(rest.auth, output);
            output.push_str("},\n");
        }
        output.push_str("_ => Ok(dever_runtime::api::method_not_allowed(&[\"GET\", \"PUT\", \"DELETE\"])),\n}; }\n");
    }
    output.push_str("match path {\n");
    for (path, methods) in paths {
        writeln!(
            output,
            "{} => Ok(dever_runtime::api::method_not_allowed(&{:?})),",
            rust_string(path),
            methods
        )
        .expect("string formatting");
    }
    output.push_str("_ => Ok(dever_runtime::api::not_found()),\n}\n}\n}\n}\n");
}

fn emit_authorization(program: &Program, output: &mut String) {
    if program.permissions.is_empty() {
        return;
    }
    output.push_str(
        "static AUTHORIZATION_PERMISSIONS: &[dever_runtime::auth::store::Permission] = &[\n",
    );
    for permission in &program.permissions {
        writeln!(output,
            "dever_runtime::auth::store::Permission {{ key: {}, component: {}, domain: {}, site: {}, action: {}, method: {} }},",
            rust_string(&permission.key),
            rust_string(&permission.component),
            rust_string(&permission.domain),
            rust_string(&permission.site),
            rust_string(&permission.action),
            rust_string(permission.method),
        ).expect("string formatting");
    }
    output.push_str(r#"];
fn authorization_control_database() -> Result<dever_runtime::database::Database, dever_runtime::orm::Error> {
let settings = dever_runtime::config::settings();
let connection = settings.tenant().map(|tenant| tenant.database()).unwrap_or("default");
dever_runtime::database::database_for(Some(connection), "")
}
async fn authorization_initialize() -> Result<(), AppError> {
let database = authorization_control_database().map_err(|error| AppError::database("authorization initialization", error))?;
dever_runtime::auth::store::sync_catalog(database.clone(), AUTHORIZATION_PERMISSIONS).await.map_err(|error| AppError::database("permission catalog", error))?;
dever_runtime::auth::store::initialize_roles(database).await.map_err(|error| AppError::database("authorization roles", error))
}
async fn tenant_owner(tenant_id: i64, site: &str, user_id: i64) -> Result<(), AppError> {
let settings = dever_runtime::config::settings();
if !settings.sites().contains_key(site) { return Err(AppError::fault("tenant owner", format!("unknown site '{site}'"))); }
let tenant = settings.tenant().ok_or_else(|| AppError::fault("tenant owner", "tenant storage is not configured"))?;
let connection = tenant.database().to_owned();
dever_runtime::tenant::scope(tenant_id, async move {
let database = dever_runtime::tenant::database(&connection).await.map_err(|error| AppError::database("tenant owner", error))?;
dever_runtime::auth::store::initialize_roles(database.clone()).await.map_err(|error| AppError::database("tenant owner", error))?;
dever_runtime::auth::store::provision_owner(database, user_id, site).await.map_err(|error| AppError::database("tenant owner", error))?;
Ok(())
}).await
}
async fn api_require_components(components: &[&str]) -> Result<(), dever_runtime::auth::Error> {
if components.is_empty() { return Ok(()); }
let tenant_id = dever_runtime::auth::tenant_id()?.ok_or(dever_runtime::auth::Error::Forbidden)?;
match dever_runtime::tenant::require_components(tenant_id, components, TENANT_COMPONENTS).await {
Ok(()) => Ok(()),
Err(error) if error.kind() == dever_runtime::orm::ErrorKind::InvalidData => Err(dever_runtime::auth::Error::Forbidden),
Err(error) => { dever_runtime::log::error(error.to_string(), Vec::new()); Err(dever_runtime::auth::Error::Internal) }
}
}
async fn api_authorize(permission: &'static str, site: &'static str) -> Result<(), dever_runtime::auth::Error> {
dever_runtime::auth::authorize_permission(permission, site).await
}
"#);
}

fn emit_permission(program: &Program, permission: Option<usize>, output: &mut String) {
    let Some(permission) = permission.map(|id| &program.permissions[id]) else {
        return;
    };
    writeln!(output,
        "if let Err(error) = api_authorize({}, {}).await {{ return Ok(dever_runtime::api::auth_error(error)); }};",
        rust_string(&permission.key),
        rust_string(&permission.site),
    ).expect("string formatting");
}

fn emit_components(components: &[String], output: &mut String) {
    if components.is_empty() {
        return;
    }
    writeln!(output,
        "if let Err(error) = api_require_components(&[{}]).await {{ return Ok(dever_runtime::api::auth_error(error)); }};",
        components.iter().map(|component| rust_string(component)).collect::<Vec<_>>().join(", "),
    ).expect("string formatting");
}

fn rest_permission(route: &crate::hir::ApiRest, method: &str) -> Option<usize> {
    match method {
        "GET" => route.permissions.read,
        "POST" => route.permissions.create,
        "PUT" => route.permissions.replace,
        "DELETE" => route.permissions.delete,
        _ => unreachable!("checked REST method"),
    }
}

fn emit_failure_mapper(program: &Program, output: &mut String) {
    output.push_str("fn api_failure_response(error: AppError) -> dever_runtime::http::Response {\nmatch &error.kind {\n");
    if let Some((error_id, definition)) = program
        .types
        .iter()
        .enumerate()
        .find(|(_, definition)| definition.name == "dever.api.Error")
    {
        let crate::types::Shape::Choice(variants) = &definition.shape else {
            unreachable!("official dever.api.Error is a choice")
        };
        let failures = program
            .failures
            .iter()
            .flat_map(|failures| failures.iter())
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        for (variant, status) in [
            ("Invalid", 400),
            ("Unauthorized", 401),
            ("Forbidden", 403),
            ("NotFound", 404),
            ("Conflict", 409),
            ("TooManyRequests", 429),
        ] {
            let index = variants
                .iter()
                .position(|candidate| candidate.name == variant)
                .expect("official dever.api.Error variant");
            if failures.contains(&crate::hir::Failure {
                ty: error_id,
                variant: index,
            }) {
                writeln!(output, "AppErrorKind::E{error_id}V{index} => dever_runtime::api::standard_error({status}),")
                    .expect("string formatting");
            }
        }
    }
    output.push_str("_ => { dever_runtime::log::error(\"API handler failed\", Vec::new()); dever_runtime::api::internal_error() }\n}\n}\n");
}

fn emit_auth_begin(
    program: &Program,
    auth: Option<usize>,
    directory: &[String],
    anonymous: bool,
    output: &mut String,
) {
    let Some(auth) = auth else { return };
    let hook = &program.auth[auth];
    let directory = directory
        .iter()
        .map(|part| rust_string(part))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(output,
        "let auth_prepared = match dever_runtime::auth::prepare(&[{directory}], &request, {anonymous}) {{ Ok(prepared) => prepared, Err(error) => return Ok(dever_runtime::api::auth_error(error)), }};"
    ).expect("string formatting");
    writeln!(output,
        "let auth_identity = match auth_prepared.claims() {{\nSome(claims) => {{\nlet auth_claims = T{} {{ f0: claims.subject.clone(), f1: claims.session.clone(), f2: claims.tenant.clone(), f3: claims.site.clone() }};",
        hook.claims,
    ).expect("string formatting");
    let target = Specialization {
        function: hook.function,
        handlers: Vec::new(),
    };
    let mut arguments = vec!["auth_claims".to_owned()];
    if !specialize::database_effects(program, &target).is_empty() {
        arguments.push("None".to_owned());
    }
    let call = format!(
        "f{}({}){}",
        hook.function,
        arguments.join(", "),
        if specialize::suspends(program, &target) {
            ".await"
        } else {
            ""
        },
    );
    writeln!(output,
        "let identity: T{} = match {call} {{ Ok(identity) => identity, Err(error) => return Ok(api_failure_response(error)), }};\nSome(dever_runtime::auth::Identity {{ id: identity.f0, user_id: identity.f1.map(|value| value.0), tenant_id: identity.f2.map(|value| value.0) }})\n}},\nNone => None,\n}};\nlet auth_response = dever_runtime::auth::scope(auth_prepared, auth_identity, async move {{",
        hook.identity,
    ).expect("string formatting");
}

fn emit_auth_end(auth: Option<usize>, output: &mut String) {
    if auth.is_some() {
        output.push_str("}).await;\nmatch auth_response { Ok(response) => response, Err(error) => Ok(dever_runtime::api::auth_error(error)), }\n");
    }
}

fn emit_handler(
    program: &Program,
    route: &ApiRoute,
    route_index: usize,
    codec: usize,
    output: &mut String,
) {
    let function = &program.functions[route.function];
    if route.multipart {
        output.push_str("let mut inputs = match dever_runtime::api::upload::read_multipart(&request).await { Ok(inputs) => inputs, Err(error) => return Ok(dever_runtime::api::invalid_input(error)), };\n");
    } else {
        emit_read_body(output);
        output.push_str("let mut inputs = match dever_runtime::api::Inputs::from_request(&request) {\nOk(inputs) => inputs, Err(error) => return Ok(dever_runtime::api::invalid_input(error)),\n};\n");
    }
    for (index, (name, parameter)) in route.inputs.iter().zip(&function.parameters).enumerate() {
        if parameter.value_type() == Some(&crate::types::Type::Upload) {
            writeln!(output, "let p{index} = match inputs.take({}) {{ Ok(upload) => upload, Err(error) => return Ok(dever_runtime::api::invalid_input(error)), }};", rust_string(name)).expect("string formatting");
            continue;
        }
        let schema = Schema::build(
            parameter.value_type().expect("checked API value input"),
            &program.types,
            Policy::ApiInput,
        )
        .expect("checked API input schema");
        let kind = schema.query_kind();
        let read = if matches!(schema.nodes[schema.root], Node::Nullable(_)) {
            "optional_json"
        } else {
            "json"
        };
        writeln!(output,
            "let raw_{index} = match inputs.{read}({}, dever_runtime::api::QueryValue::{kind}) {{ Ok(value) => value, Err(error) => return Ok(dever_runtime::api::invalid_input(error)), }};\nlet p{index} = match api_input_{route_index}_{index}_decode(&raw_{index}) {{ Ok(value) => value, Err(error) => return Ok(dever_runtime::api::invalid_input(dever_runtime::api::InputError(error))), }};",
            rust_string(name),
        ).expect("string formatting");
    }
    output.push_str("if let Err(error) = inputs.finish() { return Ok(dever_runtime::api::invalid_input(error)); }\n");
    let target = Specialization {
        function: route.function,
        handlers: Vec::new(),
    };
    let password_crypto = specialize::uses_intrinsic(
        program,
        &target,
        crate::intrinsic::Intrinsic::CryptoPasswordVerify,
    ) || specialize::uses_intrinsic(
        program,
        &target,
        crate::intrinsic::Intrinsic::CryptoPasswordHash,
    );
    let transaction_database = (!password_crypto)
        .then(|| super::transaction_database(program, route.function))
        .flatten();
    if let Some(database) = &transaction_database {
        writeln!(output, "let transaction_database = match {database} {{ Ok(database) => database, Err(error) => {{ dever_runtime::log::error(error.to_string(), Vec::new()); return Ok(dever_runtime::api::internal_error()); }} }};\nlet transaction = match transaction_database.begin().await {{ Ok(value) => value, Err(error) => {{ dever_runtime::log::error(error.to_string(), Vec::new()); return Ok(dever_runtime::api::internal_error()); }} }};")
            .expect("string formatting");
    }
    let mut arguments = (0..route.inputs.len())
        .map(|index| format!("p{index}"))
        .collect::<Vec<_>>();
    if !specialize::database_effects(program, &target).is_empty() {
        arguments.push(
            if transaction_database.is_some() {
                "Some(&transaction)"
            } else {
                "None"
            }
            .to_owned(),
        );
    }
    let call = format!(
        "f{}({}){}",
        route.function,
        arguments.join(", "),
        if specialize::suspends(program, &target) {
            ".await"
        } else {
            ""
        }
    );
    let failure_mapper = "api_failure_response";
    if transaction_database.is_some() {
        writeln!(output, "let response = match {call} {{ Ok(value) => value, Err(error) => {{ if transaction.rollback().await.is_err() {{ dever_runtime::log::error(\"API rollback failed\", Vec::new()); }} return Ok({failure_mapper}(error)); }} }};")
            .expect("string formatting");
    } else {
        writeln!(output, "let response = match {call} {{ Ok(value) => value, Err(error) => return Ok({failure_mapper}(error)), }};")
            .expect("string formatting");
    }
    let conversion = format!("wire_{codec}_encode(&response)");
    writeln!(output,
        "let value = match {conversion} {{ Ok(value) => value, Err(_error) => {{ {} dever_runtime::log::error(\"API response serialization failed\", Vec::new()); return Ok(dever_runtime::api::internal_error()); }} }};",
        if transaction_database.is_some() { "let _ = transaction.rollback().await;" } else { "" },
    ).expect("string formatting");
    if transaction_database.is_some() {
        output.push_str("if let Err(error) = transaction.commit().await { dever_runtime::log::error(error.to_string(), Vec::new()); return Ok(dever_runtime::api::internal_error()); }\n");
    }
    output.push_str("if let Err(error) = dever_runtime::api::commit_response_metadata() { dever_runtime::log::error(error, Vec::new()); return Ok(dever_runtime::api::internal_error()); }\nOk(dever_runtime::api::success(value))");
}

fn emit_read_body(output: &mut String) {
    output.push_str("if let Err(error) = dever_runtime::api::upload::read_body(&mut request).await { return Ok(dever_runtime::api::invalid_input(error)); }\n");
}

pub(super) fn emit_commands(program: &Program, output: &mut String) {
    if program.api_commands.is_empty() {
        return;
    }
    for (command_index, command) in program.api_commands.iter().enumerate() {
        let function = &program.functions[command.function];
        let schema = Schema::from_field(&function.outputs[0], &program.types, Policy::Output)
            .expect("checked CMD output schema");
        output.push_str(&super::wire::emit_named(
            &schema,
            &format!("cmd_output_{command_index}"),
        ));
        for (input_index, parameter) in function.parameters.iter().enumerate() {
            let schema = Schema::build(
                parameter.value_type().expect("checked CMD input"),
                &program.types,
                Policy::ApiInput,
            )
            .expect("checked CMD input schema");
            output.push_str(&super::wire::emit_named(
                &schema,
                &format!("cmd_input_{command_index}_{input_index}"),
            ));
        }
    }
    output.push_str("async fn cmd_dispatch(name: &str, body: &str) -> Result<dever_runtime::wire::Encoded, AppError> {\nmatch name {\n");
    for (command_index, command) in program.api_commands.iter().enumerate() {
        let function = &program.functions[command.function];
        writeln!(output, "{} => {{", rust_string(&command.name)).expect("string formatting");
        if !command.components.is_empty() {
            writeln!(output,
                "let tenant_id = dever_runtime::tenant::current_id().map_err(|error| AppError::database(\"command component\", error))?;\ndever_runtime::tenant::require_components(tenant_id, &[{}], TENANT_COMPONENTS).await.map_err(|error| AppError::database(\"command component\", error))?;",
                command.components.iter().map(|component| rust_string(component)).collect::<Vec<_>>().join(", "),
            ).expect("string formatting");
        }
        output.push_str("let mut inputs = dever_runtime::api::Inputs::from_json_object(body).map_err(|error| AppError::fault(\"command\", error))?;\n");
        for (input_index, name) in command.inputs.iter().enumerate() {
            let parameter = &function.parameters[input_index];
            let schema = Schema::build(
                parameter.value_type().expect("checked CMD input"),
                &program.types,
                Policy::ApiInput,
            )
            .expect("checked CMD input schema");
            let read = if matches!(schema.nodes[schema.root], Node::Nullable(_)) {
                "optional_raw_json"
            } else {
                "raw_json"
            };
            let absent = if read == "optional_raw_json" {
                ".unwrap_or_else(|| \"null\".into())"
            } else {
                ""
            };
            writeln!(output,
                "let raw_{input_index} = inputs.{read}({}).map_err(|error| AppError::fault(\"command\", error))?{absent};\nlet p{input_index} = cmd_input_{command_index}_{input_index}_decode(&raw_{input_index}).map_err(|error| AppError::fault(\"command\", error))?;",
                rust_string(name),
            ).expect("string formatting");
        }
        output.push_str("inputs.finish().map_err(|error| AppError::fault(\"command\", error))?;\n");
        let target = Specialization {
            function: command.function,
            handlers: Vec::new(),
        };
        let transaction_database = super::transaction_database(program, command.function);
        if let Some(database) = &transaction_database {
            writeln!(output, "let transaction_database = ({database}).map_err(|error| AppError::database(\"command\", error))?;\nlet transaction = transaction_database.begin().await.map_err(|error| AppError::database(\"command\", error))?;")
                .expect("string formatting");
        }
        let mut arguments = (0..command.inputs.len())
            .map(|index| format!("p{index}"))
            .collect::<Vec<_>>();
        if !specialize::database_effects(program, &target).is_empty() {
            arguments.push(
                if transaction_database.is_some() {
                    "Some(&transaction)"
                } else {
                    "None"
                }
                .into(),
            );
        }
        writeln!(
            output,
            "let result = f{}({}){};",
            command.function,
            arguments.join(", "),
            if specialize::suspends(program, &target) {
                ".await"
            } else {
                ""
            },
        )
        .expect("string formatting");
        if transaction_database.is_some() {
            output.push_str("let value = match result { Ok(value) => value, Err(error) => { return Err(match transaction.rollback().await { Ok(()) => error, Err(rollback) => error.cause(rollback) }); } };\n");
        } else {
            output.push_str("let value = result?;\n");
        }
        if transaction_database.is_some() {
            writeln!(output, "let encoded = match cmd_output_{command_index}_encode(&value) {{ Ok(encoded) => encoded, Err(error) => {{ let rollback = transaction.rollback().await; return Err(match rollback {{ Ok(()) => AppError::fault(\"command\", error), Err(rollback) => AppError::fault(\"command\", error).cause(rollback) }}); }} }};")
                .expect("string formatting");
        } else {
            writeln!(output, "let encoded = cmd_output_{command_index}_encode(&value).map_err(|error| AppError::fault(\"command\", error))?;")
                .expect("string formatting");
        }
        if transaction_database.is_some() {
            output.push_str("transaction.commit().await.map_err(|error| AppError::database(\"command\", error))?;\n");
        }
        output.push_str("Ok(encoded)\n");
        output.push_str("},\n");
    }
    output.push_str(
        "_ => Err(AppError::fault(\"command\", format!(\"unknown CMD '{name}'\"))),\n}\n}\n",
    );
}
