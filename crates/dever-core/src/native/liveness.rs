use std::collections::HashSet;

use crate::hir::{CallArgument, Clause, Expression, ExpressionKind, Statement};

/// HIR 在发射期间不可变；节点地址只标识同一子句中的具体读取，不进入生成程序。
pub(crate) fn last_uses(clause: &Clause) -> HashSet<*const Expression> {
    let mut live: HashSet<usize> = clause.outputs.iter().copied().collect();
    let mut moves = HashSet::new();
    for statement in clause.body.iter().rev() {
        let mut overwritten = None;
        let value = match statement {
            Statement::Assign {
                slot,
                fields,
                value,
            } => {
                if fields.is_empty() {
                    live.remove(slot);
                } else {
                    // 字段赋值必须保留宿主的其余字段，不能在右侧读值时移动宿主。
                    live.insert(*slot);
                    overwritten = Some((*slot, fields.clone()));
                }
                value
            }
            Statement::Call(value) => value,
        };
        visit(value, &mut live, &mut moves, &mut overwritten);
    }
    moves
}

fn visit(
    expression: &Expression,
    live: &mut HashSet<usize>,
    moves: &mut HashSet<*const Expression>,
    overwritten: &mut Option<(usize, Vec<usize>)>,
) {
    use ExpressionKind::*;
    if let Option::Some((slot, path)) = super::updates::place(expression) {
        if let Option::Some((target, fields)) = overwritten
            && *target == slot
            && super::updates::overlaps(fields, &path)
        {
            // 右侧最后一次重叠读取必须恰好是被覆盖字段，不能提前移动整个宿主。
            if *fields == path {
                moves.insert(expression as *const Expression);
            }
            *overwritten = None;
        }
        let mut base = expression;
        while let Field { value, .. } = &base.kind {
            base = value;
        }
        if live.insert(slot) {
            moves.insert(base as *const Expression);
        }
        return;
    }
    match &expression.kind {
        Local(slot) => {
            if live.insert(*slot) {
                moves.insert(expression as *const Expression);
            }
        }
        Constant(_) | ChoiceOptions { .. } => {}
        ModelOperation { operation, .. } => {
            for value in operation.expressions().into_iter().rev() {
                visit(value, live, moves, overwritten);
            }
        }
        JobEnqueue { arguments, .. } => {
            for value in arguments.iter().rev() {
                visit(value, live, moves, overwritten);
            }
        }
        Field { value, .. }
        | Fail(value)
        | AwaitTask(value)
        | StopTask(value)
        | Group(value)
        | AwaitGroup(value)
        | StopGroup(value)
        | ChannelReceive(value)
        | ChannelClose(value)
        | Promote(value)
        | Some(value)
        | Unary { value, .. } => visit(value, live, moves, overwritten),
        Call { arguments, .. }
        | CaptureResult { arguments, .. }
        | ParallelCall { arguments, .. }
        | BlockingCall { arguments, .. } => {
            for argument in arguments.iter().rev() {
                if let CallArgument::Value(value) = argument {
                    visit(value, live, moves, overwritten);
                }
            }
        }
        RunCall {
            group, arguments, ..
        } => {
            for argument in arguments.iter().rev() {
                if let CallArgument::Value(value) = argument {
                    visit(value, live, moves, overwritten);
                }
            }
            if let Option::Some(group) = group {
                visit(group, live, moves, overwritten);
            }
        }
        Channel { capacity, .. } => visit(capacity, live, moves, overwritten),
        ChannelSend { channel, value } => {
            visit(value, live, moves, overwritten);
            visit(channel, live, moves, overwritten);
        }
        Intrinsic { arguments, .. }
        | Collection { arguments, .. }
        | Variant { arguments, .. }
        | List(arguments) => {
            for argument in arguments.iter().rev() {
                visit(argument, live, moves, overwritten);
            }
        }
        Record { fields } => {
            for (_, value) in fields.iter().rev() {
                visit(value, live, moves, overwritten);
            }
        }
        Map(entries) => {
            for (key, value) in entries.iter().rev() {
                visit(value, live, moves, overwritten);
                visit(key, live, moves, overwritten);
            }
        }
        Binary { left, right, .. } => {
            visit(right, live, moves, overwritten);
            visit(left, live, moves, overwritten);
        }
    }
}
