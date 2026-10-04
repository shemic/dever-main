use crate::diagnostic::Diagnostic;
use crate::hir::{
    CallArgument, CallTarget, Expression, ExpressionKind, Function, HandlerTarget, Statement,
};
use crate::source::Span;
use crate::types::{Definition, Parameter};

use super::{
    Symbols,
    symbols::{referenced_types, shape_fields},
};

pub(super) fn check(
    functions: &[Function],
    types: &[Definition],
    symbols: &Symbols,
    errors: &mut Vec<Diagnostic>,
) {
    let mut calls = vec![Vec::new(); functions.len()];
    let mut dependencies = vec![Vec::new(); symbols.packages.len()];
    for definition in types {
        for field in shape_fields(&definition.shape) {
            referenced_types(&field.ty, &mut |id| {
                dependency(
                    &mut dependencies,
                    definition.owner,
                    types[id].owner,
                    definition.span,
                    symbols,
                )
            });
        }
    }
    for (caller, function) in functions.iter().enumerate() {
        for ty in function
            .parameters
            .iter()
            .flat_map(|parameter| match parameter {
                Parameter::Value(ty) => vec![ty],
                Parameter::Handler(signature) => signature
                    .parameters
                    .iter()
                    .chain(signature.outputs.iter().map(|field| &field.ty))
                    .collect(),
            })
            .chain(function.outputs.iter().map(|field| &field.ty))
        {
            referenced_types(ty, &mut |id| {
                dependency(
                    &mut dependencies,
                    function.owner,
                    types[id].owner,
                    function.span,
                    symbols,
                )
            });
        }
        visit(function, |expression| {
            referenced_types(&expression.ty, &mut |id| {
                dependency(
                    &mut dependencies,
                    function.owner,
                    types[id].owner,
                    expression.span,
                    symbols,
                )
            });
            let callee = match expression.kind.static_call().map(|(target, _)| target) {
                Some(CallTarget::Function(function))
                | Some(CallTarget::Handler(HandlerTarget::Function(function))) => Some(function),
                _ => match expression.kind.callback() {
                    Some(HandlerTarget::Function(function)) => Some(function),
                    _ => None,
                },
            };
            if let Some(callee) = callee {
                calls[caller].push((callee, expression.span));
                dependency(
                    &mut dependencies,
                    function.owner,
                    functions[callee].owner,
                    expression.span,
                    symbols,
                );
            }
            if let Some((_, arguments)) = expression.kind.static_call() {
                for argument in arguments {
                    if let CallArgument::Handler(HandlerTarget::Function(target)) = argument {
                        dependency(
                            &mut dependencies,
                            function.owner,
                            functions[*target].owner,
                            expression.span,
                            symbols,
                        );
                    }
                }
            }
        });
    }
    for (graph, message) in [
        (&calls, "recursive function calls are not allowed"),
        (&dependencies, "cyclic package dependencies are not allowed"),
    ] {
        if let Some(span) = cycle(graph) {
            errors.push(Diagnostic::error("C007", message, span));
        }
    }
    let roots = functions
        .iter()
        .enumerate()
        .filter(|(_, function)| {
            function
                .parameters
                .iter()
                .all(|parameter| matches!(parameter, Parameter::Value(_)))
        })
        .map(|(function, _)| crate::specialize::Specialization {
            function,
            handlers: Vec::new(),
        });
    if let Err(span) = crate::specialize::reachable(functions, roots) {
        errors.push(Diagnostic::error(
            "C007",
            "recursive specialized handler calls are not allowed",
            span,
        ));
    }
}

// Explicit DFS frames keep long acyclic call chains off the compiler's call stack.
pub(super) fn cycle(graph: &[Vec<(usize, Span)>]) -> Option<Span> {
    let mut state = vec![0_u8; graph.len()];
    for root in 0..graph.len() {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        let mut pending = vec![(root, 0)];
        while let Some((node, edge)) = pending.last_mut() {
            if *edge == graph[*node].len() {
                state[*node] = 2;
                pending.pop();
                continue;
            }
            let (next, span) = graph[*node][*edge];
            *edge += 1;
            match state[next] {
                1 => return Some(span),
                0 => {
                    state[next] = 1;
                    pending.push((next, 0));
                }
                _ => {}
            }
        }
    }
    None
}

fn dependency(
    graph: &mut [Vec<(usize, Span)>],
    owner: usize,
    target: usize,
    span: Span,
    symbols: &Symbols,
) {
    let same_domain = symbols.packages[owner].layout.domain().is_some()
        && symbols.packages[owner].layout.domain() == symbols.packages[target].layout.domain();
    if owner != target && !same_domain {
        graph[owner].push((target, span));
    }
}

pub(crate) fn visit(function: &Function, mut apply: impl FnMut(&Expression)) {
    let mut pending = Vec::new();
    for clause in &function.clauses {
        for statement in &clause.body {
            pending.push(match statement {
                Statement::Assign { value, .. } | Statement::Call(value) => value,
            });
        }
    }
    visit_pending(pending, &mut apply);
}

pub(crate) fn visit_expression(expression: &Expression, mut apply: impl FnMut(&Expression)) {
    visit_pending(vec![expression], &mut apply);
}

fn visit_pending<'a>(mut pending: Vec<&'a Expression>, apply: &mut impl FnMut(&'a Expression)) {
    while let Some(expression) = pending.pop() {
        apply(expression);
        match &expression.kind {
            ExpressionKind::Field { value, .. }
            | ExpressionKind::Fail(value)
            | ExpressionKind::Unary { value, .. }
            | ExpressionKind::Promote(value)
            | ExpressionKind::Some(value)
            | ExpressionKind::AwaitTask(value)
            | ExpressionKind::StopTask(value)
            | ExpressionKind::Group(value)
            | ExpressionKind::AwaitGroup(value)
            | ExpressionKind::StopGroup(value)
            | ExpressionKind::ChannelReceive(value)
            | ExpressionKind::ChannelClose(value) => pending.push(value),
            ExpressionKind::Call { arguments, .. }
            | ExpressionKind::CaptureResult { arguments, .. }
            | ExpressionKind::ParallelCall { arguments, .. }
            | ExpressionKind::BlockingCall { arguments, .. } => {
                pending.extend(arguments.iter().filter_map(|argument| match argument {
                    CallArgument::Value(value) => Some(value),
                    CallArgument::Handler(_) => None,
                }));
            }
            ExpressionKind::RunCall {
                group, arguments, ..
            } => {
                pending.extend(group.iter().map(Box::as_ref));
                pending.extend(arguments.iter().filter_map(|argument| match argument {
                    CallArgument::Value(value) => Some(value),
                    CallArgument::Handler(_) => None,
                }));
            }
            ExpressionKind::ModelOperation { operation, .. } => {
                pending.extend(operation.expressions());
            }
            ExpressionKind::Channel { capacity, .. } => pending.push(capacity),
            ExpressionKind::ChannelSend { channel, value } => {
                pending.extend([channel.as_ref(), value.as_ref()]);
            }
            ExpressionKind::Intrinsic { arguments, .. }
            | ExpressionKind::JobEnqueue { arguments, .. }
            | ExpressionKind::Collection { arguments, .. }
            | ExpressionKind::Variant { arguments, .. }
            | ExpressionKind::List(arguments) => pending.extend(arguments),
            ExpressionKind::Record { fields } => {
                pending.extend(fields.iter().map(|(_, value)| value));
            }
            ExpressionKind::Map(entries) => {
                for (key, value) in entries {
                    pending.extend([key, value]);
                }
            }
            ExpressionKind::Binary { left, right, .. } => {
                pending.extend([left.as_ref(), right.as_ref()]);
            }
            ExpressionKind::Constant(_)
            | ExpressionKind::Local(_)
            | ExpressionKind::ChoiceOptions { .. } => {}
        }
    }
}
