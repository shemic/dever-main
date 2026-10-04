use std::collections::{BTreeMap, BTreeSet};

use crate::hir::{CallArgument, Clause, Expression, ExpressionKind as Expr, Statement};
use crate::types::{Definition, Shape, Type};

/// 更新副本只读取旧值中尚未修改的字段时，旧局部可直接转交给副本。
/// 在已完成契约检查的 HIR 副本上执行，不改变诊断使用的源码事实。
pub(super) fn coalesce(clause: &mut Clause, types: &[Definition]) {
    for index in 0..clause.body.len() {
        let Statement::Assign {
            slot: target,
            fields,
            value,
        } = &clause.body[index]
        else {
            continue;
        };
        let Expr::Local(source) = value.kind else {
            continue;
        };
        let (target, source) = (*target, source);
        if target == source || !fields.is_empty() || clause.outputs.contains(&source) {
            continue;
        }
        let Type::Named(id) = value.ty else { continue };
        if !matches!(types[id].shape, Shape::Record(_)) || !value.ty.movable(types) {
            continue;
        }
        let mut changed = Vec::<Vec<usize>>::new();
        let mut safe = true;
        for statement in &mut clause.body[index + 1..] {
            visit_reads(statement_value(statement), &mut |read| {
                if let Some((slot, path)) = place(read)
                    && slot == source
                    && (path.is_empty() || changed.iter().any(|write| overlaps(write, &path)))
                {
                    safe = false;
                }
            });
            if let Statement::Assign { slot, fields, .. } = statement {
                if *slot == source {
                    safe = false;
                }
                if *slot == target {
                    changed.push(fields.clone());
                }
            }
            if !safe {
                break;
            }
        }
        if safe {
            for statement in &mut clause.body[index + 1..] {
                visit_reads(statement_value(statement), &mut |read| {
                    rename_place(read, source, target);
                });
            }
        }
    }
}

/// 大记录传给下一步后若只剩标量读取，先保留这些标量，避免为它们复制整个记录。
pub(super) fn snapshot_scalars(clause: &mut Clause, types: &[Definition]) {
    let mut index = 0;
    while index < clause.body.len() {
        let mut sources = BTreeSet::new();
        visit_reads(statement_value(&mut clause.body[index]), &mut |read| {
            if let Expr::Local(slot) = read.kind
                && matches!(read.ty, Type::Named(_))
                && read.ty.movable(types)
            {
                sources.insert(slot);
            }
        });
        for source in sources {
            if clause.outputs.contains(&source)
                || matches!(&clause.body[index], Statement::Assign { slot, .. } if *slot == source)
            {
                continue;
            }
            let mut snapshots = BTreeMap::new();
            let mut safe = true;
            for statement in &mut clause.body[index + 1..] {
                if matches!(statement, Statement::Assign { slot, .. } if *slot == source) {
                    safe = false;
                    break;
                }
                visit_reads(statement_value(statement), &mut |read| {
                    if let Some((slot, path)) = place(read)
                        && slot == source
                    {
                        if !path.is_empty()
                            && matches!(
                                read.ty,
                                Type::Bool | Type::Int | Type::Float | Type::Decimal
                            )
                        {
                            snapshots.entry(path).or_insert_with(|| read.clone());
                        } else {
                            safe = false;
                        }
                    }
                });
                if !safe {
                    break;
                }
            }
            if !safe || snapshots.is_empty() {
                continue;
            }
            let mut slots = BTreeMap::new();
            let mut assignments = Vec::new();
            for (path, value) in snapshots {
                let slot = clause.locals.len();
                clause.locals.push(value.ty.clone());
                slots.insert(path, slot);
                assignments.push(Statement::Assign {
                    slot,
                    fields: Vec::new(),
                    value,
                });
            }
            for statement in &mut clause.body[index + 1..] {
                visit_reads(statement_value(statement), &mut |read| {
                    if let Some((slot, path)) = place(read)
                        && slot == source
                    {
                        read.kind = Expr::Local(slots[&path]);
                    }
                });
            }
            let added = assignments.len();
            clause.body.splice(index..index, assignments);
            index += added;
        }
        index += 1;
    }
}

fn rename_place(expression: &mut Expression, source: usize, target: usize) {
    match &mut expression.kind {
        Expr::Local(slot) if *slot == source => *slot = target,
        Expr::Field { value, .. } => rename_place(value, source, target),
        _ => {}
    }
}

pub(super) fn overlaps(left: &[usize], right: &[usize]) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

pub(crate) fn place(expression: &Expression) -> Option<(usize, Vec<usize>)> {
    let mut base = expression;
    let mut path = Vec::new();
    while let Expr::Field { value, index } = &base.kind {
        path.push(*index);
        base = value;
    }
    let Expr::Local(slot) = base.kind else {
        return None;
    };
    path.reverse();
    Some((slot, path))
}

fn statement_value(statement: &mut Statement) -> &mut Expression {
    match statement {
        Statement::Assign { value, .. } | Statement::Call(value) => value,
    }
}

fn visit_reads(expression: &mut Expression, apply: &mut impl FnMut(&mut Expression)) {
    if place(expression).is_some() {
        apply(expression);
        return;
    }
    match &mut expression.kind {
        Expr::Local(_) | Expr::Constant(_) | Expr::ChoiceOptions { .. } => {}
        Expr::JobEnqueue { arguments, .. } => {
            for value in arguments {
                visit_reads(value, apply);
            }
        }
        Expr::ModelOperation { operation, .. } => {
            operation.visit_mut(&mut |value| visit_reads(value, apply));
        }
        Expr::Field { value, .. }
        | Expr::Fail(value)
        | Expr::AwaitTask(value)
        | Expr::StopTask(value)
        | Expr::Group(value)
        | Expr::AwaitGroup(value)
        | Expr::StopGroup(value)
        | Expr::ChannelReceive(value)
        | Expr::ChannelClose(value)
        | Expr::Promote(value)
        | Expr::Some(value)
        | Expr::Unary { value, .. } => visit_reads(value, apply),
        Expr::Call { arguments, .. }
        | Expr::CaptureResult { arguments, .. }
        | Expr::ParallelCall { arguments, .. }
        | Expr::BlockingCall { arguments, .. } => {
            for argument in arguments {
                if let CallArgument::Value(value) = argument {
                    visit_reads(value, apply);
                }
            }
        }
        Expr::RunCall {
            group, arguments, ..
        } => {
            if let Some(group) = group {
                visit_reads(group, apply);
            }
            for argument in arguments {
                if let CallArgument::Value(value) = argument {
                    visit_reads(value, apply);
                }
            }
        }
        Expr::Channel { capacity, .. } => visit_reads(capacity, apply),
        Expr::ChannelSend { channel, value } => {
            visit_reads(channel, apply);
            visit_reads(value, apply);
        }
        Expr::Intrinsic { arguments, .. }
        | Expr::Collection { arguments, .. }
        | Expr::Variant { arguments, .. }
        | Expr::List(arguments) => {
            for argument in arguments {
                visit_reads(argument, apply);
            }
        }
        Expr::Record { fields } => {
            for (_, value) in fields {
                visit_reads(value, apply);
            }
        }
        Expr::Map(entries) => {
            for (key, value) in entries {
                visit_reads(key, apply);
                visit_reads(value, apply);
            }
        }
        Expr::Binary { left, right, .. } => {
            visit_reads(left, apply);
            visit_reads(right, apply);
        }
    }
}
