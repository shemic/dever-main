//! Finite abstract-domain operations. Unknowns never establish a numeric bound.

use crate::hir::{Atom, Constant, Domain};
use crate::syntax::{BinaryOperator as Binary, UnaryOperator};
use dever_runtime::number::DecimalValue;

use super::Fact;

pub(super) fn constant_atom(constant: &Constant) -> Atom {
    match constant {
        Constant::Null => Atom::Null,
        Constant::Bool(value) => Atom::Bool(*value),
        Constant::Int(value) => Atom::Int(*value, *value),
        Constant::Decimal(value) => Atom::Decimal(*value, *value),
        _ => Atom::Literal(constant.clone()),
    }
}

pub(super) fn subset(domain: &Domain, bounds: &Domain) -> bool {
    !domain.is_empty()
        && domain.iter().all(|value| {
            bounds.iter().any(|bound| match (value, bound) {
                (_, Atom::Any) => true,
                (Atom::Int(lo, hi), Atom::Int(min, max)) => min <= lo && hi <= max,
                (Atom::Decimal(lo, hi), Atom::Decimal(min, max)) => min <= lo && hi <= max,
                _ => value == bound,
            })
        })
}

/// Empty contract bounds mean the complete type domain, not an empty value set.
pub(crate) fn bounds_subset(actual: &Domain, required: &Domain) -> bool {
    required.is_empty() || (!actual.is_empty() && subset(actual, required))
}

pub(super) fn disjoint(left: &Domain, right: &Domain) -> bool {
    left.iter().all(|left| {
        right.iter().all(|right| match (left, right) {
            (Atom::Any | Atom::Remainder(_), _) | (_, Atom::Any | Atom::Remainder(_)) => false,
            (Atom::Int(lo, hi), Atom::Int(min, max)) => hi < min || max < lo,
            (Atom::Decimal(lo, hi), Atom::Decimal(min, max)) => hi < min || max < lo,
            _ => left != right,
        })
    })
}

pub(super) fn union(left: &Domain, right: &Domain) -> Domain {
    let mut result = left.clone();
    for atom in right {
        if !result.contains(atom) {
            result.push(atom.clone());
        }
    }
    if result.iter().any(|atom| matches!(atom, Atom::Any)) {
        return vec![Atom::Any];
    }
    if result.iter().all(|atom| matches!(atom, Atom::Int(..))) {
        let (lo, hi) = result.iter().fold((i64::MAX, i64::MIN), |(lo, hi), atom| {
            let Atom::Int(min, max) = atom else {
                unreachable!()
            };
            (lo.min(*min), hi.max(*max))
        });
        return vec![Atom::Int(lo, hi)];
    }
    if result.iter().all(|atom| matches!(atom, Atom::Decimal(..))) {
        let (lo, hi) = result.iter().fold(
            (DecimalValue::maximum(), DecimalValue::minimum()),
            |(lo, hi), atom| {
                let Atom::Decimal(min, max) = atom else {
                    unreachable!()
                };
                (lo.min(*min), hi.max(*max))
            },
        );
        return vec![Atom::Decimal(lo, hi)];
    }
    // Widen large finite unions rather than growing summaries with every caller.
    if result.len() > 32 {
        vec![Atom::Any]
    } else {
        result
    }
}

pub(super) fn unary(operator: UnaryOperator, value: &Fact) -> Option<Fact> {
    let domain = value
        .domain
        .iter()
        .map(|atom| match (operator, atom) {
            (UnaryOperator::Not, Atom::Bool(value)) => Some(Atom::Bool(!value)),
            (UnaryOperator::Negate, Atom::Int(lo, hi)) => {
                Some(Atom::Int(hi.checked_neg()?, lo.checked_neg()?))
            }
            (UnaryOperator::Negate, Atom::Decimal(lo, hi)) => Some(Atom::Decimal(
                hi.checked_neg().ok()?,
                lo.checked_neg().ok()?,
            )),
            _ => None,
        })
        .collect::<Option<Domain>>()?;
    Some(Fact::domain(domain))
}

pub(super) fn binary(operator: Binary, left: &Fact, right: &Fact) -> Option<Fact> {
    if left.domain.len() != 1 || right.domain.len() != 1 {
        return None;
    }
    let atom = match (&left.domain[0], &right.domain[0]) {
        (Atom::Int(lo, hi), Atom::Int(min, max)) => match operator {
            Binary::Add => Atom::Int(lo.checked_add(*min)?, hi.checked_add(*max)?),
            Binary::Subtract => Atom::Int(lo.checked_sub(*max)?, hi.checked_sub(*min)?),
            Binary::Multiply => {
                let products = [
                    lo.checked_mul(*min)?,
                    lo.checked_mul(*max)?,
                    hi.checked_mul(*min)?,
                    hi.checked_mul(*max)?,
                ];
                Atom::Int(*products.iter().min()?, *products.iter().max()?)
            }
            _ => Atom::Bool(compare(operator, lo, hi, min, max)?),
        },
        (Atom::Decimal(lo, hi), Atom::Decimal(min, max)) => match operator {
            Binary::Add => Atom::Decimal(lo.checked_add(*min).ok()?, hi.checked_add(*max).ok()?),
            Binary::Subtract => {
                Atom::Decimal(lo.checked_sub(*max).ok()?, hi.checked_sub(*min).ok()?)
            }
            Binary::Multiply => {
                let products = [
                    lo.checked_mul(*min).ok()?,
                    lo.checked_mul(*max).ok()?,
                    hi.checked_mul(*min).ok()?,
                    hi.checked_mul(*max).ok()?,
                ];
                Atom::Decimal(*products.iter().min()?, *products.iter().max()?)
            }
            _ => Atom::Bool(compare(operator, lo, hi, min, max)?),
        },
        (Atom::Bool(left), Atom::Bool(right)) => Atom::Bool(match operator {
            Binary::And => *left && *right,
            Binary::Or => *left || *right,
            Binary::Equal => left == right,
            Binary::NotEqual => left != right,
            _ => return None,
        }),
        // A choice tag proves inequality across variants, but equal payload-bearing
        // tags do not prove equality of the complete values.
        (Atom::Variant(left), Atom::Variant(right)) if left != right => {
            Atom::Bool(match operator {
                Binary::Equal => false,
                Binary::NotEqual => true,
                _ => return None,
            })
        }
        (Atom::Null, Atom::Null) => Atom::Bool(match operator {
            Binary::Equal => true,
            Binary::NotEqual => false,
            _ => return None,
        }),
        (Atom::Null, other) | (other, Atom::Null)
            if !matches!(other, Atom::Any | Atom::Remainder(_)) =>
        {
            Atom::Bool(match operator {
                Binary::Equal => false,
                Binary::NotEqual => true,
                _ => return None,
            })
        }
        _ => return None,
    };
    Some(Fact::domain(vec![atom]))
}

fn compare<T: Ord>(operator: Binary, lo: &T, hi: &T, min: &T, max: &T) -> Option<bool> {
    match operator {
        Binary::Less if hi < min => Some(true),
        Binary::Less if lo >= max => Some(false),
        Binary::LessEqual if hi <= min => Some(true),
        Binary::LessEqual if lo > max => Some(false),
        Binary::Greater => compare(Binary::Less, min, max, lo, hi),
        Binary::GreaterEqual => compare(Binary::LessEqual, min, max, lo, hi),
        Binary::Equal if hi < min || max < lo => Some(false),
        Binary::Equal if lo == hi && min == max => Some(lo == min),
        Binary::NotEqual => compare(Binary::Equal, lo, hi, min, max).map(|value| !value),
        _ => None,
    }
}
