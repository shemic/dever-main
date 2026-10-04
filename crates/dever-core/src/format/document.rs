#[derive(Clone)]
pub(super) enum Document {
    Text(String),
    Line,
    BlankLine,
    Break(&'static str),
    Sequence(Vec<Document>),
    Indented(Box<Document>),
    Group(Box<Document>),
}

impl Document {
    pub(super) fn sequence(parts: Vec<Self>) -> Self {
        Self::Sequence(parts)
    }

    pub(super) fn space() -> Self {
        Self::Text(" ".into())
    }

    pub(super) fn indented(self) -> Self {
        Self::Indented(Box::new(self))
    }

    pub(super) fn grouped(self) -> Self {
        Self::Group(Box::new(self))
    }

    fn flat_width(&self) -> Option<usize> {
        match self {
            Self::Text(text) => Some(text.chars().count()),
            Self::Break(text) => Some(text.len()),
            Self::Line | Self::BlankLine => None,
            Self::Sequence(parts) => parts
                .iter()
                .try_fold(0, |width, part| Some(width + part.flat_width()?)),
            Self::Indented(inner) | Self::Group(inner) => inner.flat_width(),
        }
    }

    pub(super) fn render(&self) -> String {
        let mut writer = Writer {
            output: String::new(),
            column: 0,
        };
        writer.document(self, 0, false);
        writer.output.truncate(writer.output.trim_end().len());
        writer.output.push('\n');
        writer.output
    }
}

pub(super) fn join(parts: Vec<Document>, separator: Document) -> Document {
    let mut joined = Vec::new();
    for (index, part) in parts.into_iter().enumerate() {
        if index != 0 {
            joined.push(separator.clone());
        }
        joined.push(part);
    }
    Document::sequence(joined)
}

struct Writer {
    output: String,
    column: usize,
}

impl Writer {
    fn document(&mut self, document: &Document, indent: usize, flat: bool) {
        match document {
            Document::Text(text) => self.text(text, indent),
            Document::Line => self.newline(),
            Document::BlankLine => {
                self.newline();
                if !self.output.is_empty() && !self.output.ends_with("\n\n") {
                    self.output.push('\n');
                }
            }
            Document::Break(text) => {
                if flat {
                    self.text(text, indent);
                } else {
                    self.newline();
                }
            }
            Document::Sequence(parts) => {
                for part in parts {
                    self.document(part, indent, flat);
                }
            }
            Document::Indented(inner) => self.document(inner, indent + 2, flat),
            Document::Group(inner) => {
                let column = if self.column == 0 {
                    indent
                } else {
                    self.column
                };
                let fits = inner.flat_width().is_some_and(|width| column + width <= 88);
                self.document(inner, indent, flat || fits);
            }
        }
    }

    fn text(&mut self, text: &str, indent: usize) {
        if text.is_empty() || (self.column == 0 && text == " ") {
            return;
        }
        if self.column == 0 {
            self.output.extend(std::iter::repeat_n(' ', indent));
            self.column = indent;
        }
        self.output.push_str(text);
        self.column += text.chars().count();
    }

    fn newline(&mut self) {
        while self.output.ends_with(' ') {
            self.output.pop();
        }
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.column = 0;
    }
}
