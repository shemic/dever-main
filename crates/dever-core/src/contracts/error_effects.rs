use std::collections::BTreeSet;

use crate::diagnostic::Diagnostic;
use crate::hir::{
    CallArgument, CallTarget, Expression, ExpressionKind, Failure, HandlerTarget, Program,
};
use crate::types::{Shape, Type};

use super::dependencies::Dependencies;

#[derive(Clone, Default, Eq, PartialEq)]
struct FailureSummary {
    known: BTreeSet<Failure>,
    handlers: BTreeSet<usize>,
}

impl FailureSummary {
    fn include(&mut self, other: &Self) {
        self.known.extend(&other.known);
        self.handlers.extend(&other.handlers);
    }
}

pub(super) fn check(
    program: &mut Program,
    dependencies: &Dependencies,
    errors: &mut Vec<Diagnostic>,
) {
    let summaries = infer(program, dependencies);
    validate_captures(program, &summaries, errors);
    validate_payloads(program, &summaries, errors);
    program.failures = summaries
        .iter()
        .map(|summary| summary.known.clone())
        .collect();
    program.failure_handlers = summaries
        .iter()
        .map(|summary| summary.handlers.clone())
        .collect();
}

fn infer(program: &Program, dependencies: &Dependencies) -> Vec<FailureSummary> {
    let mut summaries = vec![FailureSummary::default(); program.functions.len()];
    let mut work = dependencies.work();
    while let Some(id) = work.next() {
        let mut next = FailureSummary::default();
        if let Some(external) = program.functions[id].external {
            next.known.extend(
                &program.functions[external.contract]
                    .port
                    .as_ref()
                    .expect("external operation targets a Port")
                    .failures,
            );
        }
        if let Some(port) = &program.functions[id].port {
            next.known.extend(&port.failures);
        }
        crate::check::visit(&program.functions[id], |expression| {
            next.include(&expression_failures(expression, &summaries, program));
        });
        if program.functions[id].kind == crate::syntax::FunctionKind::Transaction {
            // Begin/commit can fail even when the body handles every query error.
            next.known
                .extend(crate::capture::database_errors(&program.types));
        }
        if next != summaries[id] {
            summaries[id] = next;
            work.changed(id);
        }
    }
    summaries
}

fn expression_failures(
    expression: &Expression,
    summaries: &[FailureSummary],
    program: &Program,
) -> FailureSummary {
    match &expression.kind {
        ExpressionKind::Fail(error) => {
            let (ty, variant) = failure_variant(error);
            FailureSummary {
                known: BTreeSet::from([Failure { ty, variant }]),
                handlers: BTreeSet::new(),
            }
        }
        ExpressionKind::CaptureResult { .. } => FailureSummary::default(),
        ExpressionKind::ModelOperation { .. } | ExpressionKind::JobEnqueue { .. } => {
            FailureSummary {
                known: crate::capture::database_errors(&program.types)
                    .into_iter()
                    .collect(),
                handlers: BTreeSet::new(),
            }
        }
        ExpressionKind::Collection {
            operation,
            handler,
            arguments,
            ..
        } => {
            let mut result = handler
                .map(|target| handler_failures(target, summaries))
                .unwrap_or_default();
            if *operation != crate::hir::CollectionOp::Close
                && arguments
                    .first()
                    .is_some_and(|argument| matches!(argument.ty, Type::RowStream(_)))
            {
                result
                    .known
                    .extend(crate::capture::database_errors(&program.types));
            }
            result
        }
        ExpressionKind::Intrinsic {
            handler: Some(handler),
            ..
        } => handler_failures(*handler, summaries),
        _ => expression
            .kind
            .static_call()
            .map(|(target, arguments)| call_failures(target, arguments, summaries))
            .unwrap_or_default(),
    }
}

fn failure_variant(error: &Expression) -> (usize, usize) {
    let Type::Named(ty) = error.ty else {
        unreachable!("checked fail value is a choice variant")
    };
    let ExpressionKind::Variant { variant, .. } = error.kind else {
        unreachable!("checked fail value is a direct error variant")
    };
    (ty, variant)
}

fn handler_failures(target: HandlerTarget, summaries: &[FailureSummary]) -> FailureSummary {
    match target {
        HandlerTarget::Function(id) => summaries[id].clone(),
        HandlerTarget::Parameter(id) => FailureSummary {
            known: BTreeSet::new(),
            handlers: BTreeSet::from([id]),
        },
    }
}

fn call_failures(
    target: CallTarget,
    arguments: &[CallArgument],
    summaries: &[FailureSummary],
) -> FailureSummary {
    match target {
        CallTarget::Handler(handler) => handler_failures(handler, summaries),
        CallTarget::Function(id) => {
            let mut result = FailureSummary {
                known: summaries[id].known.clone(),
                handlers: BTreeSet::new(),
            };
            for parameter in &summaries[id].handlers {
                let CallArgument::Handler(handler) = arguments[*parameter] else {
                    unreachable!("failure handler index refers to a handler argument")
                };
                result.include(&handler_failures(handler, summaries));
            }
            result
        }
    }
}

fn validate_captures(
    program: &Program,
    summaries: &[FailureSummary],
    errors: &mut Vec<Diagnostic>,
) {
    for function in &program.functions {
        crate::check::visit(function, |expression| {
            let ExpressionKind::CaptureResult {
                target,
                arguments,
                choice,
                ..
            } = &expression.kind
            else {
                return;
            };
            let actual = call_failures(*target, arguments, summaries);
            if !actual.handlers.is_empty() {
                errors.push(Diagnostic::error(
                    "C012",
                    "result(call) cannot capture an unconstrained handler error set",
                    expression.span,
                ));
                return;
            }
            let targets = match crate::capture::targets(&program.types, *choice, &actual.known) {
                Ok(targets) => targets,
                Err(message) => {
                    errors.push(Diagnostic::error("C012", message, expression.span));
                    return;
                }
            };
            let expected = targets.keys().copied().collect::<BTreeSet<_>>();
            if actual.known.is_empty() {
                errors.push(Diagnostic::error(
                    "C012",
                    "result(call) requires a call with a recoverable error set",
                    expression.span,
                ));
            } else if actual.known != expected {
                errors.push(Diagnostic::error(
                    "C012",
                    format!(
                        "result capture errors do not match the call: expected [{}], found [{}]",
                        failure_names(program, &expected),
                        failure_names(program, &actual.known),
                    ),
                    expression.span,
                ));
            }
        });
    }
}

fn validate_payloads(
    program: &Program,
    summaries: &[FailureSummary],
    errors: &mut Vec<Diagnostic>,
) {
    let failures = summaries
        .iter()
        .flat_map(|summary| summary.known.iter())
        .copied()
        .collect::<BTreeSet<_>>();
    for failure in failures {
        let Shape::Choice(variants) = &program.types[failure.ty].shape else {
            unreachable!("failure type is a choice")
        };
        if variants[failure.variant]
            .fields
            .iter()
            .any(|field| !field.ty.transferable(&program.types))
        {
            errors.push(Diagnostic::error(
                "C005",
                "error payloads must be transferable across execution boundaries",
                program.types[failure.ty].span,
            ));
        }
    }
}

pub(crate) fn failure_names(program: &Program, failures: &BTreeSet<Failure>) -> String {
    failures
        .iter()
        .map(|failure| {
            let Shape::Choice(variants) = &program.types[failure.ty].shape else {
                unreachable!("failure type is a choice")
            };
            format!(
                "{}.{}",
                program.types[failure.ty].name, variants[failure.variant].name
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}
