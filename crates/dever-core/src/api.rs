use std::collections::BTreeSet;

use crate::diagnostic::Diagnostic;
use crate::hir::{Atom, CallArgument, CallTarget, Domain, ExpressionKind, HandlerTarget, Program};
use crate::specialize::{self, Specialization};
use crate::types::{Field, Parameter, Shape, Type};

impl Program {
    /// Application entries are known only for run/build; library checks retain unused exports.
    pub fn application_errors(&self) -> Result<Vec<Diagnostic>, String> {
        let roots = self
            .api_routes
            .iter()
            .map(|route| route.function)
            .chain(self.api_commands.iter().map(|command| command.function))
            .chain(self.jobs.iter().map(|job| job.function))
            .chain(self.auth.iter().map(|hook| hook.function))
            .chain(self.rest_binding_roots())
            .map(|function| Specialization {
                function,
                handlers: Vec::new(),
            });
        let reachable = specialize::execution_reachable(&self.functions, roots)
            .map_err(|_| "application contains a specialization cycle".to_owned())?
            .iter()
            .map(|instance| instance.function)
            .collect::<BTreeSet<_>>();
        Ok(crate::contracts::application_errors(self, &reachable))
    }

    /// Application reachability cannot be inferred for a library with unknown consumers.
    pub fn application_warnings(&self, entry: &str) -> Result<Vec<Diagnostic>, String> {
        let entry_id = self.entry(entry)?;
        let instances = specialize::reachable(
            &self.functions,
            [Specialization {
                function: entry_id,
                handlers: Vec::new(),
            }],
        )
        .map_err(|_| "application contains a specialization cycle".to_owned())?;
        let reachable = instances
            .iter()
            .map(|instance| instance.function)
            .collect::<BTreeSet<_>>();
        let mut required_exports = BTreeSet::from([entry_id]);
        for id in reachable {
            let function = &self.functions[id];
            crate::check::visit(function, |expression| {
                let mut reference = |target: HandlerTarget| {
                    if let HandlerTarget::Function(target) = target {
                        let caller = &self.packages[function.owner];
                        let callee = &self.packages[self.functions[target].owner];
                        let internal_app_call = caller.role == Some(crate::source::SourceRole::App)
                            && callee.role == Some(crate::source::SourceRole::App)
                            && caller.domain.is_some()
                            && caller.domain == callee.domain;
                        if self.functions[target].owner != function.owner && !internal_app_call {
                            required_exports.insert(target);
                        }
                    }
                };
                if let Some((target, arguments)) = expression.kind.static_call() {
                    match target {
                        CallTarget::Function(id) => reference(HandlerTarget::Function(id)),
                        CallTarget::Handler(handler) => reference(handler),
                    }
                    for argument in arguments {
                        if let CallArgument::Handler(handler) = argument {
                            reference(*handler);
                        }
                    }
                } else if let ExpressionKind::Collection {
                    handler: Some(handler),
                    ..
                } = &expression.kind
                {
                    reference(*handler);
                }
            });
        }
        let mut warnings = Vec::new();
        for (id, function) in self.functions.iter().enumerate() {
            if !function.public
                || self.packages[function.owner].bundled
                || required_exports.contains(&id)
            {
                continue;
            }
            warnings.push(Diagnostic::error(
                "W007",
                format!("exported function '{}' is not used outside its source owner by application '{entry}'; its exposure may be unnecessary", function.name),
                function.span,
            ));
        }
        warnings.sort_by_key(|warning| (warning.primary.source, warning.primary.start));
        Ok(warnings)
    }

    /// A stable, reviewable public contract. Source paths and internal IDs are excluded.
    pub fn api_snapshot(&self) -> String {
        let mut lines = BTreeSet::from(["dever-api 5".to_owned()]);
        let mut exposed_types = BTreeSet::new();
        for (id, definition) in self.types.iter().enumerate() {
            if definition.public && !self.packages[definition.owner].bundled {
                exposed_types.insert(id);
            }
        }
        for (id, function) in self.functions.iter().enumerate() {
            if !function.public || self.packages[function.owner].bundled {
                continue;
            }
            let parameters = function
                .parameters
                .iter()
                .map(|parameter| self.api_parameter(parameter, &mut exposed_types))
                .collect::<Vec<_>>()
                .join(", ");
            for field in &function.outputs {
                referenced_types(&field.ty, &mut exposed_types);
            }
            lines.insert(format!(
                "{}function {}({parameters}) ({}) pure={} recover={:?} effects=[{}] errors=[{}] recoveries={:?}",
                if function.kind == crate::syntax::FunctionKind::Transaction {
                    "transaction "
                } else {
                    ""
                },
                function.name,
                self.api_fields(&function.outputs),
                function.pure,
                function.recovery,
                self.effects[id]
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
                crate::contracts::failure_names(self, &self.failures[id]),
                self.recoveries[id],
            ));
        }
        let mut visited = BTreeSet::new();
        while let Some(id) = exposed_types.pop_first() {
            if !visited.insert(id) {
                continue;
            }
            let definition = &self.types[id];
            let shape = self.api_shape(&definition.shape, &mut exposed_types);
            lines.insert(format!("type {} {shape}", definition.name));
        }
        lines.extend(self.model_snapshot().lines().skip(1).map(str::to_owned));
        lines.extend(self.permissions.iter().map(|permission| {
            format!(
                "permission {} method={} component={} domain={} site={} action={}",
                permission.key,
                permission.method,
                permission.component,
                permission.domain,
                permission.site,
                permission.action,
            )
        }));
        lines.into_iter().collect::<Vec<_>>().join("\n") + "\n"
    }

    /// Baselines are generated explicitly; checking never updates the approved contract.
    pub fn check_api(&self, baseline: &str) -> Result<(), String> {
        let normalize = |text: &str| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        };
        let expected = normalize(baseline);
        if !expected.contains("dever-api 5") {
            return Err("invalid API baseline: expected dever-api 5".into());
        }
        let actual = normalize(&self.api_snapshot());
        if actual == expected {
            return Ok(());
        }
        let mut changes = vec!["public API differs from the approved baseline:".to_owned()];
        changes.extend(expected.difference(&actual).map(|line| format!("- {line}")));
        changes.extend(actual.difference(&expected).map(|line| format!("+ {line}")));
        Err(changes.join("\n"))
    }

    fn api_parameter(&self, parameter: &Parameter, exposed_types: &mut BTreeSet<usize>) -> String {
        match parameter {
            Parameter::Value(ty) => {
                referenced_types(ty, exposed_types);
                ty.label(&self.types)
            }
            Parameter::Handler(signature) => {
                for ty in &signature.parameters {
                    referenced_types(ty, exposed_types);
                }
                for field in &signature.outputs {
                    referenced_types(&field.ty, exposed_types);
                }
                let inputs = signature
                    .parameters
                    .iter()
                    .zip(&signature.bounds)
                    .map(|(ty, bounds)| format!("{} [{}]", ty.label(&self.types), domain(bounds)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("handler({inputs})({})", self.api_fields(&signature.outputs))
            }
        }
    }

    fn api_shape(&self, shape: &Shape, exposed_types: &mut BTreeSet<usize>) -> String {
        match shape {
            Shape::Record(fields) => {
                let visible = fields
                    .iter()
                    .filter(|field| !field.private)
                    .cloned()
                    .collect::<Vec<_>>();
                for field in &visible {
                    referenced_types(&field.ty, exposed_types);
                }
                let construction = if fields.iter().any(|field| field.private) {
                    "private"
                } else {
                    "public"
                };
                format!(
                    "record construct={construction} {{{}}}",
                    self.api_fields(&visible)
                )
            }
            Shape::Choice(variants) => variants
                .iter()
                .map(|variant| {
                    for field in &variant.fields {
                        referenced_types(&field.ty, exposed_types);
                    }
                    format!(
                        "{}{}({})",
                        if variant.error { "error " } else { "" },
                        variant.name,
                        self.api_fields(&variant.fields)
                    )
                })
                .collect::<Vec<_>>()
                .join(" | "),
        }
    }

    fn api_fields(&self, fields: &[Field]) -> String {
        fields
            .iter()
            .map(|field| {
                format!(
                    "{}: {} [{}]",
                    field.name,
                    field.ty.label(&self.types),
                    domain(&field.bounds)
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn referenced_types(ty: &Type, references: &mut BTreeSet<usize>) {
    match ty {
        Type::Named(id) => {
            references.insert(*id);
        }
        Type::Nullable(base)
        | Type::List(base)
        | Type::Stream(base)
        | Type::AsyncStream(base)
        | Type::RowStream(base)
        | Type::Related(base)
        | Type::Channel(base) => referenced_types(base, references),
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            referenced_types(key, references);
            referenced_types(value, references);
        }
        Type::Outputs(fields) | Type::Task(fields) => {
            for field in fields {
                referenced_types(&field.ty, references);
            }
        }
        _ => {}
    }
}

fn domain(bounds: &Domain) -> String {
    bounds
        .iter()
        .map(|atom| match atom {
            Atom::Int(lower, upper) => format!("{lower}..{upper}"),
            Atom::Decimal(lower, upper) => format!("{lower}..{upper}"),
            _ => unreachable!("checked field bounds are numeric intervals"),
        })
        .collect::<Vec<_>>()
        .join(" or ")
}
