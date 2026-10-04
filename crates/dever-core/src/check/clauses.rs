use std::collections::BTreeSet;

use dever_runtime::number::DecimalValue;

use crate::diagnostic::{Diagnostic, Label};
use crate::hir::{Atom, Constant, Domain, ExpressionKind};
use crate::syntax::{BinaryOperator as Op, FunctionClause, InputKind, Literal, Pattern};
use crate::types::{Field, HandlerSignature, Parameter, Shape, Type};

use super::{Checked, Context, constants, path_name, symbols};

#[derive(Clone)]
enum PatternSet {
    Any(bool),
    Null,
    Bool(bool),
    Variant(usize),
    Literal(Constant),
    Int(Option<(i64, i64)>),
    Decimal(Option<(DecimalValue, DecimalValue)>),
    Other,
}

pub(super) fn signature(
    group: &[&FunctionClause],
    context: &Context<'_>,
) -> Checked<(Vec<Parameter>, Vec<Field>)> {
    if group
        .iter()
        .any(|clause| clause.pure != group[0].pure || clause.recovery != group[0].recovery)
    {
        return Err(Diagnostic::error(
            "C013",
            "all clauses must have identical pure and recovery contracts",
            group[0].span,
        ));
    }
    let mut parameters = Vec::new();
    for axis in 0..group[0].inputs.len() {
        if let InputKind::Handler(source) = &group[0].inputs[axis].kind {
            let signature = handler_signature(source, context)?;
            for clause in &group[1..] {
                let InputKind::Handler(candidate) = &clause.inputs[axis].kind else {
                    return Err(Diagnostic::error(
                        "C005",
                        "all clauses must agree whether an input is a handler",
                        clause.inputs[axis].span,
                    ));
                };
                if handler_signature(candidate, context)? != signature {
                    return Err(Diagnostic::error(
                        "C005",
                        "all clauses must use the same handler signature",
                        clause.inputs[axis].span,
                    ));
                }
            }
            parameters.push(Parameter::Handler(signature));
            continue;
        }
        let mut inferred = None;
        let mut nullable = false;
        for clause in group {
            let input = &clause.inputs[axis];
            let InputKind::Value(pattern) = &input.kind else {
                return Err(Diagnostic::error(
                    "C005",
                    "all clauses must agree whether an input is a handler",
                    input.span,
                ));
            };
            let ty = match pattern {
                Pattern::Typed { ty, .. } => {
                    match context.variant(&path_name(&ty.name), ty.span)? {
                        Some((id, _)) => Some(Type::Named(id)),
                        None => Some(context.resolve_type(ty)?),
                    }
                }
                Pattern::Variant {
                    name,
                    arguments,
                    span,
                    ..
                } => {
                    let name = path_name(name);
                    if let Some((ty, _, _)) = context.related_variant(&name, arguments, *span)? {
                        Some(ty)
                    } else {
                        if !arguments.is_empty() {
                            return Err(Diagnostic::error(
                                "C005",
                                "choice variants do not accept type arguments",
                                *span,
                            ));
                        }
                        Some(Type::Named(
                            context
                                .variant(&name, *span)?
                                .ok_or_else(|| {
                                    Diagnostic::error(
                                        "C005",
                                        "expected a choice variant pattern",
                                        *span,
                                    )
                                })?
                                .0,
                        ))
                    }
                }
                _ => None,
            };
            if let Some(ty) = ty {
                merge(&mut inferred, &mut nullable, ty, input.span)?;
            }
        }
        let explicitly_typed = inferred.is_some();
        let mut literal_types = Vec::new();
        for clause in group {
            let input = &clause.inputs[axis];
            let InputKind::Value(pattern) = &input.kind else {
                unreachable!("input kind checked above")
            };
            if let Pattern::Literal { value, span } = pattern {
                if matches!(value, Literal::Null) {
                    nullable = true;
                    continue;
                }
                let value = constants::literal(value, inferred.as_ref(), *span)?;
                literal_types.push((value.ty, input.span));
            }
        }
        if !explicitly_typed && literal_types.iter().any(|(ty, _)| *ty == Type::Decimal) {
            inferred = Some(Type::Decimal);
        }
        for (ty, span) in literal_types {
            let ty = if ty == Type::Int && inferred == Some(Type::Decimal) {
                Type::Decimal
            } else {
                ty
            };
            merge(&mut inferred, &mut nullable, ty, span)?;
        }
        let ty = inferred.ok_or_else(|| {
            Diagnostic::error(
                "C005",
                "cannot infer this parameter's base type from null/other alone",
                group[0].inputs[axis].span,
            )
        })?;
        let ty = if nullable { ty.nullable() } else { ty };
        symbols::validate_type_constraints(&ty, context.types, group[0].inputs[axis].span)?;
        parameters.push(Parameter::Value(ty));
    }
    let outputs = context.fields(&group[0].outputs)?;
    for field in &outputs {
        symbols::validate_type_constraints(&field.ty, context.types, group[0].span)?;
    }
    for clause in &group[1..] {
        if context.fields(&clause.outputs)? != outputs {
            return Err(Diagnostic::error(
                "C005",
                "all clauses must have identical ordered output names and types",
                clause.span,
            ));
        }
    }
    Ok((parameters, outputs))
}

fn handler_signature(
    source: &crate::syntax::HandlerSignature,
    context: &Context<'_>,
) -> Checked<HandlerSignature> {
    let inputs = context.fields(&source.inputs)?;
    for (field, source) in inputs.iter().zip(&source.inputs) {
        symbols::validate_type_constraints(&field.ty, context.types, source.span)?;
    }
    let outputs = context.fields(&source.outputs)?;
    for (field, source) in outputs.iter().zip(&source.outputs) {
        symbols::validate_type_constraints(&field.ty, context.types, source.span)?;
    }
    Ok(HandlerSignature {
        bounds: inputs.iter().map(|field| field.bounds.clone()).collect(),
        parameters: inputs.into_iter().map(|field| field.ty).collect(),
        outputs,
    })
}

pub(super) fn field_bounds(
    field: &crate::syntax::Field,
    ty: &Type,
    context: &Context<'_>,
) -> Checked<Domain> {
    if field.bounds.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = Pattern::Typed {
        ty: field.ty.clone(),
        bounds: field.bounds.clone(),
    };
    let atom = match normalize(&pattern, ty, context, field.span)? {
        PatternSet::Int(Some((lo, hi))) => Atom::Int(lo, hi),
        PatternSet::Decimal(Some((lo, hi))) => Atom::Decimal(lo, hi),
        _ => {
            return Err(Diagnostic::error(
                "C011",
                "field constraint has an empty domain",
                field.span,
            ));
        }
    };
    Ok(vec![atom])
}

fn merge(
    inferred: &mut Option<Type>,
    nullable: &mut bool,
    ty: Type,
    span: crate::source::Span,
) -> Checked<()> {
    *nullable |= matches!(ty, Type::Nullable(_));
    let base = ty.base().clone();
    match inferred {
        Some(previous) if *previous != base => Err(Diagnostic::error(
            "C005",
            "clauses disagree on this parameter's base type",
            span,
        )),
        _ => {
            *inferred = Some(base);
            Ok(())
        }
    }
}

pub(super) fn check(
    group: &[&FunctionClause],
    parameters: &[Parameter],
    context: &Context<'_>,
) -> Checked<Vec<Vec<Domain>>> {
    let value_axes = parameters
        .iter()
        .enumerate()
        .filter_map(|(axis, parameter)| parameter.value_type().map(|ty| (axis, ty)))
        .collect::<Vec<_>>();
    let patterns: Vec<Vec<_>> = group
        .iter()
        .map(|clause| {
            value_axes
                .iter()
                .map(|(axis, ty)| {
                    let input = &clause.inputs[*axis];
                    let InputKind::Value(pattern) = &input.kind else {
                        return Err(Diagnostic::error(
                            "C005",
                            "all clauses must agree whether an input is a handler",
                            input.span,
                        ));
                    };
                    normalize(pattern, ty, context, input.span)
                })
                .collect::<Checked<_>>()
        })
        .collect::<Checked<_>>()?;
    let atoms: Vec<_> = value_axes
        .iter()
        .enumerate()
        .map(|(axis, (_, ty))| partition(ty, patterns.iter().map(|row| &row[axis]), context))
        .collect();
    let mut products: Vec<Vec<BTreeSet<usize>>> = Vec::new();
    for (row, clause) in patterns.iter().zip(group) {
        let mut product = Vec::new();
        for (axis, pattern) in row.iter().enumerate() {
            let selected: BTreeSet<_> = atoms[axis]
                .iter()
                .enumerate()
                .filter_map(|(id, atom)| {
                    let included = if matches!(pattern, PatternSet::Other) {
                        !patterns.iter().any(|row| {
                            !matches!(row[axis], PatternSet::Other) && contains(&row[axis], atom)
                        })
                    } else {
                        contains(pattern, atom)
                    };
                    included.then_some(id)
                })
                .collect();
            if selected.is_empty() {
                return Err(Diagnostic::error(
                    "C009",
                    "unreachable clause: this pattern selects no values",
                    clause.inputs[value_axes[axis].0].span,
                ));
            }
            product.push(selected);
        }
        for (previous, other) in products.iter().enumerate() {
            if product
                .iter()
                .zip(other)
                .all(|(left, right)| !left.is_disjoint(right))
            {
                let mut error = Diagnostic::error("C009", "function clauses overlap", clause.span);
                error.related.push(Label {
                    span: group[previous].span,
                    message: "overlapping clause".into(),
                });
                return Err(error);
            }
        }
        products.push(product);
    }
    // Subtract disjoint rectangles without enumerating the input Cartesian product.
    let mut remaining = vec![
        atoms
            .iter()
            .map(|axis| (0..axis.len()).collect())
            .collect::<Vec<BTreeSet<_>>>(),
    ];
    for covered in &products {
        remaining = remaining
            .into_iter()
            .flat_map(|region| subtract(region, covered))
            .collect();
    }
    if !remaining.is_empty() {
        return Err(Diagnostic::error(
            "C009",
            "function clauses do not cover every input combination",
            group[0].span,
        ));
    }
    Ok(products
        .into_iter()
        .map(|row| {
            row.into_iter()
                .enumerate()
                .map(|(axis, selected)| {
                    selected
                        .into_iter()
                        .map(|id| atoms[axis][id].clone())
                        .collect()
                })
                .collect()
        })
        .collect())
}

fn subtract(
    region: Vec<BTreeSet<usize>>,
    covered: &[BTreeSet<usize>],
) -> Vec<Vec<BTreeSet<usize>>> {
    let intersections: Vec<_> = region
        .iter()
        .zip(covered)
        .map(|(left, right)| left.intersection(right).copied().collect::<BTreeSet<_>>())
        .collect();
    if intersections.iter().any(BTreeSet::is_empty) {
        return vec![region];
    }
    let mut remainder = Vec::new();
    for axis in 0..region.len() {
        let difference: BTreeSet<_> = region[axis].difference(&covered[axis]).copied().collect();
        if difference.is_empty() {
            continue;
        }
        let mut piece = intersections[..axis].to_vec();
        piece.push(difference);
        piece.extend_from_slice(&region[axis + 1..]);
        remainder.push(piece);
    }
    remainder
}

fn normalize(
    pattern: &Pattern,
    parameter: &Type,
    context: &Context<'_>,
    span: crate::source::Span,
) -> Checked<PatternSet> {
    match pattern {
        Pattern::Other(_) => Ok(PatternSet::Other),
        Pattern::Variant {
            name,
            arguments,
            bindings,
            ..
        } => {
            let name = path_name(name);
            let (ty, variant, fields) =
                if let Some(found) = context.related_variant(&name, arguments, span)? {
                    found
                } else {
                    if !arguments.is_empty() {
                        return Err(Diagnostic::error(
                            "C005",
                            "choice variants do not accept type arguments",
                            span,
                        ));
                    }
                    let (id, variant) = context.variant(&name, span)?.ok_or_else(|| {
                        Diagnostic::error("C005", "expected choice variant", span)
                    })?;
                    let Shape::Choice(variants) = &context.types[id].shape else {
                        unreachable!()
                    };
                    (Type::Named(id), variant, variants[variant].fields.clone())
                };
            if ty != *parameter.base() {
                return Err(Diagnostic::error(
                    "C005",
                    "variant pattern has the wrong parameter type",
                    span,
                ));
            }
            if fields.len() != bindings.len() {
                return Err(Diagnostic::error(
                    "C005",
                    "choice pattern payload arity mismatch",
                    span,
                ));
            }
            Ok(PatternSet::Variant(variant))
        }
        Pattern::Literal {
            value: Literal::Null,
            ..
        } => Ok(PatternSet::Null),
        Pattern::Literal { value, .. } => {
            let mut value = constants::literal(value, Some(parameter), span)?;
            if value.ty == Type::Int && parameter.base() == &Type::Decimal {
                value = constants::promote(value);
            }
            let ExpressionKind::Constant(value) = value.kind else {
                unreachable!()
            };
            Ok(match value {
                Constant::Bool(value) => PatternSet::Bool(value),
                Constant::Int(value) => PatternSet::Int(Some((value, value))),
                Constant::Decimal(value) => PatternSet::Decimal(Some((value, value))),
                _ => PatternSet::Literal(value),
            })
        }
        Pattern::Typed { ty, bounds } => {
            if let Some((id, variant)) = context.variant(&path_name(&ty.name), span)? {
                let Shape::Choice(variants) = &context.types[id].shape else {
                    unreachable!()
                };
                if ty.nullable || !bounds.is_empty() || !variants[variant].fields.is_empty() {
                    return Err(Diagnostic::error(
                        "C005",
                        "payload variants require an explicit binding list",
                        span,
                    ));
                }
                return Ok(PatternSet::Variant(variant));
            }
            if bounds.is_empty() {
                return Ok(PatternSet::Any(ty.nullable));
            }
            if ty.nullable || !matches!(parameter.base(), Type::Int | Type::Decimal) {
                return Err(Diagnostic::error(
                    "C005",
                    "range bounds require non-nullable Int or Decimal patterns",
                    span,
                ));
            }
            let mut lower = false;
            let mut upper = false;
            let mut integers = Some((i64::MIN, i64::MAX));
            let mut decimals = Some((DecimalValue::minimum(), DecimalValue::maximum()));
            for bound in bounds {
                let seen = match bound.comparison {
                    Op::Greater | Op::GreaterEqual => &mut lower,
                    Op::Less | Op::LessEqual => &mut upper,
                    _ => unreachable!(),
                };
                if *seen {
                    return Err(Diagnostic::error(
                        "C009",
                        "a range may have at most one lower and one upper bound",
                        bound.span,
                    ));
                }
                *seen = true;
                let mut value = constants::literal(
                    &Literal::Number(bound.number.clone()),
                    Some(parameter),
                    bound.span,
                )?;
                if value.ty == Type::Int && parameter.base() == &Type::Decimal {
                    value = constants::promote(value);
                }
                match value.kind {
                    ExpressionKind::Constant(Constant::Int(value))
                        if parameter.base() == &Type::Int =>
                    {
                        integers = integers.and_then(|(lo, hi)| {
                            Some(match bound.comparison {
                                Op::Greater => (value.checked_add(1)?, hi),
                                Op::GreaterEqual => (value, hi),
                                Op::Less => (lo, value.checked_sub(1)?),
                                Op::LessEqual => (lo, value),
                                _ => unreachable!(),
                            })
                        });
                    }
                    ExpressionKind::Constant(Constant::Decimal(value))
                        if parameter.base() == &Type::Decimal =>
                    {
                        decimals = decimals.and_then(|(lo, hi)| {
                            Some(match bound.comparison {
                                Op::Greater => (value.next_up()?, hi),
                                Op::GreaterEqual => (value, hi),
                                Op::Less => (lo, value.next_down()?),
                                Op::LessEqual => (lo, value),
                                _ => unreachable!(),
                            })
                        });
                    }
                    _ => {
                        return Err(Diagnostic::error(
                            "C005",
                            "range bound has the wrong numeric type",
                            bound.span,
                        ));
                    }
                }
            }
            Ok(if parameter.base() == &Type::Int {
                PatternSet::Int(integers.filter(|(lo, hi)| lo <= hi))
            } else {
                PatternSet::Decimal(decimals.filter(|(lo, hi)| lo <= hi))
            })
        }
    }
}

fn partition<'a>(
    ty: &Type,
    patterns: impl Iterator<Item = &'a PatternSet>,
    context: &Context<'_>,
) -> Domain {
    let patterns: Vec<_> = patterns.collect();
    let mut atoms = match ty.base() {
        Type::Bool => vec![Atom::Bool(false), Atom::Bool(true)],
        Type::Named(id) => match &context.types[*id].shape {
            Shape::Choice(variants) => (0..variants.len()).map(Atom::Variant).collect(),
            _ => vec![Atom::Any],
        },
        Type::Related(_) => vec![Atom::Variant(0), Atom::Variant(1)],
        Type::Int => {
            let mut cuts = BTreeSet::from([i64::MIN]);
            for pattern in &patterns {
                if let PatternSet::Int(Some((lo, hi))) = pattern {
                    cuts.insert(*lo);
                    if let Some(next) = hi.checked_add(1) {
                        cuts.insert(next);
                    }
                }
            }
            let cuts: Vec<_> = cuts.into_iter().collect();
            cuts.iter()
                .enumerate()
                .map(|(index, lo)| {
                    Atom::Int(*lo, cuts.get(index + 1).map_or(i64::MAX, |next| next - 1))
                })
                .collect()
        }
        Type::Decimal => {
            let mut cuts = BTreeSet::from([DecimalValue::minimum()]);
            for pattern in &patterns {
                if let PatternSet::Decimal(Some((lo, hi))) = pattern {
                    cuts.insert(*lo);
                    if let Some(next) = hi.next_up() {
                        cuts.insert(next);
                    }
                }
            }
            let cuts: Vec<_> = cuts.into_iter().collect();
            cuts.iter()
                .enumerate()
                .map(|(index, lo)| {
                    Atom::Decimal(
                        *lo,
                        cuts.get(index + 1)
                            .and_then(|next| next.next_down())
                            .unwrap_or_else(DecimalValue::maximum),
                    )
                })
                .collect()
        }
        Type::Text | Type::Float => {
            let mut constants = Vec::new();
            for pattern in &patterns {
                if let PatternSet::Literal(value) = pattern
                    && !constants.contains(value)
                {
                    constants.push(value.clone());
                }
            }
            let mut atoms: Vec<_> = constants.iter().cloned().map(Atom::Literal).collect();
            atoms.push(Atom::Remainder(constants));
            atoms
        }
        _ => vec![Atom::Any],
    };
    if matches!(ty, Type::Nullable(_)) {
        atoms.push(Atom::Null);
    }
    atoms
}

fn contains(pattern: &PatternSet, atom: &Atom) -> bool {
    match (pattern, atom) {
        (PatternSet::Any(nullable), Atom::Null) => *nullable,
        (PatternSet::Any(_), _) => true,
        (PatternSet::Null, Atom::Null) => true,
        (PatternSet::Bool(left), Atom::Bool(right)) => left == right,
        (PatternSet::Variant(left), Atom::Variant(right)) => left == right,
        (PatternSet::Literal(left), Atom::Literal(right)) => left == right,
        (PatternSet::Int(Some((lo, hi))), Atom::Int(start, _)) => lo <= start && start <= hi,
        (PatternSet::Decimal(Some((lo, hi))), Atom::Decimal(start, _)) => {
            lo <= start && start <= hi
        }
        _ => false,
    }
}
