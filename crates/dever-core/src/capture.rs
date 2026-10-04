use std::collections::{BTreeMap, BTreeSet};

use crate::hir::Failure;
use crate::types::{Definition, Field, Shape, Type};

/// A capture may return its own error variant or wrap an error-only choice.
/// Wrapping preserves the source error's nominal identity and payload.
#[derive(Clone, Copy)]
pub(crate) struct Target {
    pub variant: usize,
    pub wrapped: bool,
}

pub(crate) fn targets(
    types: &[Definition],
    choice: usize,
    actual: &BTreeSet<Failure>,
) -> Result<BTreeMap<Failure, Target>, &'static str> {
    let Shape::Choice(variants) = &types[choice].shape else {
        unreachable!("checked capture choice")
    };
    let mut targets = BTreeMap::new();
    for (variant, definition) in variants.iter().enumerate().filter(|(_, value)| value.error) {
        let nested = error_only_payload(types, &definition.fields);
        let direct = Failure {
            ty: choice,
            variant,
        };
        if nested.is_none() || actual.contains(&direct) {
            targets.insert(
                direct,
                Target {
                    variant,
                    wrapped: false,
                },
            );
        }
        let Some((ty, count)) = nested else {
            continue;
        };
        // An explicitly raised outer variant remains nominal; it is not
        // implicitly unwrapped merely because it carries another error.
        if actual.contains(&direct) && !actual.iter().any(|failure| failure.ty == ty) {
            continue;
        }
        for nested_variant in 0..count {
            let failure = Failure {
                ty,
                variant: nested_variant,
            };
            if targets
                .insert(
                    failure,
                    Target {
                        variant,
                        wrapped: true,
                    },
                )
                .is_some()
            {
                return Err("result capture contains overlapping error wrappers");
            }
        }
    }
    Ok(targets)
}

fn error_only_payload(types: &[Definition], fields: &[Field]) -> Option<(usize, usize)> {
    let [field] = fields else {
        return None;
    };
    let Type::Named(ty) = field.ty else {
        return None;
    };
    let Shape::Choice(variants) = &types[ty].shape else {
        return None;
    };
    variants
        .iter()
        .all(|variant| variant.error)
        .then_some((ty, variants.len()))
}

pub(crate) fn database_errors(types: &[Definition]) -> Vec<Failure> {
    let ty = types
        .iter()
        .position(|definition| definition.name == "dever.database.Error")
        .expect("bundled database error choice");
    let Shape::Choice(variants) = &types[ty].shape else {
        unreachable!("bundled database error choice")
    };
    (0..variants.len())
        .map(|variant| Failure { ty, variant })
        .collect()
}
