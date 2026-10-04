use std::collections::BTreeSet;

use crate::diagnostic::Diagnostic;
use crate::hir::{Function, Job};
use crate::model::ConnectionSelector;
use crate::source::SourceRole;
use crate::syntax::{Declaration, FunctionKind, InputKind, Package, Pattern};
use crate::types::{Definition, DefinitionKind, Parameter, Shape, Type};
use crate::wire::{Policy, Schema};

impl super::body::Body<'_, '_> {
    pub(super) fn enqueue_job(
        &mut self,
        arguments: &[crate::syntax::Expression],
        scheduled: bool,
        span: crate::source::Span,
    ) -> super::Checked<crate::hir::Expression> {
        let caller = &self.context.symbols.packages[self.context.owner].layout;
        if caller.role() != Some(SourceRole::App) {
            return Err(Diagnostic::error(
                "C006",
                "only App may enqueue a Job",
                span,
            ));
        }
        let Some(crate::syntax::Expression {
            kind: crate::syntax::ExpressionKind::Name(path),
            ..
        }) = arguments.first()
        else {
            return Err(Diagnostic::error(
                "C005",
                "enqueue requires a static Job target",
                span,
            ));
        };
        let name = super::path_name(path);
        let candidates = self.context.candidates(&name);
        let targets = self
            .context
            .functions
            .iter()
            .enumerate()
            .filter(|(_, function)| {
                matches!(function.kind, FunctionKind::Job { .. })
                    && candidates.contains(&function.name)
            })
            .collect::<Vec<_>>();
        let [(target, entry)] = targets.as_slice() else {
            return Err(Diagnostic::error(
                "C004",
                "enqueue target must name one declared Job",
                span,
            ));
        };
        if self.context.symbols.packages[entry.owner].layout.domain() != caller.domain() {
            return Err(Diagnostic::error(
                "C006",
                "App may enqueue only a Job in its own domain",
                span,
            ));
        }
        let mut expected = entry
            .parameters
            .iter()
            .map(|parameter| match parameter {
                Parameter::Value(ty) => Ok(ty.clone()),
                _ => Err(Diagnostic::error(
                    "C005",
                    "Job payload must be a record",
                    span,
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        expected.push(Type::Text);
        if scheduled {
            expected.push(Type::DateTime);
        }
        if arguments.len() != expected.len() + 1 {
            return Err(Diagnostic::error(
                "C005",
                "enqueue requires target, optional payload, key and optional run_at",
                span,
            ));
        }
        let arguments = arguments[1..]
            .iter()
            .zip(&expected)
            .map(|(argument, ty)| self.expression(argument, Some(ty)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(crate::hir::Expression {
            kind: crate::hir::ExpressionKind::JobEnqueue {
                target: *target,
                arguments,
                scheduled,
            },
            ty: Type::Id,
            span,
        })
    }
}

pub(super) fn register(
    packages: &[Package],
    functions: &[Function],
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) -> Vec<Job> {
    let mut jobs = Vec::new();
    for (owner, package) in packages.iter().enumerate() {
        let is_job = package.layout.role() == Some(SourceRole::Job);
        let mut connection = None;
        let mut scheduled = BTreeSet::new();
        for declaration in &package.declarations {
            match declaration {
                Declaration::Database(binding) if is_job => {
                    if connection
                        .replace(ConnectionSelector::Explicit(binding.name.text.clone()))
                        .is_some()
                    {
                        errors.push(Diagnostic::error(
                            "C002",
                            "duplicate Job database binding",
                            binding.span,
                        ));
                    }
                }
                Declaration::Function(clause) => {
                    let declared = matches!(clause.kind, FunctionKind::Job { .. });
                    if is_job != declared {
                        errors.push(Diagnostic::error("C005", "job declarations belong only in job source; job source contains only Job entries", clause.span));
                    }
                }
                Declaration::Schedule(_) if is_job => {}
                _ if is_job => errors.push(Diagnostic::error(
                    "C005",
                    "Job files declare database, jobs and schedules; payload records belong in App",
                    crate::markdown::declaration_span(declaration),
                )),
                _ => {}
            }
        }
        if !is_job {
            continue;
        }
        let Some(connection) = connection else {
            errors.push(Diagnostic::error(
                "C014",
                "Job source requires an explicit database binding",
                package.span,
            ));
            continue;
        };
        for (function, entry) in functions
            .iter()
            .enumerate()
            .filter(|(_, function)| function.owner == owner)
        {
            let FunctionKind::Job {
                attempts,
                timeout_ms,
            } = entry.kind
            else {
                continue;
            };
            let clauses = package
                .declarations
                .iter()
                .filter_map(|declaration| match declaration {
                    Declaration::Function(clause)
                        if entry.name.ends_with(&format!(".{}", clause.name.text)) =>
                    {
                        Some(clause)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            if clauses.len() != 1 || entry.parameters.len() > 1 || !entry.outputs.is_empty()
                || clauses.iter().any(|clause| clause.pure || clause.recovery.is_some() || clause.fails.is_some()
                    || clause.inputs.iter().any(|input| !matches!(&input.kind, InputKind::Value(Pattern::Typed { bounds, .. }) if bounds.is_empty())))
            {
                errors.push(Diagnostic::error("C005", "Job requires one plain zero-output declaration with zero inputs or one App record payload", entry.span));
                continue;
            }
            let payload = match entry.parameters.as_slice() {
                [] => None,
                [Parameter::Value(Type::Named(id))]
                    if types[*id].kind == DefinitionKind::Regular
                        && matches!(types[*id].shape, Shape::Record(_))
                        && packages[types[*id].owner].layout.role() == Some(SourceRole::App)
                        && packages[types[*id].owner].layout.domain()
                            == package.layout.domain() =>
                {
                    match Schema::build(&Type::Named(*id), types, Policy::Job) {
                        Ok(schema) => Some(schema),
                        Err(reason) => {
                            errors.push(Diagnostic::error("C005", reason, entry.span));
                            continue;
                        }
                    }
                }
                _ => {
                    errors.push(Diagnostic::error(
                        "C005",
                        "Job payload must be a wire-safe record owned by the same-domain App",
                        entry.span,
                    ));
                    continue;
                }
            };
            jobs.push(Job {
                function,
                connection: connection.clone(),
                payload,
                attempts,
                timeout_ms,
                schedule: None,
                components: Vec::new(),
            });
        }
        for declaration in &package.declarations {
            let Declaration::Schedule(schedule) = declaration else {
                continue;
            };
            let identity = format!(
                "{}.{}",
                super::path_name(&package.name),
                schedule.target.text
            );
            let Some(job) = jobs
                .iter_mut()
                .find(|job| functions[job.function].name == identity)
            else {
                errors.push(Diagnostic::error(
                    "C004",
                    "schedule target must name a Job declared in this source",
                    schedule.span,
                ));
                continue;
            };
            if job.payload.is_some() || !scheduled.insert(job.function) {
                errors.push(Diagnostic::error(
                    "C005",
                    "schedule requires a unique zero-input Job target",
                    schedule.span,
                ));
                continue;
            }
            match dever_runtime::cron::Cron::parse(&schedule.cron) {
                Ok(_) => job.schedule = Some(schedule.cron.clone()),
                Err(reason) => errors.push(Diagnostic::error("C005", reason, schedule.span)),
            }
        }
    }
    jobs
}
