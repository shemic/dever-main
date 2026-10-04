use std::collections::BTreeSet;

use crate::hir::{
    CallArgument, CallTarget, Clause, CollectionOp, Expression, ExpressionKind as Expr, Projection,
    SequenceKind, Statement,
};
use crate::intrinsic::Intrinsic;
use crate::specialize::{self, Specialization};
use crate::syntax::{BinaryOperator, UnaryOperator};
use crate::types::{Parameter, Type};

use super::Emitter;

impl Emitter<'_> {
    pub(super) fn inline_sequences(&self, clause: &mut Clause) {
        let mut index = 0;
        while index + 1 < clause.body.len() {
            let Statement::Assign {
                slot,
                fields,
                value,
            } = &clause.body[index]
            else {
                index += 1;
                continue;
            };
            if !fields.is_empty() || clause.outputs.contains(slot) || !self.fusible(value) {
                index += 1;
                continue;
            }
            let next = match &clause.body[index + 1] {
                Statement::Assign { value, .. } | Statement::Call(value) => value,
            };
            let Expr::Collection { arguments, .. } = &next.kind else {
                index += 1;
                continue;
            };
            if !matches!(arguments[0].kind, Expr::Local(id) if id == *slot)
                || !super::liveness::last_uses(clause)
                    .contains(&(&arguments[0] as *const Expression))
            {
                index += 1;
                continue;
            }
            let replacement = value.clone();
            let next = match &mut clause.body[index + 1] {
                Statement::Assign { value, .. } | Statement::Call(value) => value,
            };
            let Expr::Collection { arguments, .. } = &mut next.kind else {
                unreachable!()
            };
            arguments[0] = replacement;
            clause.body.remove(index);
        }
    }

    fn fusible(&self, expression: &Expression) -> bool {
        let Expr::Collection {
            operation,
            handler: Some(handler),
            arguments,
            sequence,
            ..
        } = &expression.kind
        else {
            return false;
        };
        matches!(operation, CollectionOp::Each | CollectionOp::Filter)
            && matches!(sequence, Some(SequenceKind::List | SequenceKind::Bytes))
            && arguments.len() == 1
            && matches!(expression.ty, Type::List(_))
            && expression.ty.movable(&self.program.types)
            && arguments[0].ty.movable(&self.program.types)
            && self.total_handler(specialize::resolve_handler(*handler, &self.bindings))
    }

    pub(super) fn fused_iterator(&self, expression: &Expression) -> Option<(String, String)> {
        if !self.fusible(expression) {
            return None;
        }
        let Expr::Collection {
            operation,
            handler: Some(handler),
            arguments,
            sequence,
            ..
        } = &expression.kind
        else {
            unreachable!()
        };
        let (bindings, iterator) = self.fused_iterator(&arguments[0]).unwrap_or_else(|| {
            if let Some([text, separator]) = self.split_arguments(&arguments[0]) {
                return (
                    format!(
                        "let pipeline_text = {}; let pipeline_separator = {};",
                        self.expression(text),
                        self.expression(separator)
                    ),
                    "dever_runtime::text::pieces(&pipeline_text, &pipeline_separator)".into(),
                );
            }
            (
                format!("let pipeline_source = {};", self.expression(&arguments[0])),
                match sequence.expect("checked sequence") {
                    SequenceKind::List => "pipeline_source.into_values()",
                    SequenceKind::Bytes => {
                        "pipeline_source.values().iter().copied().map(i64::from)"
                    }
                    SequenceKind::Stream | SequenceKind::AsyncStream | SequenceKind::RowStream => {
                        unreachable!()
                    }
                }
                .into(),
            )
        });
        let target = Specialization {
            function: specialize::resolve_handler(*handler, &self.bindings),
            handlers: Vec::new(),
        };
        let name = &self.names[&target];
        let stage = if *operation == CollectionOp::Each {
            format!("map(|element| {name}(element).expect(\"checked total handler\"))")
        } else {
            format!("filter(|element| {name}(element.clone()).expect(\"checked total handler\"))")
        };
        Some((bindings, format!("{iterator}.{stage}")))
    }

    // pure 只证明无外部效果；融合还要求所有可达操作都不会产生程序故障。
    fn total_handler(&self, root: usize) -> bool {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let function = &self.program.functions[id];
            if function.suspends || !self.program.suspension_handlers[id].is_empty() || !self.program.effects[id].is_empty() || !self.program.failures[id].is_empty() || !self.program.failure_handlers[id].is_empty() || function.parameters.iter().any(|parameter| !matches!(parameter, Parameter::Value(ty) if ty.movable(&self.program.types))) {
                return false;
            }
            let mut total = true;
            crate::check::visit(function, |expression| {
                total &= match &expression.kind {
                    Expr::Constant(_)
                    | Expr::Local(_)
                    | Expr::Field { .. }
                    | Expr::Record { .. }
                    | Expr::Variant { .. }
                    | Expr::List(_)
                    | Expr::Some(_)
                    | Expr::Promote(_) => true,
                    Expr::Unary { operator, .. } => {
                        *operator == UnaryOperator::Not || expression.ty == Type::Float
                    }
                    Expr::Binary { operator, .. } => {
                        matches!(
                            operator,
                            BinaryOperator::Equal
                                | BinaryOperator::NotEqual
                                | BinaryOperator::Less
                                | BinaryOperator::LessEqual
                                | BinaryOperator::Greater
                                | BinaryOperator::GreaterEqual
                                | BinaryOperator::And
                                | BinaryOperator::Or
                        ) || matches!(expression.ty, Type::Float | Type::Text)
                    }
                    Expr::Collection { operation, .. } => matches!(
                        operation,
                        CollectionOp::Length
                            | CollectionOp::First
                            | CollectionOp::Get
                            | CollectionOp::Entries
                    ),
                    Expr::Call {
                        target: CallTarget::Function(callee),
                        arguments,
                    } if arguments
                        .iter()
                        .all(|argument| matches!(argument, CallArgument::Value(_))) =>
                    {
                        pending.push(*callee);
                        true
                    }
                    Expr::Intrinsic { operation, .. } => matches!(
                        operation,
                        Intrinsic::TextTrim
                            | Intrinsic::TextLower
                            | Intrinsic::TextUpper
                            | Intrinsic::TextCodepoint
                            | Intrinsic::TextFromCodepoint
                            | Intrinsic::TextAt
                            | Intrinsic::TextSlice
                            | Intrinsic::TextIndexOf
                            | Intrinsic::TextContains
                            | Intrinsic::TextStartsWith
                            | Intrinsic::TextEndsWith
                            | Intrinsic::TextSplit
                            | Intrinsic::TextReplace
                            | Intrinsic::IntToText
                            | Intrinsic::FloatToText
                            | Intrinsic::FloatFromInt
                    ),
                    _ => false,
                };
            });
            if !total {
                return false;
            }
        }
        true
    }

    /// 普通单表达式转发函数和直接原语共用识别，不依赖标准包或样例名称。
    pub(super) fn split_arguments<'a>(
        &self,
        expression: &'a Expression,
    ) -> Option<[&'a Expression; 2]> {
        match &expression.kind {
            Expr::Intrinsic {
                operation: Intrinsic::TextSplit,
                arguments,
                ..
            } => Some([&arguments[0], &arguments[1]]),
            Expr::Call {
                target: CallTarget::Function(id),
                arguments,
            } => {
                let [clause] = self.program.functions[*id].clauses.as_slice() else {
                    return None;
                };
                let [
                    Statement::Assign {
                        slot,
                        fields,
                        value,
                    },
                ] = clause.body.as_slice()
                else {
                    return None;
                };
                if !fields.is_empty() || clause.outputs != [*slot] {
                    return None;
                }
                let Expr::Intrinsic {
                    operation: Intrinsic::TextSplit,
                    arguments: forwarded,
                    ..
                } = &value.kind
                else {
                    return None;
                };
                let input = |expression: &Expression| {
                    let Expr::Local(slot) = expression.kind else {
                        return None;
                    };
                    let binding = clause.bindings.iter().find(|binding| {
                        binding.slot == slot && matches!(binding.projection, Projection::Whole)
                    })?;
                    let CallArgument::Value(value) = &arguments[binding.input] else {
                        return None;
                    };
                    Some((binding.input, value))
                };
                let (left, text) = input(&forwarded[0])?;
                let (right, separator) = input(&forwarded[1])?;
                // 不能删除额外参数的副作用，或重排调用方参数求值。
                (arguments.len() == 2 && left == 0 && right == 1).then_some([text, separator])
            }
            _ => None,
        }
    }
}
