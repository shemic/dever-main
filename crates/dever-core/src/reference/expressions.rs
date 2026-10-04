use super::{Evaluator, Frame, ResultValue, RuntimeFault, Value, value::Key};
use crate::{
    hir::{CallArgument, CallTarget, Expression, ExpressionKind as Expr},
    specialize::{self, Specialization},
    syntax::{BinaryOperator as Op, UnaryOperator},
    types::{Shape, Type},
};
use dever_runtime::{
    collections::{List, Map},
    number::{self, DecimalValue},
};
use std::cmp::Ordering;

impl Evaluator<'_> {
    pub(super) fn expression(&mut self, expression: &Expression, frame: &Frame) -> ResultValue {
        Ok(match &expression.kind {
            Expr::Constant(crate::hir::Constant::Text(value)) if expression.ty == Type::Uuid => {
                Value::Uuid(dever_runtime::orm::Uuid::parse(value).expect("checked Uuid literal"))
            }
            Expr::Constant(value) => Value::constant(value),
            Expr::Local(slot) => frame.locals[*slot]
                .as_ref()
                .expect("initialized local")
                .clone(),
            Expr::Field { value, index } => self.expression(value, frame)?.fields()[*index].clone(),
            Expr::ChoiceOptions { choice } => {
                let Shape::Choice(variants) = &self.program.types[*choice].shape else {
                    unreachable!("checked Model choice options")
                };
                Value::Map(Map::from_unique(
                    variants
                        .iter()
                        .enumerate()
                        .map(|(variant, definition)| {
                            (
                                super::value::Key::Variant(*choice, variant),
                                Value::Text(
                                    definition
                                        .label
                                        .as_deref()
                                        .unwrap_or(&definition.name)
                                        .to_owned(),
                                ),
                            )
                        })
                        .collect(),
                ))
            }
            Expr::Some(value) => self.expression(value, frame)?,
            Expr::Promote(value) => {
                Value::Decimal(DecimalValue::from_int(self.expression(value, frame)?.int()))
            }
            Expr::List(values) => Value::List(List::new(self.arguments(values, frame)?)),
            Expr::Map(entries) => {
                let mut pairs = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    let key = self.expression(key, frame)?;
                    let value = self.expression(value, frame)?;
                    pairs.push((Key::from_value(&key), value));
                }
                Value::Map(
                    Map::new(pairs)
                        .map_err(|message| RuntimeFault::new(message, expression.span))?,
                )
            }
            Expr::Record { fields } => {
                let Type::Named(id) = expression.ty else {
                    unreachable!()
                };
                let Shape::Record(definition) = &self.program.types[id].shape else {
                    unreachable!()
                };
                let mut values = vec![Value::Unit; definition.len()];
                // Storage order follows the declaration, execution follows the source initializer.
                for (index, value) in fields {
                    values[*index] = self.expression(value, frame)?;
                }
                Value::Record(id, values)
            }
            Expr::Variant { variant, arguments } => {
                let Type::Named(id) = expression.ty else {
                    unreachable!()
                };
                Value::Variant(id, *variant, self.arguments(arguments, frame)?)
            }
            Expr::Fail(error) => {
                let error = self.expression(error, frame)?;
                return Err(RuntimeFault::business(
                    error.clone(),
                    error.render(self.program),
                    expression.span,
                ));
            }
            Expr::Call { target, arguments } => {
                let instance = match target {
                    CallTarget::Function(function) => {
                        specialize::call_specialization(*function, arguments, &frame.handlers)
                    }
                    CallTarget::Handler(target) => Specialization {
                        function: specialize::resolve_handler(*target, &frame.handlers),
                        handlers: vec![],
                    },
                };
                let mut values = Vec::new();
                for argument in arguments {
                    if let CallArgument::Value(value) = argument {
                        values.push(self.expression(value, frame)?);
                    }
                }
                self.call(instance, values)
                    .map_err(|fault| fault.at(expression.span))?
            }
            Expr::CaptureResult {
                target,
                arguments,
                choice,
                success,
            } => {
                let instance = match target {
                    CallTarget::Function(function) => {
                        specialize::call_specialization(*function, arguments, &frame.handlers)
                    }
                    CallTarget::Handler(target) => Specialization {
                        function: specialize::resolve_handler(*target, &frame.handlers),
                        handlers: vec![],
                    },
                };
                let mut values = Vec::new();
                for argument in arguments {
                    if let CallArgument::Value(value) = argument {
                        values.push(self.expression(value, frame)?);
                    }
                }
                let failures = specialize::failures(self.program, &instance);
                let targets = crate::capture::targets(&self.program.types, *choice, &failures)
                    .expect("checked capture");
                match self.call(instance, values) {
                    Ok(Value::Unit) => Value::Variant(*choice, *success, Vec::new()),
                    Ok(Value::Fields(values)) => Value::Variant(*choice, *success, values),
                    Ok(value) => Value::Variant(*choice, *success, vec![value]),
                    Err(fault) => fault
                        .into_business(&targets, *choice)
                        .map_err(|fault| fault.at(expression.span))?,
                }
            }
            Expr::RunCall { .. }
            | Expr::ModelOperation { .. }
            | Expr::JobEnqueue { .. }
            | Expr::AwaitTask(_)
            | Expr::StopTask(_)
            | Expr::Group(_)
            | Expr::AwaitGroup(_)
            | Expr::StopGroup(_)
            | Expr::ParallelCall { .. }
            | Expr::BlockingCall { .. }
            | Expr::Channel { .. }
            | Expr::ChannelSend { .. }
            | Expr::ChannelReceive(_)
            | Expr::ChannelClose(_) => {
                return Err(RuntimeFault::new(
                    "the reference evaluator does not execute async operations",
                    expression.span,
                ));
            }
            Expr::Intrinsic {
                operation,
                arguments,
                ..
            } => {
                let arguments = self.arguments(arguments, frame)?;
                self.intrinsic(*operation, arguments, expression)?
            }
            Expr::Collection {
                operation,
                handler,
                sequence: _,
                arguments,
            } => {
                let arguments = self.arguments(arguments, frame)?;
                let handler =
                    handler.map(|target| specialize::resolve_handler(target, &frame.handlers));
                self.collection(*operation, handler, arguments, expression)?
            }
            Expr::Unary { operator, value } => {
                let value = self.expression(value, frame)?;
                match (operator, value) {
                    (UnaryOperator::Not, Value::Bool(value)) => Value::Bool(!value),
                    (UnaryOperator::Negate, Value::Float(value)) => Value::Float(-value),
                    (UnaryOperator::Negate, Value::Int(value)) => Value::Int(
                        number::int_neg(value)
                            .map_err(|message| RuntimeFault::new(message, expression.span))?,
                    ),
                    (UnaryOperator::Negate, Value::Decimal(value)) => Value::Decimal(
                        value
                            .checked_neg()
                            .map_err(|message| RuntimeFault::new(message, expression.span))?,
                    ),
                    _ => unreachable!("checked unary operator"),
                }
            }
            Expr::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.expression(left, frame)?;
                if *operator == Op::And && !left.boolean() {
                    return Ok(Value::Bool(false));
                }
                if *operator == Op::Or && left.boolean() {
                    return Ok(Value::Bool(true));
                }
                let right = self.expression(right, frame)?;
                binary(left, *operator, right)
                    .map_err(|message| RuntimeFault::new(message, expression.span))?
            }
        })
    }

    fn arguments(
        &mut self,
        expressions: &[Expression],
        frame: &Frame,
    ) -> Result<Vec<Value>, RuntimeFault> {
        expressions
            .iter()
            .map(|expression| self.expression(expression, frame))
            .collect()
    }
}

pub(super) fn binary(left: Value, operator: Op, right: Value) -> Result<Value, &'static str> {
    match operator {
        Op::Equal => return Ok(Value::Bool(left == right)),
        Op::NotEqual => return Ok(Value::Bool(left != right)),
        Op::And | Op::Or => return Ok(right),
        Op::Less | Op::LessEqual | Op::Greater | Op::GreaterEqual => {
            let ordering = match (&left, &right) {
                (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
                (Value::Decimal(a), Value::Decimal(b)) => a.partial_cmp(b),
                (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
                (Value::Text(a), Value::Text(b)) => a.partial_cmp(b),
                _ => unreachable!("checked ordered operands"),
            };
            return Ok(Value::Bool(match operator {
                Op::Less => ordering == Some(Ordering::Less),
                Op::LessEqual => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
                Op::Greater => ordering == Some(Ordering::Greater),
                Op::GreaterEqual => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
                _ => unreachable!(),
            }));
        }
        _ => {}
    }
    Ok(match (left, right) {
        (Value::Int(a), Value::Int(b)) => Value::Int(match operator {
            Op::Add => number::int_add(a, b)?,
            Op::Subtract => number::int_sub(a, b)?,
            Op::Multiply => number::int_mul(a, b)?,
            Op::IntegerDivide => number::int_div(a, b)?,
            Op::Remainder => number::int_rem(a, b)?,
            _ => unreachable!("checked Int operator"),
        }),
        (Value::Decimal(a), Value::Decimal(b)) => Value::Decimal(match operator {
            Op::Add => a.checked_add(b)?,
            Op::Subtract => a.checked_sub(b)?,
            Op::Multiply => a.checked_mul(b)?,
            Op::Divide => a.checked_div(b)?,
            _ => unreachable!("checked Decimal operator"),
        }),
        (Value::Float(a), Value::Float(b)) => Value::Float(match operator {
            Op::Add => a + b,
            Op::Subtract => a - b,
            Op::Multiply => a * b,
            Op::Divide => a / b,
            _ => unreachable!("checked Float operator"),
        }),
        (Value::Text(a), Value::Text(b)) => Value::Text(a + &b),
        _ => unreachable!("checked arithmetic operands"),
    })
}
