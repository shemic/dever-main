use std::collections::BTreeMap;

use crate::diagnostic::Diagnostic;
use crate::hir::{
    ApiCommand, ApiRest, ApiRoute, CallTarget, CollectionOp, ExpressionKind, Function,
    HandlerTarget, Program,
};
use crate::model::{ModelFieldType, RestSearch, RestValueSource, RestWriteField};
use crate::source::{SourceLayout, SourceRole, Span};
use crate::syntax::{
    ApiBinding, ApiKind, Declaration, FunctionClause, InputKind, Package, Pattern,
};
use crate::types::{Definition, Parameter, Type};
use crate::wire::{Policy, Schema};

pub(super) struct Entries {
    pub routes: Vec<ApiRoute>,
    pub commands: Vec<ApiCommand>,
    pub rest: Vec<ApiRest>,
}

pub(super) fn validate_get_effects(program: &Program, errors: &mut Vec<Diagnostic>) {
    let writes: BTreeMap<_, _> = program
        .functions
        .iter()
        .enumerate()
        .filter_map(|(id, function)| {
            let mut write = None;
            super::visit(function, |expression| {
                let writes_database = match &expression.kind {
                    ExpressionKind::ModelOperation { operation, .. } => operation.writes_database(),
                    ExpressionKind::JobEnqueue { .. } => true,
                    ExpressionKind::Intrinsic { operation, .. } => authorization_write(*operation),
                    _ => false,
                };
                if writes_database {
                    write.get_or_insert(expression.span);
                }
            });
            write.map(|span| (id, span))
        })
        .collect();
    for route in program
        .api_routes
        .iter()
        .filter(|route| route.method == "GET")
    {
        let root = crate::specialize::Specialization {
            function: route.function,
            handlers: Vec::new(),
        };
        let Ok(reachable) = crate::specialize::reachable(&program.functions, [root]) else {
            continue;
        };
        if let Some(write) = reachable
            .iter()
            .find_map(|instance| writes.get(&instance.function))
        {
            let mut error = Diagnostic::error(
                "C013",
                "GET App call chains cannot write database or authorization state or enqueue a Job",
                program.functions[route.function].span,
            );
            error.related.push(crate::diagnostic::Label {
                span: *write,
                message: "persistent write reachable from this GET binding".into(),
            });
            errors.push(error);
        }
    }
}

pub(super) fn validate_rest_bindings(program: &Program, errors: &mut Vec<Diagnostic>) {
    for route in &program.api_rest {
        let model_domain = program.models[route.model]
            .package
            .split('.')
            .take(2)
            .collect::<Vec<_>>()
            .join(".");
        for expression in route
            .contract
            .create
            .iter()
            .chain(&route.contract.replace)
            .filter_map(|write| match &write.source {
                RestValueSource::Binding { expression, .. } => Some(expression),
                RestValueSource::Input => None,
            })
            .chain(
                route
                    .contract
                    .search
                    .iter()
                    .map(|search| &search.expression),
            )
        {
            super::visit_expression(expression, |value| match &value.kind {
                ExpressionKind::Call {
                    target: CallTarget::Function(target),
                    ..
                } => {
                    if !rest_function_allowed(program, *target, &model_domain) {
                        errors.push(Diagnostic::error("C013", "REST field bindings call only pure same-domain Domain functions or pure standard library functions", value.span));
                    }
                }
                ExpressionKind::Call { .. }
                | ExpressionKind::ModelOperation { .. }
                | ExpressionKind::JobEnqueue { .. }
                | ExpressionKind::RunCall { .. }
                | ExpressionKind::AwaitTask(_)
                | ExpressionKind::StopTask(_)
                | ExpressionKind::Group(_)
                | ExpressionKind::AwaitGroup(_)
                | ExpressionKind::StopGroup(_)
                | ExpressionKind::ParallelCall { .. }
                | ExpressionKind::BlockingCall { .. }
                | ExpressionKind::Channel { .. }
                | ExpressionKind::ChannelSend { .. }
                | ExpressionKind::ChannelReceive(_)
                | ExpressionKind::ChannelClose(_)
                | ExpressionKind::CaptureResult { .. } => {
                    errors.push(Diagnostic::error("C013", "REST field bindings cannot perform database, Job, handler, task, concurrency, or captured-failure operations", value.span));
                }
                ExpressionKind::Intrinsic { operation, .. }
                    if !rest_intrinsic_allowed(*operation) =>
                {
                    errors.push(Diagnostic::error(
                        "C013",
                        "REST field binding uses a forbidden system capability",
                        value.span,
                    ));
                }
                ExpressionKind::Collection {
                    operation: CollectionOp::ParallelEach,
                    ..
                } => {
                    errors.push(Diagnostic::error(
                        "C013",
                        "REST field bindings cannot start concurrent collection work",
                        value.span,
                    ));
                }
                ExpressionKind::Collection {
                    handler: Some(HandlerTarget::Function(target)),
                    ..
                } if !rest_function_allowed(program, *target, &model_domain) => {
                    errors.push(Diagnostic::error("C013", "REST field binding collection handlers must be pure same-domain Domain functions or pure standard library functions", value.span));
                }
                ExpressionKind::Collection {
                    handler: Some(HandlerTarget::Parameter(_)),
                    ..
                } => {
                    errors.push(Diagnostic::error(
                        "C013",
                        "REST field bindings cannot use unconstrained handlers",
                        value.span,
                    ));
                }
                _ => {}
            });
        }
    }
}

fn rest_function_allowed(program: &Program, target: usize, model_domain: &str) -> bool {
    let function = &program.functions[target];
    let package = &program.packages[function.owner];
    let same_domain = package
        .domain
        .as_ref()
        .is_some_and(|(component, domain)| format!("{component}.{domain}") == model_domain);
    let context = package.bundled
        && matches!(
            function.name.as_str(),
            "dever.api.request_id"
                | "dever.api.method"
                | "dever.api.path"
                | "dever.api.client_address"
                | "dever.api.header"
                | "dever.api.cookie"
                | "dever.api.secret_cookie"
                | "dever.auth.id"
                | "dever.auth.session"
                | "dever.auth.user_id"
                | "dever.auth.tenant_id"
                | "dever.site.key"
        );
    context
        || (package.bundled && program.effects[target].is_empty())
        || (!package.bundled
            && package.role == Some(SourceRole::Domain)
            && same_domain
            && function.pure
            && program.effects[target].is_empty())
}

fn rest_intrinsic_allowed(operation: crate::intrinsic::Intrinsic) -> bool {
    use crate::intrinsic::Intrinsic::*;
    matches!(
        operation,
        CryptoPasswordVerify
            | CryptoSha256
            | CryptoHmacSha256
            | CryptoConstantTimeEq
            | TextTrim
            | TextLower
            | TextUpper
            | TextCodepoint
            | TextFromCodepoint
            | TextAt
            | TextSlice
            | TextIndexOf
            | TextContains
            | TextStartsWith
            | TextEndsWith
            | TextSplit
            | TextReplace
            | IntParse
            | IntToText
            | DecimalParse
            | DecimalToText
            | DecimalFromInt
            | DecimalRound
            | FloatParse
            | FloatToText
            | FloatFromInt
            | FloatSqrt
            | FloatSin
            | FloatCos
            | FloatLog
            | FloatPow
            | FloatIsNan
            | FloatIsFinite
            | FloatIsInfinite
            | IdFromText
            | IdToText
            | UuidParse
            | UuidToText
            | BytesFromText
            | BytesToText
            | BytesFromInts
            | BytesLength
            | BytesAt
            | BytesSlice
            | BytesConcat
            | TimeParseDateTime
            | TimeFormatDateTime
            | TimeParseDate
            | TimeFormatDate
            | TimeParseTime
            | TimeFormatTime
            | TimeDuration
            | TimeAdd
            | TimeSubtract
            | TimeDifference
            | ApiRequestId
            | ApiMethod
            | ApiPath
            | ApiClientAddress
            | ApiHeader
            | ApiCookie
            | ApiSecretCookie
            | AuthId
            | AuthSession
            | AuthUserId
            | AuthTenantId
            | SiteKey
    )
}

pub(super) fn validate_write_transactions(program: &Program, errors: &mut Vec<Diagnostic>) {
    let write_routes = program
        .api_routes
        .iter()
        .filter(|route| route.method != "GET")
        .map(|route| route.function)
        .collect::<std::collections::BTreeSet<_>>();
    for function in &write_routes {
        let root = crate::specialize::Specialization {
            function: *function,
            handlers: Vec::new(),
        };
        let password_crypto = crate::specialize::uses_intrinsic(
            program,
            &root,
            crate::intrinsic::Intrinsic::CryptoPasswordVerify,
        ) || crate::specialize::uses_intrinsic(
            program,
            &root,
            crate::intrinsic::Intrinsic::CryptoPasswordHash,
        );
        if password_crypto
            && let Some(span) = crate::specialize::write_outside_transaction(program, &root)
        {
            let mut error = Diagnostic::error(
                "C014",
                "password crypto API writes must be enclosed in an explicit transaction function",
                program.functions[*function].span,
            );
            error.related.push(crate::diagnostic::Label {
                span,
                message: "database write occurs outside the short transaction boundary".into(),
            });
            errors.push(error);
        }
    }
    let roots = write_routes
        .into_iter()
        .chain(program.api_commands.iter().map(|command| command.function))
        .chain(program.jobs.iter().map(|job| job.function))
        .collect::<std::collections::BTreeSet<_>>();
    for function in roots {
        let root = crate::specialize::Specialization {
            function,
            handlers: Vec::new(),
        };
        if crate::specialize::writes_database(program, &root)
            && crate::specialize::starts_transaction_unsafe_work(program, &root)
        {
            errors.push(Diagnostic::error(
                "C014",
                "an API, CMD, or Job database transaction cannot start concurrent work",
                program.functions[function].span,
            ));
        }
    }
}

pub(super) fn validate_context_roots(program: &Program, errors: &mut Vec<Diagnostic>) {
    for route in program.api_routes.iter().filter(|route| route.anonymous) {
        let root = crate::specialize::Specialization {
            function: route.function,
            handlers: Vec::new(),
        };
        let enqueues = crate::specialize::reachable(&program.functions, [root.clone()]).is_ok_and(
            |reachable| {
                reachable.iter().any(|instance| {
                    let mut found = false;
                    super::visit(&program.functions[instance.function], |expression| {
                        found |= matches!(expression.kind, ExpressionKind::JobEnqueue { .. });
                    });
                    found
                })
            },
        );
        if enqueues {
            errors.push(Diagnostic::error(
                "C006",
                "public API call chains cannot enqueue a Job without a trusted user identity",
                route.span,
            ));
        }
        let administers_authorization = crate::specialize::reachable(&program.functions, [root])
            .is_ok_and(|reachable| {
                reachable.iter().any(|instance| {
                    let mut found = false;
                    super::visit(&program.functions[instance.function], |expression| {
                        found |= matches!(
                            expression.kind,
                            ExpressionKind::Intrinsic { operation, .. }
                                if authorization_administration(operation)
                        );
                    });
                    found
                })
            });
        if administers_authorization {
            errors.push(Diagnostic::error(
                "C006",
                "public API call chains cannot administer authorization",
                route.span,
            ));
        }
    }
    let roots = program
        .api_commands
        .iter()
        .map(|command| command.function)
        .chain(program.jobs.iter().map(|job| job.function))
        .collect::<std::collections::BTreeSet<_>>();
    for function in roots {
        let root = crate::specialize::Specialization {
            function,
            handlers: Vec::new(),
        };
        let request = crate::specialize::has_effect(program, &root, "request");
        let identity = crate::specialize::has_effect(program, &root, "identity");
        if request || identity {
            errors.push(Diagnostic::error(
                "C006",
                if request {
                    "HTTP request capabilities are not available from CMD or Job entries"
                } else {
                    "HTTP identity and site capabilities are not available from CMD or Job entries"
                },
                program.functions[function].span,
            ));
        }
    }
}

fn authorization_write(operation: crate::intrinsic::Intrinsic) -> bool {
    use crate::intrinsic::Intrinsic::*;
    matches!(
        operation,
        AuthSaveRole | AuthGrantRole | AuthRevokeRole | AuthDisableRole
    )
}

fn authorization_administration(operation: crate::intrinsic::Intrinsic) -> bool {
    matches!(operation, crate::intrinsic::Intrinsic::AuthPermissions)
        || authorization_write(operation)
}

pub(super) fn validate_context_detachment(program: &Program, errors: &mut Vec<Diagnostic>) {
    let roots = program
        .api_routes
        .iter()
        .map(|route| route.function)
        .chain(program.api_commands.iter().map(|command| command.function))
        .chain(program.jobs.iter().map(|job| job.function))
        .collect::<std::collections::BTreeSet<_>>();
    let Ok(instances) = crate::specialize::reachable(
        &program.functions,
        roots
            .into_iter()
            .map(|function| crate::specialize::Specialization {
                function,
                handlers: Vec::new(),
            }),
    ) else {
        return;
    };
    let mut detached = Vec::new();
    for instance in instances {
        let bindings =
            crate::specialize::handler_bindings(&program.functions[instance.function], &instance);
        super::visit(&program.functions[instance.function], |expression| {
            let target = match &expression.kind {
                ExpressionKind::RunCall { target, .. }
                | ExpressionKind::ParallelCall { target, .. }
                | ExpressionKind::BlockingCall { target, .. } => Some(match target {
                    CallTarget::Function(function) => *function,
                    CallTarget::Handler(handler) => {
                        crate::specialize::resolve_handler(*handler, &bindings)
                    }
                }),
                ExpressionKind::Collection {
                    operation: CollectionOp::ParallelEach,
                    handler: Some(handler),
                    ..
                } => Some(match handler {
                    HandlerTarget::Function(function) => *function,
                    HandlerTarget::Parameter(parameter) => bindings[parameter],
                }),
                _ => None,
            };
            if target.is_some_and(|function| {
                program.effects[function].contains("request")
                    || program.effects[function].contains("identity")
            }) && !detached.contains(&expression.span)
            {
                detached.push(expression.span);
            }
        });
    }
    for span in detached {
        errors.push(Diagnostic::error(
            "C006",
            "HTTP request, identity, and site capabilities cannot be used inside detached or parallel work",
            span,
        ));
    }
}

pub(super) fn register(
    packages: &[Package],
    symbols: &super::Symbols,
    functions: &[Function],
    types: &[Definition],
    models: &[crate::model::ModelSchema],
    rest_sources: &[Vec<super::model::RestFieldSource>],
    errors: &mut Vec<Diagnostic>,
) -> Entries {
    let mut entries = Entries {
        routes: legacy_routes(packages, functions, types, errors),
        commands: Vec::new(),
        rest: Vec::new(),
    };
    let mut routes: BTreeMap<_, _> = entries
        .routes
        .iter()
        .map(|route| {
            (
                (route.method, route.path.clone()),
                functions[route.function].span,
            )
        })
        .collect();
    let mut commands = BTreeMap::new();
    for package in packages {
        let SourceLayout::Role {
            component,
            domain,
            role: SourceRole::Api,
            topics,
        } = &package.layout
        else {
            continue;
        };
        let domain_name = format!("{component}.{domain}");
        let directory = topics[..topics.len().saturating_sub(1)].to_vec();
        let prefix = std::iter::once(component.as_str())
            .chain(std::iter::once(domain.as_str()))
            .chain(topics.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("/");
        for declaration in &package.declarations {
            match declaration {
                Declaration::Api(binding) => {
                    let Some((id, inputs)) = binding_target(
                        binding, &domain_name, packages, functions, types, errors,
                    ) else { continue; };
                    if let Some(method) = binding.kind.method() {
                        let path = format!("/{prefix}/{}", binding.action.text);
                        if occupy(&mut routes, (method, path.clone()), binding.span, "HTTP route", errors) {
                            entries.routes.push(ApiRoute { method, path, function: id, inputs,
                                directory: directory.clone(), anonymous: binding.anonymous,
                                component: component.clone(), domain: domain.clone(),
                                action: binding.action.text.clone(), auth: None, permission: None,
                                components: Vec::new(),
                                span: binding.span,
                                multipart: functions[id].parameters.iter().any(|parameter| parameter.value_type() == Some(&Type::Upload)) });
                        }
                    } else {
                        let name = format!("{domain_name}.{}", binding.action.text);
                        if occupy(&mut commands, name.clone(), binding.span, "command", errors) {
                            entries.commands.push(ApiCommand { name, function: id, inputs, components: Vec::new() });
                        }
                    }
                }
                Declaration::Rest(rest) => {
                    let selected = rest.model.as_ref()
                        .map(|selector| format!("{domain_name}.{}", super::path_name(selector)));
                    let candidates: Vec<_> = models.iter().enumerate()
                        .filter(|(_, model)| {
                            let model_package = &packages[types[model.record].owner];
                            model_package.layout.domain() == package.layout.domain()
                                && selected.as_ref().is_none_or(|name| model.package == *name)
                        })
                        .collect();
                    let resolved = candidates.first().filter(|(_, schema)| {
                        candidates.len() == 1 && (selected.is_some()
                            || schema.package == format!("{domain_name}.model"))
                    });
                    let Some((model, schema)) = resolved else {
                        errors.push(Diagnostic::error(
                            "C005",
                            "rest requires one same-domain Model; use 'rest model' or 'rest model.topic' to select it explicitly",
                            rest.span,
                        ));
                        continue;
                    };
                    let mut contract = match schema.rest_contract(types) {
                        Ok(contract) => contract,
                        Err(reason) => { errors.push(Diagnostic::error("C014", reason, rest.span)); continue; }
                    };
                    let checker_owner = packages.iter().enumerate().find_map(|(owner, candidate)| {
                        (candidate.layout.role() == Some(SourceRole::Domain)
                            && candidate.layout.domain() == package.layout.domain()).then_some(owner)
                    }).unwrap_or(types[schema.record].owner);
                    let context = super::Context { symbols, types, functions, models, owner: checker_owner };
                    let mut warnings = Vec::new();
                    for source in &rest_sources[*model] {
                        let field = &schema.fields[source.field];
                        if (field.generated || field.owner.is_some())
                            && (source.create.is_some() || source.replace.is_some() || source.search.is_some())
                        {
                            errors.push(Diagnostic::error("C014", "generated and owner fields cannot declare REST bindings", source.span));
                            continue;
                        }
                        let expected = &match &types[schema.record].shape {
                            crate::types::Shape::Record(fields) => &fields[source.field].ty,
                            _ => unreachable!(),
                        };
                        for (expression, writes, label) in [
                            (source.create.as_ref(), &mut contract.create, "create"),
                            (source.replace.as_ref(), &mut contract.replace, "replace"),
                        ] {
                            let Some(expression) = expression else { continue };
                            match super::body::expression(expression, expected, expected, &context, &mut warnings) {
                                Ok(expression) => {
                                    let uses_input = expression_uses_input(&expression);
                                    if source.private && uses_input {
                                        errors.push(Diagnostic::error("C014", format!("private REST {label} binding cannot use request input"), source.span));
                                        continue;
                                    }
                                    writes.retain(|write| write.field != source.field);
                                    writes.push(RestWriteField { field: source.field, source: RestValueSource::Binding { expression, uses_input } });
                                }
                                Err(error) => errors.push(error),
                            }
                        }
                        if let Some(expression) = &source.search {
                            let valid_type = matches!(field.ty,
                                ModelFieldType::Bool | ModelFieldType::Int | ModelFieldType::Float
                                | ModelFieldType::Decimal { .. } | ModelFieldType::Text { .. }
                                | ModelFieldType::Uuid | ModelFieldType::DateTime | ModelFieldType::Date
                                | ModelFieldType::Time | ModelFieldType::Duration
                                | ModelFieldType::ModelId(_));
                            if matches!(field.name.as_str(), "page" | "size") {
                                errors.push(Diagnostic::error("C014", "REST search field cannot use the reserved page or size query name", source.span));
                            } else if source.private || field.nullable || !valid_type {
                                errors.push(Diagnostic::error("C014", "REST search requires one public, non-null, indexable scalar field", source.span));
                            } else if contract.search.is_some() {
                                errors.push(Diagnostic::error("C014", "REST supports at most one search field", source.span));
                            } else {
                                match super::body::expression(expression, expected, expected, &context, &mut warnings) {
                                    Ok(expression) if expression_uses_input(&expression) => {
                                        contract.search = Some(RestSearch { field: source.field, expression });
                                    }
                                    Ok(_) => errors.push(Diagnostic::error("C014", "REST search binding must use input", source.span)),
                                    Err(error) => errors.push(error),
                                }
                            }
                        }
                    }
                    for (index, field) in schema.fields.iter().enumerate() {
                        if source_private(schema, types, index)
                            && !field.nullable && !field.generated && field.default.is_none() && field.owner.is_none()
                            && !contract.create.iter().any(|write| write.field == index)
                        {
                            errors.push(Diagnostic::error("C014", format!("REST Model '{}.{}' is private and required, but has no generated, default, owner, or create binding value", schema.package, field.name), rest.span));
                        }
                    }
                    let path = format!("/{prefix}");
                    let mut valid = true;
                    for (method, suffix) in [
                        ("GET", ""), ("GET", "/{id}"), ("POST", ""),
                        ("PUT", "/{id}"), ("DELETE", "/{id}"),
                    ] {
                        valid &= occupy(&mut routes, (method, format!("{path}{suffix}")), rest.span, "HTTP route", errors);
                    }
                    if valid {
                        entries.rest.push(ApiRest {
                            path,
                            model: *model,
                            span: rest.span,
                            directory: directory.clone(),
                            component: component.clone(),
                            domain: domain.clone(),
                            auth: None,
                            permissions: Default::default(),
                            components: Vec::new(),
                            contract,
                        });
                    }
                }
                _ => errors.push(Diagnostic::error("C005", "API role files accept only rest and get/post/put/delete/cmd App bindings; handler function bodies and legacy prefixes are not allowed", crate::markdown::declaration_span(declaration))),
            }
        }
    }
    entries
        .routes
        .sort_by(|left, right| (&left.path, left.method).cmp(&(&right.path, right.method)));
    entries
        .commands
        .sort_by(|left, right| left.name.cmp(&right.name));
    entries
        .rest
        .sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

fn source_private(schema: &crate::model::ModelSchema, types: &[Definition], index: usize) -> bool {
    let crate::types::Shape::Record(fields) = &types[schema.record].shape else {
        unreachable!()
    };
    fields[index].private
}

fn expression_uses_input(expression: &crate::hir::Expression) -> bool {
    let mut uses = false;
    super::visit_expression(expression, |value| {
        uses |= matches!(value.kind, ExpressionKind::Local(0))
    });
    uses
}

fn occupy<K: Ord>(
    occupied: &mut BTreeMap<K, Span>,
    key: K,
    span: Span,
    kind: &str,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if let Some(previous) = occupied.insert(key, span) {
        let mut error = Diagnostic::error("C002", format!("duplicate {kind}"), span);
        error.related.push(crate::diagnostic::Label {
            span: previous,
            message: format!("first {kind} declaration"),
        });
        errors.push(error);
        return false;
    }
    true
}

fn binding_target(
    binding: &ApiBinding,
    domain: &str,
    packages: &[Package],
    functions: &[Function],
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) -> Option<(usize, Vec<String>)> {
    let target = match binding.target.as_slice() {
        [app, target] if app.text == "app" => target,
        _ => {
            errors.push(Diagnostic::error(
                "C006",
                "API bindings must target app.<function> in the same domain",
                binding.span,
            ));
            return None;
        }
    };
    let name = format!("{domain}.{}", target.text);
    let matches: Vec<_> = functions
        .iter()
        .enumerate()
        .filter(|(_, function)| {
            function.name == name && packages[function.owner].layout.role() == Some(SourceRole::App)
        })
        .collect();
    let [(id, function)] = matches.as_slice() else {
        errors.push(Diagnostic::error(
            "C004",
            "API binding requires one unambiguous same-domain App function",
            binding.span,
        ));
        return None;
    };
    if function.outputs.len() != 1 {
        errors.push(Diagnostic::error(
            "C005",
            "API/CMD bindings require exactly one JSON-serializable App output",
            binding.span,
        ));
        return None;
    }
    if let Err(reason) = Schema::from_field(&function.outputs[0], types, Policy::Output) {
        errors.push(Diagnostic::error(
            "C005",
            format!("API/CMD output is not JSON-serializable: {reason}"),
            binding.span,
        ));
    }
    let clauses: Vec<&FunctionClause> = packages[function.owner]
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Function(clause) if clause.name.text == target.text => Some(clause),
            _ => None,
        })
        .collect();
    let inputs: Vec<_> = clauses[0]
        .inputs
        .iter()
        .map(|input| input.name.text.clone())
        .collect();
    for clause in &clauses {
        if clause
            .inputs
            .iter()
            .map(|input| &input.name.text)
            .ne(inputs.iter())
        {
            errors.push(Diagnostic::error(
                "C005",
                "API-bound App clauses must use identical input names",
                clause.span,
            ));
        }
    }
    for parameter in &function.parameters {
        let Parameter::Value(ty) = parameter else {
            errors.push(Diagnostic::error(
                "C005",
                "API/CMD inputs cannot be handler parameters",
                binding.span,
            ));
            continue;
        };
        if ty == &Type::Upload {
            if binding.kind != ApiKind::Post {
                errors.push(Diagnostic::error(
                    "C005",
                    "Upload is allowed only as a direct POST input",
                    binding.span,
                ));
            }
            continue;
        }
        if function
            .parameters
            .iter()
            .any(|parameter| parameter.value_type() == Some(&Type::Upload))
            && !matches!(
                ty.base(),
                Type::Text
                    | Type::Int
                    | Type::Bool
                    | Type::Float
                    | Type::Decimal
                    | Type::Uuid
                    | Type::DateTime
                    | Type::Date
                    | Type::Time
                    | Type::Duration
            )
        {
            errors.push(Diagnostic::error(
                "C005",
                "multipart inputs must be Upload or wire-safe scalar values",
                binding.span,
            ));
        }
        match Schema::build(ty, types, Policy::ApiInput) {
            Err(reason) => errors.push(Diagnostic::error(
                "C005",
                format!("API/CMD input is not JSON-decodable: {reason}"),
                binding.span,
            )),
            Ok(schema) if matches!(binding.kind, ApiKind::Get | ApiKind::Delete) => {
                let mut node = &schema.nodes[schema.root];
                if let crate::wire::Node::Nullable(inner) = node {
                    node = &schema.nodes[*inner];
                }
                if matches!(
                    node,
                    crate::wire::Node::Record { .. }
                        | crate::wire::Node::List(_)
                        | crate::wire::Node::Json
                        | crate::wire::Node::Nullable(_)
                ) {
                    errors.push(Diagnostic::error(
                        "C005",
                        "GET/DELETE inputs must be wire-safe scalars or nullable scalars",
                        binding.span,
                    ));
                }
            }
            Ok(_) => {}
        }
    }
    Some((*id, inputs))
}

fn legacy_routes(
    packages: &[Package],
    functions: &[Function],
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) -> Vec<ApiRoute> {
    let mut routes = Vec::new();
    let mut occupied = BTreeMap::new();
    for (owner, package) in packages.iter().enumerate() {
        if crate::source::SourceMap::is_standard(package.span.source) {
            continue;
        }
        let segments: Vec<_> = package.name.iter().map(|part| part.text.as_str()).collect();
        let mut prefix: Vec<_> = match &package.layout {
            SourceLayout::Loose => {
                let Some(index) = segments.iter().position(|part| *part == "api") else {
                    continue;
                };
                if index == 0 || index + 1 == segments.len() {
                    continue;
                }
                segments[..index]
                    .iter()
                    .chain(segments[index + 1..].iter())
                    .copied()
                    .collect()
            }
            _ => continue,
        };
        for declaration in &package.declarations {
            let Declaration::Function(clause) = declaration else {
                errors.push(Diagnostic::error(
                    "C005",
                    "api/ files may contain only HTTP handler functions",
                    crate::markdown::declaration_span(declaration),
                ));
                continue;
            };
            let (method, action) = if let Some(action) = clause.name.text.strip_prefix("get_") {
                ("GET", action)
            } else if let Some(action) = clause.name.text.strip_prefix("post_") {
                ("POST", action)
            } else if let Some(action) = clause.name.text.strip_prefix("delete_") {
                ("DELETE", action)
            } else {
                errors.push(Diagnostic::error(
                    "C005",
                    "API handler names must start with get_, post_ or delete_",
                    clause.name.span,
                ));
                continue;
            };
            if action.is_empty()
                || clause.public
                || clause.kind != crate::syntax::FunctionKind::Ordinary
            {
                errors.push(Diagnostic::error(
                    "C005",
                    "API handlers must be private ordinary functions with an action after the HTTP prefix",
                    clause.span,
                ));
                continue;
            }
            let qualified = format!("{}.{}", segments.join("."), clause.name.text);
            let Some((id, function)) = functions.iter().enumerate().find(|(_, function)| {
                function.owner == owner
                    && function.name == qualified
                    && function.parameters.len() == clause.inputs.len()
            }) else {
                continue;
            };
            prefix.push(action);
            let path = format!("/{}", prefix.join("/"));
            prefix.pop();
            if let Some(previous) = occupied.insert((method, path.clone()), clause.span) {
                let mut error = Diagnostic::error("C002", "duplicate HTTP route", clause.span);
                error.related.push(crate::diagnostic::Label {
                    span: previous,
                    message: "first handler for this method and path".into(),
                });
                errors.push(error);
                continue;
            }
            if function.outputs.len() != 1 || function.outputs[0].name != "response" {
                errors.push(Diagnostic::error(
                    "C005",
                    "API handlers must have exactly one output named response",
                    clause.span,
                ));
                continue;
            }
            let mut inputs = Vec::new();
            for (index, input) in clause.inputs.iter().enumerate() {
                let supported = matches!(&input.kind,
                    InputKind::Value(Pattern::Typed { bounds, .. }) if bounds.is_empty())
                    && function.parameters.get(index).is_some_and(
                        |parameter| matches!(parameter, Parameter::Value(ty) if input_type(ty)),
                    );
                if !supported {
                    errors.push(Diagnostic::error(
                        "C005",
                        "API inputs must be plain Text, Int, Bool or nullable variants",
                        input.span,
                    ));
                }
                inputs.push(input.name.text.clone());
            }
            if let Err(reason) = Schema::from_field(&function.outputs[0], types, Policy::Output) {
                errors.push(Diagnostic::error(
                    "C005",
                    format!(
                        "API response must be JSON-serializable without private fields: {reason}"
                    ),
                    clause.span,
                ));
            }
            routes.push(ApiRoute {
                method,
                path,
                function: id,
                inputs,
                directory: Vec::new(),
                anonymous: false,
                span: clause.span,
                component: function.name.split('.').next().unwrap_or("app").to_owned(),
                domain: function.name.split('.').nth(1).unwrap_or("main").to_owned(),
                action: action.to_owned(),
                auth: None,
                permission: None,
                components: Vec::new(),
                multipart: false,
            });
        }
    }
    routes.sort_by(|left, right| (&left.path, left.method).cmp(&(&right.path, right.method)));
    routes
}

fn input_type(ty: &Type) -> bool {
    match ty {
        Type::Nullable(inner) => matches!(inner.as_ref(), Type::Text | Type::Int | Type::Bool),
        Type::Text | Type::Int | Type::Bool => true,
        _ => false,
    }
}
