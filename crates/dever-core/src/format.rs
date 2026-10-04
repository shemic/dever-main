//! Syntax-only formatting. Source token positions attach comments; the AST owns layout.

use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;
use crate::syntax::*;
use crate::token::{Kind, Token};

mod document;
mod markdown;

use document::{Document as Doc, join};

/// Formats syntactically valid source without requiring package or type resolution.
pub fn format(source: &SourceFile) -> Result<String, Vec<Diagnostic>> {
    let parsed = crate::parse_source(source)?;
    if let Some(blocks) = &parsed.blocks {
        return Ok(markdown::format(
            source,
            &parsed.package,
            &parsed.tokens,
            blocks,
        ));
    }
    Ok(format_fragment(
        source,
        parsed.tokens,
        &parsed.package.declarations,
    ))
}

fn format_fragment(
    source: &SourceFile,
    tokens: Vec<Token>,
    declarations: &[Declaration],
) -> String {
    let mut annotated: Vec<SourceToken> = Vec::new();
    let mut leading = Vec::new();
    for token in tokens {
        match token.kind {
            Kind::Newline => {}
            Kind::Comment(text) => {
                let comment = text.trim_end().to_owned();
                if let Some(previous) = annotated.last_mut().filter(|previous| {
                    source.position(previous.end).0 == source.position(token.span.start).0
                }) {
                    previous.trailing.push(comment);
                } else {
                    leading.push(comment);
                }
            }
            kind => annotated.push(SourceToken {
                kind,
                end: token.span.end,
                leading: std::mem::take(&mut leading),
                trailing: Vec::new(),
            }),
        }
    }
    let mut formatter = Formatter {
        tokens: annotated.into_iter().peekable(),
    };
    formatter.fragment(declarations).render()
}

struct SourceToken {
    kind: Kind,
    end: usize,
    leading: Vec<String>,
    trailing: Vec<String>,
}

struct Formatter {
    tokens: std::iter::Peekable<std::vec::IntoIter<SourceToken>>,
}

impl Formatter {
    fn token(&mut self, spelling: impl Into<String>) -> Doc {
        let token = self
            .tokens
            .next()
            .expect("AST follows parsed source tokens");
        let mut parts = Vec::new();
        for comment in token.leading {
            parts.extend([Doc::Line, Doc::Text(comment), Doc::Line]);
        }
        parts.push(Doc::Text(spelling.into()));
        for comment in token.trailing {
            parts.extend([Doc::space(), Doc::Text(comment), Doc::Line]);
        }
        Doc::sequence(parts)
    }

    fn at(&mut self, kind: Kind) -> bool {
        self.tokens.peek().is_some_and(|token| token.kind == kind)
    }

    fn path(&mut self, path: &Path) -> Doc {
        let mut parts = Vec::new();
        for (index, name) in path.iter().enumerate() {
            if index != 0 {
                parts.push(self.token("."));
            }
            parts.push(self.token(&name.text));
        }
        Doc::sequence(parts)
    }

    fn fragment(&mut self, declarations: &[Declaration]) -> Doc {
        let mut parts = Vec::new();
        for declaration in declarations {
            if !parts.is_empty() {
                parts.push(Doc::BlankLine);
            }
            parts.push(match declaration {
                Declaration::Api(binding) => {
                    let mut parts = Vec::new();
                    if binding.anonymous {
                        parts.extend([self.token("public"), Doc::space()]);
                    }
                    parts.extend(vec![
                        self.token(binding.kind.keyword()),
                        Doc::space(),
                        self.token(&binding.action.text),
                        Doc::space(),
                        self.token("="),
                        Doc::space(),
                        self.path(&binding.target),
                    ]);
                    Doc::sequence(parts)
                }
                Declaration::Rest(rest) => {
                    let mut parts = vec![self.token("rest")];
                    if let Some(model) = &rest.model {
                        parts.extend([Doc::space(), self.path(model)]);
                    }
                    Doc::sequence(parts)
                }
                Declaration::External(external) => {
                    let mut entries = external
                        .libs
                        .iter()
                        .map(|lib| {
                            Doc::sequence(vec![
                                self.token("lib"),
                                Doc::space(),
                                self.token(quoted_text(&lib.text)),
                            ])
                        })
                        .collect::<Vec<_>>();
                    entries.extend(external.capabilities.iter().map(|capability| {
                        Doc::sequence(vec![
                            self.token("allow"),
                            Doc::space(),
                            self.token(&capability.text),
                        ])
                    }));
                    Doc::sequence(vec![
                        self.token("external"),
                        Doc::space(),
                        self.token(external.ecosystem.keyword()),
                        Doc::space(),
                        self.token(quoted_text(&external.entry)),
                        Doc::space(),
                        self.lines("{", "}", &entries, |_, entry| entry.clone()),
                    ])
                }
                Declaration::Type(declaration) => self.type_declaration(declaration),
                Declaration::Function(function) => self.function(function),
                Declaration::Schedule(schedule) => Doc::sequence(vec![
                    self.token("schedule"),
                    Doc::space(),
                    self.token(&schedule.target.text),
                    Doc::space(),
                    self.token("="),
                    Doc::space(),
                    self.token(quoted_text(&schedule.cron)),
                ]),
                Declaration::Database(binding) => Doc::sequence(vec![
                    self.token("database"),
                    Doc::space(),
                    self.token(&binding.name.text),
                ]),
                Declaration::ModelIndex(index) => self.model_index(index),
                Declaration::Relation(relation) => Doc::sequence(vec![
                    self.token("relation"),
                    Doc::space(),
                    self.token(&relation.name.text),
                    Doc::space(),
                    self.token("="),
                    Doc::space(),
                    self.path(&relation.field),
                ]),
                Declaration::Seed(seed) => self.seed(seed),
                Declaration::Migration(migration) => self.migration(migration),
                Declaration::ModelSql(sql) => self.model_sql(sql),
            });
        }
        // End carries standalone comments after the last declaration, including comment-only tails.
        parts.push(Doc::Line);
        parts.push(self.token(""));
        debug_assert!(self.tokens.next().is_none());
        Doc::sequence(parts)
    }

    fn type_declaration(&mut self, declaration: &TypeDeclaration) -> Doc {
        let mut prefix = Vec::new();
        if declaration.public {
            prefix.extend([self.token("public"), Doc::space()]);
        }
        if declaration.setting {
            prefix.extend([self.token("setting"), Doc::space()]);
        } else {
            if declaration.global {
                prefix.extend([self.token("global"), Doc::space()]);
            }
            prefix.extend([
                self.token("type"),
                Doc::space(),
                self.token(&declaration.name.text),
                Doc::space(),
            ]);
        }
        let body = match &declaration.shape {
            TypeShape::Record(fields) => self.lines("{", "}", fields, Self::field),
            TypeShape::Choice(variants) => self.lines("{", "}", variants, |this, variant| {
                let mut parts = Vec::new();
                if variant.error {
                    parts.extend([this.token("error"), Doc::space()]);
                }
                let name = this.token(&variant.name.text);
                let payload = if this.at(Kind::LeftParen) {
                    this.positional("(", ")", &variant.payload, Self::field)
                } else {
                    Doc::sequence(Vec::new())
                };
                parts.extend([name, payload]);
                if let Some(label) = &variant.label {
                    parts.extend([
                        Doc::space(),
                        this.token("="),
                        Doc::space(),
                        this.token(quoted_text(label)),
                    ]);
                }
                Doc::sequence(parts)
            }),
        };
        Doc::sequence(vec![Doc::sequence(prefix), body])
    }

    fn field(&mut self, field: &Field) -> Doc {
        let mut parts = Vec::new();
        if field.private {
            parts.extend([self.token("private"), Doc::space()]);
        }
        if field.storage.owner.is_some() {
            parts.extend([self.token("owner"), Doc::space()]);
        }
        parts.extend([
            self.token(&field.name.text),
            self.token(":"),
            Doc::space(),
            self.type_ref(&field.ty),
        ]);
        parts.push(self.bounds(&field.bounds));
        while matches!(
            self.tokens.peek(),
            Some(token)
                if token.end <= field.span.end
                    && matches!(&token.kind, Kind::Name(name)
                        if matches!(name.as_str(), "generated" | "default" | "index" | "unique" | "from" | "create" | "replace" | "search"))
        ) {
            let Kind::Name(modifier) = self
                .tokens
                .peek()
                .map(|token| token.kind.clone())
                .expect("field modifier token")
            else {
                unreachable!()
            };
            parts.extend([Doc::space(), self.token(&modifier)]);
            match modifier.as_str() {
                "default" => parts.extend([
                    Doc::space(),
                    self.expression(field.storage.default.as_ref().expect("parsed default")),
                ]),
                "from" => parts.extend([
                    Doc::space(),
                    self.token(
                        &field
                            .storage
                            .from
                            .as_ref()
                            .expect("parsed rename source")
                            .text,
                    ),
                ]),
                "create" => parts.extend([
                    Doc::space(),
                    self.token("="),
                    Doc::space(),
                    self.expression(
                        field
                            .storage
                            .create
                            .as_ref()
                            .expect("parsed create binding"),
                    ),
                ]),
                "replace" => parts.extend([
                    Doc::space(),
                    self.token("="),
                    Doc::space(),
                    self.expression(
                        field
                            .storage
                            .replace
                            .as_ref()
                            .expect("parsed replace binding"),
                    ),
                ]),
                "search" => parts.extend([
                    Doc::space(),
                    self.token("="),
                    Doc::space(),
                    self.expression(
                        field
                            .storage
                            .search
                            .as_ref()
                            .expect("parsed search binding"),
                    ),
                ]),
                _ => {}
            }
        }
        if let Some(owner) = &field.storage.owner {
            parts.extend([
                Doc::space(),
                self.token("="),
                Doc::space(),
                self.expression(owner),
            ]);
        }
        Doc::sequence(parts)
    }

    fn type_ref(&mut self, ty: &TypeRef) -> Doc {
        let mut parts = vec![self.path(&ty.name)];
        // A following '<' can also begin a numeric pattern bound outside this type span.
        if self
            .tokens
            .peek()
            .is_some_and(|token| token.kind == Kind::Less && token.end <= ty.span.end)
        {
            parts.push(self.positional("<", ">", &ty.arguments, Self::type_ref));
        }
        if !ty.parameters.is_empty() {
            parts.push(
                self.positional("(", ")", &ty.parameters, |this, parameter| {
                    this.number(&parameter.value)
                }),
            );
        }
        if ty.nullable {
            parts.push(self.token("?"));
        }
        Doc::sequence(parts)
    }

    fn function(&mut self, function: &FunctionClause) -> Doc {
        let mut parts = Vec::new();
        if function.public {
            parts.extend([self.token("public"), Doc::space()]);
        }
        if function.kind == FunctionKind::Transaction {
            parts.extend([self.token("transaction"), Doc::space()]);
        }
        if matches!(function.kind, FunctionKind::Job { .. }) {
            parts.extend([self.token("job"), Doc::space()]);
        }
        for (index, segment) in function.name.text.split('.').enumerate() {
            if index != 0 {
                parts.push(self.token("."));
            }
            parts.push(self.token(segment));
        }
        parts.extend([
            self.positional("(", ")", &function.inputs, Self::input),
            Doc::space(),
            self.positional("(", ")", &function.outputs, Self::field),
        ]);
        // Consume source order before arranging contract documents canonically;
        if let FunctionKind::Job {
            attempts,
            timeout_ms,
        } = function.kind
        {
            for (name, value) in [("retry", attempts), ("timeout", timeout_ms)] {
                parts.extend([
                    Doc::space(),
                    self.token(name),
                    self.token("("),
                    self.token(value.to_string()),
                    self.token(")"),
                ]);
            }
        }
        // each marker keeps its attached comments when the order changes.
        let mut pure = None;
        let mut recovery = None;
        while self.at(Kind::Name("pure".into())) || self.at(Kind::Name("recover".into())) {
            if self.at(Kind::Name("pure".into())) {
                pure = Some(self.token("pure"));
            } else {
                recovery = Some(Doc::sequence(vec![
                    self.token("recover"),
                    self.token("("),
                    self.token(quoted_text(
                        function
                            .recovery
                            .as_ref()
                            .expect("parsed recovery contract"),
                    )),
                    self.token(")"),
                ]));
            }
        }
        for contract in [pure, recovery].into_iter().flatten() {
            parts.extend([Doc::space(), contract]);
        }
        if let Some(fails) = &function.fails {
            parts.extend([
                Doc::space(),
                self.token("fails"),
                Doc::space(),
                self.type_ref(fails),
            ]);
        }
        if function.bodyless {
            return Doc::sequence(parts);
        }
        parts.extend([
            Doc::space(),
            self.lines("{", "}", &function.body, Self::statement),
        ]);
        Doc::sequence(parts)
    }

    fn model_index(&mut self, index: &ModelIndex) -> Doc {
        Doc::sequence(vec![
            self.token(if index.unique { "unique" } else { "index" }),
            self.positional("(", ")", &index.fields, |this, field| {
                this.token(&field.text)
            }),
        ])
    }

    fn seed(&mut self, seed: &Seed) -> Doc {
        Doc::sequence(vec![
            self.token("seed"),
            Doc::space(),
            self.lines("{", "}", &seed.rows, |this, row| {
                this.lines("{", "}", row, |this, field| {
                    Doc::sequence(vec![
                        this.token(&field.name.text),
                        Doc::space(),
                        this.token("="),
                        Doc::space(),
                        this.expression(&field.value),
                    ])
                })
            }),
        ])
    }

    fn migration(&mut self, migration: &Migration) -> Doc {
        Doc::sequence(vec![
            self.token("migrate"),
            Doc::space(),
            self.token(&migration.name.text),
            Doc::space(),
            self.lines(
                "{",
                "}",
                &migration.operations,
                |this, operation| match operation {
                    MigrationOperation::Drop(field) => Doc::sequence(vec![
                        this.token("drop"),
                        Doc::space(),
                        this.token(&field.text),
                    ]),
                    MigrationOperation::Sql(sql) => this.migration_sql(sql),
                },
            ),
        ])
    }

    fn migration_sql(&mut self, sql: &MigrationSql) -> Doc {
        let phase = match sql.phase {
            MigrationPhase::Before => "before",
            MigrationPhase::After => "after",
        };
        Doc::sequence(vec![
            self.token(phase),
            Doc::space(),
            self.lines("{", "}", &[(), (), ()], |this, _| {
                let Kind::Name(field) = this.tokens.peek().expect("SQL field token").kind.clone()
                else {
                    unreachable!("parsed migration SQL field")
                };
                let prefix = vec![
                    this.token(&field),
                    Doc::space(),
                    this.token("="),
                    Doc::space(),
                ];
                let value = match field.as_str() {
                    "sqlite" => this.token(quoted_text(&sql.sqlite.value)),
                    "postgres" => this.token(quoted_text(&sql.postgres.value)),
                    "parameters" => this.positional("[", "]", &sql.parameters, Self::expression),
                    _ => unreachable!("parsed migration SQL field"),
                };
                Doc::sequence(vec![Doc::sequence(prefix), value])
            }),
        ])
    }

    fn model_sql(&mut self, sql: &ModelSql) -> Doc {
        Doc::sequence(vec![
            self.token("sql"),
            Doc::space(),
            self.token(&sql.name.text),
            self.positional("(", ")", &sql.inputs, Self::field),
            Doc::space(),
            self.positional("(", ")", &sql.outputs, Self::field),
            Doc::space(),
            self.lines(
                "{",
                "}",
                &[(&sql.sqlite, "sqlite"), (&sql.postgres, "postgres")],
                |this, (text, dialect)| {
                    Doc::sequence(vec![
                        this.token(*dialect),
                        Doc::space(),
                        this.token("="),
                        Doc::space(),
                        this.token(quoted_text(&text.value)),
                    ])
                },
            ),
        ])
    }

    fn input(&mut self, input: &Input) -> Doc {
        let mut parts = vec![self.token(&input.name.text), self.token(":"), Doc::space()];
        parts.push(match &input.kind {
            InputKind::Value(pattern) => self.pattern(pattern),
            InputKind::Handler(signature) => {
                let parts = vec![
                    self.token("handler"),
                    self.positional("(", ")", &signature.inputs, Self::field),
                    Doc::space(),
                    self.positional("(", ")", &signature.outputs, Self::field),
                ];
                Doc::sequence(parts)
            }
        });
        Doc::sequence(parts)
    }

    fn pattern(&mut self, pattern: &Pattern) -> Doc {
        match pattern {
            Pattern::Typed { ty, bounds } => {
                Doc::sequence(vec![self.type_ref(ty), self.bounds(bounds)])
            }
            Pattern::Variant {
                name,
                arguments,
                bindings,
                ..
            } => Doc::sequence(vec![
                self.path(name),
                if arguments.is_empty() {
                    Doc::sequence(Vec::new())
                } else {
                    self.positional("<", ">", arguments, Self::type_ref)
                },
                self.positional("(", ")", bindings, |this, name| this.token(&name.text)),
            ]),
            Pattern::Literal { value, .. } => self.literal(value),
            Pattern::Other(_) => self.token("other"),
        }
    }

    fn bounds(&mut self, bounds: &[Bound]) -> Doc {
        let mut parts = Vec::new();
        for (index, bound) in bounds.iter().enumerate() {
            if index != 0 {
                parts.extend([Doc::space(), self.token("and")]);
            }
            parts.extend([
                Doc::space(),
                self.token(operator_text(bound.comparison)),
                Doc::space(),
                self.number(&bound.number),
            ]);
        }
        Doc::sequence(parts)
    }

    fn statement(&mut self, statement: &Statement) -> Doc {
        match &statement.kind {
            StatementKind::Assign { target, value } => Doc::sequence(vec![
                self.path(target),
                Doc::space(),
                self.token("="),
                Doc::space(),
                self.expression(value),
            ]),
            StatementKind::Call(call) => self.expression(call),
        }
    }

    fn expression(&mut self, expression: &Expression) -> Doc {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.literal(literal),
            ExpressionKind::Name(path) => self.path(path),
            // Keeping explicit groups preserves associativity and gives comments a stable home.
            ExpressionKind::Group(value) => {
                self.positional("(", ")", &[value.as_ref()], |this, value| {
                    this.expression(value)
                })
            }
            ExpressionKind::Field { value, name } => Doc::sequence(vec![
                self.expression(value),
                self.token("."),
                self.token(&name.text),
            ]),
            ExpressionKind::Call {
                function,
                arguments,
            } => Doc::sequence(vec![
                self.expression(function),
                self.positional("(", ")", arguments, Self::expression),
            ]),
            ExpressionKind::Fail(error) => Doc::sequence(vec![
                self.token("fail"),
                self.positional("(", ")", &[error.as_ref()], |this, value| {
                    this.expression(value)
                }),
            ]),
            ExpressionKind::CaptureResult(call) => Doc::sequence(vec![
                self.token("result"),
                self.positional("(", ")", &[call.as_ref()], |this, value| {
                    this.expression(value)
                }),
            ]),
            ExpressionKind::Contextual(ContextualExpression::Channel { element, capacity }) => {
                Doc::sequence(vec![
                    self.token("channel"),
                    self.token("("),
                    self.type_ref(element),
                    self.token(","),
                    Doc::Break(" "),
                    self.expression(capacity),
                    self.token(")"),
                ])
                .grouped()
            }
            ExpressionKind::Record { name, fields } => Doc::sequence(vec![
                self.path(name),
                Doc::space(),
                self.lines("{", "}", fields, |this, field| {
                    Doc::sequence(vec![
                        this.token(&field.name.text),
                        Doc::space(),
                        this.token("="),
                        Doc::space(),
                        this.expression(&field.value),
                    ])
                }),
            ]),
            ExpressionKind::List(values) => self.positional("[", "]", values, Self::expression),
            ExpressionKind::Map(entries) => self.lines("{", "}", entries, |this, entry| {
                Doc::sequence(vec![
                    this.expression(&entry.key),
                    Doc::space(),
                    this.token("="),
                    Doc::space(),
                    this.expression(&entry.value),
                ])
            }),
            ExpressionKind::Unary { operator, value } => {
                let (spelling, space) = match operator {
                    UnaryOperator::Negate => ("-", ""),
                    UnaryOperator::Not => ("not", " "),
                };
                Doc::sequence(vec![
                    self.token(spelling),
                    Doc::Text(space.into()),
                    self.expression(value),
                ])
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => Doc::sequence(vec![
                self.expression(left),
                Doc::space(),
                self.token(operator_text(*operator)),
                Doc::space(),
                self.expression(right),
            ]),
        }
    }

    fn literal(&mut self, literal: &Literal) -> Doc {
        match literal {
            Literal::Bool(value) => self.token(if *value { "true" } else { "false" }),
            Literal::Null => self.token("null"),
            Literal::Number(number) => self.number(number),
            Literal::Text(text) => self.token(quoted_text(text)),
        }
    }

    fn number(&mut self, spelling: &str) -> Doc {
        if let Some(magnitude) = spelling.strip_prefix('-') {
            Doc::sequence(vec![self.token("-"), self.token(magnitude)])
        } else {
            self.token(spelling)
        }
    }

    fn positional<T>(
        &mut self,
        open: &str,
        close: &str,
        entries: &[T],
        format: impl Fn(&mut Self, &T) -> Doc,
    ) -> Doc {
        let opening = self.token(open);
        let mut members = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if index != 0 {
                members.extend([self.token(","), Doc::Break(" ")]);
            }
            members.push(format(self, entry));
        }
        Doc::sequence(vec![
            opening,
            Doc::sequence(vec![Doc::Break(""), Doc::sequence(members)]).indented(),
            Doc::Break(""),
            self.token(close),
        ])
        .grouped()
    }

    fn lines<T>(
        &mut self,
        open: &str,
        close: &str,
        entries: &[T],
        format: impl Fn(&mut Self, &T) -> Doc,
    ) -> Doc {
        let opening = self.token(open);
        let members = entries.iter().map(|entry| format(self, entry)).collect();
        let closing = self.token(close);
        if entries.is_empty() {
            return Doc::sequence(vec![opening, closing]);
        }
        Doc::sequence(vec![
            opening,
            Doc::sequence(vec![Doc::Line, join(members, Doc::Line)]).indented(),
            Doc::Line,
            closing,
        ])
    }
}

fn quoted_text(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            control if control.is_control() => {
                use std::fmt::Write;
                write!(quoted, "\\u{{{:x}}}", u32::from(control))
                    .expect("writing a Text literal to a String");
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

fn operator_text(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::IntegerDivide => "//",
        BinaryOperator::Remainder => "%",
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "!=",
        BinaryOperator::Less => "<",
        BinaryOperator::LessEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterEqual => ">=",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
    }
}
