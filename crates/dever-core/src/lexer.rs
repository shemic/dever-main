use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;
use crate::token::{Kind, Token};
use std::ops::Range;

pub(crate) fn lex(source: &SourceFile, range: Range<usize>) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let mut lexer = Lexer {
        source,
        offset: range.start,
        end: range.end,
        tokens: Vec::new(),
        errors: Vec::new(),
    };
    while let Some(ch) = lexer.peek() {
        let start = lexer.offset;
        match ch {
            ' ' | '\t' => {
                lexer.advance();
            }
            '\n' | '\r' => {
                lexer.advance();
                if ch == '\r' && lexer.peek() == Some('\n') {
                    lexer.advance();
                }
                lexer.push(start, Kind::Newline);
            }
            '#' => lexer.comment(),
            '"' => lexer.text(),
            '0'..='9' => lexer.number(),
            'a'..='z' | 'A'..='Z' | '_' => lexer.name(),
            _ => lexer.symbol(),
        }
    }
    lexer.push(lexer.offset, Kind::End);
    if lexer.errors.is_empty() {
        Ok(lexer.tokens)
    } else {
        Err(lexer.errors)
    }
}

struct Lexer<'a> {
    source: &'a SourceFile,
    offset: usize,
    end: usize,
    tokens: Vec<Token>,
    errors: Vec<Diagnostic>,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.source.text()[self.offset..self.end].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }

    fn push(&mut self, start: usize, kind: Kind) {
        self.tokens.push(Token {
            kind,
            span: self.source.span(start, self.offset),
        });
    }

    fn error(&mut self, start: usize, code: &'static str, message: impl Into<String>) {
        self.errors.push(Diagnostic::error(
            code,
            message,
            self.source.span(start, self.offset),
        ));
    }

    fn comment(&mut self) {
        let start = self.offset;
        while self.peek().is_some_and(|ch| ch != '\n' && ch != '\r') {
            self.advance();
        }
        self.push(
            start,
            Kind::Comment(self.source.text()[start..self.offset].into()),
        );
    }

    fn name(&mut self) {
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            self.advance();
        }
        let spelling = &self.source.text()[start..self.offset];
        let kind = match spelling {
            "package" => Kind::Package,
            "exposes" => Kind::Exposes,
            "public" => Kind::Public,
            "type" => Kind::Type,
            "true" => Kind::True,
            "false" => Kind::False,
            "null" => Kind::Null,
            "other" => Kind::Other,
            "and" => Kind::And,
            "or" => Kind::Or,
            "not" => Kind::Not,
            "if" | "else" | "switch" | "match" | "when" | "by" | "for" | "while" | "break"
            | "continue" | "iterate" | "recur" | "repeat" | "return" | "function" | "throw"
            | "catch" | "try" | "import" | "use" | "class" | "interface" | "unsafe" | "macro" => {
                self.error(
                    start,
                    "L003",
                    format!("'{spelling}' is not supported in Core 0.1"),
                );
                return;
            }
            _ => Kind::Name(spelling.into()),
        };
        self.push(start, kind);
    }

    fn digits(&mut self) {
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            self.advance();
        }
    }

    fn number(&mut self) {
        let start = self.offset;
        self.digits();
        if self.peek() == Some('.')
            && self.source.text()[self.offset + 1..self.end]
                .starts_with(|ch: char| ch.is_ascii_digit())
        {
            self.advance();
            self.digits();
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.advance();
            if matches!(self.peek(), Some('+' | '-')) {
                self.advance();
            }
            let exponent_start = self.offset;
            self.digits();
            if exponent_start == self.offset {
                self.error(start, "L004", "expected digits in numeric exponent");
            }
        }
        if self
            .peek()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        {
            while self
                .peek()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
            {
                self.advance();
            }
            self.error(start, "L004", "numeric literals cannot have a suffix");
        }
        self.push(
            start,
            Kind::Number(self.source.text()[start..self.offset].into()),
        );
    }

    fn text(&mut self) {
        let start = self.offset;
        self.advance();
        let mut value = String::new();
        loop {
            let escape_start = self.offset;
            match self.advance() {
                Some('"') => {
                    self.push(start, Kind::Text(value));
                    return;
                }
                Some('\n' | '\r') | None => {
                    self.error(
                        start,
                        "L005",
                        "unterminated Text literal; use an escape for a newline",
                    );
                    return;
                }
                Some('\\') => match self.advance() {
                    Some('\\') => value.push('\\'),
                    Some('"') => value.push('"'),
                    Some('n') => value.push('\n'),
                    Some('r') => value.push('\r'),
                    Some('t') => value.push('\t'),
                    Some('u') => match self.unicode_escape() {
                        Some(ch) => value.push(ch),
                        None => self.error(
                            escape_start,
                            "L006",
                            "expected a Unicode scalar escape such as \\u{41}",
                        ),
                    },
                    _ => self.error(escape_start, "L006", "invalid Text escape"),
                },
                Some(ch) => value.push(ch),
            }
        }
    }

    fn unicode_escape(&mut self) -> Option<char> {
        if self.peek() != Some('{') {
            return None;
        }
        self.advance();
        let start = self.offset;
        while self.peek().is_some_and(|ch| ch.is_ascii_hexdigit()) {
            self.advance();
        }
        let digits = &self.source.text()[start..self.offset];
        if self.peek() != Some('}') {
            return None;
        }
        self.advance();
        if digits.is_empty() || digits.len() > 6 {
            return None;
        }
        u32::from_str_radix(digits, 16)
            .ok()
            .and_then(char::from_u32)
    }

    fn symbol(&mut self) {
        let start = self.offset;
        let ch = self.advance().expect("symbol starts at a character");
        let pair = match (ch, self.peek()) {
            ('/', Some('/')) => Some(Kind::IntegerDivide),
            ('=', Some('=')) => Some(Kind::Equal),
            ('!', Some('=')) => Some(Kind::NotEqual),
            ('<', Some('=')) => Some(Kind::LessEqual),
            ('>', Some('=')) => Some(Kind::GreaterEqual),
            _ => None,
        };
        if let Some(kind) = pair {
            self.advance();
            self.push(start, kind);
            return;
        }
        let kind = match ch {
            '(' => Kind::LeftParen,
            ')' => Kind::RightParen,
            '{' => Kind::LeftBrace,
            '}' => Kind::RightBrace,
            '[' => Kind::LeftBracket,
            ']' => Kind::RightBracket,
            ':' => Kind::Colon,
            ',' => Kind::Comma,
            '.' => Kind::Dot,
            '?' => Kind::Question,
            '=' => Kind::Assign,
            '+' => Kind::Plus,
            '-' => Kind::Minus,
            '*' => Kind::Star,
            '/' => Kind::Slash,
            '%' => Kind::Percent,
            '<' => Kind::Less,
            '>' => Kind::Greater,
            _ => {
                self.error(
                    start,
                    "L001",
                    format!("unexpected character '{ch}'; identifiers must use ASCII"),
                );
                return;
            }
        };
        self.push(start, kind);
    }
}
