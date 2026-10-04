use crate::markdown::{ProgramBlock, declaration_span};
use crate::source::SourceFile;
use crate::syntax::Package;
use crate::token::{Kind, Token};

pub(super) fn format(
    source: &SourceFile,
    package: &Package,
    tokens: &[Token],
    blocks: &[ProgramBlock],
) -> String {
    let mut output = String::new();
    let mut copied = 0;
    for block in blocks {
        output.push_str(&source.text()[copied..block.content.start]);
        let first = tokens.partition_point(|token| token.span.start < block.content.start);
        let last = tokens.partition_point(|token| token.span.start < block.content.end);
        let mut block_tokens = tokens[first..last].to_vec();
        if block_tokens
            .iter()
            .any(|token| !matches!(token.kind, Kind::Newline))
        {
            block_tokens.push(Token {
                kind: Kind::End,
                span: source.span(block.content.end, block.content.end),
            });
            let declarations = &package.declarations;
            let first = declarations.partition_point(|declaration| {
                declaration_span(declaration).start < block.content.start
            });
            let last = declarations.partition_point(|declaration| {
                declaration_span(declaration).start < block.content.end
            });
            let formatted =
                super::format_fragment(source, block_tokens, &declarations[first..last]);
            for line in formatted.split_terminator('\n') {
                if !line.is_empty() {
                    output.push_str(&block.indentation);
                }
                output.push_str(line);
                output.push_str(block.newline);
            }
        }
        copied = block.content.end;
    }
    output.push_str(&source.text()[copied..]);
    output
}
