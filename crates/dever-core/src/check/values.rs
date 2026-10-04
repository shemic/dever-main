use std::collections::BTreeSet;

use super::{
    Checked,
    body::Body,
    expressions::{is_null, needs_context},
    path_name,
};
use crate::diagnostic::Diagnostic;
use crate::hir::{Expression as Value, ExpressionKind as Expr};
use crate::source::Span;
use crate::syntax::{self, Expression};
use crate::types::{DefinitionKind, Shape, Type};

impl Body<'_, '_> {
    pub(super) fn record(
        &mut self,
        name: &syntax::Path,
        fields: &[syntax::RecordField],
        span: Span,
    ) -> Checked<Value> {
        let id = self.context.named_type(&path_name(name), span)?;
        if matches!(
            self.context.types[id].kind,
            DefinitionKind::ModelId | DefinitionKind::ModelPage | DefinitionKind::ModelCursor
        ) {
            return Err(Diagnostic::error(
                "C005",
                "generated Model values are created only by database operations",
                span,
            ));
        }
        let Shape::Record(expected) = &self.context.types[id].shape else {
            return Err(Diagnostic::error(
                "C005",
                "choice values use a type-qualified variant constructor",
                span,
            ));
        };
        let expected = expected.clone();
        if !self.context.owns_private_fields(id) && expected.iter().any(|field| field.private) {
            return Err(Diagnostic::error(
                "C006",
                "record with private fields can only be constructed by its owner (Model: owning domain App)",
                span,
            ));
        }
        let mut seen = BTreeSet::new();
        let mut values = Vec::new();
        for field in fields {
            let Some((index, expected)) = expected
                .iter()
                .enumerate()
                .find(|(_, expected)| expected.name == field.name.text)
            else {
                return Err(Diagnostic::error(
                    "C004",
                    "unknown constructor field",
                    field.name.span,
                ));
            };
            if !seen.insert(index) {
                return Err(Diagnostic::error(
                    "C002",
                    "duplicate constructor field",
                    field.span,
                ));
            }
            values.push((index, self.expression(&field.value, Some(&expected.ty))?));
        }
        if seen.len() != expected.len() {
            return Err(Diagnostic::error(
                "C005",
                "record construction must initialize every field, including nullable fields",
                span,
            ));
        }
        Ok(Value {
            kind: Expr::Record { fields: values },
            ty: Type::Named(id),
            span,
        })
    }

    pub(super) fn construct_variant(
        &mut self,
        id: usize,
        variant: usize,
        arguments: &[Expression],
        span: Span,
    ) -> Checked<Value> {
        self.construct_variant_with_kind(id, variant, arguments, span, false)
    }

    pub(super) fn construct_failure_variant(
        &mut self,
        id: usize,
        variant: usize,
        arguments: &[Expression],
        span: Span,
    ) -> Checked<Value> {
        self.construct_variant_with_kind(id, variant, arguments, span, true)
    }

    fn construct_variant_with_kind(
        &mut self,
        id: usize,
        variant: usize,
        arguments: &[Expression],
        span: Span,
        failure: bool,
    ) -> Checked<Value> {
        let Shape::Choice(variants) = &self.context.types[id].shape else {
            unreachable!()
        };
        if failure && !variants[variant].error {
            return Err(Diagnostic::error(
                "C005",
                "fail requires an error choice variant",
                span,
            ));
        }
        if !failure && variants[variant].error {
            return Err(Diagnostic::error(
                "C005",
                "error choice variants can only be used with fail",
                span,
            ));
        }
        let fields = variants[variant].fields.clone();
        if fields.len() != arguments.len() {
            return Err(Diagnostic::error(
                "C005",
                "choice constructor payload arity mismatch",
                span,
            ));
        }
        let arguments = arguments
            .iter()
            .zip(fields)
            .map(|(argument, field)| self.expression(argument, Some(&field.ty)))
            .collect::<Checked<_>>()?;
        Ok(Value {
            kind: Expr::Variant { variant, arguments },
            ty: Type::Named(id),
            span,
        })
    }

    pub(super) fn list(
        &mut self,
        elements: &[Expression],
        expected: Option<&Type>,
        span: Span,
    ) -> Checked<Value> {
        let hint = match expected.map(Type::base) {
            Some(Type::List(element)) => Some(element.as_ref()),
            _ => None,
        };
        let (values, element) = self.homogeneous(elements, hint, span)?;
        Ok(Value {
            kind: Expr::List(values),
            ty: Type::List(Box::new(element)),
            span,
        })
    }

    pub(super) fn map(
        &mut self,
        entries: &[syntax::MapEntry],
        expected: Option<&Type>,
        span: Span,
    ) -> Checked<Value> {
        let (key_hint, value_hint) = match expected.map(Type::base) {
            Some(Type::Map(key, value)) => (Some(key.as_ref()), Some(value.as_ref())),
            _ => (None, None),
        };
        let keys: Vec<_> = entries.iter().map(|entry| entry.key.clone()).collect();
        let values: Vec<_> = entries.iter().map(|entry| entry.value.clone()).collect();
        let (keys, key) = self.homogeneous(&keys, key_hint, span)?;
        let (values, value) = self.homogeneous(&values, value_hint, span)?;
        if !self.context.map_key(&key) {
            return Err(Diagnostic::error("C005", "invalid Map key type", span));
        }
        let mut seen: Vec<&Value> = Vec::new();
        for key in &keys {
            if seen.iter().any(|previous| same_key(previous, key)) {
                return Err(Diagnostic::error(
                    "C002",
                    "duplicate Map literal key",
                    key.span,
                ));
            }
            seen.push(key);
        }
        Ok(Value {
            kind: Expr::Map(keys.into_iter().zip(values).collect()),
            ty: Type::Map(Box::new(key), Box::new(value)),
            span,
        })
    }

    fn homogeneous(
        &mut self,
        sources: &[Expression],
        hint: Option<&Type>,
        span: Span,
    ) -> Checked<(Vec<Value>, Type)> {
        if let Some(hint) = hint {
            return Ok((
                sources
                    .iter()
                    .map(|source| self.expression(source, Some(hint)))
                    .collect::<Checked<_>>()?,
                hint.clone(),
            ));
        }
        let seed = sources
            .iter()
            .position(|source| !needs_context(source))
            .ok_or_else(|| {
                Diagnostic::error(
                    "C005",
                    "empty or null-only collection requires a type context",
                    span,
                )
            })?;
        let first = self.expression(&sources[seed], None)?;
        let mut ty = first.ty.clone();
        if !ty.value() {
            return Err(Diagnostic::error(
                "C005",
                "collections cannot contain actions or named multi-output results",
                span,
            ));
        }
        let mut values: Vec<Option<Value>> = vec![None; sources.len()];
        values[seed] = Some(first);
        for (index, source) in sources.iter().enumerate() {
            if index == seed || needs_context(source) {
                continue;
            }
            let value = self.expression(source, None)?;
            ty = common_type(&ty, &value.ty).ok_or_else(|| {
                Diagnostic::error(
                    "C005",
                    "collection elements must have one stable type",
                    source.span,
                )
            })?;
            values[index] = Some(value);
        }
        if sources.iter().any(is_null) {
            ty = ty.nullable();
        }
        let values = sources
            .iter()
            .zip(values)
            .map(|(source, value)| match value {
                Some(value) => self.coerce(value, &ty),
                None => self.expression(source, Some(&ty)),
            })
            .collect::<Checked<_>>()?;
        Ok((values, ty))
    }
}

fn same_key(left: &Value, right: &Value) -> bool {
    if left.ty != right.ty {
        return false;
    }
    match (&left.kind, &right.kind) {
        (Expr::Constant(left), Expr::Constant(right)) => left == right,
        (
            Expr::Variant {
                variant: left,
                arguments: left_args,
            },
            Expr::Variant {
                variant: right,
                arguments: right_args,
            },
        ) => left == right && left_args.is_empty() && right_args.is_empty(),
        (Expr::Local(left), Expr::Local(right)) => left == right,
        (
            Expr::Field {
                value: left,
                index: left_index,
            },
            Expr::Field {
                value: right,
                index: right_index,
            },
        ) => left_index == right_index && same_key(left, right),
        _ => false,
    }
}

fn common_type(left: &Type, right: &Type) -> Option<Type> {
    if left == right {
        return Some(left.clone());
    }
    if matches!(
        (left, right),
        (Type::Int, Type::Decimal) | (Type::Decimal, Type::Int)
    ) {
        return Some(Type::Decimal);
    }
    if left.base() == right.base() {
        return Some(left.base().clone().nullable());
    }
    None
}
