use std::fmt::Write;

use crate::source::{SourceMap, Span};

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub primary: Span,
    pub related: Vec<Label>,
}

impl Diagnostic {
    pub(crate) fn error(code: &'static str, message: impl Into<String>, primary: Span) -> Self {
        Self {
            code,
            message: message.into(),
            primary,
            related: Vec::new(),
        }
    }

    pub fn render(&self, sources: &SourceMap) -> String {
        let mut rendered = String::new();
        render_label(
            &mut rendered,
            sources,
            self.primary,
            &format!(
                "{}[{}]: {}",
                if self.code.starts_with('W') {
                    "warning"
                } else {
                    "error"
                },
                self.code,
                self.message
            ),
        );
        for related in &self.related {
            render_label(
                &mut rendered,
                sources,
                related.span,
                &format!("note: {}", related.message),
            );
        }
        rendered
    }
}

fn render_label(output: &mut String, sources: &SourceMap, span: Span, message: &str) {
    let source = sources.get(span.source);
    let (line, column) = source.position(span.start);
    let line_text = source.line(line);
    writeln!(
        output,
        "{}:{line}:{column}: {message}",
        source.path().display()
    )
    .expect("string formatting");
    writeln!(output, "  | {line_text}").expect("string formatting");
}
