use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::diagnostic::Diagnostic;
use crate::markdown::normalize_lone_carriage_returns;
use crate::source::{SourceFile, Span};

use super::model::{
    DocumentValue, DocumentedList, FunctionDocumentation, ModelDocumentation, PackageDocumentation,
    TypeDocumentation, TypeEntries,
};

pub(super) fn package(
    source: &SourceFile,
    range: Range<usize>,
    heading: Span,
) -> Result<PackageDocumentation, Vec<Diagnostic>> {
    let mut parser = MetadataParser::new(source, range, heading, "M004", "- 包：")?;
    let name = parser.scalar("- 包：")?;
    let public_types = parser.list("- 公开类型：", false)?;
    let public_functions = parser.list("- 公开方法：", false)?;
    let usages = parser.list("- 使用：", false)?;
    parser.finish()?;
    Ok(PackageDocumentation {
        name,
        public_types,
        public_functions,
        usages,
    })
}

pub(super) fn type_declaration(
    source: &SourceFile,
    range: Range<usize>,
    heading: Span,
    entries: TypeEntries,
) -> Result<TypeDocumentation, Vec<Diagnostic>> {
    let mut parser = MetadataParser::new(source, range, heading, "M005", "- 类型：")?;
    let name = parser.scalar("- 类型：")?;
    let entries = parser.list(
        match entries {
            TypeEntries::Fields => "- 字段：",
            TypeEntries::Variants => "- 分支：",
        },
        true,
    )?;
    parser.finish()?;
    Ok(TypeDocumentation { name, entries })
}

pub(super) fn function(
    source: &SourceFile,
    range: Range<usize>,
    heading: Span,
    has_failures: bool,
) -> Result<FunctionDocumentation, Vec<Diagnostic>> {
    let mut parser = MetadataParser::new(source, range, heading, "M005", "- 函数：")?;
    let name = parser.scalar("- 函数：")?;
    let inputs = parser.list("- 输入：", true)?;
    let outputs = parser.list("- 输出：", true)?;
    let fails = if has_failures {
        Some(parser.scalar("- 允许失败：")?)
    } else {
        None
    };
    parser.finish()?;
    Ok(FunctionDocumentation {
        name,
        inputs,
        outputs,
        fails,
    })
}

pub(super) fn model_declaration(
    source: &SourceFile,
    range: Range<usize>,
    heading: Span,
    sql: bool,
) -> Result<ModelDocumentation, Vec<Diagnostic>> {
    let mut parser = MetadataParser::new(source, range, heading, "M005", "- 声明：")?;
    let declaration = parser.scalar("- 声明：")?;
    let signature = if sql {
        Some((
            parser.list("- 输入：", true)?,
            parser.list("- 输出：", true)?,
        ))
    } else {
        None
    };
    parser.finish()?;
    Ok(ModelDocumentation {
        declaration,
        signature,
    })
}

struct MetadataParser<'a> {
    source: &'a SourceFile,
    lines: Vec<SourceLine<'a>>,
    cursor: usize,
    fallback: Span,
    code: &'static str,
}

impl<'a> MetadataParser<'a> {
    fn new(
        source: &'a SourceFile,
        range: Range<usize>,
        fallback: Span,
        code: &'static str,
        marker: &str,
    ) -> Result<Self, Vec<Diagnostic>> {
        let description_start = range.start;
        let lines = source_lines(source, range);
        let Some(cursor) = lines
            .iter()
            .position(|line| line.text.trim_start().starts_with(marker))
        else {
            return Err(vec![Diagnostic::error(
                code,
                format!("section requires metadata beginning with '{marker}'"),
                fallback,
            )]);
        };
        if !has_natural_description(source, description_start..lines[cursor].start) {
            return Err(vec![Diagnostic::error(
                code,
                "section requires a natural-language description before its metadata",
                fallback,
            )]);
        }
        Ok(Self {
            source,
            lines,
            cursor,
            fallback,
            code,
        })
    }

    fn scalar(&mut self, prefix: &str) -> Result<DocumentValue, Vec<Diagnostic>> {
        self.skip_blank();
        let line = self.current()?;
        let value = parse_inline_value(self.source, line, prefix, false).map_err(|message| {
            vec![Diagnostic::error(
                self.code,
                message,
                line.span(self.source),
            )]
        })?;
        self.cursor += 1;
        Ok(value)
    }

    fn list(&mut self, header: &str, described: bool) -> Result<DocumentedList, Vec<Diagnostic>> {
        self.skip_blank();
        let line = self.current()?;
        let text = line.text.trim_end();
        if text == format!("{header}无") {
            self.cursor += 1;
            return Ok(DocumentedList {
                values: Vec::new(),
                span: line.span(self.source),
            });
        }
        if text != header {
            return Err(vec![Diagnostic::error(
                self.code,
                format!("expected '{header}' or '{header}无'"),
                line.span(self.source),
            )]);
        }
        let span = line.span(self.source);
        self.cursor += 1;
        let mut values = Vec::new();
        loop {
            self.skip_blank();
            let Some(line) = self.lines.get(self.cursor).copied() else {
                break;
            };
            if !line.text.starts_with("  - ") {
                break;
            }
            let value =
                parse_inline_value(self.source, line, "  - ", described).map_err(|message| {
                    vec![Diagnostic::error(
                        self.code,
                        message,
                        line.span(self.source),
                    )]
                })?;
            values.push(value);
            self.cursor += 1;
        }
        if values.is_empty() {
            return Err(vec![Diagnostic::error(
                self.code,
                format!("'{header}' requires at least one item; use '{header}无' when empty"),
                span,
            )]);
        }
        Ok(DocumentedList { values, span })
    }

    fn finish(&mut self) -> Result<(), Vec<Diagnostic>> {
        self.skip_blank();
        if let Some(line) = self.lines.get(self.cursor) {
            return Err(vec![Diagnostic::error(
                self.code,
                "unexpected content between documentation metadata and its Dever code block",
                line.span(self.source),
            )]);
        }
        Ok(())
    }

    fn current(&self) -> Result<SourceLine<'a>, Vec<Diagnostic>> {
        self.lines.get(self.cursor).copied().ok_or_else(|| {
            vec![Diagnostic::error(
                self.code,
                "incomplete documentation metadata",
                self.fallback,
            )]
        })
    }

    fn skip_blank(&mut self) {
        while self
            .lines
            .get(self.cursor)
            .is_some_and(|line| line.text.trim().is_empty())
        {
            self.cursor += 1;
        }
    }
}

fn has_natural_description(source: &SourceFile, range: Range<usize>) -> bool {
    let markdown = normalize_lone_carriage_returns(source.text());
    let mut depth = 0usize;
    let mut in_top_level_paragraph = false;
    for event in Parser::new(&markdown[range]) {
        match event {
            Event::Start(Tag::Paragraph) if depth == 0 => {
                in_top_level_paragraph = true;
                depth += 1;
            }
            Event::Start(_) => depth += 1,
            Event::End(TagEnd::Paragraph) => {
                depth = depth.saturating_sub(1);
                in_top_level_paragraph = false;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Text(text) if in_top_level_paragraph && !text.trim().is_empty() => return true,
            _ => {}
        }
    }
    false
}

fn parse_inline_value(
    source: &SourceFile,
    line: SourceLine<'_>,
    prefix: &str,
    described: bool,
) -> Result<DocumentValue, String> {
    let text = line.text.trim_end();
    let rest = text
        .strip_prefix(prefix)
        .ok_or_else(|| format!("expected '{prefix}`...`'"))?;
    let Some(rest) = rest.strip_prefix('`') else {
        return Err(format!("expected '{prefix}`...`'"));
    };
    let Some(closing) = rest.find('`') else {
        return Err("documentation metadata has an unclosed inline code value".into());
    };
    let value = &rest[..closing];
    if value.is_empty() || value.trim() != value {
        return Err(
            "documentation metadata value must be nonempty without surrounding spaces".into(),
        );
    }
    let suffix = &rest[closing + 1..];
    if described {
        let Some(description) = suffix.strip_prefix('：') else {
            return Err(
                "each input, output, field or variant requires '：' and a description".into(),
            );
        };
        if description.trim().is_empty() {
            return Err("input, output, field or variant description must not be empty".into());
        }
    } else if !suffix.is_empty() {
        return Err("unexpected text after documentation metadata value".into());
    }
    let value_start = line.start + prefix.len() + 1;
    Ok(DocumentValue {
        text: value.to_owned(),
        span: source.span(value_start, value_start + value.len()),
    })
}

fn source_lines(source: &SourceFile, range: Range<usize>) -> Vec<SourceLine<'_>> {
    let text = source.text();
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = range.start;
    while start < range.end {
        let mut end = start;
        while end < range.end && !matches!(bytes[end], b'\r' | b'\n') {
            end += 1;
        }
        lines.push(SourceLine {
            text: &text[start..end],
            start,
            end,
        });
        if end == range.end {
            break;
        }
        start = end + usize::from(bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n')) + 1;
    }
    lines
}

#[derive(Clone, Copy)]
struct SourceLine<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

impl SourceLine<'_> {
    fn span(self, source: &SourceFile) -> Span {
        source.span(self.start, self.end)
    }
}
