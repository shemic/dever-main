use super::{
    Checked,
    body::Body,
    constants,
    expressions::{is_null, ungroup},
};
use crate::diagnostic::Diagnostic;
use crate::hir::{Constant, Expression as Value, ExpressionKind as Expr};
use crate::source::Span;
use crate::syntax::{
    BinaryOperator as Op, Expression, ExpressionKind as Ast, Literal, UnaryOperator,
};
use crate::types::Type;

impl Body<'_, '_> {
    pub(super) fn unary(
        &mut self,
        operator: UnaryOperator,
        source: &Expression,
        expected: Option<&Type>,
        span: Span,
    ) -> Checked<Value> {
        if operator == UnaryOperator::Negate
            && let Ast::Literal(Literal::Number(text)) = &ungroup(source).kind
        {
            return constants::literal(&Literal::Number(format!("-{text}")), expected, span);
        }
        let hint = if operator == UnaryOperator::Not {
            Some(&Type::Bool)
        } else {
            expected.map(Type::base).filter(|ty| ty.numeric())
        };
        let value = self.expression(source, hint)?;
        if (operator == UnaryOperator::Not && value.ty != Type::Bool)
            || (operator == UnaryOperator::Negate && !value.ty.numeric())
        {
            return Err(Diagnostic::error(
                "C005",
                "invalid unary operand type",
                span,
            ));
        }
        let ty = value.ty.clone();
        let kind = if let Expr::Constant(constant) = &value.kind {
            Expr::Constant(constants::unary(operator, constant, span)?)
        } else {
            Expr::Unary {
                operator,
                value: Box::new(value),
            }
        };
        Ok(Value { kind, ty, span })
    }

    pub(super) fn binary(
        &mut self,
        left: &Expression,
        operator: Op,
        right: &Expression,
        expected: Option<&Type>,
        span: Span,
    ) -> Checked<Value> {
        let boolean = matches!(operator, Op::And | Op::Or);
        let equality = matches!(operator, Op::Equal | Op::NotEqual);
        let comparison = matches!(
            operator,
            Op::Less | Op::LessEqual | Op::Greater | Op::GreaterEqual
        );
        let hint = if boolean {
            Some(&Type::Bool)
        } else if !equality && !comparison && expected.map(Type::base) == Some(&Type::Float) {
            Some(&Type::Float)
        } else {
            None
        };
        let left_null = is_null(left);
        let (mut left, mut right) = if equality && left_null {
            let right = self.expression(right, None)?;
            (self.expression(left, Some(&right.ty))?, right)
        } else if hint.is_none() && contextual_decimal_literal(left) {
            let right = self.expression(right, None)?;
            let hint = (right.ty == Type::Float).then_some(&Type::Float);
            (self.expression(left, hint)?, right)
        } else {
            let left = self.expression(left, hint)?;
            let right_hint = if equality && is_null(right) {
                Some(&left.ty)
            } else if left.ty == Type::Float && contextual_decimal_literal(right) {
                Some(&Type::Float)
            } else {
                hint
            };
            let right = self.expression(right, right_hint)?;
            (left, right)
        };
        let ty = if equality {
            if left.ty != right.ty || !left.ty.comparable(self.context.types) {
                return Err(Diagnostic::error(
                    "C005",
                    "equality requires the same comparable type; it does not promote numbers",
                    span,
                ));
            }
            Type::Bool
        } else if boolean {
            Type::Bool
        } else if operator == Op::Add && left.ty == Type::Text && right.ty == Type::Text {
            Type::Text
        } else {
            if !left.ty.numeric() || !right.ty.numeric() {
                return Err(Diagnostic::error(
                    "C005",
                    "arithmetic and ordering require numeric operands",
                    span,
                ));
            }
            if matches!(operator, Op::IntegerDivide | Op::Remainder) {
                if left.ty != Type::Int || right.ty != Type::Int {
                    return Err(Diagnostic::error(
                        "C005",
                        "// and % require Int operands",
                        span,
                    ));
                }
            } else {
                let decimal = left.ty == Type::Decimal
                    || right.ty == Type::Decimal
                    || (operator == Op::Divide && left.ty == Type::Int && right.ty == Type::Int);
                if decimal {
                    if left.ty == Type::Int {
                        left = constants::promote(left);
                    }
                    if right.ty == Type::Int {
                        right = constants::promote(right);
                    }
                }
                if left.ty != right.ty {
                    return Err(Diagnostic::error(
                        "C005",
                        "Float cannot implicitly mix with Int or Decimal",
                        span,
                    ));
                }
            }
            if comparison {
                Type::Bool
            } else {
                left.ty.clone()
            }
        };
        if matches!(operator, Op::Divide | Op::IntegerDivide | Op::Remainder)
            && matches!(&right.kind, Expr::Constant(Constant::Int(0)))
        {
            return Err(Diagnostic::error("C008", "Int division by zero", span));
        }
        if operator == Op::Divide
            && matches!(&right.kind, Expr::Constant(Constant::Decimal(value)) if *value == dever_runtime::number::DecimalValue::ZERO)
        {
            return Err(Diagnostic::error("C008", "Decimal division by zero", span));
        }
        let kind = match (&left.kind, &right.kind) {
            (Expr::Constant(left), Expr::Constant(right)) => {
                Expr::Constant(constants::binary(left, operator, right, span)?)
            }
            _ => Expr::Binary {
                left: Box::new(left),
                operator,
                right: Box::new(right),
            },
        };
        Ok(Value { kind, ty, span })
    }
}

fn contextual_decimal_literal(expression: &Expression) -> bool {
    match &ungroup(expression).kind {
        Ast::Literal(Literal::Number(text)) => text.contains('.') && !text.contains(['e', 'E']),
        Ast::Unary {
            operator: UnaryOperator::Negate,
            value,
        } => contextual_decimal_literal(value),
        _ => false,
    }
}
