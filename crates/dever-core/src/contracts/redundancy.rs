use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::diagnostic::{Diagnostic, Label};
use crate::hir::{
    CallArgument, CallTarget, Clause, Expression, ExpressionKind as Expr, Function, Program,
    Projection, Statement,
};
use crate::source::Span;
use crate::types::{Parameter, Type};

pub(crate) fn check(program: &mut Program) {
    let mut warnings = Vec::new();
    let mut implementations = BTreeMap::new();
    for (id, function) in program.functions.iter().enumerate() {
        if function.port.is_some() || function.setting.is_some() {
            continue;
        }
        if !function.public {
            let key = (function.owner, function_key(function));
            if let Some(previous) = implementations.get(&key).copied() {
                let previous: &Function = &program.functions[previous];
                warnings.push(related("W003", format!("private function '{}' has the same implementation and contract as '{}'; consider sharing one implementation", function.name, previous.name), function.span, previous.span));
            } else {
                implementations.insert(key, id);
                if let Some(target) = forwarding_target(function, program) {
                    warnings.push(related("W005", format!("private function '{}' only forwards unchanged inputs to '{}' without adding a contract; review whether this wrapper is needed", function.name, target.name), function.span, target.span));
                }
            }
        }
        for clause in &function.clauses {
            repeated_calls(clause, program, &mut warnings);
        }
    }
    program.warnings.extend(warnings);
}

fn related(code: &'static str, message: String, primary: Span, previous: Span) -> Diagnostic {
    let mut warning = Diagnostic::error(code, message, primary);
    warning.related.push(Label {
        span: previous,
        message: "previous implementation or computation".into(),
    });
    warning
}

fn function_key(function: &Function) -> String {
    let mut key = format!(
        "{:?}|{:?}|{}|{}|{:?}",
        function.parameters, function.outputs, function.suspends, function.pure, function.recovery
    );
    for clause in &function.clauses {
        write!(
            key,
            "clause({:?}|{:?}|{:?}|{:?})",
            clause.patterns, clause.bindings, clause.locals, clause.outputs
        )
        .expect("string formatting");
        for statement in &clause.body {
            match statement {
                Statement::Assign {
                    slot,
                    fields,
                    value,
                } => {
                    write!(key, "assign({slot},{fields:?})").expect("string formatting");
                    expression_key(value, &[], &mut key);
                }
                Statement::Call(value) => {
                    key.push_str("action");
                    expression_key(value, &[], &mut key);
                }
            }
        }
    }
    key
}

/// Structural encoding has explicit node delimiters and never includes source positions.
fn expression_key(expression: &Expression, versions: &[usize], key: &mut String) {
    write!(key, "({:?}:", expression.ty).expect("string formatting");
    match &expression.kind {
        Expr::Constant(value) => write!(key, "constant{value:?}").expect("string formatting"),
        Expr::Local(slot) => write!(
            key,
            "local{slot}@{}",
            versions.get(*slot).copied().unwrap_or_default()
        )
        .expect("string formatting"),
        Expr::Field { value, index } => {
            write!(key, "field{index}").expect("string formatting");
            expression_key(value, versions, key);
        }
        Expr::ChoiceOptions { choice } => {
            write!(key, "choice_options{choice}").expect("string formatting");
        }
        Expr::ModelOperation { model, operation } => {
            write!(key, "model{model}/{}", model_operation_name(operation))
                .expect("string formatting");
            for value in operation.expressions() {
                expression_key(value, versions, key);
            }
        }
        Expr::JobEnqueue {
            target,
            arguments,
            scheduled,
        } => {
            write!(key, "enqueue{target}/{scheduled}").expect("string formatting");
            for value in arguments {
                expression_key(value, versions, key);
            }
        }
        Expr::Call { target, arguments } => {
            write!(key, "call{target:?}").expect("string formatting");
            call_arguments_key(arguments, versions, key);
        }
        Expr::Fail(error) => {
            key.push_str("fail");
            expression_key(error, versions, key);
        }
        Expr::CaptureResult {
            target,
            arguments,
            choice,
            success,
        } => {
            write!(key, "capture{target:?}/{choice}/{success}").expect("string formatting");
            call_arguments_key(arguments, versions, key);
        }
        Expr::RunCall {
            group,
            target,
            arguments,
        } => {
            write!(key, "run_call{target:?}").expect("string formatting");
            if let Some(group) = group {
                expression_key(group, versions, key);
            }
            call_arguments_key(arguments, versions, key);
        }
        Expr::ParallelCall { target, arguments } => {
            write!(key, "parallel_call{target:?}").expect("string formatting");
            call_arguments_key(arguments, versions, key);
        }
        Expr::BlockingCall { target, arguments } => {
            write!(key, "blocking_call{target:?}").expect("string formatting");
            call_arguments_key(arguments, versions, key);
        }
        Expr::AwaitTask(value) => {
            key.push_str("await_task");
            expression_key(value, versions, key);
        }
        Expr::StopTask(value) => {
            key.push_str("stop_task");
            expression_key(value, versions, key);
        }
        Expr::Group(value) => {
            key.push_str("group");
            expression_key(value, versions, key);
        }
        Expr::AwaitGroup(value) => {
            key.push_str("await_group");
            expression_key(value, versions, key);
        }
        Expr::StopGroup(value) => {
            key.push_str("stop_group");
            expression_key(value, versions, key);
        }
        Expr::Channel { element, capacity } => {
            write!(key, "channel{element:?}").expect("string formatting");
            expression_key(capacity, versions, key);
        }
        Expr::ChannelSend { channel, value } => {
            key.push_str("channel_send");
            expression_key(channel, versions, key);
            expression_key(value, versions, key);
        }
        Expr::ChannelReceive(value) => {
            key.push_str("channel_receive");
            expression_key(value, versions, key);
        }
        Expr::ChannelClose(value) => {
            key.push_str("channel_close");
            expression_key(value, versions, key);
        }
        Expr::Intrinsic {
            operation,
            handler,
            arguments,
        } => {
            write!(key, "intrinsic{operation:?}/{handler:?}").expect("string formatting");
            arguments_key(arguments, versions, key);
        }
        Expr::Collection {
            operation,
            handler,
            sequence,
            arguments,
        } => {
            write!(key, "collection{operation:?}/{handler:?}/{sequence:?}")
                .expect("string formatting");
            arguments_key(arguments, versions, key);
        }
        Expr::Record { fields } => {
            key.push_str("record");
            for (index, value) in fields {
                write!(key, "field{index}").expect("string formatting");
                expression_key(value, versions, key);
            }
        }
        Expr::Variant { variant, arguments } => {
            write!(key, "variant{variant}").expect("string formatting");
            arguments_key(arguments, versions, key);
        }
        Expr::List(values) => {
            key.push_str("list");
            arguments_key(values, versions, key);
        }
        Expr::Map(entries) => {
            key.push_str("map");
            for (entry_key, value) in entries {
                expression_key(entry_key, versions, key);
                expression_key(value, versions, key);
            }
        }
        Expr::Unary { operator, value } => {
            write!(key, "unary{operator:?}").expect("string formatting");
            expression_key(value, versions, key);
        }
        Expr::Binary {
            left,
            operator,
            right,
        } => {
            write!(key, "binary{operator:?}").expect("string formatting");
            expression_key(left, versions, key);
            expression_key(right, versions, key);
        }
        Expr::Promote(value) => {
            key.push_str("promote");
            expression_key(value, versions, key);
        }
        Expr::Some(value) => {
            key.push_str("some");
            expression_key(value, versions, key);
        }
    }
    key.push(')');
}

fn model_operation_name(operation: &crate::hir::ModelOperation) -> String {
    use crate::hir::ModelOperation;
    match operation {
        ModelOperation::Create { .. } => "create".into(),
        ModelOperation::CreateMany { .. } => "create_many".into(),
        ModelOperation::Get { .. } => "get".into(),
        ModelOperation::First { .. } => "first".into(),
        ModelOperation::List { .. } => "list".into(),
        ModelOperation::Cursor { .. } => "cursor".into(),
        ModelOperation::Count { .. } => "count".into(),
        ModelOperation::Exists { .. } => "exists".into(),
        ModelOperation::Stream { .. } => "stream".into(),
        ModelOperation::Update { .. } => "update".into(),
        ModelOperation::Delete { .. } => "delete".into(),
        ModelOperation::Upsert { .. } => "upsert".into(),
        ModelOperation::Sql { operation, .. } => format!("sql{operation}"),
    }
}

fn arguments_key(arguments: &[Expression], versions: &[usize], key: &mut String) {
    for argument in arguments {
        expression_key(argument, versions, key);
    }
}

fn call_arguments_key(arguments: &[CallArgument], versions: &[usize], key: &mut String) {
    for argument in arguments {
        match argument {
            CallArgument::Value(value) => expression_key(value, versions, key),
            CallArgument::Handler(target) => {
                write!(key, "handler{target:?}").expect("string formatting");
            }
        }
    }
}

struct Computation {
    span: Span,
    result_slot: usize,
    result_version: usize,
}

fn repeated_calls(clause: &Clause, program: &Program, warnings: &mut Vec<Diagnostic>) {
    let mut versions = vec![0; clause.locals.len()];
    let mut previous = BTreeMap::<String, Computation>::new();
    for statement in &clause.body {
        let Statement::Assign {
            slot,
            fields,
            value,
        } = statement
        else {
            continue;
        };
        let candidate = fields.is_empty() && reusable_call(value, program);
        let mut key = String::new();
        if candidate {
            expression_key(value, &versions, &mut key);
            if let Some(computation) = previous.get(&key)
                && versions[computation.result_slot] == computation.result_version
            {
                warnings.push(related("W004", "this pure call repeats unchanged inputs while the earlier result is still available; consider reusing that result".into(), value.span, computation.span));
            }
        }
        // A field write invalidates every projection from the containing value.
        versions[*slot] += 1;
        if candidate {
            previous.insert(
                key,
                Computation {
                    span: value.span,
                    result_slot: *slot,
                    result_version: versions[*slot],
                },
            );
        }
    }
}

fn reusable_call(expression: &Expression, program: &Program) -> bool {
    let Expr::Call {
        target: CallTarget::Function(id),
        arguments,
    } = &expression.kind
    else {
        return false;
    };
    program.effects[*id].is_empty()
        && expression.ty.movable(&program.types)
        && arguments.iter().all(|argument| match argument {
            CallArgument::Value(value) => stable_argument(value, program),
            CallArgument::Handler(_) => true,
        })
}

fn stable_argument(expression: &Expression, program: &Program) -> bool {
    if !expression.ty.movable(&program.types) {
        return false;
    }
    match &expression.kind {
        Expr::Constant(_) | Expr::Local(_) => true,
        Expr::Field { value, .. } => stable_argument(value, program),
        _ => false,
    }
}

pub(crate) fn application_errors(
    program: &Program,
    reachable: &BTreeSet<usize>,
) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    for (id, function) in program.functions.iter().enumerate() {
        let package = &program.packages[function.owner];
        if package.bundled || function.port.is_some() || function.setting.is_some() {
            continue;
        }
        let app = package.role == Some(crate::source::SourceRole::App);
        let domain = package.role == Some(crate::source::SourceRole::Domain);
        if !app && !domain {
            continue;
        }
        if !reachable.contains(&id) {
            errors.push(Diagnostic::error(
                "C016",
                format!(
                    "function '{}' is unreachable from API, CMD, or Job production entries",
                    function.name
                ),
                function.span,
            ));
        } else if !function.public {
            if let Some(target) = forwarding_target(function, program) {
                errors.push(related("C016", format!("private function '{}' only forwards unchanged inputs to '{}' without adding a contract", function.name, target.name), function.span, target.span));
            }
        } else if app && trivial_model_wrapper(function) {
            errors.push(Diagnostic::error("C016", format!("App function '{}' only forwards a Model CRUD operation; declare rest for plain CRUD", function.name), function.span));
        }
    }
    errors.sort_by_key(|error| (error.primary.source, error.primary.start));
    errors
}

fn trivial_model_wrapper(function: &Function) -> bool {
    if function.recovery.is_some() || function.clauses.len() != 1 {
        return false;
    }
    let clause = &function.clauses[0];
    if clause
        .patterns
        .iter()
        .any(|pattern| !matches!(pattern.as_slice(), [crate::hir::Atom::Any]))
    {
        return false;
    }
    let [
        Statement::Assign {
            slot,
            fields,
            value,
        },
    ] = clause.body.as_slice()
    else {
        return false;
    };
    if !fields.is_empty() || clause.outputs.as_slice() != [*slot] {
        return false;
    }
    let Expr::ModelOperation { operation, .. } = &value.kind else {
        return false;
    };
    if !matches!(
        operation.as_ref(),
        crate::hir::ModelOperation::Get { .. } | crate::hir::ModelOperation::Create { .. }
    ) {
        return false;
    }
    operation.expressions().into_iter().all(|value| matches!(value.kind, Expr::Local(slot) if clause.bindings.iter().any(|binding| binding.slot == slot && matches!(binding.projection, Projection::Whole))))
}

fn forwarding_target<'a>(function: &Function, program: &'a Program) -> Option<&'a Function> {
    if function.recovery.is_some() || function.clauses.len() != 1 {
        return None;
    }
    if function
        .parameters
        .iter()
        .any(|parameter| !matches!(parameter, Parameter::Value(ty) if plain_scalar(ty)))
        || function
            .outputs
            .iter()
            .any(|field| !field.bounds.is_empty() || !plain_scalar(&field.ty))
    {
        return None;
    }
    let clause = &function.clauses[0];
    let expression = match clause.body.as_slice() {
        [
            Statement::Assign {
                slot,
                fields,
                value,
            },
        ] if fields.is_empty() && clause.outputs.as_slice() == [*slot] => value,
        [Statement::Call(value)] if clause.outputs.is_empty() => value,
        _ => return None,
    };
    let Expr::Call {
        target: CallTarget::Function(id),
        arguments,
    } = &expression.kind
    else {
        return None;
    };
    let target = &program.functions[*id];
    if function.owner != target.owner
        || function.parameters != target.parameters
        || function.outputs != target.outputs
        || function.pure != target.pure
        || target.recovery.is_some()
        || !program.effects[*id].is_empty()
    {
        return None;
    }
    let unchanged = arguments.iter().enumerate().all(|(input, argument)| {
        matches!(argument, CallArgument::Value(Expression { kind: Expr::Local(slot), .. }) if clause.bindings.iter().any(|binding| binding.input == input && binding.slot == *slot && matches!(binding.projection, Projection::Whole)))
    });
    unchanged.then_some(target)
}

fn plain_scalar(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Bool | Type::Int | Type::Decimal | Type::Float | Type::Text | Type::Id | Type::Bytes
    )
}
