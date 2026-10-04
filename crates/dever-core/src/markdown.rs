//! Markdown documents declarations; only explicit top-level program fences produce Dever tokens.

use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

use crate::diagnostic::Diagnostic;
use crate::source::{SourceFile, Span};
use crate::syntax::{Declaration, Package};

mod contract;
pub(crate) use contract::{validate_contract, validate_structure};

pub(crate) struct ProgramBlock {
    pub outer: Range<usize>,
    pub content: Range<usize>,
    pub indentation: String,
    pub newline: &'static str,
}

pub(crate) fn blocks(source: &SourceFile) -> Result<Vec<ProgramBlock>, Vec<Diagnostic>> {
    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    let mut depth = 0;
    let markdown = normalize_lone_carriage_returns(source.text());
    for (event, range) in Parser::new(&markdown).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0
                    && matches!(tag, Tag::CodeBlock(CodeBlockKind::Fenced(ref info)) if is_program_info(info))
                {
                    match program_block(source, range) {
                        Ok(Some(block)) => blocks.push(block),
                        Ok(None) => {}
                        Err(error) => errors.push(error),
                    }
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    if errors.is_empty() && blocks.is_empty() {
        errors.push(Diagnostic::error(
            "M001",
            "Markdown source requires at least one top-level fenced 'dever' or 'typescript dever' code block",
            source.span(0, 0),
        ));
    }
    if errors.is_empty() {
        Ok(blocks)
    } else {
        Err(errors)
    }
}

fn normalize_lone_carriage_returns(text: &str) -> Cow<'_, str> {
    let is_lone_cr =
        |offset, byte| byte == b'\r' && text.as_bytes().get(offset + 1) != Some(&b'\n');
    if !text
        .bytes()
        .enumerate()
        .any(|(offset, byte)| is_lone_cr(offset, byte))
    {
        return Cow::Borrowed(text);
    }
    // The Markdown parser requires LF. Replacing only bare CR preserves every byte offset.
    Cow::Owned(
        text.char_indices()
            .map(|(offset, ch)| {
                if ch == '\r' && is_lone_cr(offset, b'\r') {
                    '\n'
                } else {
                    ch
                }
            })
            .collect(),
    )
}

fn program_block(
    source: &SourceFile,
    range: Range<usize>,
) -> Result<Option<ProgramBlock>, Diagnostic> {
    let outer = range.clone();
    let text = source.text();
    let opening_end = text[range.start..]
        .find(['\r', '\n'])
        .map_or(text.len(), |offset| range.start + offset);
    let opening = &text[range.start..opening_end];
    let fence = opening.as_bytes()[0];
    let fence_length = opening.bytes().take_while(|byte| *byte == fence).count();
    // CommonMark unescapes info strings. Only a literal language marker opts into execution.
    if !is_program_info(opening[fence_length..].trim()) {
        return Ok(None);
    }
    let newline = if text[opening_end..].starts_with("\r\n") {
        "\r\n"
    } else if text[opening_end..].starts_with('\r') {
        "\r"
    } else {
        "\n"
    };
    let content_start = (opening_end + newline.len()).min(text.len());
    let closing_start = line_start(text, range.end);
    let closing = &text[closing_start..range.end];
    let closing_fence = closing.trim_start_matches(' ');
    let closing_length = closing_fence
        .bytes()
        .take_while(|byte| *byte == fence)
        .count();
    // CommonMark permits an EOF-terminated fence; executable source deliberately requires closure.
    if closing_start < content_start
        || closing.len() - closing_fence.len() > 3
        || closing_length < fence_length
        || !closing_fence[closing_length..].trim_matches(' ').is_empty()
    {
        return Err(Diagnostic::error(
            "M002",
            "Dever program fence must have a matching closing fence",
            source.span(range.start, opening_end),
        ));
    }
    Ok(Some(ProgramBlock {
        outer,
        content: content_start..closing_start,
        indentation: text[line_start(text, range.start)..range.start].to_owned(),
        newline,
    }))
}

fn is_program_info(info: &str) -> bool {
    matches!(info.trim(), "dever" | "typescript dever")
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset]
        .rfind(['\r', '\n'])
        .map_or(0, |index| index + 1)
}

pub(crate) fn validate_declarations(
    package: &Package,
    blocks: &[ProgramBlock],
) -> Result<(), Vec<Diagnostic>> {
    let declarations = package.declarations.iter().map(declaration_span);
    let errors: Vec<_> = declarations
        .filter_map(|span| {
            let index = blocks.partition_point(|block| block.content.end <= span.start);
            let complete = blocks.get(index).is_some_and(|block| {
                block.content.start <= span.start && span.end <= block.content.end
            });
            if complete {
                return None;
            }
            Some(Diagnostic::error(
                "M003",
                "each declaration must be complete within one Dever code block",
                span,
            ))
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub(crate) fn declaration_span(declaration: &Declaration) -> Span {
    match declaration {
        Declaration::Api(binding) => binding.span,
        Declaration::Rest(rest) => rest.span,
        Declaration::External(external) => external.span,
        Declaration::Type(declaration) => declaration.span,
        Declaration::Function(function) => function.span,
        Declaration::Database(binding) => binding.span,
        Declaration::Schedule(schedule) => schedule.span,
        Declaration::ModelIndex(index) => index.span,
        Declaration::Relation(relation) => relation.span,
        Declaration::Seed(seed) => seed.span,
        Declaration::Migration(migration) => migration.span,
        Declaration::ModelSql(sql) => sql.span,
    }
}
