use crate::diagnostic::Diagnostic;
use crate::hir::Program;
use crate::syntax::MAX_SYNTAX_DEPTH;
use crate::types::{Shape, Type};

mod dependencies;
pub(crate) use dependencies::Dependencies;
mod effects;
mod error_effects;
pub(crate) use error_effects::failure_names;
// A bounded proof must reject excessive work, never treat it as a successful proof.
const MAX_PROOF_WORK: usize = 16_384;
mod facts;
mod failures;
mod redundancy;
pub(crate) use facts::bounds_subset;
pub(crate) use redundancy::application_errors;

pub(crate) fn check(program: &mut Program, errors: &mut Vec<Diagnostic>) -> Result<(), Diagnostic> {
    if let Some(error) = type_depth_error(program) {
        // 结构无效时尚未生成分析结果，调用方必须停止依赖这些结果的后续检查。
        return Err(error);
    }
    let dependencies = dependencies::Dependencies::new(program);
    effects::check(program, &dependencies, errors);
    error_effects::check(program, &dependencies, errors);
    facts::check(program, &dependencies, errors);
    failures::check(program, &dependencies, errors);
    redundancy::check(program);
    Ok(())
}

fn type_depth_error(program: &Program) -> Option<Diagnostic> {
    // Nominal references can form a deep DAG despite shallow declaration syntax.
    // Resolve depths iteratively so the limit itself cannot overflow the host stack.
    let mut depths = vec![0; program.types.len()];
    loop {
        let mut changed = false;
        for (id, definition) in program.types.iter().enumerate() {
            let depth = 1 + match &definition.shape {
                Shape::Record(fields) => fields
                    .iter()
                    .map(|field| type_depth(&field.ty, &depths))
                    .max()
                    .unwrap_or(0),
                Shape::Choice(variants) => variants
                    .iter()
                    .flat_map(|variant| &variant.fields)
                    .map(|field| type_depth(&field.ty, &depths))
                    .max()
                    .unwrap_or(0),
            };
            if depth > MAX_SYNTAX_DEPTH {
                return Some(Diagnostic::error(
                    "C015",
                    "contract type nesting exceeds the structural depth limit",
                    definition.span,
                ));
            }
            changed |= depths[id] != depth;
            depths[id] = depth;
        }
        if !changed {
            return None;
        }
    }
}

fn type_depth(ty: &Type, depths: &[usize]) -> usize {
    match ty {
        Type::Named(id) => depths[*id],
        Type::List(inner)
        | Type::Stream(inner)
        | Type::AsyncStream(inner)
        | Type::RowStream(inner)
        | Type::Nullable(inner) => 1 + type_depth(inner, depths),
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            1 + type_depth(key, depths).max(type_depth(value, depths))
        }
        Type::Outputs(fields) => {
            1 + fields
                .iter()
                .map(|field| type_depth(&field.ty, depths))
                .max()
                .unwrap_or(0)
        }
        _ => 0,
    }
}
