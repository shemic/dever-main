use crate::diagnostic::Diagnostic;
use crate::hir::Function;
use crate::source::SourceFile;
use crate::syntax::Package;
use crate::types::Definition;

use super::ProgramBlock;

mod metadata;
mod model;
mod parse;
mod validate;

pub(crate) fn validate_structure(
    source: &SourceFile,
    package: &Package,
    blocks: &[ProgramBlock],
) -> Result<(), Vec<Diagnostic>> {
    parse::contract(source, package, blocks).map(|_| ())
}

pub(crate) fn validate_contract(
    source: &SourceFile,
    package: &Package,
    owner: usize,
    definitions: &[Definition],
    functions: &[Function],
) -> Vec<Diagnostic> {
    let Ok(blocks) = super::blocks(source) else {
        return Vec::new();
    };
    let Ok(contract) = parse::contract(source, package, &blocks) else {
        return Vec::new();
    };
    validate::contract(&contract, package, owner, definitions, functions)
}
