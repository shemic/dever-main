//! A deliberately small proof of read-only SQL, not a general SQL parser.
//! Anything outside this complete single-table projection keeps its write effect.

use crate::model::{ModelSchema, ModelSql};

pub(super) fn proven(model: &ModelSchema, operation: &ModelSql) -> bool {
    dialect(model, &operation.sqlite, b'?') && dialect(model, &operation.postgres, b'$')
}

fn dialect(model: &ModelSchema, sql: &str, marker: u8) -> bool {
    let Some(tokens) = tokens(sql, marker) else {
        return false;
    };
    let mut query = Projection {
        tokens: &tokens,
        position: 0,
        model,
    };
    if !query.keyword("SELECT") || !query.fields(false) || !query.keyword("FROM") {
        return false;
    }
    if !query.identifier(&model.table) {
        return false;
    }
    if query.keyword("WHERE") {
        loop {
            if !query.field() || !query.take(Token::Equal) || !query.take(Token::Bind) {
                return false;
            }
            if !query.keyword("AND") {
                break;
            }
        }
    }
    if query.keyword("ORDER") && (!query.keyword("BY") || !query.fields(true)) {
        return false;
    }
    if query.keyword("LIMIT") && !query.take(Token::Bind) {
        return false;
    }
    query.take(Token::Terminator);
    query.position == tokens.len()
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Token<'a> {
    Word(&'a str),
    Quoted(&'a str),
    Bind,
    Comma,
    Equal,
    Terminator,
}

impl Token<'_> {
    fn identifier(self, expected: &str) -> bool {
        match self {
            Self::Word(name) => name.eq_ignore_ascii_case(expected),
            Self::Quoted(name) => name == expected,
            _ => false,
        }
    }
}

fn tokens(sql: &str, marker: u8) -> Option<Vec<Token<'_>>> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let start = position;
        let byte = bytes[position];
        position += 1;
        let token = match byte {
            b if b.is_ascii_whitespace() => continue,
            b',' => Token::Comma,
            b'=' => Token::Equal,
            b';' => Token::Terminator,
            b'"' => {
                let end = bytes[position..].iter().position(|byte| *byte == b'"')? + position;
                let name = &sql[position..end];
                position = end + 1;
                Token::Quoted(name)
            }
            b if b == marker => {
                let digits = position;
                while bytes.get(position).is_some_and(u8::is_ascii_digit) {
                    position += 1;
                }
                if position == digits {
                    return None;
                }
                // Existing Model SQL validation owns index bounds and complete binding.
                Token::Bind
            }
            b if b.is_ascii_alphabetic() || b == b'_' => {
                while bytes
                    .get(position)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    position += 1;
                }
                Token::Word(&sql[start..position])
            }
            _ => return None,
        };
        tokens.push(token);
    }
    Some(tokens)
}

struct Projection<'a> {
    tokens: &'a [Token<'a>],
    position: usize,
    model: &'a ModelSchema,
}

impl Projection<'_> {
    fn take(&mut self, token: Token<'_>) -> bool {
        if self.tokens.get(self.position) != Some(&token) {
            return false;
        }
        self.position += 1;
        true
    }

    fn keyword(&mut self, keyword: &str) -> bool {
        if !matches!(self.tokens.get(self.position), Some(Token::Word(word)) if word.eq_ignore_ascii_case(keyword))
        {
            return false;
        }
        self.position += 1;
        true
    }

    fn identifier(&mut self, expected: &str) -> bool {
        if !self
            .tokens
            .get(self.position)
            .is_some_and(|token| token.identifier(expected))
        {
            return false;
        }
        self.position += 1;
        true
    }

    fn field(&mut self) -> bool {
        if !self.tokens.get(self.position).is_some_and(|token| {
            self.model
                .fields
                .iter()
                .any(|field| token.identifier(&field.name))
        }) {
            return false;
        }
        self.position += 1;
        true
    }

    fn fields(&mut self, ordered: bool) -> bool {
        loop {
            if !self.field() {
                return false;
            }
            if ordered && !self.keyword("ASC") {
                self.keyword("DESC");
            }
            if !self.take(Token::Comma) {
                return true;
            }
        }
    }
}
