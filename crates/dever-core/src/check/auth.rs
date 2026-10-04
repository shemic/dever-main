use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Diagnostic;
use crate::hir::{
    ApiRest, ApiRoute, AuthHook, Expression, ExpressionKind, Function, Permission, Program,
    RestPermissions,
};
use crate::source::{SourceRole, Span};
use crate::syntax::{FunctionKind, Package};
use crate::types::{Definition, DefinitionKind, Parameter, Shape, Type};

pub(super) struct Registration {
    pub hooks: Vec<AuthHook>,
    pub permissions: Vec<Permission>,
}

struct SiteBinding {
    key: String,
    provider: String,
}

pub(super) fn register(
    settings: Option<&dever_runtime::config::CompilationBindings>,
    packages: &[Package],
    functions: &[Function],
    types: &[Definition],
    routes: &mut [ApiRoute],
    rest: &mut [ApiRest],
    errors: &mut Vec<Diagnostic>,
) -> Registration {
    let Some(settings) = settings else {
        return Registration {
            hooks: Vec::new(),
            permissions: Vec::new(),
        };
    };
    if routes.is_empty() && rest.is_empty() {
        return Registration {
            hooks: Vec::new(),
            permissions: Vec::new(),
        };
    }

    let claims = types
        .iter()
        .position(|definition| definition.name == "dever.auth.Claims")
        .expect("official dever.auth.Claims type");
    let mut route_providers = Vec::with_capacity(routes.len());
    let mut rest_providers = Vec::with_capacity(rest.len());
    let mut providers = BTreeSet::new();

    for route in routes.iter() {
        let binding = site_binding(settings, &route.directory, route.span, errors);
        if let Some(binding) = &binding {
            providers.insert(binding.provider.clone());
        }
        route_providers.push(binding);
    }
    for declaration in rest.iter() {
        let binding = site_binding(settings, &declaration.directory, declaration.span, errors);
        if let Some(binding) = &binding {
            providers.insert(binding.provider.clone());
        }
        rest_providers.push(binding);
    }

    let mut hooks = Vec::new();
    let mut hook_by_provider = BTreeMap::new();
    for provider_key in providers {
        let Some(verify) = settings.providers.get(&provider_key) else {
            errors.push(Diagnostic::error(
                "C005",
                format!("site authentication provider '{provider_key}' is not configured"),
                first_provider_span(
                    &provider_key,
                    &route_providers,
                    routes,
                    &rest_providers,
                    rest,
                ),
            ));
            continue;
        };
        let span = first_provider_span(
            &provider_key,
            &route_providers,
            routes,
            &rest_providers,
            rest,
        );
        let Some((function, identity)) =
            verify_contract(verify, claims, packages, functions, types, span, errors)
        else {
            continue;
        };
        let hook = hooks.len();
        hooks.push(AuthHook {
            provider: provider_key.clone(),
            function,
            claims,
            identity,
        });
        hook_by_provider.insert(provider_key, hook);
    }

    let mut permissions = Vec::new();
    let mut occupied = BTreeMap::new();
    for (route, binding) in routes.iter_mut().zip(&route_providers) {
        route.auth = binding
            .as_ref()
            .and_then(|binding| hook_by_provider.get(&binding.provider).copied());
        if !route.anonymous {
            route.permission = binding.as_ref().and_then(|binding| {
                register_permission(
                    &mut permissions,
                    &mut occupied,
                    Permission {
                        key: permission_key(
                            &route.component,
                            &route.domain,
                            &binding.key,
                            &route.action,
                        ),
                        component: route.component.clone(),
                        domain: route.domain.clone(),
                        site: binding.key.clone(),
                        action: route.action.clone(),
                        method: route.method,
                    },
                    route.span,
                    errors,
                )
            });
        }
    }
    for (declaration, binding) in rest.iter_mut().zip(&rest_providers) {
        declaration.auth = binding
            .as_ref()
            .and_then(|binding| hook_by_provider.get(&binding.provider).copied());
        let Some(binding) = binding else { continue };
        let mut permission = |action, method| {
            register_permission(
                &mut permissions,
                &mut occupied,
                Permission {
                    key: permission_key(
                        &declaration.component,
                        &declaration.domain,
                        &binding.key,
                        action,
                    ),
                    component: declaration.component.clone(),
                    domain: declaration.domain.clone(),
                    site: binding.key.clone(),
                    action: action.to_owned(),
                    method,
                },
                declaration.span,
                errors,
            )
        };
        declaration.permissions = RestPermissions {
            read: permission("read", "GET"),
            create: permission("create", "POST"),
            replace: permission("replace", "PUT"),
            delete: permission("delete", "DELETE"),
        };
    }
    Registration { hooks, permissions }
}

fn site_binding(
    settings: &dever_runtime::config::CompilationBindings,
    directory: &[String],
    span: Span,
    errors: &mut Vec<Diagnostic>,
) -> Option<SiteBinding> {
    let directory = directory.iter().map(String::as_str).collect::<Vec<_>>();
    match settings.site_for_directory(&directory) {
        Ok(Some((key, site))) => Some(SiteBinding {
            key: key.to_owned(),
            provider: site.auth.clone(),
        }),
        Ok(None) => {
            errors.push(Diagnostic::error(
                "C005",
                "HTTP API source directory is not assigned to a configured site",
                span,
            ));
            None
        }
        Err(message) => {
            errors.push(Diagnostic::error("C005", message, span));
            None
        }
    }
}

fn permission_key(component: &str, domain: &str, site: &str, action: &str) -> String {
    format!("{component}.{domain}.{site}.{action}")
}

fn register_permission(
    permissions: &mut Vec<Permission>,
    occupied: &mut BTreeMap<String, Span>,
    permission: Permission,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if let Some(previous) = occupied.insert(permission.key.clone(), span) {
        let mut error = Diagnostic::error(
            "C002",
            format!("duplicate API permission '{}'", permission.key),
            span,
        );
        error.related.push(crate::diagnostic::Label {
            span: previous,
            message: "first declaration of this permission".into(),
        });
        errors.push(error);
        return None;
    }
    let id = permissions.len();
    permissions.push(permission);
    Some(id)
}

fn first_provider_span(
    provider: &str,
    route_providers: &[Option<SiteBinding>],
    routes: &[ApiRoute],
    rest_providers: &[Option<SiteBinding>],
    rest: &[ApiRest],
) -> Span {
    route_providers
        .iter()
        .zip(routes)
        .find_map(|(candidate, route)| {
            (candidate.as_ref().map(|binding| binding.provider.as_str()) == Some(provider))
                .then_some(route.span)
        })
        .or_else(|| {
            rest_providers
                .iter()
                .zip(rest)
                .find_map(|(candidate, declaration)| {
                    (candidate.as_ref().map(|binding| binding.provider.as_str()) == Some(provider))
                        .then_some(declaration.span)
                })
        })
        .expect("provider originated from an API declaration")
}

fn verify_contract(
    name: &str,
    claims: usize,
    packages: &[Package],
    functions: &[Function],
    types: &[Definition],
    span: Span,
    errors: &mut Vec<Diagnostic>,
) -> Option<(usize, usize)> {
    let candidates = functions
        .iter()
        .enumerate()
        .filter(|(_, function)| function.name == name)
        .collect::<Vec<_>>();
    let [(function_id, function)] = candidates.as_slice() else {
        errors.push(Diagnostic::error(
            "C005",
            if candidates.is_empty() {
                format!("authentication verify App '{name}' does not exist")
            } else {
                format!("authentication verify App '{name}' is overloaded")
            },
            span,
        ));
        return None;
    };
    if packages[function.owner].layout.role() != Some(SourceRole::App)
        || !function.public
        || function.kind != FunctionKind::Ordinary
        || function.parameters.as_slice() != [Parameter::Value(Type::Named(claims))]
        || function.outputs.len() != 1
    {
        errors.push(Diagnostic::error(
            "C005",
            "authentication verify must be one ordinary App function with one dever.auth.Claims input and one identity record output",
            function.span,
        ));
        return None;
    }
    let Type::Named(identity) = function.outputs[0].ty else {
        errors.push(Diagnostic::error(
            "C005",
            "authentication verify output must be an App-owned identity record",
            function.span,
        ));
        return None;
    };
    if types[identity].owner != function.owner
        || !types[identity].public
        || types[identity].kind != DefinitionKind::Regular
        || !identity_fields(&types[identity], types)
    {
        errors.push(Diagnostic::error(
            "C005",
            "authentication identity must be an App-owned public record with id: Text, user_id: ModelId?, and tenant_id: ModelId?",
            types[identity].span,
        ));
        return None;
    }
    Some((*function_id, identity))
}

fn identity_fields(identity: &Definition, types: &[Definition]) -> bool {
    let Shape::Record(fields) = &identity.shape else {
        return false;
    };
    let [id, user_id, tenant_id] = fields.as_slice() else {
        return false;
    };
    fields
        .iter()
        .all(|field| !field.private && field.bounds.is_empty())
        && id.name == "id"
        && id.ty == Type::Text
        && user_id.name == "user_id"
        && optional_model_id(&user_id.ty, types)
        && tenant_id.name == "tenant_id"
        && optional_model_id(&tenant_id.ty, types)
}

fn optional_model_id(ty: &Type, types: &[Definition]) -> bool {
    matches!(ty, Type::Nullable(inner) if matches!(inner.as_ref(), Type::Named(id) if types[*id].kind == DefinitionKind::ModelId))
}

pub(super) fn validate(program: &Program, configured: bool, errors: &mut Vec<Diagnostic>) {
    validate_identity_types(program, configured, errors);
    for hook in &program.auth {
        let root = crate::specialize::Specialization {
            function: hook.function,
            handlers: Vec::new(),
        };
        let Ok(reachable) = crate::specialize::reachable(&program.functions, [root]) else {
            continue;
        };
        let mut forbidden = None;
        let mut tenant_model = None;
        for instance in reachable {
            crate::check::visit(&program.functions[instance.function], |expression| {
                let writes = match &expression.kind {
                    ExpressionKind::ModelOperation { operation, .. } => operation.writes_database(),
                    ExpressionKind::JobEnqueue { .. } => true,
                    _ => false,
                };
                if writes && forbidden.is_none() {
                    forbidden = Some((
                        expression.span,
                        "authentication verify cannot write to a database or enqueue a Job",
                    ));
                }
                if let ExpressionKind::ModelOperation { model, .. } = &expression.kind
                    && program.models[*model].scope == crate::model::ModelScope::Tenant
                    && tenant_model.is_none()
                {
                    tenant_model = Some((expression.span, program.models[*model].package.clone()));
                }
            });
        }
        if let Some((span, message)) = forbidden {
            errors.push(Diagnostic::error("C013", message, span));
        }
        if let Some((span, model)) = tenant_model {
            errors.push(Diagnostic::error(
                "C014",
                format!(
                    "authentication verify may access only global Models; '{model}' is tenant-scoped"
                ),
                span,
            ));
        }
        if program.effects[hook.function].contains("request") {
            errors.push(Diagnostic::error(
                "C006",
                "authentication verify receives checked Claims and cannot read HTTP request capabilities",
                program.functions[hook.function].span,
            ));
        }
        if program.effects[hook.function].contains("identity") {
            errors.push(Diagnostic::error(
                "C006",
                "authentication verify cannot read or require an identity before it establishes one",
                program.functions[hook.function].span,
            ));
        }
    }

    for (function_id, function) in program.functions.iter().enumerate() {
        let package = &program.packages[function.owner];
        if package.bundled || !program.effects[function_id].contains("identity") {
            continue;
        }
        if package.role != Some(SourceRole::App) {
            errors.push(Diagnostic::error(
                "C006",
                "authentication and site capabilities are available only from App functions",
                function.span,
            ));
            continue;
        }
    }
}

fn validate_identity_types(program: &Program, configured: bool, errors: &mut Vec<Diagnostic>) {
    for route in &program.api_routes {
        let context = IdentityContext {
            program,
            hook: route.auth.map(|index| &program.auth[index]),
            protected: !route.anonymous,
            configured,
            binding_span: route.span,
        };
        let root = crate::specialize::Specialization {
            function: route.function,
            handlers: Vec::new(),
        };
        let Ok(reachable) = crate::specialize::reachable(&program.functions, [root]) else {
            continue;
        };
        for instance in reachable {
            super::visit(&program.functions[instance.function], |expression| {
                context.validate_expression(expression, errors);
            });
        }
    }
    for route in &program.api_rest {
        let context = IdentityContext {
            program,
            hook: route.auth.map(|index| &program.auth[index]),
            protected: true,
            configured,
            binding_span: route.span,
        };
        if let Some((field, source)) = route.contract.owner {
            let Shape::Record(fields) = &program.types[program.models[route.model].record].shape
            else {
                unreachable!("checked Model record")
            };
            let (name, identity_field) = match source {
                crate::model::RestOwnerSource::User => ("user_id", 1),
                crate::model::RestOwnerSource::Tenant => ("tenant_id", 2),
            };
            context.validate_type(
                name,
                identity_field,
                fields[field].ty.clone().nullable(),
                program.types[program.models[route.model].record].span,
                errors,
            );
        }
        for expression in route
            .contract
            .create
            .iter()
            .chain(&route.contract.replace)
            .filter_map(|write| match &write.source {
                crate::model::RestValueSource::Binding { expression, .. } => Some(expression),
                crate::model::RestValueSource::Input => None,
            })
            .chain(
                route
                    .contract
                    .search
                    .iter()
                    .map(|search| &search.expression),
            )
        {
            super::visit_expression(expression, |value| {
                context.validate_expression(value, errors);
            });
        }
    }
}

struct IdentityContext<'a> {
    program: &'a Program,
    hook: Option<&'a AuthHook>,
    protected: bool,
    configured: bool,
    binding_span: Span,
}

impl IdentityContext<'_> {
    fn validate_expression(&self, expression: &Expression, errors: &mut Vec<Diagnostic>) {
        use crate::intrinsic::Intrinsic;

        let ExpressionKind::Intrinsic {
            operation,
            arguments,
            ..
        } = &expression.kind
        else {
            return;
        };
        let (name, field, actual) = match operation {
            Intrinsic::AuthUserId => ("user_id", 1, expression.ty.clone()),
            Intrinsic::AuthTenantId => ("tenant_id", 2, expression.ty.clone()),
            Intrinsic::AuthOwnsUser => ("owns_user", 1, arguments[0].ty.clone().nullable()),
            _ => return,
        };
        self.validate_type(name, field, actual, expression.span, errors);
    }

    fn validate_type(
        &self,
        name: &str,
        field: usize,
        actual: Type,
        span: Span,
        errors: &mut Vec<Diagnostic>,
    ) {
        if !optional_model_id(&actual, &self.program.types) || (!self.configured && self.protected)
        {
            return;
        }
        let expected = self.hook.and_then(|hook| {
            let Shape::Record(fields) = &self.program.types[hook.identity].shape else {
                return None;
            };
            Some(&fields[field].ty)
        });
        if self.protected && expected == Some(&actual) {
            return;
        }
        let message = match (self.protected, self.hook, expected) {
            (true, Some(hook), Some(expected)) => format!(
                "dever.auth.{name} nominal ID must match provider '{}' identity field {} rather than {}",
                hook.provider,
                expected.label(&self.program.types),
                actual.label(&self.program.types),
            ),
            _ => format!(
                "typed dever.auth.{name} requires a protected API with a configured identity provider"
            ),
        };
        let mut error = Diagnostic::error("C006", message, span);
        error.related.push(crate::diagnostic::Label {
            span: self.binding_span,
            message: "authentication context belongs to this API binding".into(),
        });
        errors.push(error);
    }
}
