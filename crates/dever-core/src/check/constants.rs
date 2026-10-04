use dever_runtime::number::{self, DecimalValue};

use crate::diagnostic::Diagnostic;
use crate::hir::{Constant, Expression, ExpressionKind};
use crate::source::Span;
use crate::syntax::{BinaryOperator as Op, Literal, UnaryOperator};
use crate::types::Type;

use super::Checked;

pub(super) fn literal(value: &Literal, expected: Option<&Type>, span: Span) -> Checked<Expression> {
    let (constant, ty) = match value {
        Literal::Bool(value) => (Constant::Bool(*value), Type::Bool),
        Literal::Text(value) if expected.map(Type::base) == Some(&Type::Uuid) => {
            dever_runtime::orm::Uuid::parse(value)
                .map_err(|error| Diagnostic::error("C005", error.to_string(), span))?;
            (Constant::Text(value.clone()), Type::Uuid)
        }
        Literal::Text(value) => (Constant::Text(value.clone()), Type::Text),
        Literal::Null => {
            let Some(ty @ Type::Nullable(_)) = expected else {
                return Err(Diagnostic::error(
                    "C005",
                    "null needs a nullable type context",
                    span,
                ));
            };
            (Constant::Null, ty.clone())
        }
        Literal::Number(text) => {
            if text.contains(['e', 'E'])
                || (text.contains('.') && expected.map(Type::base) == Some(&Type::Float))
            {
                let value: f64 = text
                    .parse()
                    .map_err(|_| Diagnostic::error("C005", "invalid Float literal", span))?;
                if !value.is_finite() {
                    return Err(Diagnostic::error(
                        "C005",
                        "Float literal must be finite",
                        span,
                    ));
                }
                (Constant::Float(value), Type::Float)
            } else if text.contains('.') {
                (
                    Constant::Decimal(
                        DecimalValue::parse(text)
                            .map_err(|message| Diagnostic::error("C005", message, span))?,
                    ),
                    Type::Decimal,
                )
            } else {
                (
                    Constant::Int(text.parse().map_err(|_| {
                        Diagnostic::error("C005", "Int literal is outside the 64-bit range", span)
                    })?),
                    Type::Int,
                )
            }
        }
    };
    Ok(Expression {
        kind: ExpressionKind::Constant(constant),
        ty,
        span,
    })
}

pub(super) fn promote(value: Expression) -> Expression {
    let span = value.span;
    let kind = match value.kind {
        ExpressionKind::Constant(Constant::Int(integer)) => {
            ExpressionKind::Constant(Constant::Decimal(DecimalValue::from_int(integer)))
        }
        _ => ExpressionKind::Promote(Box::new(value)),
    };
    Expression {
        kind,
        ty: Type::Decimal,
        span,
    }
}

pub(super) fn unary(operator: UnaryOperator, value: &Constant, span: Span) -> Checked<Constant> {
    let result = match (operator, value) {
        (UnaryOperator::Not, Constant::Bool(value)) => Ok(Constant::Bool(!value)),
        (UnaryOperator::Negate, Constant::Int(value)) => number::int_neg(*value).map(Constant::Int),
        (UnaryOperator::Negate, Constant::Decimal(value)) => {
            value.checked_neg().map(Constant::Decimal)
        }
        (UnaryOperator::Negate, Constant::Float(value)) => Ok(Constant::Float(-value)),
        _ => unreachable!("checked unary operand"),
    };
    result.map_err(|message| Diagnostic::error("C008", message, span))
}

pub(super) fn binary(
    left: &Constant,
    operator: Op,
    right: &Constant,
    span: Span,
) -> Checked<Constant> {
    use Constant as C;
    let result = match (left, operator, right) {
        (left, Op::Equal, right) => Ok(C::Bool(left == right)),
        (left, Op::NotEqual, right) => Ok(C::Bool(left != right)),
        (C::Bool(left), Op::And, C::Bool(right)) => Ok(C::Bool(*left && *right)),
        (C::Bool(left), Op::Or, C::Bool(right)) => Ok(C::Bool(*left || *right)),
        (C::Text(left), Op::Add, C::Text(right)) => Ok(C::Text(format!("{left}{right}"))),
        (C::Int(left), op, C::Int(right)) => match op {
            Op::Add => number::int_add(*left, *right).map(C::Int),
            Op::Subtract => number::int_sub(*left, *right).map(C::Int),
            Op::Multiply => number::int_mul(*left, *right).map(C::Int),
            Op::IntegerDivide => number::int_div(*left, *right).map(C::Int),
            Op::Remainder => number::int_rem(*left, *right).map(C::Int),
            _ => Ok(C::Bool(compare(left.partial_cmp(right), op))),
        },
        (C::Decimal(left), op, C::Decimal(right)) => match op {
            Op::Add => left.checked_add(*right).map(C::Decimal),
            Op::Subtract => left.checked_sub(*right).map(C::Decimal),
            Op::Multiply => left.checked_mul(*right).map(C::Decimal),
            Op::Divide => left.checked_div(*right).map(C::Decimal),
            _ => Ok(C::Bool(compare(left.partial_cmp(right), op))),
        },
        (C::Float(left), op, C::Float(right)) => Ok(match op {
            Op::Add => C::Float(left + right),
            Op::Subtract => C::Float(left - right),
            Op::Multiply => C::Float(left * right),
            Op::Divide => C::Float(left / right),
            _ => C::Bool(compare(left.partial_cmp(right), op)),
        }),
        (C::Text(left), op, C::Text(right)) => Ok(C::Bool(compare(left.partial_cmp(right), op))),
        _ => unreachable!("checked binary operands"),
    };
    result.map_err(|message| Diagnostic::error("C008", message, span))
}

fn compare(order: Option<std::cmp::Ordering>, operator: Op) -> bool {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match operator {
        Op::Less => order == Some(Less),
        Op::LessEqual => matches!(order, Some(Less | Equal)),
        Op::Greater => order == Some(Greater),
        Op::GreaterEqual => matches!(order, Some(Greater | Equal)),
        _ => unreachable!("comparison operator"),
    }
}
