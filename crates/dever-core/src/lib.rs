pub mod diagnostic;
pub mod format;
pub mod hir;
pub mod llvm;
pub mod native;
#[cfg(feature = "reference")]
pub mod reference;
pub mod source;
pub mod syntax;
pub mod wire;

mod api;
mod capture;
mod check;
mod contracts;
mod intrinsic;
mod lexer;
mod markdown;
mod model;
mod parser;
mod specialize;
mod token;
mod types;

pub use check::{check, check_with_bindings, check_with_settings};

use diagnostic::Diagnostic;
use source::SourceFile;
use syntax::Package;

/// Parses source syntax only. Name resolution and type checking are separate phases.
pub fn parse(source: &SourceFile) -> Result<Package, Vec<Diagnostic>> {
    if source::SourceMap::is_standard(source.id()) {
        return source::parsed_standard(source.id());
    }
    parse_uncached(source)
}

fn parse_uncached(source: &SourceFile) -> Result<Package, Vec<Diagnostic>> {
    parse_source(source)
        .map(|parsed| parsed.package)
        .map_err(|mut diagnostics| {
            diagnostics.sort_by_key(|diagnostic| {
                (
                    diagnostic.primary.start,
                    diagnostic.primary.end,
                    diagnostic.code,
                )
            });
            diagnostics
        })
}

struct ParsedSource {
    package: Package,
    tokens: Vec<token::Token>,
    blocks: Option<Vec<markdown::ProgramBlock>>,
}

// Parsing and formatting share extraction and block-boundary validation.
fn parse_source(source: &SourceFile) -> Result<ParsedSource, Vec<Diagnostic>> {
    let blocks = source
        .is_markdown()
        .then(|| markdown::blocks(source))
        .transpose()?;
    let mut tokens = Vec::new();
    if let Some(blocks) = &blocks {
        let mut errors = Vec::new();
        for block in blocks {
            match lexer::lex(source, block.content.clone()) {
                Ok(mut block_tokens) => {
                    block_tokens.pop(); // Each block's End is replaced by the package's End.
                    tokens.extend(block_tokens);
                }
                Err(block_errors) => errors.extend(block_errors),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let end = blocks
            .last()
            .expect("Markdown has program blocks")
            .content
            .end;
        tokens.push(token::Token {
            kind: token::Kind::End,
            span: source.span(end, end),
        });
    } else {
        tokens = lexer::lex(source, 0..source.text().len())?;
    }
    let package = parser::parse(tokens.clone(), source)?;
    if let Some(blocks) = &blocks {
        markdown::validate_declarations(&package, blocks)?;
        markdown::validate_structure(source, &package, blocks)?;
    }
    Ok(ParsedSource {
        package,
        tokens,
        blocks,
    })
}
