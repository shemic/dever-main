use std::collections::{BTreeMap, BTreeSet};

use crate::hir::{
    CallArgument, CallTarget, CollectionOp, ExpressionKind, Function, HandlerTarget, SequenceKind,
};
use crate::source::Span;
use crate::types::Parameter;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Specialization {
    pub function: usize,
    pub handlers: Vec<usize>,
}

pub(crate) fn reachable(
    functions: &[Function],
    roots: impl IntoIterator<Item = Specialization>,
) -> Result<BTreeSet<Specialization>, Span> {
    let mut state = BTreeMap::<Specialization, u8>::new();
    for root in roots {
        if state.contains_key(&root) {
            continue;
        }
        state.insert(root.clone(), 1);
        let mut pending = vec![Frame::new(root, functions)];
        while let Some(frame) = pending.last_mut() {
            if frame.edge == frame.successors.len() {
                state.insert(frame.instance.clone(), 2);
                pending.pop();
                continue;
            }
            let (next, span) = frame.successors[frame.edge].clone();
            frame.edge += 1;
            match state.get(&next).copied() {
                Some(1) => return Err(span),
                Some(2) => {}
                None => {
                    state.insert(next.clone(), 1);
                    pending.push(Frame::new(next, functions));
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(state.into_keys().collect())
}

/// Job edges create new execution roots and never participate in synchronous cycle detection.
pub(crate) fn execution_reachable(
    functions: &[Function],
    roots: impl IntoIterator<Item = Specialization>,
) -> Result<BTreeSet<Specialization>, Span> {
    let mut instances = reachable(functions, roots)?;
    loop {
        let mut jobs = BTreeSet::new();
        for instance in &instances {
            crate::check::visit(&functions[instance.function], |expression| {
                if let crate::hir::ExpressionKind::JobEnqueue { target, .. } = expression.kind {
                    jobs.insert(Specialization {
                        function: target,
                        handlers: Vec::new(),
                    });
                }
            });
        }
        jobs.retain(|job| !instances.contains(job));
        if jobs.is_empty() {
            return Ok(instances);
        }
        instances.extend(reachable(functions, jobs)?);
    }
}

pub(crate) fn handler_bindings(
    function: &Function,
    instance: &Specialization,
) -> BTreeMap<usize, usize> {
    function
        .parameters
        .iter()
        .enumerate()
        .filter_map(|(index, parameter)| {
            matches!(parameter, Parameter::Handler(_)).then_some(index)
        })
        .zip(&instance.handlers)
        .map(|(parameter, function)| (parameter, *function))
        .collect()
}

pub(crate) fn suspends(program: &crate::hir::Program, instance: &Specialization) -> bool {
    let function = &program.functions[instance.function];
    if function.suspends {
        return true;
    }
    let bindings = handler_bindings(function, instance);
    program.suspension_handlers[instance.function]
        .iter()
        .any(|parameter| {
            let target = bindings[parameter];
            program.functions[target].suspends || !program.suspension_handlers[target].is_empty()
        })
}

pub(crate) fn database_effects(
    program: &crate::hir::Program,
    instance: &Specialization,
) -> BTreeSet<crate::model::DatabaseOwner> {
    let function = &program.functions[instance.function];
    let mut databases = program.database_effects[instance.function].clone();
    let bindings = handler_bindings(function, instance);
    for parameter in &program.suspension_handlers[instance.function] {
        databases.extend(&program.database_effects[bindings[parameter]]);
    }
    databases
}

pub(crate) fn failures(
    program: &crate::hir::Program,
    instance: &Specialization,
) -> BTreeSet<crate::hir::Failure> {
    let mut failures = program.failures[instance.function].clone();
    let bindings = handler_bindings(&program.functions[instance.function], instance);
    for parameter in &program.failure_handlers[instance.function] {
        failures.extend(&program.failures[bindings[parameter]]);
    }
    failures
}

pub(crate) fn has_effect(
    program: &crate::hir::Program,
    instance: &Specialization,
    effect: &str,
) -> bool {
    if program.effects[instance.function].contains(effect) {
        return true;
    }
    let function = &program.functions[instance.function];
    let bindings = handler_bindings(function, instance);
    program.suspension_handlers[instance.function]
        .iter()
        .any(|parameter| program.effects[bindings[parameter]].contains(effect))
}

pub(crate) fn writes_database(program: &crate::hir::Program, root: &Specialization) -> bool {
    let reachable = reachable(&program.functions, [root.clone()]).expect("checked call graph");
    reachable.iter().any(|instance| {
        let mut writes = false;
        crate::check::visit(&program.functions[instance.function], |expression| {
            writes |= match &expression.kind {
                crate::hir::ExpressionKind::ModelOperation { operation, .. } => {
                    operation.writes_database()
                }
                crate::hir::ExpressionKind::JobEnqueue { .. } => true,
                _ => false,
            };
        });
        writes
    })
}

pub(crate) fn uses_intrinsic(
    program: &crate::hir::Program,
    root: &Specialization,
    expected: crate::intrinsic::Intrinsic,
) -> bool {
    reachable(&program.functions, [root.clone()]).expect("checked call graph").iter().any(|instance| {
        let mut found = false;
        crate::check::visit(&program.functions[instance.function], |expression| {
            found |= matches!(expression.kind, ExpressionKind::Intrinsic { operation, .. } if operation == expected);
        });
        found
    })
}

/// Finds a database write reachable without first crossing an explicit
/// transaction function. Password crypto APIs use this to keep slow work out
/// of SQLite transactions while preserving a short, explicit write boundary.
pub(crate) fn write_outside_transaction(
    program: &crate::hir::Program,
    root: &Specialization,
) -> Option<Span> {
    let mut pending = vec![(
        root.clone(),
        program.functions[root.function].kind == crate::syntax::FunctionKind::Transaction,
    )];
    let mut visited = BTreeSet::new();
    while let Some((instance, in_transaction)) = pending.pop() {
        if !visited.insert((instance.clone(), in_transaction)) {
            continue;
        }
        if !in_transaction {
            let mut write = None;
            crate::check::visit(&program.functions[instance.function], |expression| {
                if write.is_none()
                    && match &expression.kind {
                        ExpressionKind::ModelOperation { operation, .. } => {
                            operation.writes_database()
                        }
                        ExpressionKind::JobEnqueue { .. } => true,
                        _ => false,
                    }
                {
                    write = Some(expression.span);
                }
            });
            if write.is_some() {
                return write;
            }
        }
        for (successor, _) in Frame::new(instance, &program.functions).successors {
            let successor_in_transaction = in_transaction
                || program.functions[successor.function].kind
                    == crate::syntax::FunctionKind::Transaction;
            pending.push((successor, successor_in_transaction));
        }
    }
    None
}

/// Returns whether a transaction can outlive its sequential call chain.
///
/// `blocking(call())` runs work on the bounded blocking pool, but it remains
/// structured: the caller waits for completion. Its target is already required
/// to be synchronous and cannot contain database or concurrent work. Everything
/// else classified here can overlap, escape, or stream across the transaction.
pub(crate) fn starts_transaction_unsafe_work(
    program: &crate::hir::Program,
    root: &Specialization,
) -> bool {
    let reachable = reachable(&program.functions, [root.clone()]).expect("checked call graph");
    reachable
        .iter()
        .any(|instance| function_starts_transaction_unsafe_work(program, instance.function))
}

pub(crate) fn function_starts_transaction_unsafe_work(
    program: &crate::hir::Program,
    function: usize,
) -> bool {
    let mut unsafe_work = false;
    crate::check::visit(&program.functions[function], |expression| {
        unsafe_work |= match &expression.kind {
            ExpressionKind::RunCall { .. }
            | ExpressionKind::AwaitTask(_)
            | ExpressionKind::StopTask(_)
            | ExpressionKind::Group(_)
            | ExpressionKind::AwaitGroup(_)
            | ExpressionKind::StopGroup(_)
            | ExpressionKind::ParallelCall { .. }
            | ExpressionKind::Channel { .. }
            | ExpressionKind::ChannelSend { .. }
            | ExpressionKind::ChannelReceive(_)
            | ExpressionKind::ChannelClose(_) => true,
            ExpressionKind::ModelOperation { operation, .. } => {
                matches!(
                    operation.as_ref(),
                    crate::hir::ModelOperation::Stream { .. }
                )
            }
            ExpressionKind::Intrinsic { operation, .. } => {
                operation.is_async()
                    && !matches!(operation, crate::intrinsic::Intrinsic::UploadStore)
            }
            ExpressionKind::Collection {
                operation,
                sequence,
                ..
            } => {
                *operation == CollectionOp::ParallelEach
                    || (matches!(
                        sequence,
                        Some(SequenceKind::AsyncStream | SequenceKind::RowStream)
                    ) && *operation != CollectionOp::Close)
            }
            // BlockingCall is deliberately excluded: validate_blocking_calls
            // enforces a synchronous, non-concurrent target.
            _ => false,
        };
    });
    unsafe_work
}

pub(crate) fn concrete(program: &crate::hir::Program) -> Result<BTreeSet<Specialization>, Span> {
    let roots = program
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| {
            function
                .parameters
                .iter()
                .all(|parameter| matches!(parameter, Parameter::Value(_)))
        })
        .map(|(function, _)| Specialization {
            function,
            handlers: Vec::new(),
        });
    reachable(&program.functions, roots)
}

pub(crate) fn resolve_handler(target: HandlerTarget, bindings: &BTreeMap<usize, usize>) -> usize {
    match target {
        HandlerTarget::Function(function) => function,
        HandlerTarget::Parameter(parameter) => bindings[&parameter],
    }
}

pub(crate) fn call_specialization(
    function: usize,
    arguments: &[CallArgument],
    bindings: &BTreeMap<usize, usize>,
) -> Specialization {
    Specialization {
        function,
        handlers: arguments
            .iter()
            .filter_map(|argument| match argument {
                CallArgument::Handler(target) => Some(resolve_handler(*target, bindings)),
                CallArgument::Value(_) => None,
            })
            .collect(),
    }
}

struct Frame {
    instance: Specialization,
    successors: Vec<(Specialization, Span)>,
    edge: usize,
}

impl Frame {
    fn new(instance: Specialization, functions: &[Function]) -> Self {
        let bindings = handler_bindings(&functions[instance.function], &instance);
        let mut successors = Vec::new();
        if let Some(port) = &functions[instance.function].port {
            successors.extend(port.implementations.iter().map(|function| {
                (
                    Specialization {
                        function: *function,
                        handlers: Vec::new(),
                    },
                    functions[instance.function].span,
                )
            }));
        }
        super::check::visit(&functions[instance.function], |expression| {
            if let Some((target, arguments)) = expression.kind.static_call() {
                let successor = match target {
                    CallTarget::Function(function) => {
                        call_specialization(function, arguments, &bindings)
                    }
                    CallTarget::Handler(target) => Specialization {
                        function: resolve_handler(target, &bindings),
                        handlers: Vec::new(),
                    },
                };
                successors.push((successor, expression.span));
                return;
            }
            if let Some(target) = expression.kind.callback() {
                successors.push((
                    Specialization {
                        function: resolve_handler(target, &bindings),
                        handlers: Vec::new(),
                    },
                    expression.span,
                ));
            }
        });
        Self {
            instance,
            successors,
            edge: 0,
        }
    }
}
