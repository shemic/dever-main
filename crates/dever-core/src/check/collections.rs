use crate::diagnostic::Diagnostic;
use crate::hir::{CollectionOp as Op, Expression as Value, ExpressionKind, SequenceKind};
use crate::source::Span;
use crate::syntax::{Expression, ExpressionKind as Ast};
use crate::types::{Type, output_type};

use super::{Checked, body::Body, expressions::ungroup};

pub(super) fn operation(name: &str) -> Option<Op> {
    Some(match name {
        "each" => Op::Each,
        "parallel_each" => Op::ParallelEach,
        "reduce" => Op::Reduce,
        "reduce_until" => Op::ReduceUntil,
        "filter" => Op::Filter,
        "find" => Op::Find,
        "sum" => Op::Sum,
        "append" => Op::Append,
        "first" => Op::First,
        "get" => Op::Get,
        "put" => Op::Put,
        "remove" => Op::Remove,
        "entries" => Op::Entries,
        "length" => Op::Length,
        "close" => Op::Close,
        _ => return None,
    })
}

struct Sequence {
    value: Value,
    kind: SequenceKind,
    element: Type,
}

impl Body<'_, '_> {
    pub(super) fn collection(
        &mut self,
        operation: Op,
        sources: &[Expression],
        expected: Option<&Type>,
        span: Span,
    ) -> Checked<Value> {
        if matches!(
            operation,
            Op::Each | Op::ParallelEach | Op::Filter | Op::Find | Op::Reduce | Op::ReduceUntil
        ) || (operation == Op::Sum && sources.len() != 1)
        {
            return self.handler_sequence(operation, sources, span);
        }
        let arity = match operation {
            Op::Put => 3,
            Op::Get | Op::Remove | Op::Append => 2,
            _ => 1,
        };
        if sources.len() != arity {
            return Err(Diagnostic::error(
                "C005",
                format!("collection operation expects {arity} argument(s)"),
                span,
            ));
        }
        let mut arguments = Vec::new();
        let mut sequence = None;
        let ty = match operation {
            Op::Append => {
                let list = if let Some(Type::List(_)) = expected {
                    self.expression(&sources[0], expected)?
                } else if matches!(&ungroup(&sources[0]).kind, Ast::List(elements) if elements.is_empty())
                {
                    let value = self.expression(&sources[1], None)?;
                    let hint = Type::List(Box::new(value.ty.clone()));
                    self.expression(&sources[0], Some(&hint))?
                } else {
                    self.expression(&sources[0], None)?
                };
                let Type::List(element) = &list.ty else {
                    return Err(expected_collection(span));
                };
                let value = self.expression(&sources[1], Some(element))?;
                let ty = list.ty.clone();
                arguments.extend([list, value]);
                ty
            }
            Op::Sum | Op::First => {
                let hint = expected.map(|ty| Type::List(Box::new(ty.clone())));
                let list = self.expression(
                    &sources[0],
                    hint.as_ref()
                        .filter(|_| super::expressions::needs_context(&sources[0])),
                )?;
                let Type::List(element) = &list.ty else {
                    return Err(expected_collection(span));
                };
                let ty = if operation == Op::First {
                    element.as_ref().clone().nullable()
                } else {
                    if !element.numeric() {
                        return Err(Diagnostic::error(
                            "C005",
                            "sum requires Int, Decimal or Float elements",
                            span,
                        ));
                    }
                    element.as_ref().clone()
                };
                arguments.push(list);
                if operation == Op::Sum {
                    sequence = Some(SequenceKind::List);
                }
                ty
            }
            Op::Get | Op::Put | Op::Remove => {
                let hint = if matches!(operation, Op::Put | Op::Remove) {
                    expected.filter(|ty| matches!(ty, Type::Map(_, _)))
                } else {
                    None
                };
                let map = if hint.is_none()
                    && operation == Op::Put
                    && matches!(&ungroup(&sources[0]).kind, Ast::Map(entries) if entries.is_empty())
                {
                    let key = self.expression(&sources[1], None)?;
                    let value = self.expression(&sources[2], None)?;
                    let ty = Type::Map(Box::new(key.ty), Box::new(value.ty));
                    self.expression(&sources[0], Some(&ty))?
                } else {
                    self.expression(&sources[0], hint)?
                };
                let Type::Map(key, value) = &map.ty else {
                    return Err(expected_collection(span));
                };
                let key = self.expression(&sources[1], Some(key))?;
                let ty = if operation == Op::Get {
                    value.as_ref().clone().nullable()
                } else {
                    map.ty.clone()
                };
                let replacement = if operation == Op::Put {
                    Some(self.expression(&sources[2], Some(value))?)
                } else {
                    None
                };
                arguments.extend([map, key]);
                arguments.extend(replacement);
                ty
            }
            Op::Entries => {
                let map = self.expression(&sources[0], None)?;
                let Type::Map(key, value) = &map.ty else {
                    return Err(expected_collection(span));
                };
                let ty = Type::List(Box::new(Type::MapEntry(key.clone(), value.clone())));
                arguments.push(map);
                ty
            }
            Op::Length => {
                let value = self.expression(&sources[0], None)?;
                if !matches!(
                    value.ty,
                    Type::List(_) | Type::Map(_, _) | Type::Text | Type::Bytes
                ) {
                    return Err(expected_collection(span));
                }
                arguments.push(value);
                Type::Int
            }
            Op::Close => {
                let value = self.expression(&sources[0], None)?;
                if !matches!(
                    value.ty,
                    Type::Stream(_) | Type::AsyncStream(_) | Type::RowStream(_)
                ) {
                    return Err(Diagnostic::error(
                        "C005",
                        "close requires a Stream or AsyncStream value",
                        sources[0].span,
                    ));
                }
                sequence = Some(match value.ty {
                    Type::AsyncStream(_) => SequenceKind::AsyncStream,
                    Type::RowStream(_) => SequenceKind::RowStream,
                    _ => SequenceKind::Stream,
                });
                arguments.push(value);
                Type::Unit
            }
            _ => unreachable!("handler operations dispatched above"),
        };
        Ok(Value {
            kind: ExpressionKind::Collection {
                operation,
                handler: None,
                sequence,
                arguments,
            },
            ty,
            span,
        })
    }

    fn handler_sequence(
        &mut self,
        operation: Op,
        sources: &[Expression],
        span: Span,
    ) -> Checked<Value> {
        let (handler_arity, valid_arity) = match operation {
            Op::ParallelEach => (
                sources.len().saturating_sub(2),
                (3..=4).contains(&sources.len()),
            ),
            Op::Reduce | Op::ReduceUntil => (2, sources.len() == 3),
            _ => (
                sources.len().saturating_sub(1),
                (2..=3).contains(&sources.len()),
            ),
        };
        if !valid_arity || !(1..=2).contains(&handler_arity) {
            return Err(Diagnostic::error(
                "C005",
                if matches!(operation, Op::Reduce | Op::ReduceUntil) {
                    "expected handler, sequence and initial state"
                } else if operation == Op::ParallelEach {
                    "expected handler, sequence, worker count and optional context"
                } else {
                    "expected handler, sequence and optional context"
                },
                span,
            ));
        }
        let (handler, signature) = self.resolve_handler(&sources[0], handler_arity)?;
        let sequence_value = self.sequence(&sources[1], Some(&signature.parameters[0]))?;
        if operation == Op::ParallelEach && sequence_value.kind == SequenceKind::RowStream {
            return Err(Diagnostic::error(
                "C005",
                "database row streams use sequential each to retain one bounded connection",
                sources[1].span,
            ));
        }
        if sequence_value.element != signature.parameters[0] {
            return Err(Diagnostic::error(
                "C005",
                "handler element type does not match the sequence",
                sources[0].span,
            ));
        }
        if matches!(operation, Op::Filter | Op::Find | Op::Sum)
            && sequence_value.kind != SequenceKind::List
        {
            return Err(Diagnostic::error(
                "C005",
                "this operation only accepts List sequences",
                sources[1].span,
            ));
        }
        let output = output_type(&signature.outputs);
        let mut arguments = vec![sequence_value.value];
        let ty = match operation {
            Op::ParallelEach => {
                if output != Type::Unit {
                    return Err(Diagnostic::error(
                        "C005",
                        "parallel_each requires a zero-output handler",
                        sources[0].span,
                    ));
                }
                if !signature
                    .parameters
                    .iter()
                    .all(|ty| ty.transferable(self.context.types))
                {
                    return Err(Diagnostic::error(
                        "C005",
                        "parallel_each handler inputs cannot contain Stream values",
                        sources[0].span,
                    ));
                }
                arguments.push(self.expression(&sources[2], Some(&Type::Int))?);
                if signature.parameters.len() == 2 {
                    arguments.push(self.expression(&sources[3], Some(&signature.parameters[1]))?);
                }
                Type::Unit
            }
            Op::Each => {
                if signature.parameters.len() == 2 {
                    arguments.push(self.expression(&sources[2], Some(&signature.parameters[1]))?);
                }
                if matches!(
                    sequence_value.kind,
                    SequenceKind::Stream | SequenceKind::AsyncStream | SequenceKind::RowStream
                ) && output != Type::Unit
                {
                    return Err(Diagnostic::error(
                        "C005",
                        "each on a stream requires a zero-output handler",
                        sources[0].span,
                    ));
                }
                if output == Type::Unit {
                    Type::Unit
                } else if output.value() {
                    Type::List(Box::new(output))
                } else {
                    return Err(invalid_handler(sources[0].span));
                }
            }
            Op::Filter if output == Type::Bool => {
                if signature.parameters.len() == 2 {
                    arguments.push(self.expression(&sources[2], Some(&signature.parameters[1]))?);
                }
                Type::List(Box::new(sequence_value.element.clone()))
            }
            Op::Find if output == Type::Bool => {
                if signature.parameters.len() == 2 {
                    arguments.push(self.expression(&sources[2], Some(&signature.parameters[1]))?);
                }
                sequence_value.element.clone().nullable()
            }
            Op::Sum if output.numeric() => {
                if signature.parameters.len() == 2 {
                    arguments.push(self.expression(&sources[2], Some(&signature.parameters[1]))?);
                }
                output
            }
            Op::Reduce => {
                let state = self.expression(&sources[2], Some(&signature.parameters[1]))?;
                if signature.outputs.len() != 1 || output != state.ty {
                    return Err(invalid_handler(sources[0].span));
                }
                arguments.push(state);
                output
            }
            Op::ReduceUntil => {
                let state = self.expression(&sources[2], Some(&signature.parameters[1]))?;
                if signature.outputs.len() != 2
                    || signature.outputs[0].ty != state.ty
                    || signature.outputs[1].name != "stop"
                    || signature.outputs[1].ty != Type::Bool
                {
                    return Err(Diagnostic::error(
                        "C005",
                        "reduce_until handler must output state followed by 'stop: Bool'",
                        sources[0].span,
                    ));
                }
                arguments.push(state);
                signature.parameters[1].clone()
            }
            _ => return Err(invalid_handler(sources[0].span)),
        };
        Ok(Value {
            kind: ExpressionKind::Collection {
                operation,
                handler: Some(handler),
                sequence: Some(sequence_value.kind),
                arguments,
            },
            ty,
            span,
        })
    }

    fn sequence(&mut self, source: &Expression, expected: Option<&Type>) -> Checked<Sequence> {
        let hint = expected
            .map(|element| Type::List(Box::new(element.clone())))
            .filter(|_| super::expressions::needs_context(source));
        let value = self.expression(source, hint.as_ref())?;
        let (kind, element) = match &value.ty {
            Type::List(element) => (SequenceKind::List, element.as_ref().clone()),
            Type::Bytes => (SequenceKind::Bytes, Type::Int),
            Type::Stream(element) => (SequenceKind::Stream, element.as_ref().clone()),
            Type::AsyncStream(element) => (SequenceKind::AsyncStream, element.as_ref().clone()),
            Type::RowStream(element) => (SequenceKind::RowStream, element.as_ref().clone()),
            _ => return Err(expected_sequence(source.span)),
        };
        Ok(Sequence {
            value,
            kind,
            element,
        })
    }
}

fn expected_collection(span: Span) -> Diagnostic {
    Diagnostic::error("C005", "incorrect collection operand type", span)
}

fn expected_sequence(span: Span) -> Diagnostic {
    Diagnostic::error(
        "C005",
        "expected List, Bytes, Stream or AsyncStream sequence",
        span,
    )
}

fn invalid_handler(span: Span) -> Diagnostic {
    Diagnostic::error(
        "C005",
        "handler has invalid outputs for this collection operation",
        span,
    )
}
